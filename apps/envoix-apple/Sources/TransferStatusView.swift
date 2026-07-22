import SwiftUI
import EnvoixCore
#if os(iOS)
import QuickLook
#endif

// Extracted from Support.swift (2026-07-20 split, no behavior change)

/// Shared status / progress section used by both the send and receive views.
struct TransferStatusView: View {
    @Environment(\.appLanguage) private var language
    @AppStorage("envoix.developerMode") private var developerMode = false
    @AppStorage("envoix.verboseLog") private var verboseLog = false
    @ObservedObject var viewModel: TransferViewModel
    #if os(iOS)
    @State private var previewFileURL: URL?
    @State private var receivedItemsPresentation: ReceivedItemsPresentation?
    #endif

    var body: some View {
        if showsStatus {
            statusCard
        }
    }

    private var showsStatus: Bool {
        switch viewModel.phase {
        case .idle: return !viewModel.statusText.isEmpty
        default: return true
        }
    }

    private var statusCard: some View {
        VStack(alignment: .leading, spacing: 12) {
            HStack(alignment: .top, spacing: 12) {
                Image(systemName: iconName)
                    .font(.title3.weight(.semibold))
                    .foregroundStyle(tint)
                    .frame(width: 30, height: 30)
                    .background(tint.opacity(0.10), in: Circle())

                VStack(alignment: .leading, spacing: 4) {
                    Text(titleText)
                        .font(.title3.weight(.semibold))
                        .foregroundStyle(Theme.text)
                        .lineLimit(2)

                    if let detailText {
                        Text(detailText)
                            .font(.body)
                            .foregroundStyle(Theme.muted)
                            .lineLimit(3)
                            .fixedSize(horizontal: false, vertical: true)
                    }
                }

                Spacer(minLength: 8)
            }

            switch viewModel.phase {
            case .idle, .waiting, .canceled, .failed:
                EmptyView()
            case .paused:
                if viewModel.total > 0 {
                    transferProgressLine
                }
                if let path = currentDataPathText {
                    pathLine(path)
                }
            case .transferring:
                ProgressBar(value: viewModel.progressFraction)
                transferProgressLine
                if let path = currentDataPathText {
                    pathLine(path)
                }
            case .completed(let bytes):
                Text(byteString(bytes))
                    .font(.body.monospacedDigit())
                    .foregroundStyle(Theme.muted)
                if !viewModel.completedItemURLs.isEmpty {
                    completedFileControls(viewModel.completedItemURLs)
                } else if let url = viewModel.completedFileURL, isRegularFileURL(url) {
                    completedFileControls([url])
                }
            }

            if !viewModel.pendingOfferEntries.isEmpty {
                Divider().overlay(Theme.line)
                VStack(alignment: .leading, spacing: 5) {
                    Text(AppText.value("Incoming items", "即将接收", language: language))
                        .font(.callout.weight(.semibold))
                        .foregroundStyle(Theme.text)
                    ForEach(viewModel.pendingOfferEntries.prefix(12), id: \.entryId) { entry in
                        HStack(spacing: 6) {
                            Image(systemName: entry.kind == .directory ? "folder" : "doc")
                            Text(entry.name)
                                .lineLimit(1)
                                .truncationMode(.middle)
                            Spacer(minLength: 8)
                            if entry.kind == .file {
                                Text(byteString(entry.plaintextSize))
                                    .monospacedDigit()
                            }
                        }
                        .font(.footnote)
                        .foregroundStyle(Theme.muted)
                    }
                    if viewModel.pendingOfferEntries.count > 12 {
                        Text(AppText.value(
                            "Large inventories are summarized here; transfer still preserves the complete tree.",
                            "此处仅摘要显示大型清单；传输仍会完整保留全部目录树。",
                            language: language
                        ))
                            .font(.footnote)
                            .foregroundStyle(Theme.muted)
                    }
                }
            }

            if !viewModel.pendingSourceSelections.isEmpty {
                Divider().overlay(Theme.line)
                VStack(alignment: .leading, spacing: 10) {
                    Text(AppText.value("Source access decision", "来源访问决定", language: language))
                        .font(.callout.weight(.semibold))
                        .foregroundStyle(Theme.text)
                    ForEach(viewModel.pendingSourceSelections, id: \.rootItemId) { selection in
                        VStack(alignment: .leading, spacing: 7) {
                            Text(selection.requestedName)
                                .font(.body.weight(.semibold))
                                .lineLimit(1)
                                .truncationMode(.middle)
                            Text(AppText.value(
                                "Some descendants could not be read. Re-select the source to grant access again, send only accessible content, or remove this root.",
                                "部分子项目无法读取。你可以重新选择来源以授权、仅发送可访问内容，或移除此根项目。",
                                language: language
                            ))
                                .font(.footnote)
                                .foregroundStyle(Theme.muted)
                                .fixedSize(horizontal: false, vertical: true)
                            HStack {
                                Button(AppText.value("Send accessible content", "发送可访问内容", language: language)) {
                                    viewModel.approvePartialManifestSource(
                                        rootItemID: selection.rootItemId
                                    )
                                }
                                .buttonStyle(.borderedProminent)
                                Button(AppText.value("Remove", "移除", language: language)) {
                                    viewModel.removeManifestSource(rootItemID: selection.rootItemId)
                                }
                                .buttonStyle(.bordered)
                            }
                        }
                    }
                }
            }

            if viewModel.requiresExceptionalTransferApproval {
                Button {
                    _ = viewModel.approveExceptionalTransfer()
                } label: {
                    Label(
                        AppText.value("Receive this large transfer", "接收此大文件传输", language: language),
                        systemImage: "arrow.down.circle"
                    )
                    .frame(maxWidth: .infinity, minHeight: 38)
                }
                .buttonStyle(.borderedProminent)
                .accessibilityIdentifier("approve_exceptional_transfer")
            }

            if let stepText {
                Text(stepText)
                    .font(.callout.monospaced())
                    .foregroundStyle(Theme.muted)
                    .lineLimit(1)
                    .truncationMode(.middle)
            }

            if developerMode && !viewModel.eventLog.isEmpty {
                Divider().overlay(Theme.line)
                logsCard
            }
        }
        .padding(14)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(backgroundTint)
        .overlay(
            RoundedRectangle(cornerRadius: Theme.cardRadius)
                .strokeBorder(tint.opacity(borderOpacity), lineWidth: 0.9)
        )
        .clipShape(RoundedRectangle(cornerRadius: Theme.cardRadius))
        #if os(iOS)
        .quickLookPreview($previewFileURL)
        .sheet(item: $receivedItemsPresentation) { presentation in
            ReceivedItemsSheet(urls: presentation.urls)
        }
        #endif
    }

    private var transferProgressLine: some View {
        HStack(spacing: 6) {
            Text("\(byteString(viewModel.transferred)) / \(byteString(viewModel.total))")
            if viewModel.bytesPerSec > 0 {
                Text("·")
                Text(rateString(viewModel.bytesPerSec))
            }
            if let eta = viewModel.etaSeconds {
                Text("·")
                Text(etaString(eta))
            }
        }
        .font(.body.monospacedDigit())
        .foregroundStyle(Theme.muted)
    }

    private func pathLine(_ path: String) -> some View {
        HStack(spacing: 6) {
            Text(AppText.value("Path", "链路", language: language))
                .fontWeight(.semibold)
            Text("·")
            Text(path)
                .lineLimit(1)
                .truncationMode(.middle)
        }
        .font(.body.monospacedDigit())
        .foregroundStyle(Theme.muted)
    }

    private var currentDataPathText: String? {
        nil
    }

    @ViewBuilder private var logsCard: some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack {
                Text(AppText.value("Activity log", "活动日志", language: language))
                    .font(.callout.weight(.semibold))
                    .foregroundStyle(Theme.text)
                Spacer(minLength: 8)
                if verboseLog {
                    Text(AppText.value("Verbose", "详细", language: language))
                        .font(.caption.monospaced())
                        .foregroundStyle(Theme.muted)
                }
                Button {
                    copyWithToast(
                        viewModel.eventLog.joined(separator: "\n"),
                        AppText.value("Log copied", "日志已复制", language: language),
                        language: language
                    )
                } label: {
                    Label(AppText.value("Copy", "复制", language: language), systemImage: "doc.on.doc")
                        .labelStyle(.iconOnly)
                        .frame(width: 30, height: 30)
                        .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
            }

            ScrollView {
                VStack(alignment: .leading, spacing: 4) {
                    ForEach(viewModel.eventLog, id: \.self) { line in
                        Text(line)
                            .font(.caption.monospaced())
                            .foregroundStyle(Theme.muted)
                            .lineLimit(2)
                            .textSelection(.enabled)
                    }
                }
            }
            .frame(maxHeight: 180)
        }
    }

    private var titleText: String {
        switch viewModel.phase {
        case .idle:
            return AppText.value("Status", "状态", language: language)
        case .waiting:
            return AppText.value("Waiting for the other device", "正在等待另一台设备", language: language)
        case .transferring:
            return viewModel.fileName.isEmpty ? AppText.value("Transferring", "正在传输", language: language) : viewModel.fileName
        case .paused:
            return AppText.value("Transfer paused", "传输已暂停", language: language)
        case .completed:
            return AppText.value("Transfer completed", "传输完成", language: language)
        case .canceled:
            return AppText.value("Transfer canceled", "传输已取消", language: language)
        case .failed(let reason):
            return failureText(reason: reason).title
        }
    }

    private var detailText: String? {
        switch viewModel.phase {
        case .idle:
            return viewModel.statusText.isEmpty ? nil : viewModel.statusText
        case .waiting:
            return viewModel.statusText.isEmpty
                ? AppText.value("Keep this window open until the peer connects.", "请保持此窗口打开，直到对方连接。", language: language)
                : viewModel.statusText
        case .transferring:
            return AppText.value("Keep both devices awake until the transfer finishes.", "请保持两台设备唤醒，直到传输完成。", language: language)
        case .paused:
            return AppText.value("Resume or delete this transfer from Activity.", "请在活动页继续或删除此传输。", language: language)
        case .completed:
            return viewModel.statusText.isEmpty ? AppText.value("The file is ready.", "文件已准备好。", language: language) : viewModel.statusText
        case .canceled:
            return AppText.value("Ready to start another transfer.", "可以开始新的传输。", language: language)
        case .failed(let reason):
            return failureText(reason: reason).detail
        }
    }

    private var stepText: String? {
        let text = viewModel.statusText.trimmed
        guard !text.isEmpty else { return nil }
        if case .failed = viewModel.phase {
            return AppText.value("Last step: \(text)", "上一步：\(text)", language: language)
        }
        return nil
    }

    private var iconName: String {
        switch viewModel.phase {
        case .idle: return "info.circle"
        case .waiting: return "antenna.radiowaves.left.and.right"
        case .transferring: return "arrow.up.arrow.down.circle"
        case .paused: return "pause.circle"
        case .completed: return "checkmark.circle.fill"
        case .canceled: return "xmark.circle"
        case .failed: return "exclamationmark.triangle.fill"
        }
    }

    private var tint: Color {
        switch viewModel.phase {
        case .idle: return Theme.muted
        case .waiting, .transferring, .paused: return Theme.warning
        case .completed: return Theme.success
        case .canceled: return Theme.muted
        case .failed: return Theme.danger
        }
    }

    private var backgroundTint: Color {
        switch viewModel.phase {
        case .failed: return Theme.dangerSoft.opacity(0.55)
        case .waiting, .transferring, .paused: return Theme.warning.opacity(0.06)
        case .completed: return Theme.success.opacity(0.06)
        case .idle, .canceled: return Theme.surface
        }
    }

    private var borderOpacity: Double {
        switch viewModel.phase {
        case .idle: return 0.25
        default: return 0.35
        }
    }

    private func failureText(reason: String) -> (title: String, detail: String) {
        if let failure = viewModel.failure {
            return structuredFailureText(failure)
        }
        return fallbackFailureText(reason)
    }

    private func structuredFailureText(_ failure: FfiTransferFailure) -> (title: String, detail: String) {
        let title: String
        switch failure.code {
        case .userCanceled, .senderCanceled:
            title = AppText.value("Transfer canceled", "传输已取消", language: language)
        case .networkLost:
            title = AppText.value("Connection failed", "连接失败", language: language)
        case .authenticationFailed:
            title = AppText.value("Pairing failed", "配对失败", language: language)
        case .unsupportedFeature:
            title = AppText.value("Update required", "需要更新", language: language)
        case .internalError:
            title = AppText.value("Transfer failed", "传输失败", language: language)
        case .senderSourceUnavailable, .senderPermissionLost, .senderSourceChanged,
             .senderItemRemoved:
            title = AppText.value("Source unavailable", "发送内容不可用", language: language)
        case .protocolOrIntegrityFailure:
            title = AppText.value("Verification failed", "校验失败", language: language)
        case .receiverSpaceInsufficient:
            title = AppText.value("Not enough space", "空间不足", language: language)
        case .receiverDestinationDecisionRequired, .receiverDestinationUnavailable,
             .receiverSaveFailed, .receiverReusedObjectLost,
             .receiverFinalizationOutcomeUnknown:
            title = AppText.value("Could not save", "无法保存", language: language)
        }
        return (title, friendlyFailure(failure, language: language))
    }

    private func fallbackFailureText(_ reason: String) -> (title: String, detail: String) {
        let cleanReason = reason.trimmed
        let lower = cleanReason.lowercased()
        if lower.contains("mdns") && lower.contains("peers discovered") {
            return (
                AppText.value("No device found on the local network", "未在局域网发现设备", language: language),
                AppText.value("Make sure the other device is receiving with the same token and both devices are on the same network.", "请确认另一台设备正在使用相同口令接收，并且两台设备在同一网络中。", language: language)
            )
        }
        if cleanReason.isEmpty {
            return (
                AppText.value("Transfer failed", "传输失败", language: language),
                AppText.value("Try again, or switch pairing method if discovery keeps failing.", "请重试；如果一直无法发现设备，请切换配对方式。", language: language)
            )
        }
        return (AppText.value("Transfer failed", "传输失败", language: language), cleanReason)
    }

    /// Reveal the received file. iOS hides the raw container path unless
    /// developer mode is enabled because it is not a user-facing location.
    @ViewBuilder private func completedFileControls(_ urls: [URL]) -> some View {
        if let firstURL = urls.first {
            HStack {
            #if os(macOS)
            Button(platformRevealTitle(language: language)) { revealInFinder(urls) }
            if urls.count == 1 {
                copyPathButton(firstURL)
            }
            #elseif os(iOS)
            if urls.count == 1, isRegularFileURL(firstURL) {
                Button(platformRevealTitle(language: language)) { previewFileURL = firstURL }
                ShareLink(item: firstURL) {
                    Label(AppText.value("Share", "分享", language: language), systemImage: "square.and.arrow.up")
                }
            } else {
                Button {
                    receivedItemsPresentation = ReceivedItemsPresentation(urls: urls)
                } label: {
                    Label(
                        AppText.value(
                            "View \(urls.count) Items",
                            "查看 \(urls.count) 个项目",
                            language: language
                        ),
                        systemImage: "square.stack"
                    )
                }
            }
            if developerMode, urls.count == 1 {
                copyPathButton(firstURL)
            }
            #endif
        }
        #if os(macOS)
        if urls.count == 1 {
            Text(firstURL.path)
                .font(.body.monospaced())
                .foregroundStyle(Theme.muted)
                .textSelection(.enabled)
                .lineLimit(1)
                .truncationMode(.middle)
        } else {
            Text(AppText.value(
                "\(urls.count) received items",
                "已接收 \(urls.count) 个项目",
                language: language
            ))
            .font(.body)
            .foregroundStyle(Theme.muted)
        }
        #elseif os(iOS)
        Text(urls.count == 1
             ? AppText.value(
                "Saved as \(firstURL.lastPathComponent)",
                "已保存为 \(firstURL.lastPathComponent)",
                language: language
             )
             : AppText.value(
                "Saved \(urls.count) items",
                "已保存 \(urls.count) 个项目",
                language: language
             ))
            .font(.body)
            .foregroundStyle(Theme.muted)
            .lineLimit(1)
            .truncationMode(.middle)
        if developerMode {
            ForEach(urls, id: \.path) { url in
                Text(url.path)
                    .font(.body.monospaced())
                    .foregroundStyle(Theme.muted)
                    .textSelection(.enabled)
                    .lineLimit(1)
                    .truncationMode(.middle)
            }
        }
            #endif
        }
    }

    private func copyPathButton(_ url: URL) -> some View {
        Button(AppText.value("Copy Path", "复制路径", language: language)) {
            copyWithToast(url.path, AppText.value("Path copied", "路径已复制", language: language), language: language)
        }
    }
}
