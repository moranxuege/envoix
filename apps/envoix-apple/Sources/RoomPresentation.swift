#if os(iOS) || os(macOS)
enum RoomPresentationText {
    static func status(
        phase: RoomControlPhase,
        origin: OneTimeRoomOrigin,
        selectedPeerIsVisible: Bool,
        discoveryIsActive: Bool,
        language: String
    ) -> String {
        let key: String
        switch phase {
        case .hosting: key = "room.status.hosting"
        case .joining: key = "room.status.joining"
        case .connectingRemembered: key = "room.status.connecting"
        case .waitingRemembered: key = "room.status.waiting_remembered"
        case .connected: key = "room.status.connected"
        case .ended: key = "room.status.ended"
        case .failed: key = "room.status.needs_attention"
        case .idle:
            switch origin {
            case .nearby:
                if selectedPeerIsVisible {
                    key = "room.status.nearby_now"
                } else {
                    key = discoveryIsActive
                        ? "room.status.looking_for_device"
                        : "room.status.discovery_paused"
                }
            case .pairingCode: key = "room.status.invite_loaded"
            case .showCode: key = "room.status.ready_for_qr"
            case .roomControl: key = "room.status.connecting"
            }
        }
        return AppText.localized(key, language: language)
    }

    static func trust(authenticated: Bool, language: String) -> String {
        AppText.localized(
            authenticated ? "room.trust.authenticated" : "room.trust.unverified",
            language: language
        )
    }

    static func endedMessage(phase: RoomControlPhase, language: String) -> String? {
        switch phase {
        case .ended(let reason):
            let key: String
            switch reason {
            case .userEnded: key = "room.ended.user"
            case .idleExpired: key = "room.ended.idle"
            case .invitationExpired: key = "room.ended.invitation"
            case .peerEnded: key = "room.ended.peer"
            case .backgrounded: key = "room.ended.background"
            case .networkLost: key = "room.ended.network"
            case .protocolFailure: key = "room.ended.protocol"
            }
            return AppText.localized(key, language: language)
        case .failed(let message):
            return message
        default:
            return nil
        }
    }
}
#endif
