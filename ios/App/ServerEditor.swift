import SwiftUI

struct ServerEditor: View {
    @EnvironmentObject private var controller: TunnelController
    @Environment(\.dismiss) private var dismiss
    @State private var host: String
    @State private var port: String
    @State private var user: String
    @State private var jumpID: String
    let palette: MobilePalette

    init(settings: TunnelSettings, palette: MobilePalette) {
        self.palette = palette
        _host = State(initialValue: settings.host)
        _port = State(initialValue: String(settings.port))
        _user = State(initialValue: settings.user)
        _jumpID = State(initialValue: settings.jumpProfileID ?? "")
    }

    private var isValid: Bool {
        !host.trimmingCharacters(in: .whitespaces).isEmpty &&
        !user.trimmingCharacters(in: .whitespaces).isEmpty &&
        (1...65535).contains(Int(port) ?? 0)
    }

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 16) {
                Text("Сервер SSH").font(.title2.weight(.semibold)).accessibilityAddTraits(.isHeader)
                field("Адрес", text: $host, keyboard: .URL)
                field("Порт", text: $port, keyboard: .numberPad)
                field("Пользователь", text: $user)
                Picker("Подключаться через", selection: $jumpID) {
                    Text("Напрямую").tag("")
                    ForEach(Shared.profiles.filter { $0.id != Shared.selectedProfile?.id }) { profile in Text(profile.name).tag(profile.id) }
                }
                if !controller.message.isEmpty {
                    Text(controller.message).font(.footnote).foregroundColor(palette.muted)
                }
                Button("Сохранить") {
                    if let port = Int(port), controller.editServer(host: host, port: port, user: user, jumpID: jumpID.isEmpty ? nil : jumpID) {
                        dismiss()
                    }
                }
                .buttonStyle(MobileButtonStyle(palette: palette, primary: true))
                .disabled(!isValid)
                Button("Отмена") { dismiss() }
                    .buttonStyle(MobileButtonStyle(palette: palette))
            }
            .padding(20)
            .frame(maxWidth: 620)
            .frame(maxWidth: .infinity)
        }
        .foregroundColor(palette.text)
        .background(palette.background.ignoresSafeArea())
    }

    private func field(_ title: String, text: Binding<String>, keyboard: UIKeyboardType = .default) -> some View {
        VStack(alignment: .leading, spacing: 6) {
            Text(title).font(.subheadline.weight(.semibold)).foregroundColor(palette.muted)
            TextField(title, text: text)
                .keyboardType(keyboard)
                .textInputAutocapitalization(.never)
                .disableAutocorrection(true)
                .padding(.horizontal, 12)
                .frame(minHeight: 48)
                .background(RoundedRectangle(cornerRadius: 12).fill(palette.surface))
                .accessibilityLabel(title)
        }
    }
}

struct ProxyEditor: View {
    @EnvironmentObject private var controller: TunnelController
    @Environment(\.dismiss) private var dismiss
    @State private var port: String
    let palette: MobilePalette

    init(port: Int, palette: MobilePalette) {
        self.palette = palette
        _port = State(initialValue: String(port))
    }

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 16) {
                Text("Локальный порт SOCKS5").font(.title2.weight(.semibold))
                TextField("Порт", text: $port)
                    .keyboardType(.numberPad)
                    .textFieldStyle(.roundedBorder)
                Text("От 1024 до 65535").font(.footnote).foregroundColor(palette.muted)
                if !controller.message.isEmpty {
                    Text(controller.message).font(.footnote).foregroundColor(palette.muted)
                }
                Button("Сохранить") {
                    if let port = Int(port), controller.setSocksPort(port) { dismiss() }
                }
                .buttonStyle(MobileButtonStyle(palette: palette, primary: true))
                .disabled(!(1024...65535).contains(Int(port) ?? 0))
                Button("Отмена") { dismiss() }
                    .buttonStyle(MobileButtonStyle(palette: palette))
            }
            .padding(20)
            .frame(maxWidth: 620)
            .frame(maxWidth: .infinity)
        }
        .foregroundColor(palette.text)
        .background(palette.background.ignoresSafeArea())
    }
}
