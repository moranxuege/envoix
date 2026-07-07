#if os(iOS)
import AVFoundation
import SwiftUI
import UIKit

struct QRCodeScannerSheet: View {
    @Environment(\.dismiss) private var dismiss
    @State private var cameraStatus = AVCaptureDevice.authorizationStatus(for: .video)

    let language: String
    let onScan: (String) -> Void

    var body: some View {
        NavigationStack {
            ZStack {
                switch cameraStatus {
                case .authorized:
                    QRCodeScannerCameraView { value in
                        onScan(value)
                        dismiss()
                    }
                    .ignoresSafeArea()
                case .denied, .restricted:
                    scannerMessage(
                        title: AppText.value("Camera access is off", "相机权限未开启", language: language),
                        detail: AppText.value("Allow camera access in Settings to scan an Envoix QR invite.", "请在系统设置中允许相机访问，然后扫描 Envoix 邀请二维码。", language: language)
                    )
                case .notDetermined:
                    scannerMessage(
                        title: AppText.value("Camera permission needed", "需要相机权限", language: language),
                        detail: AppText.value("Envoix uses the camera only to scan invite QR codes.", "Envoix 仅使用相机扫描邀请二维码。", language: language)
                    )
                @unknown default:
                    scannerMessage(
                        title: AppText.value("Camera unavailable", "相机不可用", language: language),
                        detail: AppText.value("This device cannot start QR scanning.", "当前设备无法启动二维码扫描。", language: language)
                    )
                }

                if cameraStatus == .authorized {
                    scannerFrame
                }
            }
            .navigationTitle(AppText.value("Scan Invite", "扫描邀请", language: language))
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button(AppText.value("Close", "关闭", language: language)) {
                        dismiss()
                    }
                }
            }
            .task {
                await requestCameraAccessIfNeeded()
            }
        }
    }

    private var scannerFrame: some View {
        RoundedRectangle(cornerRadius: 18)
            .strokeBorder(.white.opacity(0.88), lineWidth: 3)
            .frame(width: 240, height: 240)
            .shadow(radius: 8)
            .allowsHitTesting(false)
    }

    private func scannerMessage(title: String, detail: String) -> some View {
        VStack(spacing: 12) {
            Image(systemName: "camera.viewfinder")
                .font(.system(size: 42, weight: .semibold))
                .foregroundStyle(Theme.accentStrong)
            Text(title)
                .font(.title3.weight(.semibold))
                .foregroundStyle(Theme.text)
            Text(detail)
                .font(.body)
                .foregroundStyle(Theme.muted)
                .multilineTextAlignment(.center)
        }
        .padding(24)
    }

    @MainActor private func requestCameraAccessIfNeeded() async {
        guard cameraStatus == .notDetermined else { return }
        let granted = await withCheckedContinuation { continuation in
            AVCaptureDevice.requestAccess(for: .video) { allowed in
                continuation.resume(returning: allowed)
            }
        }
        cameraStatus = granted ? .authorized : .denied
    }
}

private struct QRCodeScannerCameraView: UIViewControllerRepresentable {
    let onScan: (String) -> Void

    func makeUIViewController(context: Context) -> QRCodeScannerViewController {
        let controller = QRCodeScannerViewController()
        controller.onScan = onScan
        return controller
    }

    func updateUIViewController(_ uiViewController: QRCodeScannerViewController, context: Context) {}
}

private final class QRCodeScannerViewController: UIViewController, AVCaptureMetadataOutputObjectsDelegate {
    private let session = AVCaptureSession()
    private let sessionQueue = DispatchQueue(label: "com.envoix.qr-scanner.session")
    private var previewLayer: AVCaptureVideoPreviewLayer?
    private var didScan = false

    var onScan: ((String) -> Void)?

    override func viewDidLoad() {
        super.viewDidLoad()
        view.backgroundColor = .black
        configureSession()
    }

    override func viewDidLayoutSubviews() {
        super.viewDidLayoutSubviews()
        previewLayer?.frame = view.bounds
    }

    override func viewDidAppear(_ animated: Bool) {
        super.viewDidAppear(animated)
        sessionQueue.async { [session] in
            if !session.isRunning {
                session.startRunning()
            }
        }
    }

    override func viewWillDisappear(_ animated: Bool) {
        super.viewWillDisappear(animated)
        sessionQueue.async { [session] in
            if session.isRunning {
                session.stopRunning()
            }
        }
    }

    private func configureSession() {
        guard let device = AVCaptureDevice.default(for: .video) else {
            showUnavailableMessage()
            return
        }

        do {
            let input = try AVCaptureDeviceInput(device: device)
            guard session.canAddInput(input) else {
                showUnavailableMessage()
                return
            }
            session.addInput(input)
        } catch {
            showUnavailableMessage()
            return
        }

        let output = AVCaptureMetadataOutput()
        guard session.canAddOutput(output) else {
            showUnavailableMessage()
            return
        }
        session.addOutput(output)
        output.setMetadataObjectsDelegate(self, queue: .main)
        output.metadataObjectTypes = [.qr]

        let layer = AVCaptureVideoPreviewLayer(session: session)
        layer.videoGravity = .resizeAspectFill
        layer.frame = view.bounds
        view.layer.insertSublayer(layer, at: 0)
        previewLayer = layer
    }

    private func showUnavailableMessage() {
        let label = UILabel()
        label.text = "Camera unavailable"
        label.textColor = .white
        label.textAlignment = .center
        label.font = .preferredFont(forTextStyle: .headline)
        label.translatesAutoresizingMaskIntoConstraints = false
        view.addSubview(label)
        NSLayoutConstraint.activate([
            label.centerXAnchor.constraint(equalTo: view.centerXAnchor),
            label.centerYAnchor.constraint(equalTo: view.centerYAnchor),
            label.leadingAnchor.constraint(greaterThanOrEqualTo: view.leadingAnchor, constant: 24),
            label.trailingAnchor.constraint(lessThanOrEqualTo: view.trailingAnchor, constant: -24)
        ])
    }

    func metadataOutput(
        _ output: AVCaptureMetadataOutput,
        didOutput metadataObjects: [AVMetadataObject],
        from connection: AVCaptureConnection
    ) {
        guard !didScan else { return }
        guard let code = metadataObjects
            .compactMap({ $0 as? AVMetadataMachineReadableCodeObject })
            .first(where: { $0.type == .qr })?
            .stringValue?
            .trimmed,
            !code.isEmpty
        else {
            return
        }

        didScan = true
        sessionQueue.async { [session] in
            if session.isRunning {
                session.stopRunning()
            }
        }
        onScan?(code)
    }
}
#endif
