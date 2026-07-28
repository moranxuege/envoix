import SwiftUI
#if os(iOS)
import QuickLook
#endif

struct TransferStageView: View {
    let records: [TransferActivityRecord]
    let pendingRemovalIDs: Set<String>
    let metricsByActivityID: [String: ActivityMetrics]
    let onCopyDiagnostics: (TransferActivityRecord) -> String
    let onRemoteLogTarget: (TransferActivityRecord) -> RemoteLogUpload.Target?
    let onRemoteDiagnosticReport: (TransferActivityRecord) -> String
    let onAppDiagnosticReport: () -> String
    let onPause: (String) -> Bool
    let onCanResume: (String) -> Bool
    let onResume: (String) -> Bool
    let onCancel: (String) -> Bool
    let onApprove: (String) -> Bool
    let onDelete: (String) -> Bool

    @Environment(\.appLanguage) private var language
    @AppStorage("envoix.developerMode") private var developerMode = false
    @State private var expandedActivityIDs: Set<String> = []
    #if os(iOS)
    @State private var previewFileURL: URL?
    @State private var receivedItemsPresentation: ReceivedItemsPresentation?
    #endif

    var body: some View {
        ScrollView {
            LazyVStack(alignment: .leading, spacing: 12) {
                if records.isEmpty {
                    VStack(spacing: 10) {
                        Image(systemName: "arrow.up.arrow.down.circle")
                            .font(.system(size: 36, weight: .medium))
                            .foregroundStyle(Theme.muted)
                        Text(AppText.value("No transfers yet", "暂无传输", language: language))
                            .font(.headline)
                            .foregroundStyle(Theme.text)
                        Text(AppText.value(
                            "Prepared and active transfers will appear here.",
                            "准备中和活动中的传输会显示在这里。",
                            language: language
                        ))
                            .font(.subheadline)
                            .foregroundStyle(Theme.muted)
                            .multilineTextAlignment(.center)
                    }
                    .frame(maxWidth: .infinity)
                    .padding(.vertical, 48)
                } else {
                    activitySection(
                        AppText.localized("activity.in_progress", language: language),
                        records: records.filter { ActivityProjectionPolicy.isPending($0.state) }
                    )
                    activitySection(
                        AppText.localized("activity.needs_attention", language: language),
                        records: records.filter { $0.state == .failed }
                    )
                    activitySection(
                        AppText.localized("activity.recent", language: language),
                        records: records.filter {
                            !ActivityProjectionPolicy.isPending($0.state) && $0.state != .failed
                        }
                    )
                }
            }
            .padding(.vertical, 4)
        }
        #if os(iOS)
        .quickLookPreview($previewFileURL)
        .sheet(item: $receivedItemsPresentation) { presentation in
            ReceivedItemsSheet(urls: presentation.urls)
        }
        #endif
    }

    @ViewBuilder
    private func activitySection(_ title: String, records: [TransferActivityRecord]) -> some View {
        if !records.isEmpty {
            Text(title)
                .font(.subheadline.weight(.semibold))
                .foregroundStyle(Theme.muted)
                .padding(.top, 4)
            ForEach(records) { record in
                activityCard(record)
            }
        }
    }

    private func activityCard(_ record: TransferActivityRecord) -> some View {
        let actions = activityActionAvailability(for: record)
        let progress = TransferPresentationPolicy.progress(for: record.state)
        let metrics = metricsByActivityID[record.activityId] ?? ActivityMetrics()
        let isExpanded = expandedActivityIDs.contains(record.activityId)
        return VStack(alignment: .leading, spacing: 12) {
            Button {
                withAnimation(.easeInOut(duration: 0.18)) {
                    if isExpanded {
                        expandedActivityIDs.remove(record.activityId)
                    } else {
                        expandedActivityIDs.insert(record.activityId)
                    }
                }
            } label: {
                HStack(alignment: .top, spacing: 11) {
                    Image(systemName: icon(for: record))
                        .font(.title2)
                        .foregroundStyle(tint(for: record.state))
                        .frame(width: 32)
                    VStack(alignment: .leading, spacing: 3) {
                        Text(title(for: record))
                            .font(.headline)
                            .foregroundStyle(Theme.text)
                        Text(stateText(record))
                            .font(.subheadline)
                            .foregroundStyle(Theme.muted)
                    }
                    Spacer()
                    Text(record.direction == .send
                        ? AppText.value("Send", "发送", language: language)
                        : AppText.value("Receive", "接收", language: language))
                        .font(.caption.weight(.semibold))
                        .foregroundStyle(Theme.muted)
                    Image(systemName: isExpanded ? "chevron.up" : "chevron.down")
                        .font(.caption.weight(.bold))
                        .foregroundStyle(Theme.muted)
                        .padding(.top, 2)
                }
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .accessibilityIdentifier("activity_\(record.activityId)")

            if record.state != .delivered, progress != .hidden, record.totalBytes > 0 {
                ProgressView(
                    value: Double(record.bytesTransferred),
                    total: Double(record.totalBytes)
                )
                HStack {
                    Text("\(byteString(record.bytesTransferred)) / \(byteString(record.totalBytes))")
                    Spacer()
                    if progress == .active, metrics.speedBps > 0 {
                        Text(rateString(metrics.speedBps))
                    }
                    if progress == .active, let eta = metrics.etaSeconds {
                        Text(etaString(eta))
                    }
                }
                .font(.caption.monospacedDigit())
                .foregroundStyle(Theme.muted)
            }

            if record.direction == .receive,
               record.state == .delivered,
               !record.savedPaths.isEmpty {
                completedReceiveControls(record)
            }

            if isExpanded {
                if let path = record.connectionPath {
                    Label(
                        ConnectionPathPresentationPolicy.label(for: path, language: language),
                        systemImage: path == .relay ? "point.3.connected.trianglepath.dotted" : "link"
                    )
                    .font(.footnote.weight(.semibold))
                    .foregroundStyle(Theme.muted)
                    .accessibilityIdentifier("activity_path_\(record.activityId)")
                }

                if !record.diagnosticMessage.isEmpty {
                    Text(record.diagnosticMessage)
                        .font(.footnote)
                        .foregroundStyle(record.state == .failed ? Theme.danger : Theme.muted)
                        .textSelection(.enabled)
                }

                HStack(spacing: 8) {
                    if actions.canApprove {
                        Button(AppText.value("Accept", "接收", language: language)) {
                            _ = onApprove(record.activityId)
                        }
                        .buttonStyle(.borderedProminent)
                    }
                    if actions.canPause {
                        Button(AppText.value("Pause", "暂停", language: language)) {
                            _ = onPause(record.activityId)
                        }
                    }
                    if actions.canResume, onCanResume(record.activityId) {
                        Button(AppText.value("Resume", "恢复", language: language)) {
                            _ = onResume(record.activityId)
                        }
                    }
                    if actions.canCancel {
                        Button(AppText.value("Cancel", "取消", language: language), role: .destructive) {
                            _ = onCancel(record.activityId)
                        }
                    }
                    Spacer()
                    if developerMode || actions.canDelete {
                        Menu {
                            if developerMode {
                                Button(AppText.value("Copy diagnostics", "复制诊断信息", language: language)) {
                                    copyWithToast(
                                        onCopyDiagnostics(record),
                                        AppText.value("Diagnostics copied", "诊断信息已复制", language: language),
                                        language: language
                                    )
                                }
                                if let target = onRemoteLogTarget(record) {
                                    Button(AppText.value("Upload diagnostics", "上传诊断信息", language: language)) {
                                        Task {
                                            try? await RemoteLogUpload.upload(
                                                server: UserDefaults.standard.string(
                                                    forKey: "envoix.logServer"
                                                ) ?? defaultLogServer,
                                                target: target,
                                                body: onRemoteDiagnosticReport(record)
                                            )
                                        }
                                    }
                                }
                                Button(AppText.value("Copy app diagnostics", "复制应用诊断信息", language: language)) {
                                    copyWithToast(
                                        onAppDiagnosticReport(),
                                        AppText.value("Diagnostics copied", "诊断信息已复制", language: language),
                                        language: language
                                    )
                                }
                            }
                            if actions.canDelete {
                                Button(
                                    AppText.localized("activity.remove_record", language: language),
                                    role: .destructive
                                ) {
                                    _ = onDelete(record.activityId)
                                }
                            }
                        } label: {
                            Image(systemName: "ellipsis.circle")
                                .frame(width: 32, height: 32)
                                .contentShape(Rectangle())
                        }
                        .disabled(pendingRemovalIDs.contains(record.activityId))
                    }
                }
                .buttonStyle(.bordered)
                .transition(.opacity.combined(with: .move(edge: .top)))
            }
        }
        .card(padding: 14)
    }

    private func completedReceiveControls(_ record: TransferActivityRecord) -> some View {
        let urls = record.savedPaths.map { URL(fileURLWithPath: $0) }
        return VStack(alignment: .leading, spacing: 8) {
            Label(completedDestinationText(urls), systemImage: "folder.fill")
                .font(.subheadline.weight(.semibold))
                .foregroundStyle(Theme.text)

            Text(urls.prefix(3).map(\.lastPathComponent).joined(separator: " · "))
                .font(.caption)
                .foregroundStyle(Theme.muted)
                .lineLimit(2)
                .truncationMode(.middle)

            HStack(spacing: 8) {
                #if os(macOS)
                Button(platformRevealTitle(language: language)) {
                    revealInFinder(urls)
                }
                #elseif os(iOS)
                if urls.count == 1, let firstURL = urls.first, isRegularFileURL(firstURL) {
                    Button(platformRevealTitle(language: language)) {
                        previewFileURL = firstURL
                    }
                    ShareLink(item: firstURL) {
                        Label(
                            AppText.value("Share", "分享", language: language),
                            systemImage: "square.and.arrow.up"
                        )
                    }
                } else {
                    Button {
                        receivedItemsPresentation = ReceivedItemsPresentation(urls: urls)
                    } label: {
                        Label(
                            AppText.value("View received items", "查看已接收项目", language: language),
                            systemImage: "square.stack"
                        )
                    }
                }
                #endif
            }
            .buttonStyle(.bordered)
        }
        .accessibilityIdentifier("activity_saved_items_\(record.activityId)")
    }

    private func completedDestinationText(_ urls: [URL]) -> String {
        let parentPaths = Set(urls.map { $0.deletingLastPathComponent().path })
        if parentPaths.count == 1, let parent = urls.first?.deletingLastPathComponent() {
            return AppText.value(
                "Saved in \(parent.lastPathComponent)",
                "已保存到 \(parent.lastPathComponent)",
                language: language
            )
        }
        return AppText.value(
            "Saved \(urls.count) items",
            "已保存 \(urls.count) 个项目",
            language: language
        )
    }

    private func icon(for record: TransferActivityRecord) -> String {
        switch record.state {
        case .delivered:
            return record.direction == .send
                ? "envelope.badge.fill"
                : "tray.and.arrow.down.fill"
        case .failed:
            return "exclamationmark.triangle.fill"
        case .canceled:
            return "xmark.circle.fill"
        case .paused:
            return "pause.circle.fill"
        case .saving, .waitingForReceiverSave, .finalizingDelivery:
            return "tray.and.arrow.down.fill"
        case .waitingForPeer, .pairing, .connecting:
            return "envelope"
        case .awaitingDecision:
            return "checklist"
        default:
            return record.direction == .send ? "paperplane.fill" : "envelope.open.fill"
        }
    }

    private func title(for record: TransferActivityRecord) -> String {
        let count = Int(record.itemCount)
        if count == 0 {
            return record.direction == .send
                ? AppText.value("Outgoing transfer", "待发送内容", language: language)
                : AppText.value("Incoming transfer", "待接收内容", language: language)
        }
        if count == 1 { return AppText.value("1 item", "1 个项目", language: language) }
        return AppText.value("\(count) items", "\(count) 个项目", language: language)
    }

    private func stateText(_ record: TransferActivityRecord) -> String {
        switch record.state {
        case .preparing: return AppText.value("Preparing locally", "正在本地准备", language: language)
        case .waitingForPeer: return AppText.value("Waiting for peer", "正在等待对端", language: language)
        case .pairing: return AppText.value("Pairing", "正在配对", language: language)
        case .connecting: return AppText.value("Connecting", "正在连接", language: language)
        case .awaitingDecision: return AppText.value("Waiting for your decision", "等待你的确认", language: language)
        case .transferring:
            return record.direction == .send
                ? AppText.value("Sending", "正在发送", language: language)
                : AppText.value("Receiving", "正在接收", language: language)
        case .verifying: return AppText.value("Verifying", "正在校验", language: language)
        case .saving: return AppText.value("Saving to destination", "正在保存到目标位置", language: language)
        case .waitingForReceiverSave: return AppText.value("Waiting for receiver to save", "等待接收方完成保存", language: language)
        case .finalizingDelivery: return AppText.value("Saved; finalizing delivery", "已保存，正在完成交付确认", language: language)
        case .paused: return AppText.value("Paused", "已暂停", language: language)
        case .delivered:
            return record.direction == .send
                ? AppText.value("Delivered", "已送达", language: language)
                : AppText.value("Received", "已接收", language: language)
        case .failed: return AppText.value("Failed", "失败", language: language)
        case .canceled: return AppText.value("Canceled", "已取消", language: language)
        }
    }

    private func tint(for state: TransferActivityState) -> Color {
        switch state {
        case .delivered: return Theme.success
        case .failed: return Theme.danger
        case .canceled: return Theme.muted
        case .awaitingDecision, .paused: return Theme.warning
        default: return Theme.accentStrong
        }
    }
}
