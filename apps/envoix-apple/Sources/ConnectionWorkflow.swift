#if os(iOS)
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
    case externalShare
}

struct OneTimeRoomSession: Equatable, Identifiable {
    let id: UUID
    var origin: OneTimeRoomOrigin
    var pairingInput: String?
    var suggestedAction: OneTimeRoomAction
    let endpoint: RoomControlEndpoint?
    let baselineActivityIDs: Set<String>
    var activityIDs: Set<String>

    init(
        id: UUID = UUID(),
        origin: OneTimeRoomOrigin,
        pairingInput: String? = nil,
        suggestedAction: OneTimeRoomAction = .choose,
        endpoint: RoomControlEndpoint? = nil,
        baselineActivityIDs: Set<String>
    ) {
        self.id = id
        self.origin = origin
        self.pairingInput = pairingInput
        self.suggestedAction = suggestedAction
        self.endpoint = endpoint
        self.baselineActivityIDs = baselineActivityIDs
        self.activityIDs = []
    }

    var nearbySelection: NearbyPairingSelection? {
        guard case .nearby(let selection) = origin else { return nil }
        return selection
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

    static func isExpired(_ offer: PendingNearbyInvitation, now: Date) -> Bool {
        now.timeIntervalSince(offer.receivedAt) >= offerLifetime
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
    case connected
    case ended(RoomControlCloseReason)
    case failed(String)
}

@MainActor
final class ConnectionWorkflowState: ObservableObject {
    @Published private(set) var room: OneTimeRoomSession?
    @Published private(set) var pendingOffers: [PendingNearbyInvitation] = []
    @Published private(set) var controlPhase: RoomControlPhase = .idle
    @Published private(set) var roomInvitation: RoomControlInvitation?
    @Published private(set) var peerDisplayName: String?
    @Published private(set) var incomingRoomOffer: RoomControlTransferOffer?
    @Published private(set) var roomLifetimePolicy: RoomControlLifetimePolicy = .idleFifteenMinutes
    @Published private(set) var idleDeadline: Date?
    @Published private(set) var isRoomCreator = false

    private let gateway: RoomControlGateway?
    private let clock: () -> Date
    private var controlTask: Task<Void, Never>?
    private var controlGeneration = 0
    private var baselineActivityIDs: Set<String> = []
    private var outgoingDecisions: [String: (Bool) -> Void] = [:]
    private var incomingRoomOfferDeadline: Date?
    private var lifetimeRevision: UInt64?
    private var requestedLocalTransferActive: Bool?
    private var reportedLocalTransferActive: Bool?
    private var localTransferSyncTask: Task<Void, Never>?
    private var idleExpiryTask: Task<Void, Never>?
    private var idleExpiryRevision: UInt64?
    private var deferredControlFailure: String?

    init(
        gateway: RoomControlGateway? = nil,
        clock: @escaping () -> Date = Date.init
    ) {
        self.gateway = gateway
        self.clock = clock
    }

    deinit {
        controlTask?.cancel()
        localTransferSyncTask?.cancel()
        idleExpiryTask?.cancel()
    }

    var nextPendingOffer: PendingNearbyInvitation? {
        pendingOffers.first
    }

    func openRoom(
        origin: OneTimeRoomOrigin,
        pairingInput: String? = nil,
        suggestedAction: OneTimeRoomAction = .choose,
        existingActivityIDs: Set<String>
    ) {
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
    }

    @discardableResult
    func startHosting(
        broker: String,
        relay: String,
        displayName: String,
        identityPath: String,
        existingActivityIDs: Set<String>
    ) -> String? {
        guard let gateway else {
            return "Room control is unavailable in this build."
        }
        do {
            let now = clock()
            let invitation = try gateway.makeInvitation(
                broker: broker,
                relay: relay,
                now: now
            )
            // Generate the replacement before touching the active host. A
            // refresh failure must not invalidate a room code that still
            // works on the other device.
            gateway.close(reason: .userEnded)
            endLocalState()
            roomInvitation = invitation
            controlPhase = .hosting
            isRoomCreator = true
            baselineActivityIDs = existingActivityIDs
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
        existingActivityIDs: Set<String>
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
            roomInvitation = invitation
            controlPhase = .joining
            isRoomCreator = false
            baselineActivityIDs = existingActivityIDs
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
        outgoingDecisions[offer.id] = onDecision
        Task { @MainActor [weak self] in
            do {
                if let lifetime = try await gateway.offerTransfer(offer) {
                    self?.applyLifetime(lifetime)
                }
            } catch {
                self?.outgoingDecisions.removeValue(forKey: offer.id)?(false)
                self?.failControl(error.localizedDescription)
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
        do {
            if let lifetime = try await gateway.acceptOffer(id: offer.id) {
                applyLifetime(lifetime)
            }
            return offer
        } catch {
            failControl(error.localizedDescription)
            return nil
        }
    }

    func rejectIncomingRoomOffer() {
        guard let gateway, let offer = incomingRoomOffer else { return }
        incomingRoomOffer = nil
        incomingRoomOfferDeadline = nil
        Task { @MainActor [weak self] in
            do {
                if let lifetime = try await gateway.rejectOffer(id: offer.id) {
                    self?.applyLifetime(lifetime)
                }
            } catch {
                self?.failControl(error.localizedDescription)
            }
        }
    }

    func setKeepOpen(_ keepOpen: Bool) {
        guard isRoomCreator, controlPhase == .connected, let gateway else { return }
        let policy: RoomControlLifetimePolicy = keepOpen
            ? .untilForegroundEnds
            : .idleFifteenMinutes
        Task { @MainActor [weak self] in
            do {
                if let lifetime = try await gateway.setLifetimePolicy(policy) {
                    self?.applyLifetime(lifetime)
                }
            } catch {
                self?.failControl(error.localizedDescription)
            }
        }
    }

    func setLocalTransferActive(_ active: Bool) {
        guard controlPhase == .connected, gateway != nil else { return }
        requestedLocalTransferActive = active
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
        idleExpiryRevision = nil
        deferredControlFailure = nil
        outgoingDecisions.values.forEach { $0(false) }
        outgoingDecisions.removeAll()
        incomingRoomOffer = nil
        incomingRoomOfferDeadline = nil
        roomInvitation = nil
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
    }

    private func handle(_ event: RoomControlEvent, generation: Int) {
        guard controlGeneration == generation else { return }
        switch event {
        case .connected(let name, let creator, let lifetime):
            guard controlPhase == .hosting || controlPhase == .joining else { return }
            let endpoint = roomInvitation?.endpoint
            peerDisplayName = NearbyDiscoveryPeerRegistry.sanitizeDisplayName(name)
                ?? "Nearby device"
            isRoomCreator = creator
            controlPhase = .connected
            roomInvitation = nil
            room = OneTimeRoomSession(
                origin: .roomControl,
                endpoint: endpoint,
                baselineActivityIDs: baselineActivityIDs
            )
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
            idleDeadline = nil
            lifetimeRevision = nil
            requestedLocalTransferActive = nil
            reportedLocalTransferActive = nil
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
        let keepFailedRoom = room != nil
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
        idleDeadline = nil
        lifetimeRevision = nil
        requestedLocalTransferActive = nil
        reportedLocalTransferActive = nil
        if !keepFailedRoom {
            room = nil
        }
        controlPhase = .failed(message)
    }

    private func endLocalState() {
        controlGeneration &+= 1
        controlTask?.cancel()
        controlTask = nil
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
        guard var room, !room.baselineActivityIDs.contains(activityID) else { return }
        room.activityIDs.insert(activityID)
        self.room = room
    }
}
#endif
