import Combine
import EnvoixCore
import Foundation

struct ActivityMetrics {
    var speedBps: Double = 0
    var etaSeconds: Double?
    var log: [String] = []
}

enum TransferActivityState: Equatable {
    case preparing
    case waitingForPeer
    case pairing
    case connecting
    case awaitingDecision
    case transferring
    case verifying
    case saving
    case waitingForReceiverSave
    case finalizingDelivery
    case paused
    case delivered
    case failed
    case canceled
}

struct TransferActivityRecord: Identifiable {
    let activityId: String
    let direction: FfiTransferDirection
    let mode: FfiTransferMode
    var itemCount: UInt32
    var totalBytes: UInt64
    var bytesTransferred: UInt64
    var state: TransferActivityState
    var diagnosticMessage: String
    var failure: FfiTransferFailure?
    var savedPaths: [String]
    var roomID: String?
    var connectionPath: FfiDataPathKind?
    var updatedAt: Date

    var id: String { activityId }
}

enum ActivityProjectionPolicy {
    static func pendingCount(_ records: [TransferActivityRecord]) -> Int {
        records.lazy.filter { isPending($0.state) }.count
    }

    static func isPending(_ state: TransferActivityState) -> Bool {
        switch state {
        case .delivered, .failed, .canceled:
            return false
        default:
            return true
        }
    }
}

enum ActivityExecutionPolicy {
    static func occupiesExecutionSlot(_ state: TransferActivityState) -> Bool {
        switch state {
        case .paused, .delivered, .failed, .canceled:
            return false
        default:
            return true
        }
    }
}

private struct StoredRuntimeSettingsV2: Codable {
    let concurrentTransfers: Bool
    let language: String
    let serverURL: String
    let relayURL: String
    let configPath: String
    let speedLimitMbps: UInt64

    init(_ settings: EnvoixRuntimeSettings) {
        concurrentTransfers = settings.concurrentTransfers
        language = settings.language
        serverURL = settings.serverUrl
        relayURL = settings.relayUrl
        configPath = settings.configPath
        speedLimitMbps = settings.speedLimitMbps
    }

    var value: EnvoixRuntimeSettings {
        EnvoixRuntimeSettings(
            concurrentTransfers: concurrentTransfers,
            language: language,
            serverUrl: serverURL,
            relayUrl: relayURL,
            configPath: configPath,
            speedLimitMbps: speedLimitMbps
        )
    }
}

private struct StoredTransferRequestV2: Codable {
    let direction: String
    let mode: String
    let peerDescriptor: String
    let invite: String
    let code: String
    let token: String
    let broker: String
    let relay: String
    let configPath: String
    let pathPolicy: String
    let useRoom: Bool
    let useMdns: Bool
    let internetAvailable: Bool

    init(_ request: FfiTransferRequest) {
        direction = request.direction == .send ? "send" : "receive"
        switch request.mode {
        case .manual: mode = "manual"
        case .invite: mode = "invite"
        case .remembered: mode = "remembered"
        case .showManual: mode = "show_manual"
        case .showInvite: mode = "show_invite"
        case .mdns: mode = "mdns"
        case .room: mode = "room"
        }
        peerDescriptor = request.peerDescriptor
        invite = ""
        code = ""
        token = ""
        broker = request.broker
        relay = request.relay
        configPath = request.configPath
        switch request.pathPolicy {
        case .auto: pathPolicy = "auto"
        case .relayOnly: pathPolicy = "relay_only"
        case .directOnly: pathPolicy = "direct_only"
        }
        useRoom = request.rendezvous.useRoom
        useMdns = request.rendezvous.useMdns
        internetAvailable = request.rendezvous.internetAvailable
    }

    func value() throws -> FfiTransferRequest {
        let direction: FfiTransferDirection
        switch self.direction {
        case "send": direction = .send
        case "receive": direction = .receive
        default: throw RuntimeSettingsError("Stored transfer direction is invalid.")
        }
        let mode: FfiTransferMode
        switch self.mode {
        case "manual": mode = .manual
        case "invite": mode = .invite
        case "remembered": mode = .remembered
        case "show_manual": mode = .showManual
        case "show_invite": mode = .showInvite
        case "mdns": mode = .mdns
        case "room": mode = .room
        default: throw RuntimeSettingsError("Stored transfer mode is invalid.")
        }
        let pathPolicy: FfiPathPolicy
        switch self.pathPolicy {
        case "auto": pathPolicy = .auto
        case "relay_only": pathPolicy = .relayOnly
        case "direct_only": pathPolicy = .directOnly
        default: throw RuntimeSettingsError("Stored path policy is invalid.")
        }
        return FfiTransferRequest(
            direction: direction,
            mode: mode,
            peerDescriptor: peerDescriptor,
            invite: invite,
            code: code,
            token: token,
            rememberConsent: false,
            rememberedCredentialRef: "",
            rememberedGeneration: 0,
            rememberedPreviousGeneration: nil,
            broker: broker,
            relay: relay,
            configPath: configPath,
            pathPolicy: pathPolicy,
            rendezvous: FfiRendezvousPlan(
                useRoom: useRoom,
                useMdns: useMdns,
                internetAvailable: internetAvailable
            )
        )
    }
}

private struct StoredAppleManifestSessionV2: Codable {
    static let schemaVersion = 1

    let schemaVersion: Int
    let activityID: String
    let jobID: String?
    let stateDirectory: String
    let targetDirectory: String?
    let sourcePaths: [String]
    let sourceBookmarks: [Data]
    let destinationBookmark: Data?
    let shareDraftID: UUID?
    let settings: StoredRuntimeSettingsV2
    let request: StoredTransferRequestV2
    let itemCount: UInt32
    let totalBytes: UInt64
    let roomID: String?
}

@MainActor
final class AppModel: ObservableObject {
    static let shared = AppModel()

    let receive = TransferViewModel()
    let send = TransferViewModel()
    @Published private(set) var activities: [TransferActivityRecord] = []
    @Published private(set) var pendingActivityRemovalIDs = Set<String>()
    @Published private(set) var activityMetrics: [String: ActivityMetrics] = [:]
    @Published private(set) var transferCacheSummary = TransferCacheSummary()
    @Published private(set) var isCleaningTransferCache = false
    @Published private(set) var transferCacheError: String?
    #if os(iOS)
    @Published private(set) var pendingSendSelection: PendingSendSelection?
    #endif

    private var cancellables = Set<AnyCancellable>()
    #if os(macOS)
    private var appLifetimeDestinationAccess: [String: SecurityScopedResourceAccess] = [:]
    #endif

    private init() {
        receive.appModel = self
        send.appModel = self
        for model in [receive, send] {
            model.objectWillChange
                .sink { [weak self] in self?.objectWillChange.send() }
                .store(in: &cancellables)
            model.$transferActivity
                .compactMap { $0 }
                .sink { [weak self] in self?.upsert($0) }
                .store(in: &cancellables)
        }
        send.restoreActiveManifestSession(direction: .send)
        receive.restoreActiveManifestSession(direction: .receive)
    }

    var isActive: Bool {
        send.isBusy || receive.isBusy || activities.contains { ActivityProjectionPolicy.isPending($0.state) }
    }

    var hasExecutingActivity: Bool {
        activities.contains { ActivityExecutionPolicy.occupiesExecutionSlot($0.state) }
    }

    #if os(macOS)
    func retainDestinationAccessForAppLifetime(_ access: SecurityScopedResourceAccess?) {
        guard let access else { return }
        appLifetimeDestinationAccess[access.url.standardizedFileURL.path] = access
    }
    #endif

    @discardableResult
    func pauseActivity(_ activityID: String) -> Bool {
        owner(of: activityID)?.pause() ?? false
    }

    func canResumeActivity(_ activityID: String) -> Bool {
        guard let record = activities.first(where: { $0.activityId == activityID }),
              owner(of: activityID)?.hasResumableOperation == true else {
            return false
        }
        return record.state == .paused || (record.state == .failed && record.failure?.retryable == true)
    }

    @discardableResult
    func resumeActivity(_ activityID: String) -> Bool {
        owner(of: activityID)?.resume() ?? false
    }

    @discardableResult
    func cancelActivity(_ activityID: String) -> Bool {
        owner(of: activityID)?.cancel() ?? false
    }

    @discardableResult
    func approveActivity(_ activityID: String) -> Bool {
        owner(of: activityID)?.approveExceptionalTransfer() ?? false
    }

    @discardableResult
    func removeActivity(_ activityID: String) -> Bool {
        guard let record = activities.first(where: { $0.activityId == activityID }),
              !ActivityProjectionPolicy.isPending(record.state) else { return false }
        activities.removeAll { $0.activityId == activityID }
        activityMetrics.removeValue(forKey: activityID)
        owner(of: activityID)?.forgetActivity(activityID)
        return true
    }

    func diagnosticReport(_ record: TransferActivityRecord) -> String {
        [
            "activity_id=\(record.activityId)",
            "direction=\(record.direction)",
            "mode=\(record.mode)",
            "state=\(record.state)",
            "items=\(record.itemCount)",
            "bytes=\(record.bytesTransferred)/\(record.totalBytes)",
            "diagnostic=\(record.diagnosticMessage)",
        ].joined(separator: "\n")
    }

    func remoteLogTarget(_ record: TransferActivityRecord) -> RemoteLogUpload.Target? {
        guard let roomID = record.roomID else { return nil }
        return RemoteLogUpload.Target(
            roomID: roomID,
            side: record.direction == .send ? "sender" : "receiver"
        )
    }

    func remoteDiagnosticReport(_ record: TransferActivityRecord) -> String {
        [
            "activity_id=\(record.activityId)",
            "direction=\(record.direction)",
            "mode=\(record.mode)",
            "state=\(record.state)",
            "items=\(record.itemCount)",
            "bytes=\(record.bytesTransferred)/\(record.totalBytes)",
            "failure_code=\(String(describing: record.failure?.code))",
            "failure_category=\(String(describing: record.failure?.category))",
            "failure_phase=\(String(describing: record.failure?.phase))",
            "retryable=\(record.failure?.retryable ?? false)",
            "recovery_action=\(String(describing: record.failure?.recoveryAction))",
        ].joined(separator: "\n")
    }

    func appDiagnosticReport() -> String {
        activities.map(diagnosticReport).joined(separator: "\n\n")
    }

    func refreshTransferCache() {
        performTransferCacheWork(clean: false)
    }

    func cleanTransferCache() {
        performTransferCacheWork(clean: true)
    }

    private func performTransferCacheWork(clean: Bool) {
        guard !isCleaningTransferCache else { return }
        isCleaningTransferCache = true
        transferCacheError = nil
        let protected = Set(activities.filter { ActivityProjectionPolicy.isPending($0.state) }.map(\.activityId))
        #if os(iOS)
        let drafts = Set(
            [pendingSendSelection?.id, send.protectedShareDraftID].compactMap { $0 }
        )
        #else
        let drafts = Set<UUID>()
        #endif
        Task.detached {
            do {
                let store = TransferCacheStore()
                if clean {
                    try store.cleanUnprotected(
                        protectingDraftIDs: drafts,
                        protectingActivityIDs: protected,
                        createdBefore: Date()
                    )
                }
                let summary = try store.summary(
                    protectingDraftIDs: drafts,
                    protectingActivityIDs: protected
                )
                await MainActor.run {
                    self.transferCacheSummary = summary
                    self.isCleaningTransferCache = false
                }
            } catch {
                await MainActor.run {
                    self.transferCacheError = error.localizedDescription
                    self.isCleaningTransferCache = false
                }
            }
        }
    }

    #if os(iOS)
    func importSharedSendDraft(preferredID: UUID? = nil) throws -> SharedSendImportOutcome {
        guard !send.isBusy else { return .sendBusy }
        let store = try ShareDraftStore.live()
        try store.cleanupExpired()
        guard let draft = try store.pending(preferredID: preferredID) else {
            return .noPendingDraft
        }
        if pendingSendSelection?.id != draft.descriptor.id {
            try store.claim(id: draft.descriptor.id)
            pendingSendSelection = PendingSendSelection(
                id: draft.descriptor.id,
                fileURLs: draft.fileURLs,
                sourceAccess: ShareDraftLease(id: draft.descriptor.id, store: store)
            )
        }
        return .imported
    }

    func importOpenedSendFile(_ url: URL) throws -> OpenedSendFileOutcome {
        guard url.isFileURL else { throw OpenedSendFileError.unsupportedURL }
        let access = SecurityScopedResourceAccess(url: url)
        guard access.isActive || FileManager.default.isReadableFile(atPath: url.path) else {
            throw OpenedSendFileError.inaccessible
        }
        let values = try url.resourceValues(forKeys: [
            .isRegularFileKey,
            .isDirectoryKey,
            .isSymbolicLinkKey,
        ])
        guard values.isSymbolicLink != true,
              values.isRegularFile == true || values.isDirectory == true else {
            throw OpenedSendFileError.unsupportedItem
        }
        pendingSendSelection = PendingSendSelection(
            id: UUID(),
            fileURLs: [url],
            sourceAccess: access
        )
        return send.isBusy ? .queued : .imported
    }

    func consumePendingSendSelection(id: UUID) {
        if pendingSendSelection?.id == id { pendingSendSelection = nil }
    }
    #endif

    fileprivate func upsert(_ record: TransferActivityRecord) {
        if let index = activities.firstIndex(where: { $0.activityId == record.activityId }) {
            activities[index] = record
        } else {
            activities.insert(record, at: 0)
        }
        activities.sort { $0.updatedAt > $1.updatedAt }
        if activities.count > 50 { activities.removeLast(activities.count - 50) }
        var metrics = activityMetrics[record.activityId] ?? ActivityMetrics()
        if let owner = owner(of: record.activityId) {
            metrics.speedBps = owner.bytesPerSec
            metrics.etaSeconds = owner.etaSeconds
            metrics.log = owner.eventLog
        }
        activityMetrics[record.activityId] = metrics
    }

    private func owner(of activityID: String) -> TransferViewModel? {
        if send.ownsActivity(activityID) { return send }
        if receive.ownsActivity(activityID) { return receive }
        return nil
    }
}

@MainActor
final class TransferViewModel: ObservableObject {
    private static let activeSendSessionFileName = "active-send.json"
    private static let activeReceiveSessionFileName = "active-receive.json"

    private struct PreparedSelection {
        let job: FfiTransferJobV2
        let jobID: String
        var sourcePaths: [String]
        let sessionStateDirectory: String
        let sourceAccess: AnyObject?
    }

    private struct SendOperation {
        let job: FfiTransferJobV2
        let jobID: String
        let sourcePaths: [String]
        let settings: EnvoixRuntimeSettings
        let request: FfiTransferRequest
        let stateDirectory: String
        let sourceAccess: AnyObject?
        let rememberPersistence: RememberPersistenceContext?
    }

    private struct ReceiveOperation {
        let settings: EnvoixRuntimeSettings
        let request: FfiTransferRequest
        let stateDirectory: String
        let targetDirectory: String
        let destinationAccess: AnyObject?
        let rememberPersistence: RememberPersistenceContext?
    }

    @MainActor
    private final class ReceiveLaunchSignal {
        private var continuation: CheckedContinuation<String?, Never>?

        init(_ continuation: CheckedContinuation<String?, Never>) {
            self.continuation = continuation
        }

        func resolve(_ activityID: String?) {
            continuation?.resume(returning: activityID)
            continuation = nil
        }
    }

    /// The one lifecycle state rendered by setup, Activity, and menu-bar
    /// surfaces. `nil` means there is no active or terminal transfer to show.
    @Published private(set) var presentationState: TransferActivityState?
    @Published var invite = ""
    @Published var fileName = ""
    @Published var transferred: UInt64 = 0
    @Published var total: UInt64 = 0
    @Published var statusText = ""
    @Published var peerAddress = ""
    @Published var eventLog: [String] = []
    @Published var bytesPerSec: Double = 0
    @Published var completedFileURL: URL?
    @Published var completedItemURLs: [URL] = []
    @Published private(set) var failure: FfiTransferFailure?
    @Published private(set) var transferActivity: TransferActivityRecord?
    @Published private(set) var isPreparingManifest = false
    @Published private(set) var isManifestSelectionReady = false
    @Published private(set) var preparedManifestSourcePaths: [String] = []
    @Published private(set) var preparedInventorySummary: FfiInventorySummaryV2?
    @Published private(set) var preparedInventoryRoots: [FfiInventoryItemV2] = []
    @Published private(set) var requiresExceptionalTransferApproval = false
    @Published private(set) var pendingOfferSummary: FfiManifestOfferSummaryV2?
    @Published private(set) var pendingOfferEntries: [FfiManifestOfferEntryV2] = []
    @Published private(set) var pendingSourceSelections: [FfiSourceSelectionV2] = []
    @Published private(set) var connectionPath: FfiConnectionPathEvent?

    weak var appModel: AppModel?
    private var preparedSelection: PreparedSelection?
    private var activeSend: SendOperation?
    private var activeReceive: ReceiveOperation?
    private var pendingReceive: FfiPendingManifestV2Receive?
    private var cancellation: FfiManifestV2Cancellation?
    private var task: Task<Void, Never>?
    private var preparationTask: Task<Void, Never>?
    private var resourceAccess: AnyObject?
    private var rate = RateTracker()
    private var observedTransferred: UInt64 = 0
    private var observedTotal: UInt64 = 0
    private var lastProgressPublishAt = Date.distantPast
    fileprivate var operationID = UUID()
    private var pausedByUser = false
    private var displayLanguage = "en"

    #if os(iOS)
    var protectedShareDraftID: UUID? {
        if let id = (preparedSelection?.sourceAccess as? ShareDraftLease)?.id {
            return id
        }
        if transferActivity?.state == .delivered || transferActivity?.state == .canceled {
            return nil
        }
        return (activeSend?.sourceAccess as? ShareDraftLease)?.id
    }
    #endif

    var progressFraction: Double { total > 0 ? Double(transferred) / Double(total) : 0 }
    var etaSeconds: Double? {
        estimatedRemainingSeconds(
            total: total,
            transferred: transferred,
            bytesPerSecond: bytesPerSec,
            isStable: rate.isStable
        )
    }
    var isBusy: Bool {
        if isPreparingManifest { return true }
        return presentationState.map(ActivityProjectionPolicy.isPending) ?? false
    }
    var isFinalizing: Bool {
        presentationState.map(TransferPresentationPolicy.isFinalizing) ?? false
    }
    fileprivate var hasResumableOperation: Bool {
        activeSend != nil || activeReceive != nil
    }

    func prepareManifestSelection(selectedPaths: [String], sourceAccess: AnyObject? = nil) {
        let paths = normalizedPaths(selectedPaths)
        guard !paths.isEmpty,
              preparedSelection?.sourcePaths != paths else { return }
        preparationTask?.cancel()
        if let old = preparedSelection?.job { Task { _ = try? await old.cancelJob() } }
        preparedSelection = nil
        preparedManifestSourcePaths = []
        preparedInventorySummary = nil
        preparedInventoryRoots = []
        pendingSourceSelections = []
        isManifestSelectionReady = false
        isPreparingManifest = true
        statusText = localized("Preparing selected items…", "正在准备所选项目…")
        presentationState = .preparing
        failure = nil
        let expected = UUID()
        operationID = expected
        preparationTask = Task { @MainActor [weak self] in
            guard let self else { return }
            do {
                let job = try await createTransferJobV2(
                    storeDirectory: try jobStoreDirectory(),
                    compressionPolicy: compressionPolicy
                )
                let snapshot = try await job.addLocalPaths(paths: paths)
                guard expected == operationID else {
                    _ = try? await job.cancelJob()
                    return
                }
                preparedSelection = PreparedSelection(
                    job: job,
                    jobID: snapshot.jobId,
                    sourcePaths: paths,
                    sessionStateDirectory: try sessionStateDirectory(jobID: snapshot.jobId),
                    sourceAccess: sourceAccess
                )
                applyPreparation(
                    snapshot,
                    paths: await projectedSourcePaths(job: job, snapshot: snapshot),
                    roots: await job.listRoots()
                )
            } catch {
                guard expected == operationID else { return }
                handleFailed(error.localizedDescription)
            }
            isPreparingManifest = false
            preparationTask = nil
        }
    }

    func restoreActiveManifestSession(direction: FfiTransferDirection) {
        guard transferActivity == nil, !isBusy else { return }
        let stored: StoredAppleManifestSessionV2
        let request: FfiTransferRequest
        do {
            guard let value = try storedManifestSession(direction: direction) else { return }
            stored = value
            guard value.request.invite.isEmpty,
                  value.request.code.isEmpty,
                  value.request.token.isEmpty else {
                throw RuntimeSettingsError("Stored transfer contains obsolete authentication data.")
            }
            request = try value.request.value()
            guard request.direction == direction else {
                throw RuntimeSettingsError("Stored transfer direction does not match its session slot.")
            }
            if usesProcessOnlyAuthentication(request.mode) {
                clearStoredManifestSession(direction: direction)
                eventLog.append("discarded process-authenticated session after relaunch")
                return
            }
        } catch {
            clearStoredManifestSession(direction: direction)
            eventLog.append("discarded invalid saved session: \(error.localizedDescription)")
            return
        }

        displayLanguage = stored.settings.language
        operationID = UUID()
        transferred = 0
        total = stored.totalBytes
        fileName = stored.itemCount == 1
            ? localized("1 item", "1 个项目")
            : localized("\(stored.itemCount) items", "\(stored.itemCount) 个项目")
        transferActivity = TransferActivityRecord(
            activityId: stored.activityID,
            direction: direction,
            mode: request.mode,
            itemCount: stored.itemCount,
            totalBytes: stored.totalBytes,
            bytesTransferred: 0,
            state: direction == .send ? .connecting : .waitingForPeer,
            diagnosticMessage: localized("Restoring interrupted transfer", "正在恢复中断的传输"),
            failure: nil,
            savedPaths: [],
            roomID: stored.roomID,
            connectionPath: nil,
            updatedAt: Date()
        )
        presentationState = transferActivity?.state
        statusText = localized("Restoring interrupted transfer", "正在恢复中断的传输")
        if let transferActivity { appModel?.upsert(transferActivity) }

        let expected = operationID
        task = Task { @MainActor [weak self] in
            guard let self else { return }
            do {
                if direction == .send {
                    guard let jobID = stored.jobID else {
                        throw RuntimeSettingsError("Stored sender session has no job identifier.")
                    }
                    let job = try await restoreTransferJobV2(
                        storeDirectory: try jobStoreDirectory(),
                        jobId: jobID
                    )
                    let sourceAccess = try restoreSourceAccess(stored)
                    guard expected == operationID else { return }
                    launchSend(SendOperation(
                        job: job,
                        jobID: jobID,
                        sourcePaths: stored.sourcePaths,
                        settings: stored.settings.value,
                        request: request,
                        stateDirectory: stored.stateDirectory,
                        sourceAccess: sourceAccess,
                        rememberPersistence: nil
                    ))
                } else {
                    guard let targetDirectory = stored.targetDirectory else {
                        throw RuntimeSettingsError("Stored receiver session has no destination.")
                    }
                    let restored = try restoreDestinationAccess(
                        bookmark: stored.destinationBookmark,
                        fallbackPath: targetDirectory
                    )
                    guard expected == operationID else { return }
                    launchReceive(ReceiveOperation(
                        settings: stored.settings.value,
                        request: request,
                        stateDirectory: stored.stateDirectory,
                        targetDirectory: restored.path,
                        destinationAccess: restored.access,
                        rememberPersistence: nil
                    ))
                }
            } catch {
                guard expected == operationID else { return }
                handleFailed(error.localizedDescription)
            }
        }
    }

    @discardableResult
    func cancelManifestPreparation() -> Bool {
        guard isPreparingManifest || preparedSelection != nil else { return false }
        preparationTask?.cancel()
        if let job = preparedSelection?.job { Task { _ = try? await job.cancelJob() } }
        preparedSelection = nil
        preparedManifestSourcePaths = []
        preparedInventorySummary = nil
        preparedInventoryRoots = []
        pendingSourceSelections = []
        isPreparingManifest = false
        isManifestSelectionReady = false
        statusText = localized("Canceled", "已取消")
        if transferActivity == nil {
            presentationState = .canceled
        } else {
            operationID = UUID()
            updateActivity(state: .canceled, diagnostic: statusText)
        }
        return true
    }

    func approvePartialManifestSource(rootItemID: UInt64) {
        resolveSource(rootItemID: rootItemID, decision: .approvePartial, path: nil)
    }

    func removeManifestSource(rootItemID: UInt64) {
        resolveSource(rootItemID: rootItemID, decision: .removeSelection, path: nil)
    }

    func reauthorizeManifestSource(rootItemID: UInt64, path: String) {
        resolveSource(rootItemID: rootItemID, decision: .reauthorize, path: path)
    }

    func startSendingManifestWithToken(
        selectedPaths: [String],
        token: String,
        settings: EnvoixRuntimeSettings,
        sourceAccess: AnyObject? = nil
    ) {
        startSend(
            selectedPaths: selectedPaths,
            settings: settings,
            request: request(direction: .send, mode: .mdns, settings: settings, token: token),
            sourceAccess: sourceAccess
        )
    }

    func startSendingManifestWithRoom(
        selectedPaths: [String],
        code: String,
        settings: EnvoixRuntimeSettings,
        sourceAccess: AnyObject? = nil,
        rememberLabel: String? = nil
    ) {
        let persistence = prepareRememberPersistence(label: rememberLabel, settings: settings)
        if rememberLabel?.trimmed.isEmpty == false, persistence == nil { return }
        startSend(
            selectedPaths: selectedPaths,
            settings: settings,
            request: request(
                direction: .send,
                mode: .room,
                settings: settings,
                code: code,
                rememberConsent: persistence != nil
            ),
            sourceAccess: sourceAccess,
            roomCode: code,
            rememberPersistence: persistence
        )
    }

    func startSendingManifestWithInvite(
        selectedPaths: [String],
        invite: String,
        settings: EnvoixRuntimeSettings,
        pathPolicy: FfiPathPolicy = .auto,
        sourceAccess: AnyObject? = nil,
        rememberLabel: String? = nil
    ) {
        let persistence = prepareRememberPersistence(label: rememberLabel, settings: settings)
        if rememberLabel?.trimmed.isEmpty == false, persistence == nil { return }
        startSend(
            selectedPaths: selectedPaths,
            settings: settings,
            request: request(
                direction: .send,
                mode: .invite,
                settings: settings,
                invite: invite,
                rememberConsent: persistence != nil,
                pathPolicy: pathPolicy
            ),
            sourceAccess: sourceAccess,
            rememberPersistence: persistence
        )
    }

    func startSendingManifestToRememberedPeer(
        selectedPaths: [String],
        peer: RememberedPeerSummary,
        settings: EnvoixRuntimeSettings,
        sourceAccess: AnyObject? = nil
    ) {
        do {
            let persistence = try RememberPersistenceContext(peer: peer)
            do {
                let credential = try RememberedPeerStore.shared.credential(for: peer)
                let handle = try registerProtectedRememberedCredential(opaqueCredential: credential)
                startSend(
                    selectedPaths: selectedPaths,
                    settings: settings,
                    request: request(
                        direction: .send,
                        mode: .remembered,
                        settings: settings,
                        rememberedCredentialRef: handle,
                        rememberedGeneration: peer.generation,
                        rememberedPreviousGeneration: peer.previousGeneration
                    ),
                    sourceAccess: sourceAccess,
                    rememberPersistence: persistence
                )
            } catch {
                RememberedPeerStore.shared.delete(relationshipID: peer.relationshipID)
                handleFailed(error.localizedDescription)
            }
        } catch {
            handleFailed(error.localizedDescription)
        }
    }

    func startReceivingWithToken(
        outputDir: String,
        token: String,
        settings: EnvoixRuntimeSettings,
        destinationAccess: AnyObject? = nil
    ) {
        startReceive(
            targetDirectory: outputDir,
            settings: settings,
            request: request(direction: .receive, mode: .mdns, settings: settings, token: token),
            destinationAccess: destinationAccess
        )
    }

    func startReceivingWithRoom(
        outputDir: String,
        code: String,
        settings: EnvoixRuntimeSettings,
        destinationAccess: AnyObject? = nil,
        rememberLabel: String? = nil
    ) {
        let persistence = prepareRememberPersistence(label: rememberLabel, settings: settings)
        if rememberLabel?.trimmed.isEmpty == false, persistence == nil { return }
        startReceive(
            targetDirectory: outputDir,
            settings: settings,
            request: request(
                direction: .receive,
                mode: .room,
                settings: settings,
                code: code,
                rememberConsent: persistence != nil
            ),
            destinationAccess: destinationAccess,
            roomCode: code,
            rememberPersistence: persistence
        )
    }

    func startReceivingWithInvite(
        outputDir: String,
        invite: String,
        settings: EnvoixRuntimeSettings,
        destinationAccess: AnyObject? = nil,
        rememberLabel: String? = nil
    ) {
        let persistence = prepareRememberPersistence(label: rememberLabel, settings: settings)
        if rememberLabel?.trimmed.isEmpty == false, persistence == nil { return }
        startReceive(
            targetDirectory: outputDir,
            settings: settings,
            request: request(
                direction: .receive,
                mode: .invite,
                settings: settings,
                invite: invite,
                rememberConsent: persistence != nil
            ),
            destinationAccess: destinationAccess,
            rememberPersistence: persistence
        )
    }

    /// Starts a receiver for a room-control offer and returns only when its
    /// launch task has persisted the operation and is entering the FFI wait.
    func startReceivingRoomControlInvite(
        outputDir: String,
        invite: String,
        settings: EnvoixRuntimeSettings,
        destinationAccess: AnyObject? = nil
    ) async -> String? {
        displayLanguage = settings.language
        let request = request(
            direction: .receive,
            mode: .invite,
            settings: settings,
            invite: invite
        )
        beginActivity(direction: .receive, mode: request.mode, roomCode: nil)
        do {
            guard let activityID = transferActivity?.activityId else {
                throw RuntimeSettingsError("Cannot start a receiver without an activity.")
            }
            let operation = ReceiveOperation(
                settings: settings,
                request: request,
                stateDirectory: try receiveStateDirectory(activityID: activityID),
                targetDirectory: outputDir,
                destinationAccess: destinationAccess,
                rememberPersistence: nil
            )
            activeReceive = operation
            activeSend = nil
            return await withCheckedContinuation { continuation in
                launchReceive(
                    operation,
                    launchSignal: ReceiveLaunchSignal(continuation)
                )
            }
        } catch {
            handleFailed(error.localizedDescription)
            return nil
        }
    }

    func startReceivingFromRememberedPeer(
        outputDir: String,
        peer: RememberedPeerSummary,
        settings: EnvoixRuntimeSettings,
        destinationAccess: AnyObject? = nil
    ) {
        do {
            let persistence = try RememberPersistenceContext(peer: peer)
            do {
                let credential = try RememberedPeerStore.shared.credential(for: peer)
                let handle = try registerProtectedRememberedCredential(opaqueCredential: credential)
                startReceive(
                    targetDirectory: outputDir,
                    settings: settings,
                    request: request(
                        direction: .receive,
                        mode: .remembered,
                        settings: settings,
                        rememberedCredentialRef: handle,
                        rememberedGeneration: peer.generation,
                        rememberedPreviousGeneration: peer.previousGeneration
                    ),
                    destinationAccess: destinationAccess,
                    rememberPersistence: persistence
                )
            } catch {
                RememberedPeerStore.shared.delete(relationshipID: peer.relationshipID)
                handleFailed(error.localizedDescription)
            }
        } catch {
            handleFailed(error.localizedDescription)
        }
    }

    @discardableResult
    func approveExceptionalTransfer() -> Bool {
        guard presentationState == .awaitingDecision,
              requiresExceptionalTransferApproval,
              let pendingReceive,
              let operation = activeReceive else { return false }
        requiresExceptionalTransferApproval = false
        continueReceive(pendingReceive, operation: operation, exceptionalApproved: true)
        return true
    }

    @discardableResult
    func pause() -> Bool {
        guard let state = presentationState,
              TransferPresentationPolicy.actions(for: state, failure: failure).canPause else {
            return false
        }
        pausedByUser = true
        cancellation?.cancel()
        operationID = UUID()
        publishObservedProgress()
        pendingReceive = nil
        presentationState = .paused
        updateActivity(state: .paused, diagnostic: localized("Paused; progress is retained", "已暂停；进度已保留"))
        return true
    }

    @discardableResult
    func resume() -> Bool {
        let mayResume: Bool
        switch presentationState {
        case .paused?:
            mayResume = true
        case .failed?:
            mayResume = failure?.retryable == true
        default:
            mayResume = false
        }
        guard mayResume else { return false }
        guard hasResumableOperation else { return false }
        pausedByUser = false
        failure = nil
        transferActivity?.failure = nil
        operationID = UUID()
        rate = RateTracker()
        presentationState = nil
        if let activeSend {
            launchSend(activeSend)
            return true
        }
        if let activeReceive {
            launchReceive(activeReceive)
            return true
        }
        return false
    }

    @discardableResult
    func cancel() -> Bool {
        guard let state = presentationState,
              TransferPresentationPolicy.actions(for: state, failure: failure).canCancel else {
            return false
        }
        pausedByUser = false
        cancellation?.cancel()
        operationID = UUID()
        publishObservedProgress()
        pendingReceive = nil
        requiresExceptionalTransferApproval = false
        presentationState = .canceled
        updateActivity(state: .canceled, diagnostic: localized("Canceled", "已取消"))
        if let direction = transferActivity?.direction {
            clearStoredManifestSession(direction: direction)
            if direction == .send {
                activeSend = nil
            } else {
                activeReceive = nil
            }
        }
        resourceAccess = nil
        return true
    }

    func forgetActivity(_ activityID: String) {
        guard ownsActivity(activityID) else { return }
        if let direction = transferActivity?.direction {
            clearStoredManifestSession(direction: direction)
        }
        transferActivity = nil
        presentationState = nil
        activeSend = nil
        activeReceive = nil
        resourceAccess = nil
    }

    func ownsActivity(_ activityID: String) -> Bool {
        transferActivity?.activityId == activityID
    }

    func handleFailed(_ reason: String) {
        guard failure == nil, presentationState != .canceled, !pausedByUser else { return }
        let projected = FfiTransferFailure(
            code: .internalError,
            category: .internal,
            phase: .setup,
            origin: .local,
            direction: transferActivity?.direction ?? .send,
            retryable: false,
            recoveryAction: .none,
            userMessageKey: "transfer.internal_error",
            diagnosticMessage: reason
        )
        handleTransferFailed(projected)
    }

    fileprivate func handleInvite(_ value: String) {
        invite = value
        if value.contains("@") { peerAddress = value }
        updateActivity(state: .waitingForPeer, diagnostic: localized("Waiting for sender", "等待发送方"))
    }

    fileprivate func handleStarted(itemCount: UInt32, totalBytes: UInt64) {
        fileName = itemCount == 1 ? localized("1 item", "1 个项目") : localized("\(itemCount) items", "\(itemCount) 个项目")
        total = totalBytes
        transferActivity?.itemCount = itemCount
        transferActivity?.totalBytes = totalBytes
        updateActivity(state: .transferring, diagnostic: localized("Transferring", "正在传输"))
    }

    fileprivate func handleConnectionPath(_ event: FfiConnectionPathEvent) {
        connectionPath = event
        guard var record = transferActivity else { return }
        record.connectionPath = event.pathKind
        record.updatedAt = Date()
        transferActivity = record
        appModel?.upsert(record)
    }

    fileprivate func handlePhase(_ next: FfiManifestV2Phase) {
        let state: TransferActivityState
        let text: String
        switch next {
        case .pairing:
            state = .pairing; text = localized("Pairing", "正在配对")
        case .connecting:
            state = .connecting; text = localized("Connecting", "正在连接")
        case .transferring:
            state = .transferring; text = localized("Transferring", "正在传输")
        case .verifying:
            state = .verifying; text = localized("Verifying received content", "正在校验接收内容")
        case .saving:
            state = .saving; text = localized("Saving to the selected location", "正在保存到所选位置")
        case .waitingForReceiverSave:
            state = .waitingForReceiverSave; text = localized("Waiting for receiver to finish saving", "等待接收方完成保存")
        case .finalizingDelivery:
            state = .finalizingDelivery; text = localized("Saved; finalizing delivery", "已保存，正在完成交付确认")
        case .delivered:
            state = .delivered; text = localized("Delivered", "已送达")
        }
        if TransferPresentationPolicy.progress(for: state) == .complete {
            observedTransferred = max(max(observedTransferred, observedTotal), total)
            observedTotal = max(max(observedTotal, total), observedTransferred)
            publishObservedProgress()
            bytesPerSec = 0
        }
        statusText = text
        updateActivity(state: state, diagnostic: text)
    }

    fileprivate func handleProgress(_ bytes: UInt64, _ totalBytes: UInt64) {
        observedTransferred = max(observedTransferred, bytes)
        observedTotal = max(max(observedTotal, totalBytes), observedTransferred)
        let now = Date()
        let complete = observedTotal > 0 && observedTransferred >= observedTotal
        guard complete || now.timeIntervalSince(lastProgressPublishAt) >= 0.2 else { return }
        publishObservedProgress(now: now)
    }

    fileprivate func handleCompleted(_ bytes: UInt64) {
        transferred = max(transferred, bytes)
        total = max(total, bytes)
        observedTransferred = transferred
        observedTotal = total
        bytesPerSec = 0
        statusText = localized("Delivered", "已送达")
        updateActivity(state: .delivered, diagnostic: statusText)
        if let direction = transferActivity?.direction {
            clearStoredManifestSession(direction: direction)
        }
        #if os(iOS)
        (activeSend?.sourceAccess as? ShareDraftLease)?.acknowledge()
        #endif
        if transferActivity?.direction == .send {
            activeSend = nil
        } else {
            activeReceive = nil
        }
        resourceAccess = nil
    }

    fileprivate func handleTransferFailed(_ value: FfiTransferFailure) {
        if pausedByUser { return }
        publishObservedProgress()
        pendingReceive = nil
        requiresExceptionalTransferApproval = false
        failure = value
        statusText = friendlyFailure(value, language: displayLanguage)
        if !value.diagnosticMessage.isEmpty {
            eventLog.append("failure: \(value.diagnosticMessage)")
            if eventLog.count > 240 { eventLog.removeFirst(eventLog.count - 240) }
        }
        transferActivity?.failure = value
        if value.code == .userCanceled || value.code == .senderCanceled {
            updateActivity(state: .canceled, diagnostic: statusText)
        } else {
            updateActivity(state: .failed, diagnostic: statusText)
        }
        if !value.retryable {
            if transferActivity?.direction == .send {
                activeSend = nil
            } else {
                activeReceive = nil
            }
        }
        resourceAccess = nil
    }

    fileprivate func handleDiagnostic(_ message: String) {
        let projectedMessage: String
        if message.hasPrefix("connected via ") || message.hasPrefix("path changed:") {
            projectedMessage = connectionPath.map {
                "connection path=\($0.pathKind) event=\($0.eventKind)"
            } ?? "connection path updated"
        } else {
            projectedMessage = message
        }
        eventLog.append(projectedMessage)
        if eventLog.count > 240 { eventLog.removeFirst(eventLog.count - 240) }
    }

    private func startSend(
        selectedPaths: [String],
        settings: EnvoixRuntimeSettings,
        request: FfiTransferRequest,
        sourceAccess: AnyObject?,
        roomCode: String? = nil,
        rememberPersistence: RememberPersistenceContext? = nil
    ) {
        let paths = normalizedPaths(selectedPaths)
        guard !paths.isEmpty else {
            handleFailed(localized("Choose at least one file or folder", "请至少选择一个文件或文件夹"))
            return
        }
        displayLanguage = settings.language
        beginActivity(direction: .send, mode: request.mode, roomCode: roomCode)
        if preparedSelection?.sourcePaths != paths {
            prepareManifestSelection(selectedPaths: paths, sourceAccess: sourceAccess)
        }
        guard let preparedSelection else {
            task = Task { @MainActor [weak self] in
                guard let self else { return }
                while isPreparingManifest {
                    try? await Task.sleep(nanoseconds: 50_000_000)
                    if Task.isCancelled { return }
                }
                guard let preparedSelection, isManifestSelectionReady else { return }
                launchSend(SendOperation(
                    job: preparedSelection.job,
                    jobID: preparedSelection.jobID,
                    sourcePaths: preparedSelection.sourcePaths,
                    settings: settings,
                    request: request,
                    stateDirectory: preparedSelection.sessionStateDirectory,
                    sourceAccess: sourceAccess ?? preparedSelection.sourceAccess,
                    rememberPersistence: rememberPersistence
                ))
            }
            return
        }
        guard isManifestSelectionReady else {
            statusText = localized("Resolve source warnings before sending", "请先处理来源警告")
            return
        }
        launchSend(SendOperation(
            job: preparedSelection.job,
            jobID: preparedSelection.jobID,
            sourcePaths: preparedSelection.sourcePaths,
            settings: settings,
            request: request,
            stateDirectory: preparedSelection.sessionStateDirectory,
            sourceAccess: sourceAccess ?? preparedSelection.sourceAccess,
            rememberPersistence: rememberPersistence
        ))
    }

    private func launchSend(_ operation: SendOperation) {
        activeSend = operation
        activeReceive = nil
        resourceAccess = operation.sourceAccess
        let token = FfiManifestV2Cancellation()
        cancellation = token
        pausedByUser = false
        if operation.request.mode == .room {
            updateActivity(state: .waitingForPeer, diagnostic: localized("Waiting for peer", "正在等待对端"))
        } else {
            updateActivity(state: .connecting, diagnostic: localized("Connecting", "正在连接"))
        }
        let observer = Observer(
            viewModel: self,
            operationID: operationID,
            rememberPersistence: operation.rememberPersistence
        )
        task = Task { @MainActor [weak self] in
            guard let self else { return }
            do {
                #if os(iOS)
                if let lease = operation.sourceAccess as? ShareDraftLease,
                   let activityID = transferActivity?.activityId {
                    try lease.bind(to: activityID)
                }
                #endif
                try persistActiveSend(operation)
                let snapshot = await operation.job.snapshot()
                if snapshot.state != .sealed {
                    _ = try await operation.job.sealForSend()
                    preparedSelection = nil
                    preparedManifestSourcePaths = []
                    pendingSourceSelections = []
                    isManifestSelectionReady = false
                }
                _ = try await sendTransferJobV2(
                    job: operation.job,
                    settings: operation.settings,
                    request: operation.request,
                    stateDirectory: operation.stateDirectory,
                    cancellation: token,
                    observer: observer
                )
            } catch {
                if !pausedByUser { handleFailed(error.localizedDescription) }
            }
        }
    }

    private func startReceive(
        targetDirectory: String,
        settings: EnvoixRuntimeSettings,
        request: FfiTransferRequest,
        destinationAccess: AnyObject?,
        roomCode: String? = nil,
        rememberPersistence: RememberPersistenceContext? = nil
    ) {
        displayLanguage = settings.language
        beginActivity(direction: .receive, mode: request.mode, roomCode: roomCode)
        do {
            let operation = ReceiveOperation(
                settings: settings,
                request: request,
                stateDirectory: try receiveStateDirectory(activityID: transferActivity!.activityId),
                targetDirectory: targetDirectory,
                destinationAccess: destinationAccess,
                rememberPersistence: rememberPersistence
            )
            activeReceive = operation
            activeSend = nil
            launchReceive(operation)
        } catch {
            handleFailed(error.localizedDescription)
        }
    }

    private func launchReceive(
        _ operation: ReceiveOperation,
        launchSignal: ReceiveLaunchSignal? = nil
    ) {
        resourceAccess = operation.destinationAccess
        let token = FfiManifestV2Cancellation()
        cancellation = token
        pausedByUser = false
        updateActivity(state: .waitingForPeer, diagnostic: localized("Waiting for sender", "等待发送方"))
        let observer = Observer(
            viewModel: self,
            operationID: operationID,
            rememberPersistence: operation.rememberPersistence
        )
        let expectedOperationID = operationID
        let activityID = transferActivity?.activityId
        task = Task { @MainActor [weak self] in
            guard let self else {
                launchSignal?.resolve(nil)
                return
            }
            if launchSignal != nil,
               expectedOperationID != operationID || transferActivity?.activityId != activityID {
                launchSignal?.resolve(nil)
                return
            }
            do {
                try persistActiveReceive(operation)
                launchSignal?.resolve(activityID)
                let pending = try await receiveTransferOfferV2(
                    settings: operation.settings,
                    request: operation.request,
                    stateDirectory: operation.stateDirectory,
                    cancellation: token,
                    observer: observer
                )
                pendingReceive = pending
                let summary = pending.summary()
                pendingOfferSummary = summary
                pendingOfferEntries = pending.listEntries(offset: 0, limit: 512).entries
                total = summary.totalPlaintextBytes
                transferActivity?.itemCount = summary.fileCount + summary.directoryCount
                transferActivity?.totalBytes = total
                let available = try allocatableBytes(at: operation.targetDirectory)
                let exceptional = summary.exceptionalOffer || total > available / 2
                if exceptional {
                    requiresExceptionalTransferApproval = true
                    statusText = localized(
                        "Review this unusually large transfer before receiving",
                        "请先确认这个异常大的传输"
                    )
                    updateActivity(state: .awaitingDecision, diagnostic: statusText)
                } else {
                    continueReceive(pending, operation: operation, exceptionalApproved: false)
                }
            } catch {
                launchSignal?.resolve(nil)
                if !pausedByUser { handleFailed(error.localizedDescription) }
            }
        }
    }

    private func continueReceive(
        _ pending: FfiPendingManifestV2Receive,
        operation: ReceiveOperation,
        exceptionalApproved: Bool
    ) {
        let observer = Observer(viewModel: self, operationID: operationID)
        task = Task { @MainActor [weak self] in
            guard let self else { return }
            do {
                let available = try allocatableBytes(at: operation.targetDirectory)
                let decision = destinationDecision
                let copyDirectory: String?
                let copyAvailable: UInt64?
                if decision == .copyAfterVerify {
                    let directory = URL(fileURLWithPath: operation.targetDirectory, isDirectory: true)
                        .appendingPathComponent(".envoix-copy-staging-v2", isDirectory: true)
                    try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
                    copyDirectory = directory.path
                    copyAvailable = try allocatableBytes(at: directory.path)
                } else {
                    copyDirectory = nil
                    copyAvailable = nil
                }
                let completion = try await pending.receive(
                    destination: FfiDestinationRequestV2(
                        targetDirectory: operation.targetDirectory,
                        copyStagingDirectory: copyDirectory,
                        decision: decision,
                        targetAllocatableBytes: available,
                        stagingAllocatableBytes: copyAvailable,
                        stableObjectIdentity: true,
                        exceptionalTransferApproved: exceptionalApproved
                    ),
                    observer: observer
                )
                completedItemURLs = completion.savedPaths.map { URL(fileURLWithPath: $0) }
                completedFileURL = completedItemURLs.count == 1 ? completedItemURLs[0] : nil
                transferActivity?.savedPaths = completion.savedPaths
                pendingReceive = nil
                pendingOfferSummary = nil
                pendingOfferEntries = []
                requiresExceptionalTransferApproval = false
            } catch {
                if !pausedByUser { handleFailed(error.localizedDescription) }
            }
        }
    }

    private func resolveSource(rootItemID: UInt64, decision: FfiSourceDecisionV2, path: String?) {
        guard var selection = preparedSelection else { return }
        isPreparingManifest = true
        preparationTask = Task { @MainActor [weak self] in
            guard let self else { return }
            do {
                let previousPath = await selection.job.sourcePathForPreview(itemId: rootItemID)
                let snapshot = try await selection.job.resolveSourceIssue(
                    rootItemId: rootItemID,
                    decision: decision,
                    reauthorizedPath: path
                )
                if decision == .removeSelection {
                    selection.sourcePaths.removeAll { $0 == previousPath }
                } else if decision == .reauthorize,
                          let previousPath,
                          let path,
                          let index = selection.sourcePaths.firstIndex(of: previousPath) {
                    selection.sourcePaths[index] = path
                }
                preparedSelection = selection
                applyPreparation(
                    snapshot,
                    paths: await projectedSourcePaths(job: selection.job, snapshot: snapshot),
                    roots: await selection.job.listRoots()
                )
            } catch {
                handleFailed(error.localizedDescription)
            }
            isPreparingManifest = false
        }
    }

    private func applyPreparation(
        _ snapshot: FfiTransferJobSnapshotV2,
        paths: [String],
        roots: [FfiInventoryItemV2]
    ) {
        preparedManifestSourcePaths = paths
        preparedInventorySummary = snapshot.inventory
        preparedInventoryRoots = roots
        pendingSourceSelections = snapshot.selections.filter { $0.state == .needsDecision }
        isManifestSelectionReady = snapshot.state == .readyToSend
        statusText = isManifestSelectionReady
            ? localized("Ready to send", "已准备发送")
            : localized("Some items need your decision", "部分项目需要你的决定")
        if transferActivity == nil {
            presentationState = nil
        }
    }

    private func projectedSourcePaths(
        job: FfiTransferJobV2,
        snapshot: FfiTransferJobSnapshotV2
    ) async -> [String] {
        var paths: [String] = []
        for selection in snapshot.selections {
            if let path = await job.sourcePathForPreview(itemId: selection.rootItemId) {
                paths.append(path)
            }
        }
        return paths
    }

    private func beginActivity(
        direction: FfiTransferDirection,
        mode: FfiTransferMode,
        roomCode: String?
    ) {
        clearStoredManifestSession(direction: direction)
        task?.cancel()
        cancellation?.cancel()
        operationID = UUID()
        let activityID = UUID().uuidString
        transferActivity = TransferActivityRecord(
            activityId: activityID,
            direction: direction,
            mode: mode,
            itemCount: 0,
            totalBytes: 0,
            bytesTransferred: 0,
            state: direction == .send ? .preparing : .waitingForPeer,
            diagnosticMessage: "",
            failure: nil,
            savedPaths: [],
            roomID: roomCode.flatMap(RemoteLogUpload.roomID),
            connectionPath: nil,
            updatedAt: Date()
        )
        invite = ""
        peerAddress = ""
        transferred = 0
        total = 0
        observedTransferred = 0
        observedTotal = 0
        lastProgressPublishAt = .distantPast
        failure = nil
        completedFileURL = nil
        completedItemURLs = []
        pendingOfferSummary = nil
        pendingOfferEntries = []
        connectionPath = nil
        requiresExceptionalTransferApproval = false
        rate = RateTracker()
        eventLog = []
        presentationState = transferActivity?.state
        if let transferActivity { appModel?.upsert(transferActivity) }
    }

    private func updateActivity(state: TransferActivityState, diagnostic: String) {
        if let current = presentationState,
           TransferPresentationPolicy.isTerminal(current),
           current != state {
            return
        }
        presentationState = state
        guard var record = transferActivity else { return }
        record.state = state
        record.totalBytes = total
        record.bytesTransferred = transferred
        record.diagnosticMessage = diagnostic
        record.updatedAt = Date()
        transferActivity = record
        appModel?.upsert(record)
    }

    private func publishObservedProgress(now: Date = Date()) {
        transferred = max(transferred, observedTransferred)
        total = max(max(total, observedTotal), transferred)
        bytesPerSec = rate.update(bytes: transferred, now: now)
        lastProgressPublishAt = now
        guard var record = transferActivity else { return }
        record.bytesTransferred = transferred
        record.totalBytes = total
        record.updatedAt = now
        transferActivity = record
        appModel?.upsert(record)
    }

    private func persistActiveSend(_ operation: SendOperation) throws {
        if usesProcessOnlyAuthentication(operation.request.mode) {
            clearStoredManifestSession(direction: .send)
            return
        }
        guard let activity = transferActivity else {
            throw RuntimeSettingsError("Cannot persist a sender session without an activity.")
        }
        let shareDraftID: UUID?
        #if os(iOS)
        shareDraftID = (operation.sourceAccess as? ShareDraftLease)?.id
        #else
        shareDraftID = nil
        #endif
        let bookmarks: [Data]
        if shareDraftID == nil {
            bookmarks = try operation.sourcePaths.map {
                try makeSecurityScopedFolderBookmark(for: URL(fileURLWithPath: $0))
            }
        } else {
            bookmarks = []
        }
        try storeManifestSession(StoredAppleManifestSessionV2(
            schemaVersion: StoredAppleManifestSessionV2.schemaVersion,
            activityID: activity.activityId,
            jobID: operation.jobID,
            stateDirectory: operation.stateDirectory,
            targetDirectory: nil,
            sourcePaths: operation.sourcePaths,
            sourceBookmarks: bookmarks,
            destinationBookmark: nil,
            shareDraftID: shareDraftID,
            settings: StoredRuntimeSettingsV2(operation.settings),
            request: StoredTransferRequestV2(operation.request),
            itemCount: activity.itemCount,
            totalBytes: activity.totalBytes,
            roomID: activity.roomID
        ), direction: .send)
    }

    private func persistActiveReceive(_ operation: ReceiveOperation) throws {
        if usesProcessOnlyAuthentication(operation.request.mode) {
            clearStoredManifestSession(direction: .receive)
            return
        }
        guard let activity = transferActivity else {
            throw RuntimeSettingsError("Cannot persist a receiver session without an activity.")
        }
        let destinationURL = URL(fileURLWithPath: operation.targetDirectory, isDirectory: true)
        try storeManifestSession(StoredAppleManifestSessionV2(
            schemaVersion: StoredAppleManifestSessionV2.schemaVersion,
            activityID: activity.activityId,
            jobID: nil,
            stateDirectory: operation.stateDirectory,
            targetDirectory: operation.targetDirectory,
            sourcePaths: [],
            sourceBookmarks: [],
            destinationBookmark: try makeSecurityScopedFolderBookmark(for: destinationURL),
            shareDraftID: nil,
            settings: StoredRuntimeSettingsV2(operation.settings),
            request: StoredTransferRequestV2(operation.request),
            itemCount: activity.itemCount,
            totalBytes: activity.totalBytes,
            roomID: activity.roomID
        ), direction: .receive)
    }

    private func storeManifestSession(
        _ stored: StoredAppleManifestSessionV2,
        direction: FfiTransferDirection
    ) throws {
        let data = try JSONEncoder().encode(stored)
        try data.write(
            to: manifestSessionURL(direction: direction),
            options: [.atomic, .completeFileProtectionUnlessOpen]
        )
    }

    private func storedManifestSession(
        direction: FfiTransferDirection
    ) throws -> StoredAppleManifestSessionV2? {
        let url = try manifestSessionURL(direction: direction)
        guard FileManager.default.fileExists(atPath: url.path) else { return nil }
        let data = try Data(contentsOf: url)
        let stored = try JSONDecoder().decode(StoredAppleManifestSessionV2.self, from: data)
        guard stored.schemaVersion == StoredAppleManifestSessionV2.schemaVersion else {
            throw RuntimeSettingsError("Stored transfer session schema is unsupported.")
        }
        return stored
    }

    private func clearStoredManifestSession(direction: FfiTransferDirection) {
        guard let url = try? manifestSessionURL(direction: direction),
              FileManager.default.fileExists(atPath: url.path) else { return }
        try? FileManager.default.removeItem(at: url)
    }

    private func manifestSessionURL(direction: FfiTransferDirection) throws -> URL {
        let fileName = direction == .send
            ? Self.activeSendSessionFileName
            : Self.activeReceiveSessionFileName
        return try manifestRootDirectory().appendingPathComponent(fileName, isDirectory: false)
    }

    private func usesProcessOnlyAuthentication(_ mode: FfiTransferMode) -> Bool {
        // Transfer authentication material is intentionally never serialized.
        // Every current route therefore needs a fresh pairing after relaunch.
        switch mode {
        case .manual, .invite, .remembered, .showManual, .showInvite, .mdns, .room:
            return true
        }
    }

    private func restoreSourceAccess(_ stored: StoredAppleManifestSessionV2) throws -> AnyObject? {
        #if os(iOS)
        if let shareDraftID = stored.shareDraftID {
            let store = try ShareDraftStore.live()
            let draft = try store.load(id: shareDraftID)
            let paths = draft.fileURLs.map { $0.standardizedFileURL.path }
            guard paths == normalizedPaths(stored.sourcePaths) else {
                throw RuntimeSettingsError("The saved Share draft no longer matches the sealed transfer job.")
            }
            return ShareDraftLease(id: shareDraftID, store: store)
        }
        #else
        guard stored.shareDraftID == nil else {
            throw RuntimeSettingsError("A Share draft cannot be restored on this platform.")
        }
        #endif

        guard !stored.sourceBookmarks.isEmpty else {
            guard stored.sourcePaths.allSatisfy({ FileManager.default.isReadableFile(atPath: $0) }) else {
                throw RuntimeSettingsError("Source permission expired. Choose the source again.")
            }
            return nil
        }
        guard stored.sourceBookmarks.count == stored.sourcePaths.count else {
            throw RuntimeSettingsError("Stored source permissions are incomplete.")
        }
        var accesses: [SecurityScopedResourceAccess] = []
        for (bookmark, expectedPath) in zip(stored.sourceBookmarks, stored.sourcePaths) {
            let url = try resolveSecurityScopedFolderBookmark(bookmark)
            guard url.standardizedFileURL.path == URL(fileURLWithPath: expectedPath).standardizedFileURL.path else {
                throw RuntimeSettingsError("A sealed source moved. Start a new transfer for its new location.")
            }
            let access = SecurityScopedResourceAccess(url: url)
            guard access.isActive || FileManager.default.isReadableFile(atPath: url.path) else {
                throw RuntimeSettingsError("Source permission expired. Choose the source again.")
            }
            accesses.append(access)
        }
        return NSArray(array: accesses)
    }

    private func restoreDestinationAccess(
        bookmark: Data?,
        fallbackPath: String
    ) throws -> (path: String, access: AnyObject?) {
        guard let bookmark else {
            guard FileManager.default.isWritableFile(atPath: fallbackPath) else {
                throw RuntimeSettingsError("Destination permission expired. Choose the destination again.")
            }
            return (fallbackPath, nil)
        }
        let url = try resolveSecurityScopedFolderBookmark(bookmark)
        let access = SecurityScopedResourceAccess(url: url)
        guard access.isActive || FileManager.default.isWritableFile(atPath: url.path) else {
            throw RuntimeSettingsError("Destination permission expired. Choose the destination again.")
        }
        return (url.path, access)
    }

    private func request(
        direction: FfiTransferDirection,
        mode: FfiTransferMode,
        settings: EnvoixRuntimeSettings,
        invite: String = "",
        code: String = "",
        token: String = "",
        rememberConsent: Bool = false,
        rememberedCredentialRef: String = "",
        rememberedGeneration: UInt64 = 0,
        rememberedPreviousGeneration: UInt64? = nil,
        pathPolicy: FfiPathPolicy = .auto
    ) -> FfiTransferRequest {
        let effectiveToken = direction == .receive
            && token.isEmpty
            && (mode == .showInvite || mode == .mdns)
            ? UUID().uuidString
            : token
        return FfiTransferRequest(
            direction: direction,
            mode: mode,
            peerDescriptor: "",
            invite: invite,
            code: code,
            token: effectiveToken,
            rememberConsent: rememberConsent,
            rememberedCredentialRef: rememberedCredentialRef,
            rememberedGeneration: rememberedGeneration,
            rememberedPreviousGeneration: rememberedPreviousGeneration,
            broker: settings.serverUrl,
            relay: settings.relayUrl,
            configPath: settings.configPath,
            pathPolicy: pathPolicy,
            rendezvous: rendezvousPlan(for: mode)
        )
    }

    private func prepareRememberPersistence(
        label: String?,
        settings: EnvoixRuntimeSettings
    ) -> RememberPersistenceContext? {
        guard let label, !label.trimmed.isEmpty else { return nil }
        do {
            return try RememberPersistenceContext(
                pending: RememberedPeerStore.shared.prepare(
                    label: label,
                    broker: settings.serverUrl,
                    relay: settings.relayUrl
                )
            )
        } catch {
            handleFailed(error.localizedDescription)
            return nil
        }
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
        case .remembered:
            return FfiRendezvousPlan(useRoom: true, useMdns: false, internetAvailable: true)
        default:
            return FfiRendezvousPlan(useRoom: false, useMdns: false, internetAvailable: true)
        }
    }

    private var compressionPolicy: FfiCompressionPolicyV2 {
        switch UserDefaults.standard.string(forKey: "envoix.compressionPolicy") {
        case "never": return .never
        case "always": return .always
        default: return .smart
        }
    }

    private var destinationDecision: FfiDestinationDecisionV2 {
        UserDefaults.standard.string(forKey: "envoix.destinationSaveMode") == "copy"
            ? .copyAfterVerify
            : .saveDirectly
    }

    private func jobStoreDirectory() throws -> String {
        let directory = try manifestRootDirectory().appendingPathComponent("jobs", isDirectory: true)
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        return directory.path
    }

    private func sessionStateDirectory(jobID: String) throws -> String {
        let directory = try manifestRootDirectory()
            .appendingPathComponent("sessions", isDirectory: true)
            .appendingPathComponent(jobID, isDirectory: true)
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        return directory.path
    }

    private func receiveStateDirectory(activityID: String) throws -> String {
        let directory = try manifestRootDirectory()
            .appendingPathComponent("receives", isDirectory: true)
            .appendingPathComponent(activityID, isDirectory: true)
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        return directory.path
    }

    private func manifestRootDirectory() throws -> URL {
        guard let support = FileManager.default.urls(
            for: .applicationSupportDirectory,
            in: .userDomainMask
        ).first else { throw RuntimeSettingsError("Application Support is unavailable.") }
        let directory = support.appendingPathComponent("envoix/manifest-v2", isDirectory: true)
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        return directory
    }

    private func allocatableBytes(at path: String) throws -> UInt64 {
        let values = try URL(fileURLWithPath: path, isDirectory: true).resourceValues(
            forKeys: [.volumeAvailableCapacityForImportantUsageKey, .volumeAvailableCapacityKey]
        )
        if let bytes = values.volumeAvailableCapacityForImportantUsage, bytes >= 0 {
            return UInt64(bytes)
        }
        if let bytes = values.volumeAvailableCapacity, bytes >= 0 { return UInt64(bytes) }
        throw RuntimeSettingsError("The selected destination did not report available capacity.")
    }

    private func normalizedPaths(_ paths: [String]) -> [String] {
        var seen = Set<String>()
        return paths.compactMap {
            let path = URL(fileURLWithPath: $0).standardizedFileURL.path
            return !path.isEmpty && seen.insert(path).inserted ? path : nil
        }
    }

    private func localized(_ english: String, _ chinese: String) -> String {
        AppText.value(english, chinese, language: displayLanguage)
    }
}

final class Observer: TransferObserver, @unchecked Sendable {
    private weak var viewModel: TransferViewModel?
    private let operationID: UUID
    private let rememberPersistence: RememberPersistenceContext?

    init(
        viewModel: TransferViewModel,
        operationID: UUID,
        rememberPersistence: RememberPersistenceContext? = nil
    ) {
        self.viewModel = viewModel
        self.operationID = operationID
        self.rememberPersistence = rememberPersistence
    }

    func onInviteReady(invite: String) { hop { $0.handleInvite(invite) } }
    func onStarted(itemCount: UInt32, totalBytes: UInt64) {
        hop { $0.handleStarted(itemCount: itemCount, totalBytes: totalBytes) }
    }
    func onPhase(phase: FfiManifestV2Phase) { hop { $0.handlePhase(phase) } }
    func onProgress(transferred: UInt64, total: UInt64) {
        hop { $0.handleProgress(transferred, total) }
    }
    func onCompleted(bytes: UInt64) { hop { $0.handleCompleted(bytes) } }
    func onTransferFailed(failure: FfiTransferFailure) { hop { $0.handleTransferFailed(failure) } }
    func onConnectionPath(event: FfiConnectionPathEvent) { hop { $0.handleConnectionPath(event) } }
    func onDiagnostic(message: String) { hop { $0.handleDiagnostic(message) } }
    func onRememberedCredential(opaqueCredential: Data, generation: UInt64) -> Bool {
        rememberPersistence?.persist(opaqueCredential, generation: generation) ?? false
    }

    private func hop(_ body: @escaping @MainActor (TransferViewModel) -> Void) {
        Task { @MainActor [weak viewModel, operationID] in
            guard let viewModel, viewModel.operationID == operationID else { return }
            body(viewModel)
        }
    }
}

func estimatedRemainingSeconds(
    total: UInt64,
    transferred: UInt64,
    bytesPerSecond: Double,
    isStable: Bool
) -> Double? {
    guard isStable, total > transferred, bytesPerSecond > 0 else { return nil }
    return Double(total - transferred) / bytesPerSecond
}

struct RateTracker {
    private var lastDate: Date?
    private var lastBytes: UInt64 = 0
    private(set) var samples = 0
    private var smoothed = 0.0

    var isStable: Bool { samples >= 2 }

    mutating func update(bytes: UInt64, now: Date = Date()) -> Double {
        defer {
            lastDate = now
            lastBytes = bytes
        }
        guard let lastDate, bytes >= lastBytes else { return smoothed }
        let elapsed = now.timeIntervalSince(lastDate)
        guard elapsed > 0.1 else { return smoothed }
        let instantaneous = Double(bytes - lastBytes) / elapsed
        smoothed = samples == 0 ? instantaneous : smoothed * 0.7 + instantaneous * 0.3
        samples += 1
        return smoothed
    }
}

func friendlyError(_ reason: String, language: String = "en") -> String {
    AppText.value("Transfer failed: \(reason)", "传输失败：\(reason)", language: language)
}

func friendlyFailure(_ failure: FfiTransferFailure, language: String = "en") -> String {
    friendlyFailure(code: failure.code, diagnosticMessage: failure.diagnosticMessage, language: language)
}

func friendlyFailure(
    code: FfiFailureCode,
    diagnosticMessage: String,
    language: String = "en"
) -> String {
    switch code {
    case .userCanceled, .senderCanceled:
        return AppText.value("Transfer canceled.", "传输已取消。", language: language)
    case .networkLost:
        return AppText.value("Connection lost. Resume to continue.", "连接已断开，可恢复继续。", language: language)
    case .authenticationFailed:
        return AppText.value("Pairing authentication failed.", "配对认证失败。", language: language)
    case .roomNotFound:
        return AppText.value("The Room is not available yet. Ask the creator to keep it open and retry.", "房间尚不可用。请让创建者保持房间开启后重试。", language: language)
    case .roomExpired:
        return AppText.value("This Room expired. Create a new Room Code.", "此房间已过期。请创建新的房间码。", language: language)
    case .roomFull:
        return AppText.value("This Room is already in use. Retry shortly.", "此房间正在使用中。请稍后重试。", language: language)
    case .roomRateLimited, .endpointRateLimited, .ipRateLimited:
        return AppText.value("Too many Room attempts. Wait before retrying.", "房间尝试次数过多。请稍后再试。", language: language)
    case .roomUnderAttack:
        return AppText.value("This Room was closed for security. Create a new Room Code.", "此房间因安全原因已关闭。请创建新的房间码。", language: language)
    case .serverBusy:
        return AppText.value("The Room service is busy. Retry shortly.", "房间服务繁忙。请稍后重试。", language: language)
    case .malformedJoin, .unsupportedRendezvousVersion:
        return AppText.value("Update Envoix before joining this Room.", "请更新 Envoix 后再加入此房间。", language: language)
    case .senderPermissionLost:
        return AppText.value("Source permission expired. Choose the source again.", "来源权限已失效，请重新选择。", language: language)
    case .senderSourceUnavailable, .senderItemRemoved:
        return AppText.value("A selected source is unavailable.", "所选来源不可用。", language: language)
    case .senderSourceChanged, .protocolOrIntegrityFailure:
        return AppText.value("Content verification failed.", "内容校验失败。", language: language)
    case .receiverSpaceInsufficient:
        return AppText.value("The destination does not have enough space.", "目标位置空间不足。", language: language)
    case .receiverDestinationDecisionRequired, .receiverDestinationUnavailable:
        return AppText.value("Choose an available destination.", "请选择可用的目标位置。", language: language)
    case .receiverSaveFailed:
        return AppText.value("The receiver could not finish saving. Resume to reconcile it.", "接收端未能完成保存，请恢复以进行确认。", language: language)
    case .receiverReusedObjectLost:
        return AppText.value(
            "An existing destination item selected for reuse changed or disappeared. Restore it and resume, or start a new transfer.",
            "接收端原定复用的已有项目已更改或消失。请恢复该项目后继续，或重新发起传输。",
            language: language
        )
    case .receiverFinalizationOutcomeUnknown:
        return AppText.value(
            "The receiver cannot yet confirm the final save after an interruption. Resume to reconcile the destination.",
            "中断后接收端暂时无法确认最终保存结果，请恢复传输以核对目标位置。",
            language: language
        )
    case .unsupportedFeature:
        return AppText.value("This transfer request is not supported.", "不支持此传输请求。", language: language)
    case .internalError:
        return AppText.value("The transfer failed.", "传输失败。", language: language)
    }
}
