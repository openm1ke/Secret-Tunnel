import XCTest
@testable import SecretTunnel

final class SetupWizardTests: XCTestCase {
    private func fixture() throws -> [String: Any] {
        guard let path = ProcessInfo.processInfo.environment["WIZARD_TEST_FIXTURE"], path.hasPrefix("/"),
              FileManager.default.fileExists(atPath: path) else { throw XCTSkip("Disposable SSH fixture not supplied") }
        return try XCTUnwrap(JSONSerialization.jsonObject(with: Data(contentsOf: URL(fileURLWithPath: path))) as? [String: Any])
    }
    func testChecksumRequiresOneExactFilename() throws {
        let hash = String(repeating: "a", count: 64)
        XCTAssertEqual(try SetupAgentRelease.checksum("\(hash)  ./agent.tar.gz\n", filename: "agent.tar.gz"), hash)
        XCTAssertThrowsError(try SetupAgentRelease.checksum("\(hash)  other.tar.gz", filename: "agent.tar.gz"))
        XCTAssertThrowsError(try SetupAgentRelease.checksum("\(hash) agent.tar.gz\n\(hash) agent.tar.gz", filename: "agent.tar.gz"))
    }

    func testProfilesKeepIndependentKeysAndSelection() throws {
        let previous = Shared.selectedProfile?.id
        defer { if let previous { try? Shared.selectProfile(previous) } }
        var settings = TunnelSettings()
        settings.host = "203.0.113.10"; settings.user = "demo"
        let first = Data(try XCTUnwrap(jsonObject(RustCore.generateKey())["private"] as? String).utf8)
        let second = Data(try XCTUnwrap(jsonObject(RustCore.generateKey())["private"] as? String).utf8)
        try Shared.saveSetup(settings, key: first, name: "First")
        let firstID = try XCTUnwrap(Shared.selectedProfile?.id)
        settings.host = "203.0.113.20"
        try Shared.saveSetup(settings, key: second, name: "Second")
        let secondID = try XCTUnwrap(Shared.selectedProfile?.id)
        XCTAssertNotEqual(firstID, secondID)
        try Shared.selectProfile(firstID)
        XCTAssertEqual(Shared.loadSettings().host, "203.0.113.10")
        XCTAssertEqual(try Data(contentsOf: Shared.keyURL), first)
        try Shared.selectProfile(secondID)
        XCTAssertEqual(Shared.loadSettings().host, "203.0.113.20")
        XCTAssertEqual(try Data(contentsOf: Shared.keyURL), second)
    }

    @MainActor func testPasswordEnrollmentAndRetry() async throws {
        let fixture = try fixture()
        let model = SSHSetupModel()
        model.chooseKey(false)
        model.host = fixture["host"] as! String
        model.port = String(fixture["port"] as! Int)
        model.user = fixture["user"] as! String
        model.password = "wrong"
        await model.probe()
        XCTAssertEqual(model.fingerprint, fixture["fingerprint"] as? String, model.error)
        await model.configure()
        XCTAssertFalse(model.saved)
        XCTAssertTrue(model.error.contains("Сервер не принял"), model.error)
        model.back()
        model.password = fixture["password"] as! String
        await model.probe()
        await model.configure()
        XCTAssertTrue(model.saved, model.error)
        if !model.done { await model.configure(install: true) }
        XCTAssertTrue(model.done, model.error)
        XCTAssertEqual(model.password, "")
        XCTAssertEqual(Shared.loadSettings().hostKeySHA256, fixture["fingerprint"] as? String)
        let saved = try String(contentsOf: Shared.settingsURL, encoding: .utf8)
        XCTAssertFalse(saved.contains(fixture["password"] as! String))
        XCTAssertFalse(saved.contains("PRIVATE KEY"))
    }

    @MainActor func testExistingKeyAndChangedFingerprint() async throws {
        let values = try fixture()
        let model = SSHSetupModel()
        model.chooseKey(true)
        model.host = values["host"] as! String
        model.port = String(values["port"] as! Int)
        model.user = values["user"] as! String
        model.importKey(URL(fileURLWithPath: values["key_path"] as! String))
        XCTAssertTrue(model.valid, model.error)
        await model.probe()
        XCTAssertEqual(model.fingerprint, values["fingerprint"] as? String, model.error)
        model.fingerprint = "SHA256:" + String(repeating: "A", count: 43)
        await model.configure()
        XCTAssertFalse(model.saved)
        XCTAssertFalse(model.error.isEmpty)
        model.fingerprint = values["fingerprint"] as! String
        await model.configure()
        XCTAssertTrue(model.saved, model.error)
        XCTAssertTrue(model.done, model.error)
        XCTAssertFalse(model.busy)
        model.clear()
    }
}
