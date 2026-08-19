import SwiftUI
import EnvoixCore
#if os(iOS)
import QuickLook
#endif

struct ActivityRoomGroup: Identifiable {
    let id: String
    let activityGroupID: String?
    let activityGroupLabel: String?
    let records: [TransferActivityRecord]
    let summaryRecord: TransferActivityRecord

    init?(
        id: String,
        activityGroupID: String?,
        records: [TransferActivityRecord]
    ) {
        guard !records.isEmpty else { return nil }
        let sortedRecords = records.sorted(by: Self.recordsNewestFirst)
        self.id = id
        self.activityGroupID = activityGroupID
        self.records = sortedRecords
        activityGroupLabel = sortedRecords.lazy.compactMap {
            $0.activityGroupLabel?.trimmed
        }.first { !$0.isEmpty }
        let pendingRecords = sortedRecords.filter {
            ActivityProjectionPolicy.isPending($0.state)
        }
        if pendingRecords.isEmpty {
            summaryRecord = sortedRecords[0]
        } else {
            let highestPriority = pendingRecords.map {
                Self.priority(of: $0.state)
            }.max() ?? 0
            summaryRecord = pendingRecords.first {
                Self.priority(of: $0.state) == highestPriority
            } ?? pendingRecords[0]
        }
    }

    var itemCount: UInt64 {
        saturatingSum(records.map { UInt64($0.itemCount) })
    }

    var totalBytes: UInt64 {
        saturatingSum(records.map(\.totalBytes))
    }

    var progressRecords: [TransferActivityRecord] {
        let active = records.filter {
            TransferPresentationPolicy.progress(for: $0.state) == .active
        }
        if !active.isEmpty { return active }
        return records.filter {
            ActivityProjectionPolicy.isPending($0.state)
                && TransferPresentationPolicy.progress(for: $0.state) != .hidden
        }
    }

    var progressTotalBytes: UInt64 {
        saturatingSum(progressRecords.map(\.totalBytes))
    }

    var progressBytesTransferred: UInt64 {
        min(
            saturatingSum(progressRecords.map(\.bytesTransferred)),
            progressTotalBytes
        )
    }

    var updatedAt: Date {
        records.map(\.updatedAt).max() ?? summaryRecord.updatedAt
    }

    private func saturatingSum(_ values: [UInt64]) -> UInt64 {
        values.reduce(0) { partial, value in
            let (sum, overflow) = partial.addingReportingOverflow(value)
            return overflow ? UInt64.max : sum
        }
    }

    private static func recordsNewestFirst(
        _ lhs: TransferActivityRecord,
        _ rhs: TransferActivityRecord
    ) -> Bool {
        if lhs.updatedAt != rhs.updatedAt { return lhs.updatedAt > rhs.updatedAt }
        return lhs.activityId < rhs.activityId
    }

    private static func priority(of state: TransferActivityState) -> Int {
        switch state {
        case .awaitingDecision:
            return 6
        case .preparing, .waitingForPeer, .pairing, .connecting,
             .transferring, .verifying, .saving, .waitingForReceiverSave,
             .finalizingDelivery:
            return 5
        case .paused:
            return 4
        case .failed:
            return 3
        case .canceled:
            return 2
        case .delivered:
            return 1
        }
    }
}

func activityRoomGroups(_ records: [TransferActivityRecord]) -> [ActivityRoomGroup] {
    var grouped: [String: [TransferActivityRecord]] = [:]
    var activityGroupIDByKey: [String: String] = [:]

    for record in records {
        let normalizedGroupID = record.activityGroupID?.trimmed
        let groupKey: String
        if let normalizedGroupID, !normalizedGroupID.isEmpty {
            groupKey = "group:\(normalizedGroupID)"
            activityGroupIDByKey[groupKey] = normalizedGroupID
        } else {
            groupKey = "direct:\(record.activityId)"
        }
        grouped[groupKey, default: []].append(record)
    }

    return grouped.compactMap { key, records in
        ActivityRoomGroup(
            id: key,
            activityGroupID: activityGroupIDByKey[key],
            records: records
        )
    }.sorted {
        if $0.updatedAt != $1.updatedAt { return $0.updatedAt > $1.updatedAt }
        return $0.id < $1.id
    }
}

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
    @State private var expandedRoomIDs: Set<String> = []
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
                        Text(AppText.localized("activity.empty.title", language: language))
                            .font(.headline)
                            .foregroundStyle(Theme.text)
                        Text(AppText.localized("activity.empty.detail", language: language))
                            .font(.subheadline)
                            .foregroundStyle(Theme.muted)
                            .multilineTextAlignment(.center)
                    }
                    .frame(maxWidth: .infinity)
                    .padding(.vertical, 48)
                } else {
                    ForEach(roomGroups) { group in
                        roomCard(group)
                    }
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

    private func roomCard(_ group: ActivityRoomGroup) -> some View {
        let isExpanded = expandedRoomIDs.contains(group.id)
        let metrics = aggregateMetrics(for: group)
        return VStack(alignment: .leading, spacing: 12) {
            Button {
                withAnimation(.easeInOut(duration: 0.18)) {
                    if isExpanded {
                        expandedRoomIDs.remove(group.id)
                    } else {
                        expandedRoomIDs.insert(group.id)
                    }
                }
            } label: {
                HStack(alignment: .top, spacing: 12) {
                    Image(systemName: group.activityGroupID == nil
                        ? "arrow.left.arrow.right"
                        : "person.2.fill")
                        .font(.title2)
                        .foregroundStyle(tint(for: group.summaryRecord.state))
                        .frame(width: 34)
                    VStack(alignment: .leading, spacing: 4) {
                        Text(groupTitle(group))
                            .font(.headline.weight(.semibold))
                            .foregroundStyle(Theme.text)
                        Text(roomSummary(group))
                            .font(.subheadline)
                            .foregroundStyle(Theme.muted)
                            .multilineTextAlignment(.leading)
                    }
                    Spacer(minLength: 8)
                    Text(transferCountText(group.records.count))
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
            .accessibilityIdentifier("activity_room_\(group.id)")
            .accessibilityLabel(groupTitle(group))
            .accessibilityValue(isExpanded
                ? AppText.localized("accessibility.expanded", language: language)
                : AppText.localized("accessibility.collapsed", language: language))
            .accessibilityHint(isExpanded
                ? AppText.localized("activity.accessibility.collapse_hint", language: language)
                : AppText.localized("activity.accessibility.expand_hint", language: language))

            HStack(spacing: 8) {
                Label(
                    TransferActivityText.state(
                        group.summaryRecord.state,
                        direction: group.summaryRecord.direction,
                        language: language
                    ),
                    systemImage: icon(for: group.summaryRecord)
                )
                .foregroundStyle(tint(for: group.summaryRecord.state))
                Spacer(minLength: 8)
                Text(updatedText(group.updatedAt))
                    .foregroundStyle(Theme.muted)
            }
            .font(.caption.weight(.semibold))

            if group.progressTotalBytes > 0 {
                ProgressView(
                    value: Double(group.progressBytesTransferred),
                    total: Double(group.progressTotalBytes)
                )
                Text(
                    "\(byteString(group.progressBytesTransferred)) / "
                        + byteString(group.progressTotalBytes)
                )
                .font(.caption.monospacedDigit())
                .foregroundStyle(Theme.muted)
            }

            TransferPerformanceLine(
                currentBytesPerSecond: metrics.speedBps,
                averageBytesPerSecond: metrics.averageSpeedBps,
                etaSeconds: metrics.etaSeconds,
                currentSampleDate: metrics.currentRateUpdatedAt,
                accessibilityPrefix: "activity_room_\(group.id)"
            )

            if let path = uniformConnectionPath(for: group.records) {
                Label(
                    ConnectionPathPresentationPolicy.label(for: path, language: language),
                    systemImage: pathIcon(path)
                )
                .font(.caption.weight(.semibold))
                .foregroundStyle(Theme.muted)
                .accessibilityIdentifier("activity_room_path_\(group.id)")
            }

            if isExpanded {
                Divider()
                VStack(spacing: 10) {
                    ForEach(group.records) { record in
                        transferCard(record)
                    }
                }
                .transition(.opacity.combined(with: .move(edge: .top)))
            }
        }
        .card(raised: true, padding: 16)
    }

    private func transferCard(_ record: TransferActivityRecord) -> some View {
        let actions = activityActionAvailability(for: record)
        let progress = TransferPresentationPolicy.progress(for: record.state)
        let metrics = metricsByActivityID[record.activityId] ?? ActivityMetrics()
        return VStack(alignment: .leading, spacing: 10) {
            HStack(alignment: .top, spacing: 11) {
                Image(systemName: icon(for: record))
                    .font(.title3)
                    .foregroundStyle(tint(for: record.state))
                    .frame(width: 28)
                VStack(alignment: .leading, spacing: 3) {
                    Text(title(for: record))
                        .font(.subheadline.weight(.semibold))
                        .foregroundStyle(Theme.text)
                    Text(transferSummary(record))
                        .font(.caption)
                        .foregroundStyle(Theme.muted)
                }
                Spacer(minLength: 8)
                Text(TransferActivityText.direction(record.direction, language: language))
                    .font(.caption.weight(.semibold))
                    .foregroundStyle(tint(for: record.state))
            }
            .accessibilityIdentifier("activity_\(record.activityId)")

            if record.state != .delivered, progress != .hidden, record.totalBytes > 0 {
                ProgressView(
                    value: Double(record.bytesTransferred),
                    total: Double(record.totalBytes)
                )
                HStack {
                    Text("\(byteString(record.bytesTransferred)) / \(byteString(record.totalBytes))")
                }
                .font(.caption.monospacedDigit())
                .foregroundStyle(Theme.muted)
            }

            TransferPerformanceLine(
                currentBytesPerSecond: progress == .active ? metrics.speedBps : 0,
                averageBytesPerSecond: metrics.averageSpeedBps,
                etaSeconds: progress == .active ? metrics.etaSeconds : nil,
                currentSampleDate: metrics.currentRateUpdatedAt,
                accessibilityPrefix: "activity_\(record.activityId)"
            )

            if record.direction == .receive,
               record.state == .delivered,
               !record.savedPaths.isEmpty {
                completedReceiveControls(record)
            }

            if let path = record.connectionPath {
                Label(
                    ConnectionPathPresentationPolicy.label(for: path, language: language),
                    systemImage: pathIcon(path)
                )
                .font(.footnote.weight(.semibold))
                .foregroundStyle(Theme.muted)
                .accessibilityIdentifier("activity_path_\(record.activityId)")
            }

            stageTimingSection(metrics, activityID: record.activityId)

            if !record.diagnosticMessage.isEmpty {
                Text(record.diagnosticMessage)
                    .font(.footnote)
                    .foregroundStyle(record.state == .failed ? Theme.danger : Theme.muted)
                    .textSelection(.enabled)
            }

            HStack(spacing: 8) {
                if actions.canApprove {
                    Button(AppText.localized("common.receive", language: language)) {
                        _ = onApprove(record.activityId)
                    }
                    .buttonStyle(.borderedProminent)
                    .accessibilityIdentifier("activity_receive_\(record.activityId)")
                }
                if actions.canPause {
                    Button(AppText.localized("common.pause", language: language)) {
                        _ = onPause(record.activityId)
                    }
                    .accessibilityIdentifier("activity_pause_\(record.activityId)")
                }
                if actions.canResume, onCanResume(record.activityId) {
                    Button(AppText.localized("common.resume", language: language)) {
                        _ = onResume(record.activityId)
                    }
                    .accessibilityIdentifier("activity_resume_\(record.activityId)")
                }
                if actions.canCancel {
                    Button(
                        AppText.localized("common.cancel", language: language),
                        role: .destructive
                    ) {
                        _ = onCancel(record.activityId)
                    }
                    .accessibilityIdentifier("activity_cancel_\(record.activityId)")
                }
                Spacer()
                if developerMode || actions.canDelete {
                    Menu {
                        if developerMode {
                            Button(AppText.localized("activity.diagnostics.copy", language: language)) {
                                copyWithToast(
                                    onCopyDiagnostics(record),
                                    AppText.localized("activity.diagnostics.copied", language: language),
                                    language: language
                                )
                            }
                            if let target = onRemoteLogTarget(record) {
                                Button(AppText.localized("activity.diagnostics.upload", language: language)) {
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
                            Button(AppText.localized("activity.diagnostics.copy_app", language: language)) {
                                copyWithToast(
                                    onAppDiagnosticReport(),
                                    AppText.localized("activity.diagnostics.copied", language: language),
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
                            .accessibilityIdentifier("activity_delete_\(record.activityId)")
                        }
                    } label: {
                        Image(systemName: "ellipsis.circle")
                            .frame(width: 32, height: 32)
                            .contentShape(Rectangle())
                    }
                    .disabled(pendingRemovalIDs.contains(record.activityId))
                    .accessibilityIdentifier("activity_more_\(record.activityId)")
                }
            }
            .buttonStyle(.bordered)
        }
        .padding(12)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(Theme.surface)
        .overlay(
            RoundedRectangle(cornerRadius: 12)
                .strokeBorder(Theme.line.opacity(0.65), lineWidth: 0.8)
        )
        .clipShape(RoundedRectangle(cornerRadius: 12))
    }

    private var roomGroups: [ActivityRoomGroup] {
        activityRoomGroups(records)
    }

    private func groupTitle(_ group: ActivityRoomGroup) -> String {
        if let label = group.activityGroupLabel {
            return label
        }
        return group.activityGroupID == nil
            ? AppText.localized("activity.group.standalone", language: language)
            : AppText.localized("activity.group.one_time_room", language: language)
    }

    private func roomSummary(_ group: ActivityRoomGroup) -> String {
        var parts = [itemCountText(group.itemCount)]
        if group.totalBytes > 0 {
            parts.append(byteString(group.totalBytes))
        }
        return parts.joined(separator: " · ")
    }

    private func transferSummary(_ record: TransferActivityRecord) -> String {
        var parts = [
            TransferActivityText.state(
                record.state,
                direction: record.direction,
                language: language
            ),
            itemCountText(UInt64(record.itemCount)),
        ]
        if record.totalBytes > 0 {
            parts.append(byteString(record.totalBytes))
        }
        parts.append(updatedText(record.updatedAt))
        return parts.joined(separator: " · ")
    }

    private func itemCountText(_ count: UInt64) -> String {
        TransferActivityText.itemCount(count, language: language)
    }

    private func transferCountText(_ count: Int) -> String {
        TransferActivityText.transferCount(count, language: language)
    }

    private func updatedText(_ date: Date) -> String {
        let formatter = RelativeDateTimeFormatter()
        formatter.unitsStyle = .abbreviated
        formatter.locale = Locale(identifier: language.hasPrefix("zh") ? "zh_Hans" : "en")
        let relative = formatter.localizedString(for: date, relativeTo: Date())
        return TransferActivityText.updated(relative, language: language)
    }

    private func aggregateMetrics(for group: ActivityRoomGroup) -> ActivityMetrics {
        let trackedRecords = group.records
        let activeRecords = trackedRecords.filter {
            TransferPresentationPolicy.progress(for: $0.state) == .active
        }
        let speed = activeRecords.reduce(0.0) { partial, record in
            saturatingRateSum(
                partial,
                boundedRate(metricsByActivityID[record.activityId]?.speedBps ?? 0)
            )
        }
        let recordsWithRemainingBytes = activeRecords.filter {
            $0.totalBytes > $0.bytesTransferred
        }
        let eta: Double?
        if recordsWithRemainingBytes.isEmpty {
            eta = nil
        } else {
            let stableETAs = recordsWithRemainingBytes.compactMap { record -> Double? in
                guard TransferPresentationPolicy.progress(for: record.state) == .active else {
                    return nil
                }
                return boundedETA(metricsByActivityID[record.activityId]?.etaSeconds)
            }
            eta = stableETAs.count == recordsWithRemainingBytes.count
                ? stableETAs.max()
                : nil
        }
        return ActivityMetrics(
            speedBps: speed,
            averageSpeedBps: trackedRecords.count == 1
                ? boundedRate(
                    metricsByActivityID[trackedRecords[0].activityId]?.averageSpeedBps ?? 0
                )
                : 0,
            etaSeconds: eta
        )
    }

    private func uniformConnectionPath(
        for records: [TransferActivityRecord]
    ) -> FfiDataPathKind? {
        guard let first = records.first?.connectionPath,
              records.allSatisfy({ $0.connectionPath == first }) else {
            return nil
        }
        return first
    }

    private func boundedRate(_ value: Double) -> Double {
        guard value.isFinite, value > 0 else { return 0 }
        return min(value, Double(Int64.max))
    }

    private func saturatingRateSum(_ lhs: Double, _ rhs: Double) -> Double {
        let maximum = Double(Int64.max)
        guard lhs < maximum, rhs < maximum - lhs else { return maximum }
        return lhs + rhs
    }

    private func boundedETA(_ value: Double?) -> Double? {
        guard let value, value.isFinite, value >= 0 else { return nil }
        return min(value, Double(Int.max).nextDown)
    }

    @ViewBuilder
    private func stageTimingSection(
        _ metrics: ActivityMetrics,
        activityID: String
    ) -> some View {
        let samples = ActivityStageTimingPresentationPolicy.latestAttempt(
            from: metrics.stageTimings
        ).filter { $0.stage != .sessionStarted }
        if !samples.isEmpty {
            VStack(alignment: .leading, spacing: 8) {
                Label(
                    AppText.localized("activity.timeline", language: language),
                    systemImage: "stopwatch"
                )
                .font(.caption.weight(.semibold))
                .foregroundStyle(Theme.text)

                LazyVGrid(
                    columns: [GridItem(.adaptive(minimum: 104), alignment: .leading)],
                    alignment: .leading,
                    spacing: 7
                ) {
                    ForEach(Array(samples.enumerated()), id: \.offset) { _, sample in
                        VStack(alignment: .leading, spacing: 2) {
                            Text(TransferActivityText.stage(sample.stage, language: language))
                                .font(.caption2)
                                .foregroundStyle(Theme.muted)
                                .lineLimit(1)
                            Text(ActivityStageTimingPresentationPolicy.elapsedString(
                                microseconds: sample.elapsedMicroseconds
                            ))
                                .font(.caption.monospacedDigit().weight(.semibold))
                                .foregroundStyle(Theme.text)
                        }
                        .padding(.horizontal, 9)
                        .padding(.vertical, 7)
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .background(
                            Theme.surfaceRaised,
                            in: RoundedRectangle(cornerRadius: 9)
                        )
                        .accessibilityElement(children: .combine)
                        .accessibilityIdentifier(
                            "activity_stage_\(stageIdentifier(sample.stage))_\(activityID)"
                        )
                    }
                }
            }
            .accessibilityIdentifier("activity_stage_timing_\(activityID)")
        }
    }

    private func pathIcon(_ path: FfiDataPathKind) -> String {
        switch path {
        case .direct, .directIpv4, .directIpv6: return "arrow.left.and.right"
        case .relay: return "point.3.connected.trianglepath.dotted"
        case .wifiAware: return "wifi"
        case .other: return "link"
        }
    }

    private func stageIdentifier(_ stage: FfiTransferStage) -> String {
        switch stage {
        case .sessionStarted: return "session_started"
        case .connectionReady: return "connection_ready"
        case .authenticationStarted: return "authentication_started"
        case .authenticationComplete: return "authentication_complete"
        case .manifestOffer: return "manifest_offer"
        case .manifestAccepted: return "manifest_accepted"
        case .firstPayload: return "first_payload"
        case .payloadComplete: return "payload_complete"
        case .deliveryComplete: return "delivery_complete"
        case .canceled: return "canceled"
        case .failed: return "failed"
        }
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
                            AppText.localized("common.share", language: language),
                            systemImage: "square.and.arrow.up"
                        )
                    }
                } else {
                    Button {
                        receivedItemsPresentation = ReceivedItemsPresentation(urls: urls)
                    } label: {
                        Label(
                            AppText.localized("activity.saved.view_items", language: language),
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
            return TransferActivityText.savedIn(parent.lastPathComponent, language: language)
        }
        return TransferActivityText.savedItems(urls.count, language: language)
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
                ? AppText.localized("activity.outgoing", language: language)
                : AppText.localized("activity.incoming", language: language)
        }
        return TransferActivityText.itemCount(UInt64(count), language: language)
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
