import Foundation
#if os(iOS)
import Combine
import EnvoixCore
#endif

enum RememberedRoomOutboxState: String, Codable {
    case queued
    case offering
    case transferring
    case needsAttention = "needs_attention"
}

struct RememberedRoomOutboxEntry: Codable, Equatable, Identifiable {
    let id: String
    let relationshipID: String
    let jobID: String
    let sourcePaths: [String]
    let sourceBookmarks: [Data]
    let shareDraftID: UUID?
    let rootNames: [String]
    let itemCount: Int
    let directoryCount: Int
    let totalBytes: UInt64
    var state: RememberedRoomOutboxState
    var offerID: String?
    var activityID: String?
    var lastError: String?
    let createdAtEpochMilliseconds: Int64
    var updatedAtEpochMilliseconds: Int64
}

enum RememberedRoomOutboxError: LocalizedError {
    case invalidRecord
    case full
    case roomFull
    case corruptStore

    var errorDescription: String? {
        switch self {
        case .invalidRecord:
            return "The prepared transfer cannot be queued."
        case .full:
            return "Too many room transfers are queued."
        case .roomFull:
            return "Too many transfers are queued for this room."
        case .corruptStore:
            return "Queued room transfers are temporarily unavailable; no records were changed."
        }
    }
}

/**
 * Durable ownership index for already-prepared Manifest-v2 sender jobs.
 *
 * The Rust job remains authoritative for sources and inventory. This store
 * only associates the stable job identity with a remembered relationship and
 * prevents an interrupted room-control offer from being replayed silently.
 */
final class RememberedRoomOutboxStore: @unchecked Sendable {
    static let shared = RememberedRoomOutboxStore()
    static let didChange = Notification.Name("dev.envoix.remembered-room-outbox.changed")

    private struct Envelope: Codable {
        let version: Int
        var entries: [RememberedRoomOutboxEntry]
    }

    private let lock = NSLock()
    private let fileURL: URL?
    private let clockEpochMilliseconds: () -> Int64

    init(
        fileURL: URL? = nil,
        clockEpochMilliseconds: @escaping () -> Int64 = {
            Int64((Date().timeIntervalSince1970 * 1_000).rounded())
        }
    ) {
        self.fileURL = fileURL
        self.clockEpochMilliseconds = clockEpochMilliseconds
    }

    func entries(relationshipID: String? = nil) throws -> [RememberedRoomOutboxEntry] {
        try lock.withEnvoixLock {
            try read()
                .filter { relationshipID == nil || $0.relationshipID == relationshipID }
                .sorted {
                    ($0.createdAtEpochMilliseconds, $0.id)
                        < ($1.createdAtEpochMilliseconds, $1.id)
                }
        }
    }

    @discardableResult
    func enqueue(
        relationshipID: String,
        jobID: String,
        sourcePaths: [String] = [],
        sourceBookmarks: [Data] = [],
        shareDraftID: UUID? = nil,
        rootNames: [String],
        itemCount: Int,
        directoryCount: Int,
        totalBytes: UInt64
    ) throws -> RememberedRoomOutboxEntry {
        try lock.withEnvoixLock {
            guard !relationshipID.isEmpty,
                  relationshipID.utf8.count <= Self.maximumRelationshipIDBytes,
                  Self.validJobID(jobID),
                  sourceBookmarks.isEmpty || sourceBookmarks.count == sourcePaths.count,
                  itemCount >= 0,
                  directoryCount >= 0,
                  directoryCount <= itemCount else {
                throw RememberedRoomOutboxError.invalidRecord
            }
            var current = try read()
            if let duplicate = current.first(where: { $0.jobID == jobID }) {
                guard duplicate.relationshipID == relationshipID else {
                    throw RememberedRoomOutboxError.invalidRecord
                }
                return duplicate
            }
            guard current.count < Self.maximumGlobalEntries else {
                throw RememberedRoomOutboxError.full
            }
            guard current.filter({ $0.relationshipID == relationshipID }).count
                    < Self.maximumRelationshipEntries else {
                throw RememberedRoomOutboxError.roomFull
            }
            let now = clockEpochMilliseconds()
            let entry = RememberedRoomOutboxEntry(
                id: UUID().uuidString,
                relationshipID: relationshipID,
                jobID: jobID,
                sourcePaths: sourcePaths,
                sourceBookmarks: sourceBookmarks,
                shareDraftID: shareDraftID,
                rootNames: Array(
                    rootNames
                        .map { $0.trimmingCharacters(in: .whitespacesAndNewlines) }
                        .filter { !$0.isEmpty }
                        .map { String($0.prefix(Self.maximumRootNameCharacters)) }
                        .prefix(Self.maximumRootNames)
                ),
                itemCount: itemCount,
                directoryCount: directoryCount,
                totalBytes: totalBytes,
                state: .queued,
                offerID: nil,
                activityID: nil,
                lastError: nil,
                createdAtEpochMilliseconds: now,
                updatedAtEpochMilliseconds: now
            )
            current.append(entry)
            try write(current)
            return entry
        }
    }

    /**
     * Atomically reserves the oldest queued job for one control-session offer.
     */
    func claimNext(relationshipID: String) throws -> RememberedRoomOutboxEntry? {
        try lock.withEnvoixLock {
            var current = try read()
            guard let index = current.firstIndex(where: {
                $0.relationshipID == relationshipID && $0.state == .queued
            }) else {
                return nil
            }
            current[index].state = .offering
            current[index].offerID = UUID().uuidString
            current[index].activityID = nil
            current[index].lastError = nil
            current[index].updatedAtEpochMilliseconds = clockEpochMilliseconds()
            try write(current)
            return current[index]
        }
    }

    @discardableResult
    func markTransferring(
        id: String,
        offerID: String,
        activityID: String
    ) throws -> Bool {
        try update(id: id) { entry in
            guard entry.state == .offering,
                  entry.offerID == offerID,
                  !activityID.isEmpty else {
                return false
            }
            entry.state = .transferring
            entry.activityID = activityID
            entry.lastError = nil
            entry.updatedAtEpochMilliseconds = clockEpochMilliseconds()
            return true
        }
    }

    @discardableResult
    func requeue(
        id: String,
        offerID: String,
        message: String? = nil
    ) throws -> Bool {
        try update(id: id) { entry in
            guard entry.state == .offering, entry.offerID == offerID else {
                return false
            }
            entry.state = .queued
            entry.offerID = nil
            entry.activityID = nil
            entry.lastError = message.map { String($0.prefix(Self.maximumErrorCharacters)) }
            entry.updatedAtEpochMilliseconds = clockEpochMilliseconds()
            return true
        }
    }

    @discardableResult
    func markNeedsAttention(
        id: String,
        message: String,
        expectedOfferID: String? = nil,
        expectedActivityID: String? = nil
    ) throws -> Bool {
        try update(id: id) { entry in
            let matches: Bool
            switch entry.state {
            case .offering:
                matches = expectedOfferID != nil
                    && entry.offerID == expectedOfferID
                    && expectedActivityID == nil
            case .transferring:
                matches = expectedActivityID != nil
                    && entry.activityID == expectedActivityID
            case .queued, .needsAttention:
                matches = expectedOfferID == nil && expectedActivityID == nil
            }
            guard matches else { return false }
            entry.state = .needsAttention
            entry.lastError = String(message.prefix(Self.maximumErrorCharacters))
            entry.updatedAtEpochMilliseconds = clockEpochMilliseconds()
            return true
        }
    }

    @discardableResult
    func retry(id: String) throws -> Bool {
        try update(id: id) { entry in
            guard entry.state == .needsAttention else { return false }
            entry.state = .queued
            entry.offerID = nil
            entry.activityID = nil
            entry.lastError = nil
            entry.updatedAtEpochMilliseconds = clockEpochMilliseconds()
            return true
        }
    }

    @discardableResult
    func remove(
        id: String,
        expectedOfferID: String? = nil,
        expectedActivityID: String? = nil
    ) throws -> RememberedRoomOutboxEntry? {
        try lock.withEnvoixLock {
            var current = try read()
            guard let index = current.firstIndex(where: { $0.id == id }) else {
                return nil
            }
            let entry = current[index]
            let removable: Bool
            switch entry.state {
            case .offering:
                removable = expectedOfferID != nil
                    && entry.offerID == expectedOfferID
                    && expectedActivityID == nil
            case .transferring:
                removable = expectedActivityID != nil
                    && entry.activityID == expectedActivityID
            case .queued, .needsAttention:
                removable = expectedOfferID == nil && expectedActivityID == nil
            }
            guard removable else { return nil }
            let removed = current.remove(at: index)
            try write(current)
            return removed
        }
    }

    /**
     * An interrupted offer may already have reached the peer. Require explicit
     * confirmation before retrying so a process restart cannot duplicate files.
     */
    @discardableResult
    func reconcileInterruptedAttempts() throws -> Int {
        try lock.withEnvoixLock {
            var current = try read()
            let now = clockEpochMilliseconds()
            var changed = 0
            for index in current.indices
                where current[index].state == .offering
                    || current[index].state == .transferring {
                current[index].state = .needsAttention
                current[index].lastError =
                    "The previous send was interrupted. Check the peer before retrying."
                current[index].updatedAtEpochMilliseconds = now
                changed += 1
            }
            if changed > 0 {
                try write(current)
            }
            return changed
        }
    }

    private func update(
        id: String,
        mutation: (inout RememberedRoomOutboxEntry) -> Bool
    ) throws -> Bool {
        try lock.withEnvoixLock {
            var current = try read()
            guard let index = current.firstIndex(where: { $0.id == id }) else {
                return false
            }
            guard mutation(&current[index]) else { return false }
            try write(current)
            return true
        }
    }

    private func read() throws -> [RememberedRoomOutboxEntry] {
        let url = try resolvedFileURL()
        guard FileManager.default.fileExists(atPath: url.path) else { return [] }
        do {
            let envelope = try JSONDecoder().decode(
                Envelope.self,
                from: Data(contentsOf: url)
            )
            guard envelope.version == Self.formatVersion,
                  envelope.entries.allSatisfy(Self.valid) else {
                throw RememberedRoomOutboxError.corruptStore
            }
            return envelope.entries
        } catch let error as RememberedRoomOutboxError {
            throw error
        } catch {
            throw RememberedRoomOutboxError.corruptStore
        }
    }

    private func write(_ entries: [RememberedRoomOutboxEntry]) throws {
        let data = try JSONEncoder().encode(
            Envelope(version: Self.formatVersion, entries: entries)
        )
        try data.write(
            to: resolvedFileURL(),
            options: [.atomic, .completeFileProtection]
        )
        // Every mutation calls write while holding the store's non-recursive
        // lock. Deliver after that call stack unwinds so an observer may safely
        // reload entries without deadlocking.
        DispatchQueue.main.async { [weak self] in
            guard let self else { return }
            NotificationCenter.default.post(name: Self.didChange, object: self)
        }
    }

    private func resolvedFileURL() throws -> URL {
        let url: URL
        if let fileURL {
            url = fileURL
        } else {
            guard let support = FileManager.default.urls(
                for: .applicationSupportDirectory,
                in: .userDomainMask
            ).first else {
                throw RememberedRoomOutboxError.corruptStore
            }
            url = support
                .appendingPathComponent("envoix/room-outbox", isDirectory: true)
                .appendingPathComponent("outbox-v1.json")
        }
        try FileManager.default.createDirectory(
            at: url.deletingLastPathComponent(),
            withIntermediateDirectories: true,
            attributes: [.posixPermissions: 0o700]
        )
        return url
    }

    private static func valid(_ entry: RememberedRoomOutboxEntry) -> Bool {
        !entry.id.isEmpty
            && !entry.relationshipID.isEmpty
            && validJobID(entry.jobID)
            && (entry.sourceBookmarks.isEmpty
                || entry.sourceBookmarks.count == entry.sourcePaths.count)
            && entry.itemCount >= 0
            && entry.directoryCount >= 0
            && entry.directoryCount <= entry.itemCount
    }

    private static func validJobID(_ value: String) -> Bool {
        value.count == 32
            && value.unicodeScalars.allSatisfy { scalar in
                (48...57).contains(scalar.value)
                    || (97...102).contains(scalar.value)
            }
    }

    private static let formatVersion = 1
    private static let maximumGlobalEntries = 20
    private static let maximumRelationshipEntries = 10
    private static let maximumRelationshipIDBytes = 128
    private static let maximumRootNames = 3
    private static let maximumRootNameCharacters = 255
    private static let maximumErrorCharacters = 512
}

#if os(iOS)
enum RememberedRoomForgetPreparation {
    case ready(cleanupWarning: String?)
    case blocked(String)
}

enum RememberedRoomOutboxDeliveryCleanupPolicy {
    static func canFinalize(
        state: TransferActivityState?,
        activityMatches: Bool,
        senderOwnsNativeOperation: Bool
    ) -> Bool {
        state == .delivered
            && activityMatches
            && !senderOwnsNativeOperation
    }

    static func canPublishNeedsAttention(
        state: TransferActivityState?,
        activityMatches: Bool,
        senderOwnsNativeOperation: Bool
    ) -> Bool {
        (state == .failed || state == .canceled)
            && activityMatches
            && !senderOwnsNativeOperation
    }
}

/**
 * Owns the foreground handoff from durable room outbox records to the one
 * process-wide sender. ConnectionWorkflowState owns the authenticated control
 * session; this controller only claims one queued payload after that session
 * is connected.
 */
@MainActor
final class RememberedRoomOutboxController: ObservableObject {
    private struct ActiveDispatch {
        let entryID: String
        let offerID: String
        var activityID: String?
    }

    @Published private(set) var entries: [RememberedRoomOutboxEntry] = []
    @Published private(set) var errorMessage: String?

    private let store: RememberedRoomOutboxStore
    private var activeDispatch: ActiveDispatch?
    private var deferredTerminalSettlementActivityID: String?
    private var didReconcileLaunch = false

    init(store: RememberedRoomOutboxStore = .shared) {
        self.store = store
    }

    var queuedRelationshipIDs: Set<String> {
        let blocked = Set(
            entries.lazy
                .filter { $0.state == .needsAttention }
                .map(\.relationshipID)
        )
        return Set(entries.lazy.filter { $0.state == .queued }.map(\.relationshipID))
            .subtracting(blocked)
    }

    func entries(relationshipID: String) -> [RememberedRoomOutboxEntry] {
        entries.filter { $0.relationshipID == relationshipID }
    }

    func start() {
        guard !didReconcileLaunch else {
            refresh()
            return
        }
        didReconcileLaunch = true
        do {
            _ = try store.reconcileInterruptedAttempts()
            refresh()
        } catch {
            publish(error)
        }
    }

    func refresh() {
        do {
            entries = try store.entries()
            errorMessage = nil
        } catch {
            publish(error)
        }
    }

    func dispatchIfPossible(
        workflow: ConnectionWorkflowState,
        model: AppModel,
        endpoint: RoomControlEndpoint,
        concurrentTransfers: Bool,
        language: String,
        candidatesAllow: String,
        candidatesDeny: String,
        speedLimit: Int
    ) {
        guard activeDispatch == nil,
              workflow.controlPhase == .connected,
              workflow.incomingRoomOffer == nil,
              let relationshipID = workflow.activeRememberedRelationshipID,
              workflow.rememberedRoom?.relationshipID == relationshipID,
              workflow.activeRoomEndpoint == endpoint,
              !model.send.isBusy,
              !entries.contains(where: {
                  $0.relationshipID == relationshipID && $0.state == .needsAttention
              }) else {
            return
        }

        let claimed: RememberedRoomOutboxEntry
        let invitation: FfiPairingInvite
        let settings: EnvoixRuntimeSettings
        do {
            guard let next = try store.claimNext(relationshipID: relationshipID),
                  let offerID = next.offerID else {
                return
            }
            claimed = next
            activeDispatch = ActiveDispatch(
                entryID: next.id,
                offerID: offerID,
                activityID: nil
            )
            refresh()
            invitation = try makePairingInvite(
                role: .send,
                broker: endpoint.broker,
                relay: endpoint.relay
            )
            settings = try RuntimeSettingsProvider.make(
                concurrentTransfers: concurrentTransfers,
                language: language,
                serverURL: endpoint.broker,
                relayURL: endpoint.relay,
                candidatesAllow: candidatesAllow,
                candidatesDeny: candidatesDeny,
                speedLimit: speedLimit
            )
        } catch {
            failClaimedDispatch(claimedID: activeDispatch?.entryID, error: error)
            return
        }

        let offer = RoomControlTransferOffer(
            id: activeDispatch?.offerID ?? UUID().uuidString.lowercased(),
            transferInvite: invitation.payload,
            rootNames: claimed.rootNames,
            itemCount: UInt32(clamping: claimed.itemCount),
            directoryCount: UInt32(clamping: claimed.directoryCount),
            totalBytes: claimed.totalBytes
        )
        workflow.offerTransfer(offer) { [weak self, weak workflow, weak model] accepted in
            guard let self, let workflow, let model else { return }
            Task { @MainActor in
                await self.resolveOfferDecision(
                    accepted: accepted,
                    entry: claimed,
                    invitation: invitation,
                    settings: settings,
                    workflow: workflow,
                    model: model
                )
            }
        }
    }

    func handleSendState(
        _ state: TransferActivityState?,
        workflow: ConnectionWorkflowState,
        model: AppModel
    ) {
        guard let dispatch = activeDispatch,
              let activityID = dispatch.activityID,
              model.send.transferActivity?.activityId == activityID,
              let state else {
            return
        }
        let senderOwnsNativeOperation =
            model.send.ownsNativeSendOperation(activityID: activityID)
        if TransferPresentationPolicy.isTerminal(state), senderOwnsNativeOperation {
            deferTerminalSettlement(
                activityID: activityID,
                workflow: workflow,
                model: model
            )
            workflow.setLocalTransferActive(true)
            return
        }
        switch state {
        case .delivered:
            guard RememberedRoomOutboxDeliveryCleanupPolicy.canFinalize(
                state: state,
                activityMatches: true,
                senderOwnsNativeOperation: senderOwnsNativeOperation
            ) else {
                return
            }
            do {
                let removed = try store.remove(
                    id: dispatch.entryID,
                    expectedActivityID: activityID
                )
                activeDispatch = nil
                refresh()
                if let removed {
                    Task { [weak self] in
                        do {
                            try await model.send.discardQueuedRoomManifestArtifacts(removed)
                        } catch {
                            self?.errorMessage =
                                "Delivery finished, but some local cache files could not be cleaned: "
                                + error.localizedDescription
                        }
                    }
                }
            } catch {
                activeDispatch = nil
                publish(error)
            }
        case .failed, .canceled:
            guard RememberedRoomOutboxDeliveryCleanupPolicy.canPublishNeedsAttention(
                state: state,
                activityMatches: true,
                senderOwnsNativeOperation: senderOwnsNativeOperation
            ) else {
                return
            }
            do {
                _ = try store.markNeedsAttention(
                    id: dispatch.entryID,
                    message: model.send.statusText.isEmpty
                        ? "The transfer did not finish. Check the peer before retrying."
                        : model.send.statusText,
                    expectedActivityID: activityID
                )
                activeDispatch = nil
                refresh()
            } catch {
                activeDispatch = nil
                publish(error)
            }
        case .preparing, .pairing, .connecting, .waitingForPeer,
             .transferring, .verifying, .saving, .waitingForReceiverSave,
             .finalizingDelivery, .awaitingDecision, .paused:
            break
        }
        workflow.setLocalTransferActive(model.send.isBusy || model.receive.isBusy)
    }

    private func deferTerminalSettlement(
        activityID: String,
        workflow: ConnectionWorkflowState,
        model: AppModel
    ) {
        guard deferredTerminalSettlementActivityID == nil else { return }
        deferredTerminalSettlementActivityID = activityID
        model.send.afterNativeSendOperationRelease(activityID: activityID) { [
            weak self,
            weak workflow,
            weak model
        ] in
            guard let self else { return }
            self.deferredTerminalSettlementActivityID = nil
            guard let model,
                  let workflow,
                  self.activeDispatch?.activityID == activityID else {
                return
            }
            self.handleSendState(
                model.send.presentationState,
                workflow: workflow,
                model: model
            )
        }
    }

    func retry(id: String) {
        do {
            guard try store.retry(id: id) else { return }
            refresh()
        } catch {
            publish(error)
        }
    }

    func remove(
        _ entry: RememberedRoomOutboxEntry,
        model: AppModel
    ) async -> String? {
        guard entry.state == .queued || entry.state == .needsAttention else {
            return "Wait for the current room offer or transfer to finish."
        }
        guard !model.send.ownsNativeSendJob(jobID: entry.jobID) else {
            return "Wait for the current transfer to finish before removing it."
        }
        if let activityID = entry.activityID,
           model.send.transferActivity?.activityId == activityID {
            _ = model.send.cancel()
        }
        do {
            guard let removed = try store.remove(id: entry.id) else {
                throw RuntimeSettingsError("The queued transfer changed before it could be removed.")
            }
            refresh()
            do {
                try await model.send.discardQueuedRoomManifestArtifacts(removed)
            } catch {
                let message =
                    "The queued transfer was removed, but some local cache files could not be cleaned: "
                    + error.localizedDescription
                errorMessage = message
                return message
            }
            return nil
        } catch {
            publish(error)
            return error.localizedDescription
        }
    }

    func removeAll(
        relationshipID: String,
        model: AppModel
    ) async -> RememberedRoomForgetPreparation {
        let roomEntries: [RememberedRoomOutboxEntry]
        do {
            roomEntries = try store.entries(relationshipID: relationshipID)
        } catch {
            publish(error)
            return .blocked(error.localizedDescription)
        }
        guard roomEntries.allSatisfy({
            $0.state == .queued || $0.state == .needsAttention
        }) else {
            return .blocked(
                "Wait for the current room offer or transfer to finish before forgetting it."
            )
        }
        guard roomEntries.allSatisfy({
            !model.send.ownsNativeSendJob(jobID: $0.jobID)
        }) else {
            return .blocked(
                "Wait for the current transfer to finish before forgetting this room."
            )
        }
        var removedEntries: [RememberedRoomOutboxEntry] = []
        for entry in roomEntries {
            do {
                guard let removed = try store.remove(id: entry.id) else {
                    throw RuntimeSettingsError(
                        "A queued transfer changed before this room could be forgotten."
                    )
                }
                removedEntries.append(removed)
            } catch {
                refresh()
                publish(error)
                return .blocked(error.localizedDescription)
            }
        }
        refresh()
        var cleanupErrors: [String] = []
        for entry in removedEntries {
            do {
                try await model.send.discardQueuedRoomManifestArtifacts(entry)
            } catch {
                cleanupErrors.append(error.localizedDescription)
            }
        }
        if !cleanupErrors.isEmpty {
            let message =
                "Queued transfers were removed, but some local cache files could not be cleaned: "
                + cleanupErrors.joined(separator: "; ")
            errorMessage = message
            return .ready(cleanupWarning: message)
        }
        return .ready(cleanupWarning: nil)
    }

    private func resolveOfferDecision(
        accepted: Bool,
        entry: RememberedRoomOutboxEntry,
        invitation: FfiPairingInvite,
        settings: EnvoixRuntimeSettings,
        workflow: ConnectionWorkflowState,
        model: AppModel
    ) async {
        guard let activeDispatch,
              activeDispatch.entryID == entry.id,
              activeDispatch.offerID == entry.offerID else {
            return
        }
        guard accepted else {
            do {
                _ = try store.markNeedsAttention(
                    id: entry.id,
                    message: "The peer declined the offer or the room disconnected.",
                    expectedOfferID: activeDispatch.offerID
                )
                self.activeDispatch = nil
                refresh()
            } catch {
                self.activeDispatch = nil
                publish(error)
            }
            return
        }

        do {
            let activityID = try await model.send.startQueuedRoomManifest(
                entry,
                roomCode: invitation.roomCode,
                settings: settings
            )
            guard var current = self.activeDispatch,
                  current.entryID == entry.id,
                  current.offerID == activeDispatch.offerID,
                  try store.markTransferring(
                      id: entry.id,
                      offerID: activeDispatch.offerID,
                      activityID: activityID
                  ) else {
                _ = model.send.cancel()
                throw RuntimeSettingsError(
                    "The queued transfer changed before the send could start."
                )
            }
            current.activityID = activityID
            self.activeDispatch = current
            workflow.captureActivity(activityID)
            workflow.setLocalTransferActive(true)
            refresh()
        } catch {
            failClaimedDispatch(claimedID: entry.id, error: error)
        }
    }

    private func failClaimedDispatch(claimedID: String?, error: Error) {
        guard let claimedID,
              let dispatch = activeDispatch,
              dispatch.entryID == claimedID else {
            publish(error)
            return
        }
        do {
            _ = try store.markNeedsAttention(
                id: claimedID,
                message: error.localizedDescription,
                expectedOfferID: dispatch.offerID
            )
        } catch {
            errorMessage = error.localizedDescription
        }
        activeDispatch = nil
        refresh()
    }

    private func publish(_ error: Error) {
        errorMessage = error.localizedDescription
    }
}
#endif
