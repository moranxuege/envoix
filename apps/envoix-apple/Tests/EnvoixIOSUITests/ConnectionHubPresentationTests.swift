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

    func testRememberedDeviceCatalogKeysPreserveEnglishAndChineseLabels() {
        let labels = [
            ("common.save", "Save", "保存"),
            (
                "connection.devices.drop_failed",
                "Envoix could not read every dropped item.",
                "Envoix 无法读取全部拖入项目。"
            ),
            (
                "connection.devices.drop_hint",
                "Choose Send, or drop files and folders directly onto a device.",
                "点击“发送”，或把文件和文件夹直接拖到设备上。"
            ),
            ("connection.devices.incoming", "Incoming files", "收到文件邀请"),
            (
                "connection.devices.incoming_accessibility",
                "Incoming files waiting for your decision",
                "有文件邀请等待处理"
            ),
            ("connection.devices.open_offer", "Open", "查看"),
            ("connection.devices.title", "Devices", "设备"),
            ("connection.identity.name_field", "Visible name", "显示名称"),
            (
                "connection.identity.name_help",
                "This name is visible to nearby Envoix users.",
                "附近的 Envoix 用户会看到这个名称。"
            ),
            ("connection.identity.name_title", "Device name", "设备名称"),
        ]

        for (key, english, chinese) in labels {
            XCTAssertEqual(AppText.localized(key, language: "en"), english, key)
            XCTAssertEqual(AppText.localized(key, language: "zh-Hans"), chinese, key)
        }
    }

    func testRememberedDeviceFormatsAndStatusesCoverBoundaryInputs() {
        XCTAssertEqual(
            ConnectionHubPresentationText.rememberedDeviceCount(-1, language: "en"),
            "0 remembered"
        )
        XCTAssertEqual(
            ConnectionHubPresentationText.rememberedDeviceCount(3, language: "zh-Hans"),
            "已记住 3 台"
        )
        XCTAssertEqual(
            ConnectionHubPresentationText.pendingItemCount(1, language: "en"),
            "1 item ready. Choose a device to send it."
        )
        XCTAssertEqual(
            ConnectionHubPresentationText.pendingItemCount(2, language: "en"),
            "2 items ready. Choose a device to send them."
        )
        XCTAssertEqual(
            ConnectionHubPresentationText.pendingItemCount(2, language: "zh-Hans"),
            "已有 2 个项目，请选择发送设备。"
        )

        let statuses: [(RememberedRoomConnectionStatus, String, String)] = [
            (.offline, "Available when both apps are open", "双方打开应用时可连接"),
            (.connecting, "Connecting…", "正在连接…"),
            (.waiting, "Available to the other device…", "正在等待另一台设备…"),
            (.connected, "Connected", "已连接"),
            (.needsRepair("expired"), "Pair again to reconnect", "请重新配对后连接"),
        ]
        for (status, english, chinese) in statuses {
            XCTAssertEqual(
                ConnectionHubPresentationText.rememberedRoomStatus(status, language: "en"),
                english
            )
            XCTAssertEqual(
                ConnectionHubPresentationText.rememberedRoomStatus(status, language: "zh-Hans"),
                chinese
            )
        }
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
