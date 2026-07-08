import XCTest

final class EnvoixIOSmokeTests: XCTestCase {
    override func setUpWithError() throws {
        continueAfterFailure = false
    }

    @MainActor
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

        XCTAssertTrue(app.tabBars.buttons["Transfer"].waitForExistence(timeout: 8))
        XCTAssertTrue(app.tabBars.buttons["Activity"].exists)
        XCTAssertTrue(app.tabBars.buttons["Settings"].exists)

        XCTAssertTrue(app.buttons["transfer_role_send"].waitForExistence(timeout: 5))
        XCTAssertTrue(app.buttons["send_file_picker"].exists)
        XCTAssertTrue(app.buttons["send_start_button"].exists)
        XCTAssertFalse(app.buttons["send_start_button"].isEnabled)

        app.buttons["transfer_role_receive"].tap()

        XCTAssertTrue(app.descendants(matching: .any)["receive_room_code"].waitForExistence(timeout: 5))
        XCTAssertTrue(app.buttons["receive_start_button"].exists)
        XCTAssertTrue(app.buttons["receive_start_button"].isEnabled)
    }
}
