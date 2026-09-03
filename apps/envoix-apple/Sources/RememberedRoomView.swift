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
            roomText(.forgetQuestion),
            isPresented: $isForgetConfirmationPresented
        ) {
            Button(roomText(.cancel), role: .cancel) {}
            Button(roomText(.forgetRoom), role: .destructive) {
                onForget()
            }
        } message: {
            Text(RememberedRoomPresentationText.forgetDetail(
                hasQueuedFiles: !outboxEntries.isEmpty,
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
                roomText(.incomingFiles),
                systemImage: "arrow.down.doc.fill"
            )
            .font(.headline.weight(.semibold))
            .foregroundStyle(Theme.text)

            Text(offerSummary(offer))
                .font(.subheadline)
                .foregroundStyle(Theme.muted)

            HStack(spacing: 10) {
                Button(role: .cancel, action: onRejectOffer) {
                    Text(roomText(.decline))
                        .frame(maxWidth: .infinity)
                }
                .buttonStyle(.bordered)
                .disabled(isAcceptingOffer)

                Button(action: onAcceptOffer) {
                    if isAcceptingOffer {
                        HStack(spacing: 8) {
                            ProgressView()
                            Text(roomText(.preparingReceiver))
                        }
                        .frame(maxWidth: .infinity)
                    } else {
                        Text(roomText(.receive))
                            .frame(maxWidth: .infinity)
                    }
                }
                .buttonStyle(.borderedProminent)
                .tint(Theme.accentStrong)
                .disabled(isAcceptingOffer)
                .accessibilityLabel(
                    roomText(isAcceptingOffer ? .preparingReceiver : .receive)
                )
            }
        }
        .card(raised: true, padding: 16)
        .accessibilityIdentifier("remembered_room_incoming_offer")
    }

    @ViewBuilder
    private var outboxSection: some View {
        if !outboxEntries.isEmpty || outboxError != nil {
            VStack(alignment: .leading, spacing: 10) {
                Text(roomText(.filesForRoom))
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
                                Button(roomText(.retry)) {
                                    onRetryOutboxEntry(entry.id)
                                }
                                .buttonStyle(.borderedProminent)
                                .tint(Theme.accentStrong)

                                Button(
                                    roomText(.remove),
                                    role: .destructive
                                ) {
                                    onRemoveOutboxEntry(entry)
                                }
                                .buttonStyle(.bordered)
                            }
                        } else if entry.state == .queued {
                            Button(
                                roomText(.remove),
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
                Text(roomText(.noTransfers))
                    .font(.headline.weight(.semibold))
                    .foregroundStyle(Theme.text)
                Text(roomText(.noTransfersDetail))
                .font(.subheadline)
                .foregroundStyle(Theme.muted)
            }
            .card(padding: 16)
        } else {
            VStack(alignment: .leading, spacing: 10) {
                HStack {
                    Text(roomText(.roomActivity))
                        .font(.headline.weight(.semibold))
                    Spacer()
                    Button(roomText(.viewAll), action: onShowActivity)
                        .font(.subheadline.weight(.semibold))
                }
                ForEach(records.prefix(4), id: \.activityId) { record in
                    let progress = TransferPresentationPolicy.progress(for: record.state)
                    let metrics = metricsByActivityID[record.activityId] ?? ActivityMetrics()
                    VStack(alignment: .leading, spacing: 6) {
                        HStack {
                            Label(
                                record.direction == .send
                                    ? roomText(.sentFiles)
                                    : roomText(.receivedFiles),
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
                                    Text(roomText(.open))
                                        .font(.caption.weight(.semibold))
                                }
                                .buttonStyle(.bordered)
                                .accessibilityIdentifier(
                                    "remembered_room_open_received_\(record.activityId)"
                                )

                                ShareLink(items: urls) {
                                    Text(roomText(.share))
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
                    roomText(.addFiles),
                    systemImage: "plus"
                )
                .frame(maxWidth: .infinity, minHeight: 44)
            }
            .buttonStyle(PrimaryActionButtonStyle())
            .disabled(incomingOffer != nil)
            .accessibilityIdentifier("remembered_room_add_files")

            if status == .connected || isConnecting {
                Button(role: .destructive, action: onDisconnect) {
                    Text(roomText(.disconnect))
                    .frame(maxWidth: .infinity, minHeight: 40)
                }
                .buttonStyle(.bordered)
            } else {
                Label(
                    roomText(.reconnectsAutomatically),
                    systemImage: "info.circle"
                )
                .font(.caption)
                .foregroundStyle(statusTint)
                .frame(maxWidth: .infinity, alignment: .leading)
            }

            Button(role: .destructive) {
                isForgetConfirmationPresented = true
            } label: {
                Text(roomText(.forgetRoom))
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
        RememberedRoomPresentationText.connectionStatus(status, language: language)
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
        let count = RememberedRoomPresentationText.itemCount(
            offer.itemCount,
            language: language
        )
        return names.isEmpty ? count : "\(names) · \(count)"
    }

    private func activityStateText(_ state: TransferActivityState) -> String {
        RememberedRoomPresentationText.activityState(state, language: language)
    }

    private func outboxTitle(_ entry: RememberedRoomOutboxEntry) -> String {
        if !entry.rootNames.isEmpty {
            return entry.rootNames.joined(separator: ", ")
        }
        return roomText(.preparedFiles)
    }

    private func savedDestination(_ record: TransferActivityRecord) -> String {
        let urls = record.savedPaths.map { URL(fileURLWithPath: $0) }
        let parentPaths = Set(urls.map { $0.deletingLastPathComponent().path })
        if parentPaths.count == 1, let parent = urls.first?.deletingLastPathComponent() {
            return RememberedRoomPresentationText.savedIn(
                parent.lastPathComponent,
                language: language
            )
        }
        return RememberedRoomPresentationText.savedItems(
            urls.count,
            language: language
        )
    }

    private func outboxSummary(_ entry: RememberedRoomOutboxEntry) -> String {
        let itemText = RememberedRoomPresentationText.itemCount(
            entry.itemCount,
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
        RememberedRoomPresentationText.outboxState(state, language: language)
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

    private func roomText(_ copy: RememberedRoomCopy) -> String {
        RememberedRoomPresentationText.value(copy, language: language)
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
        let copy: AgentTransferStateCopy
        switch transfer.state {
        case .offered:
            copy = .awaitingApproval
        case .queued:
            copy = .queued
        case .connecting:
            copy = .connecting
        case .transferring:
            copy = transfer.direction == .send ? .sending : .receiving
        case .paused:
            copy = .paused
        case .awaitingDeliveryProof:
            copy = .verifyingDelivery
        case .delivered:
            copy = transfer.direction == .send ? .delivered : .received
        case .rejected:
            copy = .rejected
        case .failed:
            copy = .failed
        case .canceled:
            copy = .canceled
        }
        return AgentTransferPresentationText.state(copy, language: language)
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
            let copy: AgentTransferDetailCopy
            switch rejection {
            case .userDeclined:
                copy = .userDeclined
            case .busy:
                copy = .busy
            case .insufficientSpace:
                copy = .insufficientSpace
            case .unsupportedContent:
                copy = .unsupportedContent
            case .invalidOffer:
                copy = .invalidOffer
            }
            return AgentTransferPresentationText.detail(copy, language: language)
        }
        switch transfer.state {
        case .queued:
            return AgentTransferPresentationText.detail(.queued, language: language)
        case .awaitingDeliveryProof:
            return AgentTransferPresentationText.detail(
                .awaitingDeliveryProof,
                language: language
            )
        case .paused:
            return AgentTransferPresentationText.detail(.paused, language: language)
        default:
            return nil
        }
    }

    static func pathText(_ path: FfiAgentPathKind, language: String) -> String {
        let copy: AgentTransferPathCopy
        switch path {
        case .lan:
            copy = .lan
        case .direct:
            copy = .direct
        case .relay:
            copy = .relay
        case .wifiAware:
            copy = .wifiAware
        case .other:
            copy = .other
        }
        return AgentTransferPresentationText.path(copy, language: language)
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
                agentRoomText(.helperUnavailable),
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
                Text(agentRoomText(.roomActivity))
                    .font(.headline.weight(.semibold))
                Spacer()
                if !transfers.isEmpty {
                    Button(
                        agentRoomText(.viewAll),
                        action: onShowActivity
                    )
                    .font(.subheadline.weight(.semibold))
                }
            }

            if transfers.isEmpty {
                Text(agentRoomText(.agentRoomEmpty))
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
                        Text(agentRoomText(.preparingFiles))
                    }
                    .frame(maxWidth: .infinity, minHeight: 44)
                } else {
                    Label(
                        agentRoomText(.addFiles),
                        systemImage: "plus"
                    )
                    .frame(maxWidth: .infinity, minHeight: 44)
                }
            }
            .buttonStyle(PrimaryActionButtonStyle())
            .disabled(isPreparing)
            .accessibilityIdentifier("agent_room_add_files")

            Label(
                agentRoomText(.helperKeepsRoom),
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
            return agentConnectionText(.preparing)
        }
        if hasActivePath {
            return agentConnectionText(.transferring)
        }
        if hasPendingTransfer {
            return agentConnectionText(.waiting)
        }
        return agentConnectionText(.ready)
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

    private func agentRoomText(_ copy: RememberedRoomCopy) -> String {
        RememberedRoomPresentationText.value(copy, language: language)
    }

    private func agentConnectionText(_ copy: AgentRoomConnectionCopy) -> String {
        RememberedRoomPresentationText.agentRoomConnection(copy, language: language)
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
                        activityText(.helperRefreshUnavailable),
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
            Text(RememberedRoomPresentationText.agentActivityTitle(
                hasLoadedSnapshot: hasLoadedSnapshot,
                language: language
            ))
            .font(.headline)
            .foregroundStyle(Theme.text)
            if hasLoadedSnapshot {
                Text(activityText(.helperActivityDetail))
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
            ?? activityText(.pairedDevice)
    }

    private func path(for transferID: String) -> FfiAgentPathKind? {
        activePaths.first { $0.transferId == transferID }?.path
    }

    private func activityText(_ copy: RememberedRoomCopy) -> String {
        RememberedRoomPresentationText.value(copy, language: language)
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
        RememberedRoomPresentationText.agentTransferTitle(
            direction: transfer.direction,
            deviceLabel: deviceLabel,
            language: language
        )
    }

    private var summary: String {
        guard transfer.totalBytes > 0 else {
            return RememberedRoomPresentationText.value(.fileTransfer, language: language)
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
