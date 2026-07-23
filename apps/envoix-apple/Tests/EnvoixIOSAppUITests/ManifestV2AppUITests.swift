import XCTest

final class ManifestV2AppUITests: XCTestCase {
    private var app: XCUIApplication!

    override func setUp() {
        super.setUp()
        continueAfterFailure = false
    }

    func testEnglishSendAndReceiveExposeCanonicalInventoryControls() {
        launch(language: "en", locale: "en_US")
        assertCanonicalInventoryControls(
            expectedFileLabel: "Files",
            expectedFolderLabel: "Folder",
            expectedPairingGuidance: "Show your receive QR, or scan the other device's send QR."
        )
    }

    func testSimplifiedChineseSendAndReceiveExposeCanonicalInventoryControls() {
        launch(language: "zh-Hans", locale: "zh_CN")
        assertCanonicalInventoryControls(
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

        element("transfer_home").assertExists()
        button("home_nearby").tap()
        element("nearby_screen").assertExists()
        button("nearby_peer_card").tap()
        element("nearby_pairing_context").assertExists()
        button("nearby_pairing_send").tap()

        element("send_content_scroll").assertExists()
        element("nearby_transfer_context").assertExists()
        XCTAssertFalse(element("nearby_invite_delivery_progress").exists)

        assertPickerCanOpen(from: "send_photo_picker")
        assertPickerCanOpen(from: "send_file_picker")
        assertPickerCanOpen(from: "send_folder_picker", dismissWith: "Open")
        element("send_selection_summary").assertExists()
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
        expectedFileLabel: String,
        expectedFolderLabel: String,
        expectedPairingGuidance: String
    ) {
        element("transfer_home").assertExists()

        button("home_send").tap()
        element("send_content_scroll").assertExists()
        element("send_photo_picker").assertExists()
        let filePicker = element("send_file_picker")
        filePicker.assertExists()
        XCTAssertEqual(filePicker.label, expectedFileLabel)
        let folderPicker = element("send_folder_picker")
        folderPicker.assertExists()
        XCTAssertEqual(folderPicker.label, expectedFolderLabel)
        element("send_selection_limit").assertExists()
        // A prepared draft is intentionally restored across launches.
        element("send_start_button").assertExists()

        button("mobile_sheet_done").tap()
        element("transfer_home").assertExists()

        button("home_receive").tap()
        element("receive_content_scroll").assertExists()
        element("receive_destination_picker").assertExists()
        let pairingGuidance = element("receive_pairing_guidance")
        pairingGuidance.assertExists()
        XCTAssertEqual(pairingGuidance.label, expectedPairingGuidance)
        element("receive_start_button").assertExists()
    }

    private func assertPickerCanOpen(
        from identifier: String,
        dismissWith dismissalLabel: String = "Cancel"
    ) {
        let source = button(identifier)
        source.assertExists()
        XCTAssertTrue(source.isEnabled)
        XCTAssertTrue(source.isHittable)
        source.tap()

        let dismissal = app.buttons[dismissalLabel].firstMatch
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
