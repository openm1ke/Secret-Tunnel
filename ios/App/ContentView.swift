import NetworkExtension
import SwiftUI
import UniformTypeIdentifiers

@main
struct SecretTunnelApp: App {
    @StateObject private var controller = TunnelController()

    var body: some Scene {
        WindowGroup {
            ContentView().environmentObject(controller)
        }
    }
}

struct ContentView: View {
    @EnvironmentObject private var c: TunnelController
    @State private var importing = false

    var body: some View {
        NavigationView {
            Form {
                Section("Сервер SSH") {
                    TextField("Адрес", text: $c.settings.host)
                        .textInputAutocapitalization(.never).disableAutocorrection(true)
                        .keyboardType(.URL)
                    TextField("Порт", value: $c.settings.port, format: .number.grouping(.never))
                        .keyboardType(.numberPad)
                    TextField("Пользователь", text: $c.settings.user)
                        .textInputAutocapitalization(.never).disableAutocorrection(true)
                    Button(c.hasKey ? "Заменить приватный ключ" : "Импортировать приватный ключ") { importing = true }
                    LabeledRow(title: "Ключ", value: c.hasKey ? "импортирован" : "нет")
                }

                Section("Доверие к серверу") {
                    LabeledRow(title: "Отпечаток", value: c.settings.hostKeySHA256.isEmpty ? "не подтверждён" : c.settings.hostKeySHA256)
                    Button("Получить отпечаток сервера", action: c.probeServer).disabled(c.busy)
                    if !c.pendingFingerprint.isEmpty {
                        Text(c.pendingFingerprint).font(.footnote.monospaced()).textSelection(.enabled)
                        Button("Отпечаток совпадает — доверять", action: c.trustFingerprint)
                    }
                    Button("Проверить вход и агент", action: c.checkServer)
                        .disabled(c.busy || !c.hasKey || c.settings.hostKeySHA256.isEmpty)
                }

                Section("VPN") {
                    LabeledRow(title: "Состояние", value: c.vpnStatus.label)
                    Button(c.isActive ? "Отключить" : "Подключить весь трафик", action: c.toggle)
                        .disabled(c.busy)
                    if c.isActive {
                        LabeledRow(title: "Получено", value: ByteCountFormatter.string(fromByteCount: c.rx, countStyle: .binary))
                        LabeledRow(title: "Отправлено", value: ByteCountFormatter.string(fromByteCount: c.tx, countStyle: .binary))
                    }
                    if !c.tunnelError.isEmpty {
                        Text(c.tunnelError).foregroundColor(.red).font(.footnote)
                    }
                }

                if !c.message.isEmpty {
                    Section { Text(c.message).font(.footnote) }
                }

                if !c.logs.isEmpty {
                    Section("Журнал") {
                        ForEach(Array(c.logs.suffix(30).enumerated()), id: \.offset) { _, line in
                            Text(line).font(.caption2.monospaced())
                        }
                    }
                }
            }
            .navigationTitle("Secret Tunnel")
            .onDisappear(perform: c.saveSettings)
            .fileImporter(isPresented: $importing, allowedContentTypes: [.data, .text, .item]) { result in
                if case let .success(url) = result { c.importKey(url) }
            }
        }
        .navigationViewStyle(.stack)
    }
}

private struct LabeledRow: View {
    let title: String
    let value: String

    var body: some View {
        HStack(alignment: .firstTextBaseline) {
            Text(title)
            Spacer()
            Text(value).foregroundColor(.secondary).multilineTextAlignment(.trailing).lineLimit(3)
        }
    }
}

extension NEVPNStatus {
    var label: String {
        switch self {
        case .invalid: return "не настроен"
        case .disconnected: return "отключён"
        case .connecting: return "подключение…"
        case .connected: return "подключён"
        case .reasserting: return "переподключение…"
        case .disconnecting: return "отключение…"
        @unknown default: return "неизвестно"
        }
    }
}
