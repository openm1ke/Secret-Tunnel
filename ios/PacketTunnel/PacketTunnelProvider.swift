import NetworkExtension
import os.log

/// Device-wide tunnel: utun → hev-socks5-tunnel → Rust SOCKS5 (127.0.0.1) → SSH → server agent.
/// Sockets opened by this extension (the SSH connection) bypass its own tunnel.
final class PacketTunnelProvider: NEPacketTunnelProvider {
    private let log = OSLog(subsystem: "dev.secrettunnel.ios", category: "tunnel")
    private var hevThread: Thread?

    override func startTunnel(options: [String: NSObject]?, completionHandler: @escaping (Error?) -> Void) {
        let settings = Shared.loadSettings()
        guard Shared.hasKey else {
            return completionHandler(fail("Импортируйте приватный SSH-ключ в приложении"))
        }
        DispatchQueue.global(qos: .userInitiated).async { [self] in
            let error = RustCore.start(settings.rustConfig(keyPath: Shared.keyURL.path, newSession: true))
            if !error.isEmpty {
                os_log("SSH start failed: %{public}@", log: log, type: .error, error)
                return completionHandler(fail(error))
            }
            setTunnelNetworkSettings(networkSettings(for: settings)) { [self] error in
                if let error {
                    RustCore.stop()
                    return completionHandler(error)
                }
                let fd = st_find_utun_fd()
                guard fd >= 0 else {
                    RustCore.stop()
                    return completionHandler(fail("Не найден дескриптор utun"))
                }
                startHev(fd: fd, socksPort: settings.socksPort)
                os_log("Tunnel started, utun fd %d", log: log, fd)
                completionHandler(nil)
                startDiagnostics(socksPort: settings.socksPort)
            }
        }
    }

    override func stopTunnel(with reason: NEProviderStopReason, completionHandler: @escaping () -> Void) {
        os_log("Stopping tunnel, reason %d", log: log, reason.rawValue)
        diagnosticsTimer?.cancel()
        diagnosticsTimer = nil
        hev_socks5_tunnel_quit()
        RustCore.stop()
        completionHandler()
    }

    /// The app polls status (traffic totals, errors, log tail) through provider messages.
    override func handleAppMessage(_ messageData: Data, completionHandler: ((Data?) -> Void)?) {
        var status = jsonObject(RustCore.status())
        var tx = 0, txBytes = 0, rx = 0, rxBytes = 0
        hev_socks5_tunnel_stats(&tx, &txBytes, &rx, &rxBytes)
        status["tun_tx_bytes"] = txBytes
        status["tun_rx_bytes"] = rxBytes
        completionHandler?(try? JSONSerialization.data(withJSONObject: status))
    }

    private func networkSettings(for settings: TunnelSettings) -> NEPacketTunnelNetworkSettings {
        let network = NEPacketTunnelNetworkSettings(tunnelRemoteAddress: settings.host)
        let ipv4 = NEIPv4Settings(addresses: ["198.18.0.1"], subnetMasks: ["255.255.255.252"])
        ipv4.includedRoutes = [NEIPv4Route.default()]
        network.ipv4Settings = ipv4
        // IPv4-only tunnel: the server has no IPv6 egress, and advertising IPv6 here made
        // apps try IPv6 first and fail. Note: if the Wi-Fi itself provides IPv6, that
        // traffic is not captured by this tunnel.
        let dns = NEDNSSettings(servers: ["1.1.1.1"])
        dns.matchDomains = [""]
        network.dnsSettings = dns
        network.mtu = 1500
        return network
    }

    private func startHev(fd: Int32, socksPort: Int) {
        try? Data().write(to: Shared.hevLogURL)
        let config = """
        tunnel:
          mtu: 1500
          ipv4: 198.18.0.1
        socks5:
          address: '::1'
          port: \(socksPort)
          udp: 'udp'
        misc:
          task-stack-size: 65536
          tcp-buffer-size: 65536
          max-session-count: 1024
          connect-timeout: 14000
          tcp-read-write-timeout: 60000
          udp-read-write-timeout: 30000
          log-file: '\(Shared.hevLogURL.path)'
          log-level: info
        """
        let thread = Thread { [log] in
            let bytes = Array(config.utf8)
            let code = bytes.withUnsafeBufferPointer {
                hev_socks5_tunnel_main_from_str($0.baseAddress, UInt32($0.count), fd)
            }
            os_log("hev-socks5-tunnel exited with %d", log: log, code)
        }
        thread.name = "hev-socks5-tunnel"
        thread.stackSize = 1 << 20
        hevThread = thread
        thread.start()
    }

    // MARK: - Diagnostics (readable with `idevicesyslog | grep PacketTunnel`)

    private var diagnosticsTimer: DispatchSourceTimer?
    private var hevLogOffset: UInt64 = 0
    private var rustLogCount = 0

    private func startDiagnostics(socksPort: Int) {
        DispatchQueue.global(qos: .utility).async { [self] in
            os_log("Self-test via SOCKS: %{public}@", log: log, socksSelfTest(port: socksPort))
        }
        let timer = DispatchSource.makeTimerSource(queue: .global(qos: .utility))
        timer.schedule(deadline: .now() + 2, repeating: 5)
        timer.setEventHandler { [weak self] in self?.logDiagnostics() }
        diagnosticsTimer = timer
        timer.resume()
    }

    private func logDiagnostics() {
        var txPackets = 0, txBytes = 0, rxPackets = 0, rxBytes = 0
        hev_socks5_tunnel_stats(&txPackets, &txBytes, &rxPackets, &rxBytes)
        let status = jsonObject(RustCore.status())
        os_log("TUN stats: tx %d pkts/%d B, rx %d pkts/%d B; SSH rx %{public}@ tx %{public}@ running %{public}@ error %{public}@; memory %.1f MB",
               log: log, txPackets, txBytes, rxPackets, rxBytes,
               "\(status["rx"] ?? 0)", "\(status["tx"] ?? 0)", "\(status["running"] ?? false)", "\(status["error"] ?? "")",
               Self.footprintMB())
        // New Rust log lines other than periodic heartbeat/TCP samples.
        let lines = status["logs"] as? [String] ?? []
        if lines.count < rustLogCount { rustLogCount = 0 }
        for line in lines.dropFirst(rustLogCount) where !line.contains("SSH heartbeat") && !line.contains("SSH TCP") {
            os_log("Rust: %{public}@", log: log, String(line.prefix(400)))
        }
        rustLogCount = lines.count
        // New hev log output.
        let hevLogSize = (try? FileManager.default.attributesOfItem(atPath: Shared.hevLogURL.path)[.size] as? NSNumber)?.intValue ?? -1
        os_log("hev log %{public}@ size %d", log: log, Shared.hevLogURL.path, hevLogSize)
        if let handle = try? FileHandle(forReadingFrom: Shared.hevLogURL) {
            defer { try? handle.close() }
            try? handle.seek(toOffset: hevLogOffset)
            let data = handle.readDataToEndOfFile()
            hevLogOffset += UInt64(data.count)
            for line in String(decoding: data, as: UTF8.self).split(separator: "\n").suffix(20) {
                os_log("hev: %{public}@", log: log, String(line.prefix(400)))
            }
        }
    }

    /// Physical memory footprint (what iOS compares against the extension memory limit).
    private static func footprintMB() -> Double {
        var info = task_vm_info_data_t()
        var count = mach_msg_type_number_t(MemoryLayout<task_vm_info_data_t>.size / MemoryLayout<natural_t>.size)
        let result = withUnsafeMutablePointer(to: &info) {
            $0.withMemoryRebound(to: integer_t.self, capacity: Int(count)) {
                task_info(mach_task_self_, task_flavor_t(TASK_VM_INFO), $0, &count)
            }
        }
        return result == KERN_SUCCESS ? Double(info.phys_footprint) / 1_048_576 : -1
    }

    /// HTTP HEAD to 1.1.1.1:80 through our SOCKS5 server on [::1], bypassing utun/hev.
    private func socksSelfTest(port: Int) -> String {
        let fd = socket(AF_INET6, SOCK_STREAM, 0)
        guard fd >= 0 else { return "socket() failed \(errno)" }
        defer { close(fd) }
        var timeout = timeval(tv_sec: 10, tv_usec: 0)
        setsockopt(fd, SOL_SOCKET, SO_RCVTIMEO, &timeout, socklen_t(MemoryLayout<timeval>.size))
        var addr = sockaddr_in6()
        addr.sin6_len = UInt8(MemoryLayout<sockaddr_in6>.size)
        addr.sin6_family = sa_family_t(AF_INET6)
        addr.sin6_port = in_port_t(UInt16(port).bigEndian)
        addr.sin6_addr = in6addr_loopback
        let connected = withUnsafePointer(to: &addr) {
            $0.withMemoryRebound(to: sockaddr.self, capacity: 1) {
                connect(fd, $0, socklen_t(MemoryLayout<sockaddr_in6>.size))
            }
        }
        guard connected == 0 else { return "connect to SOCKS failed errno \(errno)" }
        func send(_ bytes: [UInt8]) -> Bool { bytes.withUnsafeBytes { write(fd, $0.baseAddress, $0.count) } == bytes.count }
        func recv(_ count: Int) -> [UInt8] {
            var buffer = [UInt8](repeating: 0, count: count)
            let n = buffer.withUnsafeMutableBytes { read(fd, $0.baseAddress, count) }
            return n > 0 ? Array(buffer.prefix(n)) : []
        }
        guard send([5, 1, 0]), recv(2) == [5, 0] else { return "SOCKS greeting failed" }
        guard send([5, 1, 0, 1, 1, 1, 1, 1, 0, 80]) else { return "SOCKS connect send failed" }
        let reply = recv(10)
        guard reply.count >= 2, reply[1] == 0 else { return "SOCKS CONNECT rejected: \(reply)" }
        guard send(Array("HEAD / HTTP/1.1\r\nHost: 1.1.1.1\r\nConnection: close\r\n\r\n".utf8)) else {
            return "HTTP send failed"
        }
        let response = recv(256)
        guard !response.isEmpty else { return "no HTTP response (errno \(errno))" }
        return "OK: " + (String(decoding: response, as: UTF8.self).split(separator: "\r\n").first.map(String.init) ?? "")
    }

    private func fail(_ message: String) -> NSError {
        NSError(domain: "SecretTunnel", code: 1, userInfo: [NSLocalizedDescriptionKey: message])
    }
}
