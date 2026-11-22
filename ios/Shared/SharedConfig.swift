import Foundation

/// Settings shared by the app and the Packet Tunnel extension through the App Group.
struct TunnelSettings: Codable, Equatable {
    var host = ""
    var port = 22
    var user = ""
    /// SHA256 host key fingerprint confirmed by the user.
    var hostKeySHA256 = ""
    var socksPort = 10808

    /// JSON for the Rust core (`ios/native`).
    func rustConfig(keyPath: String, newSession: Bool = false) -> String {
        let value: [String: Any] = [
            "name": "iOS",
            "host": host.trimmingCharacters(in: .whitespaces),
            "port": port,
            "user": user.trimmingCharacters(in: .whitespaces),
            "key_path": keyPath,
            "host_key_sha256": hostKeySHA256,
            "socks_port": socksPort,
            "new_session": newSession,
        ]
        let data = (try? JSONSerialization.data(withJSONObject: value)) ?? Data("{}".utf8)
        return String(decoding: data, as: UTF8.self)
    }
}

enum Shared {
    static var appGroup: String {
        Bundle.main.object(forInfoDictionaryKey: "STAppGroup") as? String ?? "group.dev.secrettunnel.ios"
    }

    static var groupContainer: URL? {
        FileManager.default.containerURL(forSecurityApplicationGroupIdentifier: appGroup)
    }

    /// App Group container; falls back to Application Support (e.g. unsigned simulator builds).
    static var container: URL {
        if let groupContainer { return groupContainer }
        let url = FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask)[0]
        try? FileManager.default.createDirectory(at: url, withIntermediateDirectories: true)
        return url
    }

    static var keyURL: URL { keyURL(for: selectedProfile) }
    static var settingsURL: URL { container.appendingPathComponent("settings.json") }
    static var hevLogURL: URL { container.appendingPathComponent("tun.log") }

    static func loadSettings() -> TunnelSettings {
        if let profile = selectedProfile { return profile.settings }
        return legacySettings()
    }

    static func legacySettings() -> TunnelSettings {
        guard let data = try? Data(contentsOf: settingsURL),
              let value = try? JSONDecoder().decode(TunnelSettings.self, from: data)
        else { return TunnelSettings() }
        return value
    }

    static func save(_ settings: TunnelSettings) throws {
        var catalog = try profileCatalog()
        if let index = catalog.profiles.firstIndex(where: { $0.id == catalog.selectedID }) {
            catalog.profiles[index].settings = settings
            try saveCatalog(catalog)
            return
        }
        let data = try JSONEncoder().encode(settings)
        try data.write(to: settingsURL, options: [.atomic, .completeFileProtectionUntilFirstUserAuthentication])
    }

    static func storeKey(from source: URL) throws {
        let data = try Data(contentsOf: source)
        guard data.count <= 1_048_576, String(decoding: data, as: UTF8.self).contains("PRIVATE KEY") else {
            throw NSError(domain: "SecretTunnel", code: 1, userInfo: [NSLocalizedDescriptionKey: "Файл не похож на приватный SSH-ключ"])
        }
        try data.write(to: keyURL, options: [.atomic, .completeFileProtectionUntilFirstUserAuthentication])
    }

    static var hasKey: Bool { FileManager.default.fileExists(atPath: keyURL.path) }

    static func saveSetup(_ settings: TunnelSettings, key: Data, name: String = "Мой сервер") throws {
        try addProfile(settings, key: key, name: name)
    }
}

/// Swift wrappers for the Rust C API (`secret_tunnel_ios.h`).
enum RustCore {
    private static func take(_ ptr: UnsafeMutablePointer<CChar>?) -> String {
        guard let ptr else { return "" }
        defer { st_ios_free(ptr) }
        return String(cString: ptr)
    }

    static func start(_ json: String) -> String { take(json.withCString { st_ios_start($0) }) }
    static func stop() { st_ios_stop() }
    static func status() -> String { take(st_ios_status()) }
    static func probe(_ json: String) -> String { take(json.withCString { st_ios_probe($0) }) }
    static func check(_ json: String) -> String { take(json.withCString { st_ios_check($0) }) }
    static func generateKey() -> String { take(st_ios_generate_key()) }
    static func enroll(_ json: String, password: String) -> String {
        take(json.withCString { config in password.withCString { st_ios_enroll_password(config, $0) } })
    }
    static func installAgent(_ json: String, version: String, archive: String) -> String {
        take(json.withCString { config in version.withCString { version in archive.withCString { st_ios_install_agent(config, version, $0) } } })
    }
}

func jsonObject(_ text: String) -> [String: Any] {
    (try? JSONSerialization.jsonObject(with: Data(text.utf8))) as? [String: Any] ?? [:]
}
