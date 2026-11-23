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
        for profile in catalog.profiles {
            var visited = Set<String>(); var next: String? = profile.id
            while let id = next {
                guard visited.insert(id).inserted, visited.count <= 4,
                      let server = catalog.profiles.first(where: { $0.id == id }) else { throw ProfileTransferFailure("В цепочке допустимо до трёх промежуточных серверов, без повторений.") }
                next = server.settings.jumpProfileID
            }
        }
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

struct ProfileTransferFailure: LocalizedError {
    let message: String
    init(_ message: String) { self.message = message }
    var errorDescription: String? { message }
}

extension Shared {
    static func sshJumps(for settings: TunnelSettings) throws -> [[String: Any]] {
        guard settings.jumpProfileID != nil else { return [] }
        let catalog = try profileCatalog()
        var visited = Set<String>(); var next = settings.jumpProfileID; var hops = [[String: Any]]()
        while let id = next {
            guard visited.insert(id).inserted, visited.count <= 3,
                  let profile = catalog.profiles.first(where: { $0.id == id }),
                  profile.settings.hostKeySHA256.count == 50,
                  FileManager.default.fileExists(atPath: keyURL(for: profile).path) else {
                throw ProfileTransferFailure("Проверьте цепочку SSH, ключи и отпечатки промежуточных серверов.")
            }
            hops.append(["name": profile.name, "host": profile.settings.host, "port": profile.settings.port, "user": profile.settings.user,
                         "host_key_sha256": profile.settings.hostKeySHA256, "key_path": keyURL(for: profile).path])
            next = profile.settings.jumpProfileID
        }
        return Array(hops.reversed())
    }

    static func transferDocument(ids: [String]) throws -> String {
        let catalog = try profileCatalog()
        guard !ids.isEmpty, ids.count <= 32 else { throw ProfileTransferFailure("Выберите от 1 до 32 серверов.") }
        var added = Set<String>(); var servers = [[String: Any]]()
        for root in ids {
            var next: String? = root; var route = [SavedServerProfile](); var seen = Set<String>()
            while let id = next {
                guard seen.insert(id).inserted, seen.count <= 4, let profile = catalog.profiles.first(where: { $0.id == id }) else {
                    throw ProfileTransferFailure("Проверьте цепочку SSH.")
                }
                route.append(profile); next = profile.settings.jumpProfileID
            }
            for profile in route.reversed() where added.insert(profile.id).inserted {
                guard profile.settings.hostKeySHA256.hasPrefix("SHA256:"), profile.settings.hostKeySHA256.count == 50 else {
                    throw ProfileTransferFailure("Сначала подтвердите отпечаток сервера «\(profile.name)».")
                }
                let key = try Data(contentsOf: keyURL(for: profile))
                guard key.count <= 16 * 1024 else { throw ProfileTransferFailure("Файл ключа слишком велик для переноса.") }
                servers.append(["id": profile.id, "name": profile.name, "host": profile.settings.host, "port": profile.settings.port,
                                "user": profile.settings.user, "host_key_sha256": profile.settings.hostKeySHA256,
                                "private_key": String(decoding: key, as: UTF8.self), "jump_id": profile.settings.jumpProfileID as Any? ?? NSNull()])
            }
        }
        return String(decoding: try JSONSerialization.data(withJSONObject: ["version": 1, "roots": ids, "servers": servers]), as: UTF8.self)
    }

    static func importTransferred(_ raw: String) throws -> [String] {
        struct Import: Decodable {
            struct Server: Decodable { let id, name, host, user, host_key_sha256, private_key: String; let port: Int; let jump_id: String? }
            let version: Int; let roots: [String]; let servers: [Server]
        }
        let document = try JSONDecoder().decode(Import.self, from: Data(raw.utf8))
        guard document.version == 1, (1...32).contains(document.servers.count), Set(document.servers.map(\.id)).count == document.servers.count else { throw ProfileTransferFailure("Некорректные данные переноса.") }
        var catalog = try profileCatalog()
        let ids = Dictionary(uniqueKeysWithValues: document.servers.map { ($0.id, UUID().uuidString) })
        var files = [URL]()
        do {
            let directory = container.appendingPathComponent("keys", isDirectory: true)
            try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true, attributes: [.posixPermissions: 0o700])
            for server in document.servers {
                guard let id = ids[server.id] else { throw ProfileTransferFailure("Некорректный сервер.") }
                var settings = TunnelSettings(); settings.host = server.host; settings.port = server.port; settings.user = server.user
                settings.hostKeySHA256 = server.host_key_sha256
                if let jump = server.jump_id {
                    guard let localID = ids[jump] else { throw ProfileTransferFailure("Промежуточный сервер отсутствует.") }
                    settings.jumpProfileID = localID
                }
                let profile = SavedServerProfile(id: id, name: server.name, settings: settings, keyFile: "keys/\(id).key")
                var file = keyURL(for: profile)
                try Data(server.private_key.utf8).write(to: file, options: [.atomic, .completeFileProtectionUntilFirstUserAuthentication]); files.append(file)
                try FileManager.default.setAttributes([.posixPermissions: 0o600], ofItemAtPath: file.path)
                var values = URLResourceValues(); values.isExcludedFromBackup = true; try file.setResourceValues(values)
                catalog.profiles.append(profile)
            }
            let roots = try document.roots.map { id -> String in
                guard let local = ids[id] else { throw ProfileTransferFailure("Конечный сервер отсутствует.") }; return local
            }
            guard let first = roots.first else { throw ProfileTransferFailure("Выберите конечный сервер.") }
            if catalog.selectedID.isEmpty { catalog.selectedID = first }
            try saveCatalog(catalog)
            return roots
        } catch { for file in files { try? FileManager.default.removeItem(at: file) }; throw error }
    }
}
