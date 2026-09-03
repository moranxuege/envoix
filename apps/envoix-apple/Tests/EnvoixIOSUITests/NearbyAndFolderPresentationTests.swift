import XCTest
@testable import Envoix_iOS

final class NearbyAndFolderPresentationTests: XCTestCase {
    func testNearbyTransferContextCoversBothInvitationDirections() {
        XCTAssertEqual(
            NearbyTransferContextPresentationText.detail(
                deliversInvitationOnStart: true,
                language: "en"
            ),
            "Choose the transfer details first. The BLE invitation is sent only after you tap Start."
        )
        XCTAssertEqual(
            NearbyTransferContextPresentationText.detail(
                deliversInvitationOnStart: false,
                language: "zh-Hans"
            ),
            "已接受的邀请码载入下方；附近设备名称仍未经验证。"
        )
    }

    func testNearbyTransferContextProvidesFallbackTrustAndProgressCopy() {
        XCTAssertEqual(
            NearbyTransferContextPresentationText.fallbackDeviceName(language: "en"),
            "Nearby Envoix device"
        )
        XCTAssertEqual(
            NearbyTransferContextPresentationText.fallbackDeviceName(language: "zh-Hans"),
            "附近的 Envoix 设备"
        )
        XCTAssertEqual(
            NearbyTransferContextPresentationText.trustLabel(language: "en"),
            "Unverified"
        )
        XCTAssertEqual(
            NearbyTransferContextPresentationText.deliveryStatus(language: "zh-Hans"),
            "正在发送邀请码…"
        )
    }

    func testFolderPickerCatalogProvidesMacOSAuthorizationCopy() {
        let labels = [
            (FolderPickerPresentationText.title, "Choose a save folder", "选择保存文件夹"),
            (
                FolderPickerPresentationText.detail,
                "Envoix needs access to a folder before accepting these files.",
                "接受这些文件前，Envoix 需要访问一个保存文件夹。"
            ),
            (FolderPickerPresentationText.chooseAction, "Choose Folder", "选择文件夹"),
        ]

        for (text, english, chinese) in labels {
            XCTAssertEqual(text("en"), english)
            XCTAssertEqual(text("zh-Hans"), chinese)
        }
    }
}
