#if os(iOS)
import EnvoixCore
import SwiftUI
import UniformTypeIdentifiers

private enum MobilePage {
    case connect
    case room
    case activity
    case settings
}

private enum MobileTransferRoute: String, Identifiable {
    case send
    case receive

    var id: String { rawValue }
}

struct MobileConnectionFlowView: View {
    @EnvironmentObject private var model: AppModel
    @Environment(\.scenePhase) private var scenePhase
    @AppStorage("envoix.language") private var language = "en"

    @StateObject private var nearbyCoordinator = NearbyDiscoveryCoordinator()
    @StateObject private var workflow = ConnectionWorkflowState()
    @State private var page: MobilePage = {
        #if DEBUG
        if ProcessInfo.processInfo.arguments.contains("--ui-testing-start-activity") {
            return .activity
        }
        #endif
        return .connect
    }()
    @State private var returnPage: MobilePage = .connect
    @State private var transferRoute: MobileTransferRoute?
    @State private var preservedSendSelection = SendSelectionSnapshot()
    @State private var pendingSendPairingInput: String?
    @State private var pendingReceivePairingInput: String?
    @State private var transferUsesInboundInvite = false
    @State private var transferRoomID: UUID?
    @State private var roomOwnsSendDraft = false
    @State private var scannerIsPresented = false
    @State private var manualEntryIsPresented = false
    @State private var manualPairingInput = ""
    @State private var isCloseRoomConfirmationPresented = false
    #if DEBUG
    @State private var didStageBackgroundShareFixture = false
    @State private var openInUITestFixtureURL: URL?
    #endif

    var body: some View {
        NavigationStack {
            pageContent
                .background(Theme.bg)
                .navigationTitle(pageTitle)
                .navigationBarTitleDisplayMode(.inline)
                .toolbar { toolbarContent }
        }
        .sheet(item: $transferRoute, onDismiss: finishTransferSheetPresentation) { route in
            NavigationStack {
                transferContent(route)
                    .padding(.horizontal, 16)
                    .background(Theme.bg)
                    .navigationTitle(transferTitle(route))
                    .navigationBarTitleDisplayMode(.inline)
                    .toolbar {
                        ToolbarItem(placement: .cancellationAction) {
                            Button {
                                transferRoute = nil
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
        .sheet(isPresented: $scannerIsPresented) {
            QRCodeScannerSheet(language: language) { value in
                openPairingRoom(input: value)
            }
        }
        .sheet(isPresented: $manualEntryIsPresented) {
            ManualPairingCodeSheet(
                language: language,
                input: $manualPairingInput
            ) { value in
                let error = openPairingRoom(input: value)
                if error == nil {
                    manualEntryIsPresented = false
                }
                return error
            }
        }
        .alert(item: pendingOfferBinding) { pending in
            Alert(
                title: Text(AppText.value(
                    "Unverified nearby invitation",
                    "未经验证的附近设备邀请",
                    language: language
                )),
                message: Text(AppText.value(
                    "\(pending.offer.senderDisplayName ?? "A nearby device") wants to start a one-time transfer. Confirm on the other device before accepting.",
                    "\(pending.offer.senderDisplayName ?? "附近设备") 希望开始一次性传输。接受前，请在另一台设备上确认。",
                    language: language
                )),
                primaryButton: .default(Text(AppText.value("Accept", "接受", language: language))) {
                    acceptPendingOffer(pending)
                },
                secondaryButton: .cancel(Text(AppText.value("Reject", "拒绝", language: language))) {
                    workflow.discardPendingOffer(id: pending.id)
                }
            )
        }
        .alert(
            AppText.value("Close this one-time room?", "关闭这个一次性房间？", language: language),
            isPresented: $isCloseRoomConfirmationPresented
        ) {
            Button(AppText.value("Cancel", "取消", language: language), role: .cancel) {}
            Button(AppText.value("Close room", "关闭房间", language: language), role: .destructive) {
                closeRoomNow()
            }
        } message: {
            Text(AppText.value(
                "Active transfers will continue in Activity. You can monitor or stop them there.",
                "进行中的传输会继续，并可在“活动”页面中查看或停止。",
                language: language
            ))
        }
        .onAppear {
            prepareUITestFixtures()
            presentPendingSendSelection()
            updateDiscoveryLease()
        }
        .onDisappear(perform: nearbyCoordinator.stop)
        .onOpenURL(perform: handleIncomingURL)
        .onChange(of: page) { _ in updateDiscoveryLease() }
        .onChange(of: scenePhase) { phase in
            #if DEBUG
            if phase == .background {
                stageBackgroundShareFixtureIfRequested()
            }
            #endif
            if phase == .active {
                presentPendingSendSelection()
            }
            updateDiscoveryLease()
        }
        .onChange(of: nearbyCoordinator.state.incomingRendezvousOffer?.id) { _ in
            captureIncomingNearbyOffer()
        }
        .onChange(of: model.send.transferActivity?.activityId) { activityID in
            guard let activityID,
                  transferRoomID == workflow.room?.id else { return }
            preservedSendSelection = SendSelectionSnapshot()
            workflow.captureActivity(activityID)
            if transferRoute == .send {
                transferRoute = nil
            }
        }
        .onChange(of: model.receive.transferActivity?.activityId) { activityID in
            guard let activityID,
                  transferRoomID == workflow.room?.id else { return }
            workflow.captureActivity(activityID)
            if transferRoute == .receive {
                transferRoute = nil
            }
        }
    }

    @ViewBuilder
    private var pageContent: some View {
        switch page {
        case .connect:
            ConnectionHubView(
                coordinator: nearbyCoordinator,
                openInFixtureURL: debugOpenInFixtureURL,
                onScanQRCode: { scannerIsPresented = true },
                onShowQRCode: openRoomForShowingCode,
                onEnterCode: {
                    manualPairingInput = ""
                    manualEntryIsPresented = true
                },
                onSelectPeer: openNearbyRoom
            )
        case .room:
            if let room = workflow.room {
                OneTimeRoomView(
                    room: room,
                    records: roomActivityRecords(room),
                    selectedPeerIsVisible: selectedPeerIsVisible(room),
                    discoveryIsActive: nearbyCoordinator.state.isActive,
                    onAddFiles: offerFiles,
                    onReceiveFiles: receiveFiles,
                    onShowActivity: { showPage(.activity) },
                    onClose: requestCloseRoom
                )
            } else {
                Color.clear
                    .onAppear { page = .connect }
            }
        case .activity:
            activityPage
        case .settings:
            ScrollView {
                SettingsStageView()
                    .padding(.horizontal, 16)
                    .padding(.vertical, 12)
            }
            .accessibilityIdentifier("settings_page")
        }
    }

    private var activityPage: some View {
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
        .padding(.horizontal, 16)
        .padding(.vertical, 8)
        .accessibilityIdentifier("activity_page")
    }

    @ToolbarContentBuilder
    private var toolbarContent: some ToolbarContent {
        if page != .connect {
            ToolbarItem(placement: .topBarLeading) {
                Button(action: navigateBack) {
                    Image(systemName: "chevron.left")
                        .font(.body.weight(.semibold))
                        .frame(width: 40, height: 40)
                }
                .accessibilityLabel(AppText.value("Back", "返回", language: language))
                .accessibilityIdentifier("mobile_page_back")
            }
        }

        ToolbarItemGroup(placement: .topBarTrailing) {
            if page != .activity {
                Button {
                    showPage(.activity)
                } label: {
                    Image(systemName: "clock.arrow.circlepath")
                        .font(.body.weight(.semibold))
                }
                .accessibilityLabel(AppText.value("Activity", "活动", language: language))
                .accessibilityIdentifier("open_activity")
            }

            if page != .settings {
                Button {
                    showPage(.settings)
                } label: {
                    Image(systemName: "gearshape")
                        .font(.body.weight(.semibold))
                }
                .accessibilityLabel(AppText.value("Settings", "设置", language: language))
                .accessibilityIdentifier("open_settings")
            }
        }
    }

    @ViewBuilder
    private func transferContent(_ route: MobileTransferRoute) -> some View {
        switch route {
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
                nearbySelection: workflow.room?.nearbySelection,
                nearbyInviteOffer: nearbyInviteOffer,
                onInitialPairingInputConsumed: { pendingSendPairingInput = nil },
                onSwitchToReceive: switchToReceive
            )
        case .receive:
            ReceiveView(
                viewModel: model.receive,
                initialPairingInput: pendingReceivePairingInput,
                nearbySelection: workflow.room?.nearbySelection,
                nearbyInviteOffer: nearbyInviteOffer,
                onInitialPairingInputConsumed: { pendingReceivePairingInput = nil },
                onSwitchToSend: switchToSend
            )
        }
    }

    private var pageTitle: String {
        switch page {
        case .connect: return "Envoix"
        case .room: return AppText.value("One-time Room", "一次性房间", language: language)
        case .activity: return AppText.value("Activity", "活动", language: language)
        case .settings: return AppText.value("Settings", "设置", language: language)
        }
    }

    private func transferTitle(_ route: MobileTransferRoute) -> String {
        switch route {
        case .send: return AppText.value("Offer files", "发送文件", language: language)
        case .receive: return AppText.value("Receive files", "接收文件", language: language)
        }
    }

    private var pendingOfferBinding: Binding<PendingNearbyInvitation?> {
        Binding(
            get: { workflow.nextPendingOffer },
            set: { value in
                if value == nil, let pending = workflow.nextPendingOffer {
                    workflow.discardPendingOffer(id: pending.id)
                }
            }
        )
    }

    private var nearbyInviteOffer: NearbyInviteOffer? {
        guard !transferUsesInboundInvite,
              let selection = workflow.room?.nearbySelection,
              selection.sources.contains(.bluetooth) else {
            return nil
        }
        return NearbyInviteOffer { invite, completion in
            nearbyCoordinator.offerInvite(
                peerKey: selection.discoveryPeerKey,
                invite: invite
            ) { error in
                completion(error)
            }
        }
    }

    private func openNearbyRoom(_ selection: NearbyPairingSelection) {
        workflow.discardAllPendingOffers()
        workflow.openRoom(
            origin: .nearby(selection),
            existingActivityIDs: Set(model.activities.map(\.activityId))
        )
        resetRoomTransferHandoff()
        page = .room
    }

    private func openRoomForShowingCode() {
        workflow.discardAllPendingOffers()
        workflow.openRoom(
            origin: .showCode,
            suggestedAction: .receiveFiles,
            existingActivityIDs: Set(model.activities.map(\.activityId))
        )
        resetRoomTransferHandoff()
        page = .room
        DispatchQueue.main.async {
            receiveFiles()
        }
    }

    private func openPairingRoom(input: String) -> String? {
        let pairingInput = input.trimmed
        guard !pairingInput.isEmpty else {
            return AppText.value(
                "Enter an Envoix pairing code.",
                "请输入 Envoix 配对码。",
                language: language
            )
        }

        let action: OneTimeRoomAction
        if pairingInput.lowercased().hasPrefix("envoix:")
            && !pairingInput.lowercased().hasPrefix("envoix://pair/") {
            action = .offerFiles
        } else {
            do {
                action = ConnectionWorkflowPolicy.localAction(
                    for: try parsePairingInvite(input: pairingInput).role
                )
            } catch {
                return AppText.value(
                    "This is not a valid Envoix pairing code.",
                    "这不是有效的 Envoix 配对码。",
                    language: language
                )
            }
        }

        workflow.discardAllPendingOffers()
        workflow.openRoom(
            origin: .pairingCode,
            pairingInput: pairingInput,
            suggestedAction: action,
            existingActivityIDs: Set(model.activities.map(\.activityId))
        )
        resetRoomTransferHandoff()
        page = .room
        DispatchQueue.main.async {
            switch action {
            case .offerFiles: offerFiles()
            case .receiveFiles: receiveFiles()
            case .choose: break
            }
        }
        return nil
    }

    private func offerFiles() {
        guard let room = workflow.room else { return }
        transferRoomID = room.id
        roomOwnsSendDraft = true
        transferUsesInboundInvite = room.pairingInput != nil && room.suggestedAction == .offerFiles
        pendingSendPairingInput = room.suggestedAction == .receiveFiles ? nil : room.pairingInput
        transferRoute = .send
    }

    private func receiveFiles() {
        guard let room = workflow.room else { return }
        transferRoomID = room.id
        transferUsesInboundInvite = room.pairingInput != nil && room.suggestedAction == .receiveFiles
        pendingReceivePairingInput = room.suggestedAction == .offerFiles ? nil : room.pairingInput
        transferRoute = .receive
    }

    private func switchToReceive(_ input: String, selection: SendSelectionSnapshot) {
        preservedSendSelection = selection
        pendingReceivePairingInput = input
        transferUsesInboundInvite = true
        transferRoute = .receive
    }

    private func switchToSend(_ input: String) {
        pendingSendPairingInput = input
        transferUsesInboundInvite = true
        transferRoute = .send
    }

    private func finishTransferSheetPresentation() {
        pendingSendPairingInput = nil
        pendingReceivePairingInput = nil
        transferUsesInboundInvite = false
        transferRoomID = nil
    }

    private func requestCloseRoom() {
        guard roomHasActiveTransfers else {
            closeRoomNow()
            return
        }
        isCloseRoomConfirmationPresented = true
    }

    private var roomHasActiveTransfers: Bool {
        guard let room = workflow.room else { return false }
        return model.activities.contains {
            room.activityIDs.contains($0.activityId)
                && ActivityProjectionPolicy.isPending($0.state)
        }
    }

    private func closeRoomNow() {
        transferRoute = nil
        if roomOwnsSendDraft, model.send.transferActivity == nil {
            _ = model.send.cancelManifestPreparation()
        }
        resetRoomTransferHandoff()
        workflow.closeRoom()
        page = .connect
    }

    private func resetRoomTransferHandoff() {
        preservedSendSelection = SendSelectionSnapshot()
        pendingSendPairingInput = nil
        pendingReceivePairingInput = nil
        transferUsesInboundInvite = false
        transferRoomID = nil
        roomOwnsSendDraft = false
    }

    private func roomActivityRecords(_ room: OneTimeRoomSession) -> [TransferActivityRecord] {
        model.activities.filter { room.activityIDs.contains($0.activityId) }
    }

    private func selectedPeerIsVisible(_ room: OneTimeRoomSession) -> Bool {
        guard let key = room.nearbySelection?.discoveryPeerKey else { return false }
        return nearbyCoordinator.state.peers.contains { $0.peerKey == key }
    }

    private func showPage(_ destination: MobilePage) {
        if page == .connect || page == .room {
            returnPage = page
        }
        page = destination
    }

    private func navigateBack() {
        switch page {
        case .room:
            requestCloseRoom()
        case .activity, .settings:
            page = returnPage == .room && workflow.room == nil ? .connect : returnPage
        case .connect:
            break
        }
    }

    private func updateDiscoveryLease() {
        let roomNeedsDiscovery = page == .room && workflow.room?.nearbySelection != nil
        if scenePhase != .background && (page == .connect || roomNeedsDiscovery) {
            nearbyCoordinator.start()
        } else {
            nearbyCoordinator.stop()
        }
    }

    private func captureIncomingNearbyOffer() {
        guard let offer = nearbyCoordinator.state.incomingRendezvousOffer else { return }
        defer { nearbyCoordinator.consumeRendezvousOffer(id: offer.id) }
        if page == .room,
           let selectedPeerKey = workflow.room?.nearbySelection?.discoveryPeerKey,
           offer.senderPeerKey != selectedPeerKey {
            return
        }
        guard (try? parsePairingInvite(input: offer.invite)) != nil else {
            ToastCenter.shared.show(AppText.value(
                "An invalid Bluetooth invitation was rejected.",
                "已拒绝无效的蓝牙邀请。",
                language: language
            ))
            return
        }
        guard workflow.enqueue(offer) else { return }
        DispatchQueue.main.asyncAfter(
            deadline: .now() + ConnectionWorkflowPolicy.offerLifetime
        ) {
            workflow.discardExpiredOffers()
        }
    }

    private func acceptPendingOffer(_ pending: PendingNearbyInvitation) {
        workflow.discardAllPendingOffers()
        guard let parsed = try? parsePairingInvite(input: pending.offer.invite) else { return }
        let selection = NearbyPairingSelection(
            discoveryPeerKey: pending.offer.senderPeerKey,
            displayName: pending.offer.senderDisplayName,
            sources: [.bluetooth]
        )
        let action = ConnectionWorkflowPolicy.localAction(for: parsed.role)
        workflow.acceptNearbyOffer(
            selection: selection,
            pairingInput: pending.offer.invite,
            suggestedAction: action,
            existingActivityIDs: Set(model.activities.map(\.activityId))
        )
        resetRoomTransferHandoff()
        page = .room
        DispatchQueue.main.async {
            switch action {
            case .offerFiles: offerFiles()
            case .receiveFiles: receiveFiles()
            case .choose: break
            }
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
                openExternalShareRoom()
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
            openExternalShareRoom()
            return
        }
        presentSharedDraft(preferredID: nil)
    }

    private func presentSharedDraft(preferredID: UUID?) {
        do {
            switch try model.importSharedSendDraft(preferredID: preferredID) {
            case .imported:
                openExternalShareRoom()
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

    private func openExternalShareRoom() {
        if workflow.room == nil {
            workflow.discardAllPendingOffers()
            workflow.openRoom(
                origin: .externalShare,
                suggestedAction: .offerFiles,
                existingActivityIDs: Set(model.activities.map(\.activityId))
            )
        }
        roomOwnsSendDraft = true
        page = .room
        transferRoomID = workflow.room?.id
        transferRoute = .send
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
    private var debugOpenInFixtureURL: URL? { openInUITestFixtureURL }

    private func prepareUITestFixtures() {
        FolderPickerUITestFixture.cleanIfRequested()
        FilePickerUITestFixture.cleanIfRequested()
        FilePickerUITestFixture.stageIfRequested()
        OpenInUITestFixture.cleanIfRequested()
        openInUITestFixtureURL = OpenInUITestFixture.stageIfRequested()
    }

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
    #else
    private var debugOpenInFixtureURL: URL? { nil }
    private func prepareUITestFixtures() {}
    #endif
}

private struct ManualPairingCodeSheet: View {
    @Environment(\.dismiss) private var dismiss

    let language: String
    @Binding var input: String
    let onSubmit: (String) -> String?

    @State private var error: String?

    var body: some View {
        NavigationStack {
            VStack(alignment: .leading, spacing: 16) {
                Text(AppText.value(
                    "Enter the code shown on the other device. The code opens a one-time room; it does not identify or trust that device.",
                    "输入另一台设备显示的配对码。配对码只会打开一次性房间，不代表设备身份或信任关系。",
                    language: language
                ))
                .font(.subheadline)
                .foregroundStyle(Theme.muted)
                .fixedSize(horizontal: false, vertical: true)

                TextField(
                    AppText.value("Pairing code", "配对码", language: language),
                    text: $input,
                    axis: .vertical
                )
                .textInputAutocapitalization(.never)
                .autocorrectionDisabled()
                .textFieldStyle(.roundedBorder)
                .accessibilityIdentifier("manual_pairing_code_input")

                if let error {
                    Text(error)
                        .font(.footnote)
                        .foregroundStyle(Theme.danger)
                        .fixedSize(horizontal: false, vertical: true)
                        .accessibilityIdentifier("manual_pairing_code_error")
                }

                Button {
                    error = onSubmit(input)
                } label: {
                    Text(AppText.value("Open room", "打开房间", language: language))
                        .frame(maxWidth: .infinity, minHeight: 44)
                }
                .buttonStyle(PrimaryActionButtonStyle())
                .disabled(input.trimmed.isEmpty)
                .accessibilityIdentifier("manual_pairing_code_submit")

                Spacer()
            }
            .padding(20)
            .background(Theme.bg)
            .navigationTitle(AppText.value("Enter code", "输入配对码", language: language))
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button(AppText.value("Close", "关闭", language: language)) {
                        dismiss()
                    }
                }
            }
        }
        .presentationDetents([.medium, .large])
    }
}
#endif
