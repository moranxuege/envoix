import XCTest

final class EnvoixIOSAppUITests: XCTestCase {
    private static let folderPickerRoomCode = "741205-silver-forest"
    private static let rendezvousBroker = "e946a31a2207efcd68b9dbf409c4bf241aa02a0cbc0028af2e1ed11472064eff@67.230.187.238:8445"
    private static let relayURL = "https://envoix.chkxwlyh.us:8444"
    private static let crossDeviceTimeout: TimeInterval = 180
    private static let physicalNearbyTimeout: TimeInterval = 90
    private static let defaultPhysicalNearbyHold: TimeInterval = 20
    private static let maximumPhysicalNearbyHold: TimeInterval = 120
    private static let physicalWifiAwareProbeAttemptTimeout: TimeInterval = 35
    private static let physicalWifiAwareProbeAttemptLimit = 4
    private static let physicalWifiAwareProbeScrollLimit = 12
    private static let physicalWifiAwareProbePollInterval: TimeInterval = 0.25
    private static let iOS26DocumentPickerOpenPosition = CGVector(dx: 0.86, dy: 0.11)

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
        XCTAssertTrue(app.buttons["send_nearby_devices"].isHittable)
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

    func testNearbyDiscoveryPageMergesFixtureSources() throws {
        let app = XCUIApplication()
        app.launchArguments += ["--ui-testing", "--ui-testing-discovery-fixtures"]
        app.launch()
        dismissSheetIfNeeded(app)

        let nearbyEntry = app.buttons["home_nearby"]
        XCTAssertTrue(nearbyEntry.waitForExistence(timeout: 8))
        nearbyEntry.tap()

        XCTAssertTrue(app.descendants(matching: .any)["nearby_screen"].waitForExistence(timeout: 5))
        for identifier in ["nearby_provider_bluetooth", "nearby_provider_mdns", "nearby_provider_wifi_aware"] {
            XCTAssertTrue(app.descendants(matching: .any)[identifier].exists, identifier)
        }
        XCTAssertTrue(app.descendants(matching: .any)["nearby_peer_card"].waitForExistence(timeout: 5))
        XCTAssertTrue(app.staticTexts["Nearby test device"].exists)
        XCTAssertTrue(app.staticTexts["BLE"].exists)
        XCTAssertTrue(app.staticTexts["mDNS"].exists)

        app.buttons["nearby_peer_card"].tap()
        XCTAssertTrue(app.descendants(matching: .any)["nearby_pairing_context"].waitForExistence(timeout: 3))
        XCTAssertTrue(app.descendants(matching: .any)["nearby_pairing_security"].exists)
        XCTAssertTrue(app.buttons["nearby_pairing_send"].exists)
        XCTAssertTrue(app.buttons["nearby_pairing_receive"].exists)

        app.buttons["nearby_pairing_receive"].tap()
        XCTAssertTrue(app.descendants(matching: .any)["receive_pairing_guidance"].waitForExistence(timeout: 3))
    }

    func testPhysicalWifiAwarePairingSurvivesAppRelaunch() throws {
        guard ProcessInfo.processInfo.environment["ENVOIX_PHYSICAL_WIFI_AWARE"] == "1" else {
            throw XCTSkip("Requires an explicitly paired Wi-Fi Aware device")
        }

        let app = XCUIApplication()
        app.launchArguments.append("--ui-testing")

        for launchIndex in 0..<2 {
            app.launch()
            dismissSheetIfNeeded(app)

            let nearbyEntry = app.buttons["home_nearby"]
            XCTAssertTrue(nearbyEntry.waitForExistence(timeout: 8), "launch \(launchIndex + 1)")
            nearbyEntry.tap()

            XCTAssertTrue(
                app.descendants(matching: .any)["nearby_wifi_aware_device"]
                    .waitForExistence(timeout: 20),
                "The system-paired Wi-Fi Aware device was missing after app launch \(launchIndex + 1)"
            )
            XCTAssertTrue(
                app.descendants(matching: .any)["nearby_wifi_aware_persistence"].exists
            )
            app.terminate()
        }
    }

    func testPhysicalWifiAwarePairingRemovalClearsSnapshot() throws {
        guard ProcessInfo.processInfo.environment["ENVOIX_PHYSICAL_WIFI_AWARE_REMOVED"] == "1" else {
            throw XCTSkip("Requires explicit removal of the Wi-Fi Aware pairing")
        }

        let app = XCUIApplication()
        app.launchArguments.append("--ui-testing")
        app.launch()
        defer { app.terminate() }
        dismissSheetIfNeeded(app)

        let nearbyEntry = app.buttons["home_nearby"]
        XCTAssertTrue(nearbyEntry.waitForExistence(timeout: 8))
        nearbyEntry.tap()

        XCTAssertTrue(
            app.descendants(matching: .any)["nearby_wifi_aware_empty"]
                .waitForExistence(timeout: 20),
            "The removed Wi-Fi Aware pairing remained in the provider snapshot"
        )
        XCTAssertFalse(app.descendants(matching: .any)["nearby_wifi_aware_device"].exists)
    }

    func testPhysicalWifiAwareProbeUsesWifiAwarePath() throws {
        let environment = ProcessInfo.processInfo.environment
        guard let role = environment["ENVOIX_PHYSICAL_WIFI_AWARE_PROBE_ROLE"] else {
            throw XCTSkip("Requires an explicit Wi-Fi Aware publisher or subscriber role")
        }

        let actionIdentifier: String
        switch role {
        case "publisher":
            actionIdentifier = "settings_wifi_aware_probe_receive"
        case "subscriber":
            actionIdentifier = "settings_wifi_aware_probe_send"
        default:
            XCTFail("ENVOIX_PHYSICAL_WIFI_AWARE_PROBE_ROLE must be publisher or subscriber")
            return
        }

        let app = XCUIApplication()
        app.launchArguments += [
            "--ui-testing",
            "-envoix.language", "en",
            "-envoix.developerMode", "YES",
        ]
        app.launch()
        defer { app.terminate() }
        dismissSheetIfNeeded(app)

        let settings = app.buttons["open_settings"]
        XCTAssertTrue(settings.waitForExistence(timeout: 8))
        settings.tap()

        let settingsScroll = app.scrollViews.firstMatch
        XCTAssertTrue(settingsScroll.waitForExistence(timeout: 5))
        let status = app.descendants(matching: .any)["settings_wifi_aware_probe"]
        for _ in 0..<Self.physicalWifiAwareProbeScrollLimit where !status.exists {
            settingsScroll.swipeUp()
        }
        XCTAssertTrue(status.waitForExistence(timeout: 20))
        XCTAssertTrue(
            app.descendants(matching: .any)["settings_wifi_aware_probe_target"]
                .waitForExistence(timeout: 20),
            "The paired Wi-Fi Aware target was not available"
        )

        let action = app.buttons[actionIdentifier]
        for _ in 0..<Self.physicalWifiAwareProbeScrollLimit where !action.isHittable {
            settingsScroll.swipeUp()
        }
        XCTAssertTrue(action.waitForExistence(timeout: 5))
        XCTAssertTrue(action.isEnabled)
        XCTAssertTrue(action.isHittable)
        FileHandle.standardError.write(Data("ENVOIX_WIFI_AWARE_PROBE_READY role=\(role)\n".utf8))

        var lastSummary = status.label
        for _ in 0..<Self.physicalWifiAwareProbeAttemptLimit {
            action.tap()
            let deadline = Date().addingTimeInterval(Self.physicalWifiAwareProbeAttemptTimeout)
            var attemptStarted = false
            repeat {
                lastSummary = status.label
                if !lastSummary.contains("phase=succeeded") {
                    attemptStarted = true
                }
                if attemptStarted,
                   lastSummary.contains("phase=succeeded"),
                   lastSummary.contains("path=wifi_aware") {
                    let evidence = XCTAttachment(screenshot: app.screenshot())
                    evidence.name = "wifi-aware-probe-\(role)"
                    evidence.lifetime = .keepAlways
                    add(evidence)
                    return
                }
                if attemptStarted, lastSummary.contains("phase=failed") {
                    break
                }
                RunLoop.current.run(
                    until: Date().addingTimeInterval(Self.physicalWifiAwareProbePollInterval)
                )
            } while Date() < deadline
        }

        attachFailureState(of: app, named: "wifi-aware-probe-\(role)")
        XCTFail("Wi-Fi Aware probe did not succeed for \(role): \(lastSummary)")
    }

    func testPhysicalNearbyDiscoveryFindsAndroid() throws {
        let environment = ProcessInfo.processInfo.environment
        guard environment["ENVOIX_PHYSICAL_NEARBY"] == "1" else {
            throw XCTSkip("Requires an explicit physical Android/iPhone discovery run")
        }

        let expectedName = (environment["ENVOIX_PHYSICAL_NEARBY_ANDROID_NAME"] ?? "25060RK16C")
            .trimmingCharacters(in: .whitespacesAndNewlines)
        guard !expectedName.isEmpty else {
            XCTFail("ENVOIX_PHYSICAL_NEARBY_ANDROID_NAME must be non-empty")
            return
        }
        let rawHoldSeconds = environment["ENVOIX_PHYSICAL_NEARBY_HOLD_SECONDS"]
            ?? String(Self.defaultPhysicalNearbyHold)
        guard let holdSeconds = TimeInterval(rawHoldSeconds),
              (0...Self.maximumPhysicalNearbyHold).contains(holdSeconds) else {
            XCTFail("ENVOIX_PHYSICAL_NEARBY_HOLD_SECONDS must be between 0 and 120")
            return
        }
        let app = XCUIApplication()
        app.launchArguments += ["--ui-testing", "-envoix.language", "en"]

        let permissionMonitor = addUIInterruptionMonitor(withDescription: "Nearby permissions") { alert in
            for label in ["Allow", "允许", "OK", "好"] where alert.buttons[label].exists {
                alert.buttons[label].tap()
                return true
            }
            return false
        }
        defer { removeUIInterruptionMonitor(permissionMonitor) }

        app.launch()
        app.tap()
        dismissSheetIfNeeded(app)

        let nearbyEntry = app.buttons["home_nearby"]
        XCTAssertTrue(nearbyEntry.waitForExistence(timeout: 8))
        nearbyEntry.tap()
        XCTAssertTrue(app.descendants(matching: .any)["nearby_screen"].waitForExistence(timeout: 8))

        let card = app.descendants(matching: .any)
            .matching(identifier: "nearby_peer_card")
            .containing(.staticText, identifier: expectedName)
            .firstMatch
        let permissionPump = app.staticTexts["NEARBY DEVICES"]
        let deadline = Date().addingTimeInterval(Self.physicalNearbyTimeout)
        var converged = false
        repeat {
            if permissionPump.exists {
                permissionPump.tap()
            }
            let labels = card.staticTexts
            if card.exists, labels["BLE"].exists, labels["mDNS"].exists {
                converged = true
                break
            }
            RunLoop.current.run(until: Date().addingTimeInterval(0.5))
        } while Date() < deadline

        guard converged else {
            attachFailureState(of: app, named: "physical-nearby-discovery")
            XCTFail(
                "Android discovery card did not converge on BLE and mDNS; "
                    + "ensure both devices share the same Wi-Fi"
            )
            return
        }

        let evidence = XCTAttachment(screenshot: app.screenshot())
        evidence.name = "physical-nearby-android-ble-mdns"
        evidence.lifetime = .keepAlways
        add(evidence)
        FileHandle.standardError.write(Data("ENVOIX_PHYSICAL_NEARBY_READY\n".utf8))
        if environment["ENVOIX_PHYSICAL_NEARBY_EXPECT_INVITE"] == "1" {
            let pairingContext = app.descendants(matching: .any)["nearby_pairing_context"]
            XCTAssertTrue(
                pairingContext.waitForExistence(timeout: holdSeconds),
                "Android BLE invitation did not open the iPhone pairing confirmation"
            )
            XCTAssertTrue(app.buttons["nearby_pairing_send"].isEnabled)
            XCTAssertFalse(app.buttons["nearby_pairing_receive"].isEnabled)
            let inviteEvidence = XCTAttachment(screenshot: app.screenshot())
            inviteEvidence.name = "physical-android-to-ios-ble-invite"
            inviteEvidence.lifetime = .keepAlways
            add(inviteEvidence)
            FileHandle.standardError.write(Data("ENVOIX_PHYSICAL_BLE_INVITE_RECEIVED\n".utf8))
        } else {
            RunLoop.current.run(until: Date().addingTimeInterval(holdSeconds))
        }
    }

    func testScannerAppliesReceiverQRToSendRoomCode() throws {
        let roomCode = "741205-silver-forest"
        let app = makeScannerTestApp(payload: pairingPayload(code: roomCode, role: "receive"))

        app.launch()
        dismissSheetIfNeeded(app)
        openInjectedScanner(
            homeButton: "home_send",
            scrollView: "send_content_scroll",
            scannerButton: "send_scan_receiver_qr",
            in: app
        )
        app.buttons["qr_scanner_test_payload"].tap()

        let roomCodeField = app.textFields["send_room_code_input"]
        XCTAssertTrue(roomCodeField.waitForExistence(timeout: 5))
        XCTAssertEqual(roomCodeField.value as? String, roomCode)
        XCTAssertFalse(app.staticTexts["Advanced pairing"].exists)
        XCTAssertFalse(app.buttons["Token"].exists)
    }

    func testScannerAppliesSenderQRToReceiveRoomCode() throws {
        let roomCode = "741205-silver-forest"
        let app = makeScannerTestApp(payload: pairingPayload(code: roomCode, role: "send"))

        app.launch()
        dismissSheetIfNeeded(app)
        openInjectedScanner(
            homeButton: "home_receive",
            scrollView: "receive_content_scroll",
            scannerButton: "receive_scan_sender_qr",
            in: app
        )
        app.buttons["qr_scanner_test_payload"].tap()

        let roomCodeField = app.textFields["receive_join_room_code_input"]
        XCTAssertTrue(roomCodeField.waitForExistence(timeout: 5))
        XCTAssertEqual(roomCodeField.value as? String, roomCode)
    }

    func testScannerSwitchesSendScreenToReceiveForSenderQR() throws {
        let roomCode = "741205-silver-forest"
        let app = makeScannerTestApp(payload: pairingPayload(code: roomCode, role: "send"))

        app.launch()
        dismissSheetIfNeeded(app)
        openInjectedScanner(
            homeButton: "home_send",
            scrollView: "send_content_scroll",
            scannerButton: "send_scan_receiver_qr",
            in: app
        )
        app.buttons["qr_scanner_test_payload"].tap()

        let receiveCodeField = app.textFields["receive_join_room_code_input"]
        XCTAssertTrue(receiveCodeField.waitForExistence(timeout: 8))
        XCTAssertEqual(receiveCodeField.value as? String, roomCode)
        XCTAssertFalse(app.scrollViews["send_content_scroll"].exists)
    }

    func testScannerSwitchesReceiveScreenToSendForReceiverQR() throws {
        let roomCode = "741205-silver-forest"
        let app = makeScannerTestApp(payload: pairingPayload(code: roomCode, role: "receive"))

        app.launch()
        dismissSheetIfNeeded(app)
        openInjectedScanner(
            homeButton: "home_receive",
            scrollView: "receive_content_scroll",
            scannerButton: "receive_scan_sender_qr",
            in: app
        )
        app.buttons["qr_scanner_test_payload"].tap()

        let sendCodeField = app.textFields["send_room_code_input"]
        XCTAssertTrue(sendCodeField.waitForExistence(timeout: 8))
        XCTAssertEqual(sendCodeField.value as? String, roomCode)
        XCTAssertFalse(app.scrollViews["receive_content_scroll"].exists)
    }

    func testScannerKeepsInvalidQRVisible() throws {
        let app = makeScannerTestApp(payload: "https://example.com/not-an-envoix-code")

        app.launch()
        dismissSheetIfNeeded(app)
        openInjectedScanner(
            homeButton: "home_send",
            scrollView: "send_content_scroll",
            scannerButton: "send_scan_receiver_qr",
            in: app
        )
        let testPayload = app.buttons["qr_scanner_test_payload"]
        testPayload.tap()

        XCTAssertTrue(app.descendants(matching: .any)["qr_scanner_error"].waitForExistence(timeout: 5))
        XCTAssertTrue(testPayload.exists)
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
                // Scroll children underneath the fixed action bar are not visible yet.
                return element.frame.intersects(primaryButton.frame)
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

    func testCompletedReceiveOffersOpenFileAction() throws {
        let app = XCUIApplication()
        app.launchArguments += [
            "--ui-testing",
            "--ui-testing-activity-fixtures",
            "--ui-testing-start-activity",
        ]
        app.launch()

        let details = app.buttons["activity_details_ui-completed"]
        for _ in 0..<6 where !details.isHittable {
            app.swipeUp()
        }
        XCTAssertTrue(details.waitForExistence(timeout: 3))
        details.tap()

        let openFile = app.buttons["activity_open_received_ui-completed"]
        for _ in 0..<6 where !openFile.isHittable {
            app.swipeUp()
        }
        XCTAssertTrue(openFile.waitForExistence(timeout: 3))
        XCTAssertTrue(openFile.isHittable)
    }

    func testCompletedFolderCanBeBrowsedRecursively() throws {
        let app = XCUIApplication()
        app.launchArguments += [
            "--ui-testing",
            "--ui-testing-received-folder-fixture",
            "--ui-testing-start-activity",
        ]
        app.launch()

        let details = app.buttons["activity_details_ui-completed-folder"]
        for _ in 0..<6 where !details.isHittable {
            app.swipeUp()
        }
        XCTAssertTrue(details.waitForExistence(timeout: 3))
        details.tap()

        let openItems = app.buttons["activity_open_received_ui-completed-folder"]
        for _ in 0..<6 where !openItems.isHittable {
            app.swipeUp()
        }
        XCTAssertTrue(openItems.waitForExistence(timeout: 3))
        openItems.tap()

        let album = app.buttons["received_folder_open_Album"]
        XCTAssertTrue(album.waitForExistence(timeout: 3))
        XCTAssertTrue(album.isHittable)
        album.tap()
        XCTAssertTrue(app.navigationBars["Album"].waitForExistence(timeout: 3))

        let nested = app.buttons["received_folder_open_Nested"]
        XCTAssertTrue(nested.waitForExistence(timeout: 3))
        XCTAssertTrue(nested.isHittable)
        nested.tap()
        XCTAssertTrue(app.navigationBars["Nested"].waitForExistence(timeout: 3))

        let note = app.buttons["received_item_open_note.txt"]
        XCTAssertTrue(note.waitForExistence(timeout: 3))
        XCTAssertTrue(note.isHittable)
        XCTAssertEqual(note.label, "note.txt")
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

    func testDeletionKeepsActivityVisibleWhenDurableAcknowledgementStalls() throws {
        let app = XCUIApplication()
        app.launchArguments += [
            "--ui-testing",
            "--ui-testing-activity-fixtures",
            "--ui-testing-start-activity",
            "--ui-testing-stalled-activity-removal",
        ]
        app.launch()

        let title = app.descendants(matching: .any)["activity_title_ui-completed"]
        let delete = app.buttons["activity_delete_ui-completed"]
        XCTAssertTrue(title.waitForExistence(timeout: 8))
        XCTAssertTrue(delete.waitForExistence(timeout: 3))
        delete.tap()

        let pending = app.descendants(matching: .any)["activity_removing_ui-completed"]
        XCTAssertTrue(pending.waitForExistence(timeout: 2))
        XCTAssertTrue(title.exists)
        XCTAssertTrue(delete.waitForExistence(timeout: 7))
        XCTAssertTrue(title.exists)
        XCTAssertFalse(pending.exists)
        XCTAssertTrue(delete.isHittable)
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

        guard app.buttons["mobile_sheet_done"].waitForExistence(timeout: 5) else {
            attachFailureState(of: app, named: "activity-capsule-sheet")
            XCTFail("Tapping the active transfer capsule did not present Activity")
            return
        }
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

    func testPendingMultiShareReplacesExistingActivitySheetAfterForeground() throws {
        let app = XCUIApplication()
        app.launchArguments += [
            "--ui-testing",
            "--ui-testing-stage-multi-share-on-background",
        ]
        app.launch()

        dismissSheetIfNeeded(app)
        XCTAssertTrue(app.buttons["open_activity"].waitForExistence(timeout: 8))
        app.buttons["open_activity"].tap()
        XCTAssertTrue(app.buttons["mobile_sheet_done"].waitForExistence(timeout: 5))

        XCUIDevice.shared.press(.home)
        Thread.sleep(forTimeInterval: 1)
        app.activate()

        let selection = app.descendants(matching: .any)["send_selection_summary"]
        XCTAssertTrue(selection.waitForExistence(timeout: 8))
        XCTAssertEqual(selection.value as? String, "2")
    }

    func testSystemOpenedFilePresentsSendSelection() throws {
        guard #available(iOS 16.4, *) else {
            throw XCTSkip("Opening a document through XCUITest requires iOS 16.4 or newer")
        }

        let runID = "openinui"
        let fileName = "envoix-\(runID)-open-in.txt"
        let app = XCUIApplication()
        app.launchArguments += ["--ui-testing", "--ui-testing-open-in-payload"]
        app.launchEnvironment["ENVOIX_CROSS_DEVICE_RUN_ID"] = runID
        defer { cleanOpenInPayloadFixture(app: app, runID: runID) }
        guard let sourceURL = stageOpenInPayloadFixture(app: app) else { return }

        app.open(sourceURL)

        XCTAssertTrue(app.buttons["mobile_sheet_done"].waitForExistence(timeout: 8))
        let selection = app.descendants(matching: .any)["send_selection_summary"]
        XCTAssertTrue(selection.waitForExistence(timeout: 5))
        let selectedFile = NSPredicate(format: "label CONTAINS %@", fileName)
        let selectionExpectation = XCTNSPredicateExpectation(predicate: selectedFile, object: selection)
        XCTAssertEqual(
            XCTWaiter.wait(for: [selectionExpectation], timeout: 3),
            .completed,
            "Unexpected selection label: \(selection.label)"
        )
    }

    func testNearbyDevicesFromSendPreservesOpenedFileSelection() throws {
        guard #available(iOS 16.4, *) else {
            throw XCTSkip("Opening a document through XCUITest requires iOS 16.4 or newer")
        }

        let runID = "nearby-draft"
        let fileName = "envoix-\(runID)-open-in.txt"
        let app = XCUIApplication()
        app.launchArguments += [
            "--ui-testing",
            "--ui-testing-discovery-fixtures",
            "--ui-testing-open-in-payload",
        ]
        app.launchEnvironment["ENVOIX_CROSS_DEVICE_RUN_ID"] = runID
        defer { cleanOpenInPayloadFixture(app: app, runID: runID) }
        guard let sourceURL = stageOpenInPayloadFixture(app: app) else { return }

        app.open(sourceURL)

        let initialSelection = app.descendants(matching: .any)["send_selection_summary"]
        XCTAssertTrue(initialSelection.waitForExistence(timeout: 8))
        XCTAssertTrue(initialSelection.label.contains(fileName), initialSelection.label)

        let nearby = app.buttons["send_nearby_devices"]
        XCTAssertTrue(nearby.waitForExistence(timeout: 5))
        nearby.tap()
        XCTAssertTrue(app.descendants(matching: .any)["nearby_screen"].waitForExistence(timeout: 5))
        app.buttons["mobile_sheet_done"].tap()

        XCTAssertTrue(app.buttons["home_send"].waitForExistence(timeout: 5))
        app.buttons["home_send"].tap()
        let restoredSelection = app.descendants(matching: .any)["send_selection_summary"]
        XCTAssertTrue(restoredSelection.waitForExistence(timeout: 5))
        XCTAssertTrue(restoredSelection.label.contains(fileName), restoredSelection.label)
    }

    func testSystemOpenedFileSendsToMacOSApp() throws {
#if !ENVOIX_CROSS_DEVICE_TESTING
        throw XCTSkip("Requires the explicit cross-device build and a macOS production App receiver")
#else
        guard #available(iOS 16.4, *) else {
            throw XCTSkip("Opening a document through XCUITest requires iOS 16.4 or newer")
        }

        let runID = ProcessInfo.processInfo.environment["ENVOIX_CROSS_DEVICE_RUN_ID"] ?? "manual"
        let roomCode = ProcessInfo.processInfo.environment["ENVOIX_IOS_TO_MACOS_CODE"]
            ?? Self.folderPickerRoomCode
        let fileName = "envoix-\(runID)-open-in.txt"
        let app = makeCrossDeviceSenderApp(
            runID: runID,
            pickerArguments: ["--ui-testing-open-in-payload"]
        )
        defer { cleanOpenInPayloadFixture(app: app, runID: runID) }
        guard let sourceURL = stageOpenInPayloadFixture(app: app) else { return }

        app.open(sourceURL)

        XCTAssertTrue(app.buttons["mobile_sheet_done"].waitForExistence(timeout: 8))
        let selection = app.descendants(matching: .any)["send_selection_summary"]
        XCTAssertTrue(selection.waitForExistence(timeout: 5))
        XCTAssertEqual(selection.value as? String, "1")
        XCTAssertTrue(selection.label.contains(fileName), selection.label)

        startRoomSend(code: roomCode, in: app)
        openActivity(in: app)
        waitForLatestActivityCompletion(
            description: "Open In transfer",
            in: app,
            timeout: Self.crossDeviceTimeout
        )
#endif
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

    func testShareExtensionStagesTwoFilesFromFilesHost() throws {
        #if ENVOIX_CROSS_DEVICE_TESTING
        let runID = ProcessInfo.processInfo.environment["ENVOIX_CROSS_DEVICE_RUN_ID"]
            ?? "sharehost"
        #else
        let runID = "sharehost"
        #endif
        #if ENVOIX_CROSS_DEVICE_TESTING
        let roomCode = ProcessInfo.processInfo.environment["ENVOIX_IOS_TO_MACOS_CODE"]
            ?? Self.folderPickerRoomCode
        #endif
        let fileNames = [
            "envoix-\(runID)-file-first.txt",
            "envoix-\(runID)-file-second.txt",
        ]
        #if ENVOIX_CROSS_DEVICE_TESTING
        let app = makeCrossDeviceSenderApp(
            runID: runID,
            pickerArguments: [
                "--ui-testing-file-picker",
                "--ui-testing-file-payload",
            ]
        )
        #else
        let app = XCUIApplication()
        app.launchArguments += [
            "--ui-testing",
            "--ui-testing-file-picker",
            "--ui-testing-file-payload",
        ]
        app.launchEnvironment["ENVOIX_CROSS_DEVICE_RUN_ID"] = runID
        #endif
        defer { cleanFilePayloadFixture(app: app, runID: runID) }
        let files = XCUIApplication(bundleIdentifier: "com.apple.DocumentsApp")
        files.terminate()

        app.launch()
        app.tap()
        dismissSheetIfNeeded(app)
        XCTAssertTrue(app.buttons["home_send"].waitForExistence(timeout: 8))

        XCUIDevice.shared.press(.home)
        files.activate()
        guard openFilesFolder(
            containing: "envoix-\(runID)-file-first",
            in: [files]
        ) else {
            XCTFail("The Files host did not expose the Envoix fixture folder")
            return
        }
        guard let more = firstHittableButton(
            named: ["More", "更多"],
            in: [files],
            timeout: 5
        ) else {
            XCTFail("The Files host did not expose its More action")
            return
        }
        more.tap()
        guard let select = firstHittableButton(
            named: ["Select", "选择"],
            in: [files],
            timeout: 5
        ) else {
            XCTFail("The Files host did not expose its Select action")
            return
        }
        select.tap()
        for fileName in fileNames {
            let visibleName = URL(fileURLWithPath: fileName).deletingPathExtension().lastPathComponent
            guard let file = firstHittableElement(
                containing: visibleName,
                in: [files],
                timeout: 5
            ) else {
                XCTFail("The Files host did not expose \(fileName) in selection mode")
                return
            }
            file.tap()
        }
        guard let share = firstHittableButton(
            named: ["Share", "共享"],
            in: [files],
            timeout: 5
        ) else {
            XCTFail("The Files host did not expose its Share action")
            return
        }
        share.tap()
        let springboard = XCUIApplication(bundleIdentifier: "com.apple.springboard")
        let shareSheetApplications = [files, springboard]
        guard let appRow = firstShareApplicationRow(in: files, timeout: 8) else {
            XCTFail("The share sheet did not expose its application row")
            return
        }
        var envoix = firstHittableShareCell(named: ["Envoix"], in: files)
        var moreShareCell: XCUIElement?
        for _ in 0..<12 where envoix == nil && moreShareCell == nil {
            appRow.swipeLeft()
            envoix = firstHittableShareCell(named: ["Envoix"], in: files)
            moreShareCell = firstHittableShareCell(named: ["More", "更多"], in: files)
        }
        if envoix != nil {
            guard tapSettledShareCell(
                named: ["Envoix"],
                in: files,
                within: appRow,
                timeout: 5
            ) else {
                XCTFail("The share sheet did not expose a stable Envoix extension cell")
                return
            }
        } else if moreShareCell != nil {
            guard tapSettledShareCell(
                named: ["More", "更多"],
                in: files,
                within: appRow,
                timeout: 5
            ) else {
                XCTFail("The share sheet did not expose a stable More extension cell")
                return
            }
            guard let envoix = firstHittableElement(
                containing: "Envoix",
                in: shareSheetApplications,
                timeout: 8
            ) else {
                XCTFail("The share sheet activity list did not expose Envoix")
                return
            }
            envoix.tap()
        } else {
            XCTFail("The share sheet did not expose the Envoix extension")
            return
        }

        let shareExtension = XCUIApplication(bundleIdentifier: "com.envoix.app.ios.share")
        let extensionApplications = [shareExtension, files, springboard]
        guard let ready = firstHittableElement(
            identifier: "share_status_title",
            in: extensionApplications,
            timeout: 20
        ) else {
            XCTFail("The Envoix Share Extension did not finish staging the selection")
            return
        }
        XCTAssertTrue(
            ready.label == "Ready in Envoix" || ready.label == "已在 Envoix 中准备好",
            ready.label
        )
        guard let done = firstHittableElement(
            identifier: "share_primary_action",
            in: extensionApplications,
            timeout: 5
        ) else {
            XCTFail("The Envoix Share Extension did not expose its Done action")
            return
        }
        done.tap()

        app.activate()
        let selection = app.descendants(matching: .any)["send_selection_summary"]
        XCTAssertTrue(selection.waitForExistence(timeout: 10))
        XCTAssertEqual(selection.value as? String, "2")

        #if ENVOIX_CROSS_DEVICE_TESTING
        startRoomSend(code: roomCode, in: app)
        openActivity(in: app)
        waitForLatestActivityCompletion(
            description: "Files Share Extension transfer",
            in: app,
            timeout: Self.crossDeviceTimeout
        )
        #else
        let close = app.buttons["mobile_sheet_done"]
        XCTAssertTrue(close.waitForExistence(timeout: 5))
        close.tap()
        XCTAssertTrue(app.buttons["home_send"].waitForExistence(timeout: 5))
        #endif
    }

    func testFolderPickerSendsCurrentDirectoryToMacOSApp() throws {
#if !ENVOIX_CROSS_DEVICE_TESTING
        throw XCTSkip("Requires the explicit cross-device build and a macOS production App receiver")
#else
        let runID = ProcessInfo.processInfo.environment["ENVOIX_CROSS_DEVICE_RUN_ID"] ?? "manual"
        let roomCode = ProcessInfo.processInfo.environment["ENVOIX_IOS_TO_MACOS_CODE"]
            ?? Self.folderPickerRoomCode
        let folderName = "envoix-\(runID)-folder"
        let app = makeCrossDeviceSenderApp(
            runID: runID,
            pickerArguments: [
                "--ui-testing-folder-picker",
                "--ui-testing-folder-payload",
            ]
        )
        defer { cleanFolderPayloadFixture(app: app, runID: runID) }

        app.launch()
        app.tap()
        dismissSheetIfNeeded(app)
        XCTAssertTrue(app.buttons["home_send"].waitForExistence(timeout: 8))
        app.buttons["home_send"].tap()

        XCTAssertTrue(app.buttons["send_folder_picker"].waitForExistence(timeout: 5))
        guard openCurrentFolder(named: folderName, in: app) else { return }
        let selection = app.descendants(matching: .any)["send_selection_summary"]
        XCTAssertTrue(selection.waitForExistence(timeout: 8))
        XCTAssertEqual(selection.value as? String, "1")
        XCTAssertTrue(selection.label.contains(folderName), selection.label)

        startRoomSend(code: roomCode, in: app)
        openActivity(in: app)
        waitForLatestActivityCompletion(
            description: "Folder picker transfer",
            in: app,
            timeout: Self.crossDeviceTimeout
        )
#endif
    }

    func testFilePickerSendsTwoFilesToMacOSApp() throws {
#if !ENVOIX_CROSS_DEVICE_TESTING
        throw XCTSkip("Requires the explicit cross-device build and a macOS production App receiver")
#else
        let runID = ProcessInfo.processInfo.environment["ENVOIX_CROSS_DEVICE_RUN_ID"] ?? "manual"
        let roomCode = ProcessInfo.processInfo.environment["ENVOIX_IOS_TO_MACOS_CODE"]
            ?? Self.folderPickerRoomCode
        let fileNames = [
            "envoix-\(runID)-file-first.txt",
            "envoix-\(runID)-file-second.txt",
        ]
        let app = makeCrossDeviceSenderApp(
            runID: runID,
            pickerArguments: [
                "--ui-testing-file-picker",
                "--ui-testing-file-payload",
            ]
        )
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

        startRoomSend(code: roomCode, in: app)
        openActivity(in: app)
        waitForLatestActivityCompletion(
            description: "Files picker transfer",
            in: app,
            timeout: Self.crossDeviceTimeout
        )
#endif
    }

    private func dismissSheetIfNeeded(_ app: XCUIApplication) {
        let close = app.buttons["mobile_sheet_done"]
        for _ in 0..<3 {
            guard close.waitForExistence(timeout: 1) else { return }
            guard close.isHittable else {
                RunLoop.current.run(until: Date().addingTimeInterval(0.2))
                continue
            }
            close.tap()
            if close.waitForNonExistence(timeout: 2) { return }
        }
        XCTFail("The previous mobile sheet did not close")
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
                guard application.state != .notRunning else { continue }
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

    private func openFilesFolder(
        containing fixtureLabel: String,
        in applications: [XCUIApplication]
    ) -> Bool {
        if firstHittableElement(containing: fixtureLabel, in: applications, timeout: 1) != nil {
            return true
        }

        guard openEnvoixDocuments(in: applications) else { return false }
        return firstHittableElement(
            containing: fixtureLabel,
            in: applications,
            timeout: 8
        ) != nil
    }

    private func openEnvoixDocuments(in applications: [XCUIApplication]) -> Bool {
        firstHittableButton(
            named: ["Browse", "浏览"],
            in: applications,
            timeout: 3
        )?.tap()

        if firstHittableButton(
            named: ["Open", "打开"],
            in: applications,
            timeout: 1
        ) != nil {
            return true
        }

        for locationLabel in ["On My iPhone", "在我的 iPhone 上"] {
            if let location = firstHittableElement(
                containing: locationLabel,
                in: applications,
                timeout: 1
            ) {
                location.tap()
                break
            }
        }

        if firstHittableButton(
            named: ["Open", "打开"],
            in: applications,
            timeout: 1
        ) != nil {
            return true
        }

        let predicate = NSPredicate(format: "label == 'Envoix' OR label BEGINSWITH 'Envoix,'")
        let deadline = Date().addingTimeInterval(5)
        repeat {
            for application in applications where application.state != .notRunning {
                if let appFolder = application.cells.matching(predicate)
                    .allElementsBoundByIndex
                    .first(where: { $0.exists && $0.isHittable }) {
                    appFolder.tap()
                    return true
                }
            }
            RunLoop.current.run(until: Date().addingTimeInterval(0.1))
        } while Date() < deadline
        return false
    }

    private func openCurrentFolder(named folderName: String? = nil, in app: XCUIApplication) -> Bool {
        app.buttons["send_folder_picker"].tap()
        let files = XCUIApplication(bundleIdentifier: "com.apple.DocumentsApp")
        let springboard = XCUIApplication(bundleIdentifier: "com.apple.springboard")
        let applications = [app, files, springboard]
        if #available(iOS 26.0, *) {
            if let folderName {
                guard let folder = firstHittableElement(
                    containing: folderName,
                    in: applications,
                    timeout: 8
                ) else {
                    XCTFail("The folder picker did not expose \(folderName)")
                    return false
                }
                folder.tap()
            }
            // iOS 26 does not expose the visible picker action reliably to XCUITest.
            // The caller verifies that this selects the current folder before proceeding.
            app.coordinate(withNormalizedOffset: Self.iOS26DocumentPickerOpenPosition).tap()
            return true
        }

        guard let open = firstHittableButton(
            named: ["Open", "打开"],
            in: applications,
            timeout: 8
        ) else {
            attachFailureState(of: app, named: "folder-picker-open")
            XCTFail("The folder picker did not expose its system Open action")
            return false
        }
        open.tap()
        return true
    }

    private func attachFailureState(of app: XCUIApplication, named: String) {
        let screenshot = XCTAttachment(screenshot: app.screenshot())
        screenshot.name = "\(named)-screenshot"
        screenshot.lifetime = .keepAlways
        add(screenshot)

        let hierarchy = XCTAttachment(string: app.debugDescription)
        hierarchy.name = "\(named)-hierarchy"
        hierarchy.lifetime = .keepAlways
        add(hierarchy)
    }

    private func selectFiles(named fileNames: [String], in app: XCUIApplication) -> Bool {
        app.buttons["send_file_picker"].tap()
        let files = XCUIApplication(bundleIdentifier: "com.apple.DocumentsApp")
        let springboard = XCUIApplication(bundleIdentifier: "com.apple.springboard")
        let applications = [app, files, springboard]
        let firstVisibleName = URL(fileURLWithPath: fileNames[0])
            .deletingPathExtension()
            .lastPathComponent
        guard openFilesFolder(containing: firstVisibleName, in: applications) else {
            XCTFail("The Files picker did not expose the Envoix fixture folder")
            return false
        }
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
                guard application.state != .notRunning else { continue }
                let elements = application.descendants(matching: .any)
                    .matching(predicate)
                    .allElementsBoundByIndex
                if let element = elements.first(where: { $0.exists && $0.isHittable }) {
                    return element
                }
            }
            RunLoop.current.run(until: Date().addingTimeInterval(0.1))
        } while Date() < deadline
        return nil
    }

    private func firstHittableShareCell(
        named labels: [String],
        in application: XCUIApplication
    ) -> XCUIElement? {
        let predicate = NSPredicate(
            format: "identifier == 'shareCell' AND label IN %@",
            labels
        )
        return application.cells.matching(predicate).allElementsBoundByIndex.first {
            $0.exists && $0.isHittable
        }
    }

    private func firstShareApplicationRow(
        in application: XCUIApplication,
        timeout: TimeInterval
    ) -> XCUIElement? {
        let shareCell = NSPredicate(format: "identifier == 'shareCell'")
        let deadline = Date().addingTimeInterval(timeout)
        repeat {
            if application.state != .notRunning,
               let row = application.scrollViews.allElementsBoundByIndex.first(where: {
                   $0.descendants(matching: .cell).matching(shareCell).count > 0
               }) {
                return row
            }
            RunLoop.current.run(until: Date().addingTimeInterval(0.1))
        } while Date() < deadline
        return nil
    }

    private func tapSettledShareCell(
        named labels: [String],
        in application: XCUIApplication,
        within row: XCUIElement,
        timeout: TimeInterval
    ) -> Bool {
        let minimumVisibleRatio = 0.9
        let positionTolerance = 1.0
        let deadline = Date().addingTimeInterval(timeout)
        var previousFrame: CGRect?

        repeat {
            if let element = firstHittableShareCell(named: labels, in: application) {
                let frame = element.frame
                let visibleWidth = frame.intersection(row.frame).width
                let isFullyVisible = frame.width > 0
                    && visibleWidth / frame.width >= minimumVisibleRatio
                let isSettled = previousFrame.map {
                    abs($0.minX - frame.minX) <= positionTolerance
                        && abs($0.minY - frame.minY) <= positionTolerance
                } ?? false
                if isFullyVisible, isSettled {
                    element.coordinate(
                        withNormalizedOffset: CGVector(dx: 0.5, dy: 0.5)
                    ).tap()
                    return true
                }
                previousFrame = isFullyVisible ? frame : nil
            } else {
                previousFrame = nil
            }
            RunLoop.current.run(until: Date().addingTimeInterval(0.2))
        } while Date() < deadline
        return false
    }

    private func firstHittableElement(
        identifier: String,
        in applications: [XCUIApplication],
        timeout: TimeInterval
    ) -> XCUIElement? {
        let deadline = Date().addingTimeInterval(timeout)
        repeat {
            for application in applications {
                guard application.state != .notRunning else { continue }
                let element = application.descendants(matching: .any)[identifier]
                if element.exists, element.isHittable {
                    return element
                }
            }
            RunLoop.current.run(until: Date().addingTimeInterval(0.1))
        } while Date() < deadline
        return nil
    }

    private func pairingPayload(code: String, role: String) -> String {
        "envoix://pair/\(code)?" +
            "broker=e946a31a2207efcd68b9dbf409c4bf241aa02a0cbc0028af2e1ed11472064eff" +
            "%4067.230.187.238%3A8445&" +
            "relay=https%3A%2F%2Fenvoix.chkxwlyh.us%3A8444&role=\(role)"
    }

    private func makeScannerTestApp(payload: String) -> XCUIApplication {
        let app = XCUIApplication()
        app.launchArguments = [
            "--ui-testing",
            "--ui-testing-scanner",
            "-envoix.language", "en",
            "-envoix.developerMode", "YES",
        ]
        app.launchEnvironment["ENVOIX_UI_TEST_SCAN_PAYLOAD"] = payload
        return app
    }

    private func openInjectedScanner(
        homeButton: String,
        scrollView: String,
        scannerButton: String,
        in app: XCUIApplication
    ) {
        let homeEntry = app.buttons[homeButton]
        guard homeEntry.waitForExistence(timeout: 8) else {
            XCTFail("Missing transfer entry: \(homeButton)")
            return
        }
        homeEntry.tap()

        let scroll = app.scrollViews[scrollView]
        guard scroll.waitForExistence(timeout: 5) else {
            XCTFail("Missing transfer scroll view: \(scrollView)")
            return
        }
        let scanChoice = app.buttons["Scan a QR"]
        guard reveal(scanChoice, byScrolling: scroll) else {
            XCTFail("Scan choice is not reachable")
            return
        }
        scanChoice.tap()

        let openScanner = app.buttons[scannerButton]
        guard openScanner.waitForExistence(timeout: 5) else {
            XCTFail("Missing scanner button: \(scannerButton)")
            return
        }
        openScanner.tap()
        XCTAssertTrue(app.buttons["qr_scanner_test_payload"].waitForExistence(timeout: 5))
    }

    private func makeCrossDeviceSenderApp(
        runID: String,
        pickerArguments: [String]
    ) -> XCUIApplication {
        let app = XCUIApplication()
        app.launchArguments = [
            "--ui-testing",
            "-envoix.language", "en",
            "-envoix.serverURL", Self.rendezvousBroker,
            "-envoix.relayURL", Self.relayURL,
            "-envoix.useRoom", "YES",
            "-envoix.useMdns", "YES",
        ] + pickerArguments
        app.launchEnvironment["ENVOIX_CROSS_DEVICE_RUN_ID"] = runID
        return app
    }

    private func startRoomSend(code: String, in app: XCUIApplication) {
        let codeField = app.textFields["send_room_code_input"]
        let scrollView = app.scrollViews["send_content_scroll"]
        XCTAssertTrue(scrollView.waitForExistence(timeout: 5))
        guard revealRoomCodeField(codeField, in: scrollView) else {
            XCTFail(
                "Room code field is not reachable in the Send scroll viewport; " +
                    "field=\(codeField.frame) scroll=\(scrollView.frame) app=\(app.frame)"
            )
            return
        }
        codeField.tap()
        codeField.typeText(code)

        let send = app.buttons["send_start_button"]
        XCTAssertTrue(send.isEnabled)
        XCTAssertTrue(send.isHittable)
        send.tap()
    }

    private func revealRoomCodeField(
        _ field: XCUIElement,
        in scrollView: XCUIElement
    ) -> Bool {
        let maximumAttempts = 8

        for _ in 0..<maximumAttempts {
            if isFullyVisible(field, in: scrollView) {
                return true
            }
            if field.frame.midY < scrollView.frame.midY {
                scrollView.swipeDown()
            } else {
                scrollView.swipeUp()
            }
        }
        return isFullyVisible(field, in: scrollView)
    }

    private func isFullyVisible(
        _ element: XCUIElement,
        in container: XCUIElement
    ) -> Bool {
        guard element.exists, element.isHittable else { return false }
        let elementFrame = element.frame
        let visibleFrame = elementFrame.intersection(container.frame)
        let tolerance: CGFloat = 1
        return !visibleFrame.isNull
            && visibleFrame.width >= elementFrame.width - tolerance
            && visibleFrame.height >= elementFrame.height - tolerance
    }

    private func reveal(
        _ element: XCUIElement,
        byScrolling scrollView: XCUIElement
    ) -> Bool {
        let maximumAttempts = 8
        let dragStart = scrollView.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.8))
        let dragEnd = scrollView.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.2))
        for _ in 0..<maximumAttempts {
            if element.isHittable {
                return true
            }
            dragStart.press(forDuration: 0.05, thenDragTo: dragEnd)
        }
        return element.isHittable
    }

    private func openActivity(in app: XCUIApplication) {
        let close = app.buttons["mobile_sheet_done"]
        if close.exists { return }
        let activity = app.buttons["open_activity"]
        XCTAssertTrue(activity.waitForExistence(timeout: 20))
        activity.tap()
        XCTAssertTrue(close.waitForExistence(timeout: 8))
    }

    private func waitForLatestActivityCompletion(
        description: String,
        in app: XCUIApplication,
        timeout: TimeInterval
    ) {
        let activityIdentifier = NSPredicate(format: "identifier BEGINSWITH 'activity_title_'")
        let latest = app.descendants(matching: .any).matching(activityIdentifier).firstMatch
        XCTAssertTrue(latest.waitForExistence(timeout: 8), "\(description) did not create an Activity")
        let deadline = Date().addingTimeInterval(timeout)
        while Date() < deadline {
            let label = latest.label
            if label.localizedCaseInsensitiveContains("Done") || label.contains("完成") {
                return
            }
            if label.localizedCaseInsensitiveContains("Error") || label.contains("错误") {
                XCTFail("\(description) reached a failed Activity: \(label)")
                return
            }
            RunLoop.current.run(until: Date().addingTimeInterval(0.2))
        }
        XCTFail("\(description) did not complete within \(timeout) seconds")
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

    private func stageOpenInPayloadFixture(app: XCUIApplication) -> URL? {
        app.launch()
        let fixtureURL = app.staticTexts["open_in_fixture_url"]
        guard fixtureURL.waitForExistence(timeout: 8),
              let url = URL(string: fixtureURL.label),
              url.isFileURL else {
            XCTFail("Open In fixture was not staged inside the target App container")
            return nil
        }
        app.terminate()
        return url
    }

    private func cleanOpenInPayloadFixture(app: XCUIApplication, runID: String) {
        app.terminate()
        app.launchArguments = ["--ui-testing", "--ui-testing-clean-open-in-payload"]
        app.launchEnvironment = ["ENVOIX_CROSS_DEVICE_RUN_ID": runID]
        app.launch()
        _ = app.buttons["home_send"].waitForExistence(timeout: 5)
        app.terminate()
    }
}
