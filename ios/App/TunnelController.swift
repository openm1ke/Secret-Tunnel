import Foundation
import NetworkExtension

/// Owns the NETunnelProviderManager for our Packet Tunnel extension and polls its status.
@MainActor
final class TunnelController: ObservableObject {
    @Published var settings = Shared.loadSettings()
    @Published var hasKey = Shared.hasKey
    @Published var vpnStatus: NEVPNStatus = .invalid
    @Published var message = ""
    @Published var busy = false
    @Published var pendingFingerprint = ""
    @Published var rx: Int64 = 0
    @Published var tx: Int64 = 0
    @Published var logs: [String] = []
    @Published var tunnelError = ""

    private var manager: NETunnelProviderManager?
    private var statusObserver: NSObjectProtocol?
    private var pollTimer: Timer?

    private var tunnelBundleID: String {
        Bundle.main.object(forInfoDictionaryKey: "STTunnelBundleID") as? String ?? "dev.secrettunnel.ios.PacketTunnel"
    }

    init() {
        Task { await loadManager() }
    }

    var isConnected: Bool { vpnStatus == .connected }
    var isActive: Bool { [.connecting, .connected, .reasserting].contains(vpnStatus) }

    func saveSettings() {
        do { try Shared.save(settings) } catch { message = error.localizedDescription }
    }

    func importKey(_ url: URL) {
        let scoped = url.startAccessingSecurityScopedResource()
        defer { if scoped { url.stopAccessingSecurityScopedResource() } }
        do {
            try Shared.storeKey(from: url)
            hasKey = true
            message = "Ключ импортирован"
        } catch {
            message = error.localizedDescription
        }
    }

    /// Fetches the server host key so the user can compare it before trusting.
    func probeServer() {
        saveSettings()
        let config = settings.rustConfig(keyPath: Shared.keyURL.path)
        busy = true
        message = "Получаем ключ сервера…"
        Task.detached {
            let result = jsonObject(RustCore.probe(config))
            await MainActor.run {
                self.busy = false
                if let pin = result["fingerprint"] as? String {
                    self.pendingFingerprint = pin
                    self.message = "Сверьте отпечаток с сервером и подтвердите"
                } else {
                    self.message = result["error"] as? String ?? "Не удалось получить ключ"
                }
            }
        }
    }

    func trustFingerprint() {
        settings.hostKeySHA256 = pendingFingerprint
        pendingFingerprint = ""
        saveSettings()
        message = "Отпечаток сохранён"
    }

    /// Logs in from the app (not through the VPN) and checks the server agent.
    func checkServer() {
        saveSettings()
        let config = settings.rustConfig(keyPath: Shared.keyURL.path)
        busy = true
        message = "Проверяем вход по ключу и агент…"
        Task.detached {
            let result = jsonObject(RustCore.check(config))
            await MainActor.run {
                self.busy = false
                if result["ok"] as? Bool == true {
                    self.message = result["summary"] as? String ?? "Сервер готов"
                } else {
                    self.message = result["error"] as? String ?? "Проверка не прошла"
                }
            }
        }
    }

    func toggle() {
        if isActive { manager?.connection.stopVPNTunnel() } else { Task { await connect() } }
    }

    private func connect() async {
        saveSettings()
        guard hasKey, !settings.hostKeySHA256.isEmpty else {
            message = "Импортируйте ключ и подтвердите отпечаток сервера"
            return
        }
        do {
            let manager = try await preparedManager()
            try manager.connection.startVPNTunnel()
            message = "Подключаемся…"
        } catch {
            message = "Не удалось запустить VPN: \(error.localizedDescription)"
        }
    }

    private func loadManager() async {
        let managers = (try? await NETunnelProviderManager.loadAllFromPreferences()) ?? []
        manager = managers.first { ($0.protocolConfiguration as? NETunnelProviderProtocol)?.providerBundleIdentifier == tunnelBundleID }
        observe()
    }

    private func preparedManager() async throws -> NETunnelProviderManager {
        let manager = self.manager ?? NETunnelProviderManager()
        let proto = NETunnelProviderProtocol()
        proto.providerBundleIdentifier = tunnelBundleID
        proto.serverAddress = settings.host
        proto.disconnectOnSleep = false
        manager.protocolConfiguration = proto
        manager.localizedDescription = "Secret Tunnel"
        manager.isEnabled = true
        try await manager.saveToPreferences()
        // Reload so the system-backed connection object is current after the first save.
        try await manager.loadFromPreferences()
        self.manager = manager
        observe()
        return manager
    }

    private func observe() {
        if let statusObserver { NotificationCenter.default.removeObserver(statusObserver) }
        guard let manager else { return }
        vpnStatus = manager.connection.status
        statusObserver = NotificationCenter.default.addObserver(
            forName: .NEVPNStatusDidChange, object: manager.connection, queue: .main
        ) { [weak self] _ in
            Task { @MainActor in self?.statusChanged() }
        }
        statusChanged()
    }

    private func statusChanged() {
        guard let manager else { return }
        vpnStatus = manager.connection.status
        pollTimer?.invalidate()
        if isActive {
            pollTimer = Timer.scheduledTimer(withTimeInterval: 1.5, repeats: true) { [weak self] _ in
                Task { @MainActor in self?.poll() }
            }
            poll()
        } else if vpnStatus == .disconnected, #available(iOS 16.0, *) {
            manager.connection.fetchLastDisconnectError { error in
                guard let error else { return }
                Task { @MainActor in self.message = error.localizedDescription }
            }
        }
    }

    private func poll() {
        guard let session = manager?.connection as? NETunnelProviderSession else { return }
        try? session.sendProviderMessage(Data("status".utf8)) { data in
            guard let data, let value = try? JSONSerialization.jsonObject(with: data) as? [String: Any] else { return }
            Task { @MainActor in
                self.rx = (value["rx"] as? NSNumber)?.int64Value ?? 0
                self.tx = (value["tx"] as? NSNumber)?.int64Value ?? 0
                self.logs = value["logs"] as? [String] ?? []
                self.tunnelError = value["error"] as? String ?? ""
            }
        }
    }
}
