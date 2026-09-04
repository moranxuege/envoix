import XCTest
@testable import Envoix_iOS

final class SupportPresentationTests: XCTestCase {
    func testTokenFieldCatalogProvidesActionsAndFeedback() {
        let labels: [((String) -> String, String, String)] = [
            (TokenFieldPresentationText.title, "Shared token", "共享口令"),
            (TokenFieldPresentationText.placeholder, "e.g. envoix-lan-2026", "例如 envoix-lan-2026"),
            (TokenFieldPresentationText.generateAction, "Generate", "生成"),
            (TokenFieldPresentationText.generated, "Token generated", "口令已生成"),
            (TokenFieldPresentationText.copyAction, "Copy Token", "复制口令"),
            (TokenFieldPresentationText.copied, "Token copied", "口令已复制"),
        ]

        for (text, english, chinese) in labels {
            XCTAssertEqual(text("en"), english)
            XCTAssertEqual(text("zh-Hans"), chinese)
        }
    }

    func testTokenMinimumLengthFormatRejectsNegativeInput() {
        XCTAssertEqual(
            TokenFieldPresentationText.desktopTitle(minimumLength: 12, language: "en"),
            "Shared token (same on both devices, 12+ characters)"
        )
        XCTAssertEqual(
            TokenFieldPresentationText.desktopTitle(minimumLength: 12, language: "zh-Hans"),
            "共享口令（两台设备相同，至少 12 个字符）"
        )
        XCTAssertEqual(
            TokenFieldPresentationText.desktopTitle(minimumLength: -1, language: "en"),
            "Shared token (same on both devices, 0+ characters)"
        )
    }

    func testReceivedItemsCatalogCoversBothPlatformActions() {
        XCTAssertEqual(ReceivedItemsPresentationText.title(language: "en"), "Received Items")
        XCTAssertEqual(ReceivedItemsPresentationText.title(language: "zh-Hans"), "已接收项目")
        XCTAssertEqual(
            ReceivedItemsPresentationText.emptyFolder(language: "en"),
            "This folder is empty or unavailable."
        )
        XCTAssertEqual(
            ReceivedItemsPresentationText.emptyFolder(language: "zh-Hans"),
            "此文件夹为空或当前无法访问。"
        )

        let actions: [(ReceivedItemsPresentationText.RevealTarget, String, String)] = [
            (.finder, "Reveal in Finder", "在 Finder 中显示"),
            (.file, "Open File", "打开文件"),
        ]
        XCTAssertEqual(actions.count, ReceivedItemsPresentationText.RevealTarget.allCases.count)
        for (target, english, chinese) in actions {
            XCTAssertEqual(
                ReceivedItemsPresentationText.revealAction(for: target, language: "en"),
                english
            )
            XCTAssertEqual(
                ReceivedItemsPresentationText.revealAction(for: target, language: "zh-Hans"),
                chinese
            )
        }
    }
}
