#if os(iOS) || os(macOS)
enum ConnectionHubPresentationText {
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
