import SwiftUI
import UniformTypeIdentifiers

private enum SetupStage: Int { case server, access, details, trust, finish }

@MainActor
final class SSHSetupModel: ObservableObject {
    @Published fileprivate var stage = SetupStage.server
    @Published var hasServer = true
    @Published var useKey = false
    @Published var host = ""
    @Published var name = ""
    @Published var port = "22"
    @Published var user = "root"
    @Published var password = ""
    @Published var keyLabel = ""
    @Published var fingerprint = ""
    @Published private(set) var busy = false
    @Published private(set) var error = ""
    @Published private(set) var logs: [String] = []
    @Published private(set) var saved = false
    @Published private(set) var done = false
    private var key: Data?

    var valid: Bool {
        !host.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty &&
        !user.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty &&
        (1...65535).contains(Int(port) ?? 0) && (useKey ? key != nil : !password.isEmpty)
    }

    private func log(_ value: String) { logs = Array((logs + [value]).suffix(25)) }
    private func result(_ text: String) throws -> [String: Any] {
        let value = jsonObject(text)
        if let message = value["error"] as? String { throw SetupFailure(message) }
        guard !value.isEmpty else { throw SetupFailure("Проверка не вернула результат. Повторите попытку.") }
        return value
    }
    func clear() { password = ""; key = nil }
    func chooseKey(_ enabled: Bool) {
        clear(); keyLabel = ""; useKey = enabled; stage = .details; error = ""
    }
    func back() {
        guard !busy, !saved else { return }
        stage = stage == .finish || stage == .trust ? .details : SetupStage(rawValue: max(0, stage.rawValue - 1))!
        fingerprint = ""; error = ""
    }
    func importKey(_ url: URL) {
        let scoped = url.startAccessingSecurityScopedResource()
        defer { if scoped { url.stopAccessingSecurityScopedResource() } }
        do {
            let size = try url.resourceValues(forKeys: [.fileSizeKey]).fileSize ?? 0
            guard size > 0, size <= 1_048_576 else { throw SetupFailure("Выберите файл ключа размером до 1 МБ.") }
            let data = try Data(contentsOf: url)
            guard String(decoding: data, as: UTF8.self).contains("PRIVATE KEY") else {
                throw SetupFailure("Выберите приватный SSH-ключ, файл без .pub.")
            }
            key = data; keyLabel = url.lastPathComponent; error = ""
        } catch { self.error = error.localizedDescription }
    }
    func probe() async {
        guard !busy, valid else { return }
        busy = true; error = ""; fingerprint = ""
        defer { busy = false }
        log("Подключаемся к SSH-серверу…")
        var settings = TunnelSettings()
        settings.host = host; settings.port = Int(port)!; settings.user = user
        let config = settings.rustConfig(keyPath: "")
        do {
            let raw = await Task.detached { RustCore.probe(config) }.value
            let value = try result(raw)
            guard let pin = value["fingerprint"] as? String, pin.hasPrefix("SHA256:"), pin.count == 50 else {
                throw SetupFailure("Сервер вернул некорректный отпечаток.")
            }
            fingerprint = pin; stage = .trust
            log("Сервер ответил. Получен отпечаток ключа.")
        } catch { self.error = error.localizedDescription; log("Проверка остановлена. Проверьте данные и повторите попытку.") }
    }
    func configure(install: Bool = false) async {
        guard !busy, fingerprint.hasPrefix("SHA256:"), fingerprint.count == 50 else { return }
        busy = true; error = ""; stage = .finish
        defer { busy = false }
        do {
            if !saved {
                if !useKey && key == nil {
                    log("Создаём отдельный ключ приложения…")
                    let value = try result(await Task.detached { RustCore.generateKey() }.value)
                    guard let privateKey = value["private"] as? String else { throw SetupFailure("Не удалось создать ключ.") }
                    key = Data(privateKey.utf8)
                }
                guard let key else { throw SetupFailure("Выберите приватный SSH-ключ.") }
                let temporary = FileManager.default.temporaryDirectory.appendingPathComponent("ssh-setup-\(UUID().uuidString).key")
                defer { try? FileManager.default.removeItem(at: temporary) }
                try key.write(to: temporary, options: [.atomic, .completeFileProtectionUntilFirstUserAuthentication])
                try FileManager.default.setAttributes([.posixPermissions: 0o600], ofItemAtPath: temporary.path)
                var settings = Shared.loadSettings()
                settings.host = host.trimmingCharacters(in: .whitespacesAndNewlines)
                settings.port = Int(port) ?? 0
                settings.user = user.trimmingCharacters(in: .whitespacesAndNewlines)
                settings.hostKeySHA256 = fingerprint
                var config = jsonObject(settings.rustConfig(keyPath: temporary.path))
                config["check_agent"] = false
                let json = String(decoding: try JSONSerialization.data(withJSONObject: config), as: UTF8.self)
                if useKey {
                    log("Проверяем вход по выбранному ключу…")
                    _ = try result(await Task.detached { RustCore.check(json) }.value)
                } else {
                    log("Входим по паролю и добавляем публичный ключ…")
                    let secret = password
                    _ = try result(await Task.detached { RustCore.enroll(json, password: secret) }.value)
                    log("Повторный вход по новому ключу прошёл.")
                }
                log("Сохраняем подключение…")
                try Shared.saveSetup(settings, key: key, name: name)
                saved = true; clear()
            }
            let config = Shared.loadSettings().rustConfig(keyPath: Shared.keyURL.path)
            if install {
                log("Скачиваем агент и сверяем SHA-256…")
                let version = Bundle.main.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String ?? ""
                let archive = try await SetupAgentRelease.download(version: version)
                defer { try? FileManager.default.removeItem(at: archive) }
                log("Устанавливаем агент в домашнюю папку пользователя…")
                _ = try result(await Task.detached { RustCore.installAgent(config, version: version, archive: archive.path) }.value)
            }
            log("Проверяем серверный агент и поддержку TCP/UDP…")
            _ = try result(await Task.detached { RustCore.check(config) }.value)
            done = true; log("Подключение проверено и сохранено.")
        } catch { self.error = error.localizedDescription; log("Настройка остановлена. Исправьте причину и повторите попытку.") }
    }
}

struct SetupFailure: LocalizedError {
    let message: String
    init(_ message: String) { self.message = message }
    var errorDescription: String? { message }
}

struct SetupWizard: View {
    @EnvironmentObject private var controller: TunnelController
    @Environment(\.dismiss) private var dismiss
    @StateObject private var model = SSHSetupModel()
    @State private var importing = false
    let palette: MobilePalette

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 16) {
                HStack {
                    Text("Настройка подключения").font(.title2.weight(.semibold))
                    Spacer()
                    Button("Закрыть") { dismiss() }.disabled(model.busy)
                }
                if model.stage != .finish {
                    Text("Шаг \(min(model.stage.rawValue + 1, 4)) из 4").font(.caption).foregroundColor(palette.muted)
                }
                content
                if !model.error.isEmpty {
                    Text(model.error).foregroundColor(palette.error).accessibilityIdentifier("setup.error")
                }
                if !model.logs.isEmpty {
                    VStack(alignment: .leading, spacing: 6) {
                        if model.busy { ProgressView().tint(palette.accent) }
                        Text("Ход настройки").font(.subheadline.weight(.semibold))
                        ForEach(Array(model.logs.enumerated()), id: \.offset) { _, line in
                            Text(line).font(.footnote).foregroundColor(palette.muted).fixedSize(horizontal: false, vertical: true)
                        }
                    }
                    .padding(14).frame(maxWidth: .infinity, alignment: .leading)
                    .background(RoundedRectangle(cornerRadius: 14).fill(palette.surface))
                }
                if model.stage != .server && !model.saved {
                    button("Назад") { model.back() }
                }
            }
            .padding(20).frame(maxWidth: 620).frame(maxWidth: .infinity)
        }
        .background(palette.background.ignoresSafeArea()).foregroundColor(palette.text).tint(palette.accent)
        .interactiveDismissDisabled(model.busy)
        .fileImporter(isPresented: $importing, allowedContentTypes: [.data, .text, .item]) { result in
            if case let .success(url) = result { model.importKey(url) }
        }
        .onChange(of: model.busy) { controller.busy = $0 }
        .onChange(of: model.saved) { saved in
            if saved { controller.settings = Shared.loadSettings(); controller.hasKey = Shared.hasKey }
        }
        .onDisappear { model.clear(); controller.busy = false }
    }

    @ViewBuilder private var content: some View {
        switch model.stage {
        case .server:
            Text("У вас уже есть сервер?").font(.title3.weight(.semibold))
            Text("Для подключения нужен ваш VPS с доступом по SSH.")
            button("Да, сервер уже куплен", primary: true) { model.stage = .access }
            button("Пока нет сервера") { model.hasServer = false }
            if !model.hasServer { Text("Выберите VPS с Linux x86_64 у своего хостера. После покупки понадобятся адрес, SSH-порт и данные для входа.").foregroundColor(palette.muted) }
        case .access:
            Text("Как вы входите на сервер?").font(.title3.weight(.semibold))
            button("Логин и пароль от хостера", primary: true) { model.chooseKey(false) }
            button("У меня есть SSH-ключ") { model.chooseKey(true) }
        case .details:
            field("Адрес сервера", text: $model.host, keyboard: .URL)
            field("Порт SSH", text: $model.port, keyboard: .numberPad)
            field("Пользователь", text: $model.user)
            if model.useKey {
                Text(model.keyLabel.isEmpty ? "Выберите приватный SSH-ключ, файл без .pub." : model.keyLabel).font(.footnote)
                button("Выбрать файл") { importing = true }
            } else {
                VStack(alignment: .leading, spacing: 6) {
                    Text("Пароль сервера").font(.subheadline.weight(.semibold))
                    SecureField("Пароль сервера", text: $model.password).textContentType(.password)
                        .padding(12).background(RoundedRectangle(cornerRadius: 12).fill(palette.surface))
                        .accessibilityIdentifier("setup.password")
                }.disabled(model.busy)
                Text("Приложение создаст отдельный ключ. Пароль не сохраняется.").font(.footnote).foregroundColor(palette.muted)
            }
            field("Название сервера · необязательно", text: $model.name)
            button("Проверить адрес", primary: true) { Task { await model.probe() } }.disabled(!model.valid)
        case .trust:
            Text("Подтвердите сервер").font(.title3.weight(.semibold))
            Text("Сверьте отпечаток с данными в панели хостера. Он защищает от подключения к подменённому серверу.")
            Text(model.fingerprint).font(.footnote.monospaced()).textSelection(.enabled).padding(14)
                .frame(maxWidth: .infinity, alignment: .leading)
                .background(RoundedRectangle(cornerRadius: 12).fill(palette.surface))
            button("Доверять и настроить", primary: true) { Task { await model.configure() } }
        case .finish:
            if model.done {
                Label("Подключение готово", systemImage: "checkmark.shield.fill").font(.title3.weight(.semibold))
                Text("Теперь можно подключить туннель на главной странице.")
                button("Готово", primary: true) { dismiss() }
            } else if !model.busy {
                if model.saved {
                    Text("Вход по ключу проверен. Для передачи трафика нужен серверный агент.")
                    button("Установить агент и проверить", primary: true) { Task { await model.configure(install: true) } }
                }
                button("Повторить проверку") { Task { await model.configure() } }
            } else { Text("Настраиваем подключение…").font(.title3.weight(.semibold)) }
        }
    }
    private func button(_ title: String, primary: Bool = false, _ action: @escaping () -> Void) -> some View {
        Button(title, action: action).buttonStyle(MobileButtonStyle(palette: palette, primary: primary)).disabled(model.busy)
    }
    private func field(_ title: String, text: Binding<String>, keyboard: UIKeyboardType = .default) -> some View {
        VStack(alignment: .leading, spacing: 6) {
            Text(title).font(.subheadline.weight(.semibold))
            TextField(title, text: text).keyboardType(keyboard).textInputAutocapitalization(.never).disableAutocorrection(true)
                .padding(12).background(RoundedRectangle(cornerRadius: 12).fill(palette.surface))
                .accessibilityIdentifier("setup.\(title)")
        }.disabled(model.busy)
    }
}
