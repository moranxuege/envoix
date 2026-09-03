import XCTest
@testable import Envoix_iOS

final class TransferWorkflowPresentationTests: XCTestCase {
    func testWorkflowStatusCoversEverySemanticState() {
        let states: [(TransferWorkflowStatus, String, String)] = [
            (.preparingSelection, "Preparing selected items…", "正在准备所选项目…"),
            (.restoringInterrupted, "Restoring interrupted transfer", "正在恢复中断的传输"),
            (.canceled, "Canceled", "已取消"),
            (.queuedForRoom, "Queued for this room", "已加入此房间的发送队列"),
            (.pausedRetained, "Paused; progress is retained", "已暂停；进度已保留"),
            (.waitingForSender, "Waiting for sender", "等待发送方"),
            (.waitingForPeer, "Waiting for peer", "正在等待对端"),
            (.pairing, "Pairing", "正在配对"),
            (.connecting, "Connecting", "正在连接"),
            (.transferring, "Transferring", "正在传输"),
            (.verifyingReceived, "Verifying received content", "正在校验接收内容"),
            (.savingSelected, "Saving to the selected location", "正在保存到所选位置"),
            (.waitingForReceiverSave, "Waiting for receiver to save", "等待接收方完成保存"),
            (.finalizingDelivery, "Saved; finalizing delivery", "已保存，正在完成交付确认"),
            (.delivered, "Delivered", "已送达"),
            (.previousSendActive, "Wait for the previous send to finish.", "请等待上一次发送结束。"),
            (.sourceRequired, "Choose at least one file or folder", "请至少选择一个文件或文件夹"),
            (.sourceWarnings, "Resolve source warnings before sending", "请先处理来源警告"),
            (.reviewExceptional, "Review this unusually large transfer before receiving", "请先确认这个异常大的传输"),
            (.readyToSend, "Ready to send", "已准备发送"),
            (.sourceDecisionRequired, "Some items need your decision", "部分项目需要你的决定"),
        ]

        XCTAssertEqual(states.count, TransferWorkflowStatus.allCases.count)
        for (status, english, chinese) in states {
            XCTAssertEqual(TransferWorkflowText.status(status, language: "en"), english)
            XCTAssertEqual(TransferWorkflowText.status(status, language: "zh-Hans"), chinese)
        }
    }

    func testWorkflowItemCountUsesSharedNativePluralization() {
        XCTAssertEqual(TransferWorkflowText.itemCount(1, language: "en"), "1 item")
        XCTAssertEqual(TransferWorkflowText.itemCount(2, language: "en"), "2 items")
        XCTAssertEqual(TransferWorkflowText.itemCount(2, language: "zh-Hans"), "2 个项目")
    }
}
