import Foundation
import Network
import SwiftUI

struct HostingOffer: Decodable, Identifiable {
    let id, provider, company_country, country, city, plan, currency: String
    let price_minor, cpu, ram_mb, disk_gb: Int
    let port_mbps: Int?
    let conditions, verified_at, source_url, order_url, test_source_url, test_note: String
    let update_error, price_note: String?
    let test_host: String?
    var location: String { "\(country == "RU" ? tr("Россия") : country == "FR" ? tr("Франция") : country == "NL" ? tr("Нидерланды") : country) · \(city)" }
    var price: String {
        let format = NumberFormatter(); format.locale = AppLanguage.current.locale
        format.numberStyle = .decimal; format.maximumFractionDigits = 2
        let value = format.string(from: NSNumber(value: Double(price_minor)/100)) ?? "—"
        return tr("{0} {1} / мес.", value, currency == "RUB" ? "₽" : currency == "EUR" ? "€" : "$")
    }
    var hardware: String {
        let ram = ram_mb % 1024 == 0 ? tr("{0} ГБ", ram_mb / 1024) : tr("{0} МБ", ram_mb)
        return tr("{0} vCPU · {1} RAM · {2} ГБ", cpu, ram, disk_gb)
    }
    var bandwidth: String { port_mbps.map { tr("Порт до {0} Мбит/с", $0) } ?? tr("Скорость порта не указана") }
}
struct HostingMeasurement: Decodable {
    let id, method, family: String
    let median_ms, min_ms, max_ms: Double?
    let successes, attempts: Int
    let error: String?
    var label: String {
        guard let median = median_ms, let min = min_ms, let max = max_ms else { return error ?? tr("Нет ответа TCP") }
        return String(format: tr("TCP %.0f мс · %.0f–%.0f мс · %d/%d ответов · %@"), median, min, max, successes, attempts, family)
    }
}
struct HostingDocument: Decodable { let offers: [HostingOffer] }
struct HostingResponse: Decodable {
    let catalog: HostingDocument?
    let results: [HostingMeasurement]?
    let error, warning, source: String?
    let cancelled: Bool?
}

@MainActor final class HostingCatalogModel: ObservableObject {
    @Published var offers: [HostingOffer] = []
    @Published var results: [String: HostingMeasurement] = [:]
    @Published var busy = false
    @Published var probing = false
    @Published var message = ""
    @Published var measuredAt = ""
    private var generation = 0
    private var job = ""
    private let monitor = NWPathMonitor()
    private var pathSignature: String?
    private static var cacheDir: String {
        FileManager.default.urls(for: .cachesDirectory, in: .userDomainMask)[0].appendingPathComponent("hosting").path
    }
    init() {
        monitor.pathUpdateHandler = { [weak self] path in
            let used = [NWInterface.InterfaceType.wifi, .cellular, .wiredEthernet, .other, .loopback].filter { path.usesInterfaceType($0) }
            let signature = "\(path.status)-\(path.isExpensive)-\(used)-\(path.availableInterfaces.map { $0.name }.sorted())"
            Task { @MainActor [weak self] in
                guard let self else { return }
                if let old = self.pathSignature, old != signature {
                    self.cancel(tr("Сеть изменилась. Повторите замер.")); self.results = [:]; self.measuredAt = ""
                }
                self.pathSignature = signature
            }
        }
        monitor.start(queue: DispatchQueue(label: "hosting.path"))
    }
    deinit { monitor.cancel() }
    func cancel(_ reason: String = tr("Замер отменён.")) {
        generation += 1
        _ = RustCore.hosting("{\"action\":\"cancel\",\"job\":\"\(job)\"}", cacheDir: Self.cacheDir, vpnActive: false)
        if probing { message = reason }
        probing = false; busy = false
    }
    func run(_ action: String, ids: [String] = [], vpnActive: Bool = false) async {
        guard !busy else { return }
        busy = true; probing = action == "probe"; message = ""
        generation += 1; let token = generation
        if action == "probe" { job = UUID().uuidString; results = [:]; measuredAt = "" }
        let dir = Self.cacheDir
        let request = String(decoding: (try? JSONSerialization.data(withJSONObject: ["action": action, "ids": ids, "job": job])) ?? Data(), as: UTF8.self)
        let raw = await Task.detached { RustCore.hosting(request, cacheDir: dir, vpnActive: vpnActive) }.value
        guard token == generation else { return }
        defer { busy = false; probing = false }
        do {
            let response = try JSONDecoder().decode(HostingResponse.self, from: Data(raw.utf8))
            if let catalog = response.catalog { offers = catalog.offers; results = [:]; measuredAt = "" }
            if let values = response.results {
                results = Dictionary(uniqueKeysWithValues: values.map { ($0.id, $0) })
                measuredAt = DateFormatter.localizedString(from: Date(), dateStyle: .none, timeStyle: .short)
            }
            message = response.error ?? response.warning ?? (response.source == "bundled" ? tr("Встроенная копия. Дата проверки указана у каждого тарифа.") : response.source == "cache" ? tr("Сохранённая копия каталога.") : response.source == "network" ? tr("Каталог обновлён из сети.") : "")
        } catch { message = tr("Не удалось прочитать каталог. Повторите загрузку.") }
    }
}
