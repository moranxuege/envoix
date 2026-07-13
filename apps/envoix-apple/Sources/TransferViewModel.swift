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

private struct ReceivePublication {
    let destinationDirectory: URL
    let resourceAccess: AnyObject?
    var completedRecord: FfiTransferActivityRecord?
    var isPublishing = false
}

final class AppModel: ObservableObject {
    static let shared = AppModel()

    let receive = TransferViewModel()
    let send = TransferViewModel()
    @Published private(set) var activities: [FfiTransferActivityRecord] = []
    @Published private(set) var activityMetrics: [String: ActivityMetrics] = [:]
    private var transferEventLinesByActivityID: [String: [String]] = [:]
    private var transferLogByActivityID: [String: [String]] = [:]
    private var activityResourceAccess: [String: AnyObject] = [:]
    private var receivePublications: [String: ReceivePublication] = [:]
    private var durableSessions: [String: DurableEnvoixSession] = [:]

    private var cancellables = Set<AnyCancellable>()
    private var removedActivityIDs = Set<String>()
    private let activityCap = 50
    private let recordsDirectory: URL = {
        let base = FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask).first
            ?? URL(fileURLWithPath: NSTemporaryDirectory(), isDirectory: true)
        return base.appendingPathComponent("envoix/transfer-records", isDirectory: true)
    }()
    private lazy var mailboxObserver = AppleMailboxObserver(model: self)
    private let activityLogTimestamp: DateFormatter = {
        let formatter = DateFormatter()
        formatter.dateFormat = "HH:mm:ss"
        return formatter
    }()

    private init() {
        receive.appModel = self
        send.appModel = self

        for vm in [receive, send] {
            vm.objectWillChange
                .throttle(for: .milliseconds(500), scheduler: RunLoop.main, latest: true)
                .sink { [weak self] in self?.objectWillChange.send() }
                .store(in: &cancellables)
            vm.$transferActivity
                .compactMap { $0 }
                .sink { [weak self, weak vm] record in
                    self?.handleCoreActivity(record)
                    if Self.isTerminal(record.state), let vm {
                        self?.snapshotDiagnostics(from: vm, activityID: record.activityId)
                    }
                }
                .store(in: &cancellables)
        }
        #if DEBUG
        if ProcessInfo.processInfo.arguments.contains("--ui-testing-activity-fixtures") {
            activities = PreviewFixtures.activityRecords
            activityMetrics = PreviewFixtures.activityMetrics
        } else {
            DispatchQueue.main.async { [weak self] in
                self?.restoreDurableTransfers()
            }
        }
        #else
        DispatchQueue.main.async { [weak self] in
            self?.restoreDurableTransfers()
        }
        #endif
    }

    /// True while either side has a transfer in flight.
    var isActive: Bool {
        receive.isBusy || send.isBusy || activities.contains { !Self.isTerminal($0.state) }
    }

    @discardableResult
    func pauseActivity(_ activityID: String) -> Bool {
        if durableSessions[activityID]?.pause() == true {
            syncActivitySnapshots()
            return true
        }
        return false
    }

    @discardableResult
    func resumeActivity(_ activityID: String) -> Bool {
        if retryReceivePublication(activityID) {
            return true
        }
        if durableSessions[activityID]?.resume() == true {
            syncActivitySnapshots()
            return true
        }
        return false
    }

    @discardableResult
    func cancelActivity(_ activityID: String) -> Bool {
        if durableSessions[activityID]?.cancel() == true {
            syncActivitySnapshots()
            return true
        }
        return false
    }

    func removeActivity(_ activityID: String) {
        removedActivityIDs.insert(activityID)
        _ = durableSessions.removeValue(forKey: activityID)?.remove()
        activityResourceAccess.removeValue(forKey: activityID)
        if let publication = receivePublications.removeValue(forKey: activityID),
           let path = publication.completedRecord?.completedFilePath,
           !path.isEmpty {
            try? FileManager.default.removeItem(at: URL(fileURLWithPath: path))
        }
        cleanupReceiveStaging(activityID: activityID)
        ReceivePublicationStore.remove(activityID: activityID)
        receive.forgetRoomID(for: activityID)
        send.forgetRoomID(for: activityID)
        activities.removeAll { $0.activityId == activityID }
        activityMetrics.removeValue(forKey: activityID)
        transferEventLinesByActivityID.removeValue(forKey: activityID)
        transferLogByActivityID.removeValue(forKey: activityID)
    }

    private func syncActivitySnapshots() {
        let persisted = (try? listDurableTransferRecords(recordsDir: recordsDirectory.path)) ?? []
        let live = durableSessions.values.map { $0.activity() }
        let records = persisted + live
        let uniqueRecords = Dictionary(records.map { ($0.activityId, $0) }, uniquingKeysWith: { _, latest in latest })
        for record in uniqueRecords.values where !removedActivityIDs.contains(record.activityId) {
            upsertActivity(record, speedBps: speedBps(for: record.activityId))
        }
    }

    func startDurableSession(
        settings: EnvoixRuntimeSettings,
        request: FfiTransferRequest,
        observer: TransferObserver
    ) throws -> DurableEnvoixSession {
        try FileManager.default.createDirectory(at: recordsDirectory, withIntermediateDirectories: true)
        let session = try startDurableTransfer(
            settings: settings,
            request: request,
            recordsDir: recordsDirectory.path,
            observer: observer,
            mailbox: mailboxObserver
        )
        durableSessions[request.activityId] = session
        upsertActivity(session.activity(), speedBps: 0)
        return session
    }

    func deliverReceipt(_ data: Data, activityID: String) {
        _ = durableSessions[activityID]?.receiptResponse(blob: data)
    }

    func acknowledgeReceiptPost(activityID: String) {
        _ = durableSessions[activityID]?.receiptPosted()
    }

    private func restoreDurableTransfers() {
        do {
            try FileManager.default.createDirectory(at: recordsDirectory, withIntermediateDirectories: true)
            let records = try listDurableTransferRecords(recordsDir: recordsDirectory.path)
            restoreReceivePublicationTargets(for: records)
            for record in records where !removedActivityIDs.contains(record.activityId) {
                upsertActivity(record, speedBps: 0)
                let observer = Observer(
                    nil,
                    appModel: self,
                    operationID: UUID(),
                    activityID: record.activityId
                )
                do {
                    let session = try restoreDurableTransfer(
                        activityId: record.activityId,
                        recordsDir: recordsDirectory.path,
                        observer: observer,
                        mailbox: mailboxObserver
                    )
                    durableSessions[record.activityId] = session
                    handleCoreActivity(session.activity())
                } catch {
                    handleCoreStatus("restore failed: \(error.localizedDescription)", activityID: record.activityId)
                }
            }
        } catch {
            transferLogByActivityID["app", default: []].append("restore scan failed: \(error.localizedDescription)")
        }
    }

    private func restoreReceivePublicationTargets(for records: [FfiTransferActivityRecord]) {
        let targets = ReceivePublicationStore.loadAll()
        for record in records where record.state == .publishing {
            guard let target = targets[record.activityId] else { continue }
            let destination: URL
            let access: AnyObject?
            #if os(iOS)
            if let bookmark = target.bookmark,
               let resolved = try? resolveSecurityScopedFolderBookmark(bookmark) {
                destination = resolved
                access = SecurityScopedResourceAccess(url: resolved)
            } else {
                destination = URL(fileURLWithPath: target.destinationPath, isDirectory: true)
                access = nil
            }
            #else
            destination = URL(fileURLWithPath: target.destinationPath, isDirectory: true)
            access = nil
            #endif
            receivePublications[record.activityId] = ReceivePublication(
                destinationDirectory: destination,
                resourceAccess: access,
                completedRecord: record,
                isPublishing: false
            )
            retainResourceAccess(access, for: record.activityId)
        }
    }

    func handleCoreActivity(_ record: FfiTransferActivityRecord) {
        guard !removedActivityIDs.contains(record.activityId) else { return }
        if record.direction == .receive,
           record.state == .publishing,
           receivePublications[record.activityId] != nil {
            upsertActivity(record, speedBps: 0)
            beginReceivePublication(record)
            return
        }
        upsertActivity(record, speedBps: speedBps(for: record.activityId))
        if Self.isTerminal(record.state) {
            activityResourceAccess.removeValue(forKey: record.activityId)
            if record.state == .completed || record.state == .canceled {
                receivePublications.removeValue(forKey: record.activityId)
                cleanupReceiveStaging(activityID: record.activityId)
                ReceivePublicationStore.remove(activityID: record.activityId)
            }
        }
    }

    func handleCoreEvent(_ event: FfiTransferEvent, activityID: String) {
        guard !removedActivityIDs.contains(activityID) else { return }
        var lines = transferEventLinesByActivityID[activityID] ?? []
        lines.append(TransferDiagnostics.transferEventLine(event))
        transferEventLinesByActivityID[activityID] = Array(lines.suffix(240))
    }

    func handleCoreStatus(_ message: String, activityID: String) {
        guard !removedActivityIDs.contains(activityID), !message.isEmpty else { return }
        var lines = transferLogByActivityID[activityID] ?? []
        lines.append("[\(activityLogTimestamp.string(from: Date()))] status · \(message)")
        transferLogByActivityID[activityID] = Array(lines.suffix(160))
    }

    func retainResourceAccess(_ access: AnyObject?, for activityID: String) {
        guard let access, !activityID.isEmpty else { return }
        activityResourceAccess[activityID] = access
    }

    func registerReceivePublication(
        activityID: String,
        destinationDirectory: URL,
        resourceAccess: AnyObject?
    ) {
        receivePublications[activityID] = ReceivePublication(
            destinationDirectory: destinationDirectory,
            resourceAccess: resourceAccess,
            completedRecord: nil,
            isPublishing: false
        )
        ReceivePublicationStore.save(
            ReceivePublicationTarget(
                destinationPath: destinationDirectory.path,
                bookmark: UserDefaults.standard.data(forKey: "envoix.outputDirBookmark")
            ),
            activityID: activityID
        )
        retainResourceAccess(resourceAccess, for: activityID)
    }

    func abandonReceivePublication(activityID: String) {
        receivePublications.removeValue(forKey: activityID)
        ReceivePublicationStore.remove(activityID: activityID)
        activityResourceAccess.removeValue(forKey: activityID)
    }

    private func beginReceivePublication(_ record: FfiTransferActivityRecord) {
        guard var publication = receivePublications[record.activityId],
              !publication.isPublishing else { return }
        publication.completedRecord = record
        publication.isPublishing = true
        receivePublications[record.activityId] = publication

        let source = URL(fileURLWithPath: record.completedFilePath)
        let destination = publication.destinationDirectory
        DispatchQueue.global(qos: .utility).async { [weak self] in
            let result = Result {
                try publishReceivedFile(
                    from: source,
                    to: destination,
                    expectedBytes: record.bytesTransferred
                )
            }
            DispatchQueue.main.async {
                self?.finishReceivePublication(record, result: result)
            }
        }
    }

    private func finishReceivePublication(
        _ record: FfiTransferActivityRecord,
        result: Result<URL, Error>
    ) {
        guard var publication = receivePublications[record.activityId] else { return }
        publication.isPublishing = false
        receivePublications[record.activityId] = publication
        switch result {
        case .success(let finalURL):
            guard durableSessions[record.activityId]?.publicationSucceeded(path: finalURL.path) == true else {
                publication.isPublishing = false
                receivePublications[record.activityId] = publication
                var pending = record
                pending.diagnosticMessage = "publish confirmation was not accepted"
                upsertActivity(pending, speedBps: 0)
                return
            }
            let stagingURL = URL(fileURLWithPath: record.completedFilePath)
            try? FileManager.default.removeItem(at: stagingURL)
            try? FileManager.default.removeItem(at: stagingURL.deletingLastPathComponent())
        case .failure(let error):
            var pending = record
            pending.updatedAtMs = UInt64(Date().timeIntervalSince1970 * 1000)
            pending.failureCode = .destinationConflict
            pending.failureCategory = .storage
            pending.failurePhase = .committing
            pending.failureOrigin = .local
            pending.userMessageKey = "transfer.publish_failed"
            pending.retryable = true
            pending.recoveryAction = .chooseFolder
            pending.diagnosticMessage = "publish failed: \(error.localizedDescription)"
            upsertActivity(pending, speedBps: 0)
        }
    }

    private func retryReceivePublication(_ activityID: String) -> Bool {
        guard let publication = receivePublications[activityID],
              !publication.isPublishing,
              let record = publication.completedRecord else { return false }
        beginReceivePublication(record)
        return true
    }

    private func cleanupReceiveStaging(activityID: String) {
        guard let supportDirectory = FileManager.default.urls(
            for: .applicationSupportDirectory,
            in: .userDomainMask
        ).first else { return }
        let stagingDirectory = supportDirectory
            .appendingPathComponent("envoix/receive-staging", isDirectory: true)
            .appendingPathComponent(activityID, isDirectory: true)
        try? FileManager.default.removeItem(at: stagingDirectory)
    }

    func diagnosticReport(for record: FfiTransferActivityRecord) -> String {
        let snapshot = diagnosticsSnapshot(for: record.activityId)
        return TransferDiagnostics.report(
            for: record,
            eventLog: snapshot.log,
            transferEventLines: snapshot.events,
        )
    }

    func remoteLogTarget(for record: FfiTransferActivityRecord) -> RemoteLogUpload.Target? {
        guard record.mode == .room else { return nil }
        guard let roomID = receive.roomID(for: record.activityId) ?? send.roomID(for: record.activityId) else {
            return nil
        }
        let side: String
        switch record.direction {
        case .send: side = "send"
        case .receive: side = "receive"
        case .unknown: return nil
        }
        return RemoteLogUpload.Target(roomID: roomID, side: side)
    }

    func remoteDiagnosticReport(for record: FfiTransferActivityRecord) -> String {
        let snapshot = diagnosticsSnapshot(for: record.activityId)
        return TransferDiagnostics.remoteReport(
            for: record,
            eventLog: snapshot.log,
            transferEventLines: snapshot.events,
        )
    }

    func appDiagnosticReport() -> String {
        let lines = activities.flatMap { record in
            let activityID = record.activityId
            let snapshot = diagnosticsSnapshot(for: activityID)
            return ["[\(activityID)]"]
                + snapshot.events
                + snapshot.log
        }
        return TransferDiagnostics.appReport(activities: activities, eventLines: lines)
    }

    private func diagnosticsSnapshot(for activityID: String) -> (log: [String], events: [String]) {
        if receive.ownsActivity(activityID) {
            return (receive.eventLog, receive.transferEvents.map(TransferDiagnostics.transferEventLine))
        }
        if send.ownsActivity(activityID) {
            return (send.eventLog, send.transferEvents.map(TransferDiagnostics.transferEventLine))
        }
        return (
            transferLogByActivityID[activityID] ?? [],
            transferEventLinesByActivityID[activityID] ?? []
        )
    }

    private func snapshotDiagnostics(from viewModel: TransferViewModel, activityID: String) {
        guard !activityID.isEmpty else { return }
        transferLogByActivityID[activityID] = viewModel.eventLog
        transferEventLinesByActivityID[activityID] = viewModel.transferEvents.map(TransferDiagnostics.transferEventLine)
    }

    private static func isTerminal(_ state: FfiTransferActivityState) -> Bool {
        switch state {
        case .completed, .failed, .canceled: return true
        case .queued, .binding, .waitingForPeer, .pairing, .connecting, .transferring,
                .verifying, .publishing, .unconfirmed, .paused, .unknown:
            return false
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
                receive.forgetRoomID(for: id)
                send.forgetRoomID(for: id)
                activityMetrics.removeValue(forKey: id)
                transferEventLinesByActivityID.removeValue(forKey: id)
                transferLogByActivityID.removeValue(forKey: id)
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
        case .publishing:
            message = record.diagnosticMessage.isEmpty ? "publishing" : record.diagnosticMessage
        case .unconfirmed:
            message = "delivery unconfirmed"
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

    weak var appModel: AppModel?

    private var session: DurableEnvoixSession?
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
    private static let roomIDStoreKey = "envoix.activityRoomIDs"
    private var displayLanguage = "en"
    private var currentActivityID = ""
    var activeActivityID: String { currentActivityID }
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

    var isFinalizing: Bool {
        guard let activity = transferActivity else { return false }
        return activity.state == .publishing
            || (activity.state == .verifying && activity.diagnosticMessage == "confirming")
    }

    // MARK: User actions

    /// Receive on the local network using a shared token (mDNS auto-discovery).
    func startReceivingWithToken(outputDir: String, token: String, settings: EnvoixRuntimeSettings, destinationAccess: AnyObject? = nil, publishDestinationDir: String? = nil) {
        startReceiving(
            outputDir: outputDir,
            publishDestinationDir: publishDestinationDir,
            destinationAccess: destinationAccess,
            settings: settings,
            mode: .mdns,
            token: token
        )
    }

    /// Receive by pairing through a rendezvous room code.
    func startReceivingWithRoom(outputDir: String, code: String, settings: EnvoixRuntimeSettings, destinationAccess: AnyObject? = nil, publishDestinationDir: String? = nil) {
        startReceiving(
            outputDir: outputDir,
            publishDestinationDir: publishDestinationDir,
            destinationAccess: destinationAccess,
            settings: settings,
            mode: .room,
            code: code
        )
    }

    /// Receive by publishing an invite the sender pastes/scans.
    func startReceivingWithInvite(outputDir: String, settings: EnvoixRuntimeSettings, destinationAccess: AnyObject? = nil, publishDestinationDir: String? = nil) {
        startReceiving(
            outputDir: outputDir,
            publishDestinationDir: publishDestinationDir,
            destinationAccess: destinationAccess,
            settings: settings,
            mode: .showInvite
        )
    }

    private func startReceiving(
        outputDir: String,
        publishDestinationDir: String?,
        destinationAccess: AnyObject?,
        settings: EnvoixRuntimeSettings,
        mode: FfiTransferMode,
        code: String = "",
        token: String = ""
    ) {
        do {
            let activityID = UUID().uuidString
            let coreOutputDir = try receiveOutputDir(
                activityID: activityID,
                directOutputDir: outputDir,
                requiresPublication: publishDestinationDir != nil
            )
            destinationDir = publishDestinationDir ?? outputDir
            let request = makeRequest(
                activityID: activityID,
                direction: .receive,
                mode: mode,
                settings: settings,
                outputDir: coreOutputDir,
                code: code,
                token: token,
                publicationRequired: publishDestinationDir != nil
            )
            if let publishDestinationDir {
                AppModel.shared.registerReceivePublication(
                    activityID: activityID,
                    destinationDirectory: URL(fileURLWithPath: publishDestinationDir, isDirectory: true),
                    resourceAccess: destinationAccess
                )
            }
            if mode == .room {
                rememberRoomID(for: request.activityId, code: code)
            }
            let started = start(settings: settings, request: request, phase: .waiting)
            guard started else {
                forgetRoomID(for: activityID)
                if publishDestinationDir != nil {
                    AppModel.shared.abandonReceivePublication(activityID: activityID)
                    try? FileManager.default.removeItem(atPath: coreOutputDir)
                }
                return
            }
            retainResourceAccess(destinationAccess)
        } catch {
            handleFailed(error.localizedDescription)
        }
    }

    /// Send on the local network using a shared token (mDNS auto-discovery).
    func startSendingWithToken(filePath: String, token: String, settings: EnvoixRuntimeSettings, sourceAccess: AnyObject? = nil) {
        destinationDir = nil
        let request = makeRequest(direction: .send, mode: .mdns, settings: settings, filePath: filePath, token: token)
        start(settings: settings, request: request, phase: .transferring)
        retainResourceAccess(sourceAccess)
    }

    /// Send by pairing through a rendezvous room code.
    func startSendingWithRoom(filePath: String, code: String, settings: EnvoixRuntimeSettings, sourceAccess: AnyObject? = nil) {
        destinationDir = nil
        let request = makeRequest(direction: .send, mode: .room, settings: settings, filePath: filePath, code: code)
        rememberRoomID(for: request.activityId, code: code)
        let started = start(settings: settings, request: request, phase: .waiting)
        if !started {
            forgetRoomID(for: request.activityId)
        }
        retainResourceAccess(sourceAccess)
    }

    /// Send to the peer encoded in an invite string.
    func startSendingWithInvite(filePath: String, invite: String, settings: EnvoixRuntimeSettings, sourceAccess: AnyObject? = nil) {
        destinationDir = nil
        let request = makeRequest(direction: .send, mode: .invite, settings: settings, filePath: filePath, invite: invite)
        start(settings: settings, request: request, phase: .transferring)
        retainResourceAccess(sourceAccess)
    }

    /// Requests cancellation without detaching the observer. Activity owns
    /// lifecycle controls after a transfer starts, so its terminal state must
    /// still flow back from the core before this view model resets.
    @discardableResult
    func requestCancelActivity(_ activityID: String) -> Bool {
        guard isBusy,
              !isFinalizing,
              !activityID.isEmpty,
              activityID == currentActivityID else { return false }
        suppressNextFailure = true
        let accepted = session?.cancel() ?? false
        if accepted {
            bytesPerSec = 0
            statusText = AppText.value("Cancelling…", "正在取消…", language: displayLanguage)
        } else {
            suppressNextFailure = false
        }
        return accepted
    }

    @discardableResult
    func discardActivityForRemoval(_ activityID: String) -> Bool {
        guard !activityID.isEmpty else { return false }
        let discarded = session?.remove() ?? false
        if activityID == currentActivityID {
            suppressNextFailure = true
            operationID = UUID()
            reset()
            phase = .canceled
            statusText = AppText.value("Transfer removed", "传输已删除", language: displayLanguage)
        }
        return discarded
    }

    func listTransferActivities() -> [FfiTransferActivityRecord] {
        session.map { [$0.activity()] } ?? []
    }

    func ownsActivity(_ activityID: String) -> Bool {
        !activityID.isEmpty && activityID == currentActivityID
    }

    func roomID(for activityID: String) -> String? {
        Self.persistedRoomIDs()[activityID]
    }

    func forgetRoomID(for activityID: String) {
        var roomIDs = Self.persistedRoomIDs()
        roomIDs.removeValue(forKey: activityID)
        Self.persistRoomIDs(roomIDs)
    }

    @discardableResult
    func pauseActivity(_ activityID: String) -> Bool {
        guard isBusy, !activityID.isEmpty, activityID == currentActivityID else { return false }
        let paused = session?.pause() ?? false
        if paused {
            bytesPerSec = 0
            statusText = AppText.value("Pausing…", "正在暂停…", language: displayLanguage)
        }
        return paused
    }

    @discardableResult
    func resumeActivity(_ activityID: String) -> Bool {
        guard !activityID.isEmpty, activityID == currentActivityID else { return false }
        let resumed = session?.resume() ?? false
        if resumed {
            suppressNextFailure = false
            phase = .waiting
            statusText = AppText.value("Resuming…", "正在继续…", language: displayLanguage)
        }
        return resumed
    }

    /// Creates one independently durable session for the new Activity card.
    @discardableResult
    private func start(
        settings: EnvoixRuntimeSettings,
        request: FfiTransferRequest,
        phase: Phase
    ) -> Bool {
        suppressNextFailure = false
        reset()
        currentActivityID = request.activityId
        displayLanguage = settings.language
        operationID = UUID()
        let operationID = operationID
        self.phase = phase
        do {
            guard let appModel else {
                throw RuntimeSettingsError("The transfer service is unavailable.")
            }
            let observer = Observer(
                self,
                appModel: appModel,
                operationID: operationID,
                activityID: request.activityId
            )
            session = try appModel.startDurableSession(
                settings: settings,
                request: request,
                observer: observer
            )
            return true
        } catch {
            self.phase = .failed(friendlyError(error.localizedDescription, language: displayLanguage))
            return false
        }
    }

    private func retainResourceAccess(_ access: AnyObject?) {
        if case .failed = phase {
            resourceAccess = nil
        } else {
            resourceAccess = access
            AppModel.shared.retainResourceAccess(access, for: currentActivityID)
        }
    }

    private func rememberRoomID(for activityID: String, code: String) {
        if let roomID = RemoteLogUpload.roomID(from: code) {
            var roomIDs = Self.persistedRoomIDs()
            roomIDs[activityID] = roomID
            Self.persistRoomIDs(roomIDs)
        }
    }

    private static func persistedRoomIDs() -> [String: String] {
        UserDefaults.standard.dictionary(forKey: roomIDStoreKey) as? [String: String] ?? [:]
    }

    private static func persistRoomIDs(_ roomIDs: [String: String]) {
        UserDefaults.standard.set(roomIDs, forKey: roomIDStoreKey)
    }

    private func makeRequest(
        activityID: String = UUID().uuidString,
        direction: FfiTransferDirection,
        mode: FfiTransferMode,
        settings: EnvoixRuntimeSettings,
        filePath: String = "",
        outputDir: String = "",
        invite: String = "",
        code: String = "",
        token: String = "",
        publicationRequired: Bool = false
    ) -> FfiTransferRequest {
        FfiTransferRequest(
            activityId: activityID,
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
            publicationRequired: publicationRequired,
            limits: FfiTransferLimits(
                maxParallelTransfers: settings.concurrentTransfers ? 2 : 1,
                maxParallelFiles: 1,
                maxParallelChunksPerFile: 1,
                speedLimitBps: 0
            ),
            rendezvous: rendezvousPlan(for: mode)
        )
    }

    private func receiveOutputDir(
        activityID: String,
        directOutputDir: String,
        requiresPublication: Bool
    ) throws -> String {
        guard requiresPublication else { return directOutputDir }
        guard let supportDirectory = FileManager.default.urls(
            for: .applicationSupportDirectory,
            in: .userDomainMask
        ).first else {
            throw RuntimeSettingsError("Could not create the receive staging directory.")
        }
        let stagingDirectory = supportDirectory
            .appendingPathComponent("envoix/receive-staging", isDirectory: true)
            .appendingPathComponent(activityID, isDirectory: true)
        try FileManager.default.createDirectory(
            at: stagingDirectory,
            withIntermediateDirectories: true
        )
        return stagingDirectory.path
    }

    private func rendezvousPlan(for mode: FfiTransferMode) -> FfiRendezvousPlan {
        switch mode {
        case .room:
            return FfiRendezvousPlan(
                useRoom: UserDefaults.standard.object(forKey: "envoix.useRoom") as? Bool ?? true,
                useMdns: UserDefaults.standard.object(forKey: "envoix.useMdns") as? Bool ?? true,
                internetAvailable: true
            )
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
        if record.state == .completed,
           record.direction == .receive,
           !record.completedFilePath.isEmpty {
            completedFileURL = availableCompletedFileURL(
                path: record.completedFilePath,
                expectedBytes: record.bytesTransferred
            )
            if completedFileURL == nil {
                statusText = AppText.value(
                    "Transfer confirmed, but the saved file is not currently available.",
                    "传输已确认，但当前保存位置中未找到该文件。",
                    language: displayLanguage
                )
            }
        }
        if record.state == .canceled {
            suppressNextFailure = false
            resourceAccess = nil
        }
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
        transferred = bytes
        total = max(total, bytes)
        bytesPerSec = 0
        if completedFileURL == nil, let dir = destinationDir, !fileName.isEmpty {
            completedFileURL = availableCompletedFileURL(
                path: URL(fileURLWithPath: dir).appendingPathComponent(fileName).path,
                expectedBytes: bytes
            )
        }
        resourceAccess = nil
        phase = .completed(bytes: bytes)
    }

    func handleTransferFailed(_ failure: FfiTransferFailure) {
        appendLog("failed · \(failure.diagnosticMessage)")
        if suppressNextFailure || transferActivity?.state == .canceled {
            return
        }
        self.failure = failure
        resourceAccess = nil
        phase = .failed(friendlyFailure(failure, language: displayLanguage))
    }

    func handleFailed(_ reason: String) {
        appendLog("failed · \(reason)")
        if suppressNextFailure || transferActivity?.state == .canceled {
            suppressNextFailure = false
            resourceAccess = nil
            phase = .canceled
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
        case .queued, .binding, .waitingForPeer, .pairing, .connecting:
            if phase == .paused || phase == .idle {
                phase = .waiting
            }
        case .verifying:
            if isFinalizing {
                bytesPerSec = 0
                statusText = AppText.value("Confirming delivery", "正在确认送达", language: displayLanguage)
            } else if phase == .paused || phase == .idle {
                phase = .waiting
            }
        case .publishing:
            bytesPerSec = 0
            phase = .waiting
            statusText = AppText.value("Saving to selected folder", "正在保存到所选文件夹", language: displayLanguage)
        case .unconfirmed:
            bytesPerSec = 0
            phase = .waiting
            statusText = AppText.value("Confirming delivery", "正在确认送达", language: displayLanguage)
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
    private weak var appModel: AppModel?
    private let operationID: UUID
    private let activityID: String

    init(
        _ viewModel: TransferViewModel?,
        appModel: AppModel,
        operationID: UUID,
        activityID: String
    ) {
        self.viewModel = viewModel
        self.appModel = appModel
        self.operationID = operationID
        self.activityID = activityID
    }

    func onInviteReady(invite: String) { hop { $0.handleInvite(invite) } }
    func onStarted(fileName: String, totalBytes: UInt64) { hop { $0.handleStarted(fileName, totalBytes) } }
    func onProgress(transferred: UInt64, total: UInt64) { hop { $0.handleProgress(transferred, total) } }
    func onCompleted(bytes: UInt64) { hop { $0.handleCompleted(bytes) } }
    func onTransferFailed(failure: FfiTransferFailure) { hop { $0.handleTransferFailed(failure) } }
    func onFailed(reason: String) { hop { $0.handleFailed(reason) } }
    func onTransferEvent(event: FfiTransferEvent) {
        DispatchQueue.main.async { [weak viewModel, weak appModel, operationID, activityID] in
            appModel?.handleCoreEvent(event, activityID: activityID)
            if let viewModel, viewModel.operationID == operationID {
                viewModel.handleTransferEvent(event)
            }
        }
    }

    func onTransferActivity(record: FfiTransferActivityRecord) {
        DispatchQueue.main.async { [weak viewModel, weak appModel, operationID] in
            appModel?.handleCoreActivity(record)
            if let viewModel, viewModel.operationID == operationID {
                viewModel.handleTransferActivity(record)
            }
        }
    }

    func onStatus(message: String) {
        DispatchQueue.main.async { [weak viewModel, weak appModel, operationID, activityID] in
            appModel?.handleCoreStatus(message, activityID: activityID)
            if let viewModel, viewModel.operationID == operationID {
                viewModel.handleStatus(message)
            }
        }
    }

    private func hop(_ body: @escaping (TransferViewModel) -> Void) {
        DispatchQueue.main.async { [weak viewModel, operationID] in
            if let viewModel, viewModel.operationID == operationID { body(viewModel) }
        }
    }
}
