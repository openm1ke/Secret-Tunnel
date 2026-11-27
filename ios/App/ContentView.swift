import SwiftUI
import UniformTypeIdentifiers

@main
struct SecretTunnelApp: App {
    @StateObject private var controller = TunnelController()
    @AppStorage("appearance") private var appearance: Appearance = .system

    var body: some Scene {
        WindowGroup {
            ContentView(appearance: $appearance)
                .environmentObject(controller)
                .preferredColorScheme(appearance.colorScheme)
        }
    }
}

private enum Page: String, CaseIterable {
    case home = "Главная"
    case apps = "Приложения"
    case catalog = "Каталог"
    case speed = "Скорость"
    case more = "Ещё"

    var icon: String {
        switch self {
        case .home: return "checkmark.shield"
        case .apps: return "square.grid.2x2"
        case .speed: return "speedometer"
        case .catalog: return "globe.europe.africa"
        case .more: return "ellipsis"
        }
    }
}

private enum MorePage: String {
    case stats = "Статистика"
    case servers = "Серверы"
    case proxies = "Прокси"
    case settings = "Настройки"
    case logs = "Журнал"
    case about = "О программе"
}

struct ContentView: View {
    @EnvironmentObject private var c: TunnelController
    @Environment(\.colorScheme) private var colorScheme
    @Environment(\.dynamicTypeSize) private var textSize
    @Binding var appearance: Appearance
    @AppStorage("hideSensitive") private var hideSensitive = true
    @StateObject private var speedCheck = SpeedCheck()
    @State private var page: Page = .home
    @State private var morePage: MorePage?
    @State private var editingServer = false
    @State private var importingKey = false
    @State private var showAddress = false
    @State private var editingPort = false
    @State private var setupOpen = false
    @State private var setupPurchased = false
    @State private var transferOpen = false
    @State private var transferServerID: String?
    @State private var transferPayload = ""

    private var palette: MobilePalette { colorScheme == .dark ? .dark : .light }

    var body: some View {
        VStack(spacing: 0) {
            header
            if page == .catalog {
                HostingCatalogView(palette: palette, showsBack: false, onPurchased: { setupPurchased = true; setupOpen = true }).environmentObject(c)
            } else {
            ScrollView {
                VStack(alignment: .leading, spacing: 12) {
                    currentPage
                }
                .padding(.horizontal, 20)
                .padding(.top, 8)
                .padding(.bottom, 20)
                .frame(maxWidth: 620)
                .frame(maxWidth: .infinity)
            }
            .id(page.rawValue + (morePage?.rawValue ?? ""))
            }
            bottomBar
        }
        .foregroundColor(palette.text)
        .background(palette.background.ignoresSafeArea())
        .tint(palette.accent)
        .fileImporter(isPresented: $importingKey, allowedContentTypes: [.data, .text, .item]) { result in
            if case let .success(url) = result { c.importKey(url) }
        }
        .sheet(isPresented: $editingServer) {
            ServerEditor(settings: c.settings, palette: palette)
                .environmentObject(c)
        }
        .sheet(isPresented: $editingPort) {
            ProxyEditor(port: c.settings.socksPort, palette: palette)
                .environmentObject(c)
        }
        .sheet(isPresented: $setupOpen) {
            SetupWizard(palette: palette, serverPurchased: setupPurchased).environmentObject(c)
        }
        .onChange(of: setupOpen) { if !$0 { setupPurchased = false } }
        .sheet(isPresented: $transferOpen) {
            ProfileTransferView(serverID: transferServerID, initialPayload: transferPayload, palette: palette).environmentObject(c)
        }
        .onOpenURL { url in
            guard url.scheme == "secrettunnel", url.host == "import", !transferOpen else { return }
            guard url.absoluteString.utf8.count <= 1_048_576 else { c.message = "Ссылка переноса больше 1 МБ."; return }
            transferServerID = nil; transferPayload = url.absoluteString; transferOpen = true
        }
    }

    private var header: some View {
        HStack(spacing: 12) {
            Text("Secret Tunnel")
                .font(.title2.weight(.semibold))
                .accessibilityAddTraits(.isHeader)
            Spacer(minLength: 0)
            Text(status)
                .font(.caption.weight(.semibold))
                .foregroundColor(statusColor)
                .padding(.horizontal, 12)
                .padding(.vertical, 8)
                .background(Capsule().fill(palette.raised))
        }
        .padding(.horizontal, 20)
        .padding(.top, 12)
        .padding(.bottom, 10)
    }

    private var bottomBar: some View {
        HStack(spacing: 2) {
            ForEach(Page.allCases, id: \.self) { tab in
                Button {
                    page = tab
                    morePage = nil
                } label: {
                    VStack(spacing: 4) {
                        Image(systemName: tab.icon)
                            .font(.system(size: 21, weight: .medium))
                        Text(tab.rawValue)
                            .font(.caption2.weight(page == tab ? .semibold : .regular))
                            .lineLimit(1)
                            .minimumScaleFactor(0.72)
                    }
                    .frame(maxWidth: .infinity, minHeight: 56)
                    .contentShape(Rectangle())
                    .background(RoundedRectangle(cornerRadius: 15).fill(page == tab ? palette.raised : .clear))
                    .foregroundColor(page == tab ? palette.accent : palette.muted)
                }
                .buttonStyle(.plain)
                .accessibilityLabel(tab.rawValue)
                .accessibilityAddTraits(page == tab ? .isSelected : [])
            }
        }
        .padding(.horizontal, 8)
        .padding(.vertical, 8)
        .background(palette.surface.ignoresSafeArea(edges: .bottom))
    }

    @ViewBuilder private var currentPage: some View {
        switch page {
        case .home: home
        case .apps: apps
        case .speed: speed
        case .catalog: EmptyView()
        case .more:
            if let morePage {
                Button { self.morePage = nil } label: {
                    Label("Ещё", systemImage: "chevron.left")
                        .font(.subheadline.weight(.semibold))
                        .frame(minHeight: 44)
                        .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .foregroundColor(palette.accent)
                heading(morePage.rawValue)
                switch morePage {
                case .stats: stats
                case .servers: servers
                case .proxies: proxies
                case .settings: settings
                case .logs: logs
                case .about: about
                }
            } else {
                more
            }
        }
    }

    private var status: String {
        if c.busy { return "Проверка" }
        if c.isConnected { return "Подключено" }
        if c.vpnStatus == .reasserting { return "Переподключение" }
        if c.startingVPN || c.vpnStatus == .connecting { return "Подключение" }
        if c.vpnStatus == .disconnecting { return "Остановка" }
        if !c.connectionError.isEmpty { return "Ошибка" }
        return c.simulator ? "Симулятор" : "Остановлен"
    }

    private var statusColor: Color {
        if c.isConnected { return palette.accent }
        if c.busy || c.startingVPN || c.isActive || c.vpnStatus == .disconnecting { return palette.warning }
        return c.connectionError.isEmpty ? palette.muted : palette.error
    }

    private var heroTitle: String {
        if c.busy { return "Проверяем сервер" }
        if c.isConnected { return "Туннель работает" }
        if c.startingVPN || c.isActive || c.vpnStatus == .disconnecting { return status }
        if !c.connectionError.isEmpty { return "Не удалось подключиться" }
        if !c.isConfigured { return "Настройте подключение" }
        return c.simulator ? "Проверьте SSH-соединение" : "Готово к подключению"
    }

    private var heroDetail: String {
        if !c.connectionError.isEmpty { return c.connectionError }
        if c.isConnected { return "Трафик устройства идёт через SSH-сервер. Недоступный на сервере IPv6 блокируется." }
        if c.busy { return c.message }
        if !c.isConfigured { return "Добавьте SSH-сервер, импортируйте ключ и подтвердите отпечаток." }
        if c.simulator { return "В симуляторе доступны SSH и проверка агента. Для VPN нужен iPhone или iPad." }
        return "Подключите VPN, чтобы направить трафик устройства через SSH-сервер."
    }

    private var home: some View {
        Group {
            card(raised: true) {
                heading(heroTitle)
                detail(heroDetail)
                homeAction.padding(.top, 4)
            }
            if !c.message.isEmpty && !c.busy {
                card {
                    Text(c.message).font(.subheadline)
                }
            }
            rowLink("Основной прокси", addressSummary, icon: "arrow.left.arrow.right") { go(.proxies) }
            trafficCards
            chart
        }
    }

    @ViewBuilder private var homeAction: some View {
        if c.isActive {
            action("Остановить", primary: true, c.toggle)
        } else if !c.isConfigured {
            action("Настроить подключение", primary: true) { setupOpen = true }
                .disabled(!c.canEditServer || speedCheck.running)
        } else if c.simulator {
            action("Проверить вход и агент", primary: true, c.checkServer)
                .disabled(c.busy || speedCheck.running)
        } else {
            action("Подключить", primary: true, c.toggle)
                .disabled(c.startingVPN || c.busy || c.vpnStatus == .disconnecting)
        }
    }

    private var addressSummary: String {
        guard c.hasEndpoint else { return "Адрес не указан" }
        return hideSensitive && !showAddress ? "Адрес скрыт" : "\(c.settings.host):\(c.settings.port)"
    }

    private var apps: some View {
        Group {
            heading("Приложения")
            card {
                Label("VPN для всего устройства", systemImage: "shield")
                    .font(.headline)
                detail("Этот VPN охватывает IPv4- и IPv6-трафик устройства. Для выбора отдельных приложений iOS требует, чтобы ими управлял MDM.")
                action("К подключению") { page = .home }
            }
        }
    }

    private var speed: some View {
        Group {
            heading("Скорость")
            card {
                Text("Быстрый тест").font(.headline)
                if !SpeedCheck.available {
                    detail("Для теста через SOCKS5 нужен iOS 17 или новее.")
                } else {
                    detail(c.simulator ? "HTTPS через SSH и агент. Тест передаёт до 3 МБ." : "HTTPS через локальный SOCKS5. Тест передаёт до 3 МБ.")
                    if let result = speedCheck.result {
                        valueRow("Загрузка", String(format: "%.1f Мбит/с", result.down))
                        valueRow("Отправка", String(format: "%.1f Мбит/с", result.up))
                    }
                    if !speedCheck.error.isEmpty {
                        Text(speedCheck.error).font(.subheadline).foregroundColor(palette.error)
                    }
                    if c.simulator ? c.isConfigured : c.isConnected {
                        action(speedCheck.running ? "Измеряем скорость…" : "Начать тест", primary: true) {
                            speedCheck.start(settings: c.settings, simulator: c.simulator)
                        }
                        .disabled(speedCheck.running || c.busy)
                    } else {
                        action(c.simulator ? "Настроить сервер" : "К подключению") {
                            if c.simulator { go(.servers) } else { page = .home }
                        }
                    }
                }
            }
            trafficCards
            chart
        }
    }

    private var stats: some View {
        Group {
            heading("Статистика")
            card {
                Text("За сеанс VPN").font(.headline)
                valueRow("Получено", bytes(Double(c.rx)))
                valueRow("Отправлено", bytes(Double(c.tx)))
                if c.rx == 0, c.tx == 0 {
                    detail(c.simulator ? "Счётчики VPN доступны при подключении на iPhone или iPad." : "Данные появятся после подключения VPN.")
                }
            }
        }
    }

    private var more: some View {
        Group {
            heading("Ещё")
            detail("Подключения, настройки и журнал")
            rowLink("Статистика", "Трафик текущего сеанса", icon: "chart.bar") { go(.stats) }
            rowLink("Серверы", "SSH, ключ доступа и проверка агента", icon: "server.rack") { go(.servers) }
            rowLink("Прокси", "Основной прокси и локальный порт SOCKS5", icon: "arrow.left.arrow.right") { go(.proxies) }
            rowLink("Настройки", "Оформление и конфиденциальность", icon: "gearshape") { go(.settings) }
            rowLink("Журнал", "Диагностика подключения и ошибок", icon: "list.bullet.rectangle") { go(.logs) }
            rowLink("О программе", "Версия \(version) и справка", icon: "info.circle") { go(.about) }
        }
    }

    private var servers: some View {
        Group {
            ForEach(Shared.profiles) { profile in
                card {
                    Text(profile.name).font(.headline)
                    if let jump = profile.settings.jumpProfileID, let parent = Shared.profiles.first(where: { $0.id == jump }) {
                        Text("Через \(parent.name)").font(.footnote).foregroundColor(palette.muted)
                    }
                    if Shared.selectedProfile?.id == profile.id {
                        Label("Выбран для подключения", systemImage: "checkmark.circle.fill").foregroundColor(palette.accent)
                    } else {
                        action("Использовать") { c.selectServer(profile.id) }
                            .disabled(!c.canEditServer || speedCheck.running)
                    }
                    action("Поделиться") { transferServerID = profile.id; transferPayload = ""; transferOpen = true }
                        .disabled(c.busy || speedCheck.running)
                }
            }
            card {
                Text("Сервер SSH").font(.headline)
                valueRow("Адрес", addressSummary)
                valueRow("Ключ", c.hasKey ? "Импортирован" : "Не импортирован")
                if c.hasEndpoint && hideSensitive {
                    action(showAddress ? "Скрыть адрес" : "Показать адрес") { showAddress.toggle() }
                }
                action("Мастер настройки", primary: !c.hasEndpoint) { setupOpen = true }
                    .disabled(!c.canEditServer || speedCheck.running)
                action("Каталог серверов") { page = .catalog; morePage = nil }
                action("Импортировать") { transferServerID = nil; transferPayload = ""; transferOpen = true }
                    .disabled(!c.canEditServer || speedCheck.running)
                action(c.hasEndpoint ? "Изменить сервер" : "Добавить сервер") {
                    editingServer = true
                }
                .disabled(!c.canEditServer || speedCheck.running)
                action(c.hasKey ? "Заменить приватный ключ" : "Импортировать приватный ключ") {
                    importingKey = true
                }
                .disabled(!c.canEditServer || speedCheck.running)
            }
            card {
                Text("Доверие к серверу").font(.headline)
                valueRow("Отпечаток", c.settings.hostKeySHA256.isEmpty ? "Не подтверждён" : "Подтверждён")
                if !c.pendingFingerprint.isEmpty {
                    Text(c.pendingFingerprint).font(.footnote.monospaced()).textSelection(.enabled)
                    action("Отпечаток совпадает — доверять", primary: true, c.trustFingerprint)
                        .disabled(!c.canEditServer || speedCheck.running)
                }
                action("Получить отпечаток сервера", c.probeServer)
                    .disabled(!c.canEditServer || !c.hasEndpoint || speedCheck.running)
                action("Проверить вход и агент", primary: c.isConfigured, c.checkServer)
                    .disabled(c.busy || !c.isConfigured || speedCheck.running)
                if !c.message.isEmpty { detail(c.message) }
            }
        }
    }

    private var proxies: some View {
        card {
            Text("Основной прокси").font(.headline)
            valueRow("Локальный порт SOCKS5", String(c.settings.socksPort))
            rowLink("Сервер SSH", addressSummary, icon: "server.rack") { go(.servers) }
            action("Изменить порт") { editingPort = true }
                .disabled(!c.canEditServer || speedCheck.running)
        }
    }

    private var settings: some View {
        Group {
            card {
                Text("Оформление").font(.headline)
                Menu {
                    ForEach(Appearance.allCases, id: \.self) { theme in
                        Button {
                            appearance = theme
                        } label: {
                            if theme == appearance {
                                Label(theme.label, systemImage: "checkmark")
                            } else {
                                Text(theme.label)
                            }
                        }
                    }
                } label: {
                    Text("Тема · \(appearance.label)")
                        .font(.subheadline.weight(.semibold))
                        .frame(maxWidth: .infinity, minHeight: 48)
                        .foregroundColor(palette.accent)
                        .background(RoundedRectangle(cornerRadius: 14).fill(palette.raised))
                }
            }
            card {
                Text("Конфиденциальность").font(.headline)
                Toggle("Скрывать IP-адреса", isOn: $hideSensitive)
                    .tint(palette.accent)
                    .onChange(of: hideSensitive) { _ in showAddress = false }
                detail("Скрывает адрес сервера на главной и на экране «Серверы».")
            }
        }
    }

    private var logs: some View {
        card {
            if c.message.isEmpty && c.connectionError.isEmpty && c.logs.isEmpty {
                detail("Записей пока нет. Здесь появятся результаты проверки SSH и сообщения VPN.")
            }
            if !c.message.isEmpty { detail(c.message) }
            if !c.connectionError.isEmpty { Text(c.connectionError).foregroundColor(palette.error) }
            ForEach(Array(c.logs.suffix(30).enumerated()), id: \.offset) { _, line in
                Text(line).font(.footnote.monospaced()).textSelection(.enabled)
            }
        }
    }

    private var version: String {
        Bundle.main.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String ?? ""
    }

    private var about: some View {
        card {
            Text("Secret Tunnel").font(.headline)
            detail("Версия \(version)")
            Link(destination: URL(string: "https://openm1ke.github.io/Secret-Tunnel/")!) {
                Label("Сайт и справка", systemImage: "arrow.up.right")
                    .font(.subheadline.weight(.semibold))
                    .frame(minHeight: 44)
                    .foregroundColor(palette.accent)
            }
        }
    }

    @ViewBuilder private var trafficCards: some View {
        if textSize.isAccessibilitySize {
            VStack(spacing: 12) { download; upload }
        } else {
            HStack(spacing: 12) { download; upload }
        }
    }

    private var download: some View {
        metric("Загрузка", icon: "arrow.down", rate: c.currentTraffic?.down ?? 0, total: c.rx, color: palette.accent)
    }

    private var upload: some View {
        metric("Отправка", icon: "arrow.up", rate: c.currentTraffic?.up ?? 0, total: c.tx, color: palette.upload)
    }

    private func metric(_ title: String, icon: String, rate: Double, total: Int64, color: Color) -> some View {
        card {
            Label(title, systemImage: icon).font(.footnote.weight(.semibold)).foregroundColor(color)
            Text("\(bytes(rate))/с").font(.title2.weight(.semibold).monospacedDigit())
            detail("Всего: \(bytes(Double(total)))")
        }
        .accessibilityElement(children: .combine)
    }

    private var chart: some View {
        card {
            Text("График скорости").font(.headline)
            let peak = max(1024, c.traffic.map { max($0.down, $0.up) }.max() ?? 0)
            detail("\(bytes(peak))/с")
            Canvas { context, size in
                for fraction in [0.0, 0.5, 1.0] {
                    let y = size.height * fraction
                    let line = Path { path in
                        path.move(to: CGPoint(x: 0, y: y))
                        path.addLine(to: CGPoint(x: size.width, y: y))
                    }
                    context.stroke(line, with: .color(palette.border), lineWidth: 1)
                }
                guard c.traffic.count > 1, let end = c.traffic.last?.time else { return }
                for (value, color): (KeyPath<TrafficSample, Double>, Color) in [
                    (\.down, palette.accent), (\.up, palette.upload)
                ] {
                    let line = Path { path in
                        for (index, sample) in c.traffic.enumerated() {
                            let point = CGPoint(
                                x: (sample.time - end + 60) / 60 * size.width,
                                y: (1 - sample[keyPath: value] / peak) * size.height
                            )
                            if index == 0 { path.move(to: point) } else { path.addLine(to: point) }
                        }
                    }
                    context.stroke(line, with: .color(color), style: StrokeStyle(lineWidth: 2, lineCap: .round, lineJoin: .round))
                }
            }
            .frame(height: 60)
            .accessibilityHidden(true)
            HStack {
                detail("−60 с")
                Spacer()
                detail("Последний замер")
            }
        }
    }

    private func go(_ destination: MorePage) {
        page = .more
        morePage = destination
        showAddress = false
    }

    private func heading(_ title: String) -> some View {
        Text(title)
            .font(.title2.weight(.semibold))
            .accessibilityAddTraits(.isHeader)
    }

    private func detail(_ text: String) -> some View {
        Text(text).font(.subheadline).foregroundColor(palette.muted)
    }

    private func valueRow(_ title: String, _ value: String) -> some View {
        HStack(alignment: .firstTextBaseline, spacing: 12) {
            Text(title).font(.subheadline)
            Spacer(minLength: 0)
            detail(value).multilineTextAlignment(.trailing)
        }
        .accessibilityElement(children: .combine)
    }

    private func card<Content: View>(raised: Bool = false, @ViewBuilder content: () -> Content) -> some View {
        VStack(alignment: .leading, spacing: 8, content: content)
            .padding(16)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(RoundedRectangle(cornerRadius: 22).fill(raised ? palette.raised : palette.surface))
    }

    private func action(_ title: String, primary: Bool = false, _ run: @escaping () -> Void) -> some View {
        Button(title, action: run)
            .buttonStyle(MobileButtonStyle(palette: palette, primary: primary))
    }

    private func rowLink(_ title: String, _ subtitle: String, icon: String, _ run: @escaping () -> Void) -> some View {
        Button(action: run) {
            card {
                HStack(spacing: 12) {
                    Image(systemName: icon).foregroundColor(palette.accent).accessibilityHidden(true)
                    VStack(alignment: .leading, spacing: 4) {
                        Text(title).font(.headline).foregroundColor(palette.accent)
                        detail(subtitle)
                    }
                    Spacer(minLength: 0)
                    Image(systemName: "chevron.right").font(.caption).foregroundColor(palette.muted)
                        .accessibilityHidden(true)
                }
            }
        }
        .buttonStyle(.plain)
        .accessibilityElement(children: .combine)
    }
}

private func bytes(_ value: Double) -> String {
    if value >= 1_048_576 { return String(format: "%.1f MiB", value / 1_048_576) }
    if value >= 1_024 { return String(format: "%.1f KiB", value / 1_024) }
    return "\(Int64(value)) B"
}
