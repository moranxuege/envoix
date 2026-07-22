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
    case send, receive, nearbyPairing, activity, settings

    var id: String { rawValue }
}

struct ContentView: View {
    private static let roleSwitchPresentationDelay: TimeInterval = 0.2

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
    @State private var preservedSendSelection = SendSelectionSnapshot()
    @State private var pendingSendPairingInput: String?
    @State private var pendingReceivePairingInput: String?
    @State private var nearbyPairingSelection: NearbyPairingSelection?
    #if os(iOS)
    @AppStorage("envoix.serverURL") private var nearbyServerURL = ""
    @AppStorage("envoix.relayURL") private var nearbyRelayURL = ""
    @StateObject private var nearbyCoordinator = NearbyDiscoveryCoordinator()
    @State private var nearbyInboundInvite: String?
    @State private var nearbyPairingBusy = false
    @State private var nearbyPairingError: String?
    #endif
    #if DEBUG
    @State private var didStageBackgroundShareFixture = false
    @State private var openInUITestFixtureURL: URL?
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
                                dismissMobileSheet()
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
                nearbyPairingSelection = nil
            } else if !isBusy {
                presentPendingSendSelection()
            }
        }
        .onChange(of: model.send.transferActivity?.activityId) { activityID in
            if activityID != nil {
                preservedSendSelection = SendSelectionSnapshot()
            }
            if activityID != nil, mobileSheet == .send {
                mobileSheet = nil
            }
        }
        .onChange(of: model.receive.isBusy) { isBusy in
            if isBusy, mobileSheet == .receive {
                mobileSheet = nil
                nearbyPairingSelection = nil
            }
        }
        .onChange(of: nearbyCoordinator.state.incomingRendezvousOffer?.id) { _ in
            presentIncomingNearbyOfferIfNeeded()
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
            FilePickerUITestFixture.stageIfRequested()
            OpenInUITestFixture.cleanIfRequested()
            openInUITestFixtureURL = OpenInUITestFixture.stageIfRequested()
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

                NavigationLink {
                    NearbyDiscoveryView(coordinator: nearbyCoordinator) { selection in
                        nearbyPairingSelection = selection
                        nearbyInboundInvite = nil
                        nearbyPairingError = nil
                        mobileSheet = .nearbyPairing
                    }
                } label: {
                    mobileHomeActionLabel(
                        systemImage: "dot.radiowaves.left.and.right",
                        title: AppText.value("Find nearby devices", "发现附近设备", language: language),
                        subtitle: AppText.value(
                            "Discover Envoix devices over Bluetooth and the local network.",
                            "通过蓝牙和局域网发现 Envoix 设备。",
                            language: language
                        ),
                        chevron: "chevron.right"
                    )
                }
                .buttonStyle(.plain)
                .accessibilityIdentifier("home_nearby")

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

                #if DEBUG
                if let openInUITestFixtureURL {
                    Text(openInUITestFixtureURL.absoluteString)
                        .font(.caption2)
                        .accessibilityIdentifier("open_in_fixture_url")
                }
                #endif
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
            mobileHomeActionLabel(
                systemImage: role.icon,
                title: title,
                subtitle: subtitle,
                chevron: "chevron.up"
            )
        }
        .buttonStyle(.plain)
        .accessibilityIdentifier(identifier)
    }

    private func mobileHomeActionLabel(
        systemImage: String,
        title: String,
        subtitle: String,
        chevron: String
    ) -> some View {
        HStack(spacing: 14) {
            Image(systemName: systemImage)
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
            Image(systemName: chevron)
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

    @ViewBuilder
    private func mobileSheetContent(_ sheet: MobileSheet) -> some View {
        switch sheet {
        case .send:
            SendView(
                viewModel: model.send,
                initialFiles: preservedSendSelection.items.isEmpty
                    ? model.pendingSendSelection?.fileURLs ?? []
                    : preservedSendSelection.items,
                initialFileAccess: preservedSendSelection.items.isEmpty
                    ? model.pendingSendSelection?.sourceAccess
                    : preservedSendSelection.sourceAccess,
                initialPendingSelectionID: preservedSendSelection.items.isEmpty
                    ? model.pendingSendSelection?.id
                    : preservedSendSelection.pendingSelectionID,
                initialPairingInput: pendingSendPairingInput,
                onInitialPairingInputConsumed: { pendingSendPairingInput = nil },
                onSwitchToReceive: switchMobileToReceive
            )
        case .receive:
            ReceiveView(
                viewModel: model.receive,
                initialPairingInput: pendingReceivePairingInput,
                onInitialPairingInputConsumed: { pendingReceivePairingInput = nil },
                onSwitchToSend: switchMobileToSend
            )
        case .nearbyPairing:
            if let nearbyPairingSelection {
                NearbyPairingView(
                    selection: nearbyPairingSelection,
                    sendEnabled: nearbyAllowedRole != .receive,
                    receiveEnabled: nearbyAllowedRole != .send,
                    isBusy: nearbyPairingBusy,
                    error: nearbyPairingError,
                    onSend: { beginNearbyPairing(role: .send) },
                    onReceive: { beginNearbyPairing(role: .receive) }
                )
            } else {
                EmptyView()
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
        case .nearbyPairing: return AppText.value("Experimental BLE pairing", "实验性蓝牙配对", language: language)
        case .activity: return AppText.value("Activity", "活动", language: language)
        case .settings: return AppText.value("Settings", "设置", language: language)
        }
    }

    private func switchMobileToReceive(_ input: String, selection: SendSelectionSnapshot) {
        preservedSendSelection = selection
        pendingReceivePairingInput = input
        replaceMobileSheet(with: .receive)
    }

    private func switchMobileToSend(_ input: String) {
        pendingSendPairingInput = input
        replaceMobileSheet(with: .send)
    }

    private func replaceMobileSheet(with sheet: MobileSheet) {
        mobileSheet = nil
        DispatchQueue.main.asyncAfter(deadline: .now() + Self.roleSwitchPresentationDelay) {
            mobileSheet = sheet
        }
    }

    private func dismissMobileSheet() {
        mobileSheet = nil
        nearbyPairingSelection = nil
        nearbyInboundInvite = nil
        nearbyPairingBusy = false
        nearbyPairingError = nil
    }

    private var nearbyAllowedRole: FfiInviteRole? {
        guard let nearbyInboundInvite,
              let invite = try? parsePairingInvite(input: nearbyInboundInvite) else {
            return nil
        }
        switch invite.role {
        case .send: return .receive
        case .receive: return .send
        case .unknown: return nil
        }
    }

    private func presentIncomingNearbyOfferIfNeeded() {
        guard let offer = nearbyCoordinator.state.incomingRendezvousOffer else { return }
        defer { nearbyCoordinator.consumeRendezvousOffer(id: offer.id) }
        guard (try? parsePairingInvite(input: offer.invite)) != nil else {
            ToastCenter.shared.show(AppText.value(
                "An invalid Bluetooth invitation was rejected.",
                "已拒绝无效的蓝牙邀请。",
                language: language
            ))
            return
        }
        nearbyPairingSelection = NearbyPairingSelection(
            discoveryPeerKey: offer.senderPeerKey,
            displayName: offer.senderDisplayName,
            sources: [.bluetooth]
        )
        nearbyInboundInvite = offer.invite
        nearbyPairingError = nil
        mobileSheet = .nearbyPairing
    }

    private func beginNearbyPairing(role: FfiInviteRole) {
        guard !nearbyPairingBusy, let selection = nearbyPairingSelection else { return }
        if let inbound = nearbyInboundInvite {
            guard nearbyAllowedRole == role else {
                nearbyPairingError = AppText.value(
                    "Choose the opposite role advertised by the other device.",
                    "请选择与对方设备相反的角色。",
                    language: language
                )
                return
            }
            nearbyInboundInvite = nil
            nearbyCoordinator.stop()
            if role == .send {
                pendingSendPairingInput = inbound
                replaceMobileSheet(with: .send)
            } else {
                pendingReceivePairingInput = inbound
                replaceMobileSheet(with: .receive)
            }
            return
        }

        guard selection.sources.contains(.bluetooth) else {
            nearbyPairingError = AppText.value(
                "This device is no longer reachable over Bluetooth.",
                "当前已无法通过蓝牙连接此设备。",
                language: language
            )
            return
        }
        do {
            let invite = try makePairingInvite(
                role: role,
                broker: nearbyServerURL,
                relay: nearbyRelayURL
            )
            nearbyPairingBusy = true
            nearbyPairingError = nil
            nearbyCoordinator.offerInvite(
                peerKey: selection.discoveryPeerKey,
                invite: invite.payload
            ) { error in
                nearbyPairingBusy = false
                if let error {
                    nearbyPairingError = error
                    return
                }
                nearbyCoordinator.stop()
                if role == .send {
                    pendingSendPairingInput = invite.code
                    replaceMobileSheet(with: .send)
                } else {
                    pendingReceivePairingInput = invite.code
                    replaceMobileSheet(with: .receive)
                }
            }
        } catch {
            nearbyPairingError = error.localizedDescription
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

    private var featuredActivity: TransferActivityRecord? {
        model.activities.first { ActivityProjectionPolicy.isPending($0.state) }
    }

    private func mobileActivityTitle(_ record: TransferActivityRecord) -> String {
        switch record.state {
        case .preparing, .waitingForPeer:
            return AppText.value("Waiting to connect", "等待连接", language: language)
        case .pairing, .connecting:
            return AppText.value("Connecting", "正在连接", language: language)
        case .transferring:
            return record.direction == .send
                ? AppText.value("Sending", "正在发送", language: language)
                : AppText.value("Receiving", "正在接收", language: language)
        case .verifying, .saving, .waitingForReceiverSave, .finalizingDelivery:
            return AppText.value("Saving", "正在保存", language: language)
        case .paused:
            return AppText.value("Transfer paused", "传输已暂停", language: language)
        case .delivered:
            return AppText.value("Transfer complete", "传输完成", language: language)
        case .failed:
            return AppText.value("Transfer needs attention", "传输需要处理", language: language)
        case .canceled:
            return AppText.value("Transfer", "传输", language: language)
        }
    }

    private func mobileActivitySubtitle(_ record: TransferActivityRecord) -> String {
        let name = AppText.value(
            "\(record.itemCount) items",
            "\(record.itemCount) 个项目",
            language: language
        )
        guard record.totalBytes > 0 else { return name }
        let percent = min(100, Int(Double(record.bytesTransferred) / Double(record.totalBytes) * 100))
        return "\(name) · \(percent)%"
    }

    private func mobileActivityIcon(_ record: TransferActivityRecord) -> String {
        switch record.state {
        case .paused: return "pause.fill"
        case .verifying, .saving, .waitingForReceiverSave, .finalizingDelivery: return "tray.and.arrow.down.fill"
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
        ActivityProjectionPolicy.pendingCount(model.activities)
    }

    private var hasFailedTransfer: Bool {
        if model.activities.contains(where: { $0.state == .failed }) {
            return true
        }
        return isFailed(model.receive) || isFailed(model.send)
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
    @State private var preservedSendSelection = SendSelectionSnapshot()
    @State private var pendingSendPairingInput: String?
    @State private var pendingReceivePairingInput: String?
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
