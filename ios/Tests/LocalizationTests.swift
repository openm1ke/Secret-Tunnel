import XCTest
@testable import SecretTunnel

final class LocalizationTests: XCTestCase {
    func testBundledEnglishAndRussianFallbackPreserveArguments() {
        let defaults = UserDefaults.standard
        let previous = defaults.object(forKey: "language")
        defer { if let previous { defaults.set(previous, forKey: "language") } else { defaults.removeObject(forKey: "language") } }
        defaults.set("en", forKey: "language")
        XCTAssertEqual(tr("Подключить"), "Connect")
        XCTAssertEqual(tr("Сервер «{0}» сохранён", "Рабочий {1} 🔒"), "Server “Рабочий {1} 🔒” saved")
        XCTAssertEqual(tr("Сервер теста скорости вернул HTTP 429."), "The speed test server returned HTTP 429.")
        XCTAssertEqual(tr("{0} ч {1} мин", 21, 8), "21 h 8 min")
        XCTAssertEqual(tr("E_CUSTOM 203.0.113.1"), "E_CUSTOM 203.0.113.1")
        defaults.set("ru", forKey: "language")
        XCTAssertEqual(tr("Подключить"), "Подключить")
        XCTAssertEqual(tr("{0} ч {1} мин", 21, 8), "21 ч 8 мин")
        defaults.set("unsupported", forKey: "language")
        XCTAssertEqual(AppLanguage.current, .ru)
    }
}
