#if os(iOS) || os(macOS)
enum SendPresentationPlatform: Equatable {
    case mobile
    case desktop
}

enum SendPresentationText {
    static func primaryAction(
        isPreparingManifest: Bool,
        isDeliveringInvitation: Bool,
        isWaitingForAcceptance: Bool,
        isAddingToRoom: Bool,
        isBusy: Bool,
        canAddToRoom: Bool,
        language: String
    ) -> String {
        if isPreparingManifest {
            return AppText.localized("send.action.cancel_preparation", language: language)
        }
        if isDeliveringInvitation {
            return AppText.localized(
                "transfer.action.delivering_invitation",
                language: language
            )
        }
        if isWaitingForAcceptance {
            return AppText.localized("send.action.waiting_for_acceptance", language: language)
        }
        if isAddingToRoom {
            return AppText.localized("send.action.adding_to_room", language: language)
        }
        if isBusy {
            return AppText.localized("transfer.action.managed_in_activity", language: language)
        }
        if canAddToRoom {
            return AppText.localized("send.action.add_to_room", language: language)
        }
        return AppText.localized("transfer.direction.send", language: language)
    }

    static func photoImportProgress(
        itemNumber: Int,
        itemCount: Int,
        language: String
    ) -> String {
        let number = Int64(max(itemNumber, 0))
        let count = Int64(max(itemCount, 0))
        return AppText.localized(
            "send.selection.photo_progress",
            defaultValue: "Preparing photo \(number) of \(count)…",
            language: language
        )
    }

    static func inventorySummary(
        rootCount: UInt32,
        fileCount: UInt32,
        folderCount: UInt32,
        warningCount: UInt32,
        byteDescription: String,
        language: String
    ) -> String {
        let roots = TransferContentText.rootCount(rootCount, language: language)
        let files = TransferContentText.fileCount(fileCount, language: language)
        let folders = TransferContentText.folderCount(folderCount, language: language)
        let summary = AppText.localized(
            "send.selection.inventory.summary",
            defaultValue: "\(roots) · \(files) · \(folders) · \(byteDescription)",
            language: language
        )
        guard warningCount > 0 else { return summary }
        let warnings = AppText.localized(
            "send.selection.inventory.warning_count",
            defaultValue: "\(Int64(warningCount)) warnings",
            language: language
        )
        return AppText.localized(
            "send.selection.inventory.summary_with_warnings",
            defaultValue: "\(summary) · \(warnings)",
            language: language
        )
    }

    static func removeItem(_ name: String, language: String) -> String {
        AppText.localized(
            "send.selection.remove_item",
            defaultValue: "Remove \(name)",
            language: language
        )
    }

    static func additionalTopLevelItems(_ count: Int, language: String) -> String {
        let displayCount = Int64(max(count, 0))
        return AppText.localized(
            "send.selection.additional_root_count",
            defaultValue: "\(displayCount) more top-level items are included.",
            language: language
        )
    }

    static func guidance(
        platform: SendPresentationPlatform,
        language: String
    ) -> String {
        AppText.localized(
            platform == .mobile
                ? "send.selection.guidance.mobile"
                : "send.selection.guidance.desktop",
            language: language
        )
    }

    static func selectionTitle(
        itemCount: Int,
        singleItemName: String?,
        language: String
    ) -> String {
        switch itemCount {
        case 0:
            return AppText.localized("send.selection.choose", language: language)
        case 1:
            return singleItemName ?? AppText.localized("send.selection.choose", language: language)
        default:
            let displayCount = Int64(max(itemCount, 0))
            return AppText.localized(
                "send.selection.selected_count",
                defaultValue: "\(displayCount) items selected",
                language: language
            )
        }
    }

    static func selectionSubtitle(
        itemCount: Int,
        singleItemIsDirectory: Bool,
        platform: SendPresentationPlatform,
        language: String
    ) -> String {
        let key: String
        switch itemCount {
        case 0:
            key = platform == .mobile
                ? "send.selection.subtitle.mobile_empty"
                : "send.selection.subtitle.desktop_empty"
        case 1 where singleItemIsDirectory:
            key = "send.selection.subtitle.folder"
        case 1:
            key = platform == .mobile
                ? "send.selection.subtitle.mobile_file"
                : "send.selection.subtitle.desktop_file"
        default:
            key = "send.selection.subtitle.multiple"
        }
        return AppText.localized(key, language: language)
    }
}
#endif
