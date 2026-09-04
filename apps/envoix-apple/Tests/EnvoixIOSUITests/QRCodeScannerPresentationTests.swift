import XCTest
@testable import Envoix_iOS

final class QRCodeScannerPresentationTests: XCTestCase {
    func testScannerCatalogProvidesActions() {
        let labels = [
            ("common.close", "Close", "关闭"),
            ("scanner.action.use_test_qr", "Use test QR", "使用测试二维码"),
            ("scanner.title", "Scan pairing QR", "扫描配对二维码"),
        ]

        for (key, english, chinese) in labels {
            XCTAssertEqual(AppText.localized(key, language: "en"), english, key)
            XCTAssertEqual(AppText.localized(key, language: "zh-Hans"), chinese, key)
        }
    }

    func testScannerMessagesCoverEveryCameraState() {
        let messages: [(QRCodeScannerMessageKind, String, String, String, String)] = [
            (
                .cameraAccessDenied,
                "Camera access is off",
                "相机权限未开启",
                "Allow camera access in Settings to scan an Envoix pairing QR code.",
                "请在系统设置中允许相机访问，然后扫描 Envoix 配对二维码。"
            ),
            (
                .cameraPermissionRequired,
                "Camera permission needed",
                "需要相机权限",
                "Envoix uses the camera only to scan pairing QR codes.",
                "Envoix 仅使用相机扫描配对二维码。"
            ),
            (
                .cameraUnavailable,
                "Camera unavailable",
                "相机不可用",
                "This device cannot start QR scanning.",
                "当前设备无法启动二维码扫描。"
            ),
        ]

        XCTAssertEqual(messages.count, QRCodeScannerMessageKind.allCases.count)
        for (kind, englishTitle, chineseTitle, englishDetail, chineseDetail) in messages {
            XCTAssertEqual(
                QRCodeScannerPresentationText.title(for: kind, language: "en"),
                englishTitle
            )
            XCTAssertEqual(
                QRCodeScannerPresentationText.title(for: kind, language: "zh-Hans"),
                chineseTitle
            )
            XCTAssertEqual(
                QRCodeScannerPresentationText.detail(for: kind, language: "en"),
                englishDetail
            )
            XCTAssertEqual(
                QRCodeScannerPresentationText.detail(for: kind, language: "zh-Hans"),
                chineseDetail
            )
        }
    }
}
