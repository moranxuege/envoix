#if os(iOS)
import AVFoundation
import SwiftUI
import UIKit

enum QRCodeScannerMessageKind: CaseIterable {
    case cameraAccessDenied
    case cameraPermissionRequired
    case cameraUnavailable
}

enum QRCodeScannerPresentationText {
    static func title(for kind: QRCodeScannerMessageKind, language: String) -> String {
        AppText.localized("scanner.camera.\(key(for: kind)).title", language: language)
    }

    static func detail(for kind: QRCodeScannerMessageKind, language: String) -> String {
        AppText.localized("scanner.camera.\(key(for: kind)).detail", language: language)
    }

    private static func key(for kind: QRCodeScannerMessageKind) -> String {
        switch kind {
        case .cameraAccessDenied: "denied"
        case .cameraPermissionRequired: "permission_required"
        case .cameraUnavailable: "unavailable"
        }
    }
}

struct QRCodeScannerSheet: View {
    @Environment(\.dismiss) private var dismiss
    @State private var cameraStatus = AVCaptureDevice.authorizationStatus(for: .video)
    @State private var scanError: String?

    let language: String
    let onScan: (String) -> String?

    var body: some View {
        NavigationStack {
            ZStack {
                switch cameraStatus {
                case .authorized:
                    QRCodeScannerCameraView(
                        unavailableText: QRCodeScannerPresentationText.title(
                            for: .cameraUnavailable,
                            language: language
                        ),
                        onScan: { value in acceptScannedValue(value) }
                    )
                    .ignoresSafeArea()
                case .denied, .restricted:
                    scannerMessage(.cameraAccessDenied)
                case .notDetermined:
                    scannerMessage(.cameraPermissionRequired)
                @unknown default:
                    scannerMessage(.cameraUnavailable)
                }

                if cameraStatus == .authorized {
                    scannerFrame
                }

                if let scanError {
                    scannerError(scanError)
                }

                #if DEBUG
                if let testValue = scannerUITestValue {
                    Button(AppText.localized("scanner.action.use_test_qr", language: language)) {
                        _ = acceptScannedValue(testValue)
                    }
                    .buttonStyle(.borderedProminent)
                    .accessibilityIdentifier("qr_scanner_test_payload")
                    .frame(maxHeight: .infinity, alignment: .bottom)
                    .padding(.bottom, 28)
                }
                #endif
            }
            .navigationTitle(AppText.localized("scanner.title", language: language))
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button(AppText.localized("common.close", language: language)) {
                        dismiss()
                    }
                }
            }
            .task {
                #if DEBUG
                guard scannerUITestValue == nil else { return }
                #endif
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

    private func scannerMessage(_ kind: QRCodeScannerMessageKind) -> some View {
        VStack(spacing: 12) {
            Image(systemName: "camera.viewfinder")
                .font(.system(size: 42, weight: .semibold))
                .foregroundStyle(Theme.accentStrong)
            Text(QRCodeScannerPresentationText.title(for: kind, language: language))
                .font(.title3.weight(.semibold))
                .foregroundStyle(Theme.text)
            Text(QRCodeScannerPresentationText.detail(for: kind, language: language))
                .font(.body)
                .foregroundStyle(Theme.muted)
                .multilineTextAlignment(.center)
        }
        .padding(24)
    }

    private func scannerError(_ message: String) -> some View {
        Text(message)
            .font(.callout.weight(.semibold))
            .foregroundStyle(.white)
            .multilineTextAlignment(.center)
            .padding(.horizontal, 16)
            .padding(.vertical, 12)
            .background(.black.opacity(0.78), in: RoundedRectangle(cornerRadius: 12))
            .padding(.horizontal, 24)
            .frame(maxHeight: .infinity, alignment: .bottom)
            .padding(.bottom, 88)
            .accessibilityIdentifier("qr_scanner_error")
    }

    private func acceptScannedValue(_ value: String) -> Bool {
        if let error = onScan(value) {
            scanError = error
            return false
        }
        scanError = nil
        dismiss()
        return true
    }

    #if DEBUG
    private var scannerUITestValue: String? {
        guard ProcessInfo.processInfo.arguments.contains("--ui-testing-scanner") else {
            return nil
        }
        return ProcessInfo.processInfo.environment["ENVOIX_UI_TEST_SCAN_PAYLOAD"]
    }
    #endif

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
    let unavailableText: String
    let onScan: (String) -> Bool

    func makeUIViewController(context: Context) -> QRCodeScannerViewController {
        let controller = QRCodeScannerViewController()
        controller.unavailableText = unavailableText
        controller.onScan = onScan
        return controller
    }

    func updateUIViewController(_ uiViewController: QRCodeScannerViewController, context: Context) {
        uiViewController.unavailableText = unavailableText
        uiViewController.onScan = onScan
    }
}

private final class QRCodeScannerViewController: UIViewController, AVCaptureMetadataOutputObjectsDelegate {
    private static let rejectedScanRetryDelay: TimeInterval = 1
    private let session = AVCaptureSession()
    private let sessionQueue = DispatchQueue(label: "com.envoix.qr-scanner.session")
    private var previewLayer: AVCaptureVideoPreviewLayer?
    private var didScan = false

    var unavailableText = ""
    var onScan: ((String) -> Bool)?

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
        label.text = unavailableText
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
        if onScan?(code) == true {
            sessionQueue.async { [session] in
                if session.isRunning {
                    session.stopRunning()
                }
            }
        } else {
            DispatchQueue.main.asyncAfter(deadline: .now() + Self.rejectedScanRetryDelay) { [weak self] in
                self?.didScan = false
            }
        }
    }
}
#endif
