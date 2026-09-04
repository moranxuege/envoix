import XCTest
@testable import Envoix_iOS

final class MenuBarPresentationTests: XCTestCase {
    func testMenuBarCatalogProvidesActionsAndDirections() {
        XCTAssertEqual(MenuBarPresentationText.openAppAction(language: "en"), "Open Envoix")
        XCTAssertEqual(MenuBarPresentationText.openAppAction(language: "zh-Hans"), "打开 Envoix")
        XCTAssertEqual(MenuBarPresentationText.quitAppAction(language: "en"), "Quit Envoix")
        XCTAssertEqual(MenuBarPresentationText.quitAppAction(language: "zh-Hans"), "退出 Envoix")
        XCTAssertEqual(MenuBarPresentationText.transferTitle(.send, language: "en"), "Send")
        XCTAssertEqual(MenuBarPresentationText.transferTitle(.receive, language: "zh-Hans"), "接收")
    }

    func testMenuBarSummaryCoversEveryNonTransferringState() {
        let states: [(TransferActivityState?, String, String)] = [
            (nil, "Idle", "空闲"),
            (.preparing, "Preparing…", "准备中…"),
            (.waitingForPeer, "Waiting…", "等待中…"),
            (.pairing, "Pairing…", "配对中…"),
            (.connecting, "Connecting…", "连接中…"),
            (.awaitingDecision, "Review", "待确认"),
            (.verifying, "Verifying…", "校验中…"),
            (.saving, "Saving…", "保存中…"),
            (.waitingForReceiverSave, "Receiver saving…", "接收端保存中…"),
            (.finalizingDelivery, "Finalizing…", "确认送达中…"),
            (.paused, "Paused", "已暂停"),
            (.delivered, "Delivered", "已送达"),
            (.canceled, "Canceled", "已取消"),
            (.failed, "Failed", "失败"),
        ]

        for (state, english, chinese) in states {
            XCTAssertEqual(summary(state, language: "en"), english)
            XCTAssertEqual(summary(state, language: "zh-Hans"), chinese)
        }
    }

    func testMenuBarProgressIsBoundedAndRejectsInvalidRates() {
        XCTAssertEqual(summary(.transferring, progress: 0.426, rate: 2_000_000), "43% · 2.0 MB/s")
        XCTAssertEqual(summary(.transferring, progress: -.infinity, rate: .infinity), "0%")
        XCTAssertEqual(summary(.transferring, progress: -0.5, rate: -1), "0%")
        XCTAssertEqual(summary(.transferring, progress: 1.5, rate: 0), "100%")
    }

    private func summary(
        _ state: TransferActivityState?,
        progress: Double = 0,
        rate: Double = 0,
        language: String = "en"
    ) -> String {
        MenuBarPresentationText.summary(
            state: state,
            progressFraction: progress,
            bytesPerSecond: rate,
            language: language
        )
    }
}
