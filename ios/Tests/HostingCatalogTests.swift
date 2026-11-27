import XCTest
@testable import SecretTunnel

final class HostingCatalogTests: XCTestCase {
    private func call(_ raw: String, dir: URL, active: Bool = false) async throws -> HostingResponse {
        let text = await Task.detached { RustCore.hosting(raw, cacheDir: dir.path, vpnActive: active) }.value
        return try JSONDecoder().decode(HostingResponse.self, from: Data(text.utf8))
    }
    func testBundledCatalogAndNoProfileWrites() async throws {
        let settings = try? Data(contentsOf: Shared.settingsURL)
        let dir = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        let response = try await call("{\"action\":\"load\"}", dir: dir)
        XCTAssertEqual(response.catalog?.offers.count, 7)
        XCTAssertEqual(Set(response.catalog?.offers.map(\.country) ?? []), ["RU", "FR", "NL", "DE"])
        XCTAssertEqual(response.catalog?.offers.first?.price_minor, 649)
        XCTAssertFalse(FileManager.default.fileExists(atPath: dir.path))
        XCTAssertEqual(try? Data(contentsOf: Shared.settingsURL), settings)
    }
    func testActiveTunnelBlocksMeasurementWithoutStoppingVPN() async throws {
        let response = try await call("{\"action\":\"probe\",\"ids\":[\"ovh\"]}", dir: FileManager.default.temporaryDirectory, active: true)
        XCTAssertTrue(response.error?.contains("Остановите туннель") == true)
        XCTAssertNil(response.results)
    }
    func testMalformedCacheFallsBackAndIsNotCopiedIntoSettings() async throws {
        let dir = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: dir) }
        try Data("{\"schema_version\":999}".utf8).write(to: dir.appendingPathComponent("hosting-catalog-v1.json"))
        let response = try await call("{\"action\":\"load\"}", dir: dir)
        XCTAssertEqual(response.source, "bundled")
        XCTAssertEqual(response.catalog?.offers.count, 7)
    }
    @MainActor func testModelLoadAndCancellation() async {
        let model = HostingCatalogModel()
        await model.run("load")
        XCTAssertEqual(model.offers.count, 7)
        model.cancel()
        XCTAssertFalse(model.busy)
        XCTAssertFalse(model.probing)
    }
}
