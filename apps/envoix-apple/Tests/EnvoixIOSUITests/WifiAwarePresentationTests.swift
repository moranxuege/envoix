import XCTest
@testable import Envoix_iOS

final class WifiAwarePresentationTests: XCTestCase {
    func testPairingCopyCoversEveryStaticState() {
        let copy: [(WifiAwarePairingCopy, String, String)] = [
            (.showDevice, "Show this device and code", "显示本机与验证码"),
            (.findDevice, "Find the other device", "查找另一台设备"),
            (.buildUnavailable, "Nearby pairing is unavailable in this build.", "此版本无法使用附近设备配对。"),
            (.checking, "Checking Apple paired devices…", "正在检查 Apple 配对设备…"),
            (
                .firstPairGuidance,
                "For the first pair, tap “Show this device and code” on one device. On the other, tap “Find the other device”, select it, then enter or confirm the six-digit code. The two devices must use opposite buttons.",
                "首次配对时，请在一台设备上点“显示本机与验证码”；在另一台设备上点“查找另一台设备”，选择前一台设备，再输入或确认六位码。两台设备必须使用不同的按钮。"
            ),
            (
                .observationFailed,
                "Envoix could not read Apple's paired-device list. No pairing record was changed.",
                "Envoix 无法读取 Apple 的配对设备列表；现有配对记录未被更改。"
            ),
            (.retry, "Retry", "重试"),
            (.device, "device", "设备"),
            (.unavailable, "Pairing unavailable", "配对不可用"),
        ]

        XCTAssertEqual(copy.count, WifiAwarePairingCopy.allCases.count)
        for (item, english, chinese) in copy {
            XCTAssertEqual(WifiAwarePairingPresentationText.value(item, language: "en"), english)
            XCTAssertEqual(WifiAwarePairingPresentationText.value(item, language: "zh-Hans"), chinese)
        }
    }

    func testPairingFormatsCountsAndPickerStates() {
        XCTAssertEqual(
            WifiAwarePairingPresentationText.existingPairs(1, language: "en"),
            "1 Apple-paired device already available. Existing pairs do not show another six-digit code; tap Done to resume automatic discovery. Use the controls below only to add a new device."
        )
        XCTAssertEqual(
            WifiAwarePairingPresentationText.existingPairs(2, language: "en"),
            "2 Apple-paired devices already available. Existing pairs do not show another six-digit code; tap Done to resume automatic discovery. Use the controls below only to add a new device."
        )
        XCTAssertEqual(
            WifiAwarePairingPresentationText.existingPairs(-1, language: "zh-Hans"),
            "已有 0 台 Apple 系统配对设备。已有配对不会再次显示六位码；点“完成”即可恢复自动发现。仅在添加新设备时使用下方按钮。"
        )
        XCTAssertEqual(
            WifiAwarePairingPresentationText.newPairing(totalCount: -1, language: "en"),
            "New pairing detected · 0 total"
        )
        XCTAssertEqual(
            WifiAwarePairingPresentationText.pickerResult(
                displayName: "  iPhone  ",
                snapshotConfirmed: true,
                language: "en"
            ),
            "iPhone is paired and ready"
        )
        XCTAssertEqual(
            WifiAwarePairingPresentationText.pickerResult(
                displayName: " ",
                snapshotConfirmed: false,
                language: "zh-Hans"
            ),
            "已选择 设备；正在等待 Apple 配对列表更新"
        )
    }

    func testProbeCopyCoversEveryDeveloperPanelState() {
        let copy: [(WifiAwareProbeCopy, String, String)] = [
            (.title, "Wi-Fi Aware connection probe", "Wi-Fi Aware 连接探针"),
            (.receive, "Receive probe", "接收探针"),
            (.send, "Send probe", "发送探针"),
            (.stop, "Stop", "停止"),
            (.selectFirst, "Pair and select one device before starting a probe.", "开始探针前，请先配对并选择一台设备。"),
            (.target, "Probe target", "探针目标"),
            (.waitingAccess, "Waiting for Wi-Fi Aware access…", "正在等待 Wi-Fi Aware 访问权限…"),
            (.allowDevice, "Allow device", "允许设备"),
            (.addDevice, "Add device", "添加设备"),
            (.pickerUnavailable, "Picker unavailable", "选择器不可用"),
            (.serviceMissing, "The TCP probe service is missing from Info.plist.", "Info.plist 中缺少 TCP 探针服务。"),
        ]

        XCTAssertEqual(copy.count, WifiAwareProbeCopy.allCases.count)
        for (item, english, chinese) in copy {
            XCTAssertEqual(WifiAwareDeveloperPresentationText.value(item, language: "en"), english)
            XCTAssertEqual(WifiAwareDeveloperPresentationText.value(item, language: "zh-Hans"), chinese)
        }
    }
}
