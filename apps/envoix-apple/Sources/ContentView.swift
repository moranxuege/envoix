import SwiftUI

#if os(macOS)
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
        case .send: return AppText.localized("home.send.title", language: language)
        case .receive: return AppText.localized("home.receive.title", language: language)
        }
    }

    var icon: String {
        switch self {
        case .send: return "paperplane"
        case .receive: return "tray.and.arrow.down"
        }
    }
}
#endif

struct ContentView: View {
    @EnvironmentObject private var model: AppModel
    #if os(iOS)
    @Environment(\.scenePhase) private var scenePhase
    #endif
    @AppStorage("envoix.appearance") private var appearance: Appearance = .system
    @AppStorage("envoix.language") private var language = "en"
    #if os(macOS)
    @State private var stage: AppStage = .transfer
    #endif

    var body: some View {
        ZStack {
            Theme.bg.ignoresSafeArea()
            #if os(iOS)
            MobileConnectionFlowView()
            #else
            desktopContent
            #endif

            #if os(iOS)
            if scenePhase != .active {
                Theme.bg.ignoresSafeArea()
            }
            #endif
        }
        .toastHost()
        .preferredColorScheme(appearance.colorScheme)
    }

    #if os(macOS)
    private let primaryStages: [AppStage] = [.transfer, .activity]

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
                    badge: item == .activity ? pendingTransferCount : 0
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
                .strokeBorder(
                    stage == .settings ? Theme.accent.opacity(0.45) : Theme.line.opacity(0.5),
                    lineWidth: 0.8
                )
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

    @ViewBuilder
    private var stageContent: some View {
        switch stage {
        case .transfer:
            TransferSetupStageView(send: model.send, receive: model.receive) {
                stage = .activity
            }
        case .activity:
            TransferStageView(
                records: model.activities,
                pendingRemovalIDs: model.pendingActivityRemovalIDs,
                metricsByActivityID: model.activityMetrics,
                onCopyDiagnostics: model.diagnosticReport,
                onRemoteLogTarget: model.remoteLogTarget,
                onRemoteDiagnosticReport: model.remoteDiagnosticReport,
                onAppDiagnosticReport: model.appDiagnosticReport,
                onPause: model.pauseActivity,
                onCanResume: model.canResumeActivity,
                onResume: model.resumeActivity,
                onCancel: model.cancelActivity,
                onApprove: model.approveActivity,
                onDelete: model.removeActivity
            )
        case .settings:
            SettingsStageView()
        }
    }

    private var stageTitle: String {
        switch stage {
        case .transfer: return AppText.value("Send or Receive", "发送或接收", language: language)
        case .activity: return AppText.value("Activity", "活动", language: language)
        case .settings: return AppText.value("Settings", "设置", language: language)
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
                return AppText.value(
                    "\(pendingTransferCount) pending",
                    "\(pendingTransferCount) 个待处理",
                    language: language
                )
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

    private var pendingTransferCount: Int {
        ActivityProjectionPolicy.pendingCount(model.activities)
    }

    private var hasFailedTransfer: Bool {
        model.activities.contains(where: { $0.state == .failed })
            || isFailed(model.receive)
            || isFailed(model.send)
    }

    private func isFailed(_ viewModel: TransferViewModel) -> Bool {
        viewModel.presentationState == .failed
    }
    #endif
}

#if os(macOS)
private struct TransferSetupStageView: View {
    @Environment(\.appLanguage) private var language
    @AppStorage("envoix.defaultRole") private var defaultRole = "send"
    @State private var role: TransferRole = .send
    @State private var didApplyDefaultRole = false
    @State private var preservedSendSelection = SendSelectionSnapshot()
    @State private var pendingSendPairingInput: String?
    @State private var pendingReceivePairingInput: String?
    @ObservedObject var send: TransferViewModel
    @ObservedObject var receive: TransferViewModel
    let onShowActivity: () -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            if showsActivityShortcut {
                Button(action: onShowActivity) {
                    Label(
                        AppText.value("View Activity", "查看活动", language: language),
                        systemImage: "list.bullet.rectangle"
                    )
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
                    SendView(
                        viewModel: send,
                        initialFiles: preservedSendSelection.items,
                        initialFileAccess: preservedSendSelection.sourceAccess,
                        initialPendingSelectionID: preservedSendSelection.pendingSelectionID,
                        initialPairingInput: pendingSendPairingInput,
                        onInitialPairingInputConsumed: { pendingSendPairingInput = nil },
                        onSwitchToReceive: { input, selection in
                            preservedSendSelection = selection
                            pendingReceivePairingInput = input
                            role = .receive
                        }
                    )
                case .receive:
                    ReceiveView(
                        viewModel: receive,
                        initialPairingInput: pendingReceivePairingInput,
                        onInitialPairingInputConsumed: { pendingReceivePairingInput = nil },
                        onSwitchToSend: { input in
                            pendingSendPairingInput = input
                            role = .send
                        }
                    )
                }
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
        }
        .onAppear(perform: applyDefaultRoleOnce)
        .onChange(of: send.transferActivity?.activityId) { activityID in
            if activityID != nil {
                preservedSendSelection = SendSelectionSnapshot()
            }
        }
    }

    private var rolePicker: some View {
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
                        .strokeBorder(
                            role == item ? Theme.accent.opacity(0.45) : Color.clear,
                            lineWidth: 0.8
                        )
                )
                .accessibilityIdentifier("transfer_role_\(item.rawValue)")
            }
        }
        .padding(4)
        .background(Theme.line.opacity(0.35), in: RoundedRectangle(cornerRadius: Theme.cardRadius))
    }

    private func applyDefaultRoleOnce() {
        guard !didApplyDefaultRole else { return }
        didApplyDefaultRole = true
        role = TransferRole(rawValue: defaultRole) ?? .send
    }

    private var showsActivityShortcut: Bool {
        send.presentationState != nil || receive.presentationState != nil
    }
}
#endif
