#if os(iOS) || os(macOS)
enum ReceivePresentationText {
    static func saveMethodDetail(usesCopy: Bool, language: String) -> String {
        AppText.localized(
            usesCopy
                ? "receive.destination.copy_detail"
                : "receive.destination.direct_detail",
            language: language
        )
    }

    static func folderAction(isUnavailable: Bool, language: String) -> String {
        AppText.localized(
            isUnavailable
                ? "receive.destination.choose_again"
                : "receive.destination.choose",
            language: language
        )
    }

    static func folderHelper(isUnavailable: Bool, language: String) -> String {
        AppText.localized(
            isUnavailable
                ? "receive.destination.unavailable_helper"
                : "receive.destination.default_helper",
            language: language
        )
    }

    static func primaryAction(
        isAcceptingOffer: Bool,
        isDeliveringInvitation: Bool,
        canStartAnother: Bool,
        isBusy: Bool,
        language: String
    ) -> String {
        if isAcceptingOffer {
            return AppText.localized("receive.action.accepting_offer", language: language)
        }
        if isDeliveringInvitation {
            return AppText.localized(
                "transfer.action.delivering_invitation",
                language: language
            )
        }
        if canStartAnother {
            return AppText.localized("receive.action.start_another", language: language)
        }
        if isBusy {
            return AppText.localized("transfer.action.managed_in_activity", language: language)
        }
        return AppText.localized("receive.action.start", language: language)
    }

    static func addressAction(isRevealed: Bool, language: String) -> String {
        AppText.localized(
            isRevealed ? "receive.address.hide" : "receive.address.show",
            language: language
        )
    }
}
#endif
