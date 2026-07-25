import XCTest

final class ManifestV2AppUITests: XCTestCase {
    private var app: XCUIApplication!

    override func setUp() {
        super.setUp()
        continueAfterFailure = false
    }

    func testEnglishSendAndReceiveExposeCanonicalInventoryControls() {
        launch(
            language: "en",
            locale: "en_US",
            extraArguments: ["--ui-testing-discovery-fixtures"]
        )
        assertCanonicalInventoryControls(
            expectedScanLabel: "Scan QR",
            expectedFileLabel: "Files",
            expectedFolderLabel: "Folder",
            expectedPairingGuidance: "Show your receive QR, or scan the other device's send QR."
        )
    }

    func testSimplifiedChineseSendAndReceiveExposeCanonicalInventoryControls() {
        launch(
            language: "zh-Hans",
            locale: "zh_CN",
            extraArguments: ["--ui-testing-discovery-fixtures"]
        )
        assertCanonicalInventoryControls(
            expectedScanLabel: "扫描二维码",
            expectedFileLabel: "文件",
            expectedFolderLabel: "文件夹",
            expectedPairingGuidance: "可以显示本机接收码，也可以扫描另一台设备的发送码。"
        )
    }

    func testNearbySendHandoffKeepsEverySourcePickerUsable() {
        launch(
            language: "en",
            locale: "en_US",
            extraArguments: ["--ui-testing-discovery-fixtures"]
        )

        element("connection_hub").assertExists()
        button("nearby_peer_card").tap()
        element("one_time_room").assertExists()
        element("room_context_unverified").assertExists()
        button("room_add_files").tap()

        element("send_content_scroll").assertExists()
        element("nearby_transfer_context").assertExists()
        XCTAssertFalse(element("nearby_invite_delivery_progress").exists)

        assertPickerCanOpen(from: "send_photo_picker")
        assertPickerCanOpen(from: "send_file_picker")
        // A fresh simulator has no deterministic folder in Recents. Opening
        // and canceling still verifies that the room preserves every picker
        // handoff without pretending that an unavailable folder was selected.
        assertPickerCanOpen(from: "send_folder_picker")
    }

    func testActivityAndSettingsAreSeparatePages() {
        launch(
            language: "en",
            locale: "en_US",
            extraArguments: ["--ui-testing-discovery-fixtures"]
        )

        element("connection_hub").assertExists()
        button("open_activity").tap()
        element("activity_page").assertExists()
        button("mobile_page_back").tap()
        element("connection_hub").assertExists()

        button("open_settings").tap()
        element("settings_page").assertExists()
        button("mobile_page_back").tap()
        element("connection_hub").assertExists()

        button("nearby_peer_card").tap()
        element("one_time_room").assertExists()
        button("open_activity").tap()
        element("activity_page").assertExists()
        button("mobile_page_back").tap()
        element("one_time_room").assertExists()

        button("open_settings").tap()
        element("settings_page").assertExists()
        button("mobile_page_back").tap()
        element("one_time_room").assertExists()
    }

    func testUnverifiedNearbyOfferRequiresExplicitAcceptance() {
        launch(
            language: "en",
            locale: "en_US",
            extraArguments: [
                "--ui-testing-discovery-fixtures",
                "--ui-testing-incoming-nearby-offer",
            ]
        )

        element("connection_hub").assertExists()
        XCTAssertFalse(element("one_time_room").exists)
        let accept = app.buttons["Accept"].firstMatch
        accept.assertExists()
        accept.tap()

        element("one_time_room").assertExists()
        element("room_context_unverified").assertExists()
        element("receive_content_scroll").assertExists()
    }

    private func launch(
        language: String,
        locale: String,
        extraArguments: [String] = []
    ) {
        app = XCUIApplication()
        app.launchArguments = [
            "--ui-testing",
            "-envoix.language", language,
            "-AppleLanguages", "(\(language))",
            "-AppleLocale", locale,
        ] + extraArguments
        app.launch()
    }

    private func assertCanonicalInventoryControls(
        expectedScanLabel: String,
        expectedFileLabel: String,
        expectedFolderLabel: String,
        expectedPairingGuidance: String
    ) {
        element("connection_hub").assertExists()
        let scanButton = button("connect_scan_qr")
        scanButton.assertExists()
        XCTAssertEqual(scanButton.label, expectedScanLabel)
        button("connect_show_qr").assertExists()
        button("connect_enter_code").assertExists()

        button("nearby_peer_card").tap()
        element("one_time_room").assertExists()
        element("room_one_time_notice").assertExists()

        button("room_add_files").tap()
        element("send_content_scroll").assertExists()
        element("send_photo_picker").assertExists()
        let filePicker = element("send_file_picker")
        filePicker.assertExists()
        XCTAssertEqual(filePicker.label, expectedFileLabel)
        let folderPicker = element("send_folder_picker")
        folderPicker.assertExists()
        XCTAssertEqual(folderPicker.label, expectedFolderLabel)
        element("send_selection_limit").assertExists()
        // A new one-time room never adopts another room's unstarted draft.
        element("send_start_button").assertExists()

        button("mobile_sheet_done").tap()
        element("one_time_room").assertExists()

        button("room_receive_files").tap()
        element("receive_content_scroll").assertExists()
        element("receive_destination_picker").assertExists()
        let pairingGuidance = element("receive_pairing_guidance")
        pairingGuidance.assertExists()
        XCTAssertEqual(pairingGuidance.label, expectedPairingGuidance)
        element("receive_start_button").assertExists()
    }

    private func assertPickerCanOpen(from identifier: String) {
        let source = button(identifier)
        source.assertExists()
        XCTAssertTrue(source.isEnabled)
        XCTAssertTrue(source.isHittable)
        source.tap()

        // Document pickers do not expose a stable accessibility identifier
        // for this system button on every supported iOS release.
        let dismissal = app.buttons.matching(
            NSPredicate(format: "label == %@", "Cancel")
        ).firstMatch
        dismissal.assertExists()
        dismissal.tap()
        element("nearby_transfer_context").assertExists()
    }

    private func element(_ identifier: String) -> XCUIElement {
        app.descendants(matching: .any)[identifier]
    }

    private func button(_ identifier: String) -> XCUIElement {
        app.buttons.matching(identifier: identifier).firstMatch
    }
}

private extension XCUIElement {
    func assertExists(timeout: TimeInterval = 5, file: StaticString = #filePath, line: UInt = #line) {
        XCTAssertTrue(waitForExistence(timeout: timeout), "Missing UI element: \(identifier)", file: file, line: line)
    }
}
