import XCTest

final class EnvoixIOSAppUITests: XCTestCase {
    override func setUpWithError() throws {
        continueAfterFailure = false
    }

    func testTransferScreenShowsStableControls() throws {
        let app = XCUIApplication()
        app.launchArguments.append("--ui-testing")

        addUIInterruptionMonitor(withDescription: "System permissions") { alert in
            if alert.buttons["Allow"].exists {
                alert.buttons["Allow"].tap()
                return true
            }
            if alert.buttons["OK"].exists {
                alert.buttons["OK"].tap()
                return true
            }
            return false
        }

        app.launch()
        app.tap()

        dismissSheetIfNeeded(app)

        XCTAssertTrue(app.descendants(matching: .any)["transfer_home"].waitForExistence(timeout: 8))
        XCTAssertFalse(app.buttons["stage_transfer"].exists)
        XCTAssertFalse(app.buttons["stage_activity"].exists)
        XCTAssertTrue(app.buttons["open_activity"].exists)
        XCTAssertTrue(app.buttons["open_settings"].exists)

        let sendEntry = app.buttons["home_send"]
        let receiveEntry = app.buttons["home_receive"]
        XCTAssertTrue(sendEntry.waitForExistence(timeout: 5))
        XCTAssertTrue(receiveEntry.exists)

        sendEntry.tap()

        XCTAssertTrue(app.buttons["send_file_picker"].isHittable)
        XCTAssertTrue(app.descendants(matching: .any)["send_selection_limit"].exists)
        XCTAssertTrue(app.descendants(matching: .any)["send_pairing_guidance"].exists)
        XCTAssertTrue(app.descendants(matching: .any)["pairing_panel_selector"].exists)
        XCTAssertTrue(app.buttons["send_start_button"].exists)
        XCTAssertFalse(app.buttons["send_start_button"].isEnabled)

        app.buttons["mobile_sheet_done"].tap()
        XCTAssertTrue(app.buttons["home_receive"].waitForExistence(timeout: 3))
        app.buttons["home_receive"].tap()

        XCTAssertTrue(app.descendants(matching: .any)["receive_pairing_guidance"].exists)
        XCTAssertTrue(app.descendants(matching: .any)["receive_room_code"].waitForExistence(timeout: 5))
        XCTAssertTrue(app.buttons["receive_start_button"].exists)
        XCTAssertTrue(app.buttons["receive_start_button"].isEnabled)
        XCTAssertTrue(app.buttons["receive_start_button"].isHittable)
    }

    func testChineseDarkLayoutKeepsPrimaryActionsReachable() throws {
        let app = XCUIApplication()
        app.launchArguments += [
            "--ui-testing",
            "-envoix.language",
            "zh-Hans",
            "-envoix.appearance",
            "dark",
        ]
        app.launch()

        dismissSheetIfNeeded(app)

        XCTAssertTrue(app.staticTexts["传点东西"].waitForExistence(timeout: 8))
        let send = app.buttons["home_send"]
        XCTAssertTrue(send.exists)
        XCTAssertTrue(send.isHittable)
        send.tap()

        XCTAssertTrue(app.buttons["send_file_picker"].isHittable)
        XCTAssertTrue(app.buttons["send_start_button"].exists)
        app.buttons["mobile_sheet_done"].tap()

        let receive = app.buttons["home_receive"]
        if !receive.isHittable {
            app.swipeUp()
        }
        XCTAssertTrue(receive.isHittable)
        receive.tap()
        XCTAssertTrue(app.buttons["receive_start_button"].waitForExistence(timeout: 5))
        XCTAssertTrue(app.buttons["receive_start_button"].isHittable)
        app.buttons["mobile_sheet_done"].tap()

        XCTAssertTrue(app.buttons["open_settings"].waitForExistence(timeout: 3))
        app.buttons["open_settings"].tap()
        XCTAssertTrue(app.staticTexts["深色"].waitForExistence(timeout: 5))
    }

    func testPrimarySurfacesPassAccessibilityAudit() throws {
        guard #available(iOS 17.0, *) else {
            throw XCTSkip("XCUITest accessibility audits require iOS 17 or newer")
        }

        let app = XCUIApplication()
        app.launchArguments += [
            "--ui-testing",
            "-envoix.language",
            "en",
            "-envoix.appearance",
            "light",
            "-envoix.developerMode",
            "NO",
        ]
        app.launch()

        dismissSheetIfNeeded(app)
        XCTAssertTrue(app.descendants(matching: .any)["transfer_home"].waitForExistence(timeout: 8))
        try app.performAccessibilityAudit()

        for entry in ["home_send", "home_receive"] {
            app.buttons[entry].tap()
            XCTAssertTrue(app.buttons["mobile_sheet_done"].waitForExistence(timeout: 5))

            let copyIdentifier = entry == "home_send" ? "send_room_copy" : "receive_room_copy"
            let primaryIdentifier = entry == "home_send" ? "send_start_button" : "receive_start_button"
            let primaryButton = app.buttons[primaryIdentifier]
            try app.performAccessibilityAudit { issue in
                guard issue.auditType == .contrast, let element = issue.element else { return false }
                return element.label == "Copy" && element.frame.intersects(primaryButton.frame)
            }

            let copyButton = app.buttons[copyIdentifier]
            app.swipeUp()
            for _ in 0..<5 where !copyButton.isHittable {
                app.swipeUp()
            }
            XCTAssertTrue(copyButton.isHittable)
            XCTAssertFalse(copyButton.frame.intersects(primaryButton.frame))

            app.buttons["mobile_sheet_done"].tap()
            XCTAssertTrue(app.descendants(matching: .any)["transfer_home"].waitForExistence(timeout: 3))
        }

        for entry in ["open_activity", "open_settings"] {
            app.buttons[entry].tap()
            XCTAssertTrue(app.buttons["mobile_sheet_done"].waitForExistence(timeout: 5))
            try app.performAccessibilityAudit()
            app.buttons["mobile_sheet_done"].tap()
            XCTAssertTrue(app.descendants(matching: .any)["transfer_home"].waitForExistence(timeout: 3))
        }
    }

    func testActivityFixturesPassAccessibilityAudit() throws {
        guard #available(iOS 17.0, *) else {
            throw XCTSkip("XCUITest accessibility audits require iOS 17 or newer")
        }

        let app = XCUIApplication()
        app.launchArguments += [
            "--ui-testing",
            "--ui-testing-activity-fixtures",
            "--ui-testing-start-activity",
            "--ui-testing-accessibility-text",
            "-envoix.language",
            "en",
            "-envoix.appearance",
            "light",
            "-envoix.developerMode",
            "NO",
        ]
        app.launch()

        XCTAssertTrue(
            app.descendants(matching: .any)["activity_title_ui-transferring"].waitForExistence(timeout: 8)
        )
        try app.performAccessibilityAudit { issue in
            // This fixture passes the complete audit on an iPhone SE whose
            // system content size is accessibility-extra-extra-extra-large.
            // The regular-size simulator still predicts a larger future size,
            // even though this launch pins SwiftUI to `.accessibility5`.
            issue.auditType == .textClipped
                && issue.element?.label == "field-observations-and-design-notes.zip"
                && issue.detailedDescription.contains("larger Dynamic Type")
        }
    }

    func testActivityActionsMatchCanonicalLifecycle() throws {
        let app = XCUIApplication()
        app.launchArguments += [
            "--ui-testing",
            "--ui-testing-activity-fixtures",
            "--ui-testing-start-activity",
            "-envoix.developerMode",
            "YES",
        ]
        app.launch()

        XCTAssertTrue(app.buttons["mobile_sheet_done"].waitForExistence(timeout: 8))
        XCTAssertTrue(
            app.descendants(matching: .any)["activity_title_ui-transferring"].waitForExistence(timeout: 5)
        )

        XCTAssertTrue(app.buttons["activity_pause_ui-transferring"].exists)
        XCTAssertTrue(app.buttons["activity_cancel_ui-transferring"].exists)
        XCTAssertFalse(app.buttons["activity_resume_ui-transferring"].exists)
        XCTAssertFalse(app.buttons["activity_delete_ui-transferring"].exists)

        let parkedResume = app.buttons["activity_resume_ui-paused"]
        XCTAssertTrue(parkedResume.exists)
        XCTAssertFalse(parkedResume.isEnabled)
        XCTAssertTrue(app.buttons["activity_cancel_ui-paused"].exists)
        XCTAssertFalse(app.buttons["activity_pause_ui-paused"].exists)
        XCTAssertFalse(app.buttons["activity_delete_ui-paused"].exists)

        XCTAssertTrue(app.buttons["activity_delete_ui-completed"].exists)
        XCTAssertFalse(app.buttons["activity_pause_ui-completed"].exists)
        XCTAssertFalse(app.buttons["activity_resume_ui-completed"].exists)
        XCTAssertFalse(app.buttons["activity_cancel_ui-completed"].exists)

        XCTAssertTrue(app.buttons["activity_resume_ui-failed"].exists)
        XCTAssertTrue(app.buttons["activity_delete_ui-failed"].exists)
        XCTAssertFalse(app.buttons["activity_cancel_ui-failed"].exists)

        XCTAssertTrue(app.buttons["activity_choose_folder_ui-publish-failed"].exists)
        XCTAssertFalse(app.buttons["activity_resume_ui-publish-failed"].exists)
        XCTAssertTrue(app.buttons["activity_cancel_ui-publish-failed"].exists)

        XCTAssertTrue(app.buttons["app_upload_diagnostics"].exists)
        let details = app.buttons["activity_details_ui-transferring"]
        XCTAssertTrue(details.isHittable)
        details.tap()
        let developerDetails = app.descendants(matching: .any)["activity_developer_details_ui-transferring"]
        let maximumDetailScrollAttempts = 6
        for _ in 0..<maximumDetailScrollAttempts where !developerDetails.exists {
            app.swipeUp()
        }
        XCTAssertTrue(developerDetails.waitForExistence(timeout: 3))
        XCTAssertTrue(app.descendants(matching: .any)["activity_id_ui-transferring"].exists)
    }

    func testCancellingRecoversWhenStateAcknowledgementStalls() throws {
        let app = XCUIApplication()
        app.launchArguments += [
            "--ui-testing",
            "--ui-testing-activity-fixtures",
            "--ui-testing-start-activity",
            "--ui-testing-stalled-activity-command",
        ]
        app.launch()

        let cancel = app.buttons["activity_cancel_ui-transferring"]
        XCTAssertTrue(cancel.waitForExistence(timeout: 8))
        cancel.tap()

        let pending = app.descendants(matching: .any)["activity_command_ui-transferring"]
        XCTAssertTrue(pending.waitForExistence(timeout: 2))
        XCTAssertTrue(cancel.waitForExistence(timeout: 7))
        XCTAssertFalse(pending.exists)
        XCTAssertTrue(cancel.isHittable)
    }

    func testSingleHomeOpensActivitySheet() throws {
        let app = XCUIApplication()
        app.launchArguments += ["--ui-testing", "--ui-testing-activity-fixtures"]
        app.launch()

        dismissSheetIfNeeded(app)

        XCTAssertTrue(app.descendants(matching: .any)["transfer_home"].waitForExistence(timeout: 8))
        XCTAssertFalse(app.buttons["stage_transfer"].exists)
        XCTAssertFalse(app.buttons["stage_activity"].exists)

        app.buttons["open_activity"].tap()
        XCTAssertTrue(app.buttons["mobile_sheet_done"].waitForExistence(timeout: 5))
        XCTAssertTrue(
            app.descendants(matching: .any)["activity_title_ui-transferring"].waitForExistence(timeout: 5)
        )
        app.buttons["mobile_sheet_done"].tap()
        XCTAssertTrue(app.descendants(matching: .any)["transfer_home"].waitForExistence(timeout: 3))
    }

    func testDeveloperModeToggleRespondsImmediately() throws {
        let app = XCUIApplication()
        app.launchArguments.append("--ui-testing")
        app.launch()

        dismissSheetIfNeeded(app)

        let settings = app.buttons["open_settings"]
        XCTAssertTrue(settings.waitForExistence(timeout: 8))
        settings.tap()

        XCTAssertTrue(
            app.buttons["settings_clean_transfer_cache"].waitForExistence(timeout: 5)
        )
        let developerMode = app.buttons["settings_developer_mode"]
        XCTAssertTrue(developerMode.waitForExistence(timeout: 5))
        for _ in 0..<4 where !developerMode.isHittable {
            app.swipeUp()
        }
        XCTAssertTrue(developerMode.isHittable)
        guard let initialValue = developerMode.value as? String,
              initialValue == "On" || initialValue == "Off" else {
            XCTFail("Developer mode must expose an On/Off accessibility value")
            return
        }
        let toggledValue = initialValue == "On" ? "Off" : "On"

        developerMode.tap()
        waitForValue(toggledValue, of: developerMode)

        developerMode.tap()
        waitForValue(initialValue, of: developerMode)
    }

    func testActiveTransferCapsuleOpensActivity() throws {
        let app = XCUIApplication()
        app.launchArguments += ["--ui-testing", "--ui-testing-activity-fixtures"]
        app.launch()

        dismissSheetIfNeeded(app)

        let capsule = app.buttons["active_transfer_capsule"]
        XCTAssertTrue(capsule.waitForExistence(timeout: 8))
        capsule.tap()

        XCTAssertTrue(app.buttons["mobile_sheet_done"].waitForExistence(timeout: 5))
        XCTAssertTrue(
            app.descendants(matching: .any)["activity_title_ui-transferring"].waitForExistence(timeout: 5)
        )
        XCTAssertFalse(app.buttons["active_transfer_capsule"].exists)
    }

    func testPendingMultiShareOpensWhenAppReturnsToForeground() throws {
        let app = XCUIApplication()
        app.launchArguments += [
            "--ui-testing",
            "--ui-testing-stage-multi-share-on-background",
        ]
        app.launch()

        dismissSheetIfNeeded(app)
        XCTAssertTrue(app.buttons["home_send"].waitForExistence(timeout: 8))

        XCUIDevice.shared.press(.home)
        Thread.sleep(forTimeInterval: 1)
        app.activate()

        XCTAssertTrue(app.buttons["mobile_sheet_done"].waitForExistence(timeout: 8))
        let filePicker = app.buttons["send_file_picker"]
        XCTAssertTrue(filePicker.waitForExistence(timeout: 5))
        XCTAssertEqual(filePicker.value as? String, "2")
    }

    func testSystemOpenedFilePresentsSendSelection() throws {
        guard #available(iOS 16.4, *) else {
            throw XCTSkip("Opening a document through XCUITest requires iOS 16.4 or newer")
        }

        let sourceURL = FileManager.default.temporaryDirectory
            .appendingPathComponent("open-in-envoix.txt")
        try Data("system document-open fixture".utf8).write(to: sourceURL, options: .atomic)
        defer { try? FileManager.default.removeItem(at: sourceURL) }

        let app = XCUIApplication()
        app.launchArguments.append("--ui-testing")
        app.open(sourceURL)

        XCTAssertTrue(app.buttons["mobile_sheet_done"].waitForExistence(timeout: 8))
        let filePicker = app.buttons["send_file_picker"]
        XCTAssertTrue(filePicker.waitForExistence(timeout: 5))
        let selectedFile = NSPredicate(format: "label CONTAINS %@", "open-in-envoix.txt")
        let selectionExpectation = XCTNSPredicateExpectation(predicate: selectedFile, object: filePicker)
        XCTAssertEqual(
            XCTWaiter.wait(for: [selectionExpectation], timeout: 3),
            .completed,
            "Unexpected file picker label: \(filePicker.label)"
        )
    }

    private func dismissSheetIfNeeded(_ app: XCUIApplication) {
        let close = app.buttons["mobile_sheet_done"]
        if close.waitForExistence(timeout: 1) {
            close.tap()
        }
    }

    private func waitForValue(_ value: String, of element: XCUIElement) {
        let predicate = NSPredicate(format: "value == %@", value)
        let expectation = XCTNSPredicateExpectation(predicate: predicate, object: element)
        XCTAssertEqual(XCTWaiter.wait(for: [expectation], timeout: 3), .completed)
    }
}
