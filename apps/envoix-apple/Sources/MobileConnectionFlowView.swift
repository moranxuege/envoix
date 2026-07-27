#if os(iOS)
import Combine
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

enum MobileSceneLifecycleEvent {
    case active
    case inactive
    case background

    init(scenePhase: ScenePhase) {
        switch scenePhase {
        case .active: self = .active
        case .inactive: self = .inactive
        case .background: self = .background
        @unknown default: self = .inactive
        }
    }
}

struct MobileSceneLifecycleEffects: Equatable {
    let shouldPresentPendingSendSelection: Bool
    let shouldHideRoomInvitation: Bool
    let allowsNearbyDiscovery: Bool
}

enum MobileSceneLifecyclePolicy {
    static func effects(for event: MobileSceneLifecycleEvent) -> MobileSceneLifecycleEffects {
        switch event {
        case .active:
            return MobileSceneLifecycleEffects(
                shouldPresentPendingSendSelection: true,
                shouldHideRoomInvitation: false,
                allowsNearbyDiscovery: true
            )
        case .inactive:
            return MobileSceneLifecycleEffects(
                shouldPresentPendingSendSelection: false,
                shouldHideRoomInvitation: false,
                allowsNearbyDiscovery: false
            )
        case .background:
            return MobileSceneLifecycleEffects(
                shouldPresentPendingSendSelection: false,
                shouldHideRoomInvitation: true,
                allowsNearbyDiscovery: false
            )
        }
    }
}

enum RememberedRoomLifecyclePolicy {
    static func shouldKeepConnected(
        sceneIsActive: Bool,
        externalActivityActive: Bool
    ) -> Bool {
        sceneIsActive || externalActivityActive
    }
}

struct RoomDestinationRepairRequest: Equatable, Identifiable {
    let offerID: String
    let roomID: UUID

    var id: String { "\(roomID.uuidString):\(offerID)" }

    func matches(offerID: String, roomID: UUID) -> Bool {
        self.offerID == offerID && self.roomID == roomID
    }
}

struct MobileConnectionFlowView: View {
    private typealias PreparedRoomDestination = (url: URL, access: AnyObject?)

    @EnvironmentObject private var model: AppModel
    @Environment(\.scenePhase) private var scenePhase
    @AppStorage("envoix.language") private var language = "en"
    @AppStorage("envoix.serverURL") private var serverURL = ""
    @AppStorage("envoix.relayURL") private var relayURL = ""
    @AppStorage("envoix.concurrentTransfers") private var concurrentTransfers = true
    @AppStorage("envoix.candidatesAllow") private var candidatesAllow = ""
    @AppStorage("envoix.candidatesDeny") private var candidatesDeny = ""
    @AppStorage("envoix.speedLimit") private var speedLimit = 40
    @AppStorage("envoix.outputDirDisplayName") private var outputDirDisplayName = ""

    @StateObject private var nearbyCoordinator = NearbyDiscoveryCoordinator()
    @StateObject private var presence = NearbyPresencePreferences()
    @StateObject private var workflow = ConnectionWorkflowState(
        gateway: RoomControlGatewayFactory.make()
    )
    @StateObject private var rememberedOutbox = RememberedRoomOutboxController()
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
    @State private var transferUsesRoomControl = false
    @State private var transferRememberedRelationshipID: String?
    @State private var transferRoomID: UUID?
    @State private var transferHasStarted = false
    @State private var transferSheetDismissalBlocked = false
    @State private var transferExternalActivityActive = false
    @State private var roomOwnsSendDraft = false
    @State private var scannerIsPresented = false
    @State private var manualEntryIsPresented = false
    @State private var manualPairingInput = ""
    @State private var isCloseRoomConfirmationPresented = false
    @State private var roomInvitationIsRevealed = false
    @State private var now = Date()
    @State private var pendingRoomReplacement: (() -> Void)?
    @State private var isRoomReplacementPresented = false
    @State private var acceptingRoomOfferID: String?
    @State private var roomDestinationRepair: RoomDestinationRepairRequest?
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
                            .disabled(transferSheetDismissalBlocked)
                            .accessibilityLabel(AppText.value("Close", "关闭", language: language))
                            .accessibilityIdentifier("mobile_sheet_done")
                        }
                    }
            }
            .presentationDragIndicator(.visible)
            .presentationDetents([.large])
            .interactiveDismissDisabled(transferSheetDismissalBlocked)
        }
        .sheet(item: $roomDestinationRepair) { request in
            FolderPickerSheet(
                onPick: { url in
                    completeRoomDestinationRepair(url: url, request: request)
                },
                onCancel: {
                    guard roomDestinationRepair == request else { return }
                    roomDestinationRepair = nil
                    workflow.resumeIncomingRoomOfferDeadline(id: request.offerID)
                }
            )
        }
        .fullScreenCover(isPresented: $scannerIsPresented) {
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
            let isRoomInvite = isRoomControlInput(pending.offer.invite)
            return Alert(
                title: Text(AppText.value(
                    isRoomInvite
                        ? "Unverified nearby room invitation"
                        : "Unverified nearby invitation",
                    isRoomInvite ? "未经验证的附近房间邀请" : "未经验证的附近设备邀请",
                    language: language
                )),
                message: Text(AppText.value(
                    isRoomInvite
                        ? "\(pending.offer.senderDisplayName ?? "A nearby device") wants to open a room. Confirm on the other device before accepting."
                        : "\(pending.offer.senderDisplayName ?? "A nearby device") wants to start a one-time transfer. Confirm on the other device before accepting.",
                    isRoomInvite
                        ? "\(pending.offer.senderDisplayName ?? "附近设备") 希望打开一个房间。接受前，请在另一台设备上确认。"
                        : "\(pending.offer.senderDisplayName ?? "附近设备") 希望开始一次性传输。接受前，请在另一台设备上确认。",
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
            AppText.value("End this room?", "结束这个房间？", language: language),
            isPresented: $isCloseRoomConfirmationPresented
        ) {
            Button(AppText.value("Cancel", "取消", language: language), role: .cancel) {}
            Button(AppText.value("End room", "结束房间", language: language), role: .destructive) {
                closeRoomNow()
            }
        } message: {
            Text(AppText.value(
                "Active transfers will continue in Activity. You can monitor or stop them there.",
                "进行中的传输会继续，并可在“活动”页面中查看或停止。",
                language: language
            ))
        }
        .alert(
            AppText.value("A room is already open", "已有一个房间", language: language),
            isPresented: $isRoomReplacementPresented
        ) {
            Button(AppText.value("Return to room", "返回房间", language: language)) {
                pendingRoomReplacement = nil
                if workflow.activeRoomID != nil {
                    page = .room
                }
            }
            Button(AppText.value("End and replace", "结束并替换", language: language), role: .destructive) {
                let action = pendingRoomReplacement
                pendingRoomReplacement = nil
                closeRoomNow()
                action?()
            }
            Button(AppText.value("Cancel", "取消", language: language), role: .cancel) {
                pendingRoomReplacement = nil
            }
        } message: {
            Text(AppText.value(
                "Envoix can keep one room at a time.",
                "Envoix 一次只能保留一个房间。",
                language: language
            ))
        }
        .onAppear {
            prepareUITestFixtures()
            presentPendingSendSelection()
            workflow.refreshRememberedRooms()
            rememberedOutbox.start()
            updateDiscoveryLease()
            updateRememberedReconnect()
            synchronizeRememberedOutbox()
        }
        .onDisappear {
            nearbyCoordinator.stop()
            workflow.setRememberedReconnectEnabled(
                false,
                displayName: presence.displayName,
                identityPath: roomIdentityPath ?? ""
            )
        }
        .onOpenURL(perform: handleIncomingURL)
        .onChange(of: page) { _ in updateDiscoveryLease() }
        .onChange(of: scenePhase) { phase in
            let effects = MobileSceneLifecyclePolicy.effects(
                for: MobileSceneLifecycleEvent(scenePhase: phase)
            )
            #if DEBUG
            if phase == .background {
                stageBackgroundShareFixtureIfRequested()
            }
            #endif
            if effects.shouldPresentPendingSendSelection {
                presentPendingSendSelection()
            }
            if effects.shouldHideRoomInvitation {
                roomInvitationIsRevealed = false
            }
            // A live control room survives scene backgrounding. Its explicit
            // end action, negotiated idle lifetime, or connection loss owns
            // termination; the scene transition only withdraws discovery UI.
            updateDiscoveryLease()
            updateRememberedReconnect()
            synchronizeRememberedOutbox()
        }
        .onReceive(Timer.publish(every: 1, on: .main, in: .common).autoconnect()) { date in
            now = date
            if presence.expireIfNeeded(now: date) {
                updateDiscoveryLease()
            }
            workflow.tick(now: date, hasActiveTransfer: roomHasActiveTransfers)
        }
        .onReceive(model.$activities) { _ in
            workflow.setLocalTransferActive(roomHasActiveTransfers)
        }
        .onReceive(NotificationCenter.default.publisher(
            for: RememberedRoomOutboxStore.didChange
        )) { _ in
            rememberedOutbox.refresh()
        }
        .onChange(of: rememberedOutbox.entries) { _ in
            synchronizeRememberedOutbox()
        }
        .onChange(of: model.send.presentationState) { state in
            rememberedOutbox.handleSendState(
                state,
                workflow: workflow,
                model: model
            )
            if state == .delivered || state == .failed || state == .canceled {
                workflow.refreshRememberedRooms()
                updateRememberedReconnect()
            }
            synchronizeRememberedOutbox()
        }
        .onChange(of: model.receive.presentationState) { state in
            if state == .delivered || state == .failed || state == .canceled {
                workflow.refreshRememberedRooms()
                updateRememberedReconnect()
            }
        }
        .onChange(of: presence.displayName) { _ in
            updateDiscoveryLease()
            updateRememberedReconnect()
        }
        .onChange(of: presence.visibility) { _ in updateDiscoveryLease() }
        .onChange(of: workflow.controlPhase) { phase in
            if phase == .connected,
               workflow.room != nil {
                roomInvitationIsRevealed = false
                page = .room
                workflow.setLocalTransferActive(roomHasActiveTransfers)
            } else if phase == .connected,
                      workflow.rememberedRoom != nil {
                roomInvitationIsRevealed = false
                workflow.setLocalTransferActive(roomHasActiveTransfers)
            } else if phase != .hosting {
                roomInvitationIsRevealed = false
            }
            if case .failed(let message) = phase {
                ToastCenter.shared.show(message)
            }
            if isEndedOrFailed(phase),
               transferRoute != nil,
               transferUsesRoomControl,
               transferRememberedRelationshipID == nil {
                transferRoute = nil
            }
            if isEndedOrFailed(phase) {
                roomDestinationRepair = nil
            }
            synchronizeRememberedOutbox()
        }
        .onChange(of: workflow.incomingRoomOffer?.id) { offerID in
            if let request = roomDestinationRepair, request.offerID != offerID {
                roomDestinationRepair = nil
            }
        }
        .onChange(of: nearbyCoordinator.state.incomingRendezvousOffer?.id) { _ in
            captureIncomingNearbyOffer()
        }
        .onChange(of: model.send.transferActivity?.activityId) { activityID in
            guard let activityID,
                  transferRoomID == workflow.activeRoomID else { return }
            transferHasStarted = true
            preservedSendSelection = SendSelectionSnapshot()
            workflow.captureActivity(activityID)
            workflow.setLocalTransferActive(roomHasActiveTransfers)
            if transferRoute == .send {
                transferRoute = nil
            }
        }
        .onChange(of: model.receive.transferActivity?.activityId) { activityID in
            guard let activityID,
                  transferRoomID == workflow.activeRoomID else { return }
            transferHasStarted = true
            workflow.captureActivity(activityID)
            workflow.setLocalTransferActive(roomHasActiveTransfers)
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
                presence: presence,
                openInFixtureURL: debugOpenInFixtureURL,
                roomInvitation: workflow.roomInvitation,
                roomInvitationIsRevealed: roomInvitationIsRevealed,
                roomInvitationIsStarting: workflow.controlPhase == .joining,
                rememberedRooms: workflow.rememberedPeers,
                rememberedRoomStatus: workflow.rememberedRoomStatus,
                incomingRememberedRelationshipID: workflow.incomingRoomOffer == nil
                    ? nil
                    : workflow.activeRememberedRelationshipID,
                onScanQRCode: {
                    guardRoomReplacement {
                        scannerIsPresented = true
                    }
                },
                onEnterCode: {
                    guardRoomReplacement {
                        manualPairingInput = ""
                        manualEntryIsPresented = true
                    }
                },
                onRevealRoomInvitation: revealRoomInvitation,
                onHideRoomInvitation: { roomInvitationIsRevealed = false },
                onRefreshRoomInvitation: refreshRoomInvitation,
                onCancelRoomInvitation: {
                    roomInvitationIsRevealed = false
                    workflow.endControl(reason: .userEnded)
                },
                onSetVisibility: { presence.setVisibility($0) },
                onRename: updateDisplayName,
                onSelectRememberedRoom: openRememberedRoom,
                onSelectPeer: openNearbyRoom
            )
        case .room:
            if let room = workflow.rememberedRoom {
                RememberedRoomView(
                    room: room,
                    status: workflow.rememberedRoomStatus(
                        relationshipID: room.relationshipID
                    ),
                    peerDisplayName: workflow.peerDisplayName,
                    incomingOffer: workflow.incomingRoomOffer,
                    isAcceptingOffer: acceptingRoomOfferID != nil
                        || roomDestinationRepair != nil,
                    outboxEntries: rememberedOutbox.entries(
                        relationshipID: room.relationshipID
                    ),
                    outboxError: rememberedOutbox.errorMessage,
                    records: rememberedRoomActivityRecords(room),
                    onAddFiles: offerRememberedRoomFiles,
                    onAcceptOffer: acceptIncomingRoomOffer,
                    onRejectOffer: workflow.rejectIncomingRoomOffer,
                    onRetryOutboxEntry: retryRememberedOutboxEntry,
                    onRemoveOutboxEntry: removeRememberedOutboxEntry,
                    onShowActivity: { showPage(.activity) },
                    onDisconnect: workflow.disconnectRememberedRoom,
                    onForget: forgetCurrentRememberedRoom
                )
            } else if let room = workflow.room {
                OneTimeRoomView(
                    room: room,
                    records: roomActivityRecords(room),
                    controlPhase: workflow.controlPhase,
                    peerDisplayName: workflow.peerDisplayName,
                    incomingOffer: workflow.incomingRoomOffer,
                    isAcceptingOffer: acceptingRoomOfferID != nil
                        || roomDestinationRepair != nil,
                    isRoomCreator: workflow.isRoomCreator,
                    lifetimePolicy: workflow.roomLifetimePolicy,
                    idleDeadline: workflow.idleDeadline,
                    now: now,
                    selectedPeerIsVisible: selectedPeerIsVisible(room),
                    discoveryIsActive: nearbyCoordinator.state.isActive,
                    onAddFiles: offerFiles,
                    onAcceptOffer: acceptIncomingRoomOffer,
                    onRejectOffer: workflow.rejectIncomingRoomOffer,
                    onSetKeepOpen: workflow.setKeepOpen,
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
        if page == .connect {
            ToolbarItem(placement: .topBarLeading) {
                Button {
                    showPage(.activity)
                } label: {
                    Image(systemName: "clock.arrow.circlepath")
                        .font(.body.weight(.semibold))
                        .frame(width: 40, height: 40)
                }
                .accessibilityLabel(AppText.value("Activity", "活动", language: language))
                .accessibilityIdentifier("open_activity")
            }
        } else {
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

        if page != .connect {
            ToolbarItem(placement: .topBarTrailing) {
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
            }
        }

        ToolbarItem(placement: .topBarTrailing) {
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
                roomControlOffer: transferUsesRoomControl
                    && transferRememberedRelationshipID == nil
                    ? { offer, completion in
                        workflow.offerTransfer(offer, onDecision: completion)
                    }
                    : nil,
                roomControlEndpoint: transferUsesRoomControl
                    ? workflow.activeRoomEndpoint
                    : nil,
                rememberedRoomRelationshipID: transferRememberedRelationshipID,
                onRoomOfferPendingChange: { transferSheetDismissalBlocked = $0 },
                onRememberedRoomQueued: finishQueueingRememberedRoomFiles,
                onInitialPairingInputConsumed: { pendingSendPairingInput = nil },
                onSwitchToReceive: switchToReceive,
                onExternalActivityChanged: setTransferExternalActivityActive
            )
        case .receive:
            ReceiveView(
                viewModel: model.receive,
                initialPairingInput: pendingReceivePairingInput,
                nearbySelection: workflow.room?.nearbySelection,
                nearbyInviteOffer: nearbyInviteOffer,
                roomControlTransfer: transferUsesRoomControl && transferUsesInboundInvite,
                roomControlAccept: transferUsesRoomControl && transferUsesInboundInvite
                    ? {
                        await workflow.acceptIncomingRoomOffer() != nil
                    }
                    : nil,
                onInitialPairingInputConsumed: { pendingReceivePairingInput = nil },
                onSwitchToSend: switchToSend
            )
        }
    }

    private var pageTitle: String {
        switch page {
        case .connect: return "Envoix"
        case .room: return AppText.value("Room", "房间", language: language)
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
              let selection = workflow.room?.nearbySelection else {
            return nil
        }
        return NearbyInviteOffer { invite, completion in
            nearbyCoordinator.offerInvite(
                to: selection,
                invite: invite
            ) { error in
                completion(error)
            }
        }
    }

    private func openNearbyRoom(_ selection: NearbyPairingSelection) {
        if workflow.controlPhase == .hosting,
           let payload = workflow.roomInvitation?.payload {
            deliverRoomInvitation(payload, to: selection)
            return
        }
        guardRoomReplacement {
            guard startHostingRoom() else { return }
            guard let payload = workflow.roomInvitation?.payload else { return }
            deliverRoomInvitation(payload, to: selection)
        }
    }

    private func deliverRoomInvitation(
        _ payload: String,
        to selection: NearbyPairingSelection
    ) {
        nearbyCoordinator.offerInvite(
            to: selection,
            invite: payload
        ) { error in
            if let error {
                ToastCenter.shared.show(error)
            }
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
        if isRoomControlInput(pairingInput) {
            guard !isRoomOccupied else {
                return AppText.value(
                    "End the current room before joining another one.",
                    "请先结束当前房间，再加入另一个房间。",
                    language: language
                )
            }
            guard let identityPath = roomIdentityPath else {
                return AppText.value(
                    "Application Support is unavailable.",
                    "无法访问应用支持目录。",
                    language: language
                )
            }
            let error = workflow.joinRoomControl(
                input: pairingInput,
                broker: serverURL,
                relay: relayURL,
                displayName: presence.displayName,
                identityPath: identityPath,
                existingActivityIDs: Set(model.activities.map(\.activityId))
            )
            if error == nil {
                resetRoomTransferHandoff()
            }
            return error
        }

        let action: OneTimeRoomAction
        do {
            if pairingInput.lowercased().hasPrefix("envoix:") {
                let invitation = try parsePairingInvite(input: pairingInput)
                action = ConnectionWorkflowPolicy.localAction(
                    forLocalRole: invitation.joinerRole
                )
            } else {
                _ = try normalizeRoomCode(input: pairingInput)
                action = .choose
            }
        } catch {
            return AppText.value(
                "This is not a valid Envoix InviteV2 link or Room Code.",
                "这不是有效的 Envoix InviteV2 链接或房间码。",
                language: language
            )
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
        transferHasStarted = false
        transferUsesRoomControl = room.origin == .roomControl
        transferRememberedRelationshipID = nil
        roomOwnsSendDraft = true
        transferUsesInboundInvite = room.pairingInput != nil && room.suggestedAction == .offerFiles
        pendingSendPairingInput = room.suggestedAction == .receiveFiles ? nil : room.pairingInput
        transferRoute = .send
    }

    private func offerRememberedRoomFiles() {
        guard let room = workflow.rememberedRoom else { return }
        transferRoomID = room.id
        transferHasStarted = false
        transferUsesRoomControl = true
        transferUsesInboundInvite = false
        transferRememberedRelationshipID = room.relationshipID
        roomOwnsSendDraft = true
        pendingSendPairingInput = nil
        transferRoute = .send
    }

    private func openRememberedRoom(_ relationshipID: String) {
        if workflow.activeRememberedRelationshipID == relationshipID
            || workflow.rememberedRoom?.relationshipID == relationshipID {
            openRememberedRoomNow(relationshipID)
            return
        }
        guardRoomReplacement {
            openRememberedRoomNow(relationshipID)
        }
    }

    private func openRememberedRoomNow(_ relationshipID: String) {
        if let error = workflow.openRememberedRoom(
            relationshipID: relationshipID,
            existingActivityIDs: Set(model.activities.map(\.activityId))
        ) {
            ToastCenter.shared.show(error)
            return
        }
        resetRoomTransferHandoff()
        page = .room
    }

    private func forgetCurrentRememberedRoom() {
        guard let relationshipID = workflow.rememberedRoom?.relationshipID else { return }
        Task { @MainActor in
            let preparation = await rememberedOutbox.removeAll(
                relationshipID: relationshipID,
                model: model
            )
            let cleanupWarning: String?
            switch preparation {
            case .ready(let warning):
                cleanupWarning = warning
            case .blocked(let error):
                ToastCenter.shared.show(error)
                return
            }
            if let error = await workflow.forgetRememberedRoom(
                relationshipID: relationshipID
            ) {
                ToastCenter.shared.show(error)
                return
            }
            resetRoomTransferHandoff()
            page = .connect
            if let cleanupWarning {
                ToastCenter.shared.show(cleanupWarning)
            }
        }
    }

    private func finishQueueingRememberedRoomFiles() {
        roomOwnsSendDraft = false
        transferSheetDismissalBlocked = false
        rememberedOutbox.refresh()
        synchronizeRememberedOutbox()
        transferRoute = nil
        ToastCenter.shared.show(AppText.value(
            "Files added. Envoix will send when the room reconnects.",
            "文件已加入；房间重连后会自动发送。",
            language: language
        ))
    }

    private func retryRememberedOutboxEntry(_ id: String) {
        rememberedOutbox.retry(id: id)
        synchronizeRememberedOutbox()
    }

    private func removeRememberedOutboxEntry(
        _ entry: RememberedRoomOutboxEntry
    ) {
        Task { @MainActor in
            if let error = await rememberedOutbox.remove(entry, model: model) {
                ToastCenter.shared.show(error)
            }
            synchronizeRememberedOutbox()
        }
    }

    private func receiveFiles() {
        guard let room = workflow.room else { return }
        transferRoomID = room.id
        transferHasStarted = false
        transferUsesRoomControl = room.origin == .roomControl
        transferRememberedRelationshipID = nil
        transferUsesInboundInvite = room.pairingInput != nil && room.suggestedAction == .receiveFiles
        pendingReceivePairingInput = room.suggestedAction == .offerFiles ? nil : room.pairingInput
        transferRoute = .receive
    }

    private func switchToReceive(_ input: String, selection: SendSelectionSnapshot) {
        preservedSendSelection = selection
        pendingReceivePairingInput = input
        transferUsesInboundInvite = true
        transferRememberedRelationshipID = nil
        transferRoute = .receive
    }

    private func switchToSend(_ input: String) {
        pendingSendPairingInput = input
        transferUsesInboundInvite = true
        transferRememberedRelationshipID = nil
        transferRoute = .send
    }

    private func finishTransferSheetPresentation() {
        pendingSendPairingInput = nil
        pendingReceivePairingInput = nil
        transferUsesInboundInvite = false
        transferUsesRoomControl = false
        transferRememberedRelationshipID = nil
        transferRoomID = nil
        transferHasStarted = false
        transferSheetDismissalBlocked = false
    }

    private func requestCloseRoom() {
        guard roomHasActiveTransfers else {
            closeRoomNow()
            return
        }
        isCloseRoomConfirmationPresented = true
    }

    private var roomHasActiveTransfers: Bool {
        let activityIDs = workflow.rememberedRoom?.activityIDs
            ?? workflow.room?.activityIDs
            ?? []
        return model.activities.contains {
            activityIDs.contains($0.activityId)
                && ActivityProjectionPolicy.isPending($0.state)
        }
    }

    private func closeRoomNow() {
        transferRoute = nil
        if roomOwnsSendDraft, model.send.transferActivity == nil {
            _ = model.send.cancelManifestPreparation()
        }
        resetRoomTransferHandoff()
        if isControlRoomOpen || workflow.room?.origin == .roomControl {
            workflow.endControl(reason: .userEnded)
        } else {
            workflow.closeRoom()
        }
        page = .connect
    }

    private func resetRoomTransferHandoff() {
        preservedSendSelection = SendSelectionSnapshot()
        pendingSendPairingInput = nil
        pendingReceivePairingInput = nil
        transferUsesInboundInvite = false
        transferUsesRoomControl = false
        transferRememberedRelationshipID = nil
        transferRoomID = nil
        transferHasStarted = false
        transferSheetDismissalBlocked = false
        roomOwnsSendDraft = false
        roomDestinationRepair = nil
    }

    private func roomActivityRecords(_ room: OneTimeRoomSession) -> [TransferActivityRecord] {
        model.activities.filter { room.activityIDs.contains($0.activityId) }
    }

    private func rememberedRoomActivityRecords(
        _ room: RememberedRoomSession
    ) -> [TransferActivityRecord] {
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
            if workflow.rememberedRoom != nil {
                workflow.unpinRememberedRoom()
                page = .connect
            } else {
                requestCloseRoom()
            }
        case .activity, .settings:
            page = returnPage == .room && workflow.activeRoomID == nil
                ? .connect
                : returnPage
        case .connect:
            break
        }
    }

    private func updateDiscoveryLease() {
        let effects = MobileSceneLifecyclePolicy.effects(
            for: MobileSceneLifecycleEvent(scenePhase: scenePhase)
        )
        #if DEBUG
        // XCTest can report an inactive initial scene for the entire launch.
        // Fixture providers are process-local and safe to keep active so UI
        // tests exercise deterministic discovery instead of scene timing.
        let sceneAllowsDiscovery = effects.allowsNearbyDiscovery
            || ProcessInfo.processInfo.arguments.contains("--ui-testing")
        #else
        let sceneAllowsDiscovery = effects.allowsNearbyDiscovery
        #endif
        nearbyCoordinator.configure(
            displayName: presence.displayName,
            advertisingEnabled: presence.isAdvertising(sceneIsActive: sceneAllowsDiscovery)
        )
        if sceneAllowsDiscovery && page == .connect {
            nearbyCoordinator.start()
        } else {
            nearbyCoordinator.stop()
        }
    }

    private func updateRememberedReconnect() {
        workflow.setRememberedReconnectEnabled(
            RememberedRoomLifecyclePolicy.shouldKeepConnected(
                sceneIsActive: scenePhase == .active,
                externalActivityActive: transferExternalActivityActive
            ),
            displayName: presence.displayName,
            identityPath: roomIdentityPath ?? ""
        )
    }

    private func setTransferExternalActivityActive(_ active: Bool) {
        guard transferExternalActivityActive != active else { return }
        transferExternalActivityActive = active
        updateRememberedReconnect()
    }

    private func synchronizeRememberedOutbox() {
        workflow.setQueuedRememberedRelationships(
            rememberedOutbox.queuedRelationshipIDs
        )
        guard scenePhase == .active,
              workflow.controlPhase == .connected,
              let room = workflow.rememberedRoom,
              let endpoint = workflow.activeRoomEndpoint,
              room.relationshipID == workflow.activeRememberedRelationshipID else {
            return
        }
        rememberedOutbox.dispatchIfPossible(
            workflow: workflow,
            model: model,
            endpoint: endpoint,
            concurrentTransfers: concurrentTransfers,
            language: language,
            candidatesAllow: candidatesAllow,
            candidatesDeny: candidatesDeny,
            speedLimit: speedLimit
        )
    }

    private func captureIncomingNearbyOffer() {
        guard let offer = nearbyCoordinator.state.incomingRendezvousOffer else { return }
        defer { nearbyCoordinator.consumeRendezvousOffer(id: offer.id) }
        if page == .room,
           let selectedPeerKey = workflow.room?.nearbySelection?.discoveryPeerKey,
           offer.senderPeerKey != selectedPeerKey {
            return
        }
        guard isRoomControlInput(offer.invite)
                || (try? parsePairingInvite(input: offer.invite)) != nil else {
            ToastCenter.shared.show(AppText.value(
                "An invalid nearby invitation was rejected.",
                "已拒绝无效的附近设备邀请。",
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
        let continuesCurrentNearbyRoom = !isRoomControlInput(pending.offer.invite)
            && workflow.room?.nearbySelection?.discoveryPeerKey == pending.offer.senderPeerKey
        if continuesCurrentNearbyRoom {
            acceptPendingOfferNow(pending)
        } else {
            guardRoomReplacement {
                acceptPendingOfferNow(pending)
            }
        }
    }

    private func acceptPendingOfferNow(_ pending: PendingNearbyInvitation) {
        if isRoomControlInput(pending.offer.invite) {
            guard let identityPath = roomIdentityPath else { return }
            if let error = workflow.joinRoomControl(
                input: pending.offer.invite,
                broker: serverURL,
                relay: relayURL,
                displayName: presence.displayName,
                identityPath: identityPath,
                existingActivityIDs: Set(model.activities.map(\.activityId))
            ) {
                ToastCenter.shared.show(error)
            }
            return
        }
        guard let parsed = try? parsePairingInvite(input: pending.offer.invite) else { return }
        let capturedRoute = nearbyCoordinator.state.peers.first { peer in
            peer.peerKey == pending.offer.senderPeerKey
                && peer.inviteRoute?.endpointID == pending.offer.senderInboxEndpointID
        }?.inviteRoute
        let selection = NearbyPairingSelection(
            discoveryPeerKey: pending.offer.senderPeerKey,
            displayName: pending.offer.senderDisplayName,
            sources: [pending.offer.source],
            nearbyInviteRoute: capturedRoute
        )
        let action = ConnectionWorkflowPolicy.localAction(
            forLocalRole: parsed.joinerRole
        )
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
        let input = url.absoluteString
        if input.lowercased().hasPrefix("envoix://invite/v2/") {
            guardRoomReplacement {
                if let error = openPairingRoom(input: input) {
                    ToastCenter.shared.show(error)
                }
            }
            return
        }
        if isRoomControlInput(input) {
            guardRoomReplacement {
                if let error = openPairingRoom(input: input) {
                    ToastCenter.shared.show(error)
                }
            }
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
        if isControlRoomOpen, workflow.room == nil {
            guardRoomReplacement {
                openExternalShareRoom()
            }
            return
        }
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
        transferHasStarted = false
        transferUsesRoomControl = workflow.room?.origin == .roomControl
        transferRoute = .send
    }

    private var isRoomOccupied: Bool {
        workflow.room != nil || workflow.hasPinnedRememberedRoom || isControlRoomOpen
    }

    private var isControlRoomOpen: Bool {
        switch workflow.controlPhase {
        case .hosting, .joining:
            return true
        case .connected:
            return workflow.room != nil || workflow.hasPinnedRememberedRoom
        case .connectingRemembered, .waitingRemembered:
            return workflow.hasPinnedRememberedRoom
        case .idle, .ended, .failed:
            return false
        }
    }

    private func guardRoomReplacement(_ action: @escaping () -> Void) {
        guard isRoomOccupied else {
            action()
            return
        }
        pendingRoomReplacement = action
        isRoomReplacementPresented = true
    }

    private func revealRoomInvitation() {
        if workflow.roomInvitation != nil {
            roomInvitationIsRevealed = true
            return
        }
        guardRoomReplacement {
            if startHostingRoom() {
                roomInvitationIsRevealed = true
            }
        }
    }

    private func refreshRoomInvitation() {
        guard workflow.controlPhase == .hosting else {
            revealRoomInvitation()
            return
        }
        if startHostingRoom() {
            roomInvitationIsRevealed = true
        }
    }

    @discardableResult
    private func startHostingRoom() -> Bool {
        guard let identityPath = roomIdentityPath else {
            ToastCenter.shared.show(AppText.value(
                "Application Support is unavailable.",
                "无法访问应用支持目录。",
                language: language
            ))
            return false
        }
        let error = workflow.startHosting(
            broker: serverURL,
            relay: relayURL,
            displayName: presence.displayName,
            identityPath: identityPath,
            existingActivityIDs: Set(model.activities.map(\.activityId))
        )
        if let error {
            ToastCenter.shared.show(error)
            return false
        }
        return true
    }

    private var roomIdentityPath: String? {
        FileManager.default.urls(
            for: .applicationSupportDirectory,
            in: .userDomainMask
        ).first?
            .appendingPathComponent("envoix", isDirectory: true)
            .appendingPathComponent("room-control-identity", isDirectory: false)
            .path
    }

    private func isRoomControlInput(_ input: String) -> Bool {
        let normalized = input.trimmed
        if normalized.lowercased().hasPrefix("envoix://room/") {
            return true
        }
        let characters = Array(normalized)
        guard characters.count > 8,
              characters[0].uppercased() == "R",
              characters[7] == "-" else {
            return false
        }
        return characters[1...6].allSatisfy(\.isNumber)
    }

    private func isEndedOrFailed(_ phase: RoomControlPhase) -> Bool {
        switch phase {
        case .ended, .failed:
            return true
        case .idle, .hosting, .joining, .connectingRemembered,
             .waitingRemembered, .connected:
            return false
        }
    }

    private func updateDisplayName(_ value: String) -> Bool {
        guard presence.updateDisplayName(value) else { return false }
        updateDiscoveryLease()
        return true
    }

    private func acceptIncomingRoomOffer() {
        continueAcceptingIncomingRoomOffer(using: nil)
    }

    private func continueAcceptingIncomingRoomOffer(
        using preparedDestination: PreparedRoomDestination?
    ) {
        guard let offer = workflow.incomingRoomOffer,
              let roomID = workflow.activeRoomID,
              let endpoint = workflow.activeRoomEndpoint,
              acceptingRoomOfferID == nil else { return }
        guard !model.receive.isBusy else {
            ToastCenter.shared.show(AppText.value(
                "Finish the current receive before accepting another offer.",
                "请先完成当前接收任务，再接受新的文件邀请。",
                language: language
            ))
            return
        }
        let invitation: FfiPairingInvite
        let settings: EnvoixRuntimeSettings
        do {
            invitation = try parsePairingInviteForRole(
                input: offer.transferInvite,
                localRole: .receive
            )
            guard invitation.relayUrls.count <= 1,
                  RoomControlEndpoint(transferInvitation: invitation) == endpoint else {
                throw RuntimeSettingsError("The file offer does not use this room's route.")
            }
            settings = try RuntimeSettingsProvider.make(
                concurrentTransfers: concurrentTransfers,
                language: language,
                serverURL: endpoint.broker,
                relayURL: endpoint.relay,
                candidatesAllow: candidatesAllow,
                candidatesDeny: candidatesDeny,
                speedLimit: speedLimit
            )
        } catch {
            // Invalid, expired, or cross-room invitations cannot become
            // receivable by retrying this same offer. Release both control
            // peers immediately so one bad offer cannot poison the room.
            workflow.rejectIncomingRoomOffer()
            ToastCenter.shared.show(error.localizedDescription)
            return
        }

        let destination: PreparedRoomDestination
        if let preparedDestination {
            destination = preparedDestination
        } else {
            do {
                destination = try prepareAutomaticRoomDestination()
            } catch {
                guard workflow.holdIncomingRoomOfferForDestination(id: offer.id) else {
                    return
                }
                roomDestinationRepair = RoomDestinationRepairRequest(
                    offerID: offer.id,
                    roomID: roomID
                )
                return
            }
        }

        acceptingRoomOfferID = offer.id
        Task { @MainActor in
            transferRoomID = roomID
            transferHasStarted = false
            transferUsesInboundInvite = true
            transferUsesRoomControl = true
            let result = await RoomOfferAcceptanceCoordinator.startReceiverThenAccept(
                startReceiver: {
                    await model.receive.startReceivingRoomControlInvite(
                        outputDir: destination.url.path,
                        invite: offer.transferInvite,
                        offer: offer,
                        settings: settings,
                        destinationAccess: destination.access
                    )
                },
                acceptOffer: {
                    await workflow.acceptIncomingRoomOffer() != nil
                },
                cancelReceiver: { activityID in
                    if model.receive.transferActivity?.activityId == activityID {
                        _ = model.receive.cancel()
                    }
                }
            )
            acceptingRoomOfferID = nil
            switch result {
            case .accepted:
                break
            case .receiverDidNotStart:
                workflow.rejectIncomingRoomOffer()
                resetRoomTransferHandoff()
            case .offerUnavailable:
                resetRoomTransferHandoff()
                ToastCenter.shared.show(AppText.value(
                    "The file offer is no longer available.",
                    "此文件邀请已不可用。",
                    language: language
                ))
            }
        }
    }

    private func completeRoomDestinationRepair(
        url: URL,
        request: RoomDestinationRepairRequest
    ) {
        guard roomDestinationRepair == request else { return }
        let access = SecurityScopedResourceAccess(url: url)
        do {
            guard access.isActive || FileManager.default.isWritableFile(atPath: url.path) else {
                throw RuntimeSettingsError(AppText.value(
                    "Envoix cannot access the selected save folder.",
                    "Envoix 无法访问所选保存文件夹。",
                    language: language
                ))
            }
            try validateWritableDirectoryAccess(url)
            let bookmark = try makeSecurityScopedFolderBookmark(for: url)
            UserDefaults.standard.set(bookmark, forKey: "envoix.outputDirBookmark")
            outputDirDisplayName = url.lastPathComponent.isEmpty ? url.path : url.lastPathComponent

            guard let offerID = workflow.incomingRoomOffer?.id,
                  let roomID = workflow.activeRoomID,
                  request.matches(offerID: offerID, roomID: roomID) else {
                roomDestinationRepair = nil
                return
            }

            let destination: PreparedRoomDestination = (url, access)
            roomDestinationRepair = nil
            continueAcceptingIncomingRoomOffer(using: destination)
        } catch {
            roomDestinationRepair = nil
            workflow.resumeIncomingRoomOfferDeadline(id: request.offerID)
            ToastCenter.shared.show(error.localizedDescription)
        }
    }

    private func prepareAutomaticRoomDestination() throws -> PreparedRoomDestination {
        let bookmarkKey = "envoix.outputDirBookmark"
        if let bookmark = UserDefaults.standard.data(forKey: bookmarkKey) {
            let url = try resolveSecurityScopedFolderBookmark(bookmark)
            let access = SecurityScopedResourceAccess(url: url)
            guard access.isActive || FileManager.default.isWritableFile(atPath: url.path) else {
                throw RuntimeSettingsError("The selected save folder permission expired.")
            }
            try validateWritableDirectoryAccess(url)
            return (url, access)
        }

        let documents = FileManager.default.urls(
            for: .documentDirectory,
            in: .userDomainMask
        ).first ?? URL(
            fileURLWithPath: NSHomeDirectory(),
            isDirectory: true
        ).appendingPathComponent("Documents", isDirectory: true)
        let destination = documents.appendingPathComponent("Downloads", isDirectory: true)
        try validateWritableDirectoryAccess(destination)
        return (destination, nil)
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
