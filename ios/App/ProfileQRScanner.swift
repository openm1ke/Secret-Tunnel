import SwiftUI
import AVFoundation

struct ProfileQRScanner: UIViewControllerRepresentable {
    var scanned: (String) -> Void
    func makeUIViewController(context: Context) -> ProfileQRController { ProfileQRController(scanned: scanned) }
    func updateUIViewController(_ controller: ProfileQRController, context: Context) {}
}

final class ProfileQRController: UIViewController, AVCaptureMetadataOutputObjectsDelegate {
    private let session = AVCaptureSession()
    private let queue = DispatchQueue(label: "dev.secrettunnel.qr-camera")
    private var preview: AVCaptureVideoPreviewLayer?
    private let status = UILabel()
    private let scanned: (String) -> Void
    private var accepted = false
    private let lifecycleLock = NSLock()
    private var active = true
    init(scanned: @escaping (String) -> Void) { self.scanned = scanned; super.init(nibName: nil, bundle: nil) }
    required init?(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }
    override func viewDidLoad() {
        super.viewDidLoad(); view.backgroundColor = .black
        status.text = "Наведите камеру на QR-код Secret Tunnel"; status.textColor = .white; status.numberOfLines = 0
        status.textAlignment = .center; status.translatesAutoresizingMaskIntoConstraints = false; view.addSubview(status)
        NSLayoutConstraint.activate([status.leadingAnchor.constraint(equalTo: view.leadingAnchor, constant: 20), status.trailingAnchor.constraint(equalTo: view.trailingAnchor, constant: -20), status.bottomAnchor.constraint(equalTo: view.safeAreaLayoutGuide.bottomAnchor, constant: -24)])
        AVCaptureDevice.requestAccess(for: .video) { [weak self] allowed in
            guard let self else { return }
            if allowed { self.queue.async { self.configure() } }
            else { DispatchQueue.main.async { self.status.text = "Разрешите доступ к камере в настройках iOS. Также можно вставить ссылку или открыть файл." } }
        }
    }
    private func configure() {
        lifecycleLock.lock(); let allowed = active; lifecycleLock.unlock()
        guard allowed else { return }
        guard let device = AVCaptureDevice.default(for: .video), let input = try? AVCaptureDeviceInput(device: device), session.canAddInput(input) else {
            DispatchQueue.main.async { self.status.text = "Камера недоступна. Вставьте ссылку или откройте файл." }; return
        }
        session.beginConfiguration(); session.addInput(input)
        let output = AVCaptureMetadataOutput()
        guard session.canAddOutput(output) else { session.commitConfiguration(); return }
        session.addOutput(output); output.setMetadataObjectsDelegate(self, queue: .main); output.metadataObjectTypes = [.qr]
        session.commitConfiguration()
        DispatchQueue.main.async {
            let preview = AVCaptureVideoPreviewLayer(session: self.session); preview.videoGravity = .resizeAspectFill
            self.view.layer.insertSublayer(preview, at: 0); self.preview = preview; preview.frame = self.view.bounds
        }
        session.startRunning()
    }
    override func viewDidLayoutSubviews() { super.viewDidLayoutSubviews(); preview?.frame = view.bounds }
    override func viewDidDisappear(_ animated: Bool) {
        super.viewDidDisappear(animated)
        lifecycleLock.lock(); active = false; lifecycleLock.unlock()
        queue.async { self.session.stopRunning() }
    }
    func metadataOutput(_ output: AVCaptureMetadataOutput, didOutput metadataObjects: [AVMetadataObject], from connection: AVCaptureConnection) {
        guard !accepted, let value = metadataObjects.compactMap({ ($0 as? AVMetadataMachineReadableCodeObject)?.stringValue }).first else { return }
        guard value.hasPrefix("secrettunnel://import#stp1."), value.utf8.count <= 1_048_576 else { status.text = "Это не QR-код переноса Secret Tunnel"; return }
        accepted = true; queue.async { self.session.stopRunning() }; scanned(value)
    }
}
