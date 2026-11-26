#if DEBUG
import Foundation
import Network
import UIKit

/// Opt-in USB fixture, never a production startup path. Ordinary and explicitly
/// Wi-Fi-scoped sockets exercise OS routing rather than connecting to SOCKS.
extension TunnelController {
    func runIPv6DeviceChecks() async {
        let documents = FileManager.default.urls(for: .documentDirectory, in: .userDomainMask)[0]
        let output = documents.appendingPathComponent("ipv6-device-checks.json")
        var report = [String: Any]()
        func save() {
            try? JSONSerialization.data(withJSONObject: report).write(to: output, options: [.atomic, .completeFileProtectionUntilFirstUserAuthentication])
        }
        do {
            // USB debug launch can leave the app in the background, where iOS
            // denies local-network access before showing its consent prompt.
            let foregroundDeadline = Date().addingTimeInterval(90)
            while UIApplication.shared.applicationState != .active && Date() < foregroundDeadline {
                try? await Task.sleep(nanoseconds: 300_000_000)
            }
            guard UIApplication.shared.applicationState == .active else { throw SetupFailure("Bring the QA app to the foreground") }
            let input = try Data(contentsOf: documents.appendingPathComponent("ipv6-device-fixture.json"))
            let fixture = try JSONSerialization.jsonObject(with: input) as? [String: Any] ?? [:]
            guard let physicalHost = fixture["physical_host"] as? String,
                  let portNumber = fixture["physical_port"] as? Int,
                  let physicalPort = UInt16(exactly: portNumber), physicalPort != 0,
                  let expected = fixture["expect_ipv6"] as? Bool else { throw SetupFailure("Missing IPv6 device fixture") }
            let chain = ProcessInfo.processInfo.arguments.contains("--ipv6-chain") || fixture["chain"] as? Bool == true
            let name = chain ? "IPv6 QA Chain" : "IPv6 QA"
            guard let selected = Shared.profiles.first(where: { $0.name == name }) else { throw SetupFailure("Import disposable IPv6 QA profiles first") }
            selectServer(selected.id)
            if chain {
                guard let relay = Shared.profiles.first(where: { $0.name == "IPv6 QA Relay" }) else { throw SetupFailure("Missing QA relay") }
                var next = settings; next.jumpProfileID = relay.id
                try Shared.save(next); settings = next
            }
            report["chain"] = chain
            try? await Task.sleep(nanoseconds: 500_000_000)
            if let host4 = fixture["physical_host4"] as? String {
                report["wifi_control"] = await IPv6FixtureSocket.exchange(host: host4, port: physicalPort, udp: false, wifi: true)
            }
            let control = await IPv6FixtureSocket.outcome(host: physicalHost, port: physicalPort, udp: false, wifi: true)
            report["physical_control"] = control.success
            report["physical_control_detail"] = control.detail
            save()
            toggle()
            let deadline = Date().addingTimeInterval(75)
            while !isConnected && Date() < deadline {
                try? await Task.sleep(nanoseconds: 300_000_000)
            }
            report["vpn_connected"] = isConnected
            save()
            if isConnected {
                report["tcp4"] = await IPv6FixtureSocket.exchange(host: "203.0.113.10", port: 27150, udp: false)
                report["udp4"] = await IPv6FixtureSocket.exchange(host: "203.0.113.10", port: 27150, udp: true)
                report["tcp6"] = await IPv6FixtureSocket.exchange(host: "2001:db8::10", port: 27150, udp: false)
                report["udp6"] = await IPv6FixtureSocket.exchange(host: "2001:db8::10", port: 27150, udp: true)
                report["scoped_ipv6_escaped"] = await IPv6FixtureSocket.exchange(host: physicalHost, port: physicalPort, udp: false, wifi: true)
            }
            report["passed"] = isConnected && report["physical_control"] as? Bool == true &&
                report["tcp4"] as? Bool == true && report["udp4"] as? Bool == true &&
                report["tcp6"] as? Bool == expected && report["udp6"] as? Bool == expected &&
                report["scoped_ipv6_escaped"] as? Bool == false
        } catch { report["passed"] = false; report["error"] = error.localizedDescription }
        if isActive {
            toggle()
            let deadline = Date().addingTimeInterval(15)
            while isActive && Date() < deadline { try? await Task.sleep(nanoseconds: 200_000_000) }
        }
        report["vpn_stopped"] = !isActive
        report["finished"] = true
        save()
        NSLog("ST_IPV6_DEVICE_QA finished passed=%@", report["passed"] as? Bool == true ? "true" : "false")
    }
}

private enum IPv6FixtureSocket {
    struct Result { let success: Bool; let detail: String }
    // Accessed only on one connection queue, including the deadline callback.
    private final class State: @unchecked Sendable {
        var complete = false
        var detail = "deadline"
    }
    static func exchange(host: String, port: UInt16, udp: Bool, wifi: Bool = false) async -> Bool {
        await outcome(host: host, port: port, udp: udp, wifi: wifi).success
    }

    static func outcome(host: String, port: UInt16, udp: Bool, wifi: Bool = false) async -> Result {
        await withCheckedContinuation { continuation in
            let parameters: NWParameters = udp ? .udp : .tcp
            if wifi { parameters.requiredInterfaceType = .wifi }
            let connection = NWConnection(host: NWEndpoint.Host(host), port: NWEndpoint.Port(rawValue: port)!, using: parameters)
            let queue = DispatchQueue(label: "dev.secrettunnel.ipv6-fixture")
            let payload = Data("tunnel-only-fixture".utf8)
            // All callbacks, including the deadline, run on this one queue.
            let state = State()
            func finish(_ success: Bool) {
                guard !state.complete else { return }
                state.complete = true
                connection.stateUpdateHandler = nil
                connection.cancel()
                continuation.resume(returning: Result(success: success, detail: state.detail))
            }
            queue.asyncAfter(deadline: .now() + 5) { finish(false) }
            connection.stateUpdateHandler = { connectionState in
                state.detail = String(describing: connectionState)
                switch connectionState {
                case .failed, .cancelled: finish(false)
                case .ready:
                    connection.send(content: payload, completion: .contentProcessed { error in
                        if error != nil { finish(false); return }
                        if udp {
                            connection.receiveMessage { data, _, _, error in finish(error == nil && data == payload) }
                        } else {
                            connection.receive(minimumIncompleteLength: payload.count, maximumLength: payload.count) { data, _, _, error in
                                finish(error == nil && data == payload)
                            }
                        }
                    })
                default: break
                }
            }
            connection.start(queue: queue)
        }
    }
}
#endif
