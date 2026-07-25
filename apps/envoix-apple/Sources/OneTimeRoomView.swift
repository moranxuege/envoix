#if os(iOS)
import SwiftUI

struct OneTimeRoomView: View {
    @Environment(\.appLanguage) private var language

    let room: OneTimeRoomSession
    let records: [TransferActivityRecord]
    let selectedPeerIsVisible: Bool
    let discoveryIsActive: Bool
    let onAddFiles: () -> Void
    let onReceiveFiles: () -> Void
    let onShowActivity: () -> Void
    let onClose: () -> Void

    var body: some View {
        ScrollView {
            LazyVStack(alignment: .leading, spacing: 16) {
                roomHeader
                composer
                timeline
                roomNotice
                closeButton
            }
            .padding(.horizontal, 16)
            .padding(.vertical, 16)
        }
        .background(Theme.bg)
        .accessibilityIdentifier("one_time_room")
    }

    private var roomHeader: some View {
        VStack(alignment: .leading, spacing: 12) {
            HStack(alignment: .top, spacing: 12) {
                Image(systemName: "person.crop.circle.badge.questionmark")
                    .font(.title2.weight(.semibold))
                    .foregroundStyle(Theme.accentStrong)
                    .frame(width: 48, height: 48)
                    .background(Theme.accentSoft, in: RoundedRectangle(cornerRadius: 14))

                VStack(alignment: .leading, spacing: 4) {
                    Text(roomTitle)
                        .font(.title2.bold())
                        .foregroundStyle(Theme.text)
                        .lineLimit(1)
                    Label(
                        AppText.value("Unverified", "未经验证", language: language),
                        systemImage: "exclamationmark.shield"
                    )
                    .font(.subheadline.weight(.semibold))
                    .foregroundStyle(Theme.warning)
                    .accessibilityIdentifier("room_context_unverified")
                }
                Spacer(minLength: 8)
            }

            HStack(spacing: 8) {
                Circle()
                    .fill(statusColor)
                    .frame(width: 8, height: 8)
                Text(roomStatus)
                    .font(.subheadline)
                    .foregroundStyle(Theme.muted)
            }
            .accessibilityIdentifier("room_availability")
        }
        .card(raised: true, padding: 16)
    }

    private var composer: some View {
        VStack(alignment: .leading, spacing: 12) {
            Text(AppText.value("Transfer in this room", "在此房间中传输", language: language))
                .font(.headline)
                .foregroundStyle(Theme.text)

            Button(action: onAddFiles) {
                Label(
                    AppText.value("Add files", "添加文件", language: language),
                    systemImage: "plus"
                )
                .frame(maxWidth: .infinity, minHeight: 44)
            }
            .buttonStyle(PrimaryActionButtonStyle())
            .accessibilityIdentifier("room_add_files")

            Button(action: onReceiveFiles) {
                Label(
                    AppText.value("Show a room QR", "显示房间二维码", language: language),
                    systemImage: "qrcode"
                )
                .frame(maxWidth: .infinity, minHeight: 42)
            }
            .buttonStyle(.bordered)
            .tint(Theme.accentStrong)
            .accessibilityIdentifier("room_receive_files")

            Text(AppText.value(
                "Either device can offer files. An incoming offer always waits for confirmation.",
                "任一设备都可以发送文件；收到的邀请始终需要确认。",
                language: language
            ))
            .font(.footnote)
            .foregroundStyle(Theme.muted)
            .fixedSize(horizontal: false, vertical: true)
        }
        .card(padding: 16)
    }

    private var timeline: some View {
        VStack(alignment: .leading, spacing: 12) {
            HStack {
                Text(AppText.value("Room activity", "房间活动", language: language))
                    .font(.headline)
                    .foregroundStyle(Theme.text)
                Spacer()
                Button(action: onShowActivity) {
                    Text(AppText.value("All Activity", "全部活动", language: language))
                        .font(.subheadline.weight(.semibold))
                }
                .buttonStyle(.plain)
                .foregroundStyle(Theme.accentStrong)
                .accessibilityIdentifier("room_open_activity")
            }

            if records.isEmpty {
                VStack(spacing: 8) {
                    Image(systemName: "arrow.up.arrow.down.circle")
                        .font(.system(size: 30, weight: .medium))
                        .foregroundStyle(Theme.muted)
                    Text(AppText.value(
                        "Transfers started here will appear in this timeline.",
                        "从这里开始的传输会显示在此时间线中。",
                        language: language
                    ))
                    .font(.subheadline)
                    .foregroundStyle(Theme.muted)
                    .multilineTextAlignment(.center)
                }
                .frame(maxWidth: .infinity)
                .padding(.vertical, 22)
                .accessibilityIdentifier("room_activity_empty")
            } else {
                ForEach(records.prefix(6)) { record in
                    compactActivityCard(record)
                }
            }
        }
        .card(padding: 16)
        .accessibilityIdentifier("room_activity")
    }

    private func compactActivityCard(_ record: TransferActivityRecord) -> some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack(spacing: 10) {
                Image(systemName: record.direction == .send ? "arrow.up.circle.fill" : "arrow.down.circle.fill")
                    .font(.title3)
                    .foregroundStyle(activityTint(record.state))

                VStack(alignment: .leading, spacing: 2) {
                    Text(activityTitle(record))
                        .font(.subheadline.weight(.semibold))
                        .foregroundStyle(Theme.text)
                    Text(activityState(record))
                        .font(.caption)
                        .foregroundStyle(Theme.muted)
                }
                Spacer(minLength: 8)
                Text(record.direction == .send
                     ? AppText.value("Sent", "发送", language: language)
                     : AppText.value("Received", "接收", language: language))
                    .font(.caption.weight(.semibold))
                    .foregroundStyle(Theme.muted)
            }

            if let path = record.connectionPath {
                Text(ConnectionPathPresentationPolicy.label(for: path, language: language))
                    .font(.caption)
                    .foregroundStyle(Theme.muted)
            }

            if record.totalBytes > 0,
               TransferPresentationPolicy.progress(for: record.state) != .hidden {
                ProgressView(
                    value: Double(record.state == .delivered ? record.totalBytes : record.bytesTransferred),
                    total: Double(record.totalBytes)
                )
            }
        }
        .padding(.vertical, 5)
        .accessibilityIdentifier("room_activity_\(record.activityId)")
    }

    private var roomNotice: some View {
        Label(
            AppText.value(
                "This is a one-time room. Each transfer connects and authenticates separately.",
                "这是一次性房间。每次传输都会单独连接并进行认证。",
                language: language
            ),
            systemImage: "info.circle"
        )
        .font(.footnote)
        .foregroundStyle(Theme.muted)
        .fixedSize(horizontal: false, vertical: true)
        .padding(.horizontal, 4)
        .accessibilityIdentifier("room_one_time_notice")
    }

    private var closeButton: some View {
        Button(role: .destructive, action: onClose) {
            Label(
                AppText.value("Close room", "关闭房间", language: language),
                systemImage: "xmark.circle"
            )
            .frame(maxWidth: .infinity, minHeight: 42)
        }
        .buttonStyle(.bordered)
        .accessibilityIdentifier("close_one_time_room")
    }

    private var roomTitle: String {
        if let displayName = room.nearbySelection?.displayName, !displayName.trimmed.isEmpty {
            return displayName
        }
        return AppText.value("One-time Room", "一次性房间", language: language)
    }

    private var roomStatus: String {
        switch room.origin {
        case .nearby:
            if selectedPeerIsVisible {
                return AppText.value("Nearby now", "当前在附近", language: language)
            }
            return discoveryIsActive
                ? AppText.value("Looking for this device", "正在查找此设备", language: language)
                : AppText.value("Nearby discovery paused", "附近发现已暂停", language: language)
        case .pairingCode:
            return AppText.value("Transfer code loaded", "已载入传输配对码", language: language)
        case .showCode:
            return AppText.value("Ready to show a room QR", "可显示房间二维码", language: language)
        case .externalShare:
            return AppText.value("Files ready to offer", "文件已准备发送", language: language)
        }
    }

    private var statusColor: Color {
        switch room.origin {
        case .nearby:
            return selectedPeerIsVisible ? Theme.success : Theme.warning
        case .pairingCode, .showCode, .externalShare:
            return Theme.accent
        }
    }

    private func activityTitle(_ record: TransferActivityRecord) -> String {
        if record.itemCount == 0 {
            return record.direction == .send
                ? AppText.value("Outgoing transfer", "待发送内容", language: language)
                : AppText.value("Incoming transfer", "待接收内容", language: language)
        }
        return AppText.value(
            record.itemCount == 1 ? "1 item" : "\(record.itemCount) items",
            "\(record.itemCount) 个项目",
            language: language
        )
    }

    private func activityState(_ record: TransferActivityRecord) -> String {
        switch record.state {
        case .preparing: return AppText.value("Preparing", "正在准备", language: language)
        case .waitingForPeer: return AppText.value("Waiting for the other device", "等待另一台设备", language: language)
        case .pairing, .connecting: return AppText.value("Connecting", "正在连接", language: language)
        case .awaitingDecision: return AppText.value("Waiting for confirmation", "等待确认", language: language)
        case .transferring:
            return record.direction == .send
                ? AppText.value("Sending", "正在发送", language: language)
                : AppText.value("Receiving", "正在接收", language: language)
        case .verifying: return AppText.value("Verifying", "正在校验", language: language)
        case .saving, .waitingForReceiverSave, .finalizingDelivery:
            return AppText.value("Finishing", "正在完成", language: language)
        case .paused: return AppText.value("Paused", "已暂停", language: language)
        case .delivered: return AppText.value("Delivered", "已送达", language: language)
        case .failed: return AppText.value("Needs attention", "需要处理", language: language)
        case .canceled: return AppText.value("Canceled", "已取消", language: language)
        }
    }

    private func activityTint(_ state: TransferActivityState) -> Color {
        switch state {
        case .delivered: return Theme.success
        case .failed: return Theme.danger
        case .paused, .awaitingDecision: return Theme.warning
        case .canceled: return Theme.muted
        default: return Theme.accentStrong
        }
    }

}
#endif
