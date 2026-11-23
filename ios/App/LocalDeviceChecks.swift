#if DEBUG
import Foundation
import NetworkExtension

/// A USB test build imports a one-time file from its own private container.
/// Credentials are never compiled into the app or included in the IPA.
extension TunnelController {
    func importLocalDeviceProfiles() {
        guard ProcessInfo.processInfo.arguments.contains("--import-profiles") else { return }
        struct Import: Decodable {
            struct Entry: Decodable { let name: String; let settings: TunnelSettings; let privateKey: Data }
            let profiles: [Entry]
        }
        let documents = FileManager.default.urls(for: .documentDirectory, in: .userDomainMask)[0]
        let source = documents.appendingPathComponent("local-test-profiles.json")
        do {
            let size = try source.resourceValues(forKeys: [.fileSizeKey]).fileSize ?? 0
            guard size > 0, size <= 4 * 1024 * 1024 else { throw SetupFailure("Некорректный размер файла профилей.") }
            let imported = try JSONDecoder().decode(Import.self, from: Data(contentsOf: source))
            guard !imported.profiles.isEmpty, imported.profiles.count <= 8 else { throw SetupFailure("Некорректное число профилей.") }
            for entry in imported.profiles {
                guard !entry.settings.host.isEmpty, !entry.settings.user.isEmpty, (1...65535).contains(entry.settings.port),
                      entry.settings.hostKeySHA256.hasPrefix("SHA256:"), entry.settings.hostKeySHA256.count == 50,
                      entry.privateKey.count <= 1_048_576, String(decoding: entry.privateKey, as: UTF8.self).contains("PRIVATE KEY") else {
                    throw SetupFailure("Проверьте данные импортируемого сервера.")
                }
            }
            for entry in imported.profiles {
                try Shared.saveSetup(entry.settings, key: entry.privateKey, name: entry.name)
            }
            try FileManager.default.removeItem(at: source)
            settings = Shared.loadSettings(); hasKey = Shared.hasKey
            message = "Серверы добавлены"
            NSLog("ST_DEVICE_QA imported %d profiles", imported.profiles.count)
        } catch {
            message = error.localizedDescription
            NSLog("ST_DEVICE_QA import failed")
        }
    }

    func runLocalDeviceChecks() async {
        var reports = [[String: Any]]()
        // Preserve the imported order: the last server remains selected after the test.
        for profile in Shared.profiles.filter({ ["RU", "France"].contains($0.name) }) {
            if isActive { toggle() }
            let stopped = Date().addingTimeInterval(15)
            while isActive || vpnStatus == .disconnecting, Date() < stopped {
                try? await Task.sleep(nanoseconds: 300_000_000)
            }
            selectServer(profile.id)
            guard let config = try? profile.settings.rustConfig(keyPath: Shared.keyURL(for: profile).path) else { continue }
            let checked = jsonObject(await Task.detached { RustCore.check(config) }.value)
            var report: [String: Any] = ["server": profile.name, "ssh_agent_ok": checked["ok"] as? Bool == true]
            NSLog("ST_DEVICE_QA %@ SSH/agent %@", profile.name, checked["ok"] as? Bool == true ? "OK" : "FAILED")
            if checked["ok"] as? Bool == true {
                toggle()
                let deadline = Date().addingTimeInterval(40)
                while !isConnected, Date() < deadline {
                    try? await Task.sleep(nanoseconds: 500_000_000)
                }
                report["vpn_connected"] = isConnected
                if isConnected {
                    let session = URLSession(configuration: .ephemeral)
                    defer { session.invalidateAndCancel() }
                    do {
                        var request = URLRequest(url: URL(string: "https://api.ipify.org")!)
                        request.timeoutInterval = 20
                        let (data, response) = try await session.data(for: request)
                        report["https_ok"] = (response as? HTTPURLResponse)?.statusCode == 200
                        report["egress"] = String(decoding: data.prefix(64), as: UTF8.self).trimmingCharacters(in: .whitespacesAndNewlines)
                    } catch { report["https_ok"] = false }
                }
            }
            reports.append(report)
            let destination = FileManager.default.urls(for: .documentDirectory, in: .userDomainMask)[0].appendingPathComponent("local-device-checks.json")
            try? JSONSerialization.data(withJSONObject: reports).write(to: destination, options: [.atomic, .completeFileProtectionUntilFirstUserAuthentication])
            NSLog("ST_DEVICE_QA %@ VPN %@ HTTPS %@", profile.name, report["vpn_connected"] as? Bool == true ? "OK" : "FAILED", report["https_ok"] as? Bool == true ? "OK" : "FAILED")
        }
    }
}
#endif
