import XCTest
@testable import Envoix_iOS

final class ConnectionHubPresentationTests: XCTestCase {
    func testRoomCatalogKeysPreserveEnglishAndChineseLabels() {
        let labels = [
            ("connection.room.action.enter_code", "Enter code", "输入房间码"),
            ("connection.room.action.scan_qr", "Scan QR", "扫描二维码"),
            ("connection.room.close", "Close room", "关闭房间"),
            ("connection.room.code_copied", "Room code copied", "房间码已复制"),
            ("connection.room.copy_code", "Copy room code", "复制房间码"),
            ("connection.room.hide_qr", "Hide room QR", "隐藏房间二维码"),
            (
                "connection.room.hide_qr_hint",
                "Hides the invitation without ending the room.",
                "隐藏邀请，但不会结束房间。"
            ),
            ("connection.room.renew_invitation", "Renew room invitation", "更新房间邀请"),
            ("connection.room.title", "Room", "房间"),
        ]

        for (key, english, chinese) in labels {
            XCTAssertEqual(AppText.localized(key, language: "en"), english, key)
            XCTAssertEqual(AppText.localized(key, language: "zh-Hans"), chinese, key)
        }
    }

    func testRoomActionAndStatusCoverEveryInvitationState() {
        XCTAssertEqual(roomAction(isStarting: true, hasInvitation: false), "Creating invitation…")
        XCTAssertEqual(roomAction(isStarting: false, hasInvitation: false), "Create room")
        XCTAssertEqual(roomAction(isStarting: false, hasInvitation: true), "Reveal QR")

        XCTAssertEqual(roomStatus(isStarting: true, hasInvitation: false), "Creating invitation…")
        XCTAssertEqual(roomStatus(isStarting: false, hasInvitation: false), "No active room")
        XCTAssertEqual(
            roomStatus(isStarting: false, hasInvitation: true),
            "Ready · Waiting for another device"
        )
        XCTAssertEqual(
            ConnectionHubPresentationText.roomStatus(
                isStarting: false,
                hasInvitation: true,
                language: "zh-Hans"
            ),
            "已就绪 · 正在等待另一台设备"
        )
    }

    func testRoomActionsAndQRCodeUseTheSameSquareFootprint() {
        XCTAssertEqual(RoomInvitationLayout.viewportHeight, 240)
        XCTAssertEqual(RoomInvitationLayout.contentSide(availableWidth: 320), 240)
        XCTAssertEqual(RoomInvitationLayout.contentSide(availableWidth: 220), 220)
        XCTAssertEqual(
            RoomInvitationLayout.qrImageSide(contentSide: 240)
                + QRCard.contentPadding * 2,
            240
        )
    }

    func testWholeRoomCardKeepsOneContentHeightAcrossInvitationStates() {
        XCTAssertEqual(RoomInvitationLayout.headerHeight, 44)
        XCTAssertEqual(RoomInvitationLayout.cardSpacing, 14)
        XCTAssertEqual(RoomInvitationLayout.cardContentHeight, 298)
    }

    func testRevealedQRCodeReplacesConnectionMethods() {
        XCTAssertTrue(RoomInvitationLayout.showsConnectionMethods(revealed: false))
        XCTAssertFalse(RoomInvitationLayout.showsConnectionMethods(revealed: true))
    }

    func testSendSelectionUsesStatusCardOnlyAfterTransferStarts() {
        XCTAssertFalse(SendSelectionPresentationPolicy.showsTransferStatus(hasTransferActivity: false))
        XCTAssertTrue(SendSelectionPresentationPolicy.showsTransferStatus(hasTransferActivity: true))
    }

    private func roomAction(isStarting: Bool, hasInvitation: Bool) -> String {
        ConnectionHubPresentationText.roomAction(
            isStarting: isStarting,
            hasInvitation: hasInvitation,
            language: "en"
        )
    }

    private func roomStatus(isStarting: Bool, hasInvitation: Bool) -> String {
        ConnectionHubPresentationText.roomStatus(
            isStarting: isStarting,
            hasInvitation: hasInvitation,
            language: "en"
        )
    }
}
