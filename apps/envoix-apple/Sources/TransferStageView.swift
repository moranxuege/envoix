import SwiftUI
import EnvoixCore
#if os(iOS)
import QuickLook
#endif

// Extracted from ContentView.swift (2026-07-20 split, no behavior change)

private struct ActivityActionButtonStyle: ButtonStyle {
    @Environment(\.isEnabled) private var isEnabled
    let tint: Color

    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .foregroundStyle(isEnabled ? tint : Theme.text)
            .background(
                isEnabled ? Theme.surfaceRaised : Theme.line,
                in: RoundedRectangle(cornerRadius: Theme.cardRadius)
            )
            .overlay(
                RoundedRectangle(cornerRadius: Theme.cardRadius)
                    .strokeBorder(isEnabled ? tint.opacity(0.45) : Theme.line, lineWidth: 1)
            )
            .opacity(configuration.isPressed ? 0.82 : 1)
    }
}

func manifestRootEntriesForDisplay(
    _ record: FfiManifestActivityRecord
) -> [FfiPreparedManifestEntry] {
    guard record.rootCount > 0 else { return [] }
    return Array(
        record.entries.lazy
            .filter { !$0.relativePath.contains("/") }
            .prefix(Int(record.rootCount))
    )
}

struct TransferStageView: View {
    private enum UploadStatus {
        case uploading
        case uploaded
        case failed(String)
    }

    private enum ActivityCommand: Equatable {
        case pause
        case resume
        case cancel
    }

    private static let manifestRootPreviewLimit = 6

    @Environment(\.appLanguage) private var language
    @Environment(\.dynamicTypeSize) private var dynamicTypeSize
    @AppStorage("envoix.developerMode") private var developerMode = false
    @AppStorage("envoix.logServer") private var logServer = defaultLogServer
    @State private var expandedActivityIDs: Set<String> = []
    @State private var pendingCommands: [String: ActivityCommand] = [:]
    @State private var uploadingActivityIDs: Set<String> = []
    @State private var uploadStatusByActivityID: [String: UploadStatus] = [:]
    @State private var isUploadingAppDiagnostics = false
    @State private var appUploadStatus: UploadStatus?
    #if os(iOS)
    @State private var publicationTargetActivityID: String?
    @State private var isPublicationFolderPickerPresented = false
    @State private var previewFileURL: URL?
    @State private var receivedItemsPresentation: ReceivedItemsPresentation?
    #endif
    private let commandAcknowledgementTimeout: TimeInterval = 5
    let records: [FfiTransferActivityRecord]
    let pendingRemovalIDs: Set<String>
    let manifestByActivityID: [String: FfiManifestActivityRecord]
    let metricsByActivityID: [String: ActivityMetrics]
    let onCopyDiagnostics: (FfiTransferActivityRecord) -> String
    let onRemoteLogTarget: (FfiTransferActivityRecord) -> RemoteLogUpload.Target?
    let onRemoteDiagnosticReport: (FfiTransferActivityRecord) -> String
    let onAppDiagnosticReport: () -> String
    let onPause: (String) -> Bool
    let onCanResume: (String) -> Bool
    let onResume: (String) -> Bool
    let onCancel: (String) -> Bool
    let onReplacePublicationTarget: (String, URL, Data?, AnyObject?) -> Bool
    let onDelete: (String) -> Bool

    var body: some View {
        ScrollView {
            VStack(spacing: 12) {
                if developerMode && RemoteLogUpload.isEnabledInCurrentBuild && !logServer.trimmed.isEmpty {
                    appDiagnosticsCard
                }
                if records.isEmpty {
                    emptyActivityView
                } else {
                    ForEach(records, id: \.activityId) { record in
                        activityCard(record)
                            #if os(iOS)
                            .swipeActions(edge: .trailing, allowsFullSwipe: false) {
                                if canDelete(record) && !pendingRemovalIDs.contains(record.activityId) {
                                    Button(role: .destructive) {
                                        requestDeletion(record.activityId)
                                    } label: {
                                        Label(AppText.value("Delete", "删除", language: language), systemImage: "trash")
                                    }
                                } else if canCancel(record) {
                                    Button(role: .destructive) {
                                        requestCommand(.cancel, for: record.activityId)
                                    } label: {
                                        Label(AppText.value("Cancel", "取消", language: language), systemImage: "xmark")
                                    }
                                }
                            }
                            #endif
                    }
                }
            }
            .padding(.vertical, 12)
        }
        .onChange(of: activityStateFingerprint) { _ in
            reconcilePendingCommands()
        }
        #if os(iOS)
        .quickLookPreview($previewFileURL)
        .sheet(isPresented: $isPublicationFolderPickerPresented) {
            FolderPickerSheet(
                onPick: replacePublicationTarget,
                onCancel: {
                    publicationTargetActivityID = nil
                    isPublicationFolderPickerPresented = false
                }
            )
        }
        .sheet(item: $receivedItemsPresentation) { presentation in
            ReceivedItemsSheet(urls: presentation.urls)
        }
        #endif
    }

    private var appDiagnosticsCard: some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack(spacing: 10) {
                Image(systemName: "stethoscope")
                    .font(.title3.weight(.semibold))
                    .foregroundStyle(Theme.accentStrong)
                    .frame(width: 36, height: 36)
                    .background(Theme.accentSoft, in: Circle())
                VStack(alignment: .leading, spacing: 2) {
                    Text(AppText.value("App diagnostic log", "应用诊断日志", language: language))
                        .font(.headline.weight(.semibold))
                        .foregroundStyle(Theme.text)
                    Text(AppText.value(
                        "Available before a transfer starts. Sensitive connection data is redacted.",
                        "无需先开始传输；敏感连接信息会被脱敏。",
                        language: language
                    ))
                    .font(.footnote)
                    .foregroundStyle(Theme.muted)
                }
            }

            HStack(spacing: 10) {
                Button {
                    copyWithToast(
                        onAppDiagnosticReport(),
                        AppText.value("App diagnostics copied", "应用诊断已复制", language: language),
                        language: language
                    )
                } label: {
                    Label(AppText.value("Copy report", "复制报告", language: language), systemImage: "doc.on.doc")
                        .font(.subheadline.weight(.semibold))
                        .frame(maxWidth: .infinity, minHeight: 44)
                }
                .buttonStyle(.bordered)
                .tint(Theme.accent)

                Button(action: uploadAppDiagnostics) {
                    Label(AppText.value("Upload report", "上传报告", language: language), systemImage: "arrow.up.doc")
                        .font(.subheadline.weight(.semibold))
                        .frame(maxWidth: .infinity, minHeight: 44)
                }
                .buttonStyle(.borderedProminent)
                .tint(Theme.accent)
                .disabled(isUploadingAppDiagnostics)
                .accessibilityIdentifier("app_upload_diagnostics")
            }

            if let appUploadStatus {
                Text(uploadStatusText(appUploadStatus))
                    .font(.footnote)
                    .foregroundStyle(uploadStatusColor(appUploadStatus))
            }
        }
        .card(raised: true, padding: 16)
    }

    private var emptyActivityView: some View {
        VStack(spacing: 12) {
            Image(systemName: "tray")
                .font(.system(size: 42, weight: .light))
                .foregroundStyle(Theme.muted)
            Text(AppText.value("No transfers yet", "暂无传输", language: language))
                .font(.headline.weight(.semibold))
                .foregroundStyle(Theme.text)
            Text(AppText.value("Start a send or receive from Home.", "请从“首页”开始发送或接收。", language: language))
                .font(.body)
                .foregroundStyle(Theme.muted)
                .multilineTextAlignment(.center)
        }
        .frame(maxWidth: .infinity, minHeight: 260)
    }

    private func activityCard(_ record: FfiTransferActivityRecord) -> some View {
        let metrics = metrics(for: record)
        let expanded = expandedActivityIDs.contains(record.activityId)
        let metadataLayout = dynamicTypeSize.isAccessibilitySize
            ? AnyLayout(VStackLayout(alignment: .leading, spacing: 6))
            : AnyLayout(HStackLayout(alignment: .firstTextBaseline, spacing: 8))
        let headerLayout = dynamicTypeSize.isAccessibilitySize
            ? AnyLayout(VStackLayout(alignment: .leading, spacing: 12))
            : AnyLayout(HStackLayout(alignment: .top, spacing: 12))
        return VStack(alignment: .leading, spacing: 14) {
            headerLayout {
                Image(systemName: activityIcon(for: record))
                    .font(.title3.weight(.semibold))
                    .foregroundStyle(activityTint(for: record))
                    .frame(width: 40, height: 40)
                    .background(activityTint(for: record).opacity(0.10), in: Circle())
                VStack(alignment: .leading, spacing: 6) {
                    Text(activityTitle(for: record))
                        .font(.headline.weight(.semibold))
                        .foregroundStyle(Theme.text)
                        .fixedSize(horizontal: false, vertical: true)
                    metadataLayout {
                        Text(activitySubtitle(for: record))
                            .font(.subheadline)
                            .foregroundStyle(Theme.muted)
                            .fixedSize(horizontal: false, vertical: true)
                            .frame(maxWidth: .infinity, alignment: .leading)
                        ModePill(text: activityStateText(for: record))
                            .fixedSize(horizontal: true, vertical: false)
                    }
                }
                .frame(maxWidth: .infinity, alignment: .leading)
            }
            .accessibilityElement(children: .combine)
            .accessibilityIdentifier("activity_title_\(record.activityId)")

            activitySummary(record, metrics: metrics)

            if let manifest = manifestByActivityID[record.activityId] {
                manifestSummary(manifest)
            }

            if record.totalBytes > 0 && !isTerminal(record) {
                ProgressBar(value: progressFraction(for: record))
            }

            if let recoveryText = recoveryText(for: record) {
                HStack(alignment: .top, spacing: 8) {
                    Image(systemName: "arrow.clockwise.circle")
                        .foregroundStyle(Theme.warning)
                    Text(recoveryText)
                        .font(.body)
                        .foregroundStyle(Theme.text)
                        .fixedSize(horizontal: false, vertical: true)
                    Spacer(minLength: 4)
                }
            }

            activityActions(record, expanded: expanded)

            if expanded {
                activityDetail(record, metrics: metrics)
            }
        }
        .card(raised: true, padding: 18)
        .onLongPressGesture {
            toggleActivityDetail(record.activityId)
        }
    }

    private func activitySummary(_ record: FfiTransferActivityRecord, metrics: ActivityMetrics) -> some View {
        var parts = [directionText(record.direction)]
        if isFullyResumedCompletion(record) {
            switch record.direction {
            case .send:
                parts.append(AppText.value("No data sent", "未发送数据", language: language))
                parts.append(AppText.value("Receiver already has this file", "对方已有此文件", language: language))
            case .receive:
                parts.append(AppText.value("No data received", "未接收数据", language: language))
                parts.append(AppText.value("File already exists", "文件已存在", language: language))
            case .unknown:
                parts.append(AppText.value("No data transferred", "未传输数据", language: language))
                parts.append(AppText.value("File already exists", "文件已存在", language: language))
            }
        } else if record.totalBytes > 0 {
            parts.append("\(byteString(record.bytesTransferred)) / \(byteString(record.totalBytes))")
        }
        if let speed = speedBps(for: record, metrics: metrics), speed > 0 {
            parts.append(rateString(speed))
        }
        if record.state == .transferring, let eta = metrics.etaSeconds {
            parts.append(etaString(eta))
        }
        return Text(parts.joined(separator: " · "))
            .font(.subheadline.monospacedDigit())
            .foregroundStyle(Theme.muted)
            .fixedSize(horizontal: false, vertical: true)
    }

    private func manifestSummary(_ manifest: FfiManifestActivityRecord) -> some View {
        VStack(alignment: .leading, spacing: 7) {
            HStack(spacing: 8) {
                Label(manifestInventoryText(manifest), systemImage: "square.stack.3d.up")
                    .font(.subheadline.weight(.semibold))
                    .foregroundStyle(Theme.text)
                    .lineLimit(2)
                Spacer(minLength: 8)
                if manifest.fileCount > 0 {
                    Text("\(manifest.completedFiles)/\(manifest.fileCount)")
                        .font(.subheadline.monospacedDigit().weight(.semibold))
                        .foregroundStyle(Theme.accentStrong)
                        .accessibilityLabel(AppText.value(
                            "\(manifest.completedFiles) of \(manifest.fileCount) files complete",
                            "\(manifest.fileCount) 个文件中已完成 \(manifest.completedFiles) 个",
                            language: language
                        ))
                }
            }

            if let current = manifest.currentEntry, !current.relativePath.isEmpty {
                HStack(spacing: 7) {
                    Image(systemName: "arrow.right")
                        .font(.caption.weight(.bold))
                        .foregroundStyle(Theme.accentStrong)
                    Text(current.relativePath)
                        .font(.footnote)
                        .foregroundStyle(Theme.muted)
                        .lineLimit(1)
                        .truncationMode(.middle)
                    Spacer(minLength: 6)
                    if current.totalBytes > 0 {
                        Text("\(byteString(current.bytesTransferred)) / \(byteString(current.totalBytes))")
                            .font(.caption.monospacedDigit())
                            .foregroundStyle(Theme.muted)
                    }
                }
            }
        }
        .padding(11)
        .background(Theme.accentSoft.opacity(0.55), in: RoundedRectangle(cornerRadius: Theme.cardRadius))
        .accessibilityIdentifier("activity_manifest_summary_\(manifest.activity.activityId)")
    }

    @ViewBuilder
    private func activityActions(_ record: FfiTransferActivityRecord, expanded: Bool) -> some View {
        let actionLayout = dynamicTypeSize.isAccessibilitySize
            ? AnyLayout(VStackLayout(spacing: 10))
            : AnyLayout(HStackLayout(spacing: 10))
        actionLayout {
            if pendingRemovalIDs.contains(record.activityId) {
                activityRemovalIndicator(activityID: record.activityId)
            } else if let command = pendingCommands[record.activityId] {
                activityCommandIndicator(command, activityID: record.activityId)
            } else if shouldChoosePublicationFolder(record) {
                activityAction(
                    AppText.value("Choose folder", "选择文件夹", language: language),
                    systemImage: "folder.badge.plus",
                    tint: Theme.accentStrong
                ) {
                    choosePublicationFolder(for: record.activityId)
                }
                .accessibilityIdentifier("activity_choose_folder_\(record.activityId)")
            } else if canResume(record) {
                let resumeAvailable = onCanResume(record.activityId)
                activityAction(
                    !resumeAvailable
                        ? AppText.value("Waiting", "等待", language: language)
                        : record.state == .publishing || record.state == .failed
                        ? AppText.value("Retry", "重试", language: language)
                        : AppText.value("Resume", "继续", language: language),
                    systemImage: !resumeAvailable
                        ? "hourglass"
                        : record.state == .publishing || record.state == .failed
                        ? "arrow.clockwise"
                        : "play.fill",
                    tint: Theme.accentStrong
                ) {
                    requestCommand(.resume, for: record.activityId)
                }
                .disabled(!resumeAvailable)
                .accessibilityHint(resumeAvailable
                    ? ""
                    : AppText.value(
                        "Resume becomes available when another transfer finishes or pauses.",
                        "其他任务完成或暂停后即可继续。",
                        language: language
                    ))
                .accessibilityIdentifier("activity_resume_\(record.activityId)")
            } else if canPause(record) {
                activityAction(
                    AppText.value("Pause", "暂停", language: language),
                    systemImage: "pause.fill",
                    tint: Theme.warning
                ) {
                    requestCommand(.pause, for: record.activityId)
                }
                .accessibilityIdentifier("activity_pause_\(record.activityId)")
            }

            if !pendingRemovalIDs.contains(record.activityId) {
                activityAction(
                    expanded
                        ? AppText.value("Hide details", "收起详情", language: language)
                        : AppText.value("Details", "查看详情", language: language),
                    systemImage: expanded ? "chevron.up" : "chevron.down",
                    tint: Theme.accentStrong
                ) {
                    toggleActivityDetail(record.activityId)
                }
                .accessibilityIdentifier("activity_details_\(record.activityId)")

                if pendingCommands[record.activityId] == nil && canCancel(record) {
                    destructiveActivityAction(
                        AppText.value("Cancel", "取消", language: language),
                        systemImage: "xmark"
                    ) {
                        requestCommand(.cancel, for: record.activityId)
                    }
                    .accessibilityIdentifier("activity_cancel_\(record.activityId)")
                } else if pendingCommands[record.activityId] == nil && canDelete(record) {
                    destructiveActivityAction(
                        AppText.value("Delete", "删除", language: language),
                        systemImage: "trash"
                    ) {
                        requestDeletion(record.activityId)
                    }
                    .accessibilityIdentifier("activity_delete_\(record.activityId)")
                }
            }
        }
    }

    private func activityCommandIndicator(_ command: ActivityCommand, activityID: String) -> some View {
        HStack(spacing: 8) {
            ProgressView()
                .controlSize(.small)
            Text(activityCommandText(command))
                .font(.subheadline.weight(.semibold))
                .lineLimit(1)
        }
        .foregroundStyle(Theme.muted)
        .frame(maxWidth: .infinity, minHeight: 44)
        .background(Theme.line.opacity(0.18), in: RoundedRectangle(cornerRadius: Theme.cardRadius))
        .accessibilityIdentifier("activity_command_\(activityID)")
    }

    private func activityRemovalIndicator(activityID: String) -> some View {
        HStack(spacing: 8) {
            ProgressView()
                .controlSize(.small)
            Text(AppText.value("Removing…", "正在删除…", language: language))
                .font(.subheadline.weight(.semibold))
                .lineLimit(1)
        }
        .foregroundStyle(Theme.muted)
        .frame(maxWidth: .infinity, minHeight: 44)
        .background(Theme.line.opacity(0.18), in: RoundedRectangle(cornerRadius: Theme.cardRadius))
        .accessibilityIdentifier("activity_removing_\(activityID)")
    }

    private func requestDeletion(_ activityID: String) {
        guard onDelete(activityID) else {
            ToastCenter.shared.show(AppText.value(
                "This Activity could not be removed. Try again.",
                "无法删除此活动，请重试。",
                language: language
            ))
            return
        }
    }

    private func requestCommand(_ command: ActivityCommand, for activityID: String) {
        guard pendingCommands[activityID] == nil else { return }
        let accepted: Bool
        switch command {
        case .pause:
            accepted = onPause(activityID)
        case .resume:
            accepted = onResume(activityID)
        case .cancel:
            accepted = onCancel(activityID)
        }
        if accepted {
            pendingCommands[activityID] = command
            DispatchQueue.main.asyncAfter(deadline: .now() + commandAcknowledgementTimeout) {
                guard pendingCommands[activityID] == command else { return }
                pendingCommands.removeValue(forKey: activityID)
                ToastCenter.shared.show(AppText.value(
                    "The action is taking longer than expected. You can try again.",
                    "操作响应超时，可以重试。",
                    language: language
                ))
            }
        } else {
            ToastCenter.shared.show(AppText.value(
                "This action is no longer available.",
                "当前状态已变化，无法执行此操作。",
                language: language
            ))
        }
    }

    private var activityStateFingerprint: String {
        records.map {
            "\($0.activityId):\(String(describing: $0.state)):\($0.retryable):\(String(describing: $0.recoveryAction))"
        }.joined(separator: "|")
    }

    private func shouldChoosePublicationFolder(_ record: FfiTransferActivityRecord) -> Bool {
        record.state == .publishing
            && record.retryable
            && record.recoveryAction == .chooseFolder
    }

    private func choosePublicationFolder(for activityID: String) {
        #if os(iOS)
        publicationTargetActivityID = activityID
        isPublicationFolderPickerPresented = true
        #else
        guard let url = chooseURL(directory: true) else { return }
        do {
            let bookmark = try makeSecurityScopedFolderBookmark(for: url)
            let access = SecurityScopedResourceAccess(url: url)
            try validateWritableDirectoryAccess(url)
            guard onReplacePublicationTarget(activityID, url, bookmark, access) else {
                ToastCenter.shared.show(AppText.value(
                    "This save target can no longer be changed.",
                    "当前已无法更换保存位置。",
                    language: language
                ))
                return
            }
            ToastCenter.shared.show(AppText.value(
                "Saving to the new folder",
                "正在保存到新文件夹",
                language: language
            ))
        } catch {
            ToastCenter.shared.show(AppText.value(
                "Envoix cannot write to that folder. Choose another folder or check its permissions.",
                "Envoix 无法写入该文件夹。请选择其他文件夹或检查权限。",
                language: language
            ))
        }
        #endif
    }

    #if os(iOS)
    private func replacePublicationTarget(_ url: URL) {
        defer {
            publicationTargetActivityID = nil
            isPublicationFolderPickerPresented = false
        }
        guard let activityID = publicationTargetActivityID else { return }
        do {
            let bookmark = try makeSecurityScopedFolderBookmark(for: url)
            let access = SecurityScopedResourceAccess(url: url)
            guard onReplacePublicationTarget(activityID, url, bookmark, access) else {
                ToastCenter.shared.show(AppText.value(
                    "This save target can no longer be changed.",
                    "当前已无法更换保存位置。",
                    language: language
                ))
                return
            }
            ToastCenter.shared.show(AppText.value(
                "Saving to the new folder",
                "正在保存到新文件夹",
                language: language
            ))
        } catch {
            ToastCenter.shared.show(friendlyError(error.localizedDescription, language: language))
        }
    }
    #endif

    private func reconcilePendingCommands() {
        pendingCommands = pendingCommands.filter { activityID, command in
            guard let record = records.first(where: { $0.activityId == activityID }) else { return false }
            switch command {
            case .pause:
                return record.state != .paused && !isTerminal(record)
            case .resume:
                return canResume(record)
            case .cancel:
                return !isTerminal(record)
            }
        }
    }

    private func activityCommandText(_ command: ActivityCommand) -> String {
        switch command {
        case .pause: return AppText.value("Pausing…", "正在暂停…", language: language)
        case .resume: return AppText.value("Resuming…", "正在继续…", language: language)
        case .cancel: return AppText.value("Cancelling…", "正在取消…", language: language)
        }
    }

    private func activityAction(
        _ title: String,
        systemImage: String,
        tint: Color,
        action: @escaping () -> Void
    ) -> some View {
        Button(action: action) {
            Label(title, systemImage: systemImage)
                .font(.subheadline.weight(.semibold))
                .multilineTextAlignment(.center)
                .frame(maxWidth: .infinity, minHeight: 44)
        }
        .buttonStyle(ActivityActionButtonStyle(tint: tint))
    }

    private func destructiveActivityAction(
        _ title: String,
        systemImage: String,
        action: @escaping () -> Void
    ) -> some View {
        Button(role: .destructive, action: action) {
            Label(title, systemImage: systemImage)
                .font(.subheadline.weight(.semibold))
                .multilineTextAlignment(.center)
                .frame(maxWidth: .infinity, minHeight: 44)
        }
        .buttonStyle(ActivityActionButtonStyle(tint: Theme.dangerStrong))
    }

    private func uploadDiagnostics(
        for record: FfiTransferActivityRecord,
        target: RemoteLogUpload.Target
    ) {
        guard !uploadingActivityIDs.contains(record.activityId) else { return }
        uploadingActivityIDs.insert(record.activityId)
        uploadStatusByActivityID[record.activityId] = .uploading

        Task {
            do {
                try await RemoteLogUpload.upload(
                    server: logServer,
                    target: target,
                    body: onRemoteDiagnosticReport(record)
                )
                uploadStatusByActivityID[record.activityId] = .uploaded
                ToastCenter.shared.show(AppText.value("Diagnostics uploaded", "诊断已上传", language: language))
            } catch {
                uploadStatusByActivityID[record.activityId] = .failed(error.localizedDescription)
                ToastCenter.shared.show(AppText.value("Diagnostic upload failed", "诊断上传失败", language: language))
            }
            uploadingActivityIDs.remove(record.activityId)
        }
    }

    private func uploadAppDiagnostics() {
        guard !isUploadingAppDiagnostics else { return }
        isUploadingAppDiagnostics = true
        appUploadStatus = .uploading

        Task {
            do {
                try await RemoteLogUpload.upload(
                    server: logServer,
                    target: RemoteLogUpload.appTarget(),
                    body: onAppDiagnosticReport()
                )
                appUploadStatus = .uploaded
                ToastCenter.shared.show(AppText.value("App diagnostic log uploaded", "应用诊断日志已上传", language: language))
            } catch {
                appUploadStatus = .failed(error.localizedDescription)
                ToastCenter.shared.show(AppText.value("App diagnostic log upload failed", "应用诊断日志上传失败", language: language))
            }
            isUploadingAppDiagnostics = false
        }
    }

    private func isPending(_ record: FfiTransferActivityRecord) -> Bool {
        switch record.state {
        case .queued, .binding, .waitingForPeer, .pairing, .connecting, .transferring,
                .verifying, .publishing, .unconfirmed, .paused:
            return true
        case .completed, .failed, .canceled, .unknown:
            return false
        }
    }

    private func isTerminal(_ record: FfiTransferActivityRecord) -> Bool {
        switch record.state {
        case .completed, .failed, .canceled:
            return true
        case .queued, .binding, .waitingForPeer, .pairing, .connecting, .transferring,
                .verifying, .publishing, .unconfirmed, .paused, .unknown:
            return false
        }
    }

    private func progressFraction(for record: FfiTransferActivityRecord) -> Double {
        guard record.totalBytes > 0 else { return 0 }
        return min(1, Double(record.bytesTransferred) / Double(record.totalBytes))
    }

    private func canPause(_ record: FfiTransferActivityRecord) -> Bool {
        activityActionAvailability(for: record).canPause
    }

    private func canResume(_ record: FfiTransferActivityRecord) -> Bool {
        activityActionAvailability(for: record).canResume
    }

    private func canCancel(_ record: FfiTransferActivityRecord) -> Bool {
        activityActionAvailability(for: record).canCancel
    }

    private func canDelete(_ record: FfiTransferActivityRecord) -> Bool {
        activityActionAvailability(for: record).canDelete
    }

    private func speedBps(for record: FfiTransferActivityRecord, metrics: ActivityMetrics) -> Double? {
        guard record.state == .transferring else { return nil }
        return metrics.speedBps
    }

    private func metrics(for record: FfiTransferActivityRecord) -> ActivityMetrics {
        metricsByActivityID[record.activityId] ?? ActivityMetrics()
    }

    private func toggleActivityDetail(_ activityID: String) {
        if expandedActivityIDs.contains(activityID) {
            expandedActivityIDs.remove(activityID)
        } else {
            expandedActivityIDs.insert(activityID)
        }
    }

    private func activityDetail(_ record: FfiTransferActivityRecord, metrics: ActivityMetrics) -> some View {
        VStack(alignment: .leading, spacing: 10) {
            Divider().overlay(Theme.line.opacity(0.6))

            if metrics.speedHistory.count >= 2 {
                HStack(alignment: .firstTextBaseline) {
                    Text(AppText.value("Speed", "速度", language: language))
                        .font(.caption.weight(.bold))
                        .foregroundStyle(Theme.muted)
                    Spacer(minLength: 8)
                    Text(speedSummary(metrics))
                        .font(.caption.monospacedDigit())
                        .foregroundStyle(Theme.muted)
                }
                SpeedSparkline(history: metrics.speedHistory, averageBps: metrics.avgBps)
            }

            VStack(spacing: 6) {
                if isFullyResumedCompletion(record) {
                    detailRow(
                        AppText.value("Transferred this attempt", "本次传输", language: language),
                        "0 B"
                    )
                    detailRow(
                        record.direction == .send
                            ? AppText.value("Already at receiver", "对方已有", language: language)
                            : AppText.value("Existing file", "已有文件", language: language),
                        byteString(record.totalBytes)
                    )
                } else {
                    detailRow(
                        AppText.value("Transferred", "已传输", language: language),
                        "\(byteString(record.bytesTransferred)) / \(byteString(record.totalBytes))"
                    )
                }
                if metrics.avgBps > 0 {
                    detailRow(AppText.value("Average", "平均速度", language: language), rateString(metrics.avgBps))
                }
                if metrics.peakBps > 0 {
                    detailRow(AppText.value("Peak", "峰值速度", language: language), rateString(metrics.peakBps))
                }
                if record.state == .transferring, let eta = metrics.etaSeconds {
                    detailRow(AppText.value("Remaining", "预计剩余", language: language), etaString(eta))
                }
                if record.dataPathKind != .none {
                    detailRow(AppText.value("Path", "链路", language: language), dataPathText(record))
                }
            }

            if let manifest = manifestByActivityID[record.activityId] {
                manifestDetail(manifest)
            }

            if record.direction == .receive {
                receiveDestinationDetail(
                    record,
                    manifest: manifestByActivityID[record.activityId]
                )
            }

            if developerMode {
                Divider().overlay(Theme.line.opacity(0.6))
                VStack(alignment: .leading, spacing: 6) {
                    Text(AppText.value("Developer details", "开发者详情", language: language))
                        .font(.callout.weight(.semibold))
                        .foregroundStyle(Theme.text)
                        .accessibilityIdentifier("activity_developer_details_\(record.activityId)")
                    detailRow("Activity ID", record.activityId)
                        .accessibilityIdentifier("activity_id_\(record.activityId)")
                    if !record.attemptId.isEmpty {
                        detailRow("Attempt ID", record.attemptId)
                    }
                    if !record.transferId.isEmpty {
                        detailRow("Transfer ID", record.transferId)
                    }
                    detailRow("State", "\(record.state) · \(record.direction) · \(record.mode)")
                    if let roomID = onRemoteLogTarget(record)?.roomID {
                        detailRow("Room", roomID)
                    }
                    if record.state == .failed {
                        detailRow("Failure", "\(record.failureCode) · \(record.failureCategory)")
                        detailRow("Origin", "\(record.failureOrigin) · \(record.recoveryAction)")
                    }
                }
            }

            if developerMode || record.state == .failed {
                Divider().overlay(Theme.line.opacity(0.6))
                VStack(alignment: .leading, spacing: 8) {
                    Text(AppText.value("Diagnostics", "诊断", language: language))
                        .font(.callout.weight(.semibold))
                        .foregroundStyle(Theme.text)

                    HStack(spacing: 10) {
                        activityAction(
                            AppText.value("Copy diagnostics", "复制诊断", language: language),
                            systemImage: "doc.on.doc",
                            tint: Theme.accentStrong
                        ) {
                            copyWithToast(
                                onCopyDiagnostics(record),
                                AppText.value("Diagnostics copied", "诊断信息已复制", language: language),
                                language: language
                            )
                        }

                        if
                            developerMode,
                            RemoteLogUpload.isEnabledInCurrentBuild,
                            !logServer.trimmed.isEmpty,
                            let remoteLogTarget = onRemoteLogTarget(record)
                        {
                            activityAction(
                                AppText.value("Upload diagnostic log", "上传诊断日志", language: language),
                                systemImage: "arrow.up.doc",
                                tint: Theme.accentStrong
                            ) {
                                uploadDiagnostics(for: record, target: remoteLogTarget)
                            }
                            .disabled(uploadingActivityIDs.contains(record.activityId))
                            .accessibilityIdentifier("activity_upload_diagnostics_\(record.activityId)")
                        }
                    }

                    if let uploadStatus = uploadStatusByActivityID[record.activityId] {
                        Text(uploadStatusText(uploadStatus))
                            .font(.footnote)
                            .foregroundStyle(uploadStatusColor(uploadStatus))
                    } else if developerMode && record.mode == .room && onRemoteLogTarget(record) == nil {
                        Text(AppText.value(
                            "This Room activity was created before diagnostic uploads were enabled. Start a new receiver to upload.",
                            "此 Room 活动创建于诊断上传启用之前。请新建一次接收后再上传。",
                            language: language
                        ))
                        .font(.footnote)
                        .foregroundStyle(Theme.muted)
                    }
                }
            }

            if developerMode && !metrics.log.isEmpty {
                HStack {
                    Text(AppText.value("Activity log", "活动日志", language: language))
                        .font(.caption.weight(.bold))
                        .foregroundStyle(Theme.muted)
                    Spacer(minLength: 8)
                    Button {
                        copyWithToast(
                            metrics.log.joined(separator: "\n"),
                            AppText.value("Activity log copied", "活动日志已复制", language: language),
                            language: language
                        )
                    } label: {
                        Image(systemName: "doc.on.doc")
                            .font(.caption.weight(.semibold))
                            .frame(width: 26, height: 26)
                            .contentShape(Rectangle())
                    }
                    .buttonStyle(.plain)
                    .foregroundStyle(Theme.accentStrong)
                    .help(AppText.value("Copy activity log", "复制活动日志", language: language))
                }
                ScrollView {
                    Text(metrics.log.joined(separator: "\n"))
                        .font(.caption.monospaced())
                        .foregroundStyle(Theme.muted)
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .textSelection(.enabled)
                }
                .frame(maxHeight: 120)
            }
        }
    }

    private func manifestDetail(_ manifest: FfiManifestActivityRecord) -> some View {
        let roots = manifestRootEntriesForDisplay(manifest)
        let visibleRoots = Array(roots.prefix(Self.manifestRootPreviewLimit))
        let hiddenRootCount = max(0, roots.count - visibleRoots.count)
        return VStack(alignment: .leading, spacing: 8) {
            Divider().overlay(Theme.line.opacity(0.6))
            Text(AppText.value("Transfer contents", "传输内容", language: language))
                .font(.callout.weight(.semibold))
                .foregroundStyle(Theme.text)

            VStack(spacing: 6) {
                if manifest.rootCount > 0 {
                    detailRow(
                        AppText.value("Selected", "已选择", language: language),
                        AppText.value(
                            "\(manifest.rootCount) top-level items",
                            "\(manifest.rootCount) 个顶层项目",
                            language: language
                        )
                    )
                }
                if manifest.fileCount > 0 {
                    detailRow(
                        AppText.value("Files complete", "已完成文件", language: language),
                        "\(manifest.completedFiles) / \(manifest.fileCount)"
                    )
                }
                if let resultText = manifestResultSummaryText(manifest) {
                    detailRow(AppText.value("Results", "处理结果", language: language), resultText)
                }
                if let current = manifest.currentEntry, !current.relativePath.isEmpty {
                    detailRow(AppText.value("Current item", "当前项目", language: language), current.relativePath)
                }
            }

            if !visibleRoots.isEmpty {
                VStack(alignment: .leading, spacing: 7) {
                    ForEach(visibleRoots, id: \.entryId) { entry in
                        Label(
                            entry.relativePath,
                            systemImage: entry.kind == .directory ? "folder" : "doc"
                        )
                        .font(.footnote)
                        .foregroundStyle(Theme.muted)
                        .lineLimit(2)
                        .truncationMode(.middle)
                    }
                    if hiddenRootCount > 0 {
                        Text(AppText.value(
                            "+ \(hiddenRootCount) more",
                            "另有 \(hiddenRootCount) 个",
                            language: language
                        ))
                        .font(.footnote.weight(.semibold))
                        .foregroundStyle(Theme.muted)
                    }
                }
                .padding(.top, 2)
            }
        }
        .accessibilityIdentifier("activity_manifest_detail_\(manifest.activity.activityId)")
    }

    private func manifestInventoryText(_ manifest: FfiManifestActivityRecord) -> String {
        var parts: [String] = []
        if manifest.fileCount > 0 {
            parts.append(AppText.value(
                "\(manifest.fileCount) files",
                "\(manifest.fileCount) 个文件",
                language: language
            ))
        }
        if manifest.directoryCount > 0 {
            parts.append(AppText.value(
                "\(manifest.directoryCount) folders",
                "\(manifest.directoryCount) 个文件夹",
                language: language
            ))
        }
        if parts.isEmpty {
            return AppText.value("Waiting for item list", "正在等待项目清单", language: language)
        }
        return parts.joined(separator: " · ")
    }

    private func manifestResultSummaryText(_ manifest: FfiManifestActivityRecord) -> String? {
        var skipped = 0
        var renamed = 0
        var failed = 0
        var canceled = 0
        for result in manifest.entryResults {
            switch result.status {
            case .completed:
                break
            case .skippedIdentical:
                skipped += 1
            case .renamed:
                renamed += 1
            case .failed:
                failed += 1
            case .canceled:
                canceled += 1
            }
        }
        var parts: [String] = []
        if skipped > 0 {
            parts.append(AppText.value("\(skipped) already present", "\(skipped) 个已存在", language: language))
        }
        if renamed > 0 {
            parts.append(AppText.value("\(renamed) renamed", "\(renamed) 个已重命名", language: language))
        }
        if failed > 0 {
            parts.append(AppText.value("\(failed) failed", "\(failed) 个失败", language: language))
        }
        if canceled > 0 {
            parts.append(AppText.value("\(canceled) canceled", "\(canceled) 个已取消", language: language))
        }
        return parts.isEmpty ? nil : parts.joined(separator: " · ")
    }

    @ViewBuilder
    private func receiveDestinationDetail(
        _ record: FfiTransferActivityRecord,
        manifest: FfiManifestActivityRecord?
    ) -> some View {
        Divider().overlay(Theme.line.opacity(0.6))
        let urls = completedReceiveURLs(record, manifest: manifest)
        if record.state == .completed, let firstURL = urls.first {
            let hasMultipleItems = urls.count > 1
            let destinationURL = manifest == nil
                ? firstURL.deletingLastPathComponent()
                : URL(fileURLWithPath: record.completedFilePath, isDirectory: true)
            VStack(alignment: .leading, spacing: 8) {
                Label(
                    hasMultipleItems
                        ? AppText.value("Saved items", "已保存项目", language: language)
                        : AppText.value("Saved item", "已保存项目", language: language),
                    systemImage: "checkmark.circle.fill"
                )
                    .font(.callout.weight(.semibold))
                    .foregroundStyle(Theme.success)
                Text(hasMultipleItems
                     ? AppText.value(
                        "\(urls.count) items",
                        "\(urls.count) 个项目",
                        language: language
                     )
                     : firstURL.lastPathComponent)
                    .font(.body.weight(.semibold))
                    .foregroundStyle(Theme.text)
                    .lineLimit(2)
                Text(AppText.value(
                    "Saved to \(destinationURL.lastPathComponent)",
                    "保存到 \(destinationURL.lastPathComponent)",
                    language: language
                ))
                .font(.footnote)
                .foregroundStyle(Theme.muted)

                #if os(macOS)
                Button(platformRevealTitle(language: language)) { revealInFinder(urls) }
                    .buttonStyle(.bordered)
                    .accessibilityIdentifier("activity_open_received_\(record.activityId)")
                #elseif os(iOS)
                if urls.count == 1, isRegularFileURL(firstURL) {
                    Button(platformRevealTitle(language: language)) { previewFileURL = firstURL }
                    .buttonStyle(.bordered)
                    .accessibilityIdentifier("activity_open_received_\(record.activityId)")
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
                    .buttonStyle(.bordered)
                    .accessibilityIdentifier("activity_open_received_\(record.activityId)")
                }
                #endif

                if developerMode {
                    ForEach(urls, id: \.path) { url in
                        Text(url.path)
                            .font(.caption.monospaced())
                            .foregroundStyle(Theme.muted)
                            .textSelection(.enabled)
                    }
                }
            }
        } else if record.state == .completed {
            Label(
                manifest == nil
                    ? AppText.value(
                        "Transfer confirmed, but the file is not currently available in the selected folder.",
                        "传输已确认，但当前在所选文件夹中找不到该文件。",
                        language: language
                    )
                    : AppText.value(
                        "Transfer confirmed, but the received items are not currently available in the selected folder.",
                        "传输已确认，但当前在所选文件夹中找不到接收的项目。",
                        language: language
                    ),
                systemImage: "exclamationmark.folder"
            )
            .font(.footnote)
            .foregroundStyle(Theme.warning)
            .fixedSize(horizontal: false, vertical: true)
        } else if !isTerminal(record) {
            Label(
                manifest == nil
                    ? AppText.value(
                        "The file appears in Files after transfer and verification finish.",
                        "传输及校验完成后，文件才会出现在“文件”中。",
                        language: language
                    )
                    : AppText.value(
                        "The items appear in Files after the full transfer and verification finish.",
                        "全部传输及校验完成后，项目才会出现在“文件”中。",
                        language: language
                    ),
                systemImage: record.state == .verifying ? "checkmark.shield" : "arrow.down.doc"
            )
            .font(.footnote)
            .foregroundStyle(Theme.muted)
            .fixedSize(horizontal: false, vertical: true)
        }
    }

    private func completedReceiveURLs(
        _ record: FfiTransferActivityRecord,
        manifest: FfiManifestActivityRecord?
    ) -> [URL] {
        if let manifest {
            return availableCompletedManifestItemURLs(record: manifest)
        }
        guard let url = availableCompletedFileURL(
            path: record.completedFilePath,
            expectedBytes: record.bytesTransferred
        ) else { return [] }
        return [url]
    }

    private func uploadStatusText(_ status: UploadStatus) -> String {
        switch status {
        case .uploading:
            return AppText.value("Uploading diagnostic log…", "正在上传诊断日志…", language: language)
        case .uploaded:
            return AppText.value("Diagnostic log uploaded", "诊断日志已上传", language: language)
        case let .failed(detail):
            return AppText.value(
                "Diagnostic log upload failed: \(detail)",
                "诊断日志上传失败：\(detail)",
                language: language
            )
        }
    }

    private func uploadStatusColor(_ status: UploadStatus) -> Color {
        switch status {
        case .uploading: return Theme.muted
        case .uploaded: return Theme.success
        case .failed: return Theme.danger
        }
    }

    private func detailRow(_ label: String, _ value: String) -> some View {
        HStack(alignment: .firstTextBaseline, spacing: 12) {
            Text(label)
                .font(.caption)
                .foregroundStyle(Theme.muted)
            Spacer(minLength: 8)
            Text(value)
                .font(.caption.monospacedDigit())
                .foregroundStyle(Theme.text)
                .lineLimit(1)
                .truncationMode(.middle)
                .textSelection(.enabled)
        }
    }

    private func speedSummary(_ metrics: ActivityMetrics) -> String {
        [
            metrics.avgBps > 0 ? "avg \(rateString(metrics.avgBps))" : nil,
            metrics.peakBps > 0 ? "peak \(rateString(metrics.peakBps))" : nil
        ].compactMap { $0 }.joined(separator: " · ")
    }

    private func activityTitle(for record: FfiTransferActivityRecord) -> String {
        if !record.fileName.isEmpty { return record.fileName }
        return directionText(record.direction)
    }

    private func activitySubtitle(for record: FfiTransferActivityRecord) -> String {
        if (record.state == .failed || record.state == .publishing && record.retryable)
            && !record.diagnosticMessage.isEmpty {
            if record.failureCode != .unknown {
                return friendlyFailure(
                    code: record.failureCode,
                    diagnosticMessage: record.diagnosticMessage,
                    language: language
                )
            }
            return friendlyError(record.diagnosticMessage, language: language)
        }
        return modeText(record.mode)
    }

    private func recoveryText(for record: FfiTransferActivityRecord) -> String? {
        guard record.state == .failed || record.state == .publishing && record.retryable else { return nil }
        switch record.recoveryAction {
        case .retry:
            return AppText.value("Try again when both devices are online.", "请确认两台设备在线后重试。", language: language)
        case .resume:
            return AppText.value("Retry may resume from saved partial progress.", "重试时可能会从已保存的部分进度继续。", language: language)
        case .chooseFolder:
            return AppText.value(
                "Choose another folder to save the file already received.",
                "请选择其他文件夹，继续保存已经接收完成的文件。",
                language: language
            )
        case .openSettings:
            return AppText.value("Check local network or Files permission in system settings.", "请在系统设置中检查本地网络或文件权限。", language: language)
        case .rePair:
            return AppText.value("Generate a new Room code or scan the QR code again.", "请重新生成配对码，或重新扫描二维码。", language: language)
        case .updateApp:
            return AppText.value("Update both apps before trying this transfer mode again.", "请更新两端应用后再尝试此传输模式。", language: language)
        case .switchPairingMethod:
            return AppText.value("Switch pairing method and try again.", "请切换配对方式后重试。", language: language)
        case .discardPartial:
            return AppText.value("Discard the partial file before retrying.", "请先丢弃未完成文件，再重新传输。", language: language)
        case .none:
            return record.retryable
                ? AppText.value("This failure may be retryable.", "这个失败可能可以重试。", language: language)
                : nil
        }
    }

    private func activityStateText(for record: FfiTransferActivityRecord) -> String {
        switch record.state {
        case .queued: return AppText.value("Queued", "排队", language: language)
        case .binding: return AppText.value("Preparing", "准备中", language: language)
        case .waitingForPeer: return AppText.value("Waiting", "等待", language: language)
        case .pairing: return AppText.value("Pairing", "配对", language: language)
        case .connecting: return AppText.value("Connecting", "连接", language: language)
        case .transferring: return "\(Int((progressFraction(for: record) * 100).rounded()))%"
        case .verifying: return AppText.value("Verifying", "校验", language: language)
        case .publishing:
            return record.retryable
                ? AppText.value("Save failed", "保存失败", language: language)
                : AppText.value("Saving", "保存中", language: language)
        case .unconfirmed: return AppText.value("Confirming", "确认中", language: language)
        case .completed:
            if isFullyResumedCompletion(record) {
                return record.direction == .send
                    ? AppText.value("Already received", "对方已有", language: language)
                    : AppText.value("Already present", "文件已存在", language: language)
            }
            return AppText.value("Done", "完成", language: language)
        case .failed: return AppText.value("Error", "错误", language: language)
        case .paused: return AppText.value("Paused", "已暂停", language: language)
        case .canceled: return AppText.value("Canceled", "取消", language: language)
        case .unknown: return AppText.value("Unknown", "未知", language: language)
        }
    }

    private func activityIcon(for record: FfiTransferActivityRecord) -> String {
        switch record.state {
        case .completed: return "checkmark.circle.fill"
        case .failed: return "exclamationmark.triangle.fill"
        case .paused: return "pause.circle"
        case .canceled: return "xmark.circle"
        default:
            return record.direction == .receive ? "tray.and.arrow.down" : "paperplane"
        }
    }

    private func activityTint(for record: FfiTransferActivityRecord) -> Color {
        switch record.state {
        case .completed: return Theme.success
        case .failed: return Theme.danger
        case .paused: return Theme.warning
        case .canceled, .unknown: return Theme.muted
        default: return Theme.warning
        }
    }

    private func directionText(_ direction: FfiTransferDirection) -> String {
        switch direction {
        case .send: return AppText.value("Send", "发送", language: language)
        case .receive: return AppText.value("Receive", "接收", language: language)
        case .unknown: return AppText.value("Transfer", "传输", language: language)
        }
    }

    private func modeText(_ mode: FfiTransferMode) -> String {
        switch mode {
        case .manual: return "Manual"
        case .invite, .showInvite: return "Invite"
        case .showManual: return "Manual"
        case .mdns: return "mDNS"
        case .room: return "Room"
        case .unknown: return AppText.value("Mode", "模式", language: language)
        }
    }

    private func dataPathText(_ record: FfiTransferActivityRecord) -> String {
        let pathKind: String
        switch record.dataPathKind {
        case .direct: pathKind = AppText.value("Direct", "直连", language: language)
        case .relay: pathKind = AppText.value("Relay", "中继", language: language)
        case .other: pathKind = AppText.value("Path", "路径", language: language)
        case .none: return ""
        }
        guard developerMode, !record.dataPathDetail.isEmpty else { return pathKind }
        return "\(pathKind) · \(record.dataPathDetail)"
    }
}

private struct SpeedSparkline: View {
    let history: [Double]
    let averageBps: Double

    var body: some View {
        Canvas { context, size in
            let values = Array(history.suffix(90)).filter { $0 >= 0 }
            guard values.count >= 2 else { return }
            let maxValue = max(values.max() ?? 1, 1)
            let average = min(max(averageBps, 0), maxValue)
            let width = size.width
            let height = size.height

            func point(_ index: Int, _ value: Double) -> CGPoint {
                let x = width * CGFloat(index) / CGFloat(values.count - 1)
                let y = height - height * CGFloat(value / maxValue)
                return CGPoint(x: x, y: y)
            }

            var line = Path()
            var area = Path()
            area.move(to: CGPoint(x: 0, y: height))
            for (index, value) in values.enumerated() {
                let p = point(index, value)
                if index == 0 {
                    line.move(to: p)
                } else {
                    line.addLine(to: p)
                }
                area.addLine(to: p)
            }
            area.addLine(to: CGPoint(x: width, y: height))
            area.closeSubpath()

            context.fill(area, with: .color(Theme.accent.opacity(0.14)))
            context.stroke(line, with: .color(Theme.accent), style: StrokeStyle(lineWidth: 2.2, lineCap: .round, lineJoin: .round))

            if average > 0 {
                var avgLine = Path()
                let y = height - height * CGFloat(average / maxValue)
                avgLine.move(to: CGPoint(x: 0, y: y))
                avgLine.addLine(to: CGPoint(x: width, y: y))
                context.stroke(avgLine, with: .color(Theme.muted.opacity(0.55)), style: StrokeStyle(lineWidth: 1, dash: [5, 5]))
            }
        }
        .frame(height: 50)
    }
}
