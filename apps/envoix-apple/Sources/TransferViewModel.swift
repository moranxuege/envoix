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
    var etaSeconds: Double?
    var avgBps: Double = 0
    var peakBps: Double = 0
    var speedHistory: [Double] = []
    var log: [String] = []

    fileprivate var lastLogKey: String = ""
}

func mergedActivityDiagnosticLog(
    activityTimeline: [String],
    observerLog: [String]
) -> [String] {
    activityTimeline + observerLog
}

enum ActivityProjectionPolicy {
    static func pendingCount(_ records: [FfiTransferActivityRecord]) -> Int {
        records.lazy.filter { isPending($0.state) }.count
    }

    static func isPending(_ state: FfiTransferActivityState) -> Bool {
        switch state {
        case .queued, .binding, .waitingForPeer, .pairing, .connecting, .transferring,
                .verifying, .publishing, .unconfirmed, .paused:
            return true
        case .completed, .failed, .canceled, .unknown:
            return false
        }
    }

    static func shouldAccept(
        _ incoming: FfiTransferActivityRecord,
        replacing current: FfiTransferActivityRecord
    ) -> Bool {
        guard incoming.activityId == current.activityId else { return false }
        if incoming.sequence != current.sequence {
            return incoming.sequence > current.sequence
        }
        return incoming.updatedAtMs >= current.updatedAtMs
    }

    static func pruneTerminalHistory(
        _ records: [FfiTransferActivityRecord],
        limit: Int
    ) -> [FfiTransferActivityRecord] {
        let sorted = records.sorted { lhs, rhs in
            if lhs.updatedAtMs != rhs.updatedAtMs {
                return lhs.updatedAtMs > rhs.updatedAtMs
            }
            return lhs.activityId < rhs.activityId
        }
        let nonTerminal = sorted.filter { !isTerminal($0.state) }
        let terminalLimit = max(0, limit - nonTerminal.count)
        let retainedTerminalIDs = Set(
            sorted.lazy
                .filter { isTerminal($0.state) }
                .prefix(terminalLimit)
                .map(\.activityId)
        )
        return sorted.filter { !isTerminal($0.state) || retainedTerminalIDs.contains($0.activityId) }
    }

    static func isTerminal(_ state: FfiTransferActivityState) -> Bool {
        switch state {
        case .completed, .failed, .canceled: return true
        case .queued, .binding, .waitingForPeer, .pairing, .connecting, .transferring,
                .verifying, .publishing, .unconfirmed, .paused, .unknown:
            return false
        }
    }
}

enum ActivityExecutionPolicy {
    static func occupiesExecutionSlot(_ state: FfiTransferActivityState) -> Bool {
        switch state {
        case .paused, .completed, .failed, .canceled:
            return false
        case .queued, .binding, .waitingForPeer, .pairing, .connecting, .transferring,
                .verifying, .publishing, .unconfirmed, .unknown:
            return true
        }
    }

    static func canResume(
        _ record: FfiTransferActivityRecord,
        among records: [FfiTransferActivityRecord]
    ) -> Bool {
        let limit = max(1, Int(record.limits.maxParallelTransfers))
        let occupied = records.lazy.filter {
            $0.activityId != record.activityId && occupiesExecutionSlot($0.state)
        }.count
        return occupied < limit
    }
}

private struct ReceivePublication {
    var destinationDirectory: URL
    var resourceAccess: AnyObject?
    var completedRecord: FfiTransferActivityRecord?
    var isPublishing = false
}

final class AppModel: ObservableObject {
    static let shared = AppModel()
    private static let commandSnapshotRefreshDelays: [TimeInterval] = [0.2, 1.0]
    private static let activityRemovalPollInterval: TimeInterval = 0.1
    private static let activityRemovalTimeout: TimeInterval = 5
    #if DEBUG
    private static let hostedTestRecordsDirectoryEnvironmentKey = "ENVOIX_APPLE_HOSTED_TEST_RECORDS_DIR"
    private static let stalledActivityRemovalUITestArgument = "--ui-testing-stalled-activity-removal"
    private static let macOSHostedTestArgument = "--macos-hosted-testing"
    #endif

    let receive = TransferViewModel()
    let send = TransferViewModel()
    @Published private(set) var activities: [FfiTransferActivityRecord] = []
    @Published private(set) var pendingActivityRemovalIDs = Set<String>()
    @Published private(set) var manifestActivities: [String: FfiManifestActivityRecord] = [:]
    @Published private(set) var activityMetrics: [String: ActivityMetrics] = [:]
    @Published private(set) var transferCacheSummary = TransferCacheSummary()
    @Published private(set) var isCleaningTransferCache = false
    @Published private(set) var transferCacheError: String?
    #if os(iOS)
    @Published private(set) var pendingSendSelection: PendingSendSelection?
    #endif
    private var transferEventLinesByActivityID: [String: [String]] = [:]
    private var transferLogByActivityID: [String: [String]] = [:]
    private var activityResourceAccess: [String: AnyObject] = [:]
    #if os(macOS)
    private var appLifetimeDestinationAccess: [String: SecurityScopedResourceAccess] = [:]
    #endif
    private var receivePublications: [String: ReceivePublication] = [:]
    private var durableSessions: [String: DurableEnvoixSession] = [:]
    private var durableManifestSessions: [String: DurableEnvoixManifestSession] = [:]

    private var cancellables = Set<AnyCancellable>()
    private var removedActivityIDs = Set<String>()
    private let activityCap = 50
    private let recordsDirectory: URL = {
        #if DEBUG
        if let path = ProcessInfo.processInfo.environment[AppModel.hostedTestRecordsDirectoryEnvironmentKey],
           !path.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
            return URL(fileURLWithPath: path, isDirectory: true)
        }
        if ProcessInfo.processInfo.arguments.contains(AppModel.macOSHostedTestArgument) {
            return FileManager.default.temporaryDirectory
                .appendingPathComponent(
                    "envoix-macos-hosted-\(ProcessInfo.processInfo.processIdentifier)/records",
                    isDirectory: true
                )
        }
        #endif
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
                    if ActivityProjectionPolicy.isTerminal(record.state), let vm {
                        self?.snapshotDiagnostics(from: vm, activityID: record.activityId)
                    }
                }
                .store(in: &cancellables)
        }
        #if DEBUG
        if ProcessInfo.processInfo.arguments.contains("--ui-testing-received-folder-fixture") {
            let fixture = PreviewFixtures.completedFolderReceiveFixture
            activities = [fixture.activity]
            manifestActivities = [fixture.activity.activityId: fixture.manifest]
        } else if ProcessInfo.processInfo.arguments.contains("--ui-testing-activity-fixtures") {
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
        receive.isBusy || send.isBusy || activities.contains { !ActivityProjectionPolicy.isTerminal($0.state) }
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

        let values: URLResourceValues
        do {
            values = try url.resourceValues(
                forKeys: [.isRegularFileKey, .isDirectoryKey, .isSymbolicLinkKey]
            )
        } catch {
            throw OpenedSendFileError.inaccessible
        }
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
        guard pendingSendSelection?.id == id else { return }
        pendingSendSelection = nil
    }
    #endif

    @discardableResult
    func pauseActivity(_ activityID: String) -> Bool {
        if durableManifestSessions[activityID]?.pause() == true
            || durableSessions[activityID]?.pause() == true {
            syncActivitySnapshots()
            scheduleCommandSnapshotRefreshes()
            return true
        }
        return false
    }

    @discardableResult
    func resumeActivity(_ activityID: String) -> Bool {
        guard canResumeActivity(activityID) else { return false }
        if retryReceivePublication(activityID) {
            return true
        }
        if durableManifestSessions[activityID]?.resume() == true
            || durableSessions[activityID]?.resume() == true {
            syncActivitySnapshots()
            scheduleCommandSnapshotRefreshes()
            return true
        }
        return false
    }

    func canResumeActivity(_ activityID: String) -> Bool {
        guard let record = activities.first(where: { $0.activityId == activityID }) else {
            return false
        }
        return ActivityExecutionPolicy.canResume(record, among: activities)
    }

    var hasExecutingActivity: Bool {
        activities.contains { ActivityExecutionPolicy.occupiesExecutionSlot($0.state) }
    }

    @discardableResult
    func cancelActivity(_ activityID: String) -> Bool {
        if durableManifestSessions[activityID]?.cancel() == true
            || durableSessions[activityID]?.cancel() == true {
            syncActivitySnapshots()
            scheduleCommandSnapshotRefreshes()
            return true
        }
        #if DEBUG
        if ProcessInfo.processInfo.arguments.contains("--ui-testing-stalled-activity-command"),
           activities.contains(where: { $0.activityId == activityID }) {
            return true
        }
        #endif
        return false
    }

    @discardableResult
    func removeActivity(_ activityID: String) -> Bool {
        guard activities.contains(where: { $0.activityId == activityID }),
              !pendingActivityRemovalIDs.contains(activityID) else { return false }

        pendingActivityRemovalIDs.insert(activityID)
        removedActivityIDs.insert(activityID)
        guard enqueueActivityRemoval(activityID) else {
            pendingActivityRemovalIDs.remove(activityID)
            removedActivityIDs.remove(activityID)
            return false
        }

        verifyActivityRemoval(
            activityID,
            deadline: Date().addingTimeInterval(Self.activityRemovalTimeout)
        )
        return true
    }

    private func enqueueActivityRemoval(_ activityID: String) -> Bool {
        if durableManifestSessions[activityID]?.remove() == true
            || durableSessions[activityID]?.remove() == true {
            return true
        }

        durableManifestSessions.removeValue(forKey: activityID)
        durableSessions.removeValue(forKey: activityID)
        do {
            if try listDurableManifestRecords(recordsDir: recordsDirectory.path)
                .contains(where: { $0.activity.activityId == activityID }) {
                let observer = AppleManifestObserver(
                    viewModel: nil,
                    appModel: self,
                    activityID: activityID
                )
                let session = try restoreDurableManifestTransferV2(
                    activityId: activityID,
                    recordsDir: recordsDirectory.path,
                    observer: observer
                )
                durableManifestSessions[activityID] = session
                return session.remove()
            }

            if try listDurableTransferRecords(recordsDir: recordsDirectory.path)
                .contains(where: { $0.activityId == activityID }) {
                let observer = Observer(
                    nil,
                    appModel: self,
                    operationID: UUID(),
                    activityID: activityID
                )
                let session = try restoreDurableTransferV2(
                    activityId: activityID,
                    recordsDir: recordsDirectory.path,
                    observer: observer,
                    mailbox: mailboxObserver
                )
                durableSessions[activityID] = session
                return session.remove()
            }

            return true
        } catch {
            handleCoreStatus(
                "Activity removal could not restore durable state: \(error.localizedDescription)",
                activityID: activityID
            )
            return false
        }
    }

    private func verifyActivityRemoval(_ activityID: String, deadline: Date) {
        guard pendingActivityRemovalIDs.contains(activityID) else { return }
        let isPersisted = persistedActivityPresence(activityID)
        if isPersisted == false {
            finishActivityRemoval(activityID)
            return
        }
        if Date() < deadline {
            DispatchQueue.main.asyncAfter(deadline: .now() + Self.activityRemovalPollInterval) { [weak self] in
                self?.verifyActivityRemoval(activityID, deadline: deadline)
            }
            return
        }

        pendingActivityRemovalIDs.remove(activityID)
        removedActivityIDs.remove(activityID)
        durableManifestSessions.removeValue(forKey: activityID)
        durableSessions.removeValue(forKey: activityID)
        syncActivitySnapshots()
        Task { @MainActor in
            ToastCenter.shared.show(AppText.value(
                "Activity could not be removed. It was kept so you can try again.",
                "活动未能删除，已保留以便重试。",
                language: UserDefaults.standard.string(forKey: "envoix.language") ?? "en"
            ))
        }
    }

    private func persistedActivityPresence(_ activityID: String) -> Bool? {
        #if DEBUG
        if ProcessInfo.processInfo.arguments.contains(Self.stalledActivityRemovalUITestArgument) {
            return true
        }
        #endif
        do {
            if try listDurableManifestRecords(recordsDir: recordsDirectory.path)
                .contains(where: { $0.activity.activityId == activityID }) {
                return true
            }
            return try listDurableTransferRecords(recordsDir: recordsDirectory.path)
                .contains(where: { $0.activityId == activityID })
        } catch {
            handleCoreStatus(
                "Activity removal verification failed: \(error.localizedDescription)",
                activityID: activityID
            )
            return nil
        }
    }

    private func finishActivityRemoval(_ activityID: String) {
        discardActivityResourceAccess(for: activityID)
        pendingActivityRemovalIDs.remove(activityID)
        durableManifestSessions.removeValue(forKey: activityID)
        durableSessions.removeValue(forKey: activityID)
        manifestActivities.removeValue(forKey: activityID)
        let publication = receivePublications.removeValue(forKey: activityID)
        if let publication,
           let path = publication.completedRecord?.completedFilePath,
           !path.isEmpty {
            try? FileManager.default.removeItem(at: URL(fileURLWithPath: path))
        }
        if publication != nil {
            cleanupReceiveStaging(activityID: activityID)
            ReceivePublicationStore.remove(activityID: activityID)
        }
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
        let persistedManifests = (try? listDurableManifestRecords(recordsDir: recordsDirectory.path)) ?? []
        let liveManifests = durableManifestSessions.values.map { $0.activity() }
        let manifestRecords = Dictionary(
            (persistedManifests + liveManifests).map { ($0.activity.activityId, $0) },
            uniquingKeysWith: { _, latest in latest }
        )
        for record in manifestRecords.values where !removedActivityIDs.contains(record.activity.activityId) {
            handleManifestActivity(record)
        }
    }

    private func scheduleCommandSnapshotRefreshes() {
        for delay in Self.commandSnapshotRefreshDelays {
            DispatchQueue.main.asyncAfter(deadline: .now() + delay) { [weak self] in
                self?.syncActivitySnapshots()
            }
        }
    }

    func startDurableSession(
        settings: EnvoixRuntimeSettings,
        request: FfiTransferRequest,
        observer: TransferObserver
    ) throws -> DurableEnvoixSession {
        try FileManager.default.createDirectory(at: recordsDirectory, withIntermediateDirectories: true)
        let session = try startDurableTransferV2(
            settings: settings,
            request: request,
            recordsDir: recordsDirectory.path,
            receiptServer: currentReceiptServer(),
            observer: observer,
            mailbox: mailboxObserver
        )
        durableSessions[request.activityId] = session
        if let target = ReceivePublicationStore.loadAll()[request.activityId] {
            _ = session.setPublicationTarget(target: target.ffiTarget)
        }
        upsertActivity(session.activity(), speedBps: 0)
        return session
    }

    func startDurableManifestSendSession(
        settings: EnvoixRuntimeSettings,
        request: FfiTransferRequest,
        prepared: FfiPreparedManifestSend,
        observer: ManifestTransferObserverV2
    ) throws -> DurableEnvoixManifestSession {
        try FileManager.default.createDirectory(at: recordsDirectory, withIntermediateDirectories: true)
        let session = try startDurableManifestSendV2(
            settings: settings,
            request: request,
            prepared: prepared,
            recordsDir: recordsDirectory.path,
            observer: observer
        )
        durableManifestSessions[request.activityId] = session
        handleManifestActivity(session.activity())
        return session
    }

    func startDurableManifestReceiveSession(
        settings: EnvoixRuntimeSettings,
        request: FfiTransferRequest,
        observer: ManifestTransferObserverV2
    ) throws -> DurableEnvoixManifestSession {
        try FileManager.default.createDirectory(at: recordsDirectory, withIntermediateDirectories: true)
        let session = try startDurableManifestReceiveV2(
            settings: settings,
            request: request,
            recordsDir: recordsDirectory.path,
            observer: observer
        )
        durableManifestSessions[request.activityId] = session
        if let target = ReceivePublicationStore.loadAll()[request.activityId] {
            _ = session.setPublicationTarget(target: target.ffiTarget)
        }
        handleManifestActivity(session.activity())
        return session
    }

    func deliverReceipt(_ data: Data, activityID: String) {
        _ = durableSessions[activityID]?.receiptResponse(blob: data)
    }

    func acknowledgeReceiptPost(activityID: String) {
        _ = durableSessions[activityID]?.receiptPosted()
    }

    private func restoreDurableTransfers() {
        defer {
            restoreClaimedShareDrafts()
            reconcileTransferCache()
        }
        do {
            try FileManager.default.createDirectory(at: recordsDirectory, withIntermediateDirectories: true)
            let records = try listDurableTransferRecords(recordsDir: recordsDirectory.path)
            let legacyPublicationTargets = ReceivePublicationStore.loadAll()
            for record in records where !removedActivityIDs.contains(record.activityId) {
                upsertActivity(record, speedBps: 0)
                let observer = Observer(
                    nil,
                    appModel: self,
                    operationID: UUID(),
                    activityID: record.activityId
                )
                do {
                    let session = try restoreDurableTransferV2(
                        activityId: record.activityId,
                        recordsDir: recordsDirectory.path,
                        observer: observer,
                        mailbox: mailboxObserver
                    )
                    durableSessions[record.activityId] = session
                    let restoredRecord = session.activity()
                    if restoredRecord.state == .publishing {
                        let canonicalTarget = session.publicationTarget().map(ReceivePublicationTarget.init)
                        let target = canonicalTarget ?? legacyPublicationTargets[record.activityId]
                        if canonicalTarget == nil, let target {
                            _ = session.setPublicationTarget(target: target.ffiTarget)
                        }
                        restoreReceivePublicationTarget(target, for: restoredRecord)
                    }
                    handleCoreActivity(session.activity())
                } catch {
                    handleCoreStatus("restore failed: \(error.localizedDescription)", activityID: record.activityId)
                }
            }
            let manifestRecords = try listDurableManifestRecords(recordsDir: recordsDirectory.path)
            for record in manifestRecords where !removedActivityIDs.contains(record.activity.activityId) {
                handleManifestActivity(record)
                let activityID = record.activity.activityId
                let observer = AppleManifestObserver(
                    viewModel: nil,
                    appModel: self,
                    activityID: activityID
                )
                do {
                    let session = try restoreDurableManifestTransferV2(
                        activityId: activityID,
                        recordsDir: recordsDirectory.path,
                        observer: observer
                    )
                    durableManifestSessions[activityID] = session
                    let restoredRecord = session.activity()
                    if restoredRecord.activity.state == .publishing {
                        let canonicalTarget = session.publicationTarget().map(ReceivePublicationTarget.init)
                        let target = canonicalTarget ?? legacyPublicationTargets[activityID]
                        if canonicalTarget == nil, let target {
                            _ = session.setPublicationTarget(target: target.ffiTarget)
                        }
                        restoreReceivePublicationTarget(target, for: restoredRecord.activity)
                    }
                    handleManifestActivity(restoredRecord)
                } catch {
                    handleCoreStatus("Manifest restore failed: \(error.localizedDescription)", activityID: activityID)
                }
            }
        } catch {
            transferLogByActivityID["app", default: []].append("restore scan failed: \(error.localizedDescription)")
        }
    }

    private var protectedCacheActivityIDs: Set<String> {
        var ids = Set(activities.compactMap { record -> String? in
            if !ActivityProjectionPolicy.isTerminal(record.state)
                || record.state == .failed && record.retryable {
                return record.activityId
            }
            return nil
        })
        ids.formUnion(receivePublications.keys)
        for viewModel in [receive, send] where !viewModel.activeActivityID.isEmpty {
            ids.insert(viewModel.activeActivityID)
        }
        return ids
    }

    private var protectedCacheDraftIDs: Set<UUID> {
        #if os(iOS)
        var ids = Set(activityResourceAccess.values.compactMap { ($0 as? ShareDraftLease)?.id })
        if let pendingSendSelection {
            ids.insert(pendingSendSelection.id)
        }
        return ids
        #else
        return []
        #endif
    }

    private func restoreClaimedShareDrafts() {
        #if os(iOS)
        do {
            let store = try ShareDraftStore.live()
            let protectedActivityIDs = protectedCacheActivityIDs
            for (activityID, draftID) in try store.claimedDraftsByActivityID()
                where protectedActivityIDs.contains(activityID)
                    && activityResourceAccess[activityID] == nil {
                _ = try store.load(id: draftID)
                activityResourceAccess[activityID] = ShareDraftLease(id: draftID, store: store)
            }
        } catch {
            transferCacheError = error.localizedDescription
        }
        #endif
    }

    func refreshTransferCache() {
        performTransferCacheWork(cleanup: nil)
    }

    func cleanTransferCache() {
        performTransferCacheWork(cleanup: .manual)
    }

    private func reconcileTransferCache() {
        performTransferCacheWork(cleanup: .automatic)
    }

    private enum TransferCacheCleanup {
        case automatic
        case manual
    }

    private func performTransferCacheWork(cleanup: TransferCacheCleanup?) {
        guard !isCleaningTransferCache else { return }
        let protectedActivityIDs = protectedCacheActivityIDs
        let protectedDraftIDs = protectedCacheDraftIDs
        let startedAt = Date()
        isCleaningTransferCache = true
        transferCacheError = nil
        DispatchQueue.global(qos: .utility).async { [weak self] in
            do {
                let store = TransferCacheStore()
                switch cleanup {
                case .automatic:
                    try store.reconcile(
                        protectingDraftIDs: protectedDraftIDs,
                        protectingActivityIDs: protectedActivityIDs,
                        createdBefore: startedAt
                    )
                case .manual:
                    try store.cleanUnprotected(
                        protectingDraftIDs: protectedDraftIDs,
                        protectingActivityIDs: protectedActivityIDs,
                        createdBefore: startedAt
                    )
                case nil:
                    break
                }
                let summary = try store.summary(
                    protectingDraftIDs: protectedDraftIDs,
                    protectingActivityIDs: protectedActivityIDs
                )
                DispatchQueue.main.async {
                    self?.transferCacheSummary = summary
                    self?.isCleaningTransferCache = false
                }
            } catch {
                DispatchQueue.main.async {
                    self?.transferCacheError = error.localizedDescription
                    self?.isCleaningTransferCache = false
                }
            }
        }
    }

    private func restoreReceivePublicationTarget(
        _ target: ReceivePublicationTarget?,
        for record: FfiTransferActivityRecord
    ) {
        guard let target else { return }
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

    func handleManifestActivity(_ record: FfiManifestActivityRecord) {
        let activityID = record.activity.activityId
        guard !removedActivityIDs.contains(activityID) else { return }
        if let current = manifestActivities[activityID],
           !ActivityProjectionPolicy.shouldAccept(record.activity, replacing: current.activity) {
            return
        }
        manifestActivities[activityID] = record
        handleCoreActivity(record.activity)
    }

    func handleCoreActivity(_ record: FfiTransferActivityRecord) {
        guard !removedActivityIDs.contains(record.activityId) else { return }
        if record.direction == .receive,
           record.state == .publishing,
           receivePublications[record.activityId] != nil {
            upsertActivity(record, speedBps: 0)
            if !record.retryable {
                beginReceivePublication(record)
            }
            return
        }
        upsertActivity(record, speedBps: speedBps(for: record.activityId))
        if ActivityProjectionPolicy.isTerminal(record.state) {
            let preservesResumeData = record.state == .failed && record.retryable
            if !preservesResumeData {
                discardActivityResourceAccess(for: record.activityId)
            }
            if record.direction == .receive,
               record.state == .completed
                || record.state == .canceled
                || (record.state == .failed && !record.retryable) {
                let publication = receivePublications.removeValue(forKey: record.activityId)
                if publication != nil {
                    cleanupReceiveStaging(activityID: record.activityId)
                    ReceivePublicationStore.remove(activityID: record.activityId)
                }
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
        #if os(iOS)
        if let lease = access as? ShareDraftLease {
            do {
                try lease.bind(to: activityID)
                lease.acknowledge()
            } catch {
                handleCoreStatus(
                    "Share source claim failed: \(error.localizedDescription)",
                    activityID: activityID
                )
            }
        }
        #endif
        activityResourceAccess[activityID] = access
    }

    #if os(macOS)
    func retainDestinationAccessForAppLifetime(_ access: SecurityScopedResourceAccess?) {
        guard let access else { return }
        appLifetimeDestinationAccess[access.url.standardizedFileURL.path] = access
    }
    #endif

    private func discardActivityResourceAccess(for activityID: String) {
        guard let access = activityResourceAccess.removeValue(forKey: activityID) else { return }
        #if os(iOS)
        guard let lease = access as? ShareDraftLease else { return }
        do {
            try lease.discard()
        } catch {
            handleCoreStatus(
                "Share source cleanup failed: \(error.localizedDescription)",
                activityID: activityID
            )
        }
        #endif
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

    @discardableResult
    func replaceReceivePublicationTarget(
        activityID: String,
        destinationDirectory: URL,
        bookmark: Data?,
        resourceAccess: AnyObject?
    ) -> Bool {
        guard var publication = receivePublications[activityID],
              publication.completedRecord != nil else { return false }
        let target = ReceivePublicationTarget(
            destinationPath: destinationDirectory.path,
            bookmark: bookmark
        )
        guard setPublicationTarget(target, activityID: activityID) else { return false }

        ReceivePublicationStore.save(target, activityID: activityID)
        publication.destinationDirectory = destinationDirectory
        publication.resourceAccess = resourceAccess
        publication.completedRecord = publicationActivity(activityID: activityID)
        publication.isPublishing = false
        receivePublications[activityID] = publication
        if let resourceAccess {
            retainResourceAccess(resourceAccess, for: activityID)
        } else {
            activityResourceAccess.removeValue(forKey: activityID)
        }
        guard let record = publicationActivity(activityID: activityID) else { return false }
        upsertActivity(record, speedBps: 0)
        beginReceivePublication(record)
        return true
    }

    func abandonReceivePublication(activityID: String) {
        receivePublications.removeValue(forKey: activityID)
        ReceivePublicationStore.remove(activityID: activityID)
        activityResourceAccess.removeValue(forKey: activityID)
    }

    private func setPublicationTarget(
        _ target: ReceivePublicationTarget,
        activityID: String
    ) -> Bool {
        if let session = durableManifestSessions[activityID] {
            return session.setPublicationTarget(target: target.ffiTarget)
        }
        return durableSessions[activityID]?.setPublicationTarget(target: target.ffiTarget) == true
    }

    private func publicationActivity(activityID: String) -> FfiTransferActivityRecord? {
        durableManifestSessions[activityID]?.activity().activity
            ?? durableSessions[activityID]?.activity()
    }

    private func confirmPublication(activityID: String, path: String) -> Bool {
        if let session = durableManifestSessions[activityID] {
            return session.publicationSucceeded(path: path)
        }
        return durableSessions[activityID]?.publicationSucceeded(path: path) == true
    }

    private func failPublication(activityID: String, failure: FfiTransferFailure) -> Bool {
        if let session = durableManifestSessions[activityID] {
            return session.publicationFailed(failure: failure)
        }
        return durableSessions[activityID]?.publicationFailed(failure: failure) == true
    }

    private func beginReceivePublication(_ record: FfiTransferActivityRecord) {
        guard var publication = receivePublications[record.activityId],
              !publication.isPublishing else { return }
        publication.completedRecord = record
        publication.isPublishing = true
        receivePublications[record.activityId] = publication

        let source = URL(fileURLWithPath: record.completedFilePath)
        let destination = publication.destinationDirectory
        let manifest = manifestActivities[record.activityId]
        DispatchQueue.global(qos: .utility).async { [weak self] in
            let result = Result {
                if let manifest {
                    return try publishReceivedManifest(
                        from: source,
                        to: destination,
                        record: manifest
                    )
                }
                return try publishReceivedFile(
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
            guard confirmPublication(activityID: record.activityId, path: finalURL.path) else {
                recordPublicationFailure(
                    record,
                    code: .internalError,
                    category: .internal,
                    recoveryAction: .retry,
                    userMessageKey: "transfer.publish_confirmation_failed",
                    diagnosticMessage: "publish confirmation was not accepted"
                )
                return
            }
            let stagingURL = URL(fileURLWithPath: record.completedFilePath)
            try? FileManager.default.removeItem(at: stagingURL)
            if manifestActivities[record.activityId] == nil {
                try? FileManager.default.removeItem(at: stagingURL.deletingLastPathComponent())
            }
        case .failure(let error):
            recordPublicationFailure(
                record,
                code: .destinationConflict,
                category: .storage,
                recoveryAction: .chooseFolder,
                userMessageKey: "transfer.publish_failed",
                diagnosticMessage: "publish failed: \(error.localizedDescription)"
            )
        }
    }

    private func recordPublicationFailure(
        _ record: FfiTransferActivityRecord,
        code: FfiFailureCode,
        category: FfiFailureCategory,
        recoveryAction: FfiRecoveryAction,
        userMessageKey: String,
        diagnosticMessage: String
    ) {
        let failure = FfiTransferFailure(
            code: code,
            category: category,
            phase: .committing,
            origin: .local,
            direction: .receive,
            transferId: record.transferId,
            attemptId: record.attemptId,
            retryable: true,
            recoveryAction: recoveryAction,
            userMessageKey: userMessageKey,
            diagnosticMessage: diagnosticMessage
        )
        if failPublication(activityID: record.activityId, failure: failure) {
            if let manifest = durableManifestSessions[record.activityId]?.activity() {
                handleManifestActivity(manifest)
            } else if let activity = durableSessions[record.activityId]?.activity() {
                upsertActivity(activity, speedBps: 0)
            }
            return
        }
        var pending = record
        pending.updatedAtMs = UInt64(Date().timeIntervalSince1970 * 1000)
        pending.failureCode = code
        pending.failureCategory = category
        pending.failurePhase = .committing
        pending.failureOrigin = .local
        pending.userMessageKey = userMessageKey
        pending.retryable = true
        pending.recoveryAction = recoveryAction
        pending.diagnosticMessage = diagnosticMessage
        upsertActivity(pending, speedBps: 0)
    }

    private func retryReceivePublication(_ activityID: String) -> Bool {
        guard var publication = receivePublications[activityID],
              !publication.isPublishing,
              publication.completedRecord != nil else { return false }
        let storedTarget = ReceivePublicationStore.loadAll()[activityID]
            ?? ReceivePublicationTarget(
                destinationPath: publication.destinationDirectory.path,
                bookmark: nil
            )
        guard setPublicationTarget(storedTarget, activityID: activityID),
              let record = publicationActivity(activityID: activityID) else { return false }
        publication.completedRecord = record
        receivePublications[activityID] = publication
        upsertActivity(record, speedBps: 0)
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
        let activityTimeline = activityMetrics[activityID]?.log ?? []
        if receive.ownsActivity(activityID) {
            return (
                mergedActivityDiagnosticLog(
                    activityTimeline: activityTimeline,
                    observerLog: receive.eventLog
                ),
                receive.transferEvents.map(TransferDiagnostics.transferEventLine)
            )
        }
        if send.ownsActivity(activityID) {
            return (
                mergedActivityDiagnosticLog(
                    activityTimeline: activityTimeline,
                    observerLog: send.eventLog
                ),
                send.transferEvents.map(TransferDiagnostics.transferEventLine)
            )
        }
        return (
            mergedActivityDiagnosticLog(
                activityTimeline: activityTimeline,
                observerLog: transferLogByActivityID[activityID] ?? []
            ),
            transferEventLinesByActivityID[activityID] ?? []
        )
    }

    func snapshotDiagnostics(from viewModel: TransferViewModel, activityID: String) {
        guard !activityID.isEmpty else { return }
        transferLogByActivityID[activityID] = viewModel.eventLog
        transferEventLinesByActivityID[activityID] = viewModel.transferEvents.map(TransferDiagnostics.transferEventLine)
    }

    private func speedBps(for activityID: String) -> Double {
        if receive.ownsActivity(activityID) { return receive.bytesPerSec }
        if send.ownsActivity(activityID) { return send.bytesPerSec }
        return activityMetrics[activityID]?.speedBps ?? 0
    }

    private func upsertActivity(_ record: FfiTransferActivityRecord, speedBps: Double) {
        guard !removedActivityIDs.contains(record.activityId) else { return }
        if let index = activities.firstIndex(where: { $0.activityId == record.activityId }) {
            guard ActivityProjectionPolicy.shouldAccept(record, replacing: activities[index]) else { return }
            activities[index] = record
        } else {
            activities.append(record)
        }
        upsertMetrics(for: record, speedBps: speedBps)
        activities.sort { lhs, rhs in lhs.updatedAtMs > rhs.updatedAtMs }
        if activities.count > activityCap {
            let previousIDs = Set(activities.map(\.activityId))
            activities = ActivityProjectionPolicy.pruneTerminalHistory(activities, limit: activityCap)
            let retainedIDs = Set(activities.map(\.activityId))
            let removed = previousIDs.subtracting(retainedIDs)
            for id in removed {
                receive.forgetRoomID(for: id)
                send.forgetRoomID(for: id)
                manifestActivities.removeValue(forKey: id)
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
        metrics.etaSeconds = estimatedRemainingSeconds(
            total: record.totalBytes,
            transferred: record.bytesTransferred,
            bytesPerSecond: liveSpeed,
            isStable: liveSpeed > 0
        )
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
        guard record.bytesResumed == 0,
              manifestActivities[record.activityId]?.entryResults.contains(where: {
                  $0.status == .skippedIdentical
              }) != true else { return 0 }
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
            message = record.diagnosticMessage.isEmpty ? "pairing" : record.diagnosticMessage
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
            if isFullyResumedCompletion(record) {
                message = record.direction == .send
                    ? "completed · already at receiver · \(byteString(record.totalBytes))"
                    : "completed · already present · \(byteString(record.totalBytes))"
            } else {
                message = "completed · \(byteString(record.bytesTransferred))"
            }
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

    @Published private var fallbackPhase: Phase = .idle
    @Published var invite: String = ""        // receiver only
    @Published var fileName: String = ""
    @Published var transferred: UInt64 = 0
    @Published var total: UInt64 = 0
    @Published var statusText: String = ""
    @Published var peerAddress: String = ""   // raw IP-bearing address, hidden by default
    @Published var eventLog: [String] = []
    @Published var bytesPerSec: Double = 0    // rolling average, 0 until measurable
    @Published var completedFileURL: URL?     // receiver only: where the file landed
    @Published var completedItemURLs: [URL] = []
    @Published private var fallbackFailure: FfiTransferFailure?
    @Published var transferEvents: [FfiTransferEvent] = []
    @Published var transferActivity: FfiTransferActivityRecord?
    @Published private(set) var isPreparingManifest = false

    weak var appModel: AppModel?

    private var session: DurableEnvoixSession?
    private var manifestSession: DurableEnvoixManifestSession?
    private var manifestPreparationTask: Task<Void, Never>?
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

    /// Setup failures can occur before the core creates a record. Once a
    /// canonical Activity exists, it is the only lifecycle source of truth.
    var phase: Phase {
        guard let transferActivity else { return fallbackPhase }
        return Self.presentationPhase(for: transferActivity, language: displayLanguage)
    }

    var failure: FfiTransferFailure? {
        guard let transferActivity,
              transferActivity.state == .failed
                || transferActivity.state == .publishing && transferActivity.retryable else {
            return fallbackFailure
        }
        return Self.failure(from: transferActivity)
    }

    static func presentationPhase(
        for record: FfiTransferActivityRecord,
        language: String = "en"
    ) -> Phase {
        switch record.state {
        case .queued, .binding, .waitingForPeer, .pairing, .connecting,
                .verifying, .publishing, .unconfirmed:
            return .waiting
        case .transferring:
            return .transferring
        case .paused:
            return .paused
        case .completed:
            return .completed(bytes: record.bytesTransferred)
        case .canceled:
            return .canceled
        case .failed:
            return .failed(friendlyFailure(Self.failure(from: record), language: language))
        case .unknown:
            return .failed(AppText.value(
                "The transfer entered an unknown state. Copy diagnostics from Activity.",
                "传输进入未知状态。请从活动页复制诊断信息。",
                language: language
            ))
        }
    }

    private static func failure(from record: FfiTransferActivityRecord) -> FfiTransferFailure {
        FfiTransferFailure(
            code: record.failureCode,
            category: record.failureCategory,
            phase: record.failurePhase,
            origin: record.failureOrigin,
            direction: record.direction,
            transferId: record.transferId,
            attemptId: record.attemptId,
            retryable: record.retryable,
            recoveryAction: record.recoveryAction,
            userMessageKey: record.userMessageKey,
            diagnosticMessage: record.diagnosticMessage
        )
    }

    var progressFraction: Double {
        total > 0 ? Double(transferred) / Double(total) : 0
    }

    /// Seconds left after the rolling estimator has seen enough real byte deltas.
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
        switch phase {
        case .waiting, .transferring, .paused: return true
        default: return false
        }
    }

    var isFinalizing: Bool {
        guard let activity = transferActivity else { return false }
        return activity.state == .publishing
            || activityActionAvailability(for: activity).isFinalizing
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
            let started = startManifestReceive(settings: settings, request: request)
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
        start(settings: settings, request: request)
        retainResourceAccess(sourceAccess)
    }

    /// Send by pairing through a rendezvous room code.
    func startSendingWithRoom(filePath: String, code: String, settings: EnvoixRuntimeSettings, sourceAccess: AnyObject? = nil) {
        destinationDir = nil
        let request = makeRequest(direction: .send, mode: .room, settings: settings, filePath: filePath, code: code)
        rememberRoomID(for: request.activityId, code: code)
        let started = start(settings: settings, request: request)
        if !started {
            forgetRoomID(for: request.activityId)
        }
        retainResourceAccess(sourceAccess)
    }

    /// Send to the peer encoded in an invite string.
    func startSendingWithInvite(
        filePath: String,
        invite: String,
        settings: EnvoixRuntimeSettings,
        pathPolicy: FfiPathPolicy = .auto,
        sourceAccess: AnyObject? = nil
    ) {
        destinationDir = nil
        let request = makeRequest(
            direction: .send,
            mode: .invite,
            settings: settings,
            filePath: filePath,
            invite: invite,
            pathPolicy: pathPolicy
        )
        start(settings: settings, request: request)
        retainResourceAccess(sourceAccess)
    }

    func startSendingManifestWithToken(
        selectedPaths: [String],
        token: String,
        settings: EnvoixRuntimeSettings,
        sourceAccess: AnyObject? = nil
    ) {
        let request = makeRequest(direction: .send, mode: .mdns, settings: settings, token: token)
        prepareAndStartManifestSend(
            settings: settings,
            request: request,
            selectedPaths: selectedPaths,
            sourceAccess: sourceAccess
        )
    }

    func startSendingManifestWithRoom(
        selectedPaths: [String],
        code: String,
        settings: EnvoixRuntimeSettings,
        sourceAccess: AnyObject? = nil
    ) {
        let request = makeRequest(direction: .send, mode: .room, settings: settings, code: code)
        rememberRoomID(for: request.activityId, code: code)
        prepareAndStartManifestSend(
            settings: settings,
            request: request,
            selectedPaths: selectedPaths,
            sourceAccess: sourceAccess
        )
    }

    func startSendingManifestWithInvite(
        selectedPaths: [String],
        invite: String,
        settings: EnvoixRuntimeSettings,
        pathPolicy: FfiPathPolicy = .auto,
        sourceAccess: AnyObject? = nil
    ) {
        let request = makeRequest(
            direction: .send,
            mode: .invite,
            settings: settings,
            invite: invite,
            pathPolicy: pathPolicy
        )
        prepareAndStartManifestSend(
            settings: settings,
            request: request,
            selectedPaths: selectedPaths,
            sourceAccess: sourceAccess
        )
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
        let accepted = manifestSession?.cancel() ?? session?.cancel() ?? false
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
        let discarded = manifestSession?.remove() ?? session?.remove() ?? false
        if activityID == currentActivityID {
            suppressNextFailure = true
            operationID = UUID()
            reset()
            fallbackPhase = .canceled
            statusText = AppText.value("Transfer removed", "传输已删除", language: displayLanguage)
        }
        return discarded
    }

    @discardableResult
    func cancelManifestPreparation() -> Bool {
        guard isPreparingManifest else { return false }
        manifestPreparationTask?.cancel()
        manifestPreparationTask = nil
        isPreparingManifest = false
        operationID = UUID()
        forgetRoomID(for: currentActivityID)
        resourceAccess = nil
        fallbackPhase = .canceled
        statusText = AppText.value("Canceled", "已取消", language: displayLanguage)
        return true
    }

    func listTransferActivities() -> [FfiTransferActivityRecord] {
        if let manifestSession {
            return [manifestSession.activity().activity]
        }
        return session.map { [$0.activity()] } ?? []
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
        let paused = manifestSession?.pause() ?? session?.pause() ?? false
        if paused {
            bytesPerSec = 0
            statusText = AppText.value("Pausing…", "正在暂停…", language: displayLanguage)
        }
        return paused
    }

    @discardableResult
    func resumeActivity(_ activityID: String) -> Bool {
        guard !activityID.isEmpty, activityID == currentActivityID else { return false }
        let resumed = manifestSession?.resume() ?? session?.resume() ?? false
        if resumed {
            suppressNextFailure = false
            statusText = AppText.value("Resuming…", "正在继续…", language: displayLanguage)
        }
        return resumed
    }

    @discardableResult
    private func startManifestReceive(
        settings: EnvoixRuntimeSettings,
        request: FfiTransferRequest
    ) -> Bool {
        beginManifestOperation(settings: settings, request: request)
        do {
            guard let appModel else {
                throw RuntimeSettingsError("The transfer service is unavailable.")
            }
            let observer = AppleManifestObserver(
                viewModel: self,
                appModel: appModel,
                activityID: request.activityId
            )
            let startedSession = try appModel.startDurableManifestReceiveSession(
                settings: settings,
                request: request,
                observer: observer
            )
            manifestSession = startedSession
            handleManifestActivity(startedSession.activity())
            return true
        } catch {
            fallbackPhase = .failed(friendlyError(error.localizedDescription, language: displayLanguage))
            return false
        }
    }

    private func prepareAndStartManifestSend(
        settings: EnvoixRuntimeSettings,
        request: FfiTransferRequest,
        selectedPaths: [String],
        sourceAccess: AnyObject?
    ) {
        destinationDir = nil
        beginManifestOperation(settings: settings, request: request)
        statusText = AppText.value("Preparing selected items…", "正在准备所选项目…", language: displayLanguage)
        resourceAccess = sourceAccess
        isPreparingManifest = true
        let operationID = operationID
        manifestPreparationTask = Task { @MainActor [weak self] in
            guard let self else { return }
            do {
                let prepared = try await prepareManifestSend(
                    activityId: request.activityId,
                    selectedPaths: selectedPaths
                )
                guard operationID == self.operationID,
                      request.activityId == self.currentActivityID,
                      let appModel = self.appModel else { return }
                self.manifestPreparationTask = nil
                self.isPreparingManifest = false
                let observer = AppleManifestObserver(
                    viewModel: self,
                    appModel: appModel,
                    activityID: request.activityId
                )
                let startedSession = try appModel.startDurableManifestSendSession(
                    settings: settings,
                    request: request,
                    prepared: prepared,
                    observer: observer
                )
                self.manifestSession = startedSession
                self.handleManifestActivity(startedSession.activity())
                self.retainResourceAccess(sourceAccess)
            } catch {
                guard operationID == self.operationID else { return }
                self.manifestPreparationTask = nil
                self.isPreparingManifest = false
                self.forgetRoomID(for: request.activityId)
                self.resourceAccess = nil
                self.handleFailed(error.localizedDescription)
            }
        }
    }

    private func beginManifestOperation(
        settings: EnvoixRuntimeSettings,
        request: FfiTransferRequest
    ) {
        manifestPreparationTask?.cancel()
        manifestPreparationTask = nil
        isPreparingManifest = false
        suppressNextFailure = false
        reset()
        session = nil
        manifestSession = nil
        bindPresentation(to: request.activityId)
        displayLanguage = settings.language
        operationID = UUID()
        fallbackPhase = .waiting
    }

    /// Creates one independently durable session for the new Activity card.
    @discardableResult
    private func start(
        settings: EnvoixRuntimeSettings,
        request: FfiTransferRequest
    ) -> Bool {
        manifestPreparationTask?.cancel()
        manifestPreparationTask = nil
        isPreparingManifest = false
        suppressNextFailure = false
        reset()
        session = nil
        manifestSession = nil
        bindPresentation(to: request.activityId)
        displayLanguage = settings.language
        operationID = UUID()
        let operationID = operationID
        fallbackPhase = .waiting
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
            let startedSession = try appModel.startDurableSession(
                settings: settings,
                request: request,
                observer: observer
            )
            session = startedSession
            handleTransferActivity(startedSession.activity())
            return true
        } catch {
            fallbackPhase = .failed(friendlyError(error.localizedDescription, language: displayLanguage))
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
        pathPolicy: FfiPathPolicy = .auto,
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
            pathPolicy: pathPolicy,
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

    /// Manifest V2 reports aggregate progress through structured events rather
    /// than the legacy `onProgress` callback. Feed only those byte counters into
    /// the estimator; canonical phase/state still comes from Activity snapshots.
    func handleManifestTransferEvent(_ event: FfiTransferEvent) {
        handleTransferEvent(event)
        guard event.kind == .progress, event.totalBytes > 0 else { return }
        handleProgress(event.bytesTransferred, event.totalBytes)
    }

    func handleTransferActivity(
        _ record: FfiTransferActivityRecord,
        manifest: FfiManifestActivityRecord? = nil
    ) {
        transferActivity = record
        if record.state == .completed,
           record.direction == .receive,
           !record.completedFilePath.isEmpty {
            completedItemURLs = manifest.map(availableCompletedManifestItemURLs) ?? []
            completedFileURL = manifest.flatMap(availableCompletedManifestURL)
                ?? availableCompletedFileURL(
                    path: record.completedFilePath,
                    expectedBytes: record.bytesTransferred
                )
            if completedItemURLs.isEmpty, manifest == nil, let completedFileURL {
                completedItemURLs = [completedFileURL]
            }
            if completedFileURL == nil {
                statusText = AppText.value(
                    "Transfer confirmed, but the saved file is not currently available.",
                    "传输已确认，但当前保存位置中未找到该文件。",
                    language: displayLanguage
                )
            }
        }
        if ActivityProjectionPolicy.isTerminal(record.state) {
            resourceAccess = nil
        }
        if record.state == .canceled {
            suppressNextFailure = false
        }
        syncPhase(with: record)
        releasePresentationSlotIfPaused(record)
        releasePresentationSlotIfTerminal(record)
    }

    func handleManifestActivity(_ record: FfiManifestActivityRecord) {
        guard record.activity.activityId == currentActivityID else { return }
        fileName = record.activity.fileName
        transferred = record.activity.bytesTransferred
        total = record.activity.totalBytes
        if !record.activity.invite.isEmpty {
            invite = record.activity.invite
        }
        handleTransferActivity(record.activity, manifest: record)
    }

    func handleStarted(_ name: String, _ total: UInt64) {
        appendLog("started · \(name) (\(byteString(total)))")
        fileName = name
        self.total = total
        transferred = 0
        rate.reset()
        bytesPerSec = 0
        if transferActivity == nil {
            fallbackPhase = .transferring
        }
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
            if let completedFileURL {
                completedItemURLs = [completedFileURL]
            }
        }
        resourceAccess = nil
        if transferActivity == nil {
            fallbackPhase = .completed(bytes: bytes)
        }
    }

    func handleTransferFailed(_ failure: FfiTransferFailure) {
        appendLog("failed · \(failure.diagnosticMessage)")
        if suppressNextFailure || transferActivity?.state == .canceled {
            return
        }
        fallbackFailure = failure
        resourceAccess = nil
        if transferActivity == nil {
            fallbackPhase = .failed(friendlyFailure(failure, language: displayLanguage))
        }
    }

    func handleFailed(_ reason: String) {
        appendLog("failed · \(reason)")
        if suppressNextFailure || transferActivity?.state == .canceled {
            suppressNextFailure = false
            resourceAccess = nil
            if transferActivity == nil {
                fallbackPhase = .canceled
            }
            statusText = AppText.value("Canceled", "已取消", language: displayLanguage)
            return
        }
        if let failure {
            if transferActivity == nil {
                fallbackPhase = .failed(friendlyFailure(failure, language: displayLanguage))
            }
            return
        }
        resourceAccess = nil
        if transferActivity == nil {
            fallbackPhase = .failed(friendlyError(reason, language: displayLanguage))
        }
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
        completedItemURLs = []
        fallbackFailure = nil
        transferActivity = nil
        isPreparingManifest = false
        resourceAccess = nil
        eventLog.removeAll()
        transferEvents.removeAll()
        currentActivityID = ""
        rate.reset()
        fallbackPhase = .idle
    }

    func bindPresentation(to activityID: String) {
        currentActivityID = activityID
    }

    private func releasePresentationSlotIfPaused(_ record: FfiTransferActivityRecord) {
        guard record.state == .paused, record.activityId == currentActivityID else { return }
        appModel?.snapshotDiagnostics(from: self, activityID: record.activityId)
        operationID = UUID()
        session = nil
        manifestSession = nil
        reset()
    }

    private func releasePresentationSlotIfTerminal(_ record: FfiTransferActivityRecord) {
        guard ActivityProjectionPolicy.isTerminal(record.state),
              record.activityId == currentActivityID else { return }
        appModel?.snapshotDiagnostics(from: self, activityID: record.activityId)
        operationID = UUID()
        session = nil
        manifestSession = nil
        reset()
    }

    private func syncPhase(with record: FfiTransferActivityRecord) {
        guard record.activityId == currentActivityID else { return }
        switch record.state {
        case .queued, .binding, .waitingForPeer, .pairing, .connecting:
            break
        case .verifying:
            if isFinalizing {
                bytesPerSec = 0
                statusText = AppText.value("Confirming delivery", "正在确认送达", language: displayLanguage)
            }
        case .publishing:
            bytesPerSec = 0
            statusText = AppText.value("Saving to selected folder", "正在保存到所选文件夹", language: displayLanguage)
        case .unconfirmed:
            bytesPerSec = 0
            statusText = AppText.value("Confirming delivery", "正在确认送达", language: displayLanguage)
        case .transferring:
            break
        case .paused:
            bytesPerSec = 0
            statusText = AppText.value("Paused", "已暂停", language: displayLanguage)
        case .canceled:
            bytesPerSec = 0
        case .completed:
            bytesPerSec = 0
            if isFullyResumedCompletion(record) {
                statusText = record.direction == .send
                    ? AppText.value("Receiver already has this file; no data was sent", "对方已有此文件，本次未发送数据", language: displayLanguage)
                    : AppText.value("File already exists; no data was received", "文件已存在，本次未接收数据", language: displayLanguage)
            }
        case .failed, .unknown:
            break
        }
    }
}

func estimatedRemainingSeconds(
    total: UInt64,
    transferred: UInt64,
    bytesPerSecond: Double,
    isStable: Bool
) -> Double? {
    guard isStable,
          total > transferred,
          bytesPerSecond.isFinite,
          bytesPerSecond > 0 else { return nil }
    let seconds = Double(total - transferred) / bytesPerSecond
    guard seconds.isFinite, seconds >= 0, seconds <= 7 * 24 * 60 * 60 else { return nil }
    return seconds
}

/// A short rolling window plus light smoothing. The first cumulative value is
/// only a baseline, so resumed/skipped bytes are never reported as wire speed.
struct RateTracker {
    private struct Sample { let time: TimeInterval; let bytes: UInt64 }
    private var samples: [Sample] = []
    private var smoothedBytesPerSecond: Double = 0
    private var positiveDeltaCount = 0
    private let window: TimeInterval = 4
    private let minimumObservationDuration: TimeInterval = 0.8
    private let smoothingFactor = 0.35
    private(set) var isStable = false

    mutating func reset() {
        samples.removeAll()
        smoothedBytesPerSecond = 0
        positiveDeltaCount = 0
        isStable = false
    }

    /// Records a cumulative byte count, returns the current bytes/sec estimate.
    mutating func record(
        _ bytes: UInt64,
        at now: TimeInterval = ProcessInfo.processInfo.systemUptime
    ) -> Double {
        guard now.isFinite else { return 0 }
        if let last = samples.last {
            guard now > last.time, bytes >= last.bytes else {
                if bytes < last.bytes || now < last.time {
                    reset()
                    samples.append(Sample(time: now, bytes: bytes))
                } else if bytes > last.bytes {
                    samples[samples.count - 1] = Sample(time: last.time, bytes: bytes)
                }
                return 0
            }
            if bytes > last.bytes {
                positiveDeltaCount += 1
            }
        }
        samples.append(Sample(time: now, bytes: bytes))
        samples.removeAll { now - $0.time > window }
        guard let first = samples.first, samples.count > 1 else { return 0 }
        let dt = now - first.time
        guard dt >= minimumObservationDuration, bytes > first.bytes else { return 0 }
        let raw = Double(bytes - first.bytes) / dt
        smoothedBytesPerSecond = smoothedBytesPerSecond > 0
            ? smoothingFactor * raw + (1 - smoothingFactor) * smoothedBytesPerSecond
            : raw
        isStable = positiveDeltaCount >= 2
        return isStable ? smoothedBytesPerSecond : 0
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

/// Manifest snapshots own canonical Activity state; structured events are
/// retained separately for diagnostics and never form a second state machine.
final class AppleManifestObserver: ManifestTransferObserverV2, @unchecked Sendable {
    private weak var viewModel: TransferViewModel?
    private weak var appModel: AppModel?
    private let activityID: String

    init(viewModel: TransferViewModel?, appModel: AppModel, activityID: String) {
        self.viewModel = viewModel
        self.appModel = appModel
        self.activityID = activityID
    }

    func onManifestEvent(event: FfiTransferEvent) {
        DispatchQueue.main.async { [weak viewModel, weak appModel, activityID] in
            appModel?.handleCoreEvent(event, activityID: activityID)
            if let viewModel, viewModel.activeActivityID == activityID {
                viewModel.handleManifestTransferEvent(event)
            }
        }
    }

    func onManifestActivity(record: FfiManifestActivityRecord) {
        DispatchQueue.main.async { [weak viewModel, weak appModel, activityID] in
            appModel?.handleManifestActivity(record)
            if let viewModel, viewModel.activeActivityID == activityID {
                viewModel.handleManifestActivity(record)
            }
        }
    }
}
