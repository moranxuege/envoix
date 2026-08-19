#if os(iOS) || os(macOS)
enum ConnectionHubPresentationText {
    static func rememberedDeviceCount(_ count: Int, language: String) -> String {
        let displayCount = Int64(max(count, 0))
        return AppText.localized(
            "connection.devices.remembered_count",
            defaultValue: "\(displayCount) remembered",
            language: language
        )
    }

    static func pendingItemCount(_ count: Int, language: String) -> String {
        let displayCount = Int64(max(count, 0))
        return AppText.localized(
            "connection.devices.pending_item_count",
            defaultValue: "\(displayCount) items ready. Choose a device to send them.",
            language: language
        )
    }

    static func rememberedRoomStatus(
        _ status: RememberedRoomConnectionStatus,
        language: String
    ) -> String {
        let key: String
        switch status {
        case .offline: key = "connection.devices.status.offline"
        case .connecting: key = "connection.devices.status.connecting"
        case .waiting: key = "connection.devices.status.waiting"
        case .connected: key = "connection.devices.status.connected"
        case .needsRepair: key = "connection.devices.status.needs_repair"
        }
        return AppText.localized(key, language: language)
    }

    static func roomAction(
        isStarting: Bool,
        hasInvitation: Bool,
        language: String
    ) -> String {
        if isStarting {
            return AppText.localized("connection.room.action.creating", language: language)
        }
        return AppText.localized(
            hasInvitation
                ? "connection.room.action.reveal_qr"
                : "connection.room.action.create",
            language: language
        )
    }

    static func roomStatus(
        isStarting: Bool,
        hasInvitation: Bool,
        language: String
    ) -> String {
        if isStarting {
            return AppText.localized("connection.room.action.creating", language: language)
        }
        return AppText.localized(
            hasInvitation
                ? "connection.room.status.ready"
                : "connection.room.status.none",
            language: language
        )
    }
}
#endif
