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

@MainActor
final class ConnectionWorkflowState: ObservableObject {
    @Published private(set) var room: OneTimeRoomSession?
    @Published private(set) var pendingOffers: [PendingNearbyInvitation] = []

    var nextPendingOffer: PendingNearbyInvitation? {
        pendingOffers.first
    }

    func openRoom(
        origin: OneTimeRoomOrigin,
        pairingInput: String? = nil,
        suggestedAction: OneTimeRoomAction = .choose,
        existingActivityIDs: Set<String>
    ) {
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
