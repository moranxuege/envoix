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
            expectedFolderLabel: "Folder"
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
            expectedFolderLabel: "文件夹"
        )
    }

    func testNearbySendHandoffKeepsEverySourcePickerUsable() {
        for pickerIdentifier in [
            "send_photo_picker",
            "send_file_picker",
            "send_folder_picker",
        ] {
            launch(
                language: "en",
                locale: "en_US",
                extraArguments: ["--ui-testing-discovery-fixtures"]
            )
            openNearbySend()
            assertPickerCanPresent(from: pickerIdentifier)
            app.terminate()
        }
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
        let accept = app.alerts
            .descendants(matching: .any)
            .matching(NSPredicate(format: "label == %@", "Accept"))
            .firstMatch
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
        expectedFolderLabel: String
    ) {
        element("connection_hub").assertExists()
        let scanButton = button("connect_scan_qr")
        scanButton.assertExists()
        XCTAssertEqual(scanButton.label, expectedScanLabel)
        button("connect_enter_code").assertExists()
        element("room_qr_reveal").assertExists()
        element("nearby_display_name").assertExists()
        element("nearby_visibility_menu").assertExists()

        button("nearby_peer_card").tap()
        element("one_time_room").assertExists()
        element("room_context_authenticated").assertExists()

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
    }

    private func openNearbySend() {
        element("connection_hub").assertExists()
        button("nearby_peer_card").tap()
        element("one_time_room").assertExists()
        element("room_context_authenticated").assertExists()
        button("room_add_files").tap()
        element("send_content_scroll").assertExists()
        XCTAssertFalse(element("nearby_invite_delivery_progress").exists)
    }

    private func assertPickerCanPresent(from identifier: String) {
        let source = button(identifier)
        source.assertExists()
        XCTAssertTrue(source.isEnabled)
        XCTAssertTrue(source.isHittable)
        source.tap()

        // The iOS 26.5 folder document picker keeps the covered source marked
        // hittable while its remote view is presented. The app-owned sheet
        // identifier is the stable presentation contract for that path.
        let presented: XCTNSPredicateExpectation
        if identifier == "send_folder_picker" {
            presented = XCTNSPredicateExpectation(
                predicate: NSPredicate(format: "exists == true"),
                object: element("send_folder_picker_sheet")
            )
        } else {
            presented = XCTNSPredicateExpectation(
                predicate: NSPredicate(format: "exists == true AND hittable == false"),
                object: source
            )
        }
        XCTAssertEqual(
            XCTWaiter.wait(for: [presented], timeout: 5),
            .completed,
            "Picker did not cover source control: \(identifier)"
        )
    }

    private func element(_ identifier: String) -> XCUIElement {
        app.descendants(matching: .any)
            .matching(identifier: identifier)
            .firstMatch
    }

    private func button(_ identifier: String) -> XCUIElement {
        // Xcode 26 can report SwiftUI buttons as PopUpButton through modern
        // accessibility attributes while the legacy query still says Button.
        // Query by the app's stable identifier instead of that synthesized
        // subtype so the same test binary works across simulator runtimes.
        element(identifier)
    }
}

private extension XCUIElement {
    func assertExists(timeout: TimeInterval = 5, file: StaticString = #filePath, line: UInt = #line) {
        XCTAssertTrue(waitForExistence(timeout: timeout), "Missing UI element: \(identifier)", file: file, line: line)
    }
}
