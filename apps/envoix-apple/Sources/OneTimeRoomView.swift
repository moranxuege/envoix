#if os(iOS)
import SwiftUI

struct OneTimeRoomView: View {
    @Environment(\.appLanguage) private var language
    @AppStorage("envoix.outputDirDisplayName") private var outputDirDisplayName = ""

    let room: OneTimeRoomSession
    let records: [TransferActivityRecord]
    let controlPhase: RoomControlPhase
    let peerDisplayName: String?
    let incomingOffer: RoomControlTransferOffer?
    let isRoomCreator: Bool
    let lifetimePolicy: RoomControlLifetimePolicy
    let idleDeadline: Date?
    let now: Date
    let selectedPeerIsVisible: Bool
    let discoveryIsActive: Bool
    let onAddFiles: () -> Void
    let onAcceptOffer: () -> Void
    let onRejectOffer: () -> Void
    let onSetKeepOpen: (Bool) -> Void
    let onShowActivity: () -> Void
    let onClose: () -> Void

    var body: some View {
        ScrollView {
            LazyVStack(alignment: .leading, spacing: 16) {
                roomHeader
                if let incomingOffer {
                    incomingOfferCard(incomingOffer)
                }
                composer
                timeline
                if let endedMessage {
                    endedNotice(endedMessage)
                }
            }
            .padding(.horizontal, 16)
            .padding(.vertical, 16)
        }
        .safeAreaInset(edge: .bottom) {
            roomControls
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
                    Label(trustLabel, systemImage: "exclamationmark.shield")
                    .font(.subheadline.weight(.semibold))
                    .foregroundStyle(Theme.warning)
                    .accessibilityIdentifier(
                        roomWasAuthenticated
                            ? "room_context_authenticated"
                            : "room_context_unverified"
                    )
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
            .disabled(!canOfferFiles)
            .accessibilityIdentifier("room_add_files")

            Text(composerMessage)
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

    private func incomingOfferCard(_ offer: RoomControlTransferOffer) -> some View {
        VStack(alignment: .leading, spacing: 12) {
            Label(
                AppText.value("Incoming file offer", "收到文件邀请", language: language),
                systemImage: "tray.and.arrow.down.fill"
            )
            .font(.headline)
            .foregroundStyle(Theme.text)

            Text(offerSummary(offer))
                .font(.subheadline)
                .foregroundStyle(Theme.muted)

            Label(
                AppText.value(
                    "Save to \(incomingDestinationName)",
                    "保存到 \(incomingDestinationName)",
                    language: language
                ),
                systemImage: "folder"
            )
            .font(.subheadline)
            .foregroundStyle(Theme.muted)

            ForEach(Array(offer.rootNames.prefix(3).enumerated()), id: \.offset) { _, name in
                Label(name, systemImage: "doc")
                    .font(.subheadline)
                    .foregroundStyle(Theme.text)
                    .lineLimit(1)
            }

            HStack(spacing: 10) {
                Button(role: .cancel, action: onRejectOffer) {
                    Text(AppText.value("Reject", "拒绝", language: language))
                        .frame(maxWidth: .infinity, minHeight: 42)
                }
                .buttonStyle(.bordered)
                .accessibilityIdentifier("room_offer_reject")

                Button(action: onAcceptOffer) {
                    Text(AppText.value("Accept", "接受", language: language))
                        .frame(maxWidth: .infinity, minHeight: 42)
                }
                .buttonStyle(PrimaryActionButtonStyle())
                .accessibilityIdentifier("room_offer_accept")
            }
        }
        .card(raised: true, padding: 16)
        .accessibilityIdentifier("room_incoming_offer")
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

    private func endedNotice(_ message: String) -> some View {
        Label(message, systemImage: "exclamationmark.circle.fill")
            .font(.subheadline.weight(.semibold))
            .foregroundStyle(Theme.danger)
            .fixedSize(horizontal: false, vertical: true)
            .card(padding: 14)
            .accessibilityIdentifier("room_ended_notice")
    }

    private var roomControls: some View {
        VStack(spacing: 10) {
            HStack {
                Label(roomLifetimeText, systemImage: lifetimePolicy == .untilForegroundEnds ? "infinity" : "timer")
                    .font(.subheadline.weight(.semibold))
                    .foregroundStyle(Theme.text)
                Spacer()
                if isRoomCreator && controlPhase == .connected {
                    Toggle(
                        AppText.value("Keep open", "保持开启", language: language),
                        isOn: Binding(
                            get: { lifetimePolicy == .untilForegroundEnds },
                            set: onSetKeepOpen
                        )
                    )
                    .labelsHidden()
                    .accessibilityLabel(AppText.value("Keep room open", "保持房间开启", language: language))
                    .accessibilityIdentifier("room_keep_open")
                }
            }

            Button(role: .destructive, action: onClose) {
                Label(
                    AppText.value("End room", "结束房间", language: language),
                    systemImage: "xmark.circle"
                )
                .frame(maxWidth: .infinity, minHeight: 42)
            }
            .buttonStyle(.bordered)
            .accessibilityIdentifier("close_one_time_room")
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

    private var roomTitle: String {
        if let peerDisplayName, !peerDisplayName.trimmed.isEmpty {
            return peerDisplayName
        }
        if let displayName = room.nearbySelection?.displayName, !displayName.trimmed.isEmpty {
            return displayName
        }
        return AppText.value("One-time Room", "一次性房间", language: language)
    }

    private var roomStatus: String {
        switch controlPhase {
        case .hosting:
            return AppText.value("Waiting for another device", "正在等待另一台设备", language: language)
        case .joining:
            return AppText.value("Joining room", "正在加入房间", language: language)
        case .connected:
            return AppText.value("Connected for this room", "已连接此房间", language: language)
        case .ended:
            return AppText.value("Room ended", "房间已结束", language: language)
        case .failed:
            return AppText.value("Connection needs attention", "连接需要处理", language: language)
        case .idle:
            break
        }
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
        case .roomControl:
            return AppText.value("Connecting", "正在连接", language: language)
        }
    }

    private var statusColor: Color {
        switch controlPhase {
        case .connected: return Theme.success
        case .hosting, .joining: return Theme.warning
        case .ended, .failed: return Theme.danger
        case .idle: break
        }
        switch room.origin {
        case .nearby:
            return selectedPeerIsVisible ? Theme.success : Theme.warning
        case .pairingCode, .showCode, .externalShare, .roomControl:
            return Theme.accent
        }
    }

    private var trustLabel: String {
        roomWasAuthenticated
            ? AppText.value("Authenticated for this room", "已为此房间认证", language: language)
            : AppText.value("Unverified", "未经验证", language: language)
    }

    private var roomWasAuthenticated: Bool {
        room.origin == .roomControl && peerDisplayName != nil
    }

    private var canOfferFiles: Bool {
        if room.origin == .roomControl {
            return controlPhase == .connected && incomingOffer == nil
        }
        return true
    }

    private var composerMessage: String {
        if controlPhase == .connected {
            return AppText.value(
                "Either device can offer files. Every incoming offer requires confirmation.",
                "任一设备都可以发送文件；每个收到的邀请都需要确认。",
                language: language
            )
        }
        if room.origin == .roomControl {
            return AppText.value(
                "File offers become available after the room connection is authenticated.",
                "房间连接完成认证后即可发送文件。",
                language: language
            )
        }
        return AppText.value(
            "This legacy transfer uses a one-time connection.",
            "此旧版传输使用一次性连接。",
            language: language
        )
    }

    private var roomLifetimeText: String {
        guard room.origin == .roomControl else {
            return AppText.value("One-time transfer", "一次性传输", language: language)
        }
        if lifetimePolicy == .untilForegroundEnds {
            return AppText.value("Kept open while Envoix is open", "Envoix 打开时保持房间", language: language)
        }
        guard controlPhase == .connected, let idleDeadline else {
            return AppText.value("15-minute idle limit", "空闲 15 分钟后结束", language: language)
        }
        let seconds = max(0, Int(idleDeadline.timeIntervalSince(now)))
        return AppText.value(
            "Ends after \(seconds / 60):\(String(format: "%02d", seconds % 60)) idle",
            "空闲 \(seconds / 60):\(String(format: "%02d", seconds % 60)) 后结束",
            language: language
        )
    }

    private var endedMessage: String? {
        switch controlPhase {
        case .ended(let reason):
            switch reason {
            case .userEnded:
                return AppText.value("This room was ended.", "此房间已结束。", language: language)
            case .idleExpired:
                return AppText.value("This room ended after 15 minutes without transfer activity.", "此房间在 15 分钟无传输活动后结束。", language: language)
            case .invitationExpired:
                return AppText.value("The room invitation expired.", "房间邀请已过期。", language: language)
            case .peerEnded:
                return AppText.value("The other device ended this room.", "另一台设备结束了此房间。", language: language)
            case .backgrounded:
                return AppText.value("The room ended when Envoix left the foreground.", "Envoix 离开前台后房间已结束。", language: language)
            case .networkLost:
                return AppText.value("The room connection was lost.", "房间连接已断开。", language: language)
            case .protocolFailure:
                return AppText.value("The room ended because of a connection error.", "房间因连接错误而结束。", language: language)
            }
        case .failed(let message):
            return message
        default:
            return nil
        }
    }

    private func offerSummary(_ offer: RoomControlTransferOffer) -> String {
        let itemText = AppText.value(
            offer.itemCount == 1 ? "1 item" : "\(offer.itemCount) items",
            "\(offer.itemCount) 个项目",
            language: language
        )
        return "\(itemText) · \(byteString(offer.totalBytes))"
    }

    private var incomingDestinationName: String {
        outputDirDisplayName.trimmed.isEmpty
            ? AppText.value("Envoix / Downloads", "Envoix / Downloads", language: language)
            : outputDirDisplayName
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
        case .delivered:
            return record.direction == .send
                ? AppText.value("Delivered", "已送达", language: language)
                : AppText.value("Received", "已接收", language: language)
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
