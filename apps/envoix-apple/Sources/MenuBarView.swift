#if os(macOS)
import SwiftUI
import AppKit

/// Compact status shown in the menu-bar popover. Mirrors the live transfer state
/// and offers a one-click way to bring the main window forward.
struct MenuBarView: View {
    @EnvironmentObject private var model: AppModel
    @Environment(\.openWindow) private var openWindow
    @Environment(\.appLanguage) private var language

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            Text("Envoix").font(.headline)

            TransferMenuRow(
                title: AppText.value("Receiving", "接收", language: language),
                viewModel: model.receive,
                language: language
            )
            TransferMenuRow(
                title: AppText.value("Sending", "发送", language: language),
                viewModel: model.send,
                language: language
            )

            Divider()

            Button(AppText.value("Open Envoix", "打开 Envoix", language: language)) {
                openWindow(id: "main")
                NSApp.activate(ignoringOtherApps: true)
            }
            Button(AppText.value("Quit Envoix", "退出 Envoix", language: language)) { NSApp.terminate(nil) }
        }
        .padding(14)
        .frame(width: 240)
    }

}

private struct TransferMenuRow: View {
    let title: String
    @ObservedObject var viewModel: TransferViewModel
    let language: String

    var body: some View {
        HStack {
            Text(title).foregroundStyle(.secondary)
            Spacer()
            Text(summary).font(.callout.monospacedDigit())
        }
    }

    private var summary: String {
        switch viewModel.phase {
        case .idle: return AppText.value("Idle", "空闲", language: language)
        case .waiting: return AppText.value("Waiting…", "等待中…", language: language)
        case .transferring:
            let pct = Int((viewModel.progressFraction * 100).rounded())
            return viewModel.bytesPerSec > 0
                ? "\(pct)% · \(rateString(viewModel.bytesPerSec))"
                : "\(pct)%"
        case .paused: return AppText.value("Paused", "已暂停", language: language)
        case .completed: return AppText.value("Done", "已完成", language: language)
        case .canceled: return AppText.value("Canceled", "已取消", language: language)
        case .failed: return AppText.value("Failed", "失败", language: language)
        }
    }
}
#endif
