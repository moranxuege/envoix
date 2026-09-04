import XCTest
@testable import Envoix_iOS

final class MobileConnectionFlowPresentationTests: XCTestCase {
    func testEveryStaticStateResolvesFromBothCatalogs() {
        for copy in MobileConnectionFlowCopy.allCases {
            for language in ["en", "zh-Hans"] {
                let value = MobileConnectionFlowPresentationText.value(
                    copy,
                    language: language
                )
                XCTAssertFalse(value.isEmpty, "\(copy.rawValue) was empty for \(language)")
                XCTAssertNotEqual(value, copy.rawValue, "\(copy.rawValue) was missing for \(language)")
            }
        }

        XCTAssertEqual(
            MobileConnectionFlowPresentationText.value(.cancel, language: "zh-Hans"),
            "取消"
        )
        XCTAssertEqual(
            MobileConnectionFlowPresentationText.value(.connectionInputInvalid, language: "en"),
            "This is not a valid Envoix InviteV2 link, Room link, or current Room code."
        )
        XCTAssertEqual(
            MobileConnectionFlowPresentationText.value(.saveFolderPermissionExpired, language: "zh-Hans"),
            "所选保存文件夹的访问权限已过期。"
        )
    }

    func testExternalInvitationCopyCoversEveryOriginAndKind() {
        XCTAssertEqual(
            MobileConnectionFlowPresentationText.externalInvitationTitle(
                isRoomInvitation: true,
                isNFC: true,
                language: "en"
            ),
            "Nearby Envoix room found"
        )
        XCTAssertEqual(
            MobileConnectionFlowPresentationText.externalInvitationTitle(
                isRoomInvitation: false,
                isNFC: true,
                language: "zh-Hans"
            ),
            "发现附近的 Envoix 邀请"
        )
        XCTAssertEqual(
            MobileConnectionFlowPresentationText.externalInvitationTitle(
                isRoomInvitation: true,
                isNFC: false,
                language: "zh-Hans"
            ),
            "加入此房间？"
        )
        XCTAssertEqual(
            MobileConnectionFlowPresentationText.externalInvitationTitle(
                isRoomInvitation: false,
                isNFC: false,
                language: "en"
            ),
            "Open invitation?"
        )
        XCTAssertEqual(
            MobileConnectionFlowPresentationText.externalInvitationMessage(
                isRoomInvitation: false,
                isNFC: true,
                language: "en"
            ),
            "NFC confirms touch-range proximity, not the other phone's identity. Continue to validate this one-time invitation and connect."
        )
        XCTAssertEqual(
            MobileConnectionFlowPresentationText.externalInvitationMessage(
                isRoomInvitation: true,
                isNFC: false,
                language: "zh-Hans"
            ),
            "此房间邀请来自外部且未经信任。继续后将验证并连接；它不会认证另一台设备。"
        )
        XCTAssertEqual(
            MobileConnectionFlowPresentationText.externalInvitationMessage(
                isRoomInvitation: false,
                isNFC: false,
                language: "en"
            ),
            "This external invitation is untrusted. Continue to validate it and choose the normal transfer action; it does not authenticate the other device."
        )
    }

    func testDynamicCopySanitizesNamesAndBoundsCounts() {
        XCTAssertEqual(
            MobileConnectionFlowPresentationText.outgoingVerification(
                code: "123456",
                language: "zh-Hans"
            ),
            "请在另一台设备上输入 123456。验证码不会通过蓝牙发送。"
        )
        XCTAssertEqual(
            MobileConnectionFlowPresentationText.deviceVerification(
                peerDisplayName: "  WSL  ",
                language: "en"
            ),
            "Enter the six-digit code shown by WSL. A successful match saves this device for future rooms."
        )
        XCTAssertEqual(
            MobileConnectionFlowPresentationText.deviceVerification(
                peerDisplayName: " ",
                language: "zh-Hans"
            ),
            "请输入 另一台设备 显示的六位验证码。匹配成功后会保存此设备，以便以后自动连接。"
        )
        XCTAssertEqual(
            MobileConnectionFlowPresentationText.nearbyOfferMessage(
                senderDisplayName: nil,
                isRoomInvitation: true,
                language: "en"
            ),
            "A nearby device wants to open a room. Confirm on the other device before accepting."
        )
        XCTAssertEqual(
            MobileConnectionFlowPresentationText.nearbyOfferMessage(
                senderDisplayName: "  iPhone  ",
                isRoomInvitation: false,
                language: "zh-Hans"
            ),
            "iPhone 希望开始一次性传输。接受前，请在另一台设备上确认。"
        )
        XCTAssertEqual(
            MobileConnectionFlowPresentationText.durablePairingCompleted(
                label: " ",
                language: "en"
            ),
            "Device is now securely paired."
        )
        XCTAssertEqual(
            MobileConnectionFlowPresentationText.queuedForDevice(
                label: "  WSL  ",
                language: "zh-Hans"
            ),
            "已加入发送队列：WSL。"
        )
        XCTAssertEqual(
            MobileConnectionFlowPresentationText.itemCountExceeded(
                maximum: -1,
                language: "en"
            ),
            "Choose no more than 0 items."
        )
    }
}
