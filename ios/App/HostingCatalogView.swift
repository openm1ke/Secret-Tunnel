import SwiftUI

extension HostingOffer {
    var countryName: String { Locale(identifier: "ru_RU").localizedString(forRegionCode: country) ?? country }
    var flag: String {
        let code = country.uppercased()
        guard code.utf8.count == 2, code.utf8.allSatisfy({ (65...90).contains($0) }) else { return "🌐" }
        return String(String.UnicodeScalarView(code.unicodeScalars.compactMap { UnicodeScalar(127397 + $0.value) }))
    }
    var priceValue: String { price.replacingOccurrences(of: " / мес.", with: "") }
    var portValue: String {
        guard let speed = port_mbps else { return "Не указана" }
        return speed >= 1000 && speed % 1000 == 0 ? "\(speed / 1000) Гбит/с" : "\(speed) Мбит/с"
    }
}

struct HostingCatalogView: View {
    @EnvironmentObject private var controller: TunnelController
    @Environment(\.dismiss) private var dismiss
    @Environment(\.scenePhase) private var scenePhase
    @Environment(\.dynamicTypeSize) private var typeSize
    @StateObject private var model = HostingCatalogModel()
    @State private var region = "Все"
    @State private var sort = false
    @State private var setup = false
    let palette: MobilePalette
    var showsBack = true
    var onPurchased: (() -> Void)? = nil
    private var visible: [HostingOffer] {
        let rows = model.offers.filter { region == "Все" || $0.country == region }
        return sort ? rows.sorted { (model.results[$0.id]?.median_ms ?? .infinity) < (model.results[$1.id]?.median_ms ?? .infinity) } : rows
    }
    var body: some View {
        GeometryReader { geometry in
            let wide = geometry.size.width >= 720 && !typeSize.isAccessibilitySize
            ScrollView {
                VStack(alignment: .leading, spacing: 24) {
                    if showsBack {
                        Button { dismiss() } label: { Label("Назад", systemImage: "chevron.left").frame(minHeight: 44) }
                            .buttonStyle(.plain).foregroundColor(palette.accent)
                    }
                    header(wide: wide)
                    controls(wide: geometry.size.width >= 1060 && !typeSize.isAccessibilitySize)
                    if model.busy || !model.message.isEmpty {
                        HStack(spacing: 8) {
                            if model.busy { ProgressView().tint(palette.accent) }
                            Text(model.busy ? (model.probing ? "Измеряем задержку…" : "Обновляем данные…") : model.message)
                                .font(.footnote).fixedSize(horizontal: false, vertical: true)
                        }.foregroundColor(palette.muted).accessibilityIdentifier("hosting.message")
                    }
                    if !model.results.isEmpty {
                        Toggle("Сначала с меньшей задержкой", isOn: $sort).font(.subheadline)
                        Text("Замер в \(model.measuredAt) · для другой сети повторите проверку.").font(.footnote).foregroundColor(palette.muted)
                    }
                    LazyVGrid(columns: Array(repeating: GridItem(.flexible(), spacing: 16, alignment: .top), count: wide ? 2 : 1), alignment: .leading, spacing: 16) {
                        ForEach(visible) { offer in
                            HostingOfferCard(offer: offer, result: model.results[offer.id], palette: palette)
                        }
                    }
                    footer
                }
                .padding(wide ? 32 : 20).frame(maxWidth: 1184).frame(maxWidth: .infinity)
            }
        }
        .background(palette.background.ignoresSafeArea()).foregroundColor(palette.text).tint(palette.accent)
        .task { await model.run("load"); if !Task.isCancelled { await model.run("refresh") } }
        .onDisappear { model.cancel() }
        .onChange(of: controller.isActive) { if $0 { model.cancel("Туннель включён. Замер отменён.") } }
        .onChange(of: scenePhase) { if $0 != .active { model.cancel("Приложение свёрнуто. Повторите замер.") } }
        .fullScreenCover(isPresented: $setup) { SetupWizard(palette: palette, serverPurchased: true).environmentObject(controller) }
    }
    private func header(wide: Bool) -> some View {
        VStack(alignment: .leading, spacing: 8) {
            Text("ВЫБОР СВОЕГО VPS").font(.caption.weight(.semibold)).tracking(1.4).foregroundColor(palette.accent)
            Text("Каталог серверов").font(.system(size: wide ? 34 : 28, weight: .semibold)).accessibilityAddTraits(.isHeader)
            Text("Сравните локации и задержку. Выберите хостера и настройте SSH после покупки.")
                .font(.subheadline).foregroundColor(palette.muted).frame(maxWidth: 560, alignment: .leading)
            Text("Провайдеры: \(model.offers.count) · страны: \(Set(model.offers.map(\.country)).count)")
                .font(.footnote).foregroundColor(palette.muted)
        }
    }
    @ViewBuilder private func controls(wide: Bool) -> some View {
        if wide {
            HStack(spacing: 16) { countryFilters; probeActions(wide: true).frame(width: 320) }
        } else {
            VStack(alignment: .leading, spacing: 16) { countryFilters; probeActions(wide: false) }
        }
    }
    private var countryFilters: some View {
        ScrollView(.horizontal, showsIndicators: false) {
            HStack(spacing: 8) {
                filter("Все страны", value: "Все")
                ForEach(Array(Set(model.offers.map(\.country))).sorted(), id: \.self) { code in
                    if let offer = model.offers.first(where: { $0.country == code }) { filter("\(offer.flag) \(offer.countryName)", value: code) }
                }
            }
        }
    }
    private func probeActions(wide: Bool) -> some View {
        HStack(spacing: 12) {
            Button(model.probing ? "Отменить замер" : "Проверить задержку") {
                if model.probing { model.cancel() }
                else { Task { await model.run("probe", ids: visible.map(\.id), vpnActive: controller.isActive) } }
            }.buttonStyle(MobileButtonStyle(palette: palette, primary: true))
                .frame(maxWidth: wide ? 260 : .infinity)
                .disabled(model.busy && !model.probing || model.offers.isEmpty)
                .accessibilityIdentifier("hosting.probe")
            Button { Task { await model.run("refresh") } } label: {
                Image(systemName: "arrow.clockwise").font(.body.weight(.semibold)).frame(width: 48, height: 48)
            }.buttonStyle(.plain).foregroundColor(palette.accent).background(RoundedRectangle(cornerRadius: 14).fill(palette.raised))
                .disabled(model.busy).accessibilityLabel("Обновить каталог")
        }
    }
    private func filter(_ label: String, value: String) -> some View {
        Button { region = value } label: {
            Text(label).font(.subheadline.weight(region == value ? .semibold : .regular))
                .padding(.horizontal, 16).frame(minHeight: 44)
                .foregroundColor(region == value ? palette.onAccent : palette.text)
                .background(Capsule().fill(region == value ? palette.accent : palette.surface))
                .overlay(Capsule().stroke(region == value ? .clear : palette.border, lineWidth: 1))
        }.buttonStyle(.plain).disabled(model.probing).accessibilityAddTraits(region == value ? .isSelected : [])
    }
    private var footer: some View {
        VStack(alignment: .leading, spacing: 16) {
            DisclosureGroup("Как сравнивать серверы") {
                Text("Задержка — медиана пяти TCP-подключений из текущей сети, не ICMP-пинг. Другой VPN влияет на результат. Скорость порта заявлена хостером и не гарантирует скорость SSH. Тестовый узел и будущий VPS могут различаться. Цены в исходной валюте; итоговая сумма — у хостера.")
                    .font(.footnote).foregroundColor(palette.muted).padding(.top, 8)
            }.font(.subheadline)
            Button("Сервер уже куплен — настроить SSH") {
                if let onPurchased { if showsBack { dismiss() }; onPurchased() } else { setup = true }
            }.buttonStyle(MobileButtonStyle(palette: palette, primary: false)).frame(maxWidth: 460)
        }.padding(.top, 8)
    }
}

private struct HostingOfferCard: View {
    let offer: HostingOffer
    let result: HostingMeasurement?
    let palette: MobilePalette
    @State private var expanded = false
    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            HStack(alignment: .center, spacing: 12) {
                Text(offer.flag).font(.system(size: 32)).accessibilityLabel("Локация сервера: \(offer.countryName)")
                VStack(alignment: .leading, spacing: 4) {
                    Text(offer.provider).font(.title3.weight(.semibold))
                    Text("\(offer.countryName) · \(offer.city)").font(.footnote).foregroundColor(palette.muted)
                }
                Spacer(minLength: 0)
            }
            HStack(alignment: .firstTextBaseline, spacing: 16) {
                metric(offer.priceValue, label: "в месяц", accent: false).frame(maxWidth: .infinity, alignment: .leading)
                metric(offer.test_host == nil ? "Нет узла" : result?.median_ms.map { String(format: "%.0f мс", $0) } ?? (result == nil ? "—" : "Нет замера"), label: offer.test_host == nil ? "замер недоступен" : "задержка TCP", accent: offer.test_host != nil)
                    .frame(maxWidth: .infinity, alignment: .leading).accessibilityIdentifier("hosting.result.\(offer.id)")
            }
            Text(offer.price_note ?? "Условия цены — у хостера").font(.footnote).foregroundColor(palette.muted)
                .fixedSize(horizontal: false, vertical: true)
            HStack {
                Label("Порт", systemImage: "speedometer").font(.subheadline).foregroundColor(palette.muted)
                Spacer(minLength: 4)
                Text(offer.port_mbps == nil ? offer.portValue : "до \(offer.portValue)").font(.subheadline.weight(.semibold)).foregroundColor(palette.accent)
            }.padding(12).background(RoundedRectangle(cornerRadius: 12).fill(palette.raised))
                .accessibilityElement(children: .ignore).accessibilityLabel(offer.bandwidth)
            Text(offer.hardware).font(.subheadline).foregroundColor(palette.text)
            HStack(spacing: 12) {
                if let url = URL(string: offer.order_url) {
                    Link(destination: url) { Label("Сайт хостера", systemImage: "arrow.up.right").frame(maxWidth: .infinity, minHeight: 44) }
                        .font(.subheadline.weight(.semibold)).foregroundColor(palette.accent)
                        .background(RoundedRectangle(cornerRadius: 12).stroke(palette.border, lineWidth: 1))
                }
            }
            DisclosureGroup(isExpanded: $expanded) {
                VStack(alignment: .leading, spacing: 12) {
                    Text(offer.plan).font(.subheadline.weight(.semibold))
                    Text(offer.conditions)
                    if let result { Text(result.label).accessibilityIdentifier("hosting.details.\(offer.id)") }
                    Text(offer.test_note)
                    Text("Проверено: \(String(offer.verified_at.prefix(10)))")
                    if offer.company_country != "Не уточнено" { Text("Страна компании: \(offer.company_country)") }
                    if let url = URL(string: offer.source_url) { Link("Источник цены ↗", destination: url).frame(minHeight: 44) }
                    if let url = URL(string: offer.test_source_url) { Link("Тестовый узел ↗", destination: url).frame(minHeight: 44) }
                }.font(.footnote).foregroundColor(palette.muted).padding(.top, 12)
            } label: { Text("Подробнее").frame(minHeight: 44) }.font(.subheadline).foregroundColor(palette.muted)
            if offer.update_error != nil { Label("Цена требует обновления", systemImage: "exclamationmark.circle").font(.footnote).foregroundColor(palette.warning) }
        }
        .padding(24).frame(maxWidth: .infinity, alignment: .leading)
        .background(RoundedRectangle(cornerRadius: 24).fill(palette.surface))
        .overlay(RoundedRectangle(cornerRadius: 24).stroke(palette.border.opacity(0.6), lineWidth: 1))
    }
    private func metric(_ value: String, label: String, accent: Bool) -> some View {
        VStack(alignment: .leading, spacing: 4) {
            Text(value).font(.system(size: 30, weight: .semibold, design: .rounded)).monospacedDigit().minimumScaleFactor(0.65).lineLimit(1)
                .foregroundColor(accent ? palette.accent : palette.text)
            Text(label).font(.caption).foregroundColor(palette.muted)
        }.accessibilityElement(children: .combine)
    }
}
