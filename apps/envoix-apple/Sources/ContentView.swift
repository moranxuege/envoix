import SwiftUI
import EnvoixCore
#if os(iOS)
import UniformTypeIdentifiers
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

    func mobileTitle(language: String) -> String {
        switch self {
        case .transfer: return AppText.value("Home", "首页", language: language)
        case .activity: return AppText.value("Activity", "活动", language: language)
        case .settings: return AppText.value("Settings", "设置", language: language)
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

private enum MobileSheet: String, Identifiable {
    case send, receive, activity, settings

    var id: String { rawValue }
}

struct ContentView: View {
    @EnvironmentObject private var model: AppModel
    @Environment(\.scenePhase) private var scenePhase
    @AppStorage("envoix.appearance") private var appearance: Appearance = .system
    @AppStorage("envoix.language") private var language = "en"
    @State private var stage: AppStage = .transfer
    @State private var mobileSheet: MobileSheet? = {
        #if DEBUG
        if ProcessInfo.processInfo.arguments.contains("--ui-testing-start-activity") {
            return .activity
        }
        #endif
        return nil
    }()
    #if DEBUG
    @State private var didStageBackgroundShareFixture = false
    #endif

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
        #if os(iOS)
        .onOpenURL(perform: handleIncomingURL)
        #endif
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
        NavigationStack {
            mobileHome
                .padding(.horizontal, 16)
                .background(Theme.bg)
                .navigationTitle("Envoix")
                .navigationBarTitleDisplayMode(.inline)
                .toolbar {
                    ToolbarItem(placement: .topBarLeading) {
                        Button {
                            mobileSheet = .activity
                        } label: {
                            Image(systemName: "clock.arrow.circlepath")
                                .font(.body.weight(.semibold))
                        }
                        .accessibilityLabel(AppText.value("Activity", "活动", language: language))
                        .accessibilityIdentifier("open_activity")
                    }

                    ToolbarItem(placement: .topBarTrailing) {
                        Button {
                            mobileSheet = .settings
                        } label: {
                            Image(systemName: "gearshape")
                                .font(.body.weight(.semibold))
                        }
                        .accessibilityLabel(AppText.value("Settings", "设置", language: language))
                        .accessibilityIdentifier("open_settings")
                    }
                }
        }
        .safeAreaInset(edge: .bottom, spacing: 0) {
            mobileActivityCapsule
                .padding(.bottom, 8)
        }
        .sheet(item: $mobileSheet) { sheet in
            NavigationStack {
                mobileSheetContent(sheet)
                    .padding(.horizontal, 16)
                    .background(Theme.bg)
                    .navigationTitle(mobileSheetTitle(sheet))
                    .navigationBarTitleDisplayMode(.inline)
                    .toolbar {
                        ToolbarItem(placement: .cancellationAction) {
                            Button {
                                mobileSheet = nil
                            } label: {
                                Image(systemName: "xmark")
                                    .font(.body.weight(.semibold))
                                    .frame(width: 44, height: 44)
                            }
                            .tint(Theme.accentStrong)
                            .accessibilityLabel(AppText.value("Close", "关闭", language: language))
                            .accessibilityIdentifier("mobile_sheet_done")
                        }
                    }
            }
            .presentationDragIndicator(.visible)
            .presentationDetents([.large])
        }
        .onChange(of: model.send.isBusy) { isBusy in
            if isBusy, !model.send.isPreparingManifest, mobileSheet == .send {
                mobileSheet = nil
            } else if !isBusy {
                presentPendingSendSelection()
            }
        }
        .onChange(of: model.send.transferActivity?.activityId) { activityID in
            if activityID != nil, mobileSheet == .send {
                mobileSheet = nil
            }
        }
        .onChange(of: model.receive.isBusy) { isBusy in
            if isBusy, mobileSheet == .receive { mobileSheet = nil }
        }
        .onAppear(perform: presentPendingSendSelection)
        .onChange(of: scenePhase) { phase in
            #if DEBUG
            if phase == .background {
                stageBackgroundShareFixtureIfRequested()
            }
            #endif
            guard phase == .active else { return }
            presentPendingSendSelection()
        }
        #if DEBUG
        .onAppear {
            FolderPickerUITestFixture.cleanIfRequested()
            FilePickerUITestFixture.cleanIfRequested()
            guard ProcessInfo.processInfo.arguments.contains("--ui-testing") else { return }
            let initialSheet: MobileSheet? = ProcessInfo.processInfo.arguments.contains("--ui-testing-start-activity")
                ? .activity
                : nil
            DispatchQueue.main.async {
                mobileSheet = initialSheet
            }
        }
        #endif
    }

    private var mobileHome: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 18) {
                VStack(alignment: .leading, spacing: 6) {
                    Text(AppText.value("Move something", "传点东西", language: language))
                        .font(.largeTitle.bold())
                        .foregroundStyle(Theme.text)
                    Text(AppText.value(
                        "Choose what this device will do. Either device may show a QR code; the other scans it.",
                        "选择这台设备要发送还是接收。任意一台设备都可以显示二维码，由另一台扫描。",
                        language: language
                    ))
                    .font(.body)
                    .foregroundStyle(Theme.muted)
                    .fixedSize(horizontal: false, vertical: true)
                }

                mobileHomeAction(
                    sheet: .send,
                    role: .send,
                    title: AppText.value("Send items", "发送项目", language: language),
                    subtitle: AppText.value(
                        "Choose files or a folder, then show your send QR or scan a receive QR.",
                        "选择文件或文件夹，然后显示发送码或扫描接收码。",
                        language: language
                    ),
                    identifier: "home_send"
                )

                mobileHomeAction(
                    sheet: .receive,
                    role: .receive,
                    title: AppText.value("Receive a file", "接收文件", language: language),
                    subtitle: AppText.value(
                        "Choose where to save, then show your receive QR or scan a send QR.",
                        "确认保存位置，然后显示接收码或扫描发送码。",
                        language: language
                    ),
                    identifier: "home_receive"
                )

                HStack(alignment: .top, spacing: 10) {
                    Image(systemName: "arrow.triangle.2.circlepath")
                        .foregroundStyle(Theme.accentStrong)
                    Text(AppText.value(
                        "The two devices choose opposite roles. It does not matter which one scans.",
                        "两台设备选择相反角色即可，由哪一台扫码都可以。",
                        language: language
                    ))
                    .font(.footnote)
                    .foregroundStyle(Theme.muted)
                    .fixedSize(horizontal: false, vertical: true)
                }
                .card(padding: 14)
            }
            .padding(.vertical, 16)
        }
        .accessibilityIdentifier("transfer_home")
    }

    private func mobileHomeAction(
        sheet: MobileSheet,
        role: TransferRole,
        title: String,
        subtitle: String,
        identifier: String
    ) -> some View {
        Button {
            mobileSheet = sheet
        } label: {
            HStack(spacing: 14) {
                Image(systemName: role.icon)
                    .font(.title2.weight(.semibold))
                    .foregroundStyle(Theme.accentStrong)
                    .frame(width: 50, height: 50)
                    .background(Theme.accentSoft, in: RoundedRectangle(cornerRadius: 16, style: .continuous))

                VStack(alignment: .leading, spacing: 5) {
                    Text(title)
                        .font(.title3.weight(.semibold))
                        .foregroundStyle(Theme.text)
                    Text(subtitle)
                        .font(.subheadline)
                        .foregroundStyle(Theme.muted)
                        .fixedSize(horizontal: false, vertical: true)
                }

                Spacer(minLength: 6)
                Image(systemName: "chevron.up")
                    .font(.caption.weight(.bold))
                    .foregroundStyle(Theme.muted)
            }
            .padding(16)
            .frame(maxWidth: .infinity, minHeight: 96, alignment: .leading)
            .background(Theme.surfaceRaised)
            .overlay(
                RoundedRectangle(cornerRadius: 20, style: .continuous)
                    .strokeBorder(Theme.line.opacity(0.72), lineWidth: 0.8)
            )
            .clipShape(RoundedRectangle(cornerRadius: 20, style: .continuous))
            .contentShape(RoundedRectangle(cornerRadius: 20, style: .continuous))
        }
        .buttonStyle(.plain)
        .accessibilityIdentifier(identifier)
    }

    @ViewBuilder
    private func mobileSheetContent(_ sheet: MobileSheet) -> some View {
        switch sheet {
        case .send:
            SendView(
                viewModel: model.send,
                initialFiles: model.pendingSendSelection?.fileURLs ?? [],
                initialFileAccess: model.pendingSendSelection?.sourceAccess,
                initialPendingSelectionID: model.pendingSendSelection?.id
            )
        case .receive:
            ReceiveView(viewModel: model.receive)
        case .activity:
            TransferStageView(
                records: model.activities,
                manifestByActivityID: model.manifestActivities,
                metricsByActivityID: model.activityMetrics,
                onCopyDiagnostics: model.diagnosticReport,
                onRemoteLogTarget: model.remoteLogTarget,
                onRemoteDiagnosticReport: model.remoteDiagnosticReport,
                onAppDiagnosticReport: model.appDiagnosticReport,
                onPause: model.pauseActivity,
                onCanResume: model.canResumeActivity,
                onResume: model.resumeActivity,
                onCancel: model.cancelActivity,
                onReplacePublicationTarget: model.replaceReceivePublicationTarget,
                onDelete: model.removeActivity
            )
        case .settings:
            SettingsStageView()
        }
    }

    private func mobileSheetTitle(_ sheet: MobileSheet) -> String {
        switch sheet {
        case .send: return AppText.value("Send", "发送", language: language)
        case .receive: return AppText.value("Receive", "接收", language: language)
        case .activity: return AppText.value("Activity", "活动", language: language)
        case .settings: return AppText.value("Settings", "设置", language: language)
        }
    }

    private func handleIncomingURL(_ url: URL) {
        if let id = ShareDraftLink.draftID(from: url) {
            presentSharedDraft(preferredID: id)
            return
        }
        guard url.isFileURL else { return }

        do {
            switch try model.importOpenedSendFile(url) {
            case .imported:
                mobileSheet = .send
            case .queued:
                ToastCenter.shared.show(AppText.value(
                    "The file is ready and will open after the current send finishes.",
                    "文件已准备好，将在当前发送完成后打开。",
                    language: language
                ))
            }
        } catch let error as OpenedSendFileError {
            ToastCenter.shared.show(openedSendFileErrorMessage(error))
        } catch {
            ToastCenter.shared.show(error.localizedDescription)
        }
    }

    private func presentPendingSendSelection() {
        if !model.send.isBusy, model.pendingSendSelection != nil {
            mobileSheet = .send
            return
        }
        presentSharedDraft(preferredID: nil)
    }

    private func openedSendFileErrorMessage(_ error: OpenedSendFileError) -> String {
        switch error {
        case .unsupportedURL:
            return AppText.value(
                "Envoix can open local files only.",
                "Envoix 目前只能打开本地文件。",
                language: language
            )
        case .unsupportedItem:
            return AppText.value(
                "This item type is not supported. Choose a regular file or folder.",
                "暂不支持此项目类型。请选择普通文件或文件夹。",
                language: language
            )
        case .inaccessible:
            return AppText.value(
                "Envoix could not access this file. Download it first, then try again.",
                "Envoix 无法访问此文件。请先下载完成，然后重试。",
                language: language
            )
        }
    }

    #if DEBUG
    private func stageBackgroundShareFixtureIfRequested() {
        let arguments = ProcessInfo.processInfo.arguments
        let stagesSingleItem = arguments.contains("--ui-testing-stage-share-on-background")
        let stagesMultipleItems = arguments.contains("--ui-testing-stage-multi-share-on-background")
        guard !didStageBackgroundShareFixture,
              stagesSingleItem || stagesMultipleItems else {
            return
        }
        didStageBackgroundShareFixture = true

        let sourceURLs = stagesMultipleItems
            ? ["foreground-photo.jpg", "foreground-notes.txt"].map {
                FileManager.default.temporaryDirectory.appendingPathComponent($0)
            }
            : [FileManager.default.temporaryDirectory.appendingPathComponent("foreground-share.txt")]
        do {
            for (index, sourceURL) in sourceURLs.enumerated() {
                try Data("foreground share fixture \(index)".utf8).write(to: sourceURL, options: .atomic)
            }
            defer { sourceURLs.forEach { try? FileManager.default.removeItem(at: $0) } }
            let items = sourceURLs.map {
                ShareDraftStagingItem(
                    sourceURL: $0,
                    contentTypeIdentifier: UTType(filenameExtension: $0.pathExtension)?.identifier
                        ?? UTType.data.identifier,
                    mediaKind: $0.pathExtension == "jpg" ? .image : .file,
                    preferredFileName: nil
                )
            }
            _ = try ShareDraftStore.live().stage(items: items)
        } catch {
            assertionFailure("Could not stage the background Share fixture: \(error)")
        }
    }
    #endif

    private func presentSharedDraft(preferredID: UUID?) {
        do {
            switch try model.importSharedSendDraft(preferredID: preferredID) {
            case .imported:
                mobileSheet = .send
            case .noPendingDraft:
                break
            case .sendBusy:
                ToastCenter.shared.show(AppText.value(
                    "Finish the current send, then Envoix will open the shared item.",
                    "请先完成当前发送，随后 Envoix 会打开已分享的项目。",
                    language: language
                ))
            }
        } catch {
            ToastCenter.shared.show(error.localizedDescription)
        }
    }

    @ViewBuilder
    private var mobileActivityCapsule: some View {
        if mobileSheet != .activity, let activity = featuredActivity {
            Button {
                mobileSheet = .activity
            } label: {
                HStack(spacing: 11) {
                    Image(systemName: mobileActivityIcon(activity))
                        .font(.body.weight(.semibold))
                        .foregroundStyle(Theme.accentStrong)
                        .frame(width: 34, height: 34)
                        .background(Theme.accentSoft, in: Circle())

                    VStack(alignment: .leading, spacing: 3) {
                        Text(mobileActivityTitle(activity))
                            .font(.subheadline.weight(.semibold))
                            .foregroundStyle(Theme.text)
                            .lineLimit(1)
                        Text(mobileActivitySubtitle(activity))
                            .font(.caption)
                            .foregroundStyle(Theme.muted)
                            .lineLimit(1)
                    }

                    Spacer(minLength: 8)
                    Image(systemName: "chevron.right")
                        .font(.caption.weight(.bold))
                        .foregroundStyle(Theme.muted)
                }
                .padding(.horizontal, 12)
                .frame(maxWidth: .infinity, minHeight: 54)
                .background(.regularMaterial, in: RoundedRectangle(cornerRadius: 20, style: .continuous))
                .overlay(
                    RoundedRectangle(cornerRadius: 20, style: .continuous)
                        .strokeBorder(Theme.line.opacity(0.72), lineWidth: 0.8)
                )
                .shadow(color: Theme.shadowColor, radius: 8, y: 3)
            }
            .buttonStyle(.plain)
            .padding(.horizontal, 14)
            .accessibilityIdentifier("active_transfer_capsule")
        }
    }

    private var featuredActivity: FfiTransferActivityRecord? {
        model.activities.first(where: isPending)
    }

    private func mobileActivityTitle(_ record: FfiTransferActivityRecord) -> String {
        switch record.state {
        case .queued, .binding, .waitingForPeer:
            return AppText.value("Waiting to connect", "等待连接", language: language)
        case .pairing, .connecting:
            return AppText.value("Connecting", "正在连接", language: language)
        case .transferring:
            return record.direction == .send
                ? AppText.value("Sending", "正在发送", language: language)
                : AppText.value("Receiving", "正在接收", language: language)
        case .verifying, .publishing, .unconfirmed:
            return AppText.value("Saving", "正在保存", language: language)
        case .paused:
            return AppText.value("Transfer paused", "传输已暂停", language: language)
        case .completed:
            return AppText.value("Transfer complete", "传输完成", language: language)
        case .failed:
            return AppText.value("Transfer needs attention", "传输需要处理", language: language)
        case .canceled, .unknown:
            return AppText.value("Transfer", "传输", language: language)
        }
    }

    private func mobileActivitySubtitle(_ record: FfiTransferActivityRecord) -> String {
        let name = record.fileName.trimmed.isEmpty
            ? AppText.value("Open Activity for details", "打开活动查看详情", language: language)
            : record.fileName
        guard record.totalBytes > 0 else { return name }
        let percent = min(100, Int(Double(record.bytesTransferred) / Double(record.totalBytes) * 100))
        return "\(name) · \(percent)%"
    }

    private func mobileActivityIcon(_ record: FfiTransferActivityRecord) -> String {
        switch record.state {
        case .paused: return "pause.fill"
        case .verifying, .publishing, .unconfirmed: return "tray.and.arrow.down.fill"
        default: return record.direction == .send ? "arrow.up" : "arrow.down"
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
                manifestByActivityID: model.manifestActivities,
                metricsByActivityID: model.activityMetrics,
                onCopyDiagnostics: model.diagnosticReport,
                onRemoteLogTarget: model.remoteLogTarget,
                onRemoteDiagnosticReport: model.remoteDiagnosticReport,
                onAppDiagnosticReport: model.appDiagnosticReport,
                onPause: model.pauseActivity,
                onCanResume: model.canResumeActivity,
                onResume: model.resumeActivity,
                onCancel: model.cancelActivity,
                onReplacePublicationTarget: model.replaceReceivePublicationTarget,
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
        #if os(macOS)
        desktopBody
        #else
        EmptyView()
        #endif
    }

    #if os(macOS)
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
    #endif

    #if os(macOS)
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
                        .strokeBorder(role == item ? Theme.accent.opacity(0.45) : Color.clear, lineWidth: 0.8)
                )
                .accessibilityIdentifier("transfer_role_\(item.rawValue)")
            }
        }
        .padding(4)
        .background(Theme.line.opacity(0.35), in: RoundedRectangle(cornerRadius: Theme.cardRadius))
    }
    #endif

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

private struct TransferStageView: View {
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
    #endif
    private let commandAcknowledgementTimeout: TimeInterval = 5
    let records: [FfiTransferActivityRecord]
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
        #if os(iOS)
        .sheet(isPresented: $isPublicationFolderPickerPresented) {
            FolderPickerSheet(
                onPick: replacePublicationTarget,
                onCancel: {
                    publicationTargetActivityID = nil
                    isPublicationFolderPickerPresented = false
                }
            )
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
        if record.totalBytes > 0 {
            parts.append("\(byteString(record.bytesTransferred)) / \(byteString(record.totalBytes))")
        }
        if let speed = speedBps(for: record, metrics: metrics), speed > 0 {
            parts.append(rateString(speed))
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
            if let command = pendingCommands[record.activityId] {
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
                    onDelete(record.activityId)
                }
                .accessibilityIdentifier("activity_delete_\(record.activityId)")
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
        if !onReplacePublicationTarget(activityID, url, nil, nil) {
            ToastCenter.shared.show(AppText.value(
                "This save target can no longer be changed.",
                "当前已无法更换保存位置。",
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
        if record.state == .completed, let url = completedReceiveURL(record, manifest: manifest) {
            let isMultiRootManifest = manifest.map { $0.rootCount > 1 } == true
            VStack(alignment: .leading, spacing: 8) {
                Label(
                    isMultiRootManifest
                        ? AppText.value("Saved items", "已保存项目", language: language)
                        : AppText.value("Saved item", "已保存项目", language: language),
                    systemImage: "checkmark.circle.fill"
                )
                    .font(.callout.weight(.semibold))
                    .foregroundStyle(Theme.success)
                Text(isMultiRootManifest
                     ? AppText.value(
                        "\(manifest?.rootCount ?? 0) items",
                        "\(manifest?.rootCount ?? 0) 个项目",
                        language: language
                     )
                     : url.lastPathComponent)
                    .font(.body.weight(.semibold))
                    .foregroundStyle(Theme.text)
                    .lineLimit(2)
                Text(AppText.value(
                    "Saved to \((isMultiRootManifest ? url : url.deletingLastPathComponent()).lastPathComponent)",
                    "保存到 \((isMultiRootManifest ? url : url.deletingLastPathComponent()).lastPathComponent)",
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

    private func completedReceiveURL(
        _ record: FfiTransferActivityRecord,
        manifest: FfiManifestActivityRecord?
    ) -> URL? {
        manifest.flatMap(availableCompletedManifestURL)
            ?? availableCompletedFileURL(
                path: record.completedFilePath,
                expectedBytes: record.bytesTransferred
            )
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
        case .publishing:
            return record.retryable
                ? AppText.value("Save failed", "保存失败", language: language)
                : AppText.value("Saving", "保存中", language: language)
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
    @EnvironmentObject private var model: AppModel
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
    private let coreInfo = envoixCoreInfo()

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

                #if os(macOS)
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
                #endif

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

                transferCacheSection

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
                    .accessibilityIdentifier("settings_developer_mode")
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

                Text(coreBuildLabel)
                    .font(.caption.monospaced())
                    .foregroundStyle(coreInfo.ffiApiVersion == expectedCoreFFIAPIVersion ? Theme.muted : Theme.danger)
                    .frame(maxWidth: .infinity, alignment: .center)
                    .padding(.top, 2)
                    .accessibilityIdentifier("settings_core_version")
            }
            .padding(.vertical, 12)
        }
        .onAppear {
            migrateLogServerIfNeeded()
            model.refreshTransferCache()
        }
    }

    private func migrateLogServerIfNeeded() {
        if deprecatedLogServers.contains(logServer.trimmed) {
            logServer = defaultLogServer
        }
    }

    private var coreBuildLabel: String {
        "\(appDebugBuildLabel) · Core \(coreInfo.coreVersion) · API \(coreInfo.ffiApiVersion)"
    }

    private var transferCacheSection: some View {
        VStack(alignment: .leading, spacing: 10) {
            Text(AppText.value("Transfer cache", "传输缓存", language: language))
                .font(.title3.weight(.semibold))
                .foregroundStyle(Theme.muted)
            Text(ByteCountFormatter.string(
                fromByteCount: Int64(clamping: model.transferCacheSummary.totalBytes),
                countStyle: .file
            ))
                .font(.title2.weight(.semibold))
                .foregroundStyle(Theme.text)
            Text(AppText.value(
                "Temporary Share and receive data. Active, paused, and resumable transfers are always protected.",
                "用于分享和接收的临时数据；活动中、已暂停和可续传的任务始终会被保护。",
                language: language
            ))
                .font(.body)
                .foregroundStyle(Theme.muted)
                .fixedSize(horizontal: false, vertical: true)
            if model.transferCacheSummary.protectedBytes > 0 {
                Text(AppText.value(
                    "Protected: \(cacheByteString(model.transferCacheSummary.protectedBytes))",
                    "受保护：\(cacheByteString(model.transferCacheSummary.protectedBytes))",
                    language: language
                ))
                    .font(.footnote)
                    .foregroundStyle(Theme.muted)
            }
            if let error = model.transferCacheError {
                Text(error)
                    .font(.footnote)
                    .foregroundStyle(Theme.danger)
            }
            Button {
                model.cleanTransferCache()
            } label: {
                HStack(spacing: 8) {
                    if model.isCleaningTransferCache {
                        ProgressView().controlSize(.small)
                    }
                    Text(AppText.value("Clean Up", "清理缓存", language: language))
                }
                .frame(maxWidth: .infinity, minHeight: 40)
            }
            .buttonStyle(.bordered)
            .disabled(model.isCleaningTransferCache)
            .accessibilityIdentifier("settings_clean_transfer_cache")
        }
        .card(padding: 14)
    }

    private func cacheByteString(_ bytes: UInt64) -> String {
        ByteCountFormatter.string(
            fromByteCount: Int64(clamping: bytes),
            countStyle: .file
        )
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
