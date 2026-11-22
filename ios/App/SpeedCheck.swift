import Foundation
import Network

@MainActor
final class SpeedCheck: ObservableObject {
    struct Result {
        let down: Double
        let up: Double
    }

    @Published private(set) var running = false
    @Published private(set) var result: Result?
    @Published private(set) var error = ""

    static var available: Bool {
        if #available(iOS 17.0, *) { return true }
        return false
    }

    func start(settings: TunnelSettings, simulator: Bool) {
        guard #available(iOS 17.0, *), !running else { return }
        running = true
        result = nil
        error = ""
        let config = settings.rustConfig(keyPath: Shared.keyURL.path, newSession: simulator)
        let port = settings.socksPort
        Task {
            defer { running = false }
            do {
                result = try await Task.detached {
                    if simulator {
                        let failure = RustCore.start(config)
                        guard failure.isEmpty else { throw CheckError(message: failure) }
                    }
                    defer { if simulator { RustCore.stop() } }
                    let result = try await Self.measure(port: port)
                    if simulator {
                        let status = jsonObject(RustCore.status())
                        guard (status["rx"] as? NSNumber)?.intValue ?? 0 >= 2_000_000,
                              (status["tx"] as? NSNumber)?.intValue ?? 0 >= 1_000_000 else {
                            throw CheckError(message: "Счётчики SSH не подтвердили передачу тестовых данных.")
                        }
                    }
                    return result
                }.value
            } catch {
                self.error = error.localizedDescription
            }
        }
    }

    @available(iOS 17.0, *)
    private nonisolated static func measure(port: Int) async throws -> Result {
        guard let value = UInt16(exactly: port), let proxyPort = NWEndpoint.Port(rawValue: value) else {
            throw CheckError(message: "Некорректный порт SOCKS5.")
        }
        let settings = URLSessionConfiguration.ephemeral
        var proxy = ProxyConfiguration(socksv5Proxy: .hostPort(host: "::1", port: proxyPort))
        proxy.allowFailover = false
        settings.proxyConfigurations = [proxy]
        settings.timeoutIntervalForRequest = 15
        settings.timeoutIntervalForResource = 25
        settings.urlCache = nil
        let session = URLSession(configuration: settings)
        defer { session.invalidateAndCancel() }
        let clock = ContinuousClock()
        let startedDownload = clock.now
        let (download, downloadResponse) = try await session.data(
            from: URL(string: "https://speed.cloudflare.com/__down?bytes=2000000")!
        )
        try validate(downloadResponse)
        guard download.count == 2_000_000 else {
            throw CheckError(message: "Сервер вернул неполный объём данных.")
        }
        let down = rate(download.count, startedDownload.duration(to: clock.now))
        var upload = URLRequest(url: URL(string: "https://speed.cloudflare.com/__up")!)
        upload.httpMethod = "POST"
        upload.setValue("application/octet-stream", forHTTPHeaderField: "Content-Type")
        let payload = Data(repeating: 0, count: 1_000_000)
        let startedUpload = clock.now
        let (_, uploadResponse) = try await session.upload(for: upload, from: payload)
        try validate(uploadResponse)
        return Result(down: down, up: rate(payload.count, startedUpload.duration(to: clock.now)))
    }

    private nonisolated static func validate(_ response: URLResponse) throws {
        guard let status = (response as? HTTPURLResponse)?.statusCode else {
            throw CheckError(message: "Сервер не ответил на тест скорости.")
        }
        guard (200...299).contains(status) else {
            throw CheckError(message: "Сервер теста скорости вернул HTTP \(status).")
        }
    }

    @available(iOS 17.0, *)
    private nonisolated static func rate(_ bytes: Int, _ elapsed: Duration) -> Double {
        let (seconds, attoseconds) = elapsed.components
        return Double(bytes * 8) / max(0.001, Double(seconds) + Double(attoseconds) / 1e18) / 1_000_000
    }

    private struct CheckError: LocalizedError {
        let message: String
        var errorDescription: String? { message }
    }
}
