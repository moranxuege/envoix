import XCTest
@testable import Envoix_iOS

final class RoomPresentationTests: XCTestCase {
    func testRoomCatalogProvidesStaticScreenCopy() {
        let cases: [(String, String, String)] = [
            ("common.decline", "Decline", "拒绝"),
            ("common.done", "Done", "完成"),
            ("room.action.add_files", "Add files", "添加文件"),
            ("room.action.end", "End room", "结束房间"),
            ("room.action.keep_open", "Keep open", "保持开启"),
            ("room.action.keep_open_accessibility", "Keep room open", "保持房间开启"),
            ("room.activity.all", "All Activity", "全部活动"),
            ("room.activity.empty", "Transfers started here will appear in this timeline.", "从这里开始的传输会显示在此时间线中。"),
            ("room.activity.title", "Room activity", "房间活动"),
            ("room.offer.contents", "Contents", "内容"),
            ("room.offer.destination", "Destination", "保存位置"),
            ("room.offer.incoming", "Incoming transfer", "收到传输邀请"),
            ("room.offer.preparing_receiver", "Preparing receiver…", "正在准备接收…"),
            ("room.offer.summary", "Offer summary", "内容摘要"),
        ]

        for (key, english, chinese) in cases {
            XCTAssertEqual(AppText.localized(key, language: "en"), english)
            XCTAssertEqual(AppText.localized(key, language: "zh-Hans"), chinese)
        }
    }
}
