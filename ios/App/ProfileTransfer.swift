import SwiftUI
import UniformTypeIdentifiers
import CoreImage.CIFilterBuiltins

extension UTType { static let secretTunnelProfile = UTType(exportedAs: "dev.secrettunnel.profile", conformingTo: .data) }
struct EncryptedProfileFile: FileDocument {
    static var readableContentTypes: [UTType] { [.secretTunnelProfile, .data] }
    let text: String
    init(text: String) { self.text = text }
    init(configuration: ReadConfiguration) throws {
        guard let data = configuration.file.regularFileContents, data.count <= 1_048_576 else { throw ProfileTransferFailure("Файл больше 1 МБ.") }
        text = String(decoding: data, as: UTF8.self)
    }
    func fileWrapper(configuration: WriteConfiguration) throws -> FileWrapper { FileWrapper(regularFileWithContents: Data(text.utf8)) }
}

@MainActor final class ProfileTransferModel: ObservableObject {
    @Published var payload = ""
    @Published var code = ""
    @Published private(set) var preview: [[String: Any]]?
    @Published private(set) var exported: [String: Any]?
    @Published private(set) var busy = false
    @Published private(set) var done = false
    @Published var error = ""
    @Published var notice = ""
    var link: String { exported?["link"] as? String ?? "" }
    var transferCode: String { exported?["code"] as? String ?? "" }
    private func result(_ raw: String) throws -> [String: Any] {
        let value = jsonObject(raw)
        if let error = value["error"] as? String { throw ProfileTransferFailure(error) }
        guard !value.isEmpty else { throw ProfileTransferFailure("Не удалось прочитать настройки.") }
        return value
    }
    func prepare(ids: [String]) async {
        guard !busy else { return }; busy = true; error = ""
        defer { busy = false }
        do {
            let document = try Shared.transferDocument(ids: ids)
            exported = try result(await Task.detached { RustCore.transferExport(document) }.value)
        } catch { self.error = error.localizedDescription }
    }
    func unlock() async {
        guard !busy else { return }; busy = true; error = ""
        defer { busy = false }
        do {
            let input = payload, secret = code
            preview = try result(await Task.detached { RustCore.transferUnlock(input, code: secret, preview: true) }.value)["servers"] as? [[String: Any]]
        } catch { self.error = error.localizedDescription }
    }
    func importProfiles() async {
        guard !busy else { return }; busy = true; error = ""
        defer { busy = false }
        do {
            let input = payload, secret = code
            let raw = await Task.detached { RustCore.transferUnlock(input, code: secret, preview: false) }.value
            _ = try result(raw)
            _ = try Shared.importTransferred(raw)
            payload = ""; code = ""; preview = nil; done = true
        } catch { self.error = error.localizedDescription }
    }
    func readFile(_ url: URL) async {
        guard !busy else { return }; busy = true; error = ""
        defer { busy = false }
        do {
            payload = try await Task.detached {
                let scoped = url.startAccessingSecurityScopedResource()
                defer { if scoped { url.stopAccessingSecurityScopedResource() } }
                let size = try url.resourceValues(forKeys: [.fileSizeKey]).fileSize ?? 0
                guard size > 0, size <= 1_048_576 else { throw ProfileTransferFailure("Файл пустой или больше 1 МБ.") }
                let data = try Data(contentsOf: url)
                guard data.count <= 1_048_576 else { throw ProfileTransferFailure("Файл больше 1 МБ.") }
                return String(decoding: data, as: UTF8.self)
            }.value
        } catch { self.error = error.localizedDescription }
    }
    func back() { preview = nil; error = "" }
    func clear() { payload = ""; code = ""; preview = nil; exported = nil }
}

struct ProfileTransferView: View {
    @EnvironmentObject private var controller: TunnelController
    @Environment(\.dismiss) private var dismiss
    @StateObject private var model = ProfileTransferModel()
    @State private var all = false
    @State private var scanning = false
    @State private var importing = false
    @State private var exporting = false
    @State private var showCode = false
    @State private var initialized = false
    let serverID: String?
    var initialPayload = ""
    let palette: MobilePalette
    private var canChange: Bool { !controller.isActive && controller.vpnStatus != .disconnecting && !controller.startingVPN }
    private var configuredIDs: [String] {
        Shared.profiles.filter { !$0.settings.host.isEmpty && !$0.settings.user.isEmpty && $0.settings.hostKeySHA256.count == 50 && FileManager.default.fileExists(atPath: Shared.keyURL(for: $0).path) }.map(\.id)
    }
    private var qr: UIImage? {
        guard model.exported?["qr_available"] as? Bool == true else { return nil }
        let filter = CIFilter.qrCodeGenerator(); filter.message = Data(model.link.utf8); filter.correctionLevel = "M"
        return filter.outputImage.map { UIImage(ciImage: $0.transformed(by: CGAffineTransform(scaleX: 8, y: 8))) }
    }
    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 16) {
                HStack {
                    Text(serverID == nil ? "Импортировать серверы" : "Поделиться серверами").font(.title2.weight(.semibold))
                    Spacer(); Button("Закрыть") { dismiss() }.disabled(model.busy)
                }
                if let id = serverID { exportUI(id: id) } else { importUI }
                if model.busy { ProgressView("Обрабатываем настройки…").tint(palette.accent) }
                if !model.error.isEmpty { Text(model.error).foregroundColor(palette.error).accessibilityIdentifier("transfer.error") }
                if !model.notice.isEmpty { Text(model.notice).foregroundColor(palette.muted) }
            }
            .padding(20).frame(maxWidth: 620).frame(maxWidth: .infinity)
        }
        .background(palette.background.ignoresSafeArea()).foregroundColor(palette.text).tint(palette.accent)
        .interactiveDismissDisabled(model.busy)
        .onAppear { if !initialized { model.payload = initialPayload; initialized = true } }
        .onChange(of: model.busy) { controller.busy = $0 }
        .onChange(of: model.done) { done in if done { controller.settings = Shared.loadSettings(); controller.hasKey = Shared.hasKey } }
        .onDisappear { model.clear(); controller.busy = false }
        .sheet(isPresented: $scanning) { ProfileQRScanner { value in model.payload = value; scanning = false } }
        .fileImporter(isPresented: $importing, allowedContentTypes: [.secretTunnelProfile, .data, .item]) { result in
            if case let .success(url) = result { Task { await model.readFile(url) } }
        }
        .fileExporter(isPresented: $exporting, document: EncryptedProfileFile(text: model.link), contentType: .secretTunnelProfile, defaultFilename: "SecretTunnel-servers") { result in
            switch result { case .success: model.notice = "Зашифрованный файл сохранён"; case let .failure(error): model.error = error.localizedDescription }
        }
    }
    @ViewBuilder private var importUI: some View {
        if model.done {
            Label("Серверы добавлены", systemImage: "checkmark.shield.fill").font(.title3.weight(.semibold))
            Text("Существующие подключения сохранены. Выберите нужный сервер и проверьте соединение.")
            button("Готово", primary: true) { dismiss() }
        } else if !canChange {
            Text("Сначала остановите туннель, чтобы добавить серверы.")
        } else if let servers = model.preview {
            Text("Будут добавлены серверы").font(.title3.weight(.semibold))
            ForEach(Array(servers.enumerated()), id: \.offset) { _, server in
                VStack(alignment: .leading, spacing: 6) {
                    Text(server["name"] as? String ?? "Сервер").font(.headline)
                    Text("\(server["user"] as? String ?? "")@\(server["host"] as? String ?? ""):\(server["port"] as? Int ?? 22)").font(.footnote)
                    if let jump = server["jump_id"] as? String, let parent = servers.first(where: { $0["id"] as? String == jump }) {
                        Text("Через \(parent["name"] as? String ?? "сервер")").font(.footnote)
                    }
                }.padding(14).frame(maxWidth: .infinity, alignment: .leading).background(RoundedRectangle(cornerRadius: 14).fill(palette.surface))
            }
            Text("Добавляйте только настройки от доверенного отправителя. Существующие серверы не будут заменены.").font(.footnote).foregroundColor(palette.muted)
            button("Добавить серверы", primary: true) { Task { await model.importProfiles() } }
            button("Назад") { model.back() }
        } else {
            Text("Вставьте ссылку или откройте файл .stprofile. Код переноса получите у отправителя отдельно.")
            if !controller.simulator { button("Сканировать QR-код", primary: true) { scanning = true } }
            button("Открыть файл") { importing = true }
            Text("Ссылка переноса").font(.subheadline.weight(.semibold))
            TextEditor(text: $model.payload).frame(minHeight: 96, maxHeight: 160).disableAutocorrection(true)
                .textInputAutocapitalization(.never).accessibilityLabel("Ссылка переноса").disabled(model.busy)
                .onChange(of: model.payload) { if $0.utf8.count > 1_048_576 { model.payload = ""; model.error = "Ссылка больше 1 МБ." } }
            Text("Код переноса").font(.subheadline.weight(.semibold))
            TextField("ABCD-EFGH-JKLM-NPQR-STUV", text: $model.code).textInputAutocapitalization(.characters)
                .disableAutocorrection(true).font(.system(.body, design: .monospaced)).padding(12)
                .background(RoundedRectangle(cornerRadius: 12).fill(palette.surface)).disabled(model.busy)
            button("Расшифровать и проверить", primary: true) { Task { await model.unlock() } }.disabled(model.payload.isEmpty || model.code.isEmpty)
        }
    }
    @ViewBuilder private func exportUI(id: String) -> some View {
        if model.exported == nil {
            Text("Подключение включает ключ доступа. Получатель сможет войти на ваш сервер.")
            Toggle("Все настроенные серверы", isOn: $all).disabled(model.busy)
            Text("Промежуточные серверы цепочки добавятся автоматически. Код переноса передайте отдельно.").font(.footnote).foregroundColor(palette.muted)
            button("Подготовить перенос", primary: true) { Task { await model.prepare(ids: all ? configuredIDs : [id]) } }
        } else {
            if let qr { Image(uiImage: qr).interpolation(.none).resizable().scaledToFit().frame(maxWidth: 320).padding(16).background(Color.white).frame(maxWidth: .infinity) }
            else { Text("Настройки слишком велики для QR-кода. Передайте ссылку или файл.") }
            button("Скопировать ссылку") { UIPasteboard.general.setItems([[UIPasteboard.typeAutomatic: model.link]], options: [.localOnly: true, .expirationDate: Date().addingTimeInterval(120)]); model.notice = "Ссылка скопирована" }
            button("Сохранить файл", primary: true) { exporting = true }
            Text("Код переноса").font(.headline)
            Text("Сохраните его отдельно от ссылки или файла. После закрытия окна код этого переноса нельзя восстановить.").font(.footnote).foregroundColor(palette.muted)
            if showCode { Text(model.transferCode).font(.system(.body, design: .monospaced)).textSelection(.enabled) }
            button(showCode ? "Скрыть код" : "Показать код") { showCode.toggle() }
            button("Скопировать код") { UIPasteboard.general.setItems([[UIPasteboard.typeAutomatic: model.transferCode]], options: [.localOnly: true, .expirationDate: Date().addingTimeInterval(120)]); model.notice = "Код переноса скопирован" }
        }
    }
    private func button(_ title: String, primary: Bool = false, action: @escaping () -> Void) -> some View {
        Button(title, action: action).buttonStyle(MobileButtonStyle(palette: palette, primary: primary)).disabled(model.busy)
    }
}
