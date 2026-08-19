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

    func testNearbyCatalogKeysPreserveEnglishAndChineseLabels() {
        let labels = [
            ("connection.identity.visible_as", "Visible as", "显示为"),
            ("connection.nearby.aware", "Aware", "感知"),
            (
                "connection.nearby.macos_note",
                "Discovery uses Bluetooth and the local network. Wi‑Fi Aware and NFC phone scanning are not available on macOS.",
                "通过蓝牙和局域网发现设备；macOS 暂不支持 Wi‑Fi Aware 和手机 NFC 扫描。"
            ),
            (
                "connection.nearby.open_bluetooth_settings",
                "Open Bluetooth settings",
                "打开蓝牙设置"
            ),
            (
                "connection.nearby.peer.fallback",
                "Nearby Envoix device",
                "附近的 Envoix 设备"
            ),
            ("connection.nearby.title", "Nearby devices", "附近设备"),
            ("connection.nearby.try_again", "Try again", "重试"),
            (
                "connection.nearby.wifi_aware.detail",
                "Pair once using Apple's system controls. Paired Envoix devices are then discovered automatically when both apps are open.",
                "使用 Apple 系统控件完成一次配对。之后双方打开 Envoix 时，已配对设备会被自动发现。"
            ),
            ("connection.nearby.wifi_aware.title", "Wi‑Fi Aware", "Wi‑Fi Aware"),
        ]

        for (key, english, chinese) in labels {
            XCTAssertEqual(AppText.localized(key, language: "en"), english, key)
            XCTAssertEqual(AppText.localized(key, language: "zh-Hans"), chinese, key)
        }
    }

    func testNearbyPresentationCoversVisibilityAvailabilityAndTrust() {
        XCTAssertEqual(nearbyStatus(.hidden), "Nearby off")
        XCTAssertEqual(nearbyStatus(.everyoneTenMinutes), "Nearby on")
        XCTAssertEqual(nearbyStatus(.whileAppOpen), "Nearby on")
        XCTAssertEqual(visibilityOption(.hidden), "Turn Nearby off")
        XCTAssertEqual(visibilityOption(.everyoneTenMinutes), "On for 10 minutes")
        XCTAssertEqual(visibilityOption(.whileAppOpen), "On while app is open")

        XCTAssertEqual(nearbyEmpty(isActive: false, ready: true), "Nearby is paused.")
        XCTAssertEqual(nearbyEmpty(isActive: true, ready: false), "Nearby is unavailable.")
        XCTAssertEqual(nearbyEmpty(isActive: true, ready: true), "Looking for devices…")

        XCTAssertEqual(peerHint(true), "Open an unverified one-time room")
        XCTAssertEqual(peerHint(false), "Waiting for a secure invitation path")
        XCTAssertEqual(peerTrust(available: false, requiresTap: false), "Invitation path not ready")
        XCTAssertEqual(peerTrust(available: true, requiresTap: true), "Tap to verify")
        XCTAssertEqual(peerTrust(available: true, requiresTap: false), "Unverified")
    }

    func testDiscoverySourcesAreStableAndLocalized() {
        XCTAssertEqual(
            ConnectionHubPresentationText.discoverySources([], language: "en"),
            "Discovery path unavailable"
        )
        XCTAssertEqual(
            ConnectionHubPresentationText.discoverySources(
                [.bluetooth, .mdns, .wifiAware],
                language: "en"
            ),
            "Discovered via Bluetooth · Local network · Wi‑Fi Aware"
        )
        XCTAssertEqual(
            ConnectionHubPresentationText.discoverySources(
                [.bluetooth, .mdns],
                language: "zh-Hans"
            ),
            "发现路径：蓝牙 · 局域网"
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

    private func nearbyStatus(_ visibility: NearbyVisibilityMode) -> String {
        ConnectionHubPresentationText.nearbyStatus(
            visibility: visibility,
            language: "en"
        )
    }

    private func visibilityOption(_ visibility: NearbyVisibilityMode) -> String {
        ConnectionHubPresentationText.visibilityOption(visibility, language: "en")
    }

    private func nearbyEmpty(isActive: Bool, ready: Bool) -> String {
        ConnectionHubPresentationText.nearbyEmptyState(
            isActive: isActive,
            hasReadyProvider: ready,
            language: "en"
        )
    }

    private func peerHint(_ available: Bool) -> String {
        ConnectionHubPresentationText.peerInvitationHint(
            isAvailable: available,
            language: "en"
        )
    }

    private func peerTrust(available: Bool, requiresTap: Bool) -> String {
        ConnectionHubPresentationText.peerTrust(
            invitationAvailable: available,
            requiresTapToVerify: requiresTap,
            language: "en"
        )
    }
}
