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
    let baselineActivityIDs: Set<String>
    var activityIDs: Set<String>

    init(
        id: UUID = UUID(),
        origin: OneTimeRoomOrigin,
        pairingInput: String? = nil,
        suggestedAction: OneTimeRoomAction = .choose,
        baselineActivityIDs: Set<String>
    ) {
        self.id = id
        self.origin = origin
        self.pairingInput = pairingInput
        self.suggestedAction = suggestedAction
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

    static func localAction(for remoteRole: FfiInviteRole) -> OneTimeRoomAction {
        switch remoteRole {
        case .send: return .receiveFiles
        case .receive: return .offerFiles
        case .unknown: return .choose
        }
    }

    static func isExpired(_ offer: PendingNearbyInvitation, now: Date) -> Bool {
        now.timeIntervalSince(offer.receivedAt) >= offerLifetime
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

    init(
        gateway: RoomControlGateway? = nil,
        clock: @escaping () -> Date = Date.init
    ) {
        self.gateway = gateway
        self.clock = clock
    }

    deinit {
        controlTask?.cancel()
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
        gateway.close(reason: .userEnded)
        endLocalState()
        do {
            let now = clock()
            let invitation = try gateway.makeInvitation(
                broker: broker,
                relay: relay,
                now: now
            )
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
                    guard self.controlGeneration == generation else { return }
                    self.failControl(error.localizedDescription)
                }
            }
            return nil
        } catch {
            failControl(error.localizedDescription)
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
                    guard self.controlGeneration == generation else { return }
                    self.failControl(error.localizedDescription)
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
        noteRoomActivity()
        Task { @MainActor [weak self] in
            do {
                try await gateway.offerTransfer(offer)
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
        noteRoomActivity()
        do {
            try await gateway.acceptOffer(id: offer.id)
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
        noteRoomActivity()
        Task { @MainActor [weak self] in
            do {
                try await gateway.rejectOffer(id: offer.id)
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
        roomLifetimePolicy = policy
        idleDeadline = keepOpen ? nil : clock().addingTimeInterval(
            ConnectionWorkflowPolicy.roomIdleLifetime
        )
        Task { @MainActor [weak self] in
            do {
                try await gateway.setLifetimePolicy(policy)
            } catch {
                self?.failControl(error.localizedDescription)
            }
        }
    }

    func noteRoomActivity(now: Date? = nil) {
        guard controlPhase == .connected,
              roomLifetimePolicy == .idleFifteenMinutes else { return }
        idleDeadline = (now ?? clock()).addingTimeInterval(
            ConnectionWorkflowPolicy.roomIdleLifetime
        )
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
              roomLifetimePolicy == .idleFifteenMinutes,
              !hasActiveTransfer,
              incomingRoomOffer == nil,
              let idleDeadline,
              now >= idleDeadline else {
            return
        }
        endControl(reason: .idleExpired)
    }

    func endControl(reason: RoomControlCloseReason) {
        let keepEndedRoom = room != nil
            && reason != .userEnded
            && reason != .invitationExpired
        controlGeneration &+= 1
        controlTask?.cancel()
        controlTask = nil
        outgoingDecisions.values.forEach { $0(false) }
        outgoingDecisions.removeAll()
        incomingRoomOffer = nil
        incomingRoomOfferDeadline = nil
        roomInvitation = nil
        idleDeadline = nil
        controlPhase = .ended(reason)
        if !keepEndedRoom {
            room = nil
        }
        gateway?.close(reason: reason)
    }

    private func handle(_ event: RoomControlEvent, generation: Int) {
        guard controlGeneration == generation else { return }
        switch event {
        case .connected(let name, let creator):
            guard controlPhase == .hosting || controlPhase == .joining else { return }
            peerDisplayName = NearbyDiscoveryPeerRegistry.sanitizeDisplayName(name)
                ?? "Nearby device"
            isRoomCreator = creator
            controlPhase = .connected
            roomInvitation = nil
            room = OneTimeRoomSession(
                origin: .roomControl,
                baselineActivityIDs: baselineActivityIDs
            )
            noteRoomActivity()
        case .incomingOffer(let offer):
            guard controlPhase == .connected, incomingRoomOffer == nil else { return }
            incomingRoomOffer = offer
            incomingRoomOfferDeadline = clock().addingTimeInterval(
                ConnectionWorkflowPolicy.roomOfferLifetime
            )
            noteRoomActivity()
        case .offerAccepted(let id):
            outgoingDecisions.removeValue(forKey: id)?(true)
            noteRoomActivity()
        case .offerRejected(let id):
            outgoingDecisions.removeValue(forKey: id)?(false)
            noteRoomActivity()
        case .policyChanged(let policy):
            roomLifetimePolicy = policy
            idleDeadline = policy == .idleFifteenMinutes
                ? clock().addingTimeInterval(ConnectionWorkflowPolicy.roomIdleLifetime)
                : nil
        case .closed(let reason):
            controlGeneration &+= 1
            outgoingDecisions.values.forEach { $0(false) }
            outgoingDecisions.removeAll()
            incomingRoomOffer = nil
            incomingRoomOfferDeadline = nil
            roomInvitation = nil
            idleDeadline = nil
            controlPhase = .ended(reason)
        }
    }

    private func failControl(_ message: String) {
        guard controlPhase != .idle else { return }
        let keepFailedRoom = room != nil
        controlGeneration &+= 1
        controlTask?.cancel()
        controlTask = nil
        outgoingDecisions.values.forEach { $0(false) }
        outgoingDecisions.removeAll()
        incomingRoomOffer = nil
        incomingRoomOfferDeadline = nil
        roomInvitation = nil
        idleDeadline = nil
        if !keepFailedRoom {
            room = nil
        }
        controlPhase = .failed(message)
    }

    private func endLocalState() {
        controlGeneration &+= 1
        controlTask?.cancel()
        controlTask = nil
        room = nil
        pendingOffers.removeAll()
        incomingRoomOffer = nil
        incomingRoomOfferDeadline = nil
        roomInvitation = nil
        peerDisplayName = nil
        idleDeadline = nil
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
