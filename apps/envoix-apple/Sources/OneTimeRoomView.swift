#if os(iOS) || os(macOS)
import QuickLook
import SwiftUI

struct OneTimeRoomView: View {
    @Environment(\.appLanguage) private var language
    @AppStorage("envoix.outputDirDisplayName") private var outputDirDisplayName = ""
    @State private var previewFileURL: URL?
    @State private var receivedItemsPresentation: ReceivedItemsPresentation?

    let room: OneTimeRoomSession
    let records: [TransferActivityRecord]
    let metricsByActivityID: [String: ActivityMetrics]
    let controlPhase: RoomControlPhase
    let peerDisplayName: String?
    let incomingOffer: RoomControlTransferOffer?
    let isAcceptingOffer: Bool
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
                timeline
                if let endedMessage {
                    endedNotice(endedMessage)
                }
            }
            .padding(.horizontal, 16)
            .padding(.vertical, 16)
        }
        .accessibilityIdentifier("one_time_room")
        .safeAreaInset(edge: .bottom) {
            roomControls
        }
        .background(Theme.bg)
        .quickLookPreview($previewFileURL)
        .sheet(item: $receivedItemsPresentation) { presentation in
            ReceivedItemsSheet(urls: presentation.urls)
        }
    }

    private var roomHeader: some View {
        VStack(alignment: .leading, spacing: 12) {
            HStack(alignment: .top, spacing: 12) {
                Image(systemName: roomWasAuthenticated
                    ? "person.crop.circle.badge.checkmark"
                    : "person.crop.circle.badge.questionmark")
                    .font(.title2.weight(.semibold))
                    .foregroundStyle(roomWasAuthenticated ? Theme.success : Theme.warning)
                    .frame(width: 48, height: 48)
                    .background(
                        (roomWasAuthenticated ? Theme.success : Theme.warning).opacity(0.12),
                        in: RoundedRectangle(cornerRadius: 14)
                    )

                VStack(alignment: .leading, spacing: 4) {
                    Text(roomTitle)
                        .font(.title2.bold())
                        .foregroundStyle(Theme.text)
                        .lineLimit(1)
                    Label(
                        trustLabel,
                        systemImage: roomWasAuthenticated
                            ? "checkmark.shield.fill"
                            : "exclamationmark.shield"
                    )
                        .font(.subheadline.weight(.semibold))
                        .foregroundStyle(roomWasAuthenticated ? Theme.success : Theme.warning)
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

    private var timeline: some View {
        VStack(alignment: .leading, spacing: 12) {
            HStack {
                Text(AppText.localized("room.activity.title", language: language))
                    .font(.headline)
                    .foregroundStyle(Theme.text)
                Spacer()
                Button(action: onShowActivity) {
                    Text(AppText.localized("room.activity.all", language: language))
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
                    Text(AppText.localized("room.activity.empty", language: language))
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
                AppText.localized("room.offer.incoming", language: language),
                systemImage: "tray.and.arrow.down.fill"
            )
            .font(.headline)
            .foregroundStyle(Theme.text)

            Text(AppText.localized("room.offer.summary", language: language))
                .font(.caption.weight(.semibold))
                .foregroundStyle(Theme.muted)
                .textCase(.uppercase)

            Text(offerSummary(offer))
                .font(.subheadline)
                .foregroundStyle(Theme.text)

            Text(AppText.localized("room.offer.destination", language: language))
                .font(.caption.weight(.semibold))
                .foregroundStyle(Theme.muted)
                .textCase(.uppercase)

            Label(incomingDestinationName, systemImage: "folder")
                .font(.subheadline)
                .foregroundStyle(Theme.text)

            if !offer.rootNames.isEmpty {
                Text(AppText.localized("room.offer.contents", language: language))
                    .font(.caption.weight(.semibold))
                    .foregroundStyle(Theme.muted)
                    .textCase(.uppercase)

                ForEach(Array(offer.rootNames.prefix(3).enumerated()), id: \.offset) { _, name in
                    Label(name, systemImage: "doc.on.doc")
                        .font(.subheadline)
                        .foregroundStyle(Theme.text)
                        .lineLimit(1)
                }
            }

            if additionalItemCount(offer) > 0 {
                Text(AppText.value(
                    "+\(additionalItemCount(offer)) more",
                    "另有 \(additionalItemCount(offer)) 项",
                    language: language
                ))
                .font(.caption.weight(.semibold))
                .foregroundStyle(Theme.muted)
            }

            HStack(spacing: 10) {
                Button(role: .cancel, action: onRejectOffer) {
                    Text(AppText.localized("common.decline", language: language))
                        .frame(maxWidth: .infinity, minHeight: 42)
                }
                .buttonStyle(.bordered)
                .disabled(isAcceptingOffer)
                .accessibilityIdentifier("room_offer_reject")

                Button(action: onAcceptOffer) {
                    if isAcceptingOffer {
                        HStack(spacing: 8) {
                            ProgressView()
                            Text(AppText.localized("room.offer.preparing_receiver", language: language))
                        }
                        .frame(maxWidth: .infinity, minHeight: 42)
                    } else {
                        Text(AppText.localized("common.receive", language: language))
                            .frame(maxWidth: .infinity, minHeight: 42)
                    }
                }
                .buttonStyle(PrimaryActionButtonStyle())
                .disabled(isAcceptingOffer)
                .accessibilityLabel(
                    isAcceptingOffer
                        ? AppText.localized("room.offer.preparing_receiver", language: language)
                        : AppText.localized("common.receive", language: language)
                )
                .accessibilityIdentifier("room_offer_accept")
            }
        }
        .card(raised: true, padding: 16)
        .accessibilityIdentifier("room_incoming_offer")
    }

    private func compactActivityCard(_ record: TransferActivityRecord) -> some View {
        let progress = TransferPresentationPolicy.progress(for: record.state)
        let metrics = metricsByActivityID[record.activityId] ?? ActivityMetrics()
        return VStack(alignment: .leading, spacing: 8) {
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
                Text(TransferActivityText.direction(record.direction, language: language))
                    .font(.caption.weight(.semibold))
                    .foregroundStyle(Theme.muted)
            }

            if record.totalBytes > 0,
               record.state != .delivered,
               progress != .hidden {
                ProgressView(
                    value: Double(record.bytesTransferred),
                    total: Double(record.totalBytes)
                )
                Text("\(byteString(record.bytesTransferred)) / \(byteString(record.totalBytes))")
                    .font(.caption.monospacedDigit())
                    .foregroundStyle(Theme.muted)
            }

            TransferPerformanceLine(
                currentBytesPerSecond: progress == .active ? metrics.speedBps : 0,
                averageBytesPerSecond: metrics.averageSpeedBps,
                etaSeconds: progress == .active ? metrics.etaSeconds : nil,
                currentSampleDate: metrics.currentRateUpdatedAt,
                accessibilityPrefix: "room_activity_\(record.activityId)"
            )

            if let path = record.connectionPath {
                Label(
                    ConnectionPathPresentationPolicy.label(for: path, language: language),
                    systemImage: path == .wifiAware ? "wifi" : "link"
                )
                .font(.caption.weight(.semibold))
                .foregroundStyle(Theme.muted)
                .accessibilityIdentifier("room_activity_path_\(record.activityId)")
            }

            if record.direction == .receive,
               record.state == .delivered,
               !record.savedPaths.isEmpty {
                let urls = record.savedPaths.map { URL(fileURLWithPath: $0) }
                HStack(spacing: 8) {
                    Label(roomSavedDestination(record), systemImage: "folder.fill")
                        .font(.caption.weight(.semibold))
                        .foregroundStyle(Theme.text)
                        .lineLimit(1)
                        .truncationMode(.middle)

                    Spacer(minLength: 8)

                    Button {
                        openReceivedItems(urls)
                    } label: {
                        Text(AppText.localized("common.open", language: language))
                            .font(.caption.weight(.semibold))
                    }
                    .buttonStyle(.bordered)
                    .accessibilityIdentifier("room_open_received_\(record.activityId)")

                    ShareLink(items: urls) {
                        Text(AppText.localized("common.share", language: language))
                            .font(.caption.weight(.semibold))
                    }
                    .buttonStyle(.bordered)
                    .accessibilityIdentifier("room_share_received_\(record.activityId)")
                }
            }
        }
        .padding(.vertical, 5)
        .accessibilityIdentifier("room_activity_\(record.activityId)")
    }

    private func openReceivedItems(_ urls: [URL]) {
        guard let first = urls.first else { return }
        if urls.count == 1, isRegularFileURL(first) {
            previewFileURL = first
        } else {
            receivedItemsPresentation = ReceivedItemsPresentation(urls: urls)
        }
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
            Button(action: onAddFiles) {
                Label(
                    AppText.localized("room.action.add_files", language: language),
                    systemImage: "plus"
                )
                .frame(maxWidth: .infinity, minHeight: 44)
            }
            .buttonStyle(PrimaryActionButtonStyle())
            .disabled(!canOfferFiles)
            .accessibilityIdentifier("room_add_files")

            HStack {
                Label(roomLifetimeText, systemImage: lifetimePolicy == .untilForegroundEnds ? "infinity" : "timer")
                    .font(.subheadline.weight(.semibold))
                    .foregroundStyle(Theme.text)
                    .accessibilityIdentifier("room_lifetime_status")
                Spacer()
                if isRoomCreator && controlPhase == .connected {
                    Toggle(
                        AppText.localized("room.action.keep_open", language: language),
                        isOn: Binding(
                            get: { lifetimePolicy == .untilForegroundEnds },
                            set: onSetKeepOpen
                        )
                    )
                    .labelsHidden()
                    .accessibilityLabel(
                        AppText.localized("room.action.keep_open_accessibility", language: language)
                    )
                    .accessibilityIdentifier("room_keep_open")
                }
            }

            if roomIsTerminal {
                Button(action: onClose) {
                    Label(
                        AppText.localized("common.done", language: language),
                        systemImage: "checkmark.circle"
                    )
                    .frame(maxWidth: .infinity, minHeight: 42)
                }
                .buttonStyle(.bordered)
                .accessibilityIdentifier("close_one_time_room")
            } else {
                Button(role: .destructive, action: onClose) {
                    Label(
                        AppText.localized("room.action.end", language: language),
                        systemImage: "xmark.circle"
                    )
                    .frame(maxWidth: .infinity, minHeight: 42)
                }
                .buttonStyle(.bordered)
                .accessibilityIdentifier("close_one_time_room")
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

    private var roomTitle: String {
        if let peerDisplayName, !peerDisplayName.trimmed.isEmpty {
            return peerDisplayName
        }
        if let displayName = room.nearbySelection?.displayName, !displayName.trimmed.isEmpty {
            return displayName
        }
        return AppText.localized("activity.group.one_time_room", language: language)
    }

    private var roomStatus: String {
        RoomPresentationText.status(
            phase: controlPhase,
            origin: room.origin,
            selectedPeerIsVisible: selectedPeerIsVisible,
            discoveryIsActive: discoveryIsActive,
            language: language
        )
    }

    private var statusColor: Color {
        switch controlPhase {
        case .connected: return Theme.success
        case .hosting, .joining, .connectingRemembered, .waitingRemembered:
            return Theme.warning
        case .ended, .failed: return Theme.danger
        case .idle: break
        }
        switch room.origin {
        case .nearby:
            return selectedPeerIsVisible ? Theme.success : Theme.warning
        case .pairingCode, .showCode, .roomControl:
            return Theme.accent
        }
    }

    private var trustLabel: String {
        RoomPresentationText.trust(authenticated: roomWasAuthenticated, language: language)
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

    private var roomIsTerminal: Bool {
        switch controlPhase {
        case .ended, .failed:
            return true
        case .idle, .hosting, .joining, .connectingRemembered, .waitingRemembered, .connected:
            return false
        }
    }

    private var roomLifetimeText: String {
        guard room.origin == .roomControl else {
            return AppText.value("One-time transfer", "一次性传输", language: language)
        }
        switch controlPhase {
        case .ended, .failed:
            return AppText.value("Room closed", "房间已关闭", language: language)
        case .idle, .hosting, .joining, .connectingRemembered, .waitingRemembered, .connected:
            break
        }
        if lifetimePolicy == .untilForegroundEnds {
            return AppText.value("Kept open while Envoix is open", "Envoix 打开时保持房间", language: language)
        }
        guard let idleDeadline else {
            return AppText.value(
                "Idle timer paused during transfer",
                "传输期间空闲计时暂停",
                language: language
            )
        }
        let seconds = max(0, Int(ceil(idleDeadline.timeIntervalSince(now))))
        return AppText.value(
            "Ends in \(seconds / 60):\(String(format: "%02d", seconds % 60)) if idle",
            "空闲时将在 \(seconds / 60):\(String(format: "%02d", seconds % 60)) 后结束",
            language: language
        )
    }

    private func roomSavedDestination(_ record: TransferActivityRecord) -> String {
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

    private var endedMessage: String? {
        RoomPresentationText.endedMessage(phase: controlPhase, language: language)
    }

    private func offerSummary(_ offer: RoomControlTransferOffer) -> String {
        let fileCount = offer.itemCount >= offer.directoryCount
            ? offer.itemCount - offer.directoryCount
            : 0
        let fileText = AppText.value(
            fileCount == 1 ? "1 file" : "\(fileCount) files",
            "\(fileCount) 个文件",
            language: language
        )
        let folderText = AppText.value(
            offer.directoryCount == 1 ? "1 folder" : "\(offer.directoryCount) folders",
            "\(offer.directoryCount) 个文件夹",
            language: language
        )
        return "\(fileText) · \(folderText) · \(byteString(offer.totalBytes))"
    }

    private func additionalItemCount(_ offer: RoomControlTransferOffer) -> UInt32 {
        let shownCount = UInt32(offer.rootNames.count)
        return offer.itemCount > shownCount
            ? offer.itemCount - shownCount
            : 0
    }

    private var incomingDestinationName: String {
        outputDirDisplayName.trimmed.isEmpty
            ? AppText.value("Envoix / Downloads", "Envoix / Downloads", language: language)
            : outputDirDisplayName
    }

    private func activityTitle(_ record: TransferActivityRecord) -> String {
        if record.itemCount == 0 {
            return record.direction == .send
                ? AppText.localized("activity.outgoing", language: language)
                : AppText.localized("activity.incoming", language: language)
        }
        return TransferActivityText.itemCount(UInt64(record.itemCount), language: language)
    }

    private func activityState(_ record: TransferActivityRecord) -> String {
        TransferActivityText.state(
            record.state,
            direction: record.direction,
            language: language
        )
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
