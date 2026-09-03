#if os(iOS) || os(macOS)
import EnvoixCore
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

#if os(macOS)
enum MacOSAgentTransferPresentationPolicy {
    static func isTerminal(_ state: FfiApplicationTransferState) -> Bool {
        switch state {
        case .delivered, .rejected, .failed, .canceled:
            return true
        case .offered, .queued, .connecting, .transferring, .paused,
             .awaitingDeliveryProof:
            return false
        }
    }

    static func showsProgress(_ state: FfiApplicationTransferState) -> Bool {
        switch state {
        case .connecting, .transferring, .paused, .awaitingDeliveryProof:
            return true
        case .offered, .queued, .delivered, .rejected, .failed, .canceled:
            return false
        }
    }

    static func stateText(
        _ transfer: FfiApplicationTransfer,
        language: String
    ) -> String {
        switch transfer.state {
        case .offered:
            return AppText.value("Awaiting approval", "等待接收确认", language: language)
        case .queued:
            return AppText.value("Queued", "等待发送", language: language)
        case .connecting:
            return AppText.value("Connecting", "正在连接", language: language)
        case .transferring:
            return transfer.direction == .send
                ? AppText.value("Sending", "正在发送", language: language)
                : AppText.value("Receiving", "正在接收", language: language)
        case .paused:
            return AppText.value("Paused", "已暂停", language: language)
        case .awaitingDeliveryProof:
            return AppText.value("Verifying delivery", "正在确认送达", language: language)
        case .delivered:
            return transfer.direction == .send
                ? AppText.value("Delivered", "已送达", language: language)
                : AppText.value("Received", "已接收", language: language)
        case .rejected:
            return AppText.value("Rejected", "已拒绝", language: language)
        case .failed:
            return AppText.value("Failed", "失败", language: language)
        case .canceled:
            return AppText.value("Canceled", "已取消", language: language)
        }
    }

    static func detail(
        _ transfer: FfiApplicationTransfer,
        language: String
    ) -> String? {
        if let failure = transfer.failure {
            return friendlyFailure(
                code: failure.code,
                diagnosticMessage: "",
                language: language
            )
        }
        if let rejection = transfer.rejection {
            switch rejection {
            case .userDeclined:
                return AppText.value(
                    "The receiving device declined this transfer.",
                    "接收设备拒绝了此传输。",
                    language: language
                )
            case .busy:
                return AppText.value(
                    "The receiving device is busy. Send the files again later.",
                    "接收设备正忙，请稍后重新发送。",
                    language: language
                )
            case .insufficientSpace:
                return AppText.value(
                    "The receiving device does not have enough free space.",
                    "接收设备没有足够的可用空间。",
                    language: language
                )
            case .unsupportedContent:
                return AppText.value(
                    "The receiving device does not support this content.",
                    "接收设备不支持此内容。",
                    language: language
                )
            case .invalidOffer:
                return AppText.value(
                    "The receiving device could not validate this offer.",
                    "接收设备无法验证此发送邀请。",
                    language: language
                )
            }
        }
        switch transfer.state {
        case .queued:
            return AppText.value(
                "Waiting for the paired device. The helper will retry in the background.",
                "正在等待已配对设备；helper 会在后台继续重试。",
                language: language
            )
        case .awaitingDeliveryProof:
            return AppText.value(
                "All bytes were sent. Waiting for the receiver to confirm a durable save.",
                "文件数据已发完，正在等待接收端确认已安全保存。",
                language: language
            )
        case .paused:
            return AppText.value(
                "This transfer is paused and remains in the helper queue.",
                "此传输已暂停，并保留在 helper 队列中。",
                language: language
            )
        default:
            return nil
        }
    }

    static func pathText(_ path: FfiAgentPathKind, language: String) -> String {
        switch path {
        case .lan:
            return AppText.value("Local network", "局域网", language: language)
        case .direct:
            return AppText.value("Direct connection", "直连", language: language)
        case .relay:
            return AppText.value("Relay", "中继", language: language)
        case .wifiAware:
            return AppText.value("Wi-Fi Aware", "Wi-Fi Aware", language: language)
        case .other:
            return AppText.value("Network connection", "网络连接", language: language)
        }
    }
}

struct MacOSAgentRoomView: View {
    @Environment(\.appLanguage) private var language

    let device: MacOSAgentDevice
    let transfers: [FfiApplicationTransfer]
    let activePaths: [FfiAgentTransferPath]
    let isPreparing: Bool
    let loadError: String?
    let onAddFiles: () -> Void
    let onShowActivity: () -> Void

    var body: some View {
        ScrollView {
            LazyVStack(alignment: .leading, spacing: 14) {
                connectionCard
                staleSnapshotWarning
                transferSection
            }
            .padding(.horizontal, 16)
            .padding(.top, 12)
            .padding(.bottom, 120)
        }
        .safeAreaInset(edge: .bottom) {
            controls
        }
        .background(Theme.bg)
        .accessibilityIdentifier("agent_room")
    }

    private var connectionCard: some View {
        HStack(spacing: 13) {
            Image(systemName: connectionIcon)
                .font(.title2)
                .foregroundStyle(connectionTint)
                .frame(width: 36)

            VStack(alignment: .leading, spacing: 4) {
                Text(device.label)
                    .font(.headline.weight(.semibold))
                    .foregroundStyle(Theme.text)
                Text(connectionText)
                    .font(.subheadline)
                    .foregroundStyle(Theme.muted)
                    .fixedSize(horizontal: false, vertical: true)
            }
            Spacer(minLength: 8)
            if isPreparing || hasPendingTransfer {
                ProgressView()
                    .tint(Theme.accentStrong)
            }
        }
        .card(raised: true, padding: 16)
        .accessibilityIdentifier("agent_room_status")
    }

    @ViewBuilder
    private var staleSnapshotWarning: some View {
        if loadError != nil {
            Label(
                AppText.value(
                    "The helper is temporarily unavailable. Showing the last known status.",
                    "helper 暂时不可用，当前显示上次已知状态。",
                    language: language
                ),
                systemImage: "exclamationmark.triangle.fill"
            )
            .font(.footnote)
            .foregroundStyle(Theme.danger)
            .card(padding: 14)
            .accessibilityIdentifier("agent_room_snapshot_warning")
        }
    }

    private var transferSection: some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack {
                Text(AppText.value("Room activity", "房间活动", language: language))
                    .font(.headline.weight(.semibold))
                Spacer()
                if !transfers.isEmpty {
                    Button(
                        AppText.value("View all", "查看全部", language: language),
                        action: onShowActivity
                    )
                    .font(.subheadline.weight(.semibold))
                }
            }

            if transfers.isEmpty {
                Text(AppText.value(
                    "No transfers yet. Files added here are owned by the background helper and remain queued while the other device is offline.",
                    "暂无传输。在这里添加的文件由后台 helper 管理；另一台设备离线时会保留在队列中。",
                    language: language
                ))
                .font(.subheadline)
                .foregroundStyle(Theme.muted)
                .fixedSize(horizontal: false, vertical: true)
            } else {
                ForEach(Array(transfers.prefix(6)), id: \.id) { transfer in
                    MacOSAgentTransferCard(
                        transfer: transfer,
                        deviceLabel: nil,
                        path: path(for: transfer.id)
                    )
                    if transfer.id != transfers.prefix(6).last?.id {
                        Divider()
                    }
                }
            }
        }
        .card(padding: 16)
        .accessibilityIdentifier("agent_room_activity")
    }

    private var controls: some View {
        VStack(spacing: 8) {
            Button(action: onAddFiles) {
                if isPreparing {
                    HStack(spacing: 8) {
                        ProgressView()
                        Text(AppText.value(
                            "Preparing files…",
                            "正在准备文件…",
                            language: language
                        ))
                    }
                    .frame(maxWidth: .infinity, minHeight: 44)
                } else {
                    Label(
                        AppText.value("Add files", "添加文件", language: language),
                        systemImage: "plus"
                    )
                    .frame(maxWidth: .infinity, minHeight: 44)
                }
            }
            .buttonStyle(PrimaryActionButtonStyle())
            .disabled(isPreparing)
            .accessibilityIdentifier("agent_room_add_files")

            Label(
                AppText.value(
                    "The helper keeps this secure room available when the window closes.",
                    "窗口关闭后，helper 仍会维护这个安全房间。",
                    language: language
                ),
                systemImage: "lock.shield"
            )
            .font(.caption)
            .foregroundStyle(Theme.muted)
            .frame(maxWidth: .infinity, alignment: .leading)
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

    private var hasPendingTransfer: Bool {
        transfers.contains {
            !MacOSAgentTransferPresentationPolicy.isTerminal($0.state)
        }
    }

    private var hasActivePath: Bool {
        let transferIDs = Set(transfers.map(\.id))
        return activePaths.contains { transferIDs.contains($0.transferId) }
    }

    private var connectionText: String {
        if isPreparing {
            return AppText.value(
                "Preparing files in the background helper…",
                "正在后台 helper 中准备文件…",
                language: language
            )
        }
        if hasActivePath {
            return AppText.value(
                "Connected · transferring securely",
                "已连接 · 正在安全传输",
                language: language
            )
        }
        if hasPendingTransfer {
            return AppText.value(
                "Waiting for the paired device · retrying in the background",
                "正在等待已配对设备 · 后台持续重试",
                language: language
            )
        }
        return AppText.value(
            "Ready · files will queue until the other device is online",
            "就绪 · 另一台设备上线前文件会保留在队列中",
            language: language
        )
    }

    private var connectionIcon: String {
        if hasActivePath { return "checkmark.circle.fill" }
        if isPreparing || hasPendingTransfer { return "arrow.triangle.2.circlepath" }
        return "paperplane.circle.fill"
    }

    private var connectionTint: Color {
        if hasActivePath { return Theme.success }
        if isPreparing || hasPendingTransfer { return Theme.accentStrong }
        return Theme.success
    }

    private func path(for transferID: String) -> FfiAgentPathKind? {
        activePaths.first { $0.transferId == transferID }?.path
    }
}

struct MacOSAgentActivityView: View {
    @Environment(\.appLanguage) private var language

    let transfers: [FfiApplicationTransfer]
    let devices: [MacOSAgentDevice]
    let activePaths: [FfiAgentTransferPath]
    let hasLoadedSnapshot: Bool
    let loadError: String?

    var body: some View {
        ScrollView {
            LazyVStack(alignment: .leading, spacing: 12) {
                if let loadError, !loadError.isEmpty {
                    Label(
                        AppText.value(
                            "Could not refresh the helper. Showing the last known status.",
                            "无法刷新 helper，当前显示上次已知状态。",
                            language: language
                        ),
                        systemImage: "exclamationmark.triangle.fill"
                    )
                    .font(.footnote)
                    .foregroundStyle(Theme.danger)
                    .card(padding: 14)
                    .accessibilityIdentifier("agent_activity_snapshot_warning")
                }

                if transfers.isEmpty {
                    emptyState
                } else {
                    ForEach(transfers, id: \.id) { transfer in
                        MacOSAgentTransferCard(
                            transfer: transfer,
                            deviceLabel: deviceLabel(for: transfer.relationshipId),
                            path: path(for: transfer.id)
                        )
                        .card(raised: true, padding: 16)
                    }
                }
            }
            .padding(.vertical, 4)
        }
        .accessibilityIdentifier("agent_activity")
    }

    private var emptyState: some View {
        VStack(spacing: 10) {
            if !hasLoadedSnapshot, loadError == nil {
                ProgressView()
            } else {
                Image(systemName: "arrow.up.arrow.down.circle")
                    .font(.system(size: 36, weight: .medium))
                    .foregroundStyle(Theme.muted)
            }
            Text(AppText.value(
                hasLoadedSnapshot ? "No helper transfers yet" : "Loading helper activity…",
                hasLoadedSnapshot ? "暂无 helper 传输" : "正在载入 helper 活动…",
                language: language
            ))
            .font(.headline)
            .foregroundStyle(Theme.text)
            if hasLoadedSnapshot {
                Text(AppText.value(
                    "Transfers sent to paired devices will appear here.",
                    "发送到已配对设备的传输会显示在这里。",
                    language: language
                ))
                .font(.subheadline)
                .foregroundStyle(Theme.muted)
                .multilineTextAlignment(.center)
            }
        }
        .frame(maxWidth: .infinity)
        .padding(.vertical, 48)
    }

    private func deviceLabel(for relationshipID: String) -> String {
        devices.first { $0.id == relationshipID }?.label
            ?? AppText.value("Paired device", "已配对设备", language: language)
    }

    private func path(for transferID: String) -> FfiAgentPathKind? {
        activePaths.first { $0.transferId == transferID }?.path
    }
}

private struct MacOSAgentTransferCard: View {
    @Environment(\.appLanguage) private var language

    let transfer: FfiApplicationTransfer
    let deviceLabel: String?
    let path: FfiAgentPathKind?

    var body: some View {
        VStack(alignment: .leading, spacing: 9) {
            HStack(alignment: .top, spacing: 11) {
                Image(systemName: icon)
                    .font(.title3)
                    .foregroundStyle(tint)
                    .frame(width: 28)
                VStack(alignment: .leading, spacing: 3) {
                    Text(title)
                        .font(.subheadline.weight(.semibold))
                        .foregroundStyle(Theme.text)
                    Text(summary)
                        .font(.caption)
                        .foregroundStyle(Theme.muted)
                }
                Spacer(minLength: 8)
                Text(MacOSAgentTransferPresentationPolicy.stateText(
                    transfer,
                    language: language
                ))
                .font(.caption.weight(.semibold))
                .foregroundStyle(tint)
            }

            if MacOSAgentTransferPresentationPolicy.showsProgress(transfer.state),
               transfer.totalBytes > 0 {
                ProgressView(
                    value: Double(min(transfer.transferredBytes, transfer.totalBytes)),
                    total: Double(transfer.totalBytes)
                )
                Text(
                    "\(byteString(transfer.transferredBytes)) / "
                        + byteString(transfer.totalBytes)
                )
                .font(.caption.monospacedDigit())
                .foregroundStyle(Theme.muted)
            }

            if let path {
                Label(
                    MacOSAgentTransferPresentationPolicy.pathText(
                        path,
                        language: language
                    ),
                    systemImage: path == .wifiAware ? "wifi" : "link"
                )
                .font(.caption.weight(.semibold))
                .foregroundStyle(Theme.muted)
            }

            if let detail = MacOSAgentTransferPresentationPolicy.detail(
                transfer,
                language: language
            ) {
                Text(detail)
                    .font(.footnote)
                    .foregroundStyle(
                        transfer.state == .failed || transfer.state == .rejected
                            ? Theme.danger
                            : Theme.muted
                    )
                    .fixedSize(horizontal: false, vertical: true)
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier("agent_transfer_\(transfer.id)")
    }

    private var title: String {
        let action = transfer.direction == .send
            ? AppText.value("Send", "发送", language: language)
            : AppText.value("Receive", "接收", language: language)
        guard let deviceLabel else {
            return transfer.direction == .send
                ? AppText.value("Sent files", "发送文件", language: language)
                : AppText.value("Received files", "接收文件", language: language)
        }
        return "\(action) · \(deviceLabel)"
    }

    private var summary: String {
        guard transfer.totalBytes > 0 else {
            return AppText.value("File transfer", "文件传输", language: language)
        }
        return byteString(transfer.totalBytes)
    }

    private var icon: String {
        switch transfer.state {
        case .delivered:
            return "checkmark.circle.fill"
        case .rejected, .failed:
            return "exclamationmark.triangle.fill"
        case .canceled:
            return "xmark.circle.fill"
        case .queued:
            return "clock.fill"
        case .connecting:
            return "arrow.triangle.2.circlepath"
        default:
            return transfer.direction == .send
                ? "arrow.up.circle.fill"
                : "arrow.down.circle.fill"
        }
    }

    private var tint: Color {
        switch transfer.state {
        case .delivered:
            return Theme.success
        case .rejected, .failed:
            return Theme.danger
        case .canceled, .paused, .queued:
            return Theme.muted
        case .offered, .connecting, .transferring, .awaitingDeliveryProof:
            return Theme.accentStrong
        }
    }
}
#endif
#endif
