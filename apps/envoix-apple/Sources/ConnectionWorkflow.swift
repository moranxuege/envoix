#if os(iOS) || os(macOS)
import Combine
import EnvoixCore
import Foundation

enum OneTimeRoomAction: Equatable {
    case offerFiles
    case receiveFiles
    case choose
}

enum OneTimeRoomOrigin: Equatable {
    case nearby(NearbyPairingSelection)
    case roomControl
    case pairingCode
    case showCode
}

struct OneTimeRoomSession: Equatable, Identifiable {
    let id: UUID
    var origin: OneTimeRoomOrigin
    var pairingInput: String?
    var suggestedAction: OneTimeRoomAction
    let endpoint: RoomControlEndpoint?
    var nearbySelection: NearbyPairingSelection?
    let baselineActivityIDs: Set<String>
    var activityIDs: Set<String>

    init(
        id: UUID = UUID(),
        origin: OneTimeRoomOrigin,
        pairingInput: String? = nil,
        suggestedAction: OneTimeRoomAction = .choose,
        endpoint: RoomControlEndpoint? = nil,
        nearbySelection: NearbyPairingSelection? = nil,
        baselineActivityIDs: Set<String>
    ) {
        self.id = id
        self.origin = origin
        self.pairingInput = pairingInput
        self.suggestedAction = suggestedAction
        self.endpoint = endpoint
        if let nearbySelection {
            self.nearbySelection = nearbySelection
        } else if case .nearby(let selection) = origin {
            self.nearbySelection = selection
        } else {
            self.nearbySelection = nil
        }
        self.baselineActivityIDs = baselineActivityIDs
        self.activityIDs = []
    }
}

struct RememberedRoomSession: Equatable, Identifiable {
    let id: UUID
    let relationshipID: String
    var label: String
    let endpoint: RoomControlEndpoint
    let baselineActivityIDs: Set<String>
    var activityIDs: Set<String>

    init(peer: RememberedPeerSummary, baselineActivityIDs: Set<String>) {
        id = UUID(uuidString: peer.relationshipID) ?? UUID()
        relationshipID = peer.relationshipID
        label = peer.label
        endpoint = RoomControlEndpoint(broker: peer.broker, relay: peer.relay)
        self.baselineActivityIDs = baselineActivityIDs
        activityIDs = []
    }
}

enum RememberedRoomConnectionStatus: Equatable {
    case offline
    case connecting
    case waiting
    case connected
    case needsRepair(String)
}

struct RememberedRoomReconnectPolicy: Equatable {
    let connectorAttemptTimeout: TimeInterval
    let responderAttemptTimeout: TimeInterval
    let sameLocatorCooldown: TimeInterval
    let minimumBackoff: TimeInterval
    let maximumBackoff: TimeInterval
    let passiveConnectedDwell: TimeInterval

    static let live = RememberedRoomReconnectPolicy(
        connectorAttemptTimeout: 75,
        responderAttemptTimeout: 240,
        sameLocatorCooldown: 6,
        minimumBackoff: 30,
        maximumBackoff: 300,
        passiveConnectedDwell: 45
    )

    func timeout(for mode: RememberedRoomConnectMode) -> TimeInterval {
        mode == .connector ? connectorAttemptTimeout : responderAttemptTimeout
    }

    func delay(failureCount: Int, jitterUnit: Double) -> TimeInterval {
        let exponent = min(max(0, failureCount - 1), 6)
        let base = min(maximumBackoff, minimumBackoff * pow(2, Double(exponent)))
        return min(maximumBackoff, base + base * 0.35 * min(max(jitterUnit, 0), 1))
    }

    func collisionDelay(jitterUnit: Double) -> TimeInterval {
        1 + 5 * min(max(jitterUnit, 0), 1)
    }

    func requiredCooldown(
        failureCode: FfiFailureCode?,
        retryAfterSeconds: UInt64?
    ) -> TimeInterval? {
        if failureCode == .roomExpired {
            return 300
        }
        return retryAfterSeconds.map {
            min(TimeInterval($0), maximumBackoff)
        }
    }
}

struct PendingNearbyInvitation: Equatable, Identifiable {
    let offer: NearbyRendezvousOffer
    let receivedAt: Date

    var id: String { offer.id }
    var deduplicationKey: String {
        "\(offer.senderPeerKey)\u{0}\(offer.invite)"
    }
}

enum ConnectionWorkflowPolicy {
    static let maximumPendingOffers = 4
    static let offerLifetime: TimeInterval = 30
    static let roomOfferLifetime: TimeInterval = 60
    static let roomInvitationLifetime: TimeInterval = 5 * 60
    static let roomIdleLifetime: TimeInterval = 15 * 60

    static func localAction(forLocalRole role: FfiInviteRole) -> OneTimeRoomAction {
        switch role {
        case .send: return .offerFiles
        case .receive: return .receiveFiles
        }
    }

    enum PendingSharedSendDestination: Equatable {
        case none
        case connectionHub
        case oneTimeRoom
        case rememberedRoom
    }

    static func pendingSharedSendDestination(
        hasPendingSelection: Bool,
        sendIsBusy: Bool,
        transferIsPresented: Bool,
        selectionWasPresented: Bool,
        hasConnectedOneTimeRoom: Bool,
        hasConnectedRememberedRoom: Bool
    ) -> PendingSharedSendDestination {
        guard hasPendingSelection,
              !sendIsBusy,
              !transferIsPresented,
              !selectionWasPresented else {
            return .none
        }
        if hasConnectedRememberedRoom {
            return .rememberedRoom
        }
        if hasConnectedOneTimeRoom {
            return .oneTimeRoom
        }
        return .connectionHub
    }

    static func isExpired(_ offer: PendingNearbyInvitation, now: Date) -> Bool {
        now.timeIntervalSince(offer.receivedAt) >= offerLifetime
    }

    static func rememberedGenerationSchedule(
        current: UInt64,
        previous: UInt64?,
        mode: RememberedRoomConnectMode
    ) -> [UInt64] {
        _ = mode
        guard let previous, previous != current else { return [current] }
        return [current, previous]
    }

    static func nextRememberedMode(
        after mode: RememberedRoomConnectMode,
        jitterUnit: Double
    ) -> RememberedRoomConnectMode {
        guard jitterUnit < 0.75 else { return mode }
        return mode == .connector ? .responder : .connector
    }
}

enum RoomOfferAcceptanceResult: Equatable {
    case accepted(activityID: String)
    case receiverDidNotStart
    case offerUnavailable(activityID: String)
}

/// Keeps the control-channel acknowledgement behind receiver startup. The
/// sender must not begin until the receiving transfer owns an activity and is
/// waiting for its peer.
@MainActor
enum RoomOfferAcceptanceCoordinator {
    static func startReceiverThenAccept(
        startReceiver: () async -> String?,
        acceptOffer: () async -> Bool,
        cancelReceiver: (String) -> Void
    ) async -> RoomOfferAcceptanceResult {
        guard let activityID = await startReceiver() else {
            return .receiverDidNotStart
        }
        guard await acceptOffer() else {
            cancelReceiver(activityID)
            return .offerUnavailable(activityID: activityID)
        }
        return .accepted(activityID: activityID)
    }
}

enum RoomControlPhase: Equatable {
    case idle
    case hosting
    case joining
    case connectingRemembered
    case waitingRemembered
    case connected
    case ended(RoomControlCloseReason)
    case failed(String)
}

@MainActor
final class ConnectionWorkflowState: ObservableObject {
    @Published private(set) var room: OneTimeRoomSession?
    @Published private(set) var rememberedRoom: RememberedRoomSession?
    @Published private(set) var rememberedPeers: [RememberedPeerSummary] = []
    @Published private(set) var activeRememberedRelationshipID: String?
    @Published private(set) var rememberedRoomErrors: [String: String] = [:]
    @Published private(set) var pendingOffers: [PendingNearbyInvitation] = []
    @Published private(set) var controlPhase: RoomControlPhase = .idle
    @Published private(set) var roomInvitation: RoomControlInvitation?
    @Published private(set) var peerDisplayName: String?
    @Published private(set) var incomingRoomOffer: RoomControlTransferOffer?
    @Published private(set) var roomLifetimePolicy: RoomControlLifetimePolicy = .idleFifteenMinutes
    @Published private(set) var idleDeadline: Date?
    @Published private(set) var isRoomCreator = false

    private let gateway: RoomControlGateway?
    private let rememberedStore: RememberedPeerStore
    private let reconnectPolicy: RememberedRoomReconnectPolicy
    private let clock: () -> Date
    private let jitterUnit: () -> Double
    private var controlTask: Task<Void, Never>?
    private var rememberedReconnectTask: Task<Void, Never>?
    private var rememberedReconnectTaskID: UUID?
    private var controlGeneration = 0
    private var baselineActivityIDs: Set<String> = []
    private var pendingControlNearbySelection: NearbyPairingSelection?
    private var pendingDeviceVerification = false
    private var outgoingDecisions: [String: (Bool) -> Void] = [:]
    private var incomingRoomOfferDeadline: Date?
    private var lifetimeRevision: UInt64?
    private var requestedLocalTransferActive: Bool?
    private var reportedLocalTransferActive: Bool?
    private var localTransferSyncTask: Task<Void, Never>?
    private var idleExpiryTask: Task<Void, Never>?
    private var passiveRememberedDwellTask: Task<Void, Never>?
    private var idleExpiryRevision: UInt64?
    private var deferredControlFailure: String?
    private var rememberedReconnectEnabled = false
    private var rememberedDisplayName = ""
    private var rememberedIdentityPath = ""
    private var selectedRememberedRelationshipID: String?
    private var leasedRememberedRelationshipID: String?
    private var rememberedFailureCounts: [String: Int] = [:]
    private var nextRememberedMode: [String: RememberedRoomConnectMode] = [:]
    private var rememberedCredentialReferences: [String: String] = [:]
    private var rememberedRetryNotBefore: [String: Date] = [:]
    private var blockedRememberedRelationships = Set<String>()
    private var suppressedRememberedRelationships = Set<String>()
    private var queuedRememberedRelationships = Set<String>()
    private var passiveRememberedCursor = 0

    init(
        gateway: RoomControlGateway? = nil,
        rememberedStore: RememberedPeerStore = .shared,
        reconnectPolicy: RememberedRoomReconnectPolicy = .live,
        jitterUnit: @escaping () -> Double = { Double.random(in: 0...1) },
        clock: @escaping () -> Date = Date.init
    ) {
        self.gateway = gateway
        self.rememberedStore = rememberedStore
        self.reconnectPolicy = reconnectPolicy
        self.jitterUnit = jitterUnit
        self.clock = clock
    }

    deinit {
        controlTask?.cancel()
        rememberedReconnectTask?.cancel()
        localTransferSyncTask?.cancel()
        idleExpiryTask?.cancel()
        passiveRememberedDwellTask?.cancel()
        if let leasedRememberedRelationshipID {
            rememberedStore.releaseSession(leasedRememberedRelationshipID)
        }
    }

    var nextPendingOffer: PendingNearbyInvitation? {
        pendingOffers.first
    }

    var activeRoomID: UUID? {
        rememberedRoom?.id ?? room?.id
    }

    var activeRoomEndpoint: RoomControlEndpoint? {
        rememberedRoom?.endpoint ?? room?.endpoint
    }

    var hasPinnedRememberedRoom: Bool {
        guard let selectedRememberedRelationshipID else { return false }
        return rememberedRoom?.relationshipID == selectedRememberedRelationshipID
    }

    func rememberedRoomStatus(
        relationshipID: String
    ) -> RememberedRoomConnectionStatus {
        if let message = rememberedRoomErrors[relationshipID],
           blockedRememberedRelationships.contains(relationshipID) {
            return .needsRepair(message)
        }
        guard activeRememberedRelationshipID == relationshipID else {
            return .offline
        }
        switch controlPhase {
        case .connectingRemembered: return .connecting
        case .waitingRemembered: return .waiting
        case .connected: return .connected
        default: return .offline
        }
    }

    func refreshRememberedRooms() {
        do {
            rememberedPeers = try rememberedStore.peers()
            let known = Set(rememberedPeers.map(\.relationshipID))
            rememberedRoomErrors = rememberedRoomErrors.filter { known.contains($0.key) }
            blockedRememberedRelationships.formIntersection(known)
            suppressedRememberedRelationships.formIntersection(known)
            queuedRememberedRelationships.formIntersection(known)
            nextRememberedMode = nextRememberedMode.filter { known.contains($0.key) }
            rememberedCredentialReferences = rememberedCredentialReferences.filter {
                known.contains($0.key)
            }
            rememberedFailureCounts = rememberedFailureCounts.filter { known.contains($0.key) }
            rememberedRetryNotBefore = rememberedRetryNotBefore.filter {
                known.contains($0.key)
            }
            if let rememberedRoom,
               let refreshed = rememberedPeers.first(where: {
                   $0.relationshipID == rememberedRoom.relationshipID
               }) {
                var updated = rememberedRoom
                updated.label = refreshed.label
                self.rememberedRoom = updated
            }
        } catch {
            // Preserve the last good index on transient protected-storage errors.
        }
    }

    /**
     * Gives durable local work priority over passive availability probes.
     * Connector/responder remain hidden transport roles and may alternate after
     * a collision; this only chooses the first attempt for newly queued work.
     */
    func setQueuedRememberedRelationships(_ relationshipIDs: Set<String>) {
        let known = Set(rememberedPeers.map(\.relationshipID))
        let updated = relationshipIDs.intersection(known)
        let newlyQueued = updated.subtracting(queuedRememberedRelationships)
        queuedRememberedRelationships = updated
        for relationshipID in newlyQueued {
            suppressedRememberedRelationships.remove(relationshipID)
            nextRememberedMode[relationshipID] = .connector
            rememberedFailureCounts[relationshipID] = 0
            rememberedRetryNotBefore.removeValue(forKey: relationshipID)
        }
        guard !newlyQueued.isEmpty else {
            schedulePassiveRememberedDwellIfNeeded()
            scheduleRememberedReconnect()
            return
        }
        if let activeRelationshipID = activeRememberedRelationshipID,
           controlPhase == .connected,
           selectedRememberedRelationshipID != activeRelationshipID,
           !updated.contains(activeRelationshipID),
           incomingRoomOffer == nil,
           outgoingDecisions.isEmpty,
           requestedLocalTransferActive != true,
           reportedLocalTransferActive != true {
            stopRememberedControl(reason: .idleExpired, keepSelection: false)
            scheduleRememberedReconnect()
            return
        }
        let shouldPreemptPassiveAttempt = controlPhase != .connected
            && rememberedReconnectTask != nil
            && (activeRememberedRelationshipID.map {
                !queuedRememberedRelationships.contains($0)
            } ?? true)
        scheduleRememberedReconnect(restart: shouldPreemptPassiveAttempt)
    }

    func setRememberedReconnectEnabled(
        _ enabled: Bool,
        displayName: String,
        identityPath: String
    ) {
        rememberedDisplayName = displayName
        rememberedIdentityPath = identityPath
        guard enabled != rememberedReconnectEnabled else {
            if enabled {
                refreshRememberedRooms()
                scheduleRememberedReconnect()
            }
            return
        }
        rememberedReconnectEnabled = enabled
        if enabled {
            suppressedRememberedRelationships.removeAll()
            if let selectedRememberedRelationshipID {
                nextRememberedMode[selectedRememberedRelationshipID] = .connector
                rememberedFailureCounts[selectedRememberedRelationshipID] = 0
            }
            refreshRememberedRooms()
            scheduleRememberedReconnect()
        } else {
            stopRememberedControl(reason: .backgrounded, keepSelection: true)
        }
    }

    @discardableResult
    func openRememberedRoom(
        relationshipID: String,
        existingActivityIDs: Set<String>
    ) -> String? {
        refreshRememberedRooms()
        guard let peer = rememberedPeers.first(where: {
            $0.relationshipID == relationshipID
        }) else {
            return "This remembered room is no longer available."
        }
        guard room == nil, roomInvitation == nil else {
            return "End the current one-time room before opening this room."
        }
        if activeRememberedRelationshipID == relationshipID,
           controlPhase == .connected {
            selectedRememberedRelationshipID = relationshipID
            cancelPassiveRememberedDwell()
            if rememberedRoom == nil {
                rememberedRoom = RememberedRoomSession(
                    peer: peer,
                    baselineActivityIDs: existingActivityIDs
                )
            }
            return nil
        }
        if activeRememberedRelationshipID != nil,
           activeRememberedRelationshipID != relationshipID {
            stopRememberedControl(reason: .userEnded, keepSelection: false)
        } else if rememberedReconnectTask != nil,
                  activeRememberedRelationshipID != relationshipID {
            stopRememberedControl(reason: .userEnded, keepSelection: false)
        }
        selectedRememberedRelationshipID = relationshipID
        rememberedRoom = RememberedRoomSession(
            peer: peer,
            baselineActivityIDs: existingActivityIDs
        )
        suppressedRememberedRelationships.remove(relationshipID)
        nextRememberedMode[relationshipID] = .connector
        rememberedFailureCounts[relationshipID] = 0
        scheduleRememberedReconnect(restart: true)
        return nil
    }

    func disconnectRememberedRoom() {
        guard let relationshipID = rememberedRoom?.relationshipID
                ?? activeRememberedRelationshipID else {
            return
        }
        suppressedRememberedRelationships.insert(relationshipID)
        stopRememberedControl(reason: .userEnded, keepSelection: true)
    }

    func unpinRememberedRoom() {
        guard let relationshipID = rememberedRoom?.relationshipID,
              selectedRememberedRelationshipID == relationshipID else {
            return
        }
        selectedRememberedRelationshipID = nil
        schedulePassiveRememberedDwellIfNeeded()
    }

    @discardableResult
    func forgetRememberedRoom(relationshipID: String) async -> String? {
        var cancelledTask: Task<Void, Never>?
        if activeRememberedRelationshipID == relationshipID
            || rememberedRoom?.relationshipID == relationshipID {
            suppressedRememberedRelationships.insert(relationshipID)
            cancelledTask = rememberedReconnectTask
            stopRememberedControl(reason: .userEnded, keepSelection: false)
        }
        await cancelledTask?.value
        guard let peer = rememberedPeers.first(where: {
            $0.relationshipID == relationshipID
        }) else {
            return nil
        }
        do {
            try rememberedStore.delete(peer)
            if selectedRememberedRelationshipID == relationshipID {
                selectedRememberedRelationshipID = nil
                rememberedRoom = nil
            }
            rememberedRoomErrors.removeValue(forKey: relationshipID)
            blockedRememberedRelationships.remove(relationshipID)
            suppressedRememberedRelationships.remove(relationshipID)
            nextRememberedMode.removeValue(forKey: relationshipID)
            rememberedCredentialReferences.removeValue(forKey: relationshipID)
            rememberedFailureCounts.removeValue(forKey: relationshipID)
            rememberedRetryNotBefore.removeValue(forKey: relationshipID)
            refreshRememberedRooms()
            scheduleRememberedReconnect()
            return nil
        } catch {
            return error.localizedDescription
        }
    }

    func openRoom(
        origin: OneTimeRoomOrigin,
        pairingInput: String? = nil,
        suggestedAction: OneTimeRoomAction = .choose,
        existingActivityIDs: Set<String>
    ) {
        stopRememberedControl(reason: .userEnded, keepSelection: false)
        if controlPhase != .idle {
            gateway?.close(reason: .userEnded)
            endLocalState()
        }
        room = OneTimeRoomSession(
            origin: origin,
            pairingInput: pairingInput,
            suggestedAction: suggestedAction,
            baselineActivityIDs: existingActivityIDs
        )
    }

    func acceptNearbyOffer(
        selection: NearbyPairingSelection,
        pairingInput: String,
        suggestedAction: OneTimeRoomAction,
        existingActivityIDs: Set<String>
    ) {
        if var current = room,
           current.nearbySelection?.discoveryPeerKey == selection.discoveryPeerKey {
            current.origin = .nearby(selection)
            current.nearbySelection = selection
            current.pairingInput = pairingInput
            current.suggestedAction = suggestedAction
            room = current
            return
        }
        openRoom(
            origin: .nearby(selection),
            pairingInput: pairingInput,
            suggestedAction: suggestedAction,
            existingActivityIDs: existingActivityIDs
        )
    }

    func closeRoom() {
        room = nil
        scheduleRememberedReconnect()
    }

    private func scheduleRememberedReconnect(restart: Bool = false) {
        guard rememberedReconnectEnabled,
              gateway != nil,
              !rememberedIdentityPath.trimmed.isEmpty,
              room == nil,
              roomInvitation == nil,
              controlTask == nil else {
            return
        }
        if restart {
            cancelRememberedAttempt(reason: .userEnded)
        }
        guard rememberedReconnectTask == nil else { return }
        refreshRememberedRooms()
        guard rememberedPeers.contains(where: {
            !blockedRememberedRelationships.contains($0.relationshipID)
                && !suppressedRememberedRelationships.contains($0.relationshipID)
        }) else { return }

        let taskID = UUID()
        rememberedReconnectTaskID = taskID
        rememberedReconnectTask = Task { @MainActor [weak self] in
            guard let self else { return }
            await runRememberedReconnectLoop()
            guard rememberedReconnectTaskID == taskID else { return }
            rememberedReconnectTask = nil
            rememberedReconnectTaskID = nil
            if rememberedReconnectEnabled {
                scheduleRememberedReconnect()
            }
        }
    }

    private func runRememberedReconnectLoop() async {
        while rememberedReconnectEnabled,
              room == nil,
              roomInvitation == nil,
              !Task.isCancelled {
            refreshRememberedRooms()
            guard let peer = nextRememberedCandidate(now: clock()) else {
                guard let retryAt = earliestRememberedRetryDate() else { return }
                let delay = max(0, retryAt.timeIntervalSince(clock()))
                if delay > 0 {
                    try? await Task.sleep(
                        nanoseconds: UInt64(delay * 1_000_000_000)
                    )
                }
                continue
            }
            let relationshipID = peer.relationshipID
            let mode = nextRememberedMode[relationshipID]
                ?? (relationshipID == selectedRememberedRelationshipID
                    || queuedRememberedRelationships.contains(relationshipID)
                    ? .connector
                    : .responder)
            let generation = controlGeneration
            let outcome = await connectRememberedPeer(
                peer,
                mode: mode,
                controlGeneration: generation
            )
            guard rememberedReconnectEnabled,
                  controlGeneration == generation,
                  !Task.isCancelled else {
                return
            }
            switch outcome {
            case .sessionEnded:
                return
            case .authenticatedFailure(let message):
                blockedRememberedRelationships.insert(relationshipID)
                rememberedRoomErrors[relationshipID] = message
                activeRememberedRelationshipID = nil
                if selectedRememberedRelationshipID == relationshipID {
                    controlPhase = .failed(message)
                } else if rememberedRoom == nil {
                    controlPhase = .idle
                }
            case .preAuthenticationFailure(let requiredCooldown):
                let failureCount = (rememberedFailureCounts[relationshipID] ?? 0) + 1
                rememberedFailureCounts[relationshipID] = failureCount
                nextRememberedMode[relationshipID] =
                    selectedRememberedRelationshipID == relationshipID
                    ? ConnectionWorkflowPolicy.nextRememberedMode(
                        after: mode,
                        jitterUnit: jitterUnit()
                    )
                    : .responder
                activeRememberedRelationshipID = nil
                if selectedRememberedRelationshipID == relationshipID {
                    controlPhase = .ended(.networkLost)
                } else if rememberedRoom == nil {
                    controlPhase = .idle
                }
                let scheduledDelay = failureCount == 1
                    && selectedRememberedRelationshipID == relationshipID
                    ? reconnectPolicy.collisionDelay(jitterUnit: jitterUnit())
                    : reconnectPolicy.delay(
                        failureCount: failureCount,
                        jitterUnit: jitterUnit()
                    )
                let delay = max(
                    scheduledDelay,
                    max(
                        reconnectPolicy.sameLocatorCooldown,
                        requiredCooldown ?? 0
                    )
                )
                rememberedRetryNotBefore[relationshipID] = clock()
                    .addingTimeInterval(delay)
            }
        }
    }

    private enum RememberedConnectOutcome {
        case sessionEnded
        case preAuthenticationFailure(requiredCooldown: TimeInterval?)
        case authenticatedFailure(String)
    }

    private func connectRememberedPeer(
        _ peer: RememberedPeerSummary,
        mode: RememberedRoomConnectMode,
        controlGeneration generation: Int
    ) async -> RememberedConnectOutcome {
        guard let gateway else {
            return .authenticatedFailure("Room control is unavailable in this build.")
        }
        do {
            try rememberedStore.acquireSession(peer.relationshipID)
            leasedRememberedRelationshipID = peer.relationshipID
        } catch {
            return .preAuthenticationFailure(requiredCooldown: nil)
        }
        defer { releaseRememberedLease(peer.relationshipID) }

        let material: RememberedPeerSessionMaterial
        let credentialReference: String
        do {
            material = try rememberedStore.sessionMaterial(
                relationshipID: peer.relationshipID
            )
            if let existing = rememberedCredentialReferences[peer.relationshipID] {
                credentialReference = existing
            } else {
                credentialReference = try registerProtectedRememberedCredential(
                    opaqueCredential: material.opaqueCredential
                )
                rememberedCredentialReferences[peer.relationshipID] = credentialReference
            }
        } catch RememberedPeerStoreError.keychain {
            return .preAuthenticationFailure(
                requiredCooldown: reconnectPolicy.minimumBackoff
            )
        } catch {
            return .authenticatedFailure(error.localizedDescription)
        }

        activeRememberedRelationshipID = peer.relationshipID
        controlPhase = mode == .connector
            ? .connectingRemembered
            : .waitingRemembered
        isRoomCreator = false
        roomLifetimePolicy = .untilForegroundEnds
        idleDeadline = nil
        baselineActivityIDs = rememberedRoom?.relationshipID == peer.relationshipID
            ? rememberedRoom?.baselineActivityIDs ?? []
            : []

        let generations = ConnectionWorkflowPolicy.rememberedGenerationSchedule(
            current: material.summary.generation,
            previous: material.summary.previousGeneration,
            mode: mode
        )
        var lastPreAuthenticationFailure = false
        var sweepCooldown: TimeInterval?
        for attemptedGeneration in generations {
            guard controlGeneration == generation,
                  rememberedReconnectEnabled,
                  !Task.isCancelled else {
                return .sessionEnded
            }
            do {
                try await gateway.connectRemembered(
                    attempt: RememberedRoomConnectAttempt(
                        credentialReference: credentialReference,
                        generation: attemptedGeneration,
                        endpoint: RoomControlEndpoint(
                            broker: material.summary.broker,
                            relay: material.summary.relay
                        ),
                        displayName: rememberedDisplayName,
                        identityPath: rememberedIdentityPath
                    ),
                    mode: mode,
                    timeout: reconnectPolicy.timeout(for: mode),
                    beforeConnected: { [rememberedStore] authenticatedGeneration in
                        guard authenticatedGeneration < UInt64.max else {
                            throw RuntimeSettingsError(
                                "Remembered-room credential generation is exhausted."
                            )
                        }
                        try rememberedStore.rotate(
                            relationshipID: peer.relationshipID,
                            opaqueCredential: material.opaqueCredential,
                            generation: authenticatedGeneration + 1
                        )
                    },
                    onEvent: { [weak self] event in
                        self?.handleRemembered(
                            event,
                            peer: material.summary,
                            generation: generation
                        )
                    }
                )
                return .sessionEnded
            } catch is CancellationError {
                return .sessionEnded
            } catch let failure as RememberedRoomConnectFailure {
                if failure.peerAuthenticated {
                    return .authenticatedFailure(failure.reason)
                }
                if let requiredCooldown = reconnectPolicy.requiredCooldown(
                    failureCode: failure.failureCode,
                    retryAfterSeconds: failure.retryAfterSeconds
                ) {
                    sweepCooldown = max(sweepCooldown ?? 0, requiredCooldown)
                    if failure.failureCode == .roomExpired {
                        lastPreAuthenticationFailure = true
                        continue
                    }
                    return .preAuthenticationFailure(
                        requiredCooldown: sweepCooldown
                    )
                }
                lastPreAuthenticationFailure = true
            } catch {
                return .authenticatedFailure(error.localizedDescription)
            }
        }
        return lastPreAuthenticationFailure
            ? .preAuthenticationFailure(requiredCooldown: sweepCooldown)
            : .authenticatedFailure("Remembered-room authentication did not start.")
    }

    private func nextRememberedCandidate(
        now: Date
    ) -> RememberedPeerSummary? {
        let candidates = rememberedPeers.filter {
            !blockedRememberedRelationships.contains($0.relationshipID)
                && !suppressedRememberedRelationships.contains($0.relationshipID)
                && (rememberedRetryNotBefore[$0.relationshipID] ?? .distantPast) <= now
        }
        guard !candidates.isEmpty else { return nil }
        if let selectedRememberedRelationshipID,
           let selected = candidates.first(where: {
               $0.relationshipID == selectedRememberedRelationshipID
           }) {
            return selected
        }
        if let queued = candidates.first(where: {
            queuedRememberedRelationships.contains($0.relationshipID)
        }) {
            return queued
        }
        let index = passiveRememberedCursor % candidates.count
        passiveRememberedCursor = (index + 1) % candidates.count
        return candidates[index]
    }

    private func earliestRememberedRetryDate() -> Date? {
        rememberedPeers.compactMap { peer in
            guard !blockedRememberedRelationships.contains(peer.relationshipID),
                  !suppressedRememberedRelationships.contains(peer.relationshipID) else {
                return nil
            }
            return rememberedRetryNotBefore[peer.relationshipID]
        }.min()
    }

    private func cancelRememberedAttempt(reason: RoomControlCloseReason) {
        guard rememberedReconnectTask != nil
                || activeRememberedRelationshipID != nil else {
            return
        }
        cancelPassiveRememberedDwell()
        controlGeneration &+= 1
        rememberedReconnectTask?.cancel()
        gateway?.close(reason: reason)
        if rememberedReconnectTask == nil {
            releaseRememberedLease()
        }
        activeRememberedRelationshipID = nil
    }

    private func stopRememberedControl(
        reason: RoomControlCloseReason,
        keepSelection: Bool
    ) {
        let wasRemembered = rememberedReconnectTask != nil
            || activeRememberedRelationshipID != nil
            || rememberedRoom != nil
        guard wasRemembered else { return }
        cancelRememberedAttempt(reason: reason)
        cancelPassiveRememberedDwell()
        localTransferSyncTask?.cancel()
        localTransferSyncTask = nil
        idleExpiryTask?.cancel()
        idleExpiryTask = nil
        idleExpiryRevision = nil
        outgoingDecisions.values.forEach { $0(false) }
        outgoingDecisions.removeAll()
        incomingRoomOffer = nil
        incomingRoomOfferDeadline = nil
        peerDisplayName = nil
        idleDeadline = nil
        lifetimeRevision = nil
        requestedLocalTransferActive = nil
        reportedLocalTransferActive = nil
        isRoomCreator = false
        controlPhase = .ended(reason)
        if !keepSelection {
            rememberedRoom = nil
            selectedRememberedRelationshipID = nil
        }
    }

    private func cancelPassiveRememberedDwell() {
        passiveRememberedDwellTask?.cancel()
        passiveRememberedDwellTask = nil
    }

    private func schedulePassiveRememberedDwellIfNeeded() {
        cancelPassiveRememberedDwell()
        guard let relationshipID = activeRememberedRelationshipID,
              controlPhase == .connected,
              selectedRememberedRelationshipID != relationshipID,
              !queuedRememberedRelationships.contains(relationshipID),
              incomingRoomOffer == nil,
              outgoingDecisions.isEmpty,
              requestedLocalTransferActive != true else {
            return
        }
        let generation = controlGeneration
        let delay = reconnectPolicy.passiveConnectedDwell
        passiveRememberedDwellTask = Task { @MainActor [weak self] in
            guard delay > 0 else { return }
            do {
                try await Task.sleep(
                    nanoseconds: UInt64(delay * 1_000_000_000)
                )
            } catch {
                return
            }
            guard let self,
                  controlGeneration == generation,
                  activeRememberedRelationshipID == relationshipID,
                  selectedRememberedRelationshipID != relationshipID,
                  !queuedRememberedRelationships.contains(relationshipID),
                  incomingRoomOffer == nil,
                  outgoingDecisions.isEmpty,
                  requestedLocalTransferActive != true else {
                return
            }
            passiveRememberedDwellTask = nil
            stopRememberedControl(reason: .idleExpired, keepSelection: false)
        }
    }

    private func releaseRememberedLease(_ relationshipID: String? = nil) {
        guard let leasedRememberedRelationshipID,
              relationshipID == nil
                || relationshipID == leasedRememberedRelationshipID else {
            return
        }
        rememberedStore.releaseSession(leasedRememberedRelationshipID)
        self.leasedRememberedRelationshipID = nil
    }

    @discardableResult
    func startHosting(
        broker: String,
        relay: String,
        displayName: String,
        identityPath: String,
        existingActivityIDs: Set<String>,
        nearbySelection: NearbyPairingSelection? = nil,
        invitationInput: String? = nil,
        verifiedPeerLabel: String? = nil
    ) -> String? {
        guard let gateway else {
            return "Room control is unavailable in this build."
        }
        do {
            let now = clock()
            let invitation: RoomControlInvitation
            if let invitationInput {
                invitation = try gateway.parseInvitation(
                    invitationInput,
                    broker: broker,
                    relay: relay,
                    now: now
                )
            } else {
                invitation = try gateway.makeInvitation(
                    broker: broker,
                    relay: relay,
                    now: now
                )
            }
            // Generate the replacement before touching the active host. A
            // refresh failure must not invalidate a room code that still
            // works on the other device.
            gateway.close(reason: .userEnded)
            endLocalState()
            if let verifiedPeerLabel {
                try gateway.prepareDeviceVerification(
                    label: verifiedPeerLabel,
                    endpoint: invitation.endpoint
                )
            }
            pendingDeviceVerification = verifiedPeerLabel != nil
            roomInvitation = invitation
            controlPhase = .hosting
            isRoomCreator = true
            baselineActivityIDs = existingActivityIDs
            pendingControlNearbySelection = nearbySelection
            let generation = controlGeneration
            controlTask = Task { @MainActor [weak self] in
                guard let self else { return }
                do {
                    try await gateway.host(
                        invitation: invitation,
                        displayName: displayName,
                        identityPath: identityPath,
                        onEvent: { [weak self] event in
                            Task { @MainActor in
                                self?.handle(event, generation: generation)
                            }
                        }
                    )
                } catch where Task.isCancelled {
                    return
                } catch {
                    self.handleControlFailure(
                        error.localizedDescription,
                        generation: generation
                    )
                }
            }
            return nil
        } catch {
            return error.localizedDescription
        }
    }

    @discardableResult
    func joinRoomControl(
        input: String,
        broker: String,
        relay: String,
        displayName: String,
        identityPath: String,
        existingActivityIDs: Set<String>,
        nearbySelection: NearbyPairingSelection? = nil,
        verifiedPeerLabel: String? = nil
    ) -> String? {
        guard let gateway else {
            return "Room control is unavailable in this build."
        }
        gateway.close(reason: .userEnded)
        endLocalState()
        do {
            let invitation = try gateway.parseInvitation(
                input,
                broker: broker,
                relay: relay,
                now: clock()
            )
            if let verifiedPeerLabel {
                try gateway.prepareDeviceVerification(
                    label: verifiedPeerLabel,
                    endpoint: invitation.endpoint
                )
            }
            pendingDeviceVerification = verifiedPeerLabel != nil
            roomInvitation = invitation
            controlPhase = .joining
            isRoomCreator = false
            baselineActivityIDs = existingActivityIDs
            pendingControlNearbySelection = nearbySelection
            let generation = controlGeneration
            controlTask = Task { @MainActor [weak self] in
                guard let self else { return }
                do {
                    try await gateway.join(
                        invitation: invitation,
                        displayName: displayName,
                        identityPath: identityPath,
                        onEvent: { [weak self] event in
                            Task { @MainActor in
                                self?.handle(event, generation: generation)
                            }
                        }
                    )
                } catch where Task.isCancelled {
                    return
                } catch {
                    self.handleControlFailure(
                        error.localizedDescription,
                        generation: generation
                    )
                }
            }
            return nil
        } catch {
            failControl(error.localizedDescription)
            return error.localizedDescription
        }
    }

    func refreshHosting(
        broker: String,
        relay: String,
        displayName: String,
        identityPath: String,
        existingActivityIDs: Set<String>
    ) -> String? {
        startHosting(
            broker: broker,
            relay: relay,
            displayName: displayName,
            identityPath: identityPath,
            existingActivityIDs: existingActivityIDs
        )
    }

    func canReuseHostingInvitation(for selection: NearbyPairingSelection) -> Bool {
        guard controlPhase == .hosting,
              roomInvitation != nil,
              let scopedSelection = pendingControlNearbySelection else {
            return false
        }
        return scopedSelection == selection
    }

    func offerTransfer(
        _ offer: RoomControlTransferOffer,
        onDecision: @escaping (Bool) -> Void
    ) {
        guard controlPhase == .connected,
              incomingRoomOffer == nil,
              outgoingDecisions.isEmpty,
              let gateway else {
            onDecision(false)
            return
        }
        cancelPassiveRememberedDwell()
        outgoingDecisions[offer.id] = onDecision
        let generation = controlGeneration
        Task { @MainActor [weak self] in
            do {
                if let lifetime = try await gateway.offerTransfer(offer) {
                    guard self?.controlGeneration == generation else { return }
                    self?.applyLifetime(lifetime)
                }
            } catch {
                guard let self, controlGeneration == generation else { return }
                outgoingDecisions.removeValue(forKey: offer.id)?(false)
                handleControlFailure(error.localizedDescription, generation: generation)
            }
        }
    }

    func acceptIncomingRoomOffer() async -> RoomControlTransferOffer? {
        guard let gateway, let offer = incomingRoomOffer else { return nil }
        // Claim the decision synchronously before crossing an await boundary.
        // Otherwise the MainActor can re-enter through `tick` at the deadline
        // and race an automatic rejection against this acceptance.
        incomingRoomOffer = nil
        incomingRoomOfferDeadline = nil
        let generation = controlGeneration
        do {
            if let lifetime = try await gateway.acceptOffer(id: offer.id) {
                guard controlGeneration == generation else { return nil }
                applyLifetime(lifetime)
            }
            guard controlGeneration == generation else { return nil }
            return offer
        } catch {
            handleControlFailure(error.localizedDescription, generation: generation)
            return nil
        }
    }

    func holdIncomingRoomOfferForDestination(id: String) -> Bool {
        guard incomingRoomOffer?.id == id else { return false }
        incomingRoomOfferDeadline = nil
        return true
    }

    func resumeIncomingRoomOfferDeadline(id: String) {
        guard incomingRoomOffer?.id == id else { return }
        incomingRoomOfferDeadline = clock().addingTimeInterval(
            ConnectionWorkflowPolicy.roomOfferLifetime
        )
    }

    func rejectIncomingRoomOffer() {
        guard let gateway, let offer = incomingRoomOffer else { return }
        incomingRoomOffer = nil
        incomingRoomOfferDeadline = nil
        let generation = controlGeneration
        Task { @MainActor [weak self] in
            do {
                if let lifetime = try await gateway.rejectOffer(id: offer.id) {
                    guard self?.controlGeneration == generation else { return }
                    self?.applyLifetime(lifetime)
                }
                self?.schedulePassiveRememberedDwellIfNeeded()
            } catch {
                self?.handleControlFailure(error.localizedDescription, generation: generation)
            }
        }
    }

    func setKeepOpen(_ keepOpen: Bool) {
        guard isRoomCreator, controlPhase == .connected, let gateway else { return }
        let policy: RoomControlLifetimePolicy = keepOpen
            ? .untilForegroundEnds
            : .idleFifteenMinutes
        let generation = controlGeneration
        Task { @MainActor [weak self] in
            do {
                if let lifetime = try await gateway.setLifetimePolicy(policy) {
                    guard self?.controlGeneration == generation else { return }
                    self?.applyLifetime(lifetime)
                }
            } catch {
                self?.handleControlFailure(error.localizedDescription, generation: generation)
            }
        }
    }

    func setLocalTransferActive(_ active: Bool) {
        guard controlPhase == .connected, gateway != nil else { return }
        requestedLocalTransferActive = active
        if active {
            cancelPassiveRememberedDwell()
        } else {
            schedulePassiveRememberedDwellIfNeeded()
        }
        startLocalTransferSyncIfNeeded()
    }

    func tick(now: Date? = nil, hasActiveTransfer: Bool) {
        let now = now ?? clock()
        if incomingRoomOffer != nil,
           let incomingRoomOfferDeadline,
           now >= incomingRoomOfferDeadline {
            rejectIncomingRoomOffer()
        }
        if controlPhase == .hosting,
           let roomInvitation,
           now >= roomInvitation.expiresAt {
            endControl(reason: .invitationExpired)
            return
        }
        guard controlPhase == .connected,
              isRoomCreator,
              roomLifetimePolicy == .idleFifteenMinutes,
              !hasActiveTransfer,
              localTransferSyncTask == nil,
              incomingRoomOffer == nil,
              let idleDeadline,
              now >= idleDeadline,
              let lifetimeRevision,
              idleExpiryTask == nil else {
            return
        }
        attemptIdleExpiry(revision: lifetimeRevision)
    }

    func endControl(reason: RoomControlCloseReason) {
        if activeRememberedRelationshipID != nil || rememberedRoom != nil {
            stopRememberedControl(reason: reason, keepSelection: true)
            scheduleRememberedReconnect()
            return
        }
        finishControl(reason: reason, notifyGateway: true)
    }

    private func finishControl(
        reason: RoomControlCloseReason,
        notifyGateway: Bool
    ) {
        let keepEndedRoom = room != nil
            && reason != .userEnded
            && reason != .invitationExpired
        controlGeneration &+= 1
        controlTask?.cancel()
        controlTask = nil
        localTransferSyncTask?.cancel()
        localTransferSyncTask = nil
        idleExpiryTask?.cancel()
        idleExpiryTask = nil
        cancelPassiveRememberedDwell()
        idleExpiryRevision = nil
        deferredControlFailure = nil
        outgoingDecisions.values.forEach { $0(false) }
        outgoingDecisions.removeAll()
        incomingRoomOffer = nil
        incomingRoomOfferDeadline = nil
        roomInvitation = nil
        pendingControlNearbySelection = nil
        pendingDeviceVerification = false
        idleDeadline = nil
        lifetimeRevision = nil
        requestedLocalTransferActive = nil
        reportedLocalTransferActive = nil
        controlPhase = .ended(reason)
        if !keepEndedRoom {
            room = nil
        }
        if notifyGateway {
            gateway?.close(reason: reason)
        }
        scheduleRememberedReconnect()
    }

    private func handle(_ event: RoomControlEvent, generation: Int) {
        guard controlGeneration == generation else { return }
        switch event {
        case .connected(let name, let creator, let lifetime):
            guard controlPhase == .hosting || controlPhase == .joining else { return }
            let endpoint = roomInvitation?.endpoint
            if pendingDeviceVerification {
                refreshRememberedRooms()
                pendingDeviceVerification = false
            }
            peerDisplayName = NearbyDiscoveryPeerRegistry.sanitizeDisplayName(name)
                ?? "Nearby device"
            isRoomCreator = creator
            controlPhase = .connected
            roomInvitation = nil
            room = OneTimeRoomSession(
                origin: .roomControl,
                endpoint: endpoint,
                nearbySelection: pendingControlNearbySelection,
                baselineActivityIDs: baselineActivityIDs
            )
            pendingControlNearbySelection = nil
            pendingDeviceVerification = false
            applyLifetime(lifetime)
        case .incomingOffer(let offer):
            guard controlPhase == .connected, incomingRoomOffer == nil else { return }
            incomingRoomOffer = offer
            incomingRoomOfferDeadline = clock().addingTimeInterval(
                ConnectionWorkflowPolicy.roomOfferLifetime
            )
        case .offerAccepted(let id):
            outgoingDecisions.removeValue(forKey: id)?(true)
        case .offerRejected(let id):
            outgoingDecisions.removeValue(forKey: id)?(false)
        case .lifetimeChanged(let lifetime):
            applyLifetime(lifetime)
        case .closed(let reason):
            controlGeneration &+= 1
            controlTask?.cancel()
            controlTask = nil
            localTransferSyncTask?.cancel()
            localTransferSyncTask = nil
            idleExpiryTask?.cancel()
            idleExpiryTask = nil
            idleExpiryRevision = nil
            deferredControlFailure = nil
            outgoingDecisions.values.forEach { $0(false) }
            outgoingDecisions.removeAll()
            incomingRoomOffer = nil
            incomingRoomOfferDeadline = nil
            roomInvitation = nil
            pendingControlNearbySelection = nil
            pendingDeviceVerification = false
            idleDeadline = nil
            lifetimeRevision = nil
            requestedLocalTransferActive = nil
            reportedLocalTransferActive = nil
            controlPhase = .ended(reason)
            scheduleRememberedReconnect()
        }
    }

    private func handleRemembered(
        _ event: RoomControlEvent,
        peer: RememberedPeerSummary,
        generation: Int
    ) {
        guard controlGeneration == generation,
              activeRememberedRelationshipID == peer.relationshipID else {
            return
        }
        switch event {
        case .connected(let name, _, let lifetime):
            guard controlPhase == .connectingRemembered
                    || controlPhase == .waitingRemembered else {
                return
            }
            refreshRememberedRooms()
            let currentPeer = rememberedPeers.first(where: {
                $0.relationshipID == peer.relationshipID
            }) ?? peer
            if rememberedRoom?.relationshipID != peer.relationshipID {
                rememberedRoom = RememberedRoomSession(
                    peer: currentPeer,
                    baselineActivityIDs: baselineActivityIDs
                )
            }
            peerDisplayName = NearbyDiscoveryPeerRegistry.sanitizeDisplayName(name)
                ?? currentPeer.label
            isRoomCreator = false
            controlPhase = .connected
            rememberedRoomErrors.removeValue(forKey: peer.relationshipID)
            rememberedFailureCounts[peer.relationshipID] = 0
            rememberedRetryNotBefore.removeValue(forKey: peer.relationshipID)
            nextRememberedMode[peer.relationshipID] = .responder
            applyLifetime(lifetime)
            schedulePassiveRememberedDwellIfNeeded()
        case .incomingOffer(let offer):
            guard controlPhase == .connected, incomingRoomOffer == nil else { return }
            cancelPassiveRememberedDwell()
            incomingRoomOffer = offer
            incomingRoomOfferDeadline = clock().addingTimeInterval(
                ConnectionWorkflowPolicy.roomOfferLifetime
            )
        case .offerAccepted(let id):
            outgoingDecisions.removeValue(forKey: id)?(true)
        case .offerRejected(let id):
            outgoingDecisions.removeValue(forKey: id)?(false)
            schedulePassiveRememberedDwellIfNeeded()
        case .lifetimeChanged(let lifetime):
            applyLifetime(lifetime)
        case .closed(let reason):
            controlGeneration &+= 1
            localTransferSyncTask?.cancel()
            localTransferSyncTask = nil
            idleExpiryTask?.cancel()
            idleExpiryTask = nil
            idleExpiryRevision = nil
            deferredControlFailure = nil
            outgoingDecisions.values.forEach { $0(false) }
            outgoingDecisions.removeAll()
            incomingRoomOffer = nil
            incomingRoomOfferDeadline = nil
            idleDeadline = nil
            lifetimeRevision = nil
            requestedLocalTransferActive = nil
            reportedLocalTransferActive = nil
            activeRememberedRelationshipID = nil
            nextRememberedMode[peer.relationshipID] = .connector
            cancelPassiveRememberedDwell()
            if selectedRememberedRelationshipID != peer.relationshipID {
                rememberedRoom = nil
            }
            controlPhase = .ended(reason)
        }
    }

    private func applyLifetime(_ lifetime: RoomControlLifetimeState) {
        guard controlPhase == .connected else { return }
        if let lifetimeRevision, lifetime.revision <= lifetimeRevision { return }
        lifetimeRevision = lifetime.revision
        roomLifetimePolicy = lifetime.policy
        idleDeadline = lifetime.idleDeadline
    }

    private func startLocalTransferSyncIfNeeded() {
        guard localTransferSyncTask == nil,
              requestedLocalTransferActive != reportedLocalTransferActive,
              let gateway else {
            return
        }
        let generation = controlGeneration
        localTransferSyncTask = Task { @MainActor [weak self] in
            guard let self else { return }
            while self.controlGeneration == generation,
                  self.controlPhase == .connected,
                  let requested = self.requestedLocalTransferActive,
                  requested != self.reportedLocalTransferActive {
                do {
                    let lifetime = try await gateway.setLocalTransferActive(requested)
                    guard self.controlGeneration == generation,
                          self.controlPhase == .connected else {
                        return
                    }
                    self.reportedLocalTransferActive = requested
                    if let lifetime {
                        self.applyLifetime(lifetime)
                    }
                } catch {
                    guard self.controlGeneration == generation else { return }
                    self.localTransferSyncTask = nil
                    self.failControl(error.localizedDescription)
                    return
                }
            }
            self.localTransferSyncTask = nil
        }
    }

    private func attemptIdleExpiry(revision: UInt64) {
        guard let gateway else { return }
        let generation = controlGeneration
        idleExpiryRevision = revision
        idleExpiryTask = Task { @MainActor [weak self] in
            guard let self else { return }
            do {
                try await gateway.expireIdleDeadline()
                guard self.controlGeneration == generation,
                      self.idleExpiryRevision == revision else {
                    return
                }
                self.idleExpiryTask = nil
                self.idleExpiryRevision = nil
                self.deferredControlFailure = nil
                self.finishControl(reason: .idleExpired, notifyGateway: false)
            } catch {
                guard self.controlGeneration == generation,
                      self.idleExpiryRevision == revision else {
                    return
                }
                // A transfer-active update may win the race after the UI's
                // transmitted deadline expires. Rust then rejects the close;
                // retain the channel and reduce its newer authoritative state.
                let snapshot = gateway.lifetimeSnapshot()
                self.idleExpiryTask = nil
                self.idleExpiryRevision = nil
                if let snapshot {
                    self.applyLifetime(snapshot)
                }
                if let deferredControlFailure = self.deferredControlFailure {
                    self.deferredControlFailure = nil
                    self.failControl(deferredControlFailure)
                }
                // With the same revision, the next tick may retry. A newer
                // revision carries its own deadline (or pauses it entirely).
            }
        }
    }

    private func handleControlFailure(_ message: String, generation: Int) {
        guard controlGeneration == generation else { return }
        if idleExpiryTask != nil {
            deferredControlFailure = message
            return
        }
        failControl(message)
    }

    private func failControl(_ message: String) {
        guard controlPhase != .idle else { return }
        #if DEBUG
        NSLog("Envoix room control failed: %@", message)
        #endif
        let keepFailedRoom = room != nil
        cancelPassiveRememberedDwell()
        controlGeneration &+= 1
        controlTask?.cancel()
        controlTask = nil
        localTransferSyncTask?.cancel()
        localTransferSyncTask = nil
        idleExpiryTask?.cancel()
        idleExpiryTask = nil
        idleExpiryRevision = nil
        deferredControlFailure = nil
        outgoingDecisions.values.forEach { $0(false) }
        outgoingDecisions.removeAll()
        incomingRoomOffer = nil
        incomingRoomOfferDeadline = nil
        roomInvitation = nil
        pendingControlNearbySelection = nil
        pendingDeviceVerification = false
        idleDeadline = nil
        lifetimeRevision = nil
        requestedLocalTransferActive = nil
        reportedLocalTransferActive = nil
        if !keepFailedRoom {
            room = nil
        }
        controlPhase = .failed(message)
        scheduleRememberedReconnect()
    }

    private func endLocalState() {
        controlGeneration &+= 1
        controlTask?.cancel()
        controlTask = nil
        rememberedReconnectTask?.cancel()
        rememberedReconnectTask = nil
        rememberedReconnectTaskID = nil
        cancelPassiveRememberedDwell()
        releaseRememberedLease()
        activeRememberedRelationshipID = nil
        rememberedRoom = nil
        selectedRememberedRelationshipID = nil
        localTransferSyncTask?.cancel()
        localTransferSyncTask = nil
        idleExpiryTask?.cancel()
        idleExpiryTask = nil
        idleExpiryRevision = nil
        deferredControlFailure = nil
        room = nil
        pendingOffers.removeAll()
        incomingRoomOffer = nil
        incomingRoomOfferDeadline = nil
        roomInvitation = nil
        pendingControlNearbySelection = nil
        pendingDeviceVerification = false
        peerDisplayName = nil
        idleDeadline = nil
        lifetimeRevision = nil
        requestedLocalTransferActive = nil
        reportedLocalTransferActive = nil
        roomLifetimePolicy = .idleFifteenMinutes
        outgoingDecisions.values.forEach { $0(false) }
        outgoingDecisions.removeAll()
        controlPhase = .idle
    }

    @discardableResult
    func enqueue(
        _ offer: NearbyRendezvousOffer,
        receivedAt: Date = Date(),
        now: Date = Date()
    ) -> Bool {
        discardExpiredOffers(now: now)
        let pending = PendingNearbyInvitation(offer: offer, receivedAt: receivedAt)
        guard !pendingOffers.contains(where: {
            $0.deduplicationKey == pending.deduplicationKey
        }) else {
            return false
        }
        pendingOffers.append(pending)
        if pendingOffers.count > ConnectionWorkflowPolicy.maximumPendingOffers {
            pendingOffers.removeFirst(pendingOffers.count - ConnectionWorkflowPolicy.maximumPendingOffers)
        }
        return true
    }

    func discardPendingOffer(id: String) {
        pendingOffers.removeAll { $0.id == id }
    }

    func discardAllPendingOffers() {
        pendingOffers.removeAll()
    }

    func discardExpiredOffers(now: Date = Date()) {
        pendingOffers.removeAll { ConnectionWorkflowPolicy.isExpired($0, now: now) }
    }

    func captureActivity(_ activityID: String) {
        if var rememberedRoom,
           !rememberedRoom.baselineActivityIDs.contains(activityID) {
            rememberedRoom.activityIDs.insert(activityID)
            self.rememberedRoom = rememberedRoom
            return
        }
        guard var room, !room.baselineActivityIDs.contains(activityID) else { return }
        room.activityIDs.insert(activityID)
        self.room = room
    }
}
#endif
