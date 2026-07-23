import SwiftUI

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
    let onDelete: (String) -> Bool

    @Environment(\.appLanguage) private var language

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
                    ForEach(records) { record in
                        activityCard(record)
                    }
                }
            }
            .padding(.vertical, 4)
        }
    }

    private func activityCard(_ record: TransferActivityRecord) -> some View {
        let actions = activityActionAvailability(for: record)
        let metrics = metricsByActivityID[record.activityId] ?? ActivityMetrics()
        return VStack(alignment: .leading, spacing: 12) {
            HStack(alignment: .top) {
                Image(systemName: icon(for: record.state))
                    .font(.title2)
                    .foregroundStyle(tint(for: record.state))
                VStack(alignment: .leading, spacing: 3) {
                    Text(title(for: record))
                        .font(.headline)
                        .foregroundStyle(Theme.text)
                    Text(stateText(record.state))
                        .font(.subheadline)
                        .foregroundStyle(Theme.muted)
                }
                Spacer()
                Text(record.direction == .send
                    ? AppText.value("Send", "发送", language: language)
                    : AppText.value("Receive", "接收", language: language))
                    .font(.caption.weight(.semibold))
                    .foregroundStyle(Theme.muted)
            }

            if record.totalBytes > 0 {
                ProgressView(
                    value: Double(record.bytesTransferred),
                    total: Double(record.totalBytes)
                )
                HStack {
                    Text("\(byteString(record.bytesTransferred)) / \(byteString(record.totalBytes))")
                    Spacer()
                    if metrics.speedBps > 0 { Text(rateString(metrics.speedBps)) }
                    if let eta = metrics.etaSeconds { Text(etaString(eta)) }
                }
                .font(.caption.monospacedDigit())
                .foregroundStyle(Theme.muted)
            }

            if !record.diagnosticMessage.isEmpty {
                Text(record.diagnosticMessage)
                    .font(.footnote)
                    .foregroundStyle(record.state == .failed ? Theme.danger : Theme.muted)
                    .textSelection(.enabled)
            }

            if record.state == .delivered, !record.savedPaths.isEmpty {
                VStack(alignment: .leading, spacing: 5) {
                    ForEach(record.savedPaths.prefix(8), id: \.self) { path in
                        Text(path)
                            .font(.caption.monospaced())
                            .lineLimit(1)
                            .textSelection(.enabled)
                    }
                }
            }

            HStack(spacing: 8) {
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
                Menu {
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
                                    server: UserDefaults.standard.string(forKey: "envoix.logServer") ?? defaultLogServer,
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
                    if actions.canDelete {
                        Button(AppText.value("Delete", "删除", language: language), role: .destructive) {
                            _ = onDelete(record.activityId)
                        }
                    }
                } label: {
                    Image(systemName: "ellipsis.circle")
                }
                .disabled(pendingRemovalIDs.contains(record.activityId))
            }
            .buttonStyle(.bordered)
        }
        .card(padding: 14)
    }

    private func title(for record: TransferActivityRecord) -> String {
        let count = Int(record.itemCount)
        if count == 1 { return AppText.value("1 item", "1 个项目", language: language) }
        return AppText.value("\(count) items", "\(count) 个项目", language: language)
    }

    private func stateText(_ state: TransferActivityState) -> String {
        switch state {
        case .preparing: return AppText.value("Preparing locally", "正在本地准备", language: language)
        case .waitingForPeer: return AppText.value("Waiting for peer", "正在等待对端", language: language)
        case .pairing: return AppText.value("Pairing", "正在配对", language: language)
        case .connecting: return AppText.value("Connecting", "正在连接", language: language)
        case .transferring: return AppText.value("Transferring", "正在传输", language: language)
        case .verifying: return AppText.value("Verifying", "正在校验", language: language)
        case .saving: return AppText.value("Saving to destination", "正在保存到目标位置", language: language)
        case .waitingForReceiverSave: return AppText.value("Waiting for receiver to save", "等待接收方完成保存", language: language)
        case .finalizingDelivery: return AppText.value("Saved; finalizing delivery", "已保存，正在完成交付确认", language: language)
        case .paused: return AppText.value("Paused", "已暂停", language: language)
        case .delivered: return AppText.value("Delivered", "已送达", language: language)
        case .failed: return AppText.value("Failed", "失败", language: language)
        case .canceled: return AppText.value("Canceled", "已取消", language: language)
        }
    }

    private func icon(for state: TransferActivityState) -> String {
        switch state {
        case .delivered: return "checkmark.circle.fill"
        case .failed: return "exclamationmark.triangle.fill"
        case .canceled: return "xmark.circle.fill"
        case .paused: return "pause.circle.fill"
        case .saving, .waitingForReceiverSave, .finalizingDelivery: return "internaldrive.fill"
        default: return "arrow.up.arrow.down.circle.fill"
        }
    }

    private func tint(for state: TransferActivityState) -> Color {
        switch state {
        case .delivered: return Theme.success
        case .failed: return Theme.danger
        case .canceled: return Theme.muted
        default: return Theme.accentStrong
        }
    }
}
