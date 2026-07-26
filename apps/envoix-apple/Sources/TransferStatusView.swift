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
        viewModel.presentationState != nil || !viewModel.statusText.isEmpty
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

            if let state = viewModel.presentationState {
                let progress = TransferPresentationPolicy.progress(for: state)
                if state != .delivered, progress != .hidden, viewModel.total > 0 {
                    ProgressBar(value: viewModel.progressFraction)
                    transferProgressLine
                }
                if progress == .active || progress == .retained,
                   let path = currentDataPathText {
                    pathLine(path)
                }
                if state == .delivered {
                    if !viewModel.completedItemURLs.isEmpty {
                        completedFileControls(viewModel.completedItemURLs)
                    } else if let url = viewModel.completedFileURL, isRegularFileURL(url) {
                        completedFileControls([url])
                    }
                }
            }

            if let summary = viewModel.preparedInventorySummary,
               !viewModel.preparedInventoryRoots.isEmpty {
                Divider().overlay(Theme.line)
                VStack(alignment: .leading, spacing: 6) {
                    Text(AppText.value("Prepared items", "已准备的项目", language: language))
                        .font(.callout.weight(.semibold))
                        .foregroundStyle(Theme.text)
                    Text(preparedInventorySummaryText(summary))
                        .font(.footnote.monospacedDigit())
                        .foregroundStyle(Theme.muted)
                    ForEach(viewModel.preparedInventoryRoots.prefix(6), id: \.itemId) { item in
                        HStack(spacing: 6) {
                            Image(systemName: inventoryIcon(name: item.name, isDirectory: item.kind == .directory))
                                .foregroundStyle(item.kind == .directory ? Theme.warning : Theme.accentStrong)
                                .frame(width: 24)
                            Text(item.name)
                                .lineLimit(1)
                                .truncationMode(.middle)
                            Spacer(minLength: 8)
                            if item.kind == .file {
                                Text(byteString(item.plaintextSize))
                                    .monospacedDigit()
                            }
                        }
                        .font(.footnote)
                        .foregroundStyle(item.hasWarning ? Theme.danger : Theme.muted)
                        .padding(.horizontal, 9)
                        .frame(minHeight: 34)
                        .background(Theme.surfaceRaised, in: RoundedRectangle(cornerRadius: 9))
                    }
                    if summary.rootCount > 6 {
                        Text(AppText.value(
                            "\(summary.rootCount - 6) more top-level items are included.",
                            "还包含 \(summary.rootCount - 6) 个顶层项目。",
                            language: language
                        ))
                            .font(.footnote)
                            .foregroundStyle(Theme.muted)
                    }
                }
                .accessibilityIdentifier("prepared_inventory")
            }

            if !viewModel.pendingOfferEntries.isEmpty {
                Divider().overlay(Theme.line)
                VStack(alignment: .leading, spacing: 5) {
                    Text(AppText.value("Incoming items", "即将接收", language: language))
                        .font(.callout.weight(.semibold))
                        .foregroundStyle(Theme.text)
                    if let summary = viewModel.pendingOfferSummary {
                        Text(incomingInventorySummaryText(summary))
                            .font(.footnote.monospacedDigit())
                            .foregroundStyle(Theme.muted)
                    }
                    ForEach(viewModel.pendingOfferEntries.prefix(6), id: \.entryId) { entry in
                        HStack(spacing: 6) {
                            Image(systemName: inventoryIcon(name: entry.name, isDirectory: entry.kind == .directory))
                                .foregroundStyle(entry.kind == .directory ? Theme.warning : Theme.accentStrong)
                                .frame(width: 24)
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
                        .padding(.horizontal, 9)
                        .frame(minHeight: 34)
                        .background(Theme.surfaceRaised, in: RoundedRectangle(cornerRadius: 9))
                    }
                    if viewModel.pendingOfferEntries.count > 6 {
                        Text(AppText.value(
                            "\(viewModel.pendingOfferEntries.count - 6) more items are included in the authenticated manifest.",
                            "已认证清单中还包含 \(viewModel.pendingOfferEntries.count - 6) 个项目。",
                            language: language
                        ))
                            .font(.footnote)
                            .foregroundStyle(Theme.muted)
                    }
                }
                .accessibilityIdentifier("incoming_inventory")
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

    private func preparedInventorySummaryText(_ summary: FfiInventorySummaryV2) -> String {
        let base = AppText.value(
            "\(summary.rootCount) roots · \(summary.fileCount) files · \(summary.directoryCount) folders · \(byteString(summary.totalPlaintextBytes))",
            "\(summary.rootCount) 个根项目 · \(summary.fileCount) 个文件 · \(summary.directoryCount) 个文件夹 · \(byteString(summary.totalPlaintextBytes))",
            language: language
        )
        guard summary.warningCount > 0 else { return base }
        return base + AppText.value(
            " · \(summary.warningCount) warnings",
            " · \(summary.warningCount) 个警告",
            language: language
        )
    }

    private func incomingInventorySummaryText(_ summary: FfiManifestOfferSummaryV2) -> String {
        AppText.value(
            "\(summary.rootCount) roots · \(summary.fileCount) files · \(summary.directoryCount) folders · \(byteString(summary.totalPlaintextBytes))",
            "\(summary.rootCount) 个根项目 · \(summary.fileCount) 个文件 · \(summary.directoryCount) 个文件夹 · \(byteString(summary.totalPlaintextBytes))",
            language: language
        )
    }

    private func inventoryIcon(name: String, isDirectory: Bool) -> String {
        if isDirectory { return "folder.fill" }
        switch URL(fileURLWithPath: name).pathExtension.lowercased() {
        case "jpg", "jpeg", "png", "gif", "heic", "webp":
            return "photo"
        case "mov", "mp4", "m4v":
            return "video"
        case "mp3", "m4a", "wav", "flac":
            return "waveform"
        case "pdf":
            return "doc.richtext"
        case "zip", "tar", "gz", "7z":
            return "archivebox"
        default:
            return "doc.fill"
        }
    }

    private var transferProgressLine: some View {
        HStack(spacing: 6) {
            Text("\(byteString(viewModel.transferred)) / \(byteString(viewModel.total))")
            if viewModel.presentationState == .transferring, viewModel.bytesPerSec > 0 {
                Text("·")
                Text(rateString(viewModel.bytesPerSec))
            }
            if viewModel.presentationState == .transferring, let eta = viewModel.etaSeconds {
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
        guard let path = viewModel.connectionPath else { return nil }
        return ConnectionPathPresentationPolicy.label(for: path, language: language)
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
        switch viewModel.presentationState {
        case nil:
            return AppText.value("Selection status", "选择状态", language: language)
        case .preparing?:
            return AppText.value("Preparing locally", "正在本地准备", language: language)
        case .waitingForPeer?:
            return AppText.value("Waiting for the other device", "正在等待另一台设备", language: language)
        case .pairing?:
            return AppText.value("Pairing devices", "正在配对设备", language: language)
        case .connecting?:
            return AppText.value("Connecting", "正在连接", language: language)
        case .awaitingDecision?:
            return AppText.value("Review incoming transfer", "确认接收内容", language: language)
        case .transferring?:
            if !viewModel.fileName.isEmpty { return viewModel.fileName }
            return viewModel.transferActivity?.direction == .send
                ? AppText.value("Sending", "正在发送", language: language)
                : AppText.value("Receiving", "正在接收", language: language)
        case .verifying?:
            return AppText.value("Verifying", "正在校验", language: language)
        case .saving?:
            return AppText.value("Saving", "正在保存", language: language)
        case .waitingForReceiverSave?:
            return AppText.value("Waiting for receiver to save", "等待接收方完成保存", language: language)
        case .finalizingDelivery?:
            return AppText.value("Finalizing delivery", "正在完成交付确认", language: language)
        case .paused?:
            return AppText.value("Transfer paused", "传输已暂停", language: language)
        case .delivered?:
            return viewModel.transferActivity?.direction == .receive
                ? AppText.value("Received", "已接收", language: language)
                : AppText.value("Delivered", "已送达", language: language)
        case .canceled?:
            return AppText.value("Transfer canceled", "传输已取消", language: language)
        case .failed?:
            return failureText(reason: viewModel.statusText).title
        }
    }

    private var detailText: String? {
        if viewModel.presentationState == .failed {
            return failureText(reason: viewModel.statusText).detail
        }
        switch viewModel.presentationState {
        case nil:
            return viewModel.statusText.isEmpty ? nil : viewModel.statusText
        case .preparing?:
            return AppText.value("Reading and validating the selected items.", "正在读取并验证所选项目。", language: language)
        case .waitingForPeer?:
            return AppText.value("Keep this window open until the peer connects.", "请保持此窗口打开，直到对方连接。", language: language)
        case .pairing?, .connecting?:
            return AppText.value("Keep both devices awake while the connection is established.", "建立连接时请保持两台设备唤醒。", language: language)
        case .awaitingDecision?:
            return viewModel.statusText.isEmpty
                ? AppText.value("Review the authenticated inventory before accepting.", "接收前请确认已认证的内容清单。", language: language)
                : viewModel.statusText
        case .transferring?:
            return AppText.value("Keep both devices awake until payload transfer finishes.", "请保持两台设备唤醒，直到内容传输完成。", language: language)
        case .verifying?:
            return AppText.value("Checking received content before publication.", "发布前正在校验接收内容。", language: language)
        case .saving?, .waitingForReceiverSave?, .finalizingDelivery?:
            return AppText.value("Payload is complete; delivery is still being finalized.", "内容传输已完成，正在完成最终交付。", language: language)
        case .paused?:
            return AppText.value("Resume or remove this transfer from Activity.", "请在活动页继续或移除此传输。", language: language)
        case .delivered?:
            return viewModel.transferActivity?.direction == .receive
                ? AppText.value("The received content is ready.", "接收内容已准备就绪。", language: language)
                : AppText.value("The receiver confirmed the saved content.", "接收方已确认内容保存完成。", language: language)
        case .canceled?:
            return AppText.value("Ready to start another transfer.", "可以开始新的传输。", language: language)
        case .failed?:
            return viewModel.statusText.isEmpty ? nil : viewModel.statusText
        }
    }

    private var stepText: String? {
        let text = viewModel.statusText.trimmed
        guard !text.isEmpty else { return nil }
        if viewModel.presentationState == .failed {
            return AppText.value("Last step: \(text)", "上一步：\(text)", language: language)
        }
        return nil
    }

    private var iconName: String {
        switch viewModel.presentationState {
        case nil: return "info.circle"
        case .preparing?: return "doc.badge.gearshape"
        case .waitingForPeer?: return "antenna.radiowaves.left.and.right"
        case .pairing?: return "person.2"
        case .connecting?: return "link"
        case .awaitingDecision?: return "checklist"
        case .transferring?: return "arrow.up.arrow.down.circle"
        case .verifying?: return "checkmark.shield"
        case .saving?, .waitingForReceiverSave?, .finalizingDelivery?: return "tray.and.arrow.down"
        case .paused?: return "pause.circle"
        case .delivered?: return "checkmark.circle.fill"
        case .canceled?: return "xmark.circle"
        case .failed?: return "exclamationmark.triangle.fill"
        }
    }

    private var tint: Color {
        switch viewModel.presentationState {
        case nil: return Theme.muted
        case .awaitingDecision?, .paused?: return Theme.warning
        case .delivered?: return Theme.success
        case .canceled?: return Theme.muted
        case .failed?: return Theme.danger
        default: return Theme.accentStrong
        }
    }

    private var backgroundTint: Color {
        switch viewModel.presentationState {
        case .failed?: return Theme.dangerSoft.opacity(0.55)
        case .awaitingDecision?, .paused?: return Theme.warning.opacity(0.06)
        case .delivered?: return Theme.success.opacity(0.06)
        default: return Theme.surface
        }
    }

    private var borderOpacity: Double {
        viewModel.presentationState == nil ? 0.25 : 0.35
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
