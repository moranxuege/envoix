#if os(iOS) || os(macOS)
import QuickLook
import SwiftUI

struct RememberedRoomView: View {
    @Environment(\.appLanguage) private var language

    let room: RememberedRoomSession
    let status: RememberedRoomConnectionStatus
    let peerDisplayName: String?
    let incomingOffer: RoomControlTransferOffer?
    let isAcceptingOffer: Bool
    let outboxEntries: [RememberedRoomOutboxEntry]
    let outboxError: String?
    let records: [TransferActivityRecord]
    let metricsByActivityID: [String: ActivityMetrics]
    let onAddFiles: () -> Void
    let onAcceptOffer: () -> Void
    let onRejectOffer: () -> Void
    let onRetryOutboxEntry: (String) -> Void
    let onRemoveOutboxEntry: (RememberedRoomOutboxEntry) -> Void
    let onShowActivity: () -> Void
    let onDisconnect: () -> Void
    let onForget: () -> Void

    @State private var isForgetConfirmationPresented = false
    @State private var previewFileURL: URL?
    @State private var receivedItemsPresentation: ReceivedItemsPresentation?

    var body: some View {
        ScrollView {
            LazyVStack(alignment: .leading, spacing: 14) {
                connectionCard
                if let incomingOffer {
                    incomingOfferCard(incomingOffer)
                }
                outboxSection
                activitySection
            }
            .padding(.horizontal, 16)
            .padding(.top, 12)
            .padding(.bottom, 150)
        }
        .safeAreaInset(edge: .bottom) {
            controls
        }
        .background(Theme.bg)
        .accessibilityIdentifier("remembered_room")
        .alert(
            AppText.value("Forget this room?", "忘记这个房间？", language: language),
            isPresented: $isForgetConfirmationPresented
        ) {
            Button(AppText.value("Cancel", "取消", language: language), role: .cancel) {}
            Button(AppText.value("Forget room", "忘记房间", language: language), role: .destructive) {
                onForget()
            }
        } message: {
            Text(AppText.value(
                outboxEntries.isEmpty
                    ? "You will need to pair with this device again."
                    : "Queued files for this room will be removed. You will need to pair again.",
                outboxEntries.isEmpty
                    ? "之后需要与此设备重新配对。"
                    : "此房间的待发送文件会被移除，之后需要重新配对。",
                language: language
            ))
        }
        .quickLookPreview($previewFileURL)
        .sheet(item: $receivedItemsPresentation) { presentation in
            ReceivedItemsSheet(urls: presentation.urls)
        }
    }

    private var connectionCard: some View {
        HStack(spacing: 13) {
            Image(systemName: statusIcon)
                .font(.title2)
                .foregroundStyle(statusTint)
                .frame(width: 36)

            VStack(alignment: .leading, spacing: 4) {
                Text(peerDisplayName?.trimmed.isEmpty == false
                     ? peerDisplayName!
                     : room.label)
                    .font(.headline.weight(.semibold))
                    .foregroundStyle(Theme.text)
                Text(statusText)
                    .font(.subheadline)
                    .foregroundStyle(Theme.muted)
                    .fixedSize(horizontal: false, vertical: true)
            }
            Spacer(minLength: 8)
            if isConnecting {
                ProgressView()
                    .tint(Theme.accentStrong)
            }
        }
        .card(raised: true, padding: 16)
        .accessibilityIdentifier("remembered_room_status")
    }

    private func incomingOfferCard(
        _ offer: RoomControlTransferOffer
    ) -> some View {
        VStack(alignment: .leading, spacing: 10) {
            Label(
                AppText.value("Incoming files", "收到文件邀请", language: language),
                systemImage: "arrow.down.doc.fill"
            )
            .font(.headline.weight(.semibold))
            .foregroundStyle(Theme.text)

            Text(offerSummary(offer))
                .font(.subheadline)
                .foregroundStyle(Theme.muted)

            HStack(spacing: 10) {
                Button(role: .cancel, action: onRejectOffer) {
                    Text(AppText.value("Decline", "拒绝", language: language))
                        .frame(maxWidth: .infinity)
                }
                .buttonStyle(.bordered)
                .disabled(isAcceptingOffer)

                Button(action: onAcceptOffer) {
                    if isAcceptingOffer {
                        HStack(spacing: 8) {
                            ProgressView()
                            Text(AppText.value(
                                "Preparing receiver…",
                                "正在准备接收…",
                                language: language
                            ))
                        }
                        .frame(maxWidth: .infinity)
                    } else {
                        Text(AppText.value("Receive", "接收", language: language))
                            .frame(maxWidth: .infinity)
                    }
                }
                .buttonStyle(.borderedProminent)
                .tint(Theme.accentStrong)
                .disabled(isAcceptingOffer)
                .accessibilityLabel(AppText.value(
                    isAcceptingOffer ? "Preparing receiver…" : "Receive",
                    isAcceptingOffer ? "正在准备接收…" : "接收",
                    language: language
                ))
            }
        }
        .card(raised: true, padding: 16)
        .accessibilityIdentifier("remembered_room_incoming_offer")
    }

    @ViewBuilder
    private var outboxSection: some View {
        if !outboxEntries.isEmpty || outboxError != nil {
            VStack(alignment: .leading, spacing: 10) {
                Text(AppText.value("Files for this room", "此房间的文件", language: language))
                    .font(.headline.weight(.semibold))
                    .foregroundStyle(Theme.text)

                if let outboxError {
                    Label(outboxError, systemImage: "exclamationmark.triangle.fill")
                        .font(.footnote)
                        .foregroundStyle(Theme.danger)
                }

                ForEach(outboxEntries) { entry in
                    VStack(alignment: .leading, spacing: 7) {
                        HStack(alignment: .firstTextBaseline, spacing: 10) {
                            Image(systemName: outboxIcon(entry.state))
                                .foregroundStyle(outboxTint(entry.state))
                            VStack(alignment: .leading, spacing: 3) {
                                Text(outboxTitle(entry))
                                    .font(.subheadline.weight(.semibold))
                                    .foregroundStyle(Theme.text)
                                    .lineLimit(2)
                                Text(outboxSummary(entry))
                                    .font(.caption)
                                    .foregroundStyle(Theme.muted)
                            }
                            Spacer(minLength: 8)
                            Text(outboxStateText(entry.state))
                                .font(.caption.weight(.semibold))
                                .foregroundStyle(outboxTint(entry.state))
                        }

                        if let error = entry.lastError, !error.isEmpty {
                            Text(error)
                                .font(.caption)
                                .foregroundStyle(Theme.danger)
                                .fixedSize(horizontal: false, vertical: true)
                        }

                        if entry.state == .needsAttention {
                            HStack(spacing: 10) {
                                Button(AppText.value("Retry", "重试", language: language)) {
                                    onRetryOutboxEntry(entry.id)
                                }
                                .buttonStyle(.borderedProminent)
                                .tint(Theme.accentStrong)

                                Button(
                                    AppText.value("Remove", "移除", language: language),
                                    role: .destructive
                                ) {
                                    onRemoveOutboxEntry(entry)
                                }
                                .buttonStyle(.bordered)
                            }
                        } else if entry.state == .queued {
                            Button(
                                AppText.value("Remove", "移除", language: language),
                                role: .destructive
                            ) {
                                onRemoveOutboxEntry(entry)
                            }
                            .font(.caption.weight(.semibold))
                        }
                    }
                    .padding(.vertical, 5)

                    if entry.id != outboxEntries.last?.id {
                        Divider()
                    }
                }
            }
            .card(padding: 16)
            .accessibilityIdentifier("remembered_room_outbox")
        }
    }

    @ViewBuilder
    private var activitySection: some View {
        if records.isEmpty {
            VStack(alignment: .leading, spacing: 8) {
                Text(AppText.value("No transfers yet", "暂无传输", language: language))
                    .font(.headline.weight(.semibold))
                    .foregroundStyle(Theme.text)
                Text(AppText.value(
                    "Either member can offer files after the room reconnects.",
                    "房间重新连接后，任意一方都可以发送文件。",
                    language: language
                ))
                .font(.subheadline)
                .foregroundStyle(Theme.muted)
            }
            .card(padding: 16)
        } else {
            VStack(alignment: .leading, spacing: 10) {
                HStack {
                    Text(AppText.value("Room activity", "房间活动", language: language))
                        .font(.headline.weight(.semibold))
                    Spacer()
                    Button(AppText.value("View all", "查看全部", language: language), action: onShowActivity)
                        .font(.subheadline.weight(.semibold))
                }
                ForEach(records.prefix(4), id: \.activityId) { record in
                    let progress = TransferPresentationPolicy.progress(for: record.state)
                    let metrics = metricsByActivityID[record.activityId] ?? ActivityMetrics()
                    VStack(alignment: .leading, spacing: 6) {
                        HStack {
                            Label(
                                record.direction == .send
                                    ? AppText.value("Sent files", "发送文件", language: language)
                                    : AppText.value("Received files", "接收文件", language: language),
                                systemImage: record.direction == .send
                                    ? "arrow.up.circle.fill"
                                    : "arrow.down.circle.fill"
                            )
                            .font(.subheadline.weight(.semibold))
                            Spacer()
                            Text(activityStateText(record.state))
                                .font(.caption)
                                .foregroundStyle(Theme.muted)
                        }
                        if record.totalBytes > 0,
                           record.state != .delivered,
                           progress != .hidden {
                            ProgressView(
                                value: Double(record.bytesTransferred),
                                total: Double(record.totalBytes)
                            )
                            Text(
                                "\(byteString(record.bytesTransferred)) / "
                                    + byteString(record.totalBytes)
                            )
                            .font(.caption.monospacedDigit())
                            .foregroundStyle(Theme.muted)
                        }
                        TransferPerformanceLine(
                            currentBytesPerSecond: progress == .active ? metrics.speedBps : 0,
                            averageBytesPerSecond: metrics.averageSpeedBps,
                            etaSeconds: progress == .active ? metrics.etaSeconds : nil,
                            currentSampleDate: metrics.currentRateUpdatedAt,
                            accessibilityPrefix: "remembered_room_activity_\(record.activityId)"
                        )
                        if let path = record.connectionPath {
                            Label(
                                ConnectionPathPresentationPolicy.label(
                                    for: path,
                                    language: language
                                ),
                                systemImage: path == .wifiAware ? "wifi" : "link"
                            )
                            .font(.caption.weight(.semibold))
                            .foregroundStyle(Theme.muted)
                            .accessibilityIdentifier(
                                "remembered_room_activity_path_\(record.activityId)"
                            )
                        }
                        if record.direction == .receive,
                           record.state == .delivered,
                           !record.savedPaths.isEmpty {
                            let urls = record.savedPaths.map { URL(fileURLWithPath: $0) }
                            HStack(spacing: 8) {
                                Label(savedDestination(record), systemImage: "folder.fill")
                                    .font(.caption.weight(.semibold))
                                    .foregroundStyle(Theme.text)
                                    .lineLimit(1)
                                    .truncationMode(.middle)

                                Spacer(minLength: 8)

                                Button {
                                    openReceivedItems(urls)
                                } label: {
                                    Text(AppText.value("Open", "打开", language: language))
                                        .font(.caption.weight(.semibold))
                                }
                                .buttonStyle(.bordered)
                                .accessibilityIdentifier(
                                    "remembered_room_open_received_\(record.activityId)"
                                )

                                ShareLink(items: urls) {
                                    Text(AppText.value("Share", "分享", language: language))
                                        .font(.caption.weight(.semibold))
                                }
                                .buttonStyle(.bordered)
                                .accessibilityIdentifier(
                                    "remembered_room_share_received_\(record.activityId)"
                                )
                            }
                        }
                    }
                    .padding(.vertical, 4)
                }
            }
            .card(padding: 16)
        }
    }

    private func openReceivedItems(_ urls: [URL]) {
        guard let first = urls.first else { return }
        if urls.count == 1, isRegularFileURL(first) {
            previewFileURL = first
        } else {
            receivedItemsPresentation = ReceivedItemsPresentation(urls: urls)
        }
    }

    private var controls: some View {
        VStack(spacing: 9) {
            Button(action: onAddFiles) {
                Label(
                    AppText.value("Add files", "添加文件", language: language),
                    systemImage: "plus"
                )
                .frame(maxWidth: .infinity, minHeight: 44)
            }
            .buttonStyle(PrimaryActionButtonStyle())
            .disabled(incomingOffer != nil)
            .accessibilityIdentifier("remembered_room_add_files")

            if status == .connected || isConnecting {
                Button(role: .destructive, action: onDisconnect) {
                    Text(AppText.value(
                        "Disconnect for now",
                        "暂时断开连接",
                        language: language
                    ))
                    .frame(maxWidth: .infinity, minHeight: 40)
                }
                .buttonStyle(.bordered)
            } else {
                Label(
                    AppText.value(
                        "Reconnects automatically while both apps are open",
                        "双方打开应用时会自动重新连接",
                        language: language
                    ),
                    systemImage: "info.circle"
                )
                .font(.caption)
                .foregroundStyle(statusTint)
                .frame(maxWidth: .infinity, alignment: .leading)
            }

            Button(role: .destructive) {
                isForgetConfirmationPresented = true
            } label: {
                Text(AppText.value("Forget room", "忘记房间", language: language))
                    .font(.footnote.weight(.semibold))
            }
        }
        .padding(.horizontal, 16)
        .padding(.top, 12)
        .padding(.bottom, 8)
        .background(.regularMaterial)
        .overlay(alignment: .top) {
            Rectangle()
                .fill(Theme.line.opacity(0.7))
                .frame(height: 0.5)
        }
    }

    private var isConnecting: Bool {
        status == .connecting || status == .waiting
    }

    private var statusText: String {
        switch status {
        case .offline:
            return AppText.value(
                "Offline · waiting for the other app",
                "离线 · 等待另一台设备打开应用",
                language: language
            )
        case .available:
            return AppText.value("Ready to send", "可发送", language: language)
        case .connecting:
            return AppText.value("Reconnecting securely…", "正在安全重连…", language: language)
        case .waiting:
            return AppText.value(
                "Ready for the other device…",
                "正在等待另一台设备…",
                language: language
            )
        case .connected:
            return AppText.value(
                "Connected · either member can send",
                "已连接 · 双方均可发送",
                language: language
            )
        case .needsRepair(let message):
            return message
        }
    }

    private var statusIcon: String {
        switch status {
        case .offline: return "wifi.slash"
        case .available: return "paperplane.circle.fill"
        case .connecting: return "arrow.triangle.2.circlepath"
        case .waiting: return "antenna.radiowaves.left.and.right"
        case .connected: return "checkmark.circle.fill"
        case .needsRepair: return "exclamationmark.triangle.fill"
        }
    }

    private var statusTint: Color {
        switch status {
        case .available, .connected: return Theme.success
        case .connecting, .waiting: return Theme.accentStrong
        case .needsRepair: return Theme.danger
        case .offline: return Theme.muted
        }
    }

    private func offerSummary(_ offer: RoomControlTransferOffer) -> String {
        let names = offer.rootNames.prefix(3).joined(separator: ", ")
        let count = AppText.value(
            "\(offer.itemCount) items",
            "\(offer.itemCount) 个项目",
            language: language
        )
        return names.isEmpty ? count : "\(names) · \(count)"
    }

    private func activityStateText(_ state: TransferActivityState) -> String {
        switch state {
        case .preparing: return AppText.value("Preparing", "正在准备", language: language)
        case .pairing: return AppText.value("Pairing", "正在配对", language: language)
        case .connecting: return AppText.value("Connecting", "正在连接", language: language)
        case .waitingForPeer: return AppText.value("Waiting", "正在等待", language: language)
        case .transferring: return AppText.value("Transferring", "正在传输", language: language)
        case .verifying: return AppText.value("Verifying", "正在校验", language: language)
        case .saving: return AppText.value("Saving", "正在保存", language: language)
        case .waitingForReceiverSave: return AppText.value("Finalizing", "正在完成", language: language)
        case .finalizingDelivery: return AppText.value("Finalizing", "正在完成", language: language)
        case .awaitingDecision: return AppText.value("Needs attention", "需要处理", language: language)
        case .paused: return AppText.value("Paused", "已暂停", language: language)
        case .delivered: return AppText.value("Delivered", "已送达", language: language)
        case .failed: return AppText.value("Failed", "失败", language: language)
        case .canceled: return AppText.value("Canceled", "已取消", language: language)
        }
    }

    private func outboxTitle(_ entry: RememberedRoomOutboxEntry) -> String {
        if !entry.rootNames.isEmpty {
            return entry.rootNames.joined(separator: ", ")
        }
        return AppText.value("Prepared files", "已准备文件", language: language)
    }

    private func savedDestination(_ record: TransferActivityRecord) -> String {
        let urls = record.savedPaths.map { URL(fileURLWithPath: $0) }
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

    private func outboxSummary(_ entry: RememberedRoomOutboxEntry) -> String {
        let itemText = AppText.value(
            "\(entry.itemCount) items",
            "\(entry.itemCount) 个项目",
            language: language
        )
        guard entry.totalBytes > 0 else { return itemText }
        let size = ByteCountFormatter.string(
            fromByteCount: Int64(clamping: entry.totalBytes),
            countStyle: .file
        )
        return "\(itemText) · \(size)"
    }

    private func outboxStateText(_ state: RememberedRoomOutboxState) -> String {
        switch state {
        case .queued:
            return AppText.value("Queued", "等待发送", language: language)
        case .offering:
            return AppText.value("Offering", "正在邀请", language: language)
        case .transferring:
            return AppText.value("Sending", "正在发送", language: language)
        case .needsAttention:
            return AppText.value("Check", "需处理", language: language)
        }
    }

    private func outboxIcon(_ state: RememberedRoomOutboxState) -> String {
        switch state {
        case .queued: return "clock.fill"
        case .offering: return "paperplane.fill"
        case .transferring: return "arrow.up.circle.fill"
        case .needsAttention: return "exclamationmark.triangle.fill"
        }
    }

    private func outboxTint(_ state: RememberedRoomOutboxState) -> Color {
        switch state {
        case .queued: return Theme.muted
        case .offering, .transferring: return Theme.accentStrong
        case .needsAttention: return Theme.danger
        }
    }
}
#endif
