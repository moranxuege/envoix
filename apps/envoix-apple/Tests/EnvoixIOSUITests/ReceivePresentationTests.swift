import XCTest
@testable import Envoix_iOS

final class ReceivePresentationTests: XCTestCase {
    func testReceiveCatalogProvidesStaticDestinationAndInviteCopy() {
        let cases: [(String, String, String)] = [
            (
                "receive.concurrent.finish_send",
                "Finish sending before starting a receive.",
                "请先完成发送任务，再开始接收。"
            ),
            ("receive.destination.authorized", "Save folder authorized", "保存文件夹已授权"),
            ("receive.destination.default_restored", "Default save folder restored", "已恢复默认保存位置"),
            ("receive.destination.error.choose_first", "Choose a save folder first.", "请先选择保存文件夹。"),
            (
                "receive.destination.error.ios_not_writable",
                "Envoix cannot write to the selected Files folder. Choose it again or reset to the default save folder.",
                "Envoix 无法写入已选择的 Files 文件夹。请重新选择，或重置为默认保存位置。"
            ),
            (
                "receive.destination.error.ios_unavailable",
                "The selected Files folder is unavailable. Choose it again or reset to the default save folder.",
                "已选择的 Files 文件夹不可用。请重新选择，或重置为默认保存位置。"
            ),
            (
                "receive.destination.error.macos_access_denied",
                "macOS did not grant access to the selected folder. Choose it again and confirm the system prompt.",
                "macOS 未授予所选文件夹访问权限。请重新选择并确认系统授权提示。"
            ),
            (
                "receive.destination.error.macos_grant_first",
                "Choose the save folder once to grant macOS access before receiving.",
                "接收前请先选择一次保存文件夹，以授予 macOS 访问权限。"
            ),
            (
                "receive.destination.error.not_writable",
                "Envoix cannot write to the selected save folder. Choose it again or check its permissions.",
                "Envoix 无法写入所选保存文件夹。请重新选择或检查文件夹权限。"
            ),
            (
                "receive.destination.error.permission_unavailable",
                "The save-folder permission is unavailable. Choose the folder again.",
                "保存文件夹权限不可用。请重新选择该文件夹。"
            ),
            ("receive.destination.ios_default", "On My iPhone / Envoix / Downloads", "我的 iPhone / Envoix / Downloads"),
            ("receive.destination.ios_unavailable", "Selected Files folder unavailable", "已选 Files 文件夹不可用"),
            ("receive.destination.macos_choose", "Choose a save folder", "请选择保存文件夹"),
            (
                "receive.destination.macos_unavailable",
                "Selected folder unavailable — choose again",
                "已选文件夹不可用——请重新选择"
            ),
            ("receive.destination.method_copy", "Verify, then copy", "校验后复制"),
            ("receive.destination.method_direct", "Save directly", "直接保存"),
            ("receive.destination.method_title", "Save method", "保存方式"),
            ("receive.destination.reset", "Reset", "重置"),
            ("receive.destination.select", "Select", "选择"),
            ("receive.destination.selected", "Save folder selected", "已选择保存文件夹"),
            ("receive.destination.title", "Save to", "保存到"),
            (
                "receive.invite.role",
                "This invitation assigns this device the Receive role.",
                "此邀请已将本设备指定为接收端。"
            ),
            ("receive.invite.verified", "InviteV2 verified", "InviteV2 已验证"),
            (
                "receive.error.offer_unavailable",
                "The file offer is no longer available.",
                "此文件邀请已不可用。"
            ),
            (
                "receive.error.room_control_unavailable",
                "Room control receive is unavailable on this platform.",
                "此平台不支持房间控制接收。"
            ),
            (
                "receive.error.room_offer_needs_invite",
                "This room offer needs a new InviteV2 invitation.",
                "此房间邀请需要新的 InviteV2 邀请。"
            ),
        ]
        for (key, english, chinese) in cases {
            XCTAssertEqual(AppText.localized(key, language: "en"), english, key)
            XCTAssertEqual(AppText.localized(key, language: "zh-Hans"), chinese, key)
        }
    }

    func testDestinationProjectionCoversSaveModeAndFolderAvailability() {
        XCTAssertEqual(
            ReceivePresentationText.saveMethodDetail(usesCopy: true, language: "en"),
            "Uses additional temporary space and saving time for destinations that cannot safely finalize the same object."
        )
        XCTAssertEqual(
            ReceivePresentationText.saveMethodDetail(usesCopy: false, language: "zh-Hans"),
            "在所选存储上只写入一次，校验完成后直接显示文件。"
        )
        XCTAssertEqual(
            ReceivePresentationText.folderAction(isUnavailable: false, language: "en"),
            "Choose"
        )
        XCTAssertEqual(
            ReceivePresentationText.folderAction(isUnavailable: true, language: "zh-Hans"),
            "重新选择"
        )
        XCTAssertEqual(
            ReceivePresentationText.folderHelper(isUnavailable: true, language: "en"),
            "The selected Files folder permission expired. Choose it again or reset to the default folder."
        )
        XCTAssertEqual(
            ReceivePresentationText.folderHelper(isUnavailable: false, language: "zh-Hans"),
            "默认保存到 Files > On My iPhone > Envoix > Downloads。也可以选择其他 Files 文件夹。"
        )
    }

    func testPrimaryActionUsesStablePriority() {
        XCTAssertEqual(primary(isAccepting: true, isDelivering: true, another: true, busy: true), "Accepting offer…")
        XCTAssertEqual(primary(isDelivering: true, another: true, busy: true), "Delivering Invitation…")
        XCTAssertEqual(primary(another: true, busy: true), "Start Another Receive")
        XCTAssertEqual(primary(busy: true), "Managed in Activity")
        XCTAssertEqual(primary(), "Start Receiving")
        XCTAssertEqual(
            ReceivePresentationText.primaryAction(
                isAcceptingOffer: false,
                isDeliveringInvitation: false,
                canStartAnother: false,
                isBusy: false,
                language: "zh-Hans"
            ),
            "开始接收"
        )
    }

    func testAddressActionTracksDisclosureState() {
        XCTAssertEqual(
            ReceivePresentationText.addressAction(isRevealed: false, language: "en"),
            "Show address"
        )
        XCTAssertEqual(
            ReceivePresentationText.addressAction(isRevealed: true, language: "zh-Hans"),
            "隐藏地址"
        )
    }

    private func primary(
        isAccepting: Bool = false,
        isDelivering: Bool = false,
        another: Bool = false,
        busy: Bool = false
    ) -> String {
        ReceivePresentationText.primaryAction(
            isAcceptingOffer: isAccepting,
            isDeliveringInvitation: isDelivering,
            canStartAnother: another,
            isBusy: busy,
            language: "en"
        )
    }
}
