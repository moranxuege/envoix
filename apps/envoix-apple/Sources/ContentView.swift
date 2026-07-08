import SwiftUI
import EnvoixCore

private enum AppStage: String, CaseIterable {
    case sender, receiver, transfer, settings

    func title(language: String) -> String {
        switch self {
        case .sender: return AppText.value("Sender", "发送", language: language)
        case .receiver: return AppText.value("Receiver", "接收", language: language)
        case .transfer: return AppText.value("Activity", "活动", language: language)
        case .settings: return AppText.value("Settings", "设置", language: language)
        }
    }

    var icon: String {
        switch self {
        case .sender: return "paperplane"
        case .receiver: return "tray.and.arrow.down"
        case .transfer: return "arrow.up.arrow.down"
        case .settings: return "gearshape"
        }
    }
}

struct ContentView: View {
    @EnvironmentObject private var model: AppModel
    @AppStorage("envoix.appearance") private var appearance: Appearance = .system
    @AppStorage("envoix.language") private var language = "en"
    @State private var stage: AppStage = .sender

    private let primaryStages: [AppStage] = [.sender, .receiver, .transfer]

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
                        .background(Theme.surface)
                        .navigationTitle(item.title(language: language))
                        .navigationBarTitleDisplayMode(.inline)
                }
                .tabItem {
                    Label(item.title(language: language), systemImage: item.icon)
                }
                .tag(item)
            }
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
                Text(AppText.value(platformPairingTitle, platformPairingTitleZh, language: language))
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
        case .sender:
            SendView(viewModel: model.send)
        case .receiver:
            ReceiveView(viewModel: model.receive)
        case .transfer:
            TransferStageView(records: model.activities)
        case .settings:
            SettingsStageView()
        }
    }

    private var platformPairingTitle: String {
        #if os(iOS)
        return "iPhone Pairing"
        #else
        return "macOS Pairing"
        #endif
    }

    private var platformPairingTitleZh: String {
        #if os(iOS)
        return "iPhone 配对"
        #else
        return "macOS 配对"
        #endif
    }

    private var stageTitle: String {
        switch stage {
        case .sender:
            return AppText.value("Send a File", "发送文件", language: language)
        case .receiver:
            return AppText.value("Receive a File", "接收文件", language: language)
        case .transfer:
            return AppText.value("Activity", "活动", language: language)
        case .settings:
            return AppText.value("Settings", "设置", language: language)
        }
    }

    private var headerStatus: String {
        switch stage {
        case .sender:
            return model.send.isBusy
                ? AppText.value("Sending", "正在发送", language: language)
                : AppText.value("Ready to send", "可发送", language: language)
        case .receiver:
            return model.receive.isBusy
                ? AppText.value("Waiting for sender", "等待发送方", language: language)
                : AppText.value("Ready to receive", "可接收", language: language)
        case .transfer:
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
        case .sender: return "paperplane"
        case .receiver: return "antenna.radiowaves.left.and.right"
        case .transfer: return "arrow.up.arrow.down"
        case .settings: return "gearshape"
        }
    }

    private var headerKind: StatusPill.Kind {
        switch stage {
        case .sender:
            return kind(for: model.send)
        case .receiver:
            return kind(for: model.receive)
        case .transfer:
            return hasFailedTransfer ? .error : (pendingTransferCount > 0 ? .warning : .neutral)
        case .settings:
            return .neutral
        }
    }

    private func kind(for viewModel: TransferViewModel) -> StatusPill.Kind {
        switch viewModel.phase {
        case .completed: return .success
        case .failed: return .error
        case .waiting, .transferring: return .warning
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
        case .queued, .binding, .waitingForPeer, .pairing, .connecting, .transferring, .verifying:
            return true
        case .completed, .failed, .canceled, .unknown:
            return false
        }
    }

    private func pendingCount(for viewModel: TransferViewModel) -> Int {
        switch viewModel.phase {
        case .waiting, .transferring:
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

private struct TransferStageView: View {
    @Environment(\.appLanguage) private var language
    let records: [FfiTransferActivityRecord]

    var body: some View {
        ScrollView {
            VStack(spacing: 12) {
                overviewCard
                if records.isEmpty {
                    emptyActivityView
                } else {
                    ForEach(records, id: \.activityId) { record in
                        activityCard(record)
                    }
                }
            }
            .padding(.vertical, 12)
        }
    }

    private var overviewCard: some View {
        HStack(spacing: 14) {
            Image(systemName: overviewIcon)
                .font(.system(size: 34, weight: .semibold))
                .foregroundStyle(overviewTint)
                .frame(width: 44)

            VStack(alignment: .leading, spacing: 4) {
                Text(overviewTitle)
                    .font(.title2.weight(.semibold))
                    .foregroundStyle(Theme.text)
                Text(activitySummary)
                    .font(.title3)
                    .foregroundStyle(Theme.muted)
            }

            Spacer(minLength: 8)
        }
        .card(raised: true, padding: 16)
    }

    private var emptyActivityView: some View {
        HStack(spacing: 12) {
            Image(systemName: "tray")
                .font(.title3.weight(.semibold))
                .foregroundStyle(Theme.muted)
                .frame(width: 30)
            Text(AppText.value("Transfers will appear here once started.", "开始传输后会显示在这里。", language: language))
                .font(.title3)
                .foregroundStyle(Theme.muted)
            Spacer(minLength: 8)
        }
        .card(raised: true, padding: 14)
    }

    private func activityCard(_ record: FfiTransferActivityRecord) -> some View {
        VStack(alignment: .leading, spacing: 12) {
            HStack(alignment: .top, spacing: 10) {
                Image(systemName: activityIcon(for: record))
                    .foregroundStyle(activityTint(for: record))
                    .frame(width: 22)
                VStack(alignment: .leading, spacing: 3) {
                    Text(activityTitle(for: record))
                        .font(.title2.weight(.semibold))
                        .foregroundStyle(Theme.text)
                        .lineLimit(1)
                        .truncationMode(.middle)
                    Text(activitySubtitle(for: record))
                        .font(.title3)
                        .foregroundStyle(Theme.muted)
                        .lineLimit(2)
                }
                Spacer(minLength: 8)
                ModePill(text: activityStateText(for: record))
            }

            if record.totalBytes > 0 && !isTerminal(record) {
                ProgressBar(value: progressFraction(for: record))
            }

            HStack(spacing: 8) {
                Text(directionText(record.direction))
                if record.totalBytes > 0 {
                    Text("·")
                    Text("\(byteString(record.bytesTransferred)) / \(byteString(record.totalBytes))")
                }
                if record.dataPathKind != .none {
                    Text("·")
                    Text(dataPathText(record))
                        .lineLimit(1)
                        .truncationMode(.middle)
                }
                Spacer(minLength: 4)
            }
            .font(.body.monospacedDigit())
            .foregroundStyle(Theme.muted)
        }
        .card(raised: true, padding: 14)
    }

    private var pendingCount: Int {
        records.filter { isPending($0) }.count
    }

    private var failedCount: Int {
        records.filter { $0.state == .failed }.count
    }

    private var receivePendingCount: Int {
        records.filter { isPending($0) && $0.direction == .receive }.count
    }

    private var sendPendingCount: Int {
        records.filter { isPending($0) && $0.direction == .send }.count
    }

    private var overviewIcon: String {
        if pendingCount > 0 { return "clock.badge.exclamationmark" }
        if failedCount > 0 { return "exclamationmark.triangle" }
        return "checkmark.circle"
    }

    private var overviewTint: Color {
        if pendingCount > 0 { return Theme.warning }
        if failedCount > 0 { return Theme.danger }
        return Theme.success
    }

    private var overviewTitle: String {
        if pendingCount > 0 {
            return AppText.value(
                "\(pendingCount) pending task\(pendingCount == 1 ? "" : "s")",
                "\(pendingCount) 个待处理任务",
                language: language
            )
        }
        if failedCount > 0 {
            return AppText.value(
                "\(failedCount) item\(failedCount == 1 ? "" : "s") need attention",
                "\(failedCount) 个项目需要处理",
                language: language
            )
        }
        return AppText.value("No pending transfers", "没有待处理传输", language: language)
    }

    private var activitySummary: String {
        if pendingCount == 0 {
            if failedCount > 0 {
                return AppText.value("Review failed transfers below, or start a new operation when ready.", "请查看下方失败的传输，或在准备好后开始新操作。", language: language)
            }
            return AppText.value("Completed transfers stay visible below until the next operation.", "已完成的传输会保留在下方，直到下一次操作。", language: language)
        }
        if receivePendingCount > 0 && sendPendingCount > 0 {
            return AppText.value("Receiving and sending are both in progress.", "接收和发送都在进行中。", language: language)
        }
        if receivePendingCount > 0 {
            return AppText.value("A receive task is currently waiting or transferring.", "当前有一个接收任务正在等待或传输。", language: language)
        }
        if sendPendingCount > 0 {
            return AppText.value("A send task is currently transferring.", "当前有一个发送任务正在传输。", language: language)
        }
        return AppText.value("Review failed tasks below before starting another transfer.", "开始新的传输前，请先查看下方失败任务。", language: language)
    }

    private func isPending(_ record: FfiTransferActivityRecord) -> Bool {
        switch record.state {
        case .queued, .binding, .waitingForPeer, .pairing, .connecting, .transferring, .verifying:
            return true
        case .completed, .failed, .canceled, .unknown:
            return false
        }
    }

    private func isTerminal(_ record: FfiTransferActivityRecord) -> Bool {
        switch record.state {
        case .completed, .failed, .canceled:
            return true
        case .queued, .binding, .waitingForPeer, .pairing, .connecting, .transferring, .verifying, .unknown:
            return false
        }
    }

    private func progressFraction(for record: FfiTransferActivityRecord) -> Double {
        guard record.totalBytes > 0 else { return 0 }
        return min(1, Double(record.bytesTransferred) / Double(record.totalBytes))
    }

    private func activityTitle(for record: FfiTransferActivityRecord) -> String {
        if !record.fileName.isEmpty { return record.fileName }
        return directionText(record.direction)
    }

    private func activitySubtitle(for record: FfiTransferActivityRecord) -> String {
        if record.state == .failed && !record.diagnosticMessage.isEmpty {
            return friendlyError(record.diagnosticMessage, language: language)
        }
        return "\(modeText(record.mode)) · \(activityStateText(for: record))"
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
        case .completed: return AppText.value("Done", "完成", language: language)
        case .failed: return AppText.value("Error", "错误", language: language)
        case .canceled: return AppText.value("Canceled", "取消", language: language)
        case .unknown: return AppText.value("Unknown", "未知", language: language)
        }
    }

    private func activityIcon(for record: FfiTransferActivityRecord) -> String {
        switch record.state {
        case .completed: return "checkmark.circle.fill"
        case .failed: return "exclamationmark.triangle.fill"
        case .canceled: return "xmark.circle"
        default:
            return record.direction == .receive ? "tray.and.arrow.down" : "paperplane"
        }
    }

    private func activityTint(for record: FfiTransferActivityRecord) -> Color {
        switch record.state {
        case .completed: return Theme.success
        case .failed: return Theme.danger
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
        switch record.dataPathKind {
        case .direct: return AppText.value("Direct", "直连", language: language)
        case .relay: return AppText.value("Relay", "中继", language: language)
        case .other: return record.dataPathDetail.isEmpty ? AppText.value("Path", "路径", language: language) : record.dataPathDetail
        case .none: return ""
        }
    }
}

private struct SettingsStageView: View {
    @AppStorage("envoix.appearance") private var appearance: Appearance = .system
    @AppStorage("envoix.concurrentTransfers") private var concurrentTransfers = true
    @AppStorage("envoix.language") private var language = "en"
    @AppStorage("envoix.serverURL") private var serverURL = ""
    @AppStorage("envoix.relayURL") private var relayURL = ""
    @AppStorage("envoix.configChunkSize") private var configChunkSize = ""
    @AppStorage("envoix.developerMode") private var developerMode = false
    @AppStorage("envoix.verboseLog") private var verboseLog = false
    @AppStorage("envoix.speedLimit") private var speedLimit = 40

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 14) {
                Button {
                    concurrentTransfers.toggle()
                } label: {
                    HStack {
                        Text(AppText.value("Allow simultaneous send and receive", "允许同时发送和接收", language: language))
                            .font(.title3)
                        Spacer()
                        Text(concurrentTransfers
                             ? AppText.value("On", "开启", language: language)
                             : AppText.value("Off", "关闭", language: language))
                            .fontWeight(.bold)
                            .foregroundStyle(Theme.accentStrong)
                    }
                    .frame(minHeight: 42)
                }
                .buttonStyle(.plain)
                .card(raised: true, padding: 14)

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
                    Text(AppText.value("Developer mode", "开发者模式", language: language))
                        .font(.title3.weight(.semibold))
                        .foregroundStyle(Theme.muted)
                    Toggle(AppText.value("Enable developer mode", "开启开发者模式", language: language), isOn: $developerMode)
                        .toggleStyle(.switch)
                    if developerMode {
                        Toggle(
                            AppText.value("Verbose logging", "详细日志", language: language),
                            isOn: $verboseLog
                        )
                        .toggleStyle(.switch)
                        Text(AppText.value("Verbose logging is currently UI-only for Activity logs.", "详细日志目前仅用于活动日志展示。", language: language))
                            .font(.body)
                            .foregroundStyle(Theme.muted)
                    }
                }
                .padding(.horizontal, 14)
                .padding(.vertical, 12)
                .frame(maxWidth: .infinity, alignment: .leading)
                .background(Theme.surface)
                .overlay(
                    RoundedRectangle(cornerRadius: Theme.cardRadius)
                        .strokeBorder(Theme.line.opacity(0.75), lineWidth: 0.8)
                )
                .clipShape(RoundedRectangle(cornerRadius: Theme.cardRadius))

                if developerMode {
                    settingField(
                        AppText.value("Rendezvous broker", "配对服务器", language: language),
                        text: $serverURL,
                        placeholder: defaultRendezvousBroker,
                        helper: AppText.value("Leave empty to use the built-in Envoix broker.", "留空则使用内置 Envoix 配对服务器。", language: language)
                    )
                    settingField(
                        AppText.value("Relay URL", "中继 URL", language: language),
                        text: $relayURL,
                        placeholder: defaultRelayURL,
                        helper: AppText.value("Leave empty to use the built-in relay for Room pairing.", "留空则使用内置中继服务。", language: language)
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
                }

                Text(AppText.value(
                    "Speed limiting is not exposed yet because current transfers do not enforce it.",
                    "当前传输尚未强制执行限速，因此暂不展示速度限制设置。",
                    language: language
                ))
                .font(.body)
                .foregroundStyle(Theme.muted)
                .card(padding: 14)

                Text(appDebugBuildLabel)
                    .font(.caption.monospaced())
                    .foregroundStyle(Theme.muted)
                    .frame(maxWidth: .infinity, alignment: .center)
                    .padding(.top, 2)
            }
            .padding(.vertical, 12)
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

    private func settingField(
        _ title: String,
        text: Binding<String>,
        placeholder: String = "",
        helper: String? = nil
    ) -> some View {
        VStack(alignment: .leading, spacing: 8) {
            Text(title)
                .font(.title3.weight(.semibold))
                .foregroundStyle(Theme.muted)
            TextField(placeholder.isEmpty ? title : placeholder, text: text)
                .textFieldStyle(.plain)
                .font(.body.monospaced())
                .foregroundStyle(Theme.text)
                .padding(.horizontal, 10)
                .frame(minHeight: 44)
                .background(Theme.surface)
                .overlay(
                    RoundedRectangle(cornerRadius: Theme.cardRadius)
                        .strokeBorder(Theme.line.opacity(0.75), lineWidth: 0.8)
                )
                .clipShape(RoundedRectangle(cornerRadius: Theme.cardRadius))
            if let helper {
                Text(helper)
                    .font(.body)
                    .foregroundStyle(Theme.muted)
                    .fixedSize(horizontal: false, vertical: true)
            }
        }
        .card(padding: 14)
    }
}
