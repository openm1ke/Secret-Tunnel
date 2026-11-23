import XCTest
import CoreImage
@testable import SecretTunnel

final class ProfileTransferTests: XCTestCase {
    private func fixture() throws -> (URL, [String: Any]) {
        guard let path = ProcessInfo.processInfo.environment["PROFILE_TRANSFER_FIXTURE"], path.hasPrefix("/"),
              FileManager.default.fileExists(atPath: path) else { throw XCTSkip("Disposable desktop transfer fixture not supplied") }
        let url = URL(fileURLWithPath: path)
        return (url, try XCTUnwrap(JSONSerialization.jsonObject(with: Data(contentsOf: url)) as? [String: Any]))
    }
    func testDesktopQRCodeDecodesToTheEncryptedLink() throws {
        let (url, values) = try fixture()
        let image = try XCTUnwrap(CIImage(contentsOf: url.deletingPathExtension().appendingPathExtension("png")))
        let detector = try XCTUnwrap(CIDetector(ofType: CIDetectorTypeQRCode, context: nil, options: [CIDetectorAccuracy: CIDetectorAccuracyHigh]))
        let feature = try XCTUnwrap(detector.features(in: image).first as? CIQRCodeFeature)
        XCTAssertEqual(feature.messageString, values["link"] as? String)
    }
    func testDesktopTransferKeepsKeysPinsAndChainWithoutReplacingProfiles() throws {
        let (_, values) = try fixture()
        let link = try XCTUnwrap(values["link"] as? String), code = try XCTUnwrap(values["code"] as? String)
        let before = Shared.profiles; let selected = Shared.selectedProfile?.id
        defer { if let selected { try? Shared.selectProfile(selected) } }
        let wrong = (code.first == "A" ? "B" : "A") + code.dropFirst()
        XCTAssertNotNil(jsonObject(RustCore.transferUnlock(link, code: wrong, preview: false))["error"])
        XCTAssertEqual(Shared.profiles.map(\.id), before.map(\.id))
        let preview = jsonObject(RustCore.transferUnlock(link, code: code, preview: true))
        XCTAssertNil(preview["error"])
        XCTAssertFalse(String(decoding: try JSONSerialization.data(withJSONObject: preview), as: UTF8.self).contains("private_key"))
        let raw = RustCore.transferUnlock(link, code: code, preview: false)
        XCTAssertNil(jsonObject(raw)["error"])
        let roots = try Shared.importTransferred(raw)
        XCTAssertEqual(roots.count, 1)
        XCTAssertEqual(Shared.profiles.count, before.count + 2)
        XCTAssertTrue(before.allSatisfy { old in Shared.profiles.contains(old) })
        if let selected { XCTAssertEqual(Shared.selectedProfile?.id, selected) }
        let target = try XCTUnwrap(Shared.profiles.first { $0.id == roots[0] })
        let config = jsonObject(try target.settings.rustConfig(keyPath: Shared.keyURL(for: target).path))
        let hops = try XCTUnwrap(config["ssh_jumps"] as? [[String: Any]])
        XCTAssertEqual(hops.count, 1); XCTAssertEqual(hops[0]["name"] as? String, "Demo RU")
        XCTAssertEqual(String(decoding: try Data(contentsOf: Shared.keyURL(for: target)), as: UTF8.self), values["private_key"] as? String)
        let encrypted = jsonObject(RustCore.transferExport(try Shared.transferDocument(ids: roots)))
        XCTAssertNil(encrypted["error"])
        let returned = jsonObject(RustCore.transferUnlock(try XCTUnwrap(encrypted["link"] as? String), code: try XCTUnwrap(encrypted["code"] as? String), preview: true))
        XCTAssertEqual((returned["servers"] as? [Any])?.count, 2)
    }
}
