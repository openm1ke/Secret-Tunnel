import Foundation

struct SavedServerProfile: Codable, Identifiable, Equatable {
    let id: String
    var name: String
    var settings: TunnelSettings
    let keyFile: String
}

struct ServerProfileCatalog: Codable {
    var selectedID: String
    var profiles: [SavedServerProfile]
}

extension Shared {
    static var profilesURL: URL { container.appendingPathComponent("servers.json") }
    static var profiles: [SavedServerProfile] { (try? profileCatalog().profiles) ?? [] }
    static var selectedProfile: SavedServerProfile? {
        guard let catalog = try? profileCatalog() else { return nil }
        return catalog.profiles.first { $0.id == catalog.selectedID }
    }

    static func profileCatalog() throws -> ServerProfileCatalog {
        if FileManager.default.fileExists(atPath: profilesURL.path) {
            let catalog = try JSONDecoder().decode(ServerProfileCatalog.self, from: Data(contentsOf: profilesURL))
            guard Set(catalog.profiles.map(\.id)).count == catalog.profiles.count,
                  catalog.profiles.allSatisfy({ profile in
                      (profile.id == "legacy" && profile.keyFile == "ssh_key") ||
                      (UUID(uuidString: profile.id) != nil && profile.keyFile == "keys/\(profile.id).key")
                  }), catalog.profiles.contains(where: { $0.id == catalog.selectedID }) else {
                throw NSError(domain: "SecretTunnel", code: 2, userInfo: [NSLocalizedDescriptionKey: "Не удалось прочитать список серверов."])
            }
            return catalog
        }
        // Keep the original key in place; creating the first new profile never overwrites it.
        let settings = legacySettings()
        if !settings.host.isEmpty && FileManager.default.fileExists(atPath: container.appendingPathComponent("ssh_key").path) {
            return ServerProfileCatalog(selectedID: "legacy", profiles: [SavedServerProfile(id: "legacy", name: "Мой сервер", settings: settings, keyFile: "ssh_key")])
        }
        return ServerProfileCatalog(selectedID: "", profiles: [])
    }

    static func keyURL(for profile: SavedServerProfile?) -> URL {
        container.appendingPathComponent(profile?.keyFile ?? "ssh_key")
    }

    static func saveCatalog(_ catalog: ServerProfileCatalog) throws {
        try JSONEncoder().encode(catalog).write(to: profilesURL, options: [.atomic, .completeFileProtectionUntilFirstUserAuthentication])
    }

    static func selectProfile(_ id: String) throws {
        var catalog = try profileCatalog()
        guard let profile = catalog.profiles.first(where: { $0.id == id }),
              FileManager.default.fileExists(atPath: keyURL(for: profile).path) else {
            throw NSError(domain: "SecretTunnel", code: 3, userInfo: [NSLocalizedDescriptionKey: "У выбранного сервера нет ключа доступа."])
        }
        catalog.selectedID = id
        try saveCatalog(catalog)
    }

    static func addProfile(_ settings: TunnelSettings, key: Data, name: String) throws {
        var catalog = try profileCatalog()
        let id = UUID().uuidString
        let profile = SavedServerProfile(id: id, name: name.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty ? "Мой сервер" : name, settings: settings, keyFile: "keys/\(id).key")
        let directory = container.appendingPathComponent("keys", isDirectory: true)
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true, attributes: [.posixPermissions: 0o700])
        var destination = keyURL(for: profile)
        do {
            try key.write(to: destination, options: [.atomic, .completeFileProtectionUntilFirstUserAuthentication])
            try FileManager.default.setAttributes([.posixPermissions: 0o600], ofItemAtPath: destination.path)
            var values = URLResourceValues(); values.isExcludedFromBackup = true
            try destination.setResourceValues(values)
            catalog.profiles.append(profile)
            catalog.selectedID = id
            try saveCatalog(catalog)
        } catch {
            try? FileManager.default.removeItem(at: destination)
            throw error
        }
    }
}
