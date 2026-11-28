import Foundation
import CryptoKit

/// Only public release assets are accepted; no server-supplied URL is executed.
enum SetupAgentRelease {
    static func download(version: String) async throws -> URL {
        guard version.range(of: #"^\d+\.\d+\.\d+$"#, options: .regularExpression) != nil else {
            throw SetupFailure(tr("Некорректная версия приложения."))
        }
        let base = "https://github.com/openm1ke/Secret-Tunnel/releases/download/v\(version)/"
        let raw = try await receive("https://api.github.com/repos/openm1ke/Secret-Tunnel/releases/tags/v\(version)", limit: 2 * 1024 * 1024)
        let metadata = try JSONSerialization.jsonObject(with: raw) as? [String: Any] ?? [:]
        guard metadata["tag_name"] as? String == "v\(version)", metadata["draft"] as? Bool == false,
              metadata["prerelease"] as? Bool == false, let assets = metadata["assets"] as? [[String: Any]] else {
            throw SetupFailure(tr("GitHub вернул неподходящий релиз."))
        }
        let filename = "SecretTunnel-\(version)-server-linux-x86_64.tar.gz"
        func assetSize(_ name: String, limit: Int) throws -> Int {
            guard let asset = assets.first(where: { $0["name"] as? String == name && $0["browser_download_url"] as? String == base + name }),
                  let size = asset["size"] as? Int, (1...limit).contains(size) else {
                throw SetupFailure(tr("В релизе нет файла {0}. Повторите позже.", name))
            }
            return size
        }
        let size = try assetSize(filename, limit: 32 * 1024 * 1024)
        let sums = try await receive(base + "SHA256SUMS.txt", limit: assetSize("SHA256SUMS.txt", limit: 2 * 1024 * 1024))
        let expected = try checksum(String(decoding: sums, as: UTF8.self), filename: filename)
        let archive = try await receive(base + filename, limit: size)
        guard archive.count == size, SHA256.hash(data: archive).map({ String(format: "%02x", $0) }).joined() == expected else {
            throw SetupFailure(tr("Архив скачан не полностью или контрольная сумма не совпала. Повторите загрузку."))
        }
        let location = FileManager.default.temporaryDirectory.appendingPathComponent("ssh-setup-agent-\(UUID().uuidString).tar.gz")
        try archive.write(to: location, options: [.atomic, .completeFileProtectionUntilFirstUserAuthentication])
        return location
    }

    static func checksum(_ contents: String, filename: String) throws -> String {
        let found = contents.split(whereSeparator: \.isNewline).compactMap { line -> String? in
            let parts = line.split(maxSplits: 1, whereSeparator: \.isWhitespace)
            guard parts.count == 2 else { return nil }
            var name = String(parts[1]).trimmingCharacters(in: .whitespaces)
            if name.hasPrefix("*") { name.removeFirst() }
            if name.hasPrefix("./") { name.removeFirst(2) }
            let hash = String(parts[0]).lowercased()
            return name == filename && hash.count == 64 && hash.allSatisfy({ $0.isHexDigit && $0.isASCII }) ? hash : nil
        }
        guard found.count == 1 else { throw SetupFailure(tr("В релизе нет однозначной контрольной суммы агента.")) }
        return found[0]
    }

    private static func receive(_ address: String, limit: Int) async throws -> Data {
        let config = URLSessionConfiguration.ephemeral
        config.timeoutIntervalForRequest = 30
        config.timeoutIntervalForResource = 120
        let session = URLSession(configuration: config, delegate: ReleaseRedirect(), delegateQueue: nil)
        defer { session.invalidateAndCancel() }
        var request = URLRequest(url: URL(string: address)!)
        request.setValue("SecretTunnel-iOS", forHTTPHeaderField: "User-Agent")
        let (bytes, response) = try await session.bytes(for: request)
        guard (response as? HTTPURLResponse)?.statusCode == 200 else {
            throw SetupFailure(tr("Не удалось скачать агент с GitHub. Проверьте сеть и повторите позже."))
        }
        guard response.expectedContentLength <= Int64(limit) else { throw SetupFailure(tr("Размер файла больше допустимого.")) }
        var output = Data()
        output.reserveCapacity(min(limit, 32 * 1024 * 1024))
        let deadline = Date().addingTimeInterval(120)
        for try await byte in bytes {
            guard output.count < limit else { throw SetupFailure(tr("Размер файла больше заявленного.")) }
            output.append(byte)
            if output.count % 65536 == 0, Date() > deadline { throw SetupFailure(tr("Истекло время загрузки агента.")) }
        }
        return output
    }
}

private final class ReleaseRedirect: NSObject, URLSessionTaskDelegate {
    func urlSession(_ session: URLSession, task: URLSessionTask, willPerformHTTPRedirection response: HTTPURLResponse,
                    newRequest request: URLRequest, completionHandler: @escaping (URLRequest?) -> Void) {
        let url = request.url
        let host = url?.host?.lowercased() ?? ""
        let accepted = url?.scheme == "https" && url?.user == nil && url?.password == nil &&
            (url?.port == nil || url?.port == 443) &&
            (["github.com", "api.github.com"].contains(host) || host.hasSuffix(".githubusercontent.com"))
        completionHandler(accepted ? request : nil)
    }
}
