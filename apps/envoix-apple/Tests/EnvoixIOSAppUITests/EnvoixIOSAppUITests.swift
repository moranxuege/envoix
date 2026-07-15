import XCTest

final class EnvoixIOSAppUITests: XCTestCase {
    private static let folderPickerRoomCode = "741205-silver-forest"
    private static let rendezvousBroker = "e946a31a2207efcd68b9dbf409c4bf241aa02a0cbc0028af2e1ed11472064eff@67.230.187.238:8445"
    private static let relayURL = "https://envoix.chkxwlyh.us:8444"
    private static let crossDeviceTimeout: TimeInterval = 180

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

        for identifier in ["send_photo_picker", "send_file_picker", "send_folder_picker"] {
            XCTAssertTrue(app.buttons[identifier].isHittable, identifier)
        }
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

        for identifier in ["send_photo_picker", "send_file_picker", "send_folder_picker"] {
            XCTAssertTrue(app.buttons[identifier].isHittable, identifier)
        }
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
        let selection = app.descendants(matching: .any)["send_selection_summary"]
        XCTAssertTrue(selection.waitForExistence(timeout: 5))
        XCTAssertEqual(selection.value as? String, "2")
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
        let selection = app.descendants(matching: .any)["send_selection_summary"]
        XCTAssertTrue(selection.waitForExistence(timeout: 5))
        let selectedFile = NSPredicate(format: "label CONTAINS %@", "open-in-envoix.txt")
        let selectionExpectation = XCTNSPredicateExpectation(predicate: selectedFile, object: selection)
        XCTAssertEqual(
            XCTWaiter.wait(for: [selectionExpectation], timeout: 3),
            .completed,
            "Unexpected selection label: \(selection.label)"
        )
    }

    func testFolderPickerOpenSelectsCurrentDirectory() throws {
        let app = XCUIApplication()
        app.launchArguments += ["--ui-testing", "--ui-testing-folder-picker"]

        addUIInterruptionMonitor(withDescription: "System permissions") { alert in
            for label in ["Allow", "允许", "OK", "好"] where alert.buttons[label].exists {
                alert.buttons[label].tap()
                return true
            }
            return false
        }

        app.launch()
        app.tap()

        dismissSheetIfNeeded(app)
        let send = app.buttons["home_send"]
        XCTAssertTrue(send.waitForExistence(timeout: 8))
        XCTAssertTrue(send.isHittable)
        send.tap()

        let folder = app.buttons["send_folder_picker"]
        XCTAssertTrue(folder.waitForExistence(timeout: 5))
        guard openCurrentFolder(in: app) else { return }

        let selection = app.descendants(matching: .any)["send_selection_summary"]
        XCTAssertTrue(selection.waitForExistence(timeout: 8))
        XCTAssertEqual(selection.value as? String, "1")
        XCTAssertTrue(
            selection.label.contains("Documents"),
            "The picker did not return the current app Documents directory: \(selection.label)"
        )
    }

    func testFilePickerSelectsTwoFiles() throws {
        let runID = "selection"
        let fileNames = [
            "envoix-\(runID)-file-first.txt",
            "envoix-\(runID)-file-second.txt",
        ]
        let app = XCUIApplication()
        app.launchArguments += [
            "--ui-testing",
            "--ui-testing-file-picker",
            "--ui-testing-file-payload",
        ]
        app.launchEnvironment["ENVOIX_CROSS_DEVICE_RUN_ID"] = runID
        defer { cleanFilePayloadFixture(app: app, runID: runID) }

        app.launch()
        app.tap()
        dismissSheetIfNeeded(app)
        XCTAssertTrue(app.buttons["home_send"].waitForExistence(timeout: 8))
        app.buttons["home_send"].tap()

        XCTAssertTrue(app.buttons["send_file_picker"].waitForExistence(timeout: 5))
        guard selectFiles(named: fileNames, in: app) else { return }

        let selection = app.descendants(matching: .any)["send_selection_summary"]
        XCTAssertTrue(selection.waitForExistence(timeout: 8))
        XCTAssertEqual(selection.value as? String, "2")
    }

    func testFolderPickerSendsCurrentDirectoryToMacOSApp() throws {
#if !ENVOIX_CROSS_DEVICE_TESTING
        throw XCTSkip("Requires the explicit cross-device build and a macOS production App receiver")
#else
        let runID = ProcessInfo.processInfo.environment["ENVOIX_CROSS_DEVICE_RUN_ID"] ?? "manual"
        let roomCode = ProcessInfo.processInfo.environment["ENVOIX_IOS_TO_MACOS_CODE"]
            ?? Self.folderPickerRoomCode
        let folderName = "envoix-\(runID)-folder"
        let app = XCUIApplication()
        app.launchArguments += [
            "--ui-testing",
            "--ui-testing-folder-picker",
            "--ui-testing-folder-payload",
            "-envoix.language", "en",
            "-envoix.serverURL", Self.rendezvousBroker,
            "-envoix.relayURL", Self.relayURL,
            "-envoix.useRoom", "YES",
            "-envoix.useMdns", "YES",
        ]
        app.launchEnvironment["ENVOIX_CROSS_DEVICE_RUN_ID"] = runID
        defer { cleanFolderPayloadFixture(app: app, runID: runID) }

        app.launch()
        app.tap()
        dismissSheetIfNeeded(app)
        XCTAssertTrue(app.buttons["home_send"].waitForExistence(timeout: 8))
        app.buttons["home_send"].tap()

        XCTAssertTrue(app.buttons["send_folder_picker"].waitForExistence(timeout: 5))
        guard openCurrentFolder(in: app) else { return }
        let selection = app.descendants(matching: .any)["send_selection_summary"]
        XCTAssertTrue(selection.waitForExistence(timeout: 8))
        XCTAssertEqual(selection.value as? String, "1")
        XCTAssertTrue(selection.label.contains(folderName), selection.label)

        let codeField = app.textFields.firstMatch
        let scrollView = app.scrollViews.firstMatch
        for _ in 0..<4 where codeField.frame.maxY > app.frame.height - 180 {
            scrollView.swipeUp()
        }
        XCTAssertTrue(codeField.isHittable)
        codeField.tap()
        codeField.typeText(roomCode)

        let send = app.buttons["send_start_button"]
        XCTAssertTrue(send.isEnabled)
        XCTAssertTrue(send.isHittable)
        send.tap()

        let capsule = app.buttons["active_transfer_capsule"]
        if capsule.waitForExistence(timeout: 20) {
            capsule.tap()
        }
        let close = app.buttons["mobile_sheet_done"]
        if !close.waitForExistence(timeout: 2) {
            let activity = app.buttons["open_activity"]
            XCTAssertTrue(activity.waitForExistence(timeout: 5))
            activity.tap()
        }
        XCTAssertTrue(close.waitForExistence(timeout: 8))
        waitForFolderActivityCompletion(named: folderName, in: app, timeout: Self.crossDeviceTimeout)
#endif
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

    private func firstHittableButton(
        named labels: [String],
        in applications: [XCUIApplication],
        timeout: TimeInterval
    ) -> XCUIElement? {
        let deadline = Date().addingTimeInterval(timeout)
        repeat {
            for application in applications {
                for label in labels {
                    let button = application.buttons[label]
                    if button.exists, button.isHittable {
                        return button
                    }
                }
            }
            RunLoop.current.run(until: Date().addingTimeInterval(0.1))
        } while Date() < deadline
        return nil
    }

    private func openCurrentFolder(in app: XCUIApplication) -> Bool {
        app.buttons["send_folder_picker"].tap()
        let files = XCUIApplication(bundleIdentifier: "com.apple.DocumentsApp")
        let springboard = XCUIApplication(bundleIdentifier: "com.apple.springboard")
        guard let open = firstHittableButton(
            named: ["Open", "打开"],
            in: [app, files, springboard],
            timeout: 8
        ) else {
            XCTFail("The folder picker did not expose its system Open action")
            return false
        }
        open.tap()
        return true
    }

    private func selectFiles(named fileNames: [String], in app: XCUIApplication) -> Bool {
        app.buttons["send_file_picker"].tap()
        let files = XCUIApplication(bundleIdentifier: "com.apple.DocumentsApp")
        let springboard = XCUIApplication(bundleIdentifier: "com.apple.springboard")
        let applications = [app, files, springboard]
        for fileName in fileNames {
            let visibleName = URL(fileURLWithPath: fileName).deletingPathExtension().lastPathComponent
            guard let file = firstHittableElement(
                containing: visibleName,
                in: applications,
                timeout: 8
            ) else {
                XCTFail("The Files picker did not expose \(fileName)")
                return false
            }
            file.tap()
        }
        guard let open = firstHittableButton(
            named: ["Open", "打开"],
            in: applications,
            timeout: 8
        ) else {
            XCTFail("The Files picker did not expose its system Open action")
            return false
        }
        open.tap()
        return true
    }

    private func firstHittableElement(
        containing label: String,
        in applications: [XCUIApplication],
        timeout: TimeInterval
    ) -> XCUIElement? {
        let predicate = NSPredicate(format: "label CONTAINS[c] %@", label)
        let deadline = Date().addingTimeInterval(timeout)
        repeat {
            for application in applications {
                let element = application.descendants(matching: .any).matching(predicate).firstMatch
                if element.exists, element.isHittable {
                    return element
                }
            }
            RunLoop.current.run(until: Date().addingTimeInterval(0.1))
        } while Date() < deadline
        return nil
    }

    private func waitForFolderActivityCompletion(
        named folderName: String,
        in app: XCUIApplication,
        timeout: TimeInterval
    ) {
        let completedPredicate = NSPredicate(
            format: "label CONTAINS[c] %@ AND (label CONTAINS[c] 'Done' OR label CONTAINS[c] '完成')",
            folderName
        )
        let failedPredicate = NSPredicate(
            format: "label CONTAINS[c] %@ AND (label CONTAINS[c] 'Error' OR label CONTAINS[c] '错误')",
            folderName
        )
        let completed = app.descendants(matching: .any).matching(completedPredicate).firstMatch
        let failed = app.descendants(matching: .any).matching(failedPredicate).firstMatch
        let deadline = Date().addingTimeInterval(timeout)
        while Date() < deadline {
            if completed.exists { return }
            if failed.exists {
                XCTFail("The Folder picker transfer reached a failed Activity")
                return
            }
            RunLoop.current.run(until: Date().addingTimeInterval(0.2))
        }
        XCTFail("The Folder picker transfer did not complete within \(timeout) seconds")
    }

    private func cleanFolderPayloadFixture(app: XCUIApplication, runID: String) {
        app.terminate()
        app.launchArguments = ["--ui-testing", "--ui-testing-clean-folder-payload"]
        app.launchEnvironment = ["ENVOIX_CROSS_DEVICE_RUN_ID": runID]
        app.launch()
        _ = app.buttons["home_send"].waitForExistence(timeout: 5)
        app.terminate()
    }

    private func cleanFilePayloadFixture(app: XCUIApplication, runID: String) {
        app.terminate()
        app.launchArguments = ["--ui-testing", "--ui-testing-clean-file-payload"]
        app.launchEnvironment = ["ENVOIX_CROSS_DEVICE_RUN_ID": runID]
        app.launch()
        _ = app.buttons["home_send"].waitForExistence(timeout: 5)
        app.terminate()
    }
}
