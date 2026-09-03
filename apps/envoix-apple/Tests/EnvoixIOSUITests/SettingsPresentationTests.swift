import XCTest
@testable import Envoix_iOS

final class SettingsPresentationTests: XCTestCase {
    func testSettingsCatalogProvidesStaticCopy() {
        let labels = [
            ("settings.advanced", "Advanced", "高级"),
            ("settings.appearance.dark", "Dark", "深色"),
            ("settings.appearance.light", "Light", "浅色"),
            ("settings.appearance.options", "System / Light / Dark", "跟随系统 / 浅色 / 深色"),
            ("settings.appearance.system", "System", "跟随系统"),
            ("settings.appearance.title", "Appearance", "外观"),
            ("settings.background.refresh", "Refresh", "刷新"),
            ("settings.background.status.checking", "Checking…", "正在检查…"),
            ("settings.background.status.off", "Off", "已关闭"),
            ("settings.background.status.starting", "Starting…", "正在启动…"),
            ("settings.background.title", "Background service", "后台服务"),
            ("settings.cache.clean_up", "Clean Up", "清理缓存"),
            ("settings.cache.title", "Transfer cache", "传输缓存"),
            ("settings.compression.always", "Always", "始终"),
            ("settings.compression.never", "Never", "从不"),
            ("settings.compression.smart", "Smart", "智能"),
            ("settings.compression.title", "Compression", "压缩"),
            ("settings.developer.enable", "Enable developer mode", "开启开发者模式"),
            ("settings.developer.log_server", "Remote log server", "远程日志服务器"),
            ("settings.developer.title", "Developer tools", "开发者工具"),
            ("settings.developer.verbose_logging", "Verbose logging", "详细日志"),
            ("settings.language.title", "Language", "语言"),
            ("settings.network.avoid_tailscale", "Avoid Tailscale addresses", "避开 Tailscale 地址"),
            ("settings.network.broker", "Rendezvous broker", "配对服务器"),
            ("settings.network.candidate_allow", "Candidate allow", "候选地址 allow"),
            ("settings.network.candidate_deny", "Candidate deny", "候选地址 deny"),
            ("settings.network.relay", "Relay URL", "中继 URL"),
            ("settings.network.title", "Pairing and network", "配对与网络"),
        ]

        for (key, english, chinese) in labels {
            XCTAssertEqual(AppText.localized(key, language: "en"), english, key)
            XCTAssertEqual(AppText.localized(key, language: "zh-Hans"), chinese, key)
        }
    }

    func testSettingsCatalogProvidesExplanatoryCopy() {
        let keys = [
            "settings.background.enable",
            "settings.background.enable_detail",
            "settings.background.status.approval_required",
            "settings.background.status.helper_missing",
            "settings.background.status.incompatible",
            "settings.background.status.registration_failed",
            "settings.background.status.unavailable",
            "settings.cache.detail",
            "settings.compression.detail",
            "settings.developer.enable_detail",
            "settings.developer.log_server_detail",
            "settings.developer.verbose_logging_detail",
            "settings.network.avoid_tailscale_detail",
            "settings.network.broker_detail",
            "settings.network.candidate_allow_detail",
            "settings.network.candidate_deny_detail",
            "settings.network.relay_detail",
        ]

        for key in keys {
            XCTAssertNotEqual(AppText.localized(key, language: "en"), key)
            XCTAssertNotEqual(AppText.localized(key, language: "zh-Hans"), key)
        }
    }

    func testSettingsCatalogFormatsDynamicValues() {
        XCTAssertEqual(readyStatus(1, language: "en"), "Ready · 1 paired device")
        XCTAssertEqual(readyStatus(3, language: "en"), "Ready · 3 paired devices")
        XCTAssertEqual(readyStatus(3, language: "zh-Hans"), "已就绪 · 3 台已配对设备")
        XCTAssertEqual(protectedStatus("1 MB", language: "en"), "Protected: 1 MB")
        XCTAssertEqual(protectedStatus("1 MB", language: "zh-Hans"), "受保护：1 MB")
    }

    private func readyStatus(_ count: Int64, language: String) -> String {
        AppText.localized(
            "settings.background.status.ready",
            defaultValue: "Ready · \(count) paired devices",
            language: language
        )
    }

    private func protectedStatus(_ value: String, language: String) -> String {
        AppText.localized(
            "settings.cache.protected",
            defaultValue: "Protected: \(value)",
            language: language
        )
    }
}
