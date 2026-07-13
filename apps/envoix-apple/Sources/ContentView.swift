import SwiftUI
import EnvoixCore
#if os(iOS)
import UniformTypeIdentifiers
import UIKit
#endif

private enum AppStage: String, CaseIterable {
    case transfer, activity, settings

    func title(language: String) -> String {
        switch self {
        case .transfer: return AppText.value("Transfer", "传输", language: language)
        case .activity: return AppText.value("Activity", "活动", language: language)
        case .settings: return AppText.value("Settings", "设置", language: language)
        }
    }

    var icon: String {
        switch self {
        case .transfer: return "arrow.up.arrow.down"
        case .activity: return "list.bullet.rectangle"
        case .settings: return "gearshape"
        }
    }
}

private enum TransferRole: String, CaseIterable {
    case send, receive

    func title(language: String) -> String {
        switch self {
        case .send: return AppText.value("Send", "发送", language: language)
        case .receive: return AppText.value("Receive", "接收", language: language)
        }
    }

    var icon: String {
        switch self {
        case .send: return "paperplane"
        case .receive: return "tray.and.arrow.down"
        }
    }
}

struct ContentView: View {
    @EnvironmentObject private var model: AppModel
    @AppStorage("envoix.appearance") private var appearance: Appearance = .system
    @AppStorage("envoix.language") private var language = "en"
    @State private var stage: AppStage = {
        #if DEBUG
        if ProcessInfo.processInfo.arguments.contains("--ui-testing-start-activity") {
            return .activity
        }
        #endif
        return .transfer
    }()
    @Namespace private var mobileStageSelection

    private let primaryStages: [AppStage] = [.transfer, .activity]

    var body: some View {
        ZStack {
            Theme.bg.ignoresSafeArea()

            #if os(iOS)
            mobileContent
            #else
            desktopContent
            #endif
        }
        .toastHost()
        .preferredColorScheme(appearance.colorScheme)
    }

    private var desktopContent: some View {
            HStack(spacing: 0) {
                stageRail

                VStack(alignment: .leading, spacing: 0) {
                    desktopToolbar

                    stageContent
                        .frame(maxWidth: .infinity, maxHeight: .infinity)
                }
                .padding(24)
            }
            .frame(minWidth: 760, idealWidth: 920, minHeight: 620, idealHeight: 680)
            .background(Theme.surface)
    }

    #if os(iOS)
    private var mobileContent: some View {
        TabView(selection: $stage) {
            ForEach(AppStage.allCases, id: \.self) { item in
                NavigationStack {
                    stageContent(for: item)
                        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
                        .padding(.horizontal, 16)
                        .background(Theme.bg)
                        .navigationTitle(item.title(language: language))
                        .navigationBarTitleDisplayMode(.inline)
                }
                .toolbar(.hidden, for: .tabBar)
                .tabItem {
                    Label(item.title(language: language), systemImage: item.icon)
                }
                .tag(item)
                .badge(item == .activity ? pendingTransferCount : 0)
            }
        }
        .safeAreaInset(edge: .bottom, spacing: 0) {
            mobileStageBar
        }
        .simultaneousGesture(
            DragGesture(minimumDistance: 32)
                .onEnded { value in switchStage(for: value) }
        )
    }

    private var mobileStageBar: some View {
        HStack(spacing: 4) {
            ForEach(AppStage.allCases, id: \.self) { item in
                Button {
                    withAnimation(.easeInOut(duration: 0.24)) {
                        stage = item
                    }
                } label: {
                    ZStack {
                        if stage == item {
                            RoundedRectangle(cornerRadius: 21, style: .continuous)
                                .fill(Theme.accentSoft.opacity(0.94))
                                .matchedGeometryEffect(id: "mobile-stage-selection", in: mobileStageSelection)
                        }

                        VStack(spacing: 3) {
                            ZStack(alignment: .topTrailing) {
                                Image(systemName: item.icon)
                                    .font(.system(size: 19, weight: .semibold))
                                if item == .activity && pendingTransferCount > 0 {
                                    Text("\(min(pendingTransferCount, 99))")
                                        .font(.system(size: 9, weight: .bold, design: .rounded))
                                        .foregroundStyle(.white)
                                        .padding(.horizontal, 4)
                                        .frame(minWidth: 17, minHeight: 17)
                                        .background(Theme.danger, in: Capsule())
                                        .offset(x: 11, y: -8)
                                }
                            }
                            Text(item.title(language: language))
                                .font(.caption.weight(stage == item ? .bold : .semibold))
                                .lineLimit(1)
                                .minimumScaleFactor(0.8)
                        }
                        .foregroundStyle(stage == item ? Theme.accentStrong : Theme.muted)
                        .padding(.horizontal, 4)
                        .padding(.vertical, 8)
                    }
                    .frame(maxWidth: .infinity)
                    .frame(height: 58)
                    .contentShape(RoundedRectangle(cornerRadius: 21, style: .continuous))
                }
                .buttonStyle(.plain)
                .accessibilityLabel(item.title(language: language))
                .accessibilityIdentifier("stage_\(item.rawValue)")
                .accessibilityAddTraits(stage == item ? .isSelected : [])
            }
        }
        .padding(5)
        .background(.ultraThinMaterial, in: RoundedRectangle(cornerRadius: 26, style: .continuous))
        .overlay(
            RoundedRectangle(cornerRadius: 26, style: .continuous)
                .strokeBorder(Theme.line.opacity(0.72), lineWidth: 0.8)
        )
        .shadow(color: Theme.shadowColor, radius: 12, y: 4)
        .padding(.horizontal, 14)
        .padding(.top, 6)
        .padding(.bottom, 8)
    }

    private func switchStage(for value: DragGesture.Value) {
        let translation = value.translation
        guard abs(translation.width) > abs(translation.height) * 1.4 else { return }

        let edgeWidth: CGFloat = 28
        let screenWidth = UIScreen.main.bounds.width
        let movesFromLeadingEdge = value.startLocation.x <= edgeWidth && translation.width > 0
        let movesFromTrailingEdge = value.startLocation.x >= screenWidth - edgeWidth && translation.width < 0
        let requiredDistance: CGFloat = (movesFromLeadingEdge || movesFromTrailingEdge) ? 90 : 190
        guard abs(translation.width) >= requiredDistance else { return }

        guard let index = AppStage.allCases.firstIndex(of: stage) else { return }
        let offset = translation.width < 0 ? 1 : -1
        let nextIndex = index + offset
        guard AppStage.allCases.indices.contains(nextIndex) else { return }
        withAnimation {
            stage = AppStage.allCases[nextIndex]
        }
    }
    #endif

    private var stageRail: some View {
        VStack(alignment: .leading, spacing: 10) {
            Text("Envoix")
                .font(.title.weight(.semibold))
                .foregroundStyle(Theme.text)
                .padding(.bottom, 22)

            ForEach(primaryStages, id: \.self) { item in
                RailButton(
                    title: item.title(language: language),
                    systemImage: item.icon,
                    isSelected: stage == item,
                    badge: item == .transfer ? pendingTransferCount : 0
                ) {
                    stage = item
                }
            }

            Spacer(minLength: 12)

            settingsEntry
        }
        .padding(22)
        .frame(width: 230)
        .frame(maxHeight: .infinity, alignment: .topLeading)
        .background(Theme.surfaceRaised)
        .overlay(alignment: .trailing) {
            Rectangle()
                .fill(Theme.line)
                .frame(width: 1)
        }
    }

    private var settingsEntry: some View {
        Button {
            stage = .settings
        } label: {
            HStack(spacing: 10) {
                Image(systemName: "gearshape")
                    .font(.title3.weight(.semibold))
                    .frame(width: 24)
                Text(AppText.value("Settings", "设置", language: language))
                    .font(.title3.weight(stage == .settings ? .semibold : .regular))
                Spacer(minLength: 8)
            }
            .padding(.horizontal, 14)
            .frame(maxWidth: .infinity, minHeight: 44, alignment: .leading)
            .contentShape(RoundedRectangle(cornerRadius: Theme.cardRadius))
        }
        .buttonStyle(.plain)
        .foregroundStyle(stage == .settings ? Theme.accentStrong : Theme.muted)
        .background(
            stage == .settings ? Theme.accentSoft : Color.clear,
            in: RoundedRectangle(cornerRadius: Theme.cardRadius)
        )
        .overlay(
            RoundedRectangle(cornerRadius: Theme.cardRadius)
                .strokeBorder(stage == .settings ? Theme.accent.opacity(0.45) : Theme.line.opacity(0.5), lineWidth: 0.8)
        )
        .help(AppText.value("Settings", "设置", language: language))
    }

    private var desktopToolbar: some View {
        HStack(alignment: .top, spacing: 16) {
            VStack(alignment: .leading, spacing: 4) {
                Text("ENVOIX")
                    .font(.title3.weight(.semibold))
                    .foregroundStyle(Theme.accentStrong)
                Text(stageTitle)
                    .font(.largeTitle.bold())
                    .foregroundStyle(Theme.text)
            }
            Spacer(minLength: 16)
            StatusPill(text: headerStatus, systemImage: headerIcon, kind: headerKind)
        }
        .padding(.bottom, 20)
    }

    @ViewBuilder private var stageContent: some View {
        stageContent(for: stage)
    }

    @ViewBuilder private func stageContent(for stage: AppStage) -> some View {
        switch stage {
        case .transfer:
            TransferSetupStageView(send: model.send, receive: model.receive) {
                self.stage = .activity
            }
        case .activity:
            TransferStageView(
                records: model.activities,
                metricsByActivityID: model.activityMetrics,
                onCopyDiagnostics: model.diagnosticReport,
                onRemoteLogTarget: model.remoteLogTarget,
                onRemoteDiagnosticReport: model.remoteDiagnosticReport,
                onAppDiagnosticReport: model.appDiagnosticReport,
                onPause: model.pauseActivity,
                onResume: model.resumeActivity,
                onCancel: model.cancelActivity,
                onDelete: model.removeActivity
            )
        case .settings:
            SettingsStageView()
        }
    }

    private var stageTitle: String {
        switch stage {
        case .transfer:
            return AppText.value("Send or Receive", "发送或接收", language: language)
        case .activity:
            return AppText.value("Activity", "活动", language: language)
        case .settings:
            return AppText.value("Settings", "设置", language: language)
        }
    }

    private var headerStatus: String {
        switch stage {
        case .transfer:
            if model.send.isBusy && model.receive.isBusy {
                return AppText.value("Send and receive active", "发送和接收进行中", language: language)
            }
            if model.send.isBusy {
                return AppText.value("Sending", "正在发送", language: language)
            }
            if model.receive.isBusy {
                return AppText.value("Waiting for sender", "等待发送方", language: language)
            }
            return AppText.value("Ready", "就绪", language: language)
        case .activity:
            if hasFailedTransfer {
                return AppText.value("Needs attention", "需要处理", language: language)
            }
            if pendingTransferCount > 0 {
                return AppText.value("\(pendingTransferCount) pending", "\(pendingTransferCount) 个待处理", language: language)
            }
            return AppText.value("All clear", "无待处理", language: language)
        case .settings:
            return AppText.value("Preferences", "偏好设置", language: language)
        }
    }

    private var headerIcon: String {
        switch stage {
        case .transfer: return "arrow.up.arrow.down"
        case .activity: return "list.bullet.rectangle"
        case .settings: return "gearshape"
        }
    }

    private var headerKind: StatusPill.Kind {
        switch stage {
        case .transfer:
            if isFailed(model.send) || isFailed(model.receive) { return .error }
            return model.isActive ? .warning : .neutral
        case .activity:
            return hasFailedTransfer ? .error : (pendingTransferCount > 0 ? .warning : .neutral)
        case .settings:
            return .neutral
        }
    }

    private func kind(for viewModel: TransferViewModel) -> StatusPill.Kind {
        switch viewModel.phase {
        case .completed: return .success
        case .failed: return .error
        case .waiting, .transferring, .paused: return .warning
        case .idle, .canceled: return .neutral
        }
    }

    private var pendingTransferCount: Int {
        if !model.activities.isEmpty {
            return model.activities.filter { isPending($0) }.count
        }
        return pendingCount(for: model.receive) + pendingCount(for: model.send)
    }

    private var hasFailedTransfer: Bool {
        if model.activities.contains(where: { $0.state == .failed }) {
            return true
        }
        return isFailed(model.receive) || isFailed(model.send)
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

    private func pendingCount(for viewModel: TransferViewModel) -> Int {
        switch viewModel.phase {
        case .waiting, .transferring, .paused:
            return 1
        case .idle, .completed, .canceled, .failed:
            return 0
        }
    }

    private func isFailed(_ viewModel: TransferViewModel) -> Bool {
        if case .failed = viewModel.phase { return true }
        return false
    }
}

private struct TransferSetupStageView: View {
    @Environment(\.appLanguage) private var language
    @AppStorage("envoix.defaultRole") private var defaultRole = "send"
    @State private var role: TransferRole = .send
    @State private var didApplyDefaultRole = false
    @ObservedObject var send: TransferViewModel
    @ObservedObject var receive: TransferViewModel
    let onShowActivity: () -> Void

    var body: some View {
        #if os(iOS)
        mobileBody
        #else
        desktopBody
        #endif
    }

    private var desktopBody: some View {
        VStack(alignment: .leading, spacing: 14) {
            if showsActivityShortcut {
                Button(action: onShowActivity) {
                    Label(AppText.value("View Activity", "查看活动", language: language), systemImage: "list.bullet.rectangle")
                        .font(.body.weight(.semibold))
                        .frame(maxWidth: .infinity, minHeight: 40)
                        .contentShape(Rectangle())
                }
                .buttonStyle(.bordered)
                .controlSize(.regular)
                .accessibilityIdentifier("transfer_view_activity_button")
            }
            rolePicker
            Group {
                switch role {
                case .send:
                    SendView(viewModel: send)
                case .receive:
                    ReceiveView(viewModel: receive)
                }
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
        }
        .onAppear(perform: applyDefaultRoleOnce)
    }

    #if os(iOS)
    private var mobileBody: some View {
        VStack(alignment: .leading, spacing: 14) {
            rolePicker

            Group {
                switch role {
                case .send:
                    SendView(viewModel: send)
                case .receive:
                    ReceiveView(viewModel: receive)
                }
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
        }
        .onAppear(perform: applyDefaultRoleOnce)
        .onChange(of: send.isBusy) { isBusy in
            if isBusy { onShowActivity() }
        }
        .onChange(of: receive.isBusy) { isBusy in
            if isBusy { onShowActivity() }
        }
    }
    #endif

    @ViewBuilder
    private var rolePicker: some View {
        #if os(iOS)
        Picker(AppText.value("Transfer direction", "传输方向", language: language), selection: $role) {
            ForEach(TransferRole.allCases, id: \.self) { item in
                Label(item.title(language: language), systemImage: item.icon)
                    .tag(item)
                    .accessibilityIdentifier("transfer_role_\(item.rawValue)")
            }
        }
        .pickerStyle(.segmented)
        .controlSize(.large)
        .font(.body.weight(.semibold))
        .frame(minHeight: 54)
        .labelsHidden()
        .animation(.easeInOut(duration: 0.22), value: role)
        .simultaneousGesture(
            DragGesture(minimumDistance: 18)
                .onEnded { value in
                    guard abs(value.translation.width) > abs(value.translation.height),
                          abs(value.translation.width) >= 36 else { return }
                    withAnimation(.easeInOut(duration: 0.22)) {
                        role = value.translation.width < 0 ? .receive : .send
                    }
                }
        )
        .accessibilityIdentifier("transfer_role_selector")
        #else
        HStack(spacing: 4) {
            ForEach(TransferRole.allCases, id: \.self) { item in
                Button {
                    role = item
                } label: {
                    Label(item.title(language: language), systemImage: item.icon)
                        .font(.body.weight(.semibold))
                        .frame(maxWidth: .infinity, minHeight: 44)
                        .foregroundStyle(role == item ? Theme.accentStrong : Theme.muted)
                        .contentShape(RoundedRectangle(cornerRadius: Theme.cardRadius))
                }
                .buttonStyle(.plain)
                .background(
                    role == item ? Theme.surface : Color.clear,
                    in: RoundedRectangle(cornerRadius: Theme.cardRadius)
                )
                .overlay(
                    RoundedRectangle(cornerRadius: Theme.cardRadius)
                        .strokeBorder(role == item ? Theme.accent.opacity(0.45) : Color.clear, lineWidth: 0.8)
                )
                .accessibilityIdentifier("transfer_role_\(item.rawValue)")
            }
        }
        .padding(4)
        .background(Theme.line.opacity(0.35), in: RoundedRectangle(cornerRadius: Theme.cardRadius))
        #endif
    }

    private func applyDefaultRoleOnce() {
        guard !didApplyDefaultRole else { return }
        didApplyDefaultRole = true
        role = TransferRole(rawValue: defaultRole) ?? .send
    }

    private var showsActivityShortcut: Bool {
        !isIdle(send) || !isIdle(receive)
    }

    private func isIdle(_ viewModel: TransferViewModel) -> Bool {
        if case .idle = viewModel.phase { return true }
        return false
    }
}

private struct TransferStageView: View {
    private enum UploadStatus {
        case uploading
        case uploaded
        case failed(String)
    }

    private enum ActivityCommand {
        case pause
        case resume
        case cancel
    }

    @Environment(\.appLanguage) private var language
    @AppStorage("envoix.developerMode") private var developerMode = false
    @AppStorage("envoix.logServer") private var logServer = defaultLogServer
    @State private var expandedActivityIDs: Set<String> = []
    @State private var pendingCommands: [String: ActivityCommand] = [:]
    @State private var uploadingActivityIDs: Set<String> = []
    @State private var uploadStatusByActivityID: [String: UploadStatus] = [:]
    @State private var isUploadingAppDiagnostics = false
    @State private var appUploadStatus: UploadStatus?
    let records: [FfiTransferActivityRecord]
    let metricsByActivityID: [String: ActivityMetrics]
    let onCopyDiagnostics: (FfiTransferActivityRecord) -> String
    let onRemoteLogTarget: (FfiTransferActivityRecord) -> RemoteLogUpload.Target?
    let onRemoteDiagnosticReport: (FfiTransferActivityRecord) -> String
    let onAppDiagnosticReport: () -> String
    let onPause: (String) -> Bool
    let onResume: (String) -> Bool
    let onCancel: (String) -> Bool
    let onDelete: (String) -> Void

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
                                if canDelete(record) {
                                    Button(role: .destructive) {
                                        onDelete(record.activityId)
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
                    copyToPasteboard(onAppDiagnosticReport())
                    ToastCenter.shared.show(AppText.value("App diagnostics copied", "应用诊断已复制", language: language))
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
            Text(AppText.value("Start a send or receive from Transfer.", "请从“传输”页面开始发送或接收。", language: language))
                .font(.body)
                .foregroundStyle(Theme.muted)
                .multilineTextAlignment(.center)
        }
        .frame(maxWidth: .infinity, minHeight: 260)
    }

    private func activityCard(_ record: FfiTransferActivityRecord) -> some View {
        let metrics = metrics(for: record)
        let expanded = expandedActivityIDs.contains(record.activityId)
        return VStack(alignment: .leading, spacing: 14) {
            HStack(alignment: .top, spacing: 12) {
                Image(systemName: activityIcon(for: record))
                    .font(.title3.weight(.semibold))
                    .foregroundStyle(activityTint(for: record))
                    .frame(width: 40, height: 40)
                    .background(activityTint(for: record).opacity(0.10), in: Circle())
                VStack(alignment: .leading, spacing: 3) {
                    Text(activityTitle(for: record))
                        .font(.headline.weight(.semibold))
                        .foregroundStyle(Theme.text)
                        .lineLimit(1)
                        .truncationMode(.middle)
                        .layoutPriority(1)
                        .accessibilityIdentifier("activity_title_\(record.activityId)")
                    Text(activitySubtitle(for: record))
                        .font(.subheadline)
                        .foregroundStyle(Theme.muted)
                        .lineLimit(1)
                        .truncationMode(.middle)
                }
                Spacer(minLength: 8)
                ModePill(text: activityStateText(for: record))
                    .fixedSize(horizontal: true, vertical: false)
            }

            activitySummary(record, metrics: metrics)

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
        if record.totalBytes > 0 {
            parts.append("\(byteString(record.bytesTransferred)) / \(byteString(record.totalBytes))")
        }
        if let speed = speedBps(for: record, metrics: metrics), speed > 0 {
            parts.append(rateString(speed))
        }
        return Text(parts.joined(separator: " · "))
            .font(.subheadline.monospacedDigit())
            .foregroundStyle(Theme.muted)
            .lineLimit(1)
            .truncationMode(.middle)
    }

    @ViewBuilder
    private func activityActions(_ record: FfiTransferActivityRecord, expanded: Bool) -> some View {
        HStack(spacing: 10) {
            if let command = pendingCommands[record.activityId] {
                activityCommandIndicator(command)
            } else if canResume(record) {
                activityAction(
                    AppText.value("Resume", "继续", language: language),
                    systemImage: "play.fill",
                    tint: Theme.accent
                ) {
                    requestCommand(.resume, for: record.activityId)
                }
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

            activityAction(
                expanded
                    ? AppText.value("Hide details", "收起详情", language: language)
                    : AppText.value("Details", "查看详情", language: language),
                systemImage: expanded ? "chevron.up" : "chevron.down",
                tint: Theme.accent
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
                    onDelete(record.activityId)
                }
                .accessibilityIdentifier("activity_delete_\(record.activityId)")
            }
        }
    }

    private func activityCommandIndicator(_ command: ActivityCommand) -> some View {
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
        } else {
            ToastCenter.shared.show(AppText.value(
                "This action is no longer available.",
                "当前状态已变化，无法执行此操作。",
                language: language
            ))
        }
    }

    private var activityStateFingerprint: String {
        records.map { "\($0.activityId):\(String(describing: $0.state))" }.joined(separator: "|")
    }

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
                .lineLimit(1)
                .minimumScaleFactor(0.8)
                .frame(maxWidth: .infinity, minHeight: 44)
        }
        .buttonStyle(.bordered)
        .tint(tint)
    }

    private func destructiveActivityAction(
        _ title: String,
        systemImage: String,
        action: @escaping () -> Void
    ) -> some View {
        Button(role: .destructive, action: action) {
            Label(title, systemImage: systemImage)
                .font(.subheadline.weight(.semibold))
                .lineLimit(1)
                .minimumScaleFactor(0.8)
                .frame(maxWidth: .infinity, minHeight: 44)
        }
        .buttonStyle(.bordered)
        .tint(Theme.danger)
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
                detailRow(
                    AppText.value("Transferred", "已传输", language: language),
                    "\(byteString(record.bytesTransferred)) / \(byteString(record.totalBytes))"
                )
                if metrics.avgBps > 0 {
                    detailRow(AppText.value("Average", "平均速度", language: language), rateString(metrics.avgBps))
                }
                if metrics.peakBps > 0 {
                    detailRow(AppText.value("Peak", "峰值速度", language: language), rateString(metrics.peakBps))
                }
                if record.dataPathKind != .none {
                    detailRow(AppText.value("Path", "链路", language: language), dataPathText(record))
                }
            }

            if record.direction == .receive {
                receiveDestinationDetail(record)
            }

            if developerMode {
                Divider().overlay(Theme.line.opacity(0.6))
                VStack(alignment: .leading, spacing: 6) {
                    Text(AppText.value("Developer details", "开发者详情", language: language))
                        .font(.callout.weight(.semibold))
                        .foregroundStyle(Theme.text)
                    detailRow("Activity ID", record.activityId)
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
                            tint: Theme.accent
                        ) {
                            copyToPasteboard(onCopyDiagnostics(record))
                            ToastCenter.shared.show(AppText.value("Diagnostics copied", "诊断信息已复制", language: language))
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
                                tint: Theme.accent
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
                        copyToPasteboard(metrics.log.joined(separator: "\n"))
                        ToastCenter.shared.show(AppText.value("Activity log copied", "活动日志已复制", language: language))
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

    @ViewBuilder
    private func receiveDestinationDetail(_ record: FfiTransferActivityRecord) -> some View {
        Divider().overlay(Theme.line.opacity(0.6))
        if record.state == .completed,
           let url = availableCompletedFileURL(
               path: record.completedFilePath,
               expectedBytes: record.bytesTransferred
           ) {
            VStack(alignment: .leading, spacing: 8) {
                Label(AppText.value("Saved file", "已保存文件", language: language), systemImage: "checkmark.circle.fill")
                    .font(.callout.weight(.semibold))
                    .foregroundStyle(Theme.success)
                Text(url.lastPathComponent)
                    .font(.body.weight(.semibold))
                    .foregroundStyle(Theme.text)
                    .lineLimit(2)
                Text(AppText.value(
                    "Saved to \(url.deletingLastPathComponent().lastPathComponent)",
                    "保存到 \(url.deletingLastPathComponent().lastPathComponent)",
                    language: language
                ))
                .font(.footnote)
                .foregroundStyle(Theme.muted)

                #if os(macOS)
                Button(platformRevealTitle(language: language)) { revealInFinder(url) }
                    .buttonStyle(.bordered)
                #endif

                if developerMode {
                    Text(url.path)
                        .font(.caption.monospaced())
                        .foregroundStyle(Theme.muted)
                        .textSelection(.enabled)
                }
            }
        } else if record.state == .completed {
            Label(
                AppText.value(
                    "Transfer confirmed, but the file is not currently available in the selected folder.",
                    "传输已确认，但当前在所选文件夹中找不到该文件。",
                    language: language
                ),
                systemImage: "exclamationmark.folder"
            )
            .font(.footnote)
            .foregroundStyle(Theme.warning)
            .fixedSize(horizontal: false, vertical: true)
        } else if !isTerminal(record) {
            Label(
                AppText.value(
                    "The file appears in Files after transfer and verification finish.",
                    "传输及校验完成后，文件才会出现在“文件”中。",
                    language: language
                ),
                systemImage: record.state == .verifying ? "checkmark.shield" : "arrow.down.doc"
            )
            .font(.footnote)
            .foregroundStyle(Theme.muted)
            .fixedSize(horizontal: false, vertical: true)
        }
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
        if record.state == .failed && !record.diagnosticMessage.isEmpty {
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
        guard record.state == .failed else { return nil }
        switch record.recoveryAction {
        case .retry:
            return AppText.value("Try again when both devices are online.", "请确认两台设备在线后重试。", language: language)
        case .resume:
            return AppText.value("Retry may resume from saved partial progress.", "重试时可能会从已保存的部分进度继续。", language: language)
        case .chooseFolder:
            return AppText.value("Choose another save folder, then start the receive again.", "请选择其他保存文件夹，然后重新开始接收。", language: language)
        case .openSettings:
            return AppText.value("Check local network or Files permission in system settings.", "请在系统设置中检查本地网络或文件权限。", language: language)
        case .rePair:
            return AppText.value("Generate a new code or scan the QR code again.", "请重新生成短码，或重新扫描二维码。", language: language)
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
        case .publishing: return AppText.value("Saving", "保存中", language: language)
        case .unconfirmed: return AppText.value("Confirming", "确认中", language: language)
        case .completed: return AppText.value("Done", "完成", language: language)
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

private struct SettingsStageView: View {
    @AppStorage("envoix.appearance") private var appearance: Appearance = .system
    @AppStorage("envoix.language") private var language = "en"
    @AppStorage("envoix.defaultRole") private var defaultRole = "send"
    @AppStorage("envoix.serverURL") private var serverURL = ""
    @AppStorage("envoix.relayURL") private var relayURL = ""
    @AppStorage("envoix.configChunkSize") private var configChunkSize = ""
    @AppStorage("envoix.candidatesAllow") private var candidatesAllow = ""
    @AppStorage("envoix.candidatesDeny") private var candidatesDeny = ""
    @AppStorage("envoix.useRoom") private var useRoom = true
    @AppStorage("envoix.useMdns") private var useMdns = true
    @AppStorage("envoix.developerMode") private var developerMode = false
    @AppStorage("envoix.verboseLog") private var verboseLog = false
    @AppStorage("envoix.logServer") private var logServer = defaultLogServer
    @State private var showAdvanced = false

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 14) {
                appearanceSection

                VStack(alignment: .leading, spacing: 8) {
                    Text(AppText.value("Language", "语言", language: language))
                        .font(.title3.weight(.semibold))
                        .foregroundStyle(Theme.muted)
                    Picker("Language", selection: $language) {
                        Text("English").tag("en")
                        Text("简体中文").tag("zh-Hans")
                    }
                    .pickerStyle(.segmented)
                    .labelsHidden()
                }
                .card(padding: 14)

                VStack(alignment: .leading, spacing: 8) {
                    Text(AppText.value("Default role for a new code", "新建短码的默认角色", language: language))
                        .font(.title3.weight(.semibold))
                        .foregroundStyle(Theme.muted)
                    Picker("Default role", selection: $defaultRole) {
                        Text(AppText.value("Send", "发送", language: language)).tag("send")
                        Text(AppText.value("Receive", "接收", language: language)).tag("receive")
                    }
                    .pickerStyle(.segmented)
                    .labelsHidden()
                }
                .card(padding: 14)

                VStack(alignment: .leading, spacing: 8) {
                    Text(AppText.value("Pairing", "配对", language: language))
                        .font(.title3.weight(.semibold))
                        .foregroundStyle(Theme.muted)
                    settingToggle(
                        AppText.value("Avoid Tailscale addresses", "避开 Tailscale 地址", language: language),
                        subtitle: AppText.value("Prefer the real WAN or relay path instead of 100.x candidates.", "不广播 100.x 候选地址，优先使用真实网络或中继。", language: language),
                        isOn: avoidTailscaleBinding
                    )
                    Divider().overlay(Theme.line.opacity(0.5))
                    settingToggle(
                        AppText.value("Internet pairing", "互联网配对", language: language),
                        subtitle: AppText.value("Use the rendezvous broker for Room pairing.", "通过配对服务器建立 Room。", language: language),
                        isOn: $useRoom
                    )
                    Divider().overlay(Theme.line.opacity(0.5))
                    settingToggle(
                        AppText.value("Local Wi-Fi pairing", "本地 Wi‑Fi 配对", language: language),
                        subtitle: AppText.value("Also try mDNS on the same network.", "同时尝试同一网络内的 mDNS。", language: language),
                        isOn: $useMdns
                    )
                }
                .card(padding: 14)

                advancedHeader

                if showAdvanced {
                    settingField(
                        AppText.value("Rendezvous broker", "配对服务器", language: language),
                        text: $serverURL,
                        placeholder: defaultRendezvousBroker,
                        helper: AppText.value("Leave empty to use the built-in Envoix broker.", "留空则使用内置 Envoix 配对服务器。", language: language),
                        isURL: true
                    )
                    settingField(
                        AppText.value("Relay URL", "中继 URL", language: language),
                        text: $relayURL,
                        placeholder: defaultRelayURL,
                        helper: AppText.value("Leave empty to use the built-in relay for Room pairing.", "留空则使用内置中继服务。", language: language),
                        isURL: true
                    )

                    settingField(
                        AppText.value("config.toml · chunk size", "config.toml · 块大小", language: language),
                        text: $configChunkSize,
                        placeholder: AppText.value("16MB / 65536", "16MB / 65536", language: language),
                        helper: AppText.value(
                            "Chunk size override written into runtime config.toml (leave empty to disable).",
                            "可选块大小覆盖，写入 runtime config.toml；留空则不使用。",
                            language: language
                        )
                    )
                    settingMultilineField(
                        AppText.value("Candidate allow", "候选地址 allow", language: language),
                        text: $candidatesAllow,
                        helper: AppText.value("One CIDR per line. Empty means allow all.", "每行一个 CIDR；留空表示全部允许。", language: language)
                    )
                    settingMultilineField(
                        AppText.value("Candidate deny", "候选地址 deny", language: language),
                        text: $candidatesDeny,
                        helper: AppText.value("One CIDR per line. Avoid Tailscale edits this list.", "每行一个 CIDR；避开 Tailscale 会修改此列表。", language: language)
                    )
                }

                VStack(alignment: .leading, spacing: 8) {
                    Text(AppText.value("Developer tools", "开发者工具", language: language))
                        .font(.title3.weight(.semibold))
                        .foregroundStyle(Theme.muted)
                    settingToggle(
                        AppText.value("Enable developer mode", "开启开发者模式", language: language),
                        subtitle: AppText.value(
                            "Reveal path selection, IDs, failure details, live logs and diagnostic reports.",
                            "显示链路选择、ID、失败详情、实时日志和诊断报告。",
                            language: language
                        ),
                        isOn: $developerMode
                    )
                    if developerMode {
                        Divider().overlay(Theme.line.opacity(0.5))
                        settingToggle(
                            AppText.value("Verbose logging", "详细日志", language: language),
                            subtitle: AppText.value(
                                "Capture path selection and hole-punching internals. High volume.",
                                "记录链路选择和打洞内部信息；日志量较大。",
                                language: language
                            ),
                            isOn: $verboseLog
                        )
                        #if DEBUG
                        Divider().overlay(Theme.line.opacity(0.5))
                        VStack(alignment: .leading, spacing: 8) {
                            let title = AppText.value("Remote log server", "远程日志服务器", language: language)
                            settingInput(
                                title: title,
                                text: $logServer,
                                placeholder: defaultLogServer,
                                isURL: true
                            )
                            Text(AppText.value(
                                "Redacted reports only. HTTPS is tried before HTTP fallback.",
                                "只上传脱敏报告；优先 HTTPS，失败后回退 HTTP。",
                                language: language
                            ))
                                .font(.footnote)
                                .foregroundStyle(Theme.muted)
                                .fixedSize(horizontal: false, vertical: true)
                        }
                        #endif
                    }
                }
                .card(padding: 14)

                Text(appDebugBuildLabel)
                    .font(.caption.monospaced())
                    .foregroundStyle(Theme.muted)
                    .frame(maxWidth: .infinity, alignment: .center)
                    .padding(.top, 2)
            }
            .padding(.vertical, 12)
        }
        .onAppear(perform: migrateLogServerIfNeeded)
    }

    private func migrateLogServerIfNeeded() {
        if deprecatedLogServers.contains(logServer.trimmed) {
            logServer = defaultLogServer
        }
    }

    private var appearanceSection: some View {
        VStack(alignment: .leading, spacing: 8) {
            Text(AppText.value("Appearance", "外观", language: language))
                .font(.title3.weight(.semibold))
                .foregroundStyle(Theme.muted)

            Button {
                appearance = appearance.next
            } label: {
                HStack(spacing: 10) {
                    Image(systemName: appearance.icon)
                        .font(.title3.weight(.semibold))
                        .foregroundStyle(Theme.accentStrong)
                        .frame(width: 24)
                    Text(appearanceTitle)
                        .font(.title3.weight(.semibold))
                        .foregroundStyle(Theme.text)
                    Spacer()
                    Text(AppText.value("System / Light / Dark", "跟随系统 / 浅色 / 深色", language: language))
                        .font(.body)
                        .foregroundStyle(Theme.muted)
                }
                .frame(minHeight: 42)
                .contentShape(RoundedRectangle(cornerRadius: Theme.cardRadius))
            }
            .buttonStyle(.plain)
        }
        .card(padding: 14)
    }

    private var appearanceTitle: String {
        switch appearance {
        case .system:
            return AppText.value("System", "跟随系统", language: language)
        case .light:
            return AppText.value("Light", "浅色", language: language)
        case .dark:
            return AppText.value("Dark", "深色", language: language)
        }
    }

    private var advancedHeader: some View {
        Button {
            withAnimation(.easeInOut(duration: 0.16)) {
                showAdvanced.toggle()
            }
        } label: {
            HStack {
                Text(AppText.value("Advanced", "高级", language: language))
                    .font(.headline.weight(.semibold))
                    .foregroundStyle(Theme.text)
                Spacer()
                Image(systemName: showAdvanced ? "chevron.up" : "chevron.down")
                    .foregroundStyle(Theme.muted)
            }
            .frame(maxWidth: .infinity, minHeight: 36, alignment: .leading)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
    }

    private var avoidTailscaleBinding: Binding<Bool> {
        Binding(
            get: {
                let deny = Set(configListLines(candidatesDeny))
                return Self.tailscaleCIDRs.allSatisfy { deny.contains($0) }
            },
            set: { enabled in
                var deny = configListLines(candidatesDeny)
                if enabled {
                    deny = Array(Set(deny).union(Self.tailscaleCIDRs)).sorted()
                } else {
                    deny.removeAll { Self.tailscaleCIDRs.contains($0) }
                }
                candidatesDeny = deny.joined(separator: "\n")
            }
        )
    }

    private func settingField(
        _ title: String,
        text: Binding<String>,
        placeholder: String = "",
        helper: String? = nil,
        isURL: Bool = false
    ) -> some View {
        VStack(alignment: .leading, spacing: 8) {
            Text(title)
                .font(.title3.weight(.semibold))
                .foregroundStyle(Theme.muted)
            settingInput(title: title, text: text, placeholder: placeholder, isURL: isURL)
            if let helper {
                Text(helper)
                    .font(.body)
                    .foregroundStyle(Theme.muted)
                    .fixedSize(horizontal: false, vertical: true)
            }
        }
        .card(padding: 14)
    }

    private func settingMultilineField(
        _ title: String,
        text: Binding<String>,
        helper: String
    ) -> some View {
        VStack(alignment: .leading, spacing: 8) {
            Text(title)
                .font(.title3.weight(.semibold))
                .foregroundStyle(Theme.muted)
            TextEditor(text: text)
                .font(.body.monospaced())
                .foregroundStyle(Theme.text)
                .frame(minHeight: 88)
                .scrollContentBackground(.hidden)
                .padding(.horizontal, 8)
                .padding(.vertical, 6)
                .background(Theme.surface)
                .overlay(
                    RoundedRectangle(cornerRadius: Theme.cardRadius)
                        .strokeBorder(Theme.line.opacity(0.75), lineWidth: 0.8)
                )
                .clipShape(RoundedRectangle(cornerRadius: Theme.cardRadius))
            Text(helper)
                .font(.body)
                .foregroundStyle(Theme.muted)
                .fixedSize(horizontal: false, vertical: true)
        }
        .card(padding: 14)
    }

    @ViewBuilder
    private func settingInput(
        title: String,
        text: Binding<String>,
        placeholder: String,
        isURL: Bool
    ) -> some View {
        let prompt = placeholder.isEmpty ? title : placeholder
        #if os(iOS)
        TextField(prompt, text: text)
            .textFieldStyle(.plain)
            .font(.body.monospaced())
            .foregroundStyle(Theme.text)
            .lineLimit(1)
            .textInputAutocapitalization(.never)
            .autocorrectionDisabled()
            .keyboardType(isURL ? .URL : .default)
            .padding(.horizontal, 10)
            .frame(minHeight: 44)
            .background(Theme.surface)
            .overlay(
                RoundedRectangle(cornerRadius: Theme.cardRadius)
                    .strokeBorder(Theme.line.opacity(0.75), lineWidth: 0.8)
            )
            .clipShape(RoundedRectangle(cornerRadius: Theme.cardRadius))
        #else
        TextField(prompt, text: text)
            .textFieldStyle(.plain)
            .font(.body.monospaced())
            .foregroundStyle(Theme.text)
            .lineLimit(1)
            .padding(.horizontal, 10)
            .frame(minHeight: 44)
            .background(Theme.surface)
            .overlay(
                RoundedRectangle(cornerRadius: Theme.cardRadius)
                    .strokeBorder(Theme.line.opacity(0.75), lineWidth: 0.8)
            )
            .clipShape(RoundedRectangle(cornerRadius: Theme.cardRadius))
        #endif
    }

    private func settingToggle(_ title: String, isOn: Binding<Bool>) -> some View {
        settingToggle(title, subtitle: nil, isOn: isOn)
    }

    private func settingToggle(
        _ title: String,
        subtitle: String?,
        isOn: Binding<Bool>
    ) -> some View {
        Button {
            withAnimation(.easeInOut(duration: 0.15)) {
                isOn.wrappedValue.toggle()
            }
        } label: {
            HStack(spacing: 12) {
                VStack(alignment: .leading, spacing: 3) {
                    Text(title)
                        .font(.title3)
                        .foregroundStyle(Theme.text)
                    if let subtitle {
                        Text(subtitle)
                            .font(.body)
                            .foregroundStyle(Theme.muted)
                            .fixedSize(horizontal: false, vertical: true)
                    }
                }
                Spacer(minLength: 12)
                SettingSwitchIndicator(isOn: isOn.wrappedValue)
            }
            .frame(maxWidth: .infinity, minHeight: 52, alignment: .leading)
            .contentShape(RoundedRectangle(cornerRadius: Theme.cardRadius))
        }
        .buttonStyle(.plain)
        .accessibilityElement(children: .combine)
        .accessibilityLabel(title)
        .accessibilityValue(isOn.wrappedValue ? "On" : "Off")
    }

    private static let tailscaleCIDRs = ["100.64.0.0/10", "fd7a:115c:a1e0::/48"]
}

private struct SettingSwitchIndicator: View {
    let isOn: Bool

    var body: some View {
        RoundedRectangle(cornerRadius: 16)
            .fill(isOn ? Theme.accent : Theme.line)
            .frame(width: 48, height: 28)
            .overlay(alignment: isOn ? .trailing : .leading) {
                Circle()
                    .fill(Color.white)
                    .shadow(color: Color.black.opacity(0.12), radius: 2, y: 1)
                    .frame(width: 24, height: 24)
                    .padding(2)
            }
    }
}
