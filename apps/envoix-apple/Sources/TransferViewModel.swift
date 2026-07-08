import Foundation
import Combine
import EnvoixCore

/// App-wide shared state: the two long-lived transfer view models (one per tab).
///
/// Both the main window and the menu-bar popover observe the same instances, so
/// status stays in sync everywhere. Re-emitting the children's `objectWillChange`
/// lets a view that observes only `AppModel` still update on transfer progress.
struct ActivityMetrics {
    var speedBps: Double = 0
    var avgBps: Double = 0
    var peakBps: Double = 0
    var speedHistory: [Double] = []
    var log: [String] = []

    fileprivate var lastLogKey: String = ""
}

final class AppModel: ObservableObject {
    static let shared = AppModel()

    let receive = TransferViewModel()
    let send = TransferViewModel()
    @Published private(set) var activities: [FfiTransferActivityRecord] = []
    @Published private(set) var activityMetrics: [String: ActivityMetrics] = [:]

    private var cancellables = Set<AnyCancellable>()
    private var removedActivityIDs = Set<String>()
    private var sharedSession: EnvoixSession?
    private var sharedSessionSettingsKey = ""
    private let activityCap = 50
    private let activityLogTimestamp: DateFormatter = {
        let formatter = DateFormatter()
        formatter.dateFormat = "HH:mm:ss"
        return formatter
    }()

    private init() {
        receive.sessionProvider = { [weak self] settings in
            self?.transferSession(for: settings) ?? EnvoixSession.newWithSettings(settings: settings)
        }
        send.sessionProvider = { [weak self] settings in
            self?.transferSession(for: settings) ?? EnvoixSession.newWithSettings(settings: settings)
        }

        for vm in [receive, send] {
            vm.objectWillChange
                .sink { [weak self] in self?.objectWillChange.send() }
                .store(in: &cancellables)
            vm.$transferActivity
                .compactMap { $0 }
                .sink { [weak self, weak vm] record in
                    self?.upsertActivity(record, speedBps: vm?.bytesPerSec ?? 0)
                    self?.syncActivitySnapshots()
                }
                .store(in: &cancellables)
        }
    }

    /// True while either side has a transfer in flight.
    var isActive: Bool { receive.isBusy || send.isBusy }

    private func transferSession(for settings: EnvoixRuntimeSettings) -> EnvoixSession {
        let key = sessionSettingsKey(for: settings)
        if sharedSession == nil || (!isActive && key != sharedSessionSettingsKey) {
            let session = EnvoixSession.newWithSettings(settings: settings)
            sharedSession = session
            sharedSessionSettingsKey = key
            return session
        }
        if let sharedSession {
            return sharedSession
        }
        let session = EnvoixSession.newWithSettings(settings: settings)
        sharedSession = session
        sharedSessionSettingsKey = key
        return session
    }

    private func sessionSettingsKey(for settings: EnvoixRuntimeSettings) -> String {
        [
            settings.concurrentTransfers ? "parallel" : "serial",
            settings.language,
            settings.serverUrl,
            settings.relayUrl,
            settings.configPath,
            String(settings.speedLimitMbps),
        ].joined(separator: "\u{1f}")
    }

    func pauseActivity(_ activityID: String) {
        if receive.pauseActivity(activityID) {
            syncActivitySnapshots()
            return
        }
        if send.pauseActivity(activityID) {
            syncActivitySnapshots()
        }
    }

    func resumeActivity(_ activityID: String) {
        if receive.resumeActivity(activityID) {
            syncActivitySnapshots()
            return
        }
        if send.resumeActivity(activityID) {
            syncActivitySnapshots()
        }
    }

    func removeActivity(_ activityID: String) {
        removedActivityIDs.insert(activityID)
        receive.cancelActivityForRemoval(activityID)
        send.cancelActivityForRemoval(activityID)
        activities.removeAll { $0.activityId == activityID }
        activityMetrics.removeValue(forKey: activityID)
    }

    private func syncActivitySnapshots() {
        let records = receive.listTransferActivities() + send.listTransferActivities()
        for record in records where !removedActivityIDs.contains(record.activityId) {
            upsertActivity(record, speedBps: speedBps(for: record.activityId))
        }
    }

    private func speedBps(for activityID: String) -> Double {
        if receive.ownsActivity(activityID) { return receive.bytesPerSec }
        if send.ownsActivity(activityID) { return send.bytesPerSec }
        return activityMetrics[activityID]?.speedBps ?? 0
    }

    private func upsertActivity(_ record: FfiTransferActivityRecord, speedBps: Double) {
        guard !removedActivityIDs.contains(record.activityId) else { return }
        if let index = activities.firstIndex(where: { $0.activityId == record.activityId }) {
            activities[index] = record
        } else {
            activities.append(record)
        }
        upsertMetrics(for: record, speedBps: speedBps)
        activities.sort { lhs, rhs in lhs.updatedAtMs > rhs.updatedAtMs }
        if activities.count > activityCap {
            let removed = activities.suffix(activities.count - activityCap).map(\.activityId)
            activities.removeLast(activities.count - activityCap)
            for id in removed {
                activityMetrics.removeValue(forKey: id)
            }
        }
    }

    private func upsertMetrics(for record: FfiTransferActivityRecord, speedBps: Double) {
        var metrics = activityMetrics[record.activityId] ?? ActivityMetrics()
        let liveSpeed = record.state == .transferring ? speedBps : 0
        metrics.speedBps = liveSpeed
        metrics.avgBps = averageBps(for: record)
        if liveSpeed > 0 {
            metrics.peakBps = max(metrics.peakBps, liveSpeed)
            metrics.speedHistory.append(liveSpeed)
            if metrics.speedHistory.count > 90 {
                metrics.speedHistory.removeFirst(metrics.speedHistory.count - 90)
            }
        }

        let logKey = activityLogKey(for: record)
        if logKey != metrics.lastLogKey {
            metrics.lastLogKey = logKey
            metrics.log.append(activityLogLine(for: record, speedBps: liveSpeed))
            if metrics.log.count > 160 {
                metrics.log.removeFirst(metrics.log.count - 160)
            }
        }

        activityMetrics[record.activityId] = metrics
    }

    private func averageBps(for record: FfiTransferActivityRecord) -> Double {
        let endMs = record.completedAtMs > 0 ? record.completedAtMs : record.updatedAtMs
        guard record.startedAtMs > 0, endMs > record.startedAtMs, record.bytesTransferred > 0 else { return 0 }
        return Double(record.bytesTransferred) * 1000 / Double(endMs - record.startedAtMs)
    }

    private func activityLogKey(for record: FfiTransferActivityRecord) -> String {
        switch record.state {
        case .transferring:
            let bucket: UInt64
            if record.totalBytes > 0 {
                bucket = min(20, record.bytesTransferred * 20 / record.totalBytes)
            } else {
                bucket = record.bytesTransferred / (10 * 1024 * 1024)
            }
            return "progress:\(bucket):\(record.dataPathKind):\(record.dataPathDetail)"
        default:
            return "\(record.state):\(record.dataPathKind):\(record.dataPathDetail):\(record.diagnosticMessage):\(record.bytesTransferred):\(record.totalBytes)"
        }
    }

    private func activityLogLine(for record: FfiTransferActivityRecord, speedBps: Double) -> String {
        let date = Date(timeIntervalSince1970: TimeInterval(record.updatedAtMs) / 1000)
        let prefix = "[\(activityLogTimestamp.string(from: date))]"
        let message: String
        switch record.state {
        case .queued:
            message = "queued · \(record.direction)"
        case .binding:
            message = "preparing · \(record.mode)"
        case .waitingForPeer:
            message = "waiting for peer"
        case .pairing:
            message = "pairing"
        case .connecting:
            message = record.dataPathKind == .none ? "connecting" : "connected · \(record.dataPathKind) \(record.dataPathDetail)"
        case .transferring:
            var parts = ["progress · \(byteString(record.bytesTransferred)) / \(byteString(record.totalBytes))"]
            if speedBps > 0 { parts.append(rateString(speedBps)) }
            message = parts.joined(separator: " · ")
        case .verifying:
            message = "verifying"
        case .completed:
            message = "completed · \(byteString(record.bytesTransferred))"
        case .failed:
            message = record.diagnosticMessage.isEmpty ? "failed" : "failed · \(record.diagnosticMessage)"
        case .paused:
            message = "paused"
        case .canceled:
            message = "canceled"
        case .unknown:
            message = "unknown state"
        }
        return "\(prefix) \(message)"
    }
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
        case paused
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
    @Published var transferEvents: [FfiTransferEvent] = []
    @Published var transferActivity: FfiTransferActivityRecord?

    var sessionProvider: ((EnvoixRuntimeSettings) -> EnvoixSession)?

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
    private var currentActivityID = ""
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
        case .waiting, .transferring, .paused: return true
        default: return false
        }
    }

    // MARK: User actions

    /// Receive on the local network using a shared token (mDNS auto-discovery).
    func startReceivingWithToken(outputDir: String, token: String, settings: EnvoixRuntimeSettings, destinationAccess: AnyObject? = nil) {
        destinationDir = outputDir
        let request = makeRequest(direction: .receive, mode: .mdns, settings: settings, outputDir: outputDir, token: token)
        start(settings: settings, phase: .waiting, activityID: request.activityId) { try $0.startTransfer(request: request, observer: $1) }
        retainResourceAccess(destinationAccess)
    }

    /// Receive by pairing through a rendezvous room code.
    func startReceivingWithRoom(outputDir: String, code: String, settings: EnvoixRuntimeSettings, destinationAccess: AnyObject? = nil) {
        destinationDir = outputDir
        let request = makeRequest(direction: .receive, mode: .room, settings: settings, outputDir: outputDir, code: code)
        start(settings: settings, phase: .waiting, activityID: request.activityId) { try $0.startTransfer(request: request, observer: $1) }
        retainResourceAccess(destinationAccess)
    }

    /// Receive by publishing an invite the sender pastes/scans.
    func startReceivingWithInvite(outputDir: String, settings: EnvoixRuntimeSettings, destinationAccess: AnyObject? = nil) {
        destinationDir = outputDir
        let request = makeRequest(direction: .receive, mode: .showInvite, settings: settings, outputDir: outputDir)
        start(settings: settings, phase: .waiting, activityID: request.activityId) { try $0.startTransfer(request: request, observer: $1) }
        retainResourceAccess(destinationAccess)
    }

    /// Send on the local network using a shared token (mDNS auto-discovery).
    func startSendingWithToken(filePath: String, token: String, settings: EnvoixRuntimeSettings, sourceAccess: AnyObject? = nil) {
        destinationDir = nil
        let request = makeRequest(direction: .send, mode: .mdns, settings: settings, filePath: filePath, token: token)
        start(settings: settings, phase: .transferring, activityID: request.activityId) { try $0.startTransfer(request: request, observer: $1) }
        retainResourceAccess(sourceAccess)
    }

    /// Send by pairing through a rendezvous room code.
    func startSendingWithRoom(filePath: String, code: String, settings: EnvoixRuntimeSettings, sourceAccess: AnyObject? = nil) {
        destinationDir = nil
        let request = makeRequest(direction: .send, mode: .room, settings: settings, filePath: filePath, code: code)
        start(settings: settings, phase: .waiting, activityID: request.activityId) { try $0.startTransfer(request: request, observer: $1) }
        retainResourceAccess(sourceAccess)
    }

    /// Send to the peer encoded in an invite string.
    func startSendingWithInvite(filePath: String, invite: String, settings: EnvoixRuntimeSettings, sourceAccess: AnyObject? = nil) {
        destinationDir = nil
        let request = makeRequest(direction: .send, mode: .invite, settings: settings, filePath: filePath, invite: invite)
        start(settings: settings, phase: .transferring, activityID: request.activityId) { try $0.startTransfer(request: request, observer: $1) }
        retainResourceAccess(sourceAccess)
    }

    func cancel() {
        suppressNextFailure = true
        operationID = UUID()
        if currentActivityID.isEmpty || session?.cancelActivity(activityId: currentActivityID) != true {
            session?.cancel()
        }
        reset()
        phase = .canceled
        statusText = AppText.value("Transfer canceled", "传输已取消", language: displayLanguage)
    }

    func cancelActivityForRemoval(_ activityID: String) {
        guard isBusy, !activityID.isEmpty, activityID == currentActivityID else { return }
        cancel()
    }

    func listTransferActivities() -> [FfiTransferActivityRecord] {
        session?.listTransferActivities() ?? []
    }

    func ownsActivity(_ activityID: String) -> Bool {
        !activityID.isEmpty && activityID == currentActivityID
    }

    @discardableResult
    func pauseActivity(_ activityID: String) -> Bool {
        guard isBusy, !activityID.isEmpty, activityID == currentActivityID else { return false }
        let paused = session?.pauseActivity(activityId: activityID) ?? false
        if paused {
            bytesPerSec = 0
            statusText = AppText.value("Pausing…", "正在暂停…", language: displayLanguage)
        }
        return paused
    }

    @discardableResult
    func resumeActivity(_ activityID: String) -> Bool {
        guard !activityID.isEmpty, activityID == currentActivityID else { return false }
        let resumed = session?.resumeActivity(activityId: activityID) ?? false
        if resumed {
            suppressNextFailure = false
            phase = .waiting
            statusText = AppText.value("Resuming…", "正在继续…", language: displayLanguage)
        }
        return resumed
    }

    /// Spins up a fresh session and launches `operation`, surfacing setup errors.
    private func start(
        settings: EnvoixRuntimeSettings,
        phase: Phase,
        activityID: String,
        operation: (EnvoixSession, Observer) throws -> Void
    ) {
        suppressNextFailure = false
        reset()
        currentActivityID = activityID
        displayLanguage = settings.language
        operationID = UUID()
        let operationID = operationID
        let session = sessionProvider?(settings) ?? EnvoixSession.newWithSettings(settings: settings)
        self.session = session
        self.phase = phase
        do {
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

    private func makeRequest(
        direction: FfiTransferDirection,
        mode: FfiTransferMode,
        settings: EnvoixRuntimeSettings,
        filePath: String = "",
        outputDir: String = "",
        invite: String = "",
        code: String = "",
        token: String = ""
    ) -> FfiTransferRequest {
        FfiTransferRequest(
            activityId: UUID().uuidString,
            direction: direction,
            mode: mode,
            filePath: filePath,
            outputDir: outputDir,
            peerDescriptor: "",
            invite: invite,
            code: code,
            token: token,
            broker: settings.serverUrl,
            relay: settings.relayUrl,
            configPath: settings.configPath,
            pathPolicy: .auto,
            resume: true,
            limits: FfiTransferLimits(
                maxParallelTransfers: settings.concurrentTransfers ? 2 : 1,
                maxParallelFiles: 1,
                maxParallelChunksPerFile: 1,
                speedLimitBps: 0
            ),
            rendezvous: rendezvousPlan(for: mode)
        )
    }

    private func rendezvousPlan(for mode: FfiTransferMode) -> FfiRendezvousPlan {
        switch mode {
        case .room:
            return FfiRendezvousPlan(useRoom: true, useMdns: true, internetAvailable: true)
        case .mdns:
            return FfiRendezvousPlan(useRoom: false, useMdns: true, internetAvailable: true)
        default:
            return FfiRendezvousPlan(useRoom: false, useMdns: false, internetAvailable: true)
        }
    }

    // MARK: Core callbacks (already on main via Observer)

    func handleInvite(_ invite: String) { self.invite = invite }

    func handleTransferEvent(_ event: FfiTransferEvent) {
        transferEvents.append(event)
        if transferEvents.count > 240 {
            transferEvents.removeFirst(transferEvents.count - 240)
        }
    }

    func handleTransferActivity(_ record: FfiTransferActivityRecord) {
        transferActivity = record
        syncPhase(with: record)
    }

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
        transferActivity = nil
        resourceAccess = nil
        eventLog.removeAll()
        transferEvents.removeAll()
        currentActivityID = ""
        rate.reset()
        phase = .idle
    }

    private func syncPhase(with record: FfiTransferActivityRecord) {
        guard record.activityId == currentActivityID else { return }
        switch record.state {
        case .queued, .binding, .waitingForPeer, .pairing, .connecting, .verifying:
            if phase == .paused || phase == .idle {
                phase = .waiting
            }
        case .transferring:
            phase = .transferring
        case .paused:
            bytesPerSec = 0
            phase = .paused
            statusText = AppText.value("Paused", "已暂停", language: displayLanguage)
        case .canceled:
            bytesPerSec = 0
            phase = .canceled
        case .completed:
            bytesPerSec = 0
            phase = .completed(bytes: record.bytesTransferred)
        case .failed, .unknown:
            break
        }
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
    friendlyFailure(
        code: failure.code,
        diagnosticMessage: failure.diagnosticMessage,
        language: language
    )
}

func friendlyFailure(code: FfiFailureCode, diagnosticMessage: String, language: String = "en") -> String {
    switch code {
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
        return diagnosticMessage
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
    func onTransferEvent(event: FfiTransferEvent) { hop { $0.handleTransferEvent(event) } }
    func onTransferActivity(record: FfiTransferActivityRecord) { hop { $0.handleTransferActivity(record) } }
    func onStatus(message: String) { hop { $0.handleStatus(message) } }

    private func hop(_ body: @escaping (TransferViewModel) -> Void) {
        DispatchQueue.main.async { [weak viewModel, operationID] in
            if let viewModel, viewModel.operationID == operationID { body(viewModel) }
        }
    }
}
