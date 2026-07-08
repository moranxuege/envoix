import Foundation
import Combine
import EnvoixCore

/// App-wide shared state: the two long-lived transfer view models (one per tab).
///
/// Both the main window and the menu-bar popover observe the same instances, so
/// status stays in sync everywhere. Re-emitting the children's `objectWillChange`
/// lets a view that observes only `AppModel` still update on transfer progress.
final class AppModel: ObservableObject {
    static let shared = AppModel()

    let receive = TransferViewModel()
    let send = TransferViewModel()

    private var cancellables = Set<AnyCancellable>()

    private init() {
        for vm in [receive, send] {
            vm.objectWillChange
                .sink { [weak self] in self?.objectWillChange.send() }
                .store(in: &cancellables)
        }
    }

    /// True while either side has a transfer in flight.
    var isActive: Bool { receive.isBusy || send.isBusy }
}

/// Drives one send or receive operation and exposes its state to SwiftUI.
///
/// All `@Published` mutations happen on the main thread: user actions are
/// invoked from the UI, and core callbacks are marshaled to main by `Observer`.
final class TransferViewModel: ObservableObject {
    enum Phase: Equatable {
        case idle
        case waiting          // receiver: endpoint up, invite shown, awaiting sender
        case transferring
        case completed(bytes: UInt64)
        case canceled
        case failed(String)
    }

    @Published var phase: Phase = .idle
    @Published var invite: String = ""        // receiver only
    @Published var fileName: String = ""
    @Published var transferred: UInt64 = 0
    @Published var total: UInt64 = 0
    @Published var statusText: String = ""
    @Published var peerAddress: String = ""   // raw IP-bearing address, hidden by default
    @Published var eventLog: [String] = []
    @Published var bytesPerSec: Double = 0    // rolling average, 0 until measurable
    @Published var completedFileURL: URL?     // receiver only: where the file landed
    @Published var failure: FfiTransferFailure?

    private var session: EnvoixSession?
    private var destinationDir: String?       // receiver only
    private var resourceAccess: AnyObject?    // keeps iOS Files permission alive
    private var rate = RateTracker()
    private var suppressNextFailure = false
    private var logLastProgress: Date = .distantPast
    private var configLogTimestamp: DateFormatter = {
        let formatter = DateFormatter()
        formatter.dateFormat = "HH:mm:ss"
        return formatter
    }()
    private var displayLanguage = "en"
    fileprivate var operationID = UUID()

    var progressFraction: Double {
        total > 0 ? Double(transferred) / Double(total) : 0
    }

    /// Seconds left at the current average rate, or nil if not yet estimable.
    var etaSeconds: Double? {
        guard bytesPerSec > 0, total > transferred else { return nil }
        return Double(total - transferred) / bytesPerSec
    }

    var isBusy: Bool {
        switch phase {
        case .waiting, .transferring: return true
        default: return false
        }
    }

    // MARK: User actions

    /// Receive on the local network using a shared token (mDNS auto-discovery).
    func startReceivingWithToken(outputDir: String, token: String, settings: EnvoixRuntimeSettings, destinationAccess: AnyObject? = nil) {
        destinationDir = outputDir
        start(settings: settings, phase: .waiting) { try $0.receiveMdns(outputDir: outputDir, token: token, observer: $1) }
        retainResourceAccess(destinationAccess)
    }

    /// Receive by pairing through a rendezvous room code.
    func startReceivingWithRoom(outputDir: String, code: String, settings: EnvoixRuntimeSettings, destinationAccess: AnyObject? = nil) {
        destinationDir = outputDir
        start(settings: settings, phase: .waiting) { try $0.receiveRoom(outputDir: outputDir, code: code, observer: $1) }
        retainResourceAccess(destinationAccess)
    }

    /// Receive by publishing an invite the sender pastes/scans.
    func startReceivingWithInvite(outputDir: String, settings: EnvoixRuntimeSettings, destinationAccess: AnyObject? = nil) {
        destinationDir = outputDir
        start(settings: settings, phase: .waiting) { try $0.receive(outputDir: outputDir, observer: $1) }
        retainResourceAccess(destinationAccess)
    }

    /// Send on the local network using a shared token (mDNS auto-discovery).
    func startSendingWithToken(filePath: String, token: String, settings: EnvoixRuntimeSettings, sourceAccess: AnyObject? = nil) {
        destinationDir = nil
        start(settings: settings, phase: .transferring) { try $0.sendMdns(filePath: filePath, token: token, observer: $1) }
        retainResourceAccess(sourceAccess)
    }

    /// Send by pairing through a rendezvous room code.
    func startSendingWithRoom(filePath: String, code: String, settings: EnvoixRuntimeSettings, sourceAccess: AnyObject? = nil) {
        destinationDir = nil
        start(settings: settings, phase: .waiting) { try $0.sendRoom(filePath: filePath, code: code, observer: $1) }
        retainResourceAccess(sourceAccess)
    }

    /// Send to the peer encoded in an invite string.
    func startSendingWithInvite(filePath: String, invite: String, settings: EnvoixRuntimeSettings, sourceAccess: AnyObject? = nil) {
        destinationDir = nil
        start(settings: settings, phase: .transferring) { try $0.sendInvite(invite: invite, filePath: filePath, observer: $1) }
        retainResourceAccess(sourceAccess)
    }

    func cancel() {
        suppressNextFailure = true
        operationID = UUID()
        session?.cancel()
        reset()
        phase = .canceled
        statusText = AppText.value("Transfer canceled", "传输已取消", language: displayLanguage)
    }

    /// Spins up a fresh session and launches `operation`, surfacing setup errors.
    private func start(
        settings: EnvoixRuntimeSettings,
        phase: Phase,
        operation: (EnvoixSession, Observer) throws -> Void
    ) {
        suppressNextFailure = false
        reset()
        displayLanguage = settings.language
        operationID = UUID()
        let operationID = operationID
        self.phase = phase
        do {
            let session = EnvoixSession.newWithSettings(settings: settings)
            self.session = session
            try operation(session, Observer(self, operationID: operationID))
        } catch {
            self.phase = .failed(friendlyError(error.localizedDescription, language: displayLanguage))
        }
    }

    private func retainResourceAccess(_ access: AnyObject?) {
        if case .failed = phase {
            resourceAccess = nil
        } else {
            resourceAccess = access
        }
    }

    // MARK: Core callbacks (already on main via Observer)

    func handleInvite(_ invite: String) { self.invite = invite }

    func handleStarted(_ name: String, _ total: UInt64) {
        appendLog("started · \(name) (\(byteString(total)))")
        fileName = name
        self.total = total
        transferred = 0
        rate.reset()
        bytesPerSec = 0
        phase = .transferring
    }

    func handleProgress(_ transferred: UInt64, _ total: UInt64) {
        appendLog("progress · \(byteString(transferred)) / \(byteString(total))", throttle: true)
        self.transferred = transferred
        self.total = total
        bytesPerSec = rate.record(transferred)
    }

    func handleCompleted(_ bytes: UInt64) {
        appendLog("completed · \(byteString(bytes))")
        transferred = total
        bytesPerSec = 0
        if let dir = destinationDir, !fileName.isEmpty {
            completedFileURL = URL(fileURLWithPath: dir).appendingPathComponent(fileName)
        }
        resourceAccess = nil
        phase = .completed(bytes: bytes)
    }

    func handleTransferFailed(_ failure: FfiTransferFailure) {
        appendLog("failed · \(failure.diagnosticMessage)")
        if suppressNextFailure {
            return
        }
        self.failure = failure
        resourceAccess = nil
        phase = .failed(friendlyFailure(failure, language: displayLanguage))
    }

    func handleFailed(_ reason: String) {
        appendLog("failed · \(reason)")
        if suppressNextFailure {
            suppressNextFailure = false
            reset()
            statusText = AppText.value("Canceled", "已取消", language: displayLanguage)
            return
        }
        if let failure {
            phase = .failed(friendlyFailure(failure, language: displayLanguage))
            return
        }
        resourceAccess = nil
        phase = .failed(friendlyError(reason, language: displayLanguage))
    }

    /// The core echoes the bound peer as `"address: <descriptor>"`, which
    /// carries the real IP. Keep that out of the general status line and stash
    /// it separately so the UI can gate it behind an explicit reveal.
    func handleStatus(_ message: String) {
        let prefix = "address: "
        if message.hasPrefix(prefix) {
            appendLog("address · \(message.dropFirst(prefix.count))")
            peerAddress = String(message.dropFirst(prefix.count))
        } else {
            appendLog("status · \(message)")
            statusText = message
        }
    }

    private func appendLog(_ message: String, throttle: Bool = false) {
        if throttle {
            let now = Date()
            if now.timeIntervalSince(logLastProgress) < 0.8 {
                return
            }
            logLastProgress = now
        }
        eventLog.append("[\(configLogTimestamp.string(from: Date()))] \(message)")
        if eventLog.count > 160 {
            eventLog.removeFirst(eventLog.count - 160)
        }
    }

    private func reset() {
        invite = ""
        fileName = ""
        transferred = 0
        total = 0
        statusText = ""
        peerAddress = ""
        bytesPerSec = 0
        completedFileURL = nil
        failure = nil
        resourceAccess = nil
        eventLog.removeAll()
        rate.reset()
        phase = .idle
    }
}

/// Rolling-window throughput estimate: average speed over roughly the last few
/// seconds, which absorbs short bursts/stalls without lagging the whole transfer.
private struct RateTracker {
    private struct Sample { let time: TimeInterval; let bytes: UInt64 }
    private var samples: [Sample] = []
    private let window: TimeInterval = 3

    mutating func reset() { samples.removeAll() }

    /// Records a cumulative byte count, returns the current bytes/sec estimate.
    mutating func record(_ bytes: UInt64) -> Double {
        let now = ProcessInfo.processInfo.systemUptime
        samples.append(Sample(time: now, bytes: bytes))
        samples.removeAll { now - $0.time > window }
        guard let first = samples.first, samples.count > 1 else { return 0 }
        let dt = now - first.time
        guard dt > 0 else { return 0 }
        return Double(bytes - first.bytes) / dt
    }
}

/// Maps common raw failure strings to friendlier UI text; passes others through.
func friendlyError(_ reason: String, language: String = "en") -> String {
    let lower = reason.lowercased()
    if lower.contains("timed out") || lower.contains("timeout") || lower.contains("deadline") {
        return AppText.value(
            "Couldn't reach the other device. Make sure both are on the same Wi-Fi network and the token matches.",
            "无法连接另一台设备。请确认两台设备在同一 Wi-Fi，且口令匹配。",
            language: language
        )
    }
    if lower.contains("no peer") || lower.contains("not found") || lower.contains("no route") {
        return AppText.value(
            "No device found. Check that the other side is running and the token or invite is correct.",
            "未发现设备。请确认另一端正在运行，并且口令或邀请信息正确。",
            language: language
        )
    }
    if lower.contains("expired") {
        return AppText.value(
            "This invite has expired. Ask the receiver to generate a new one.",
            "此邀请已过期。请让接收方重新生成。",
            language: language
        )
    }
    if lower.contains("permission") || lower.contains("denied") {
        return AppText.value(
            "Access was denied. Check the destination folder permissions and local-network access.",
            "访问被拒绝。请检查目标文件夹权限和本地网络访问权限。",
            language: language
        )
    }
    return reason
}

func friendlyFailure(_ failure: FfiTransferFailure, language: String = "en") -> String {
    switch failure.code {
    case .userCanceled:
        return AppText.value("Transfer canceled", "传输已取消", language: language)
    case .peerCanceled:
        return AppText.value("The other device canceled the transfer.", "另一台设备取消了传输。", language: language)
    case .networkLost:
        return AppText.value("The connection was lost. Try again when both devices are online.", "连接已中断。请确认两台设备在线后重试。", language: language)
    case .peerUnreachable:
        return AppText.value("No device found. Check that the other side is running and the code or token is correct.", "未发现设备。请确认另一端正在运行，并且配对码或口令正确。", language: language)
    case .authenticationFailed:
        return AppText.value("Pairing failed. Check the code or token on both devices.", "配对失败。请检查两台设备上的配对码或口令。", language: language)
    case .permissionDenied:
        return AppText.value("Access was denied. Choose a folder again or check local-network permissions.", "访问被拒绝。请重新选择文件夹，或检查本地网络权限。", language: language)
    case .diskFull:
        return AppText.value("There is not enough space to save the file.", "没有足够空间保存文件。", language: language)
    case .hashMismatch:
        return AppText.value("The received file did not pass verification. Please retry the transfer.", "接收的文件未通过校验。请重新传输。", language: language)
    case .protocolError:
        return AppText.value("The two devices did not agree on the transfer protocol. Update both apps and try again.", "两台设备的传输协议不一致。请更新两端应用后重试。", language: language)
    case .destinationConflict:
        return AppText.value("The file could not be saved to the selected destination. Choose another folder and try again.", "文件无法保存到当前目标位置。请选择其他文件夹后重试。", language: language)
    case .unsupportedFeature:
        return AppText.value("This transfer mode is not supported by the other app version.", "另一端应用版本不支持此传输模式。", language: language)
    case .timeout:
        return AppText.value("The transfer timed out. Try again; a retry may resume from partial progress.", "传输超时。请重试，可能会从已有进度继续。", language: language)
    case .internalError:
        return AppText.value("An internal transfer error occurred. Try again or copy diagnostics from Activity.", "发生内部传输错误。请重试，或从活动页复制诊断信息。", language: language)
    case .unknown:
        return failure.diagnosticMessage
    }
}

/// Bridges core `TransferObserver` callbacks (delivered on Rust runtime threads)
/// onto the main thread before touching the view model.
final class Observer: TransferObserver, @unchecked Sendable {
    private weak var viewModel: TransferViewModel?
    private let operationID: UUID

    init(_ viewModel: TransferViewModel, operationID: UUID) {
        self.viewModel = viewModel
        self.operationID = operationID
    }

    func onInviteReady(invite: String) { hop { $0.handleInvite(invite) } }
    func onStarted(fileName: String, totalBytes: UInt64) { hop { $0.handleStarted(fileName, totalBytes) } }
    func onProgress(transferred: UInt64, total: UInt64) { hop { $0.handleProgress(transferred, total) } }
    func onCompleted(bytes: UInt64) { hop { $0.handleCompleted(bytes) } }
    func onTransferFailed(failure: FfiTransferFailure) { hop { $0.handleTransferFailed(failure) } }
    func onFailed(reason: String) { hop { $0.handleFailed(reason) } }
    func onStatus(message: String) { hop { $0.handleStatus(message) } }

    private func hop(_ body: @escaping (TransferViewModel) -> Void) {
        DispatchQueue.main.async { [weak viewModel, operationID] in
            if let viewModel, viewModel.operationID == operationID { body(viewModel) }
        }
    }
}
