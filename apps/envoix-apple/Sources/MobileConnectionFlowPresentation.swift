import Foundation

enum MobileConnectionFlowCopy: String, CaseIterable {
    case cancel = "common.cancel"
    case continueAction = "mobile_flow.action.continue"
    case close = "common.close"
    case verifyNearbyDevice = "mobile_flow.verification.nearby.title"
    case cancelVerification = "mobile_flow.verification.cancel"
    case enterVerificationCode = "mobile_flow.verification.enter.title"
    case verifyAndConnect = "mobile_flow.verification.connect"
    case verificationInstruction = "mobile_flow.verification.instruction"
    case verifyThisDevice = "mobile_flow.verification.device.title"
    case verifyDevice = "mobile_flow.verification.device.action"
    case acceptNearbyOffer = "mobile_flow.nearby_offer.accept"
    case rejectNearbyOffer = "mobile_flow.nearby_offer.reject"
    case endRoomQuestion = "mobile_flow.room.end_question"
    case keepRoom = "mobile_flow.room.keep"
    case endRoom = "room.action.end"
    case endRoomDetail = "mobile_flow.room.end_detail"
    case roomAlreadyOpen = "mobile_flow.room.already_open"
    case returnToRoom = "mobile_flow.room.return"
    case endAndReplace = "mobile_flow.room.end_and_replace"
    case oneRoomAtATime = "mobile_flow.room.single_limit"
    case newWindow = "mobile_flow.navigation.new_window"
    case refreshPairedDevices = "mobile_flow.paired.refresh"
    case backgroundHelper = "mobile_flow.activity.background_helper"
    case oneTimeTransfers = "mobile_flow.activity.one_time_transfers"
    case activity = "mobile_flow.navigation.activity"
    case back = "mobile_flow.navigation.back"
    case settings = "mobile_flow.navigation.settings"
    case room = "connection.room.title"
    case offerFiles = "mobile_flow.transfer.offer_files"
    case receiveFiles = "mobile_flow.transfer.receive_files"
    case connectionInputRequired = "mobile_flow.connection_input.required"
    case connectionInputInvalid = "mobile_flow.connection_input.invalid"
    case roomOccupied = "mobile_flow.connection_input.room_occupied"
    case applicationSupportUnavailable = "mobile_flow.error.application_support_unavailable"
    case inviteV2Unavailable = "mobile_flow.connection_input.invite_v2_unavailable"
    case droppedItemsSendBusy = "mobile_flow.send.drop_busy"
    case anotherSendBusy = "mobile_flow.send.busy"
    case queuedForReconnect = "mobile_flow.send.queued_for_reconnect"
    case invalidNearbyInvitation = "mobile_flow.nearby_offer.invalid"
    case currentVerificationCodeRequired = "mobile_flow.verification.current_code_required"
    case openedFileQueued = "mobile_flow.open_file.queued"
    case nfcReadPrompt = "mobile_flow.nfc.read_prompt"
    case sharedItemSendBusy = "mobile_flow.share.send_busy"
    case sharedItemsNeedRoom = "mobile_flow.share.need_room"
    case anotherReceiveBusy = "mobile_flow.receive.busy"
    case offerUnavailable = "mobile_flow.receive.offer_unavailable"
    case offerRouteMismatch = "mobile_flow.receive.route_mismatch"
    case saveFolderInaccessible = "mobile_flow.save.inaccessible"
    case saveFolderRequiredOnMac = "mobile_flow.save.required_on_mac"
    case saveFolderPermissionExpired = "mobile_flow.save.permission_expired"
    case localFilesOnly = "mobile_flow.open_file.local_only"
    case unsupportedItem = "mobile_flow.open_file.unsupported_item"
    case inaccessibleItem = "mobile_flow.open_file.inaccessible"
    case manualEntryDetail = "mobile_flow.manual_entry.detail"
    case manualEntryTitle = "mobile_flow.manual_entry.title"
    case paste = "common.paste"
    case clipboardEmpty = "transfer.pairing.clipboard_empty"
}

enum MobileConnectionFlowPresentationText {
    static func value(_ copy: MobileConnectionFlowCopy, language: String) -> String {
        AppText.localized(copy.rawValue, language: language)
    }

    static func externalInvitationTitle(
        isRoomInvitation: Bool,
        isNFC: Bool,
        language: String
    ) -> String {
        let key: String
        switch (isNFC, isRoomInvitation) {
        case (true, true): key = "mobile_flow.external.nfc_room.title"
        case (true, false): key = "mobile_flow.external.nfc_invitation.title"
        case (false, true): key = "mobile_flow.external.room.title"
        case (false, false): key = "mobile_flow.external.invitation.title"
        }
        return AppText.localized(key, language: language)
    }

    static func externalInvitationMessage(
        isRoomInvitation: Bool,
        isNFC: Bool,
        language: String
    ) -> String {
        if isNFC {
            return AppText.localized("mobile_flow.external.nfc.detail", language: language)
        }
        return AppText.localized(
            isRoomInvitation
                ? "mobile_flow.external.room.detail"
                : "mobile_flow.external.invitation.detail",
            language: language
        )
    }

    static func outgoingVerification(code: String, language: String) -> String {
        AppText.localized(
            "mobile_flow.verification.outgoing_code",
            defaultValue: "Enter \(code) on the other device. The code is never sent over Bluetooth.",
            language: language
        )
    }

    static func deviceVerification(peerDisplayName: String?, language: String) -> String {
        let peer = normalizedName(
            peerDisplayName,
            fallbackKey: "mobile_flow.peer.other_device",
            language: language
        )
        return AppText.localized(
            "mobile_flow.verification.device.detail",
            defaultValue: "Enter the six-digit code shown by \(peer). A successful match saves this device for future rooms.",
            language: language
        )
    }

    static func nearbyOfferTitle(isRoomInvitation: Bool, language: String) -> String {
        AppText.localized(
            isRoomInvitation
                ? "mobile_flow.nearby_offer.room.title"
                : "mobile_flow.nearby_offer.invitation.title",
            language: language
        )
    }

    static func nearbyOfferMessage(
        senderDisplayName: String?,
        isRoomInvitation: Bool,
        language: String
    ) -> String {
        let sender = normalizedName(
            senderDisplayName,
            fallbackKey: "mobile_flow.peer.nearby_device",
            language: language
        )
        if isRoomInvitation {
            return AppText.localized(
                "mobile_flow.nearby_offer.room.detail",
                defaultValue: "\(sender) wants to open a room. Confirm on the other device before accepting.",
                language: language
            )
        }
        return AppText.localized(
            "mobile_flow.nearby_offer.invitation.detail",
            defaultValue: "\(sender) wants to start a one-time transfer. Confirm on the other device before accepting.",
            language: language
        )
    }

    static func durablePairingCompleted(label: String, language: String) -> String {
        let device = normalizedName(
            label,
            fallbackKey: "mobile_flow.peer.device",
            language: language
        )
        return AppText.localized(
            "mobile_flow.paired.completed",
            defaultValue: "\(device) is now securely paired.",
            language: language
        )
    }

    static func queuedForDevice(label: String, language: String) -> String {
        let device = normalizedName(
            label,
            fallbackKey: "mobile_flow.peer.device",
            language: language
        )
        return AppText.localized(
            "mobile_flow.paired.queued",
            defaultValue: "Queued for \(device).",
            language: language
        )
    }

    static func itemCountExceeded(maximum: Int, language: String) -> String {
        let displayMaximum = Int64(max(maximum, 0))
        return AppText.localized(
            "mobile_flow.open_file.item_limit",
            defaultValue: "Choose no more than \(displayMaximum) items.",
            language: language
        )
    }

    private static func normalizedName(
        _ value: String?,
        fallbackKey: String,
        language: String
    ) -> String {
        let trimmed = value?.trimmingCharacters(in: .whitespacesAndNewlines)
        if let trimmed, !trimmed.isEmpty {
            return trimmed
        }
        return AppText.localized(fallbackKey, language: language)
    }
}
