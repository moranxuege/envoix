#if os(iOS) || os(macOS)
import Foundation

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

    static func lifetime(
        origin: OneTimeRoomOrigin,
        phase: RoomControlPhase,
        policy: RoomControlLifetimePolicy,
        idleDeadline: Date?,
        now: Date,
        language: String
    ) -> String {
        guard origin == .roomControl else {
            return AppText.localized("room.lifetime.one_time_transfer", language: language)
        }
        switch phase {
        case .ended, .failed:
            return AppText.localized("room.lifetime.closed", language: language)
        case .idle, .hosting, .joining, .connectingRemembered, .waitingRemembered, .connected:
            break
        }
        if policy == .untilForegroundEnds {
            return AppText.localized("room.lifetime.kept_open", language: language)
        }
        guard let idleDeadline else {
            return AppText.localized("room.lifetime.timer_paused", language: language)
        }
        let seconds = max(0, Int(ceil(idleDeadline.timeIntervalSince(now))))
        let remaining = "\(seconds / 60):\(String(format: "%02d", seconds % 60))"
        return AppText.localized(
            "room.lifetime.ends_if_idle",
            defaultValue: "Ends in \(remaining) if idle",
            language: language
        )
    }

    static func additionalItems(_ count: UInt32, language: String) -> String {
        let displayCount = Int64(count)
        return AppText.localized(
            "room.offer.additional_count",
            defaultValue: "+\(displayCount) more",
            language: language
        )
    }

    static func fileCount(_ count: UInt32, language: String) -> String {
        let displayCount = Int64(count)
        return AppText.localized(
            "room.offer.file_count",
            defaultValue: "\(displayCount) files",
            language: language
        )
    }

    static func folderCount(_ count: UInt32, language: String) -> String {
        let displayCount = Int64(count)
        return AppText.localized(
            "room.offer.folder_count",
            defaultValue: "\(displayCount) folders",
            language: language
        )
    }

    static func offerSummary(
        fileCount: UInt32,
        folderCount: UInt32,
        byteDescription: String,
        language: String
    ) -> String {
        let files = self.fileCount(fileCount, language: language)
        let folders = self.folderCount(folderCount, language: language)
        return AppText.localized(
            "room.offer.summary_format",
            defaultValue: "\(files) · \(folders) · \(byteDescription)",
            language: language
        )
    }
}
#endif
