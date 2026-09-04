#if os(iOS) || os(macOS)
struct PairedDevicePresentation: Equatable, Identifiable {
    let id: String
    let label: String
}

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
        case .available: key = "connection.devices.status.available"
        case .connecting: key = "connection.devices.status.connecting"
        case .waiting: key = "connection.devices.status.waiting"
        case .connected: key = "connection.devices.status.connected"
        case .needsRepair: key = "connection.devices.status.needs_repair"
        }
        return AppText.localized(key, language: language)
    }

    static func nearbyStatus(
        visibility: NearbyVisibilityMode,
        language: String
    ) -> String {
        AppText.localized(
            visibility == .hidden
                ? "connection.nearby.status.off"
                : "connection.nearby.status.on",
            language: language
        )
    }

    static func visibilityOption(
        _ visibility: NearbyVisibilityMode,
        language: String
    ) -> String {
        let key: String
        switch visibility {
        case .hidden: key = "connection.nearby.visibility.off"
        case .everyoneTenMinutes: key = "connection.nearby.visibility.ten_minutes"
        case .whileAppOpen: key = "connection.nearby.visibility.app_open"
        }
        return AppText.localized(key, language: language)
    }

    static func nearbyEmptyState(
        isActive: Bool,
        hasReadyProvider: Bool,
        language: String
    ) -> String {
        let key: String
        if !isActive {
            key = "connection.nearby.empty.paused"
        } else if !hasReadyProvider {
            key = "connection.nearby.empty.unavailable"
        } else {
            key = "connection.nearby.empty.searching"
        }
        return AppText.localized(key, language: language)
    }

    static func peerInvitationHint(isAvailable: Bool, language: String) -> String {
        AppText.localized(
            isAvailable
                ? "connection.nearby.peer.open_hint"
                : "connection.nearby.peer.waiting_hint",
            language: language
        )
    }

    static func peerTrust(
        invitationAvailable: Bool,
        requiresTapToVerify: Bool,
        language: String
    ) -> String {
        let key: String
        if !invitationAvailable {
            key = "connection.nearby.peer.invitation_not_ready"
        } else if requiresTapToVerify {
            key = "connection.nearby.peer.tap_to_verify"
        } else {
            key = "room.trust.unverified"
        }
        return AppText.localized(key, language: language)
    }

    static func discoverySources(
        _ sources: Set<NearbyDiscoverySource>,
        language: String
    ) -> String {
        let labels = NearbyDiscoverySource.allCases.compactMap { source -> String? in
            guard sources.contains(source) else { return nil }
            let key: String
            switch source {
            case .bluetooth: key = "connection.nearby.source.bluetooth"
            case .mdns: key = "connection.nearby.source.local_network"
            case .wifiAware: key = "connection.nearby.wifi_aware.title"
            }
            return AppText.localized(key, language: language)
        }
        guard !labels.isEmpty else {
            return AppText.localized(
                "connection.nearby.source.unavailable",
                language: language
            )
        }
        let paths = labels.joined(separator: " · ")
        return AppText.localized(
            "connection.nearby.source.discovered_via",
            defaultValue: "Discovered via \(paths)",
            language: language
        )
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
