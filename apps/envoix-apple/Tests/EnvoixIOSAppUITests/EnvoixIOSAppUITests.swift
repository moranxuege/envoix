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

        XCTAssertTrue(app.buttons["stage_transfer"].waitForExistence(timeout: 8))
        XCTAssertTrue(app.buttons["stage_activity"].exists)
        XCTAssertTrue(app.buttons["stage_settings"].exists)

        XCTAssertTrue(app.buttons["transfer_role_send"].waitForExistence(timeout: 5))
        XCTAssertTrue(app.buttons["transfer_role_receive"].exists)

        app.buttons["transfer_role_send"].tap()

        XCTAssertTrue(app.buttons["send_file_picker"].isHittable)
        XCTAssertTrue(app.buttons["send_start_button"].exists)
        XCTAssertFalse(app.buttons["send_start_button"].isEnabled)

        app.buttons["transfer_role_receive"].tap()

        XCTAssertTrue(app.descendants(matching: .any)["receive_room_code"].waitForExistence(timeout: 5))
        XCTAssertTrue(app.buttons["receive_start_button"].exists)
        XCTAssertTrue(app.buttons["receive_start_button"].isEnabled)
        XCTAssertTrue(app.buttons["receive_start_button"].isHittable)
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

        XCTAssertTrue(app.buttons["stage_activity"].waitForExistence(timeout: 8))
        XCTAssertTrue(
            app.descendants(matching: .any)["activity_title_ui-transferring"].waitForExistence(timeout: 5)
        )

        XCTAssertTrue(app.buttons["activity_pause_ui-transferring"].exists)
        XCTAssertTrue(app.buttons["activity_cancel_ui-transferring"].exists)
        XCTAssertFalse(app.buttons["activity_resume_ui-transferring"].exists)
        XCTAssertFalse(app.buttons["activity_delete_ui-transferring"].exists)

        XCTAssertTrue(app.buttons["activity_resume_ui-paused"].exists)
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

        XCTAssertTrue(app.buttons["app_upload_diagnostics"].exists)
        let details = app.buttons["activity_details_ui-transferring"]
        XCTAssertTrue(details.isHittable)
        details.tap()
        XCTAssertTrue(app.staticTexts["Developer details"].waitForExistence(timeout: 3))
        XCTAssertTrue(app.staticTexts["Activity ID"].exists)
    }
}
