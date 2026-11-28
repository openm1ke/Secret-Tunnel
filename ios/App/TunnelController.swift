import Foundation
import NetworkExtension

/// Owns the NETunnelProviderManager for our Packet Tunnel extension and polls its status.
@MainActor
final class TunnelController: ObservableObject {
    @Published var settings = Shared.loadSettings()
    @Published var hasKey = Shared.hasKey
    @Published var vpnStatus: NEVPNStatus = .invalid
    @Published var message = ""
    @Published var busy = false
    @Published var pendingFingerprint = ""
    @Published var rx: Int64 = 0
    @Published var tx: Int64 = 0
    @Published var logs: [String] = []
    @Published var tunnelError = ""
    @Published private(set) var vpnError = ""
    @Published private(set) var startingVPN = false
    @Published private(set) var traffic = [TrafficSample]()
    @Published private(set) var currentTraffic: TrafficSample?

    private var manager: NETunnelProviderManager?
    private var statusObserver: NSObjectProtocol?
    private var pollTimer: Timer?
    private var lastCounters: (time: TimeInterval, rx: Int64, tx: Int64)?

    private var tunnelBundleID: String {
        Bundle.main.object(forInfoDictionaryKey: "STTunnelBundleID") as? String ?? "dev.secrettunnel.ios.PacketTunnel"
    }

    init() {
        #if DEBUG
        importLocalDeviceProfiles()
        #endif
        #if !targetEnvironment(simulator)
        Task {
            await loadManager()
            #if DEBUG
            if ProcessInfo.processInfo.arguments.contains("--check-saved-servers") { await runLocalDeviceChecks() }
            if ProcessInfo.processInfo.arguments.contains("--ipv6-device-checks") { await runIPv6DeviceChecks() }
            #endif
        }
        #endif
    }

    var simulator: Bool {
        #if targetEnvironment(simulator)
        true
        #else
        false
        #endif
    }

    var isConnected: Bool { vpnStatus == .connected }
    var isActive: Bool { [.connecting, .connected, .reasserting].contains(vpnStatus) }
    var canEditServer: Bool { !busy && !startingVPN && !isActive && vpnStatus != .disconnecting }
    var hasEndpoint: Bool {
        !settings.host.trimmingCharacters(in: .whitespaces).isEmpty &&
        !settings.user.trimmingCharacters(in: .whitespaces).isEmpty &&
        (1...65535).contains(settings.port)
    }
    var isConfigured: Bool { hasEndpoint && hasKey && !settings.hostKeySHA256.isEmpty }
    var connectionError: String { vpnError.isEmpty ? tunnelError : vpnError }

    func selectServer(_ id: String) {
        guard canEditServer else { return }
        do {
            try Shared.selectProfile(id)
            settings = Shared.loadSettings()
            hasKey = Shared.hasKey
            pendingFingerprint = ""
            vpnError = ""; tunnelError = ""; message = ""
        } catch { message = error.localizedDescription }
    }

    func editServer(host: String, port: Int, user: String, jumpID: String? = nil) -> Bool {
        guard canEditServer else { return false }
        let host = host.trimmingCharacters(in: .whitespaces)
        let user = user.trimmingCharacters(in: .whitespaces)
        guard !host.isEmpty, !user.isEmpty, (1...65535).contains(port) else {
            message = tr("Укажите адрес, пользователя и порт от 1 до 65535.")
            return false
        }
        var updated = settings
        if updated.host != host || updated.port != port {
            updated.hostKeySHA256 = ""
            pendingFingerprint = ""
        }
        updated.host = host
        updated.port = port
        updated.user = user
        updated.jumpProfileID = jumpID
        guard saveSettings(updated) else { return false }
        if updated != settings {
            settings = updated
            vpnError = ""
            message = ""
        }
        return true
    }

    private func saveSettings(_ value: TunnelSettings) -> Bool {
        do {
            try Shared.save(value)
            return true
        } catch {
            message = error.localizedDescription
            return false
        }
    }

    func setSocksPort(_ port: Int) -> Bool {
        guard canEditServer, (1024...65535).contains(port) else { return false }
        var updated = settings
        updated.socksPort = port
        guard saveSettings(updated) else { return false }
        settings = updated
        return true
    }

    func importKey(_ url: URL) {
        guard canEditServer else { return }
        let scoped = url.startAccessingSecurityScopedResource()
        defer { if scoped { url.stopAccessingSecurityScopedResource() } }
        do {
            try Shared.storeKey(from: url)
            hasKey = true
            message = tr("Ключ импортирован")
        } catch {
            message = error.localizedDescription
        }
    }

    /// Fetches the server host key so the user can compare it before trusting.
    func probeServer() {
        guard canEditServer, hasEndpoint else { return }
        pendingFingerprint = ""
        guard let config = checkedRuntimeConfig() else { return }
        busy = true
        message = tr("Получаем ключ сервера…")
        Task.detached {
            let result = jsonObject(RustCore.probe(config))
            await MainActor.run {
                self.busy = false
                if let pin = result["fingerprint"] as? String {
                    self.pendingFingerprint = pin
                    self.message = tr("Сверьте отпечаток с сервером и подтвердите")
                } else {
                    self.message = result["error"] as? String ?? tr("Не удалось получить ключ")
                }
            }
        }
    }

    func trustFingerprint() {
        guard canEditServer, !pendingFingerprint.isEmpty else { return }
        var confirmed = settings
        confirmed.hostKeySHA256 = pendingFingerprint
        guard saveSettings(confirmed) else { return }
        settings = confirmed
        pendingFingerprint = ""
        message = tr("Отпечаток сохранён")
    }

    /// Logs in from the app (not through the VPN) and checks the server agent.
    func checkServer() {
        guard isConfigured, !busy else { return }
        guard let config = checkedRuntimeConfig() else { return }
        busy = true
        message = tr("Проверяем вход по ключу и агент…")
        Task.detached {
            let result = jsonObject(RustCore.check(config))
            await MainActor.run {
                self.busy = false
                if result["ok"] as? Bool == true {
                    self.message = result["summary"] as? String ?? tr("Сервер готов")
                } else {
                    self.message = result["error"] as? String ?? tr("Проверка не прошла")
                }
            }
        }
    }

    func toggle() {
        if isActive { manager?.connection.stopVPNTunnel() } else { Task { await connect() } }
    }

    private func checkedRuntimeConfig() -> String? {
        do { return try settings.rustConfig(keyPath: Shared.keyURL.path) }
        catch { message = error.localizedDescription; return nil }
    }

    private func connect() async {
        guard !startingVPN, !busy, vpnStatus != .disconnecting else { return }
        guard isConfigured else {
            message = tr("Импортируйте ключ и подтвердите отпечаток сервера")
            return
        }
        #if targetEnvironment(simulator)
        vpnError = tr("VPN доступен на iPhone и iPad. В симуляторе можно проверить SSH и агент.")
        #else
        guard Shared.groupContainer != nil else {
            vpnError = tr("Недоступна общая папка VPN. Проверьте App Group и подпись сборки.")
            return
        }
        vpnError = ""
        tunnelError = ""
        startingVPN = true
        defer { startingVPN = false }
        do {
            let manager = try await preparedManager()
            try manager.connection.startVPNTunnel()
            statusChanged()
        } catch {
            vpnError = error.localizedDescription
        }
        #endif
    }

    private func loadManager() async {
        let managers = (try? await NETunnelProviderManager.loadAllFromPreferences()) ?? []
        manager = managers.first { ($0.protocolConfiguration as? NETunnelProviderProtocol)?.providerBundleIdentifier == tunnelBundleID }
        observe()
    }

    private func preparedManager() async throws -> NETunnelProviderManager {
        let manager = self.manager ?? NETunnelProviderManager()
        let proto = NETunnelProviderProtocol()
        proto.providerBundleIdentifier = tunnelBundleID
        proto.serverAddress = TunnelNetworkPolicy.entryHost(target: settings.host, jumps: try Shared.sshJumps(for: settings))
        proto.disconnectOnSleep = false
        TunnelNetworkPolicy.configure(proto)
        manager.protocolConfiguration = proto
        manager.localizedDescription = "Secret Tunnel"
        manager.isEnabled = true
        do {
            try await manager.saveToPreferences()
            // Reload so the system-backed connection object is current after the first save.
            try await manager.loadFromPreferences()
        } catch {
            throw VPNProfileError(underlying: error)
        }
        self.manager = manager
        observe()
        return manager
    }

    private func observe() {
        if let statusObserver { NotificationCenter.default.removeObserver(statusObserver) }
        guard let manager else { return }
        vpnStatus = manager.connection.status
        statusObserver = NotificationCenter.default.addObserver(
            forName: .NEVPNStatusDidChange, object: manager.connection, queue: .main
        ) { [weak self] _ in
            Task { @MainActor in self?.statusChanged() }
        }
        statusChanged()
    }

    private func statusChanged() {
        guard let manager else { return }
        vpnStatus = manager.connection.status
        pollTimer?.invalidate()
        if isActive {
            pollTimer = Timer.scheduledTimer(withTimeInterval: 1.5, repeats: true) { [weak self] _ in
                Task { @MainActor in self?.poll() }
            }
            poll()
        } else {
            currentTraffic = nil
            lastCounters = nil
            if vpnStatus == .disconnected, #available(iOS 16.0, *) {
                manager.connection.fetchLastDisconnectError { error in
                    guard let error else { return }
                    Task { @MainActor in
                        if self.vpnStatus == .disconnected {
                            self.vpnError = error.localizedDescription
                        }
                    }
                }
            }
        }
    }

    private func poll() {
        guard let session = manager?.connection as? NETunnelProviderSession else { return }
        try? session.sendProviderMessage(Data("status".utf8)) { data in
            guard let data, let value = try? JSONSerialization.jsonObject(with: data) as? [String: Any] else { return }
            Task { @MainActor in
                guard self.isActive else { return }
                self.sampleTraffic(
                    rx: (value["rx"] as? NSNumber)?.int64Value ?? 0,
                    tx: (value["tx"] as? NSNumber)?.int64Value ?? 0
                )
                self.logs = value["logs"] as? [String] ?? []
                self.tunnelError = value["error"] as? String ?? ""
            }
        }
    }

    private func sampleTraffic(rx: Int64, tx: Int64) {
        let now = ProcessInfo.processInfo.systemUptime
        let previous = lastCounters
        lastCounters = (now, rx, tx)
        self.rx = rx
        self.tx = tx
        guard let previous else {
            traffic.removeAll()
            return
        }
        let elapsed = now - previous.time
        guard elapsed > 0, rx >= previous.rx, tx >= previous.tx else { return }
        let sample = TrafficSample(
            time: now,
            down: Double(rx - previous.rx) / elapsed,
            up: Double(tx - previous.tx) / elapsed
        )
        currentTraffic = sample
        traffic.removeAll { $0.time < now - 60 }
        traffic.append(sample)
    }
}

private struct VPNProfileError: LocalizedError {
    let underlying: Error
    var errorDescription: String? {
        tr("Не удалось настроить профиль VPN. {0}", underlying.localizedDescription)
    }
}

struct TrafficSample {
    let time: TimeInterval
    let down: Double
    let up: Double
}
