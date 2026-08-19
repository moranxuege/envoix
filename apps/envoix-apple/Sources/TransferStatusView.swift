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
                TransferPerformanceLine(
                    currentBytesPerSecond: progress == .active ? viewModel.bytesPerSec : 0,
                    averageBytesPerSecond: viewModel.averageBytesPerSec,
                    etaSeconds: progress == .active ? viewModel.etaSeconds : nil,
                    currentSampleDate: viewModel.currentRateUpdatedAt,
                    font: .caption,
                    accessibilityPrefix: "transfer"
                )
                if let path = currentDataPathText {
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
        Text("\(byteString(viewModel.transferred)) / \(byteString(viewModel.total))")
        .font(.body.monospacedDigit())
        .foregroundStyle(Theme.muted)
        .accessibilityIdentifier("transfer_byte_progress")
    }

    private func pathLine(_ path: String) -> some View {
        Text(path)
            .lineLimit(1)
            .truncationMode(.middle)
        .font(.body.monospacedDigit())
        .foregroundStyle(Theme.muted)
        .accessibilityIdentifier("transfer_data_path")
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
        let failureTitle = viewModel.presentationState == .failed
            ? failureText(reason: viewModel.statusText).title
            : nil
        return TransferStatusText.title(
            state: viewModel.presentationState,
            direction: viewModel.transferActivity?.direction,
            fileName: viewModel.fileName,
            failureTitle: failureTitle,
            language: language
        )
    }

    private var detailText: String? {
        let failureDetail = viewModel.presentationState == .failed
            ? failureText(reason: viewModel.statusText).detail
            : nil
        return TransferStatusText.detail(
            state: viewModel.presentationState,
            direction: viewModel.transferActivity?.direction,
            statusText: viewModel.statusText,
            failureDetail: failureDetail,
            language: language
        )
    }

    private var stepText: String? {
        TransferStatusText.lastStep(
            state: viewModel.presentationState,
            statusText: viewModel.statusText,
            language: language
        )
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

    private func failureText(reason: String) -> TransferFailurePresentationCopy {
        if let failure = viewModel.failure {
            return structuredFailureText(failure)
        }
        return TransferStatusText.fallbackFailure(reason: reason, language: language)
    }

    private func structuredFailureText(
        _ failure: FfiTransferFailure
    ) -> TransferFailurePresentationCopy {
        TransferFailurePresentationCopy(
            title: TransferStatusText.failureTitle(failure.code, language: language),
            detail: friendlyFailure(failure, language: language)
        )
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

struct TransferPerformanceLine: View {
    @Environment(\.appLanguage) private var language

    let currentBytesPerSecond: Double
    let averageBytesPerSecond: Double
    let etaSeconds: Double?
    let currentSampleDate: Date?
    var font: Font = .caption
    let accessibilityPrefix: String

    var body: some View {
        if needsFreshnessRefresh {
            TimelineView(.periodic(from: .now, by: 1)) { context in
                content(now: context.date)
            }
        } else {
            content(now: Date())
        }
    }

    @ViewBuilder
    private func content(now: Date) -> some View {
        let showCurrentMetrics = TransferMetricFreshnessPolicy.isFresh(
            sampledAt: currentSampleDate,
            now: now
        )
        if hasMetrics(showCurrentMetrics: showCurrentMetrics) {
            ViewThatFits(in: .horizontal) {
                HStack(spacing: 10) { metrics(showCurrentMetrics: showCurrentMetrics) }
                VStack(alignment: .leading, spacing: 5) {
                    metrics(showCurrentMetrics: showCurrentMetrics)
                }
            }
            .font(font.monospacedDigit())
            .foregroundStyle(Theme.muted)
            .accessibilityElement(children: .contain)
            .accessibilityIdentifier("\(accessibilityPrefix)_performance")
        }
    }

    private var needsFreshnessRefresh: Bool {
        bounded(currentBytesPerSecond) != nil || boundedETA != nil
    }

    private func hasMetrics(showCurrentMetrics: Bool) -> Bool {
        bounded(averageBytesPerSecond) != nil
            || (showCurrentMetrics && (
                bounded(currentBytesPerSecond) != nil || boundedETA != nil
            ))
    }

    @ViewBuilder
    private func metrics(showCurrentMetrics: Bool) -> some View {
        if showCurrentMetrics, let current = bounded(currentBytesPerSecond) {
            metric(
                AppText.value("Now", "当前", language: language),
                value: rateString(current),
                systemImage: "speedometer",
                identifier: "\(accessibilityPrefix)_speed_current"
            )
        }
        if let average = bounded(averageBytesPerSecond) {
            metric(
                AppText.value("Average", "平均", language: language),
                value: rateString(average),
                systemImage: "chart.line.uptrend.xyaxis",
                identifier: "\(accessibilityPrefix)_speed_average"
            )
        }
        if showCurrentMetrics, let eta = boundedETA {
            Label(etaString(eta), systemImage: "clock")
                .lineLimit(1)
                .accessibilityIdentifier("\(accessibilityPrefix)_eta")
        }
    }

    private func metric(
        _ label: String,
        value: String,
        systemImage: String,
        identifier: String
    ) -> some View {
        Label("\(label) \(value)", systemImage: systemImage)
            .lineLimit(1)
            .accessibilityIdentifier(identifier)
    }

    private func bounded(_ value: Double) -> Double? {
        guard value.isFinite, value > 0 else { return nil }
        return min(value, Double(Int64.max))
    }

    private var boundedETA: Double? {
        guard let etaSeconds, etaSeconds.isFinite, etaSeconds >= 0 else { return nil }
        return min(etaSeconds, Double(Int.max).nextDown)
    }
}
