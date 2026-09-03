import EnvoixCore
import Foundation

enum RememberedRoomCopy: String, CaseIterable {
    case forgetQuestion = "remembered_room.forget.question"
    case cancel = "common.cancel"
    case forgetRoom = "remembered_room.forget.action"
    case incomingFiles = "room.offer.incoming"
    case decline = "common.decline"
    case preparingReceiver = "room.offer.preparing_receiver"
    case receive = "common.receive"
    case filesForRoom = "remembered_room.outbox.title"
    case retry = "remembered_room.action.retry"
    case remove = "common.remove"
    case noTransfers = "remembered_room.activity.empty.title"
    case noTransfersDetail = "remembered_room.activity.empty.detail"
    case roomActivity = "room.activity.title"
    case viewAll = "remembered_room.activity.view_all"
    case sentFiles = "remembered_room.transfer.sent_files"
    case receivedFiles = "remembered_room.transfer.received_files"
    case open = "common.open"
    case share = "common.share"
    case addFiles = "room.action.add_files"
    case disconnect = "remembered_room.action.disconnect"
    case reconnectsAutomatically = "remembered_room.reconnect.automatic"
    case preparedFiles = "remembered_room.outbox.prepared_files"
    case helperUnavailable = "remembered_room.agent.helper_unavailable"
    case agentRoomEmpty = "remembered_room.agent.room.empty"
    case preparingFiles = "remembered_room.agent.preparing_files"
    case helperKeepsRoom = "remembered_room.agent.keeps_room"
    case helperRefreshUnavailable = "remembered_room.agent.refresh_unavailable"
    case noHelperTransfers = "remembered_room.agent.activity.empty"
    case loadingHelperActivity = "remembered_room.agent.activity.loading"
    case helperActivityDetail = "remembered_room.agent.activity.detail"
    case pairedDevice = "remembered_room.agent.paired_device"
    case send = "home.send.title"
    case fileTransfer = "remembered_room.transfer.file_transfer"
}

enum RememberedRoomActivityStateCopy: String, CaseIterable {
    case preparing
    case pairing
    case connecting
    case waiting
    case transferring
    case verifying
    case saving
    case finalizing
    case needsAttention = "needs_attention"
    case paused
    case delivered
    case failed
    case canceled
}

enum RememberedRoomOutboxStateCopy: String, CaseIterable {
    case queued
    case offering
    case sending
    case check
}

enum RememberedRoomConnectionCopy: String, CaseIterable {
    case offline
    case available
    case connecting
    case waiting
    case connected
}

enum AgentRoomConnectionCopy: String, CaseIterable {
    case preparing
    case transferring
    case waiting
    case ready
}

enum AgentTransferStateCopy: String, CaseIterable {
    case awaitingApproval = "awaiting_approval"
    case queued
    case connecting
    case sending
    case receiving
    case paused
    case verifyingDelivery = "verifying_delivery"
    case delivered
    case received
    case rejected
    case failed
    case canceled
}

enum AgentTransferDetailCopy: String, CaseIterable {
    case userDeclined = "user_declined"
    case busy
    case insufficientSpace = "insufficient_space"
    case unsupportedContent = "unsupported_content"
    case invalidOffer = "invalid_offer"
    case queued
    case awaitingDeliveryProof = "awaiting_delivery_proof"
    case paused
}

enum AgentTransferPathCopy: String, CaseIterable {
    case lan
    case direct
    case relay
    case wifiAware = "wifi_aware"
    case other
}

enum RememberedRoomPresentationText {
    static func value(_ copy: RememberedRoomCopy, language: String) -> String {
        AppText.localized(copy.rawValue, language: language)
    }

    static func forgetDetail(hasQueuedFiles: Bool, language: String) -> String {
        AppText.localized(
            hasQueuedFiles
                ? "remembered_room.forget.detail_with_queue"
                : "remembered_room.forget.detail",
            language: language
        )
    }

    static func connectionStatus(
        _ status: RememberedRoomConnectionStatus,
        language: String
    ) -> String {
        let copy: RememberedRoomConnectionCopy
        switch status {
        case .offline: copy = .offline
        case .available: copy = .available
        case .connecting: copy = .connecting
        case .waiting: copy = .waiting
        case .connected: copy = .connected
        case .needsRepair(let message): return message
        }
        return AppText.localized(
            "remembered_room.connection.\(copy.rawValue)",
            language: language
        )
    }

    static func itemCount(_ count: Int, language: String) -> String {
        TransferActivityText.itemCount(UInt64(max(count, 0)), language: language)
    }

    static func itemCount(_ count: UInt32, language: String) -> String {
        TransferActivityText.itemCount(UInt64(count), language: language)
    }

    static func activityState(
        _ state: TransferActivityState,
        language: String
    ) -> String {
        let copy: RememberedRoomActivityStateCopy
        switch state {
        case .preparing: copy = .preparing
        case .pairing: copy = .pairing
        case .connecting: copy = .connecting
        case .waitingForPeer: copy = .waiting
        case .transferring: copy = .transferring
        case .verifying: copy = .verifying
        case .saving: copy = .saving
        case .waitingForReceiverSave, .finalizingDelivery: copy = .finalizing
        case .awaitingDecision: copy = .needsAttention
        case .paused: copy = .paused
        case .delivered: copy = .delivered
        case .failed: copy = .failed
        case .canceled: copy = .canceled
        }
        return AppText.localized(
            "remembered_room.activity.state.\(copy.rawValue)",
            language: language
        )
    }

    static func outboxState(
        _ state: RememberedRoomOutboxState,
        language: String
    ) -> String {
        let copy: RememberedRoomOutboxStateCopy
        switch state {
        case .queued: copy = .queued
        case .offering: copy = .offering
        case .transferring: copy = .sending
        case .needsAttention: copy = .check
        }
        return AppText.localized(
            "remembered_room.outbox.state.\(copy.rawValue)",
            language: language
        )
    }

    static func savedIn(_ destination: String, language: String) -> String {
        TransferActivityText.savedIn(destination, language: language)
    }

    static func savedItems(_ count: Int, language: String) -> String {
        TransferActivityText.savedItems(count, language: language)
    }

    static func agentRoomConnection(
        _ copy: AgentRoomConnectionCopy,
        language: String
    ) -> String {
        AppText.localized(
            "remembered_room.agent.connection.\(copy.rawValue)",
            language: language
        )
    }

    static func agentActivityTitle(hasLoadedSnapshot: Bool, language: String) -> String {
        value(
            hasLoadedSnapshot ? .noHelperTransfers : .loadingHelperActivity,
            language: language
        )
    }

    static func agentTransferTitle(
        direction: FfiTransferDirection,
        deviceLabel: String?,
        language: String
    ) -> String {
        let label = deviceLabel?.trimmingCharacters(in: .whitespacesAndNewlines)
        guard let label, !label.isEmpty else {
            return value(
                direction == .send ? .sentFiles : .receivedFiles,
                language: language
            )
        }
        let action = value(direction == .send ? .send : .receive, language: language)
        return "\(action) · \(label)"
    }
}

enum AgentTransferPresentationText {
    static func state(_ copy: AgentTransferStateCopy, language: String) -> String {
        AppText.localized(
            "remembered_room.agent.transfer.state.\(copy.rawValue)",
            language: language
        )
    }

    static func detail(_ copy: AgentTransferDetailCopy, language: String) -> String {
        AppText.localized(
            "remembered_room.agent.transfer.detail.\(copy.rawValue)",
            language: language
        )
    }

    static func path(_ copy: AgentTransferPathCopy, language: String) -> String {
        AppText.localized(
            "remembered_room.agent.transfer.path.\(copy.rawValue)",
            language: language
        )
    }
}
