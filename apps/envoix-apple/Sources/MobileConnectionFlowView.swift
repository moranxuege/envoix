#if os(iOS) || os(macOS)
import Combine
import EnvoixCore
import SwiftUI
import UniformTypeIdentifiers
#if os(iOS)
import UIKit
#endif

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

enum NearbyDiscoveryLeasePolicy {
    static func shouldRun(
        sceneAllowsDiscovery: Bool,
        isConnectionPage: Bool,
        discoveryIsEnabled: Bool,
        systemPairingIsActive: Bool
    ) -> Bool {
        sceneAllowsDiscovery
            && isConnectionPage
            && discoveryIsEnabled
            && !systemPairingIsActive
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

#if os(iOS)
enum ExternalInvitationOrigin: Equatable {
    case nfc
    case deepLink
    case universalLink
}

struct PendingExternalInvitation: Equatable, Identifiable {
    let id: UUID
    let invitation: String
    let origin: ExternalInvitationOrigin

    init(
        id: UUID = UUID(),
        invitation: String,
        origin: ExternalInvitationOrigin
    ) {
        self.id = id
        self.invitation = invitation
        self.origin = origin
    }
}

enum ExternalInvitationRoutingPolicy {
    static func shouldStage(
        invitation: String,
        pendingInvitation: String?,
        openedInvitation: String?
    ) -> Bool {
        pendingInvitation == nil && openedInvitation != invitation
    }
}

struct NFCInvitationReadinessGate {
    static let automaticReadCooldownMilliseconds: Int64 = 60_000
    private static let maximumClaimedOfferCount = 256

    private var claimedOfferIDs = Set<String>()
    private var claimedOfferOrder: [String] = []
    private var automaticSheetIsAvailable = true
    private var lastAutomaticReadAtMilliseconds: Int64?

    mutating func claim(
        offer: NearbyNFCReadinessOffer,
        nowMilliseconds: Int64,
        applicationIsActive: Bool,
        isConnectPage: Bool,
        eligibleBluetoothPeerKeys: Set<String>
    ) -> Bool {
        guard applicationIsActive,
              isConnectPage,
              automaticSheetIsAvailable,
              offer.isFresh(at: nowMilliseconds),
              eligibleBluetoothPeerKeys.contains(offer.presenterPeerKey),
              !claimedOfferIDs.contains(offer.id) else {
            return false
        }
        if let lastAutomaticReadAtMilliseconds {
            guard nowMilliseconds >= lastAutomaticReadAtMilliseconds,
                  nowMilliseconds - lastAutomaticReadAtMilliseconds
                      >= Self.automaticReadCooldownMilliseconds else {
                return false
            }
        }
        claimedOfferIDs.insert(offer.id)
        claimedOfferOrder.append(offer.id)
        if claimedOfferOrder.count > Self.maximumClaimedOfferCount {
            claimedOfferIDs.remove(claimedOfferOrder.removeFirst())
        }
        automaticSheetIsAvailable = false
        lastAutomaticReadAtMilliseconds = nowMilliseconds
        return true
    }

    mutating func didBeginManualRead() {
        automaticSheetIsAvailable = false
    }

    mutating func didLeaveConnectPage() {
        automaticSheetIsAvailable = true
    }
}

private struct AutomaticNFCPresentationEnvironment: Equatable {
    let transferSheetIsPresented: Bool
    let destinationRepairIsPresented: Bool
    let scannerIsPresented: Bool
    let manualEntryIsPresented: Bool
    let nearbyOfferAlertIsPresented: Bool
    let closeRoomAlertIsPresented: Bool
    let replaceRoomAlertIsPresented: Bool
    let roomVerificationIsPresented: Bool
    let externalConfirmationIsPresented: Bool
    let systemPairingIsPresented: Bool
    let connectionHubModalIsPresented: Bool

    var hasConflict: Bool {
        transferSheetIsPresented
            || destinationRepairIsPresented
            || scannerIsPresented
            || manualEntryIsPresented
            || nearbyOfferAlertIsPresented
            || closeRoomAlertIsPresented
            || replaceRoomAlertIsPresented
            || roomVerificationIsPresented
            || externalConfirmationIsPresented
            || systemPairingIsPresented
            || connectionHubModalIsPresented
    }
}

private struct ExternalInvitationConfirmationOverlay: View {
    let language: String
    let isRoomInvitation: Bool
    let origin: ExternalInvitationOrigin
    let onContinue: () -> Void
    let onCancel: () -> Void

    var body: some View {
        ZStack(alignment: .bottom) {
            Color.black.opacity(0.18)
                .ignoresSafeArea()
                .accessibilityHidden(true)

            VStack(alignment: .leading, spacing: 10) {
                Label(
                    confirmationTitle,
                    systemImage: origin == .nfc
                        ? "wave.3.right.circle"
                        : "exclamationmark.shield"
                )
                .font(.headline.weight(.semibold))
                .foregroundStyle(Theme.text)

                Text(confirmationMessage)
                    .font(.footnote)
                    .foregroundStyle(Theme.muted)
                    .fixedSize(horizontal: false, vertical: true)

                HStack(spacing: 10) {
                    Spacer(minLength: 0)
                    Button(
                        MobileConnectionFlowPresentationText.value(.cancel, language: language),
                        action: onCancel
                    )
                    .buttonStyle(.bordered)
                    .accessibilityIdentifier("external_invitation_cancel")

                    Button(
                        MobileConnectionFlowPresentationText.value(
                            .continueAction,
                            language: language
                        ),
                        action: onContinue
                    )
                    .buttonStyle(.borderedProminent)
                    .tint(Theme.accentStrong)
                    .accessibilityIdentifier("external_invitation_continue")
                }
            }
            .padding(16)
            .frame(maxWidth: 420, alignment: .leading)
            .background(
                .regularMaterial,
                in: RoundedRectangle(cornerRadius: 18, style: .continuous)
            )
            .overlay {
                RoundedRectangle(cornerRadius: 18, style: .continuous)
                    .stroke(Theme.line.opacity(0.7), lineWidth: 0.5)
            }
            .shadow(color: .black.opacity(0.16), radius: 18, y: 8)
            .padding(.horizontal, 16)
            .padding(.bottom, 14)
            .accessibilityElement(children: .contain)
            .accessibilityIdentifier("external_invitation_confirmation")
        }
    }

    private var confirmationTitle: String {
        MobileConnectionFlowPresentationText.externalInvitationTitle(
            isRoomInvitation: isRoomInvitation,
            isNFC: origin == .nfc,
            language: language
        )
    }

    private var confirmationMessage: String {
        MobileConnectionFlowPresentationText.externalInvitationMessage(
            isRoomInvitation: isRoomInvitation,
            isNFC: origin == .nfc,
            language: language
        )
    }
}
#endif

struct MobileConnectionFlowView: View {
    private typealias PreparedRoomDestination = (url: URL, access: AnyObject?)

    @EnvironmentObject private var model: AppModel
    @Environment(\.scenePhase) private var scenePhase
    @Environment(\.horizontalSizeClass) private var horizontalSizeClass
    #if os(iOS)
    @Environment(\.openWindow) private var openWindow
    #endif
    @AppStorage("envoix.language") private var language = "en"
    @AppStorage("envoix.serverURL") private var serverURL = ""
    @AppStorage("envoix.relayURL") private var relayURL = ""
    @AppStorage("envoix.concurrentTransfers") private var concurrentTransfers = true
    @AppStorage("envoix.candidatesAllow") private var candidatesAllow = ""
    @AppStorage("envoix.candidatesDeny") private var candidatesDeny = ""
    @AppStorage("envoix.speedLimit") private var speedLimit = 40
    @AppStorage("envoix.outputDirDisplayName") private var outputDirDisplayName = ""

    @ObservedObject private var runtime = AppleApplicationRuntime.shared
    @ObservedObject private var nearbyCoordinator =
        AppleApplicationRuntime.shared.nearbyCoordinator
    @ObservedObject private var presence = AppleApplicationRuntime.shared.presence
    @ObservedObject private var workflow = AppleApplicationRuntime.shared.workflow
    @ObservedObject private var rememberedOutbox =
        AppleApplicationRuntime.shared.rememberedOutbox
    #if os(macOS)
    @ObservedObject private var helperTransfers =
        AppleApplicationRuntime.shared.helperTransfers
    #endif
    @StateObject private var navigation = MobileSceneNavigationState(
        initialPage: {
            #if DEBUG
            if ProcessInfo.processInfo.arguments.contains("--ui-testing-start-activity") {
                return .activity
            }
            #endif
            return .connect
        }()
    )
    #if os(iOS) && canImport(CoreNFC)
    @StateObject private var nfcInvitationExchange = NFCInvitationExchange()
    #endif
    #if os(iOS)
    @State private var pendingExternalInvitation: PendingExternalInvitation?
    @State private var openedExternalInvitation: String?
    @State private var nfcReadinessGate = NFCInvitationReadinessGate()
    #endif
    @State private var connectionHubModalIsPresented = false
    @State private var sceneID = UUID()
    #if os(iOS)
    @State private var splitViewVisibility = NavigationSplitViewVisibility.all
    #endif
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
    @State private var presentedSharedSelectionID: UUID?
    @State private var scannerIsPresented = false
    @State private var manualEntryIsPresented = false
    @State private var manualPairingInput = ""
    @State private var outgoingBleVerification: BleVerificationInvitation?
    @State private var pendingBleVerificationOffer: NearbyRendezvousOffer?
    @State private var bleVerificationInput = ""
    @State private var roomVerificationInput = ""
    @State private var isCloseRoomConfirmationPresented = false
    @State private var roomInvitationIsRevealed = false
    @State private var now = Date()
    @State private var pendingRoomReplacement: (() -> Void)?
    @State private var isRoomReplacementPresented = false
    @State private var acceptingRoomOfferID: String?
    @State private var roomDestinationRepair: RoomDestinationRepairRequest?
    #if os(macOS)
    @State private var helperFileImporterIsPresented = false
    @State private var helperFileImporterDeviceID: String?
    @State private var selectedHelperDeviceID: String?
    @State private var macActivityShowsLegacyTransfers = false
    #endif
    #if DEBUG && os(iOS)
    @State private var didStageBackgroundShareFixture = false
    @State private var openInUITestFixtureURL: URL?
    #endif

    var body: some View {
        ZStack {
            navigationShell
            #if os(iOS)
            .allowsHitTesting(pendingExternalInvitation == nil)
            .accessibilityHidden(pendingExternalInvitation != nil)
            #endif

            #if os(iOS)
            if let pendingExternalInvitation {
                ExternalInvitationConfirmationOverlay(
                    language: language,
                    isRoomInvitation: pendingExternalInvitation.invitation
                        .hasPrefix(roomControlURLPrefix),
                    origin: pendingExternalInvitation.origin,
                    onContinue: {
                        continueExternalInvitation(pendingExternalInvitation)
                    },
                    onCancel: {
                        cancelExternalInvitation(pendingExternalInvitation)
                    }
                )
                .zIndex(1)
            }
            #endif
        }
        .sheet(item: $transferRoute, onDismiss: finishTransferSheetPresentation) { route in
            NavigationStack {
                transferContent(route)
                    .padding(.horizontal, 16)
                    .background(Theme.bg)
                    .navigationTitle(transferTitle(route))
                    #if os(iOS)
                    .navigationBarTitleDisplayMode(.inline)
                    #endif
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
                            .accessibilityLabel(flowText(.close))
                            .accessibilityIdentifier("mobile_sheet_done")
                        }
                    }
            }
            #if os(iOS)
            .presentationDragIndicator(.visible)
            .presentationDetents([.large])
            #endif
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
        .modifier(QRScannerPresentationModifier(
            isPresented: $scannerIsPresented,
            language: language,
            onScan: openPairingRoom
        ))
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
        #if os(macOS)
        .fileImporter(
            isPresented: $helperFileImporterIsPresented,
            allowedContentTypes: [.item],
            allowsMultipleSelection: true,
            onCompletion: handleHelperFileImport
        )
        #endif
        .alert(
            flowText(.verifyNearbyDevice),
            isPresented: Binding(
                get: { outgoingBleVerification != nil },
                set: {
                    if !$0, outgoingBleVerification != nil {
                        outgoingBleVerification = nil
                        closeRoomNow()
                    }
                }
            )
        ) {
            Button(flowText(.cancelVerification), role: .destructive) {
                outgoingBleVerification = nil
                closeRoomNow()
            }
        } message: {
            Text(MobileConnectionFlowPresentationText.outgoingVerification(
                code: outgoingBleVerification?.verificationCode ?? "",
                language: language
            ))
            .privacySensitive()
        }
        .alert(
            flowText(.enterVerificationCode),
            isPresented: Binding(
                get: { pendingBleVerificationOffer != nil },
                set: { if !$0 { pendingBleVerificationOffer = nil } }
            )
        ) {
            SecureField("000000", text: Binding(
                get: { bleVerificationInput },
                set: { bleVerificationInput = String($0.filter { $0.isASCII && $0.isNumber }.prefix(6)) }
            ))
            .privacySensitive()
            Button(flowText(.verifyAndConnect)) {
                guard let offer = pendingBleVerificationOffer else { return }
                if let error = acceptBleVerificationOffer(offer, code: bleVerificationInput) {
                    ToastCenter.shared.show(error)
                }
                bleVerificationInput = ""
            }
            .disabled(bleVerificationInput.count != 6)
            Button(flowText(.cancel), role: .cancel) {
                pendingBleVerificationOffer = nil
                bleVerificationInput = ""
            }
        } message: {
            Text(flowText(.verificationInstruction))
        }
        .alert(
            flowText(.verifyThisDevice),
            isPresented: Binding(
                get: {
                    runtime.isPresentationOwner(sceneID)
                        && workflow.verificationRequested
                },
                set: { presented in
                    if runtime.isPresentationOwner(sceneID),
                       !presented,
                       workflow.verificationRequested {
                        workflow.cancelDeviceVerification()
                        roomVerificationInput = ""
                    }
                }
            )
        ) {
            SecureField("000000", text: Binding(
                get: { roomVerificationInput },
                set: {
                    roomVerificationInput = String(
                        $0.filter { $0.isASCII && $0.isNumber }.prefix(6)
                    )
                }
            ))
            .privacySensitive()
            Button(flowText(.verifyDevice)) {
                if let error = workflow.submitDeviceVerification(roomVerificationInput) {
                    ToastCenter.shared.show(error)
                }
                roomVerificationInput = ""
            }
            .disabled(roomVerificationInput.count != 6)
            .accessibilityIdentifier("room_device_verification_submit")
            Button(flowText(.cancel), role: .cancel) {
                workflow.cancelDeviceVerification()
                roomVerificationInput = ""
            }
        } message: {
            Text(MobileConnectionFlowPresentationText.deviceVerification(
                peerDisplayName: workflow.peerDisplayName,
                language: language
            ))
        }
        .alert(item: pendingOfferBinding) { pending in
            let isRoomInvite = connectionInputKind(
                pending.offer.invite,
                allowBareRoomControl: false
            ) == .roomControl
            return Alert(
                title: Text(MobileConnectionFlowPresentationText.nearbyOfferTitle(
                    isRoomInvitation: isRoomInvite,
                    language: language
                )),
                message: Text(MobileConnectionFlowPresentationText.nearbyOfferMessage(
                    senderDisplayName: pending.offer.senderDisplayName,
                    isRoomInvitation: isRoomInvite,
                    language: language
                )),
                primaryButton: .default(Text(flowText(.acceptNearbyOffer))) {
                    acceptPendingOffer(pending)
                },
                secondaryButton: .cancel(Text(flowText(.rejectNearbyOffer))) {
                    workflow.discardPendingOffer(id: pending.id)
                }
            )
        }
        .alert(
            flowText(.endRoomQuestion),
            isPresented: $isCloseRoomConfirmationPresented
        ) {
            Button(flowText(.keepRoom), role: .cancel) {}
            Button(flowText(.endRoom), role: .destructive) {
                closeRoomNow()
            }
        } message: {
            Text(flowText(.endRoomDetail))
        }
        .alert(
            flowText(.roomAlreadyOpen),
            isPresented: $isRoomReplacementPresented
        ) {
            Button(flowText(.returnToRoom)) {
                pendingRoomReplacement = nil
                if workflow.activeRoomID != nil {
                    navigation.page = .room
                }
            }
            Button(flowText(.endAndReplace), role: .destructive) {
                let action = pendingRoomReplacement
                pendingRoomReplacement = nil
                closeRoomNow()
                action?()
            }
            Button(flowText(.cancel), role: .cancel) {
                pendingRoomReplacement = nil
            }
        } message: {
            Text(flowText(.oneRoomAtATime))
        }
        .onAppear {
            prepareUITestFixtures()
            workflow.refreshRememberedRooms()
            #if os(macOS)
            Task { await helperTransfers.refresh() }
            #endif
            rememberedOutbox.start()
            updateRuntimeRequest()
            presentPendingSendSelection()
            synchronizeRememberedOutbox()
            beginOfferGatedNFCReadIfNeeded()
        }
        .onDisappear {
            runtime.removeScene(id: sceneID)
            #if os(iOS) && canImport(CoreNFC)
            nfcInvitationExchange.cancelReading()
            #endif
        }
        .onOpenURL(perform: handleIncomingURL)
        #if os(macOS)
        .onChange(of: model.pendingSendSelection?.id) { selectionID in
            if selectionID != nil {
                presentPendingSendSelection()
            }
        }
        #endif
        #if os(iOS)
        .onContinueUserActivity(NSUserActivityTypeBrowsingWeb) { activity in
            guard let url = activity.webpageURL else { return }
            handleIncomingURL(url)
        }
        #endif
        .onChange(of: navigation.page) { newPage in
            updateRuntimeRequest()
            #if os(macOS)
            if newPage == .room || newPage == .activity {
                Task { await helperTransfers.refreshSnapshot() }
            }
            #endif
            #if os(iOS) && canImport(CoreNFC)
            if newPage == .connect {
                beginOfferGatedNFCReadIfNeeded()
            } else {
                nfcReadinessGate.didLeaveConnectPage()
                nfcInvitationExchange.cancelReading()
            }
            #endif
        }
        .onChange(of: runtime.presentationOwnerSceneID) { ownerSceneID in
            guard ownerSceneID == sceneID else { return }
            presentPendingSendSelection()
            captureIncomingNearbyOffer()
            beginOfferGatedNFCReadIfNeeded()
        }
        .onChange(of: scenePhase) { phase in
            let effects = MobileSceneLifecyclePolicy.effects(
                for: MobileSceneLifecycleEvent(scenePhase: phase)
            )
            #if DEBUG && os(iOS)
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
            updateRuntimeRequest()
            synchronizeRememberedOutbox()
            #if os(iOS) && canImport(CoreNFC)
            if phase == .active {
                beginOfferGatedNFCReadIfNeeded()
            } else {
                nfcInvitationExchange.cancelReading()
            }
            #endif
            #if os(macOS)
            if phase == .active {
                Task { await helperTransfers.refresh() }
            }
            #endif
        }
        .onReceive(Timer.publish(every: 1, on: .main, in: .common).autoconnect()) { date in
            now = date
            if presence.expireIfNeeded(now: date) {
                updateRuntimeRequest()
            }
            workflow.tick(now: date, hasActiveTransfer: roomHasActiveTransfers)
            #if os(macOS)
            if scenePhase == .active,
               (navigation.page == .room
                || navigation.page == .activity
                || helperTransfers.hasPendingTransfers) {
                Task { await helperTransfers.refreshSnapshot() }
            }
            #endif
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
                updateRuntimeRequest()
                presentPendingSendSelection()
            }
            synchronizeRememberedOutbox()
        }
        .onChange(of: model.send.isPreparingManifest) { isPreparing in
            if !isPreparing {
                presentPendingSendSelection()
            }
        }
        .onChange(of: model.receive.presentationState) { state in
            if state == .delivered || state == .failed || state == .canceled {
                workflow.refreshRememberedRooms()
                updateRuntimeRequest()
            }
        }
        .onChange(of: presence.displayName) { _ in
            updateRuntimeRequest()
        }
        .onChange(of: presence.visibility) { _ in updateRuntimeRequest() }
        .onChange(of: workflow.controlPhase) { phase in
            if phase == .connected,
               workflow.room != nil {
                roomInvitationIsRevealed = false
                if runtime.isPresentationOwner(sceneID) {
                    navigation.page = .room
                }
                workflow.setLocalTransferActive(roomHasActiveTransfers)
            } else if phase == .connected,
                      workflow.rememberedRoom != nil {
                roomInvitationIsRevealed = false
                workflow.setLocalTransferActive(roomHasActiveTransfers)
            } else if phase != .hosting {
                roomInvitationIsRevealed = false
            }
            if phase == .connected {
                outgoingBleVerification = nil
                pendingBleVerificationOffer = nil
                presentPendingSendSelection()
            }
            if runtime.isPresentationOwner(sceneID),
               case .failed(let message) = phase {
                ToastCenter.shared.show(message)
            }
            if isEndedOrFailed(phase),
               transferRoute != nil,
               transferUsesRoomControl,
               transferRememberedRelationshipID == nil {
                transferRoute = nil
            }
            if isEndedOrFailed(phase) {
                outgoingBleVerification = nil
                roomDestinationRepair = nil
            }
            synchronizeRememberedOutbox()
        }
        .onChange(of: workflow.durablePairingCompletedLabel) { label in
            guard let label,
                  runtime.isPresentationOwner(sceneID) else { return }
            ToastCenter.shared.show(MobileConnectionFlowPresentationText.durablePairingCompleted(
                label: label,
                language: language
            ))
            #if os(macOS)
            Task {
                await runtime.helperService.refresh()
                await helperTransfers.refresh()
            }
            #endif
        }
        .onChange(of: workflow.incomingRoomOffer?.id) { offerID in
            if let request = roomDestinationRepair, request.offerID != offerID {
                roomDestinationRepair = nil
            }
        }
        .onChange(
            of: nearbyCoordinator.state.incomingRendezvousOffer?.deliveryID
        ) { _ in
            captureIncomingNearbyOffer()
        }
        #if os(iOS) && canImport(CoreNFC)
        .onChange(of: nearbyCoordinator.state.incomingNFCReadinessOffer?.id) { _ in
            beginOfferGatedNFCReadIfNeeded()
        }
        .onChange(of: automaticNFCPresentationEnvironment) { _ in
            beginOfferGatedNFCReadIfNeeded()
        }
        #endif
        .onChange(of: model.send.transferActivity?.activityId) { activityID in
            guard let activityID,
                  transferRoomID == workflow.activeRoomID else { return }
            transferHasStarted = true
            preservedSendSelection = SendSelectionSnapshot()
            workflow.captureActivity(activityID)
            assignActivityToActiveRoom(activityID)
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
            assignActivityToActiveRoom(activityID)
            workflow.setLocalTransferActive(roomHasActiveTransfers)
            if transferRoute == .receive {
                transferRoute = nil
            }
        }
    }

    @ViewBuilder
    private var navigationShell: some View {
        #if os(iOS)
        if horizontalSizeClass == .regular {
            NavigationSplitView(columnVisibility: $splitViewVisibility) {
                sidebar
            } detail: {
                pageNavigationStack
            }
            .navigationSplitViewStyle(.balanced)
        } else {
            pageNavigationStack
        }
        #else
        pageNavigationStack
        #endif
    }

    private var pageNavigationStack: some View {
        NavigationStack {
            pageContent
                .background(Theme.bg)
                .navigationTitle(pageTitle)
                #if os(iOS)
                .navigationBarTitleDisplayMode(.inline)
                #endif
                .toolbar { toolbarContent }
        }
    }

    #if os(iOS)
    private var sidebar: some View {
        List(selection: sidebarSelection) {
            sidebarRow(.connect, systemImage: "sparkles")
            if workflow.activeRoomID != nil {
                sidebarRow(.room, systemImage: "person.2.fill")
            }
            sidebarRow(.activity, systemImage: "clock.arrow.circlepath")
            sidebarRow(.settings, systemImage: "gearshape")
        }
        .navigationTitle("Envoix")
        .toolbar {
            ToolbarItem(placement: .primaryAction) {
                Button {
                    openWindow(id: "main")
                } label: {
                    Label(
                        flowText(.newWindow),
                        systemImage: "plus.rectangle.on.rectangle"
                    )
                }
                .keyboardShortcut("n", modifiers: .command)
                .accessibilityIdentifier("ipad_new_window")
            }
        }
        .accessibilityIdentifier("ipad_sidebar")
    }

    private var sidebarSelection: Binding<MobilePage?> {
        Binding(
            get: { navigation.page },
            set: { destination in
                if let destination {
                    showPage(destination)
                }
            }
        )
    }

    private func sidebarRow(_ destination: MobilePage, systemImage: String) -> some View {
        Label(pageTitle(destination), systemImage: systemImage)
            .tag(destination)
            .accessibilityIdentifier("ipad_sidebar_\(destination.rawValue)")
    }
    #endif

    private var pairedDevices: [PairedDevicePresentation] {
        #if os(macOS)
        return helperTransfers.devices.map {
            PairedDevicePresentation(id: $0.id, label: $0.label)
        }
        #else
        return workflow.rememberedPeers.map {
            PairedDevicePresentation(id: $0.relationshipID, label: $0.label)
        }
        #endif
    }

    private var incomingPairedDeviceID: String? {
        #if os(macOS)
        return nil
        #else
        return workflow.incomingRoomOffer == nil
            ? nil
            : workflow.activeRememberedRelationshipID
        #endif
    }

    private func pairedDeviceStatus(_ deviceID: String) -> RememberedRoomConnectionStatus {
        #if os(macOS)
        return helperTransfers.isPreparing(deviceID: deviceID) ? .connecting : .available
        #else
        return workflow.rememberedRoomStatus(relationshipID: deviceID)
        #endif
    }

    private func selectPairedDevice(_ deviceID: String) {
        #if os(macOS)
        guard helperTransfers.devices.contains(where: { $0.id == deviceID }) else {
            ToastCenter.shared.show(flowText(.refreshPairedDevices))
            return
        }
        selectedHelperDeviceID = deviceID
        navigation.show(.room)
        Task { await helperTransfers.refreshSnapshot() }
        #else
        openRememberedRoom(deviceID)
        #endif
    }

    private func sendToPairedDevice(_ deviceID: String) {
        #if os(macOS)
        if let selection = model.pendingSendSelection,
           !selection.fileURLs.isEmpty {
            queueHelperTransfer(
                deviceID: deviceID,
                urls: selection.fileURLs,
                pendingSelectionID: selection.id
            )
            return
        }
        helperFileImporterDeviceID = deviceID
        helperFileImporterIsPresented = true
        #else
        offerFilesToRememberedRoom(deviceID)
        #endif
    }

    private func sendDroppedItemsToPairedDevice(_ deviceID: String, _ urls: [URL]) {
        #if os(macOS)
        queueHelperTransfer(deviceID: deviceID, urls: urls, pendingSelectionID: nil)
        #else
        offerDroppedItemsToRememberedRoom(deviceID, urls)
        #endif
    }

    #if os(macOS)
    private func handleHelperFileImport(_ result: Result<[URL], Error>) {
        guard let deviceID = helperFileImporterDeviceID else { return }
        helperFileImporterDeviceID = nil
        switch result {
        case let .success(urls):
            guard !urls.isEmpty else { return }
            queueHelperTransfer(deviceID: deviceID, urls: urls, pendingSelectionID: nil)
        case let .failure(error):
            if (error as? CocoaError)?.code != .userCancelled {
                ToastCenter.shared.show(error.localizedDescription)
            }
        }
    }

    private func queueHelperTransfer(
        deviceID: String,
        urls: [URL],
        pendingSelectionID: UUID?
    ) {
        guard let device = helperTransfers.devices.first(where: { $0.id == deviceID }) else {
            ToastCenter.shared.show(flowText(.refreshPairedDevices))
            return
        }
        Task { @MainActor in
            do {
                _ = try await helperTransfers.createTransfer(deviceID: deviceID, urls: urls)
                if let pendingSelectionID {
                    model.consumePendingSendSelection(id: pendingSelectionID)
                }
                selectedHelperDeviceID = deviceID
                navigation.show(.room)
                await helperTransfers.refreshSnapshot()
                ToastCenter.shared.show(MobileConnectionFlowPresentationText.queuedForDevice(
                    label: device.label,
                    language: language
                ))
            } catch {
                ToastCenter.shared.show(error.localizedDescription)
            }
        }
    }
    #endif

    @ViewBuilder
    private var pageContent: some View {
        switch navigation.page {
        case .connect:
            ConnectionHubView(
                coordinator: nearbyCoordinator,
                presence: presence,
                openInFixtureURL: debugOpenInFixtureURL,
                roomInvitation: workflow.roomInvitation,
                roomInvitationIsRevealed: roomInvitationIsRevealed,
                roomInvitationIsStarting: workflow.controlPhase == .joining,
                rememberedRooms: pairedDevices,
                pendingSendItemCount: model.pendingSendSelection?.fileURLs.count ?? 0,
                rememberedRoomStatus: pairedDeviceStatus,
                incomingRememberedRelationshipID: incomingPairedDeviceID,
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
                nfcIsAvailable: nfcInvitationIsAvailable,
                nfcIsActive: nfcInvitationIsActive,
                onScanNFC: scanNFCInvitation,
                onRevealRoomInvitation: revealRoomInvitation,
                onHideRoomInvitation: { roomInvitationIsRevealed = false },
                onRefreshRoomInvitation: refreshRoomInvitation,
                onCancelRoomInvitation: requestCloseRoom,
                onSetVisibility: { presence.setVisibility($0) },
                onRename: updateDisplayName,
                onSelectRememberedRoom: selectPairedDevice,
                onSendToRememberedRoom: sendToPairedDevice,
                onSendDroppedItems: sendDroppedItemsToPairedDevice,
                onPrepareNearbyPairing: {
                    await runtime.beginSystemPairing(for: sceneID)
                },
                onFinishNearbyPairing: {
                    Task { @MainActor in
                        await runtime.finishSystemPairing(for: sceneID)
                    }
                },
                onModalPresentationChanged: {
                    connectionHubModalIsPresented = $0
                },
                onSelectPeer: openNearbyRoom
            )
        case .room:
            roomPage
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

    @ViewBuilder
    private var roomPage: some View {
        #if os(macOS)
        if let device = selectedHelperDevice {
            MacOSAgentRoomView(
                device: device,
                transfers: helperTransfers.transfers(deviceID: device.id),
                activePaths: helperTransfers.activePaths,
                isPreparing: helperTransfers.isPreparing(deviceID: device.id),
                loadError: helperTransfers.loadError,
                onAddFiles: { sendToPairedDevice(device.id) },
                onShowActivity: {
                    macActivityShowsLegacyTransfers = false
                    showPage(.activity)
                }
            )
        } else {
            legacyRoomPage
        }
        #else
        legacyRoomPage
        #endif
    }

    @ViewBuilder
    private var legacyRoomPage: some View {
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
                metricsByActivityID: model.activityMetrics,
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
                metricsByActivityID: model.activityMetrics,
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
                .onAppear { navigation.page = .connect }
        }
    }

    @ViewBuilder
    private var activityPage: some View {
        #if os(macOS)
        VStack(spacing: 8) {
            if !model.activities.isEmpty {
                Picker("", selection: $macActivityShowsLegacyTransfers) {
                    Text(flowText(.backgroundHelper))
                    .tag(false)
                    Text(flowText(.oneTimeTransfers))
                    .tag(true)
                }
                .pickerStyle(.segmented)
                .labelsHidden()
                .padding(.horizontal, 16)
                .accessibilityIdentifier("activity_source_picker")
            }

            if macActivityShowsLegacyTransfers, !model.activities.isEmpty {
                legacyActivityPage
            } else {
                MacOSAgentActivityView(
                    transfers: helperTransfers.transfers,
                    devices: helperTransfers.devices,
                    activePaths: helperTransfers.activePaths,
                    hasLoadedSnapshot: helperTransfers.hasLoadedSnapshot,
                    loadError: helperTransfers.loadError
                )
                .padding(.horizontal, 16)
                .padding(.vertical, 8)
            }
        }
        .accessibilityIdentifier("activity_page")
        #else
        legacyActivityPage
            .accessibilityIdentifier("activity_page")
        #endif
    }

    private var legacyActivityPage: some View {
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
    }

    @ToolbarContentBuilder
    private var toolbarContent: some ToolbarContent {
        if navigation.page == .connect {
            ToolbarItem(placement: leadingToolbarPlacement) {
                Button {
                    showPage(.activity)
                } label: {
                    Image(systemName: "clock.arrow.circlepath")
                        .font(.body.weight(.semibold))
                        .frame(width: 40, height: 40)
                }
                .accessibilityLabel(flowText(.activity))
                .accessibilityIdentifier("open_activity")
            }
        } else {
            ToolbarItem(placement: leadingToolbarPlacement) {
                Button(action: navigateBack) {
                    Image(systemName: "chevron.left")
                        .font(.body.weight(.semibold))
                        .frame(width: 40, height: 40)
                }
                .accessibilityLabel(flowText(.back))
                .accessibilityIdentifier("mobile_page_back")
            }
        }

        if navigation.page != .connect {
            ToolbarItem(placement: trailingToolbarPlacement) {
                if navigation.page != .activity {
                Button {
                    showPage(.activity)
                } label: {
                    Image(systemName: "clock.arrow.circlepath")
                        .font(.body.weight(.semibold))
                }
                .accessibilityLabel(flowText(.activity))
                .accessibilityIdentifier("open_activity")
                }
            }
        }

        ToolbarItem(placement: trailingToolbarPlacement) {
            if navigation.page != .settings {
                Button {
                    showPage(.settings)
                } label: {
                    Image(systemName: "gearshape")
                        .font(.body.weight(.semibold))
                }
                .accessibilityLabel(flowText(.settings))
                .accessibilityIdentifier("open_settings")
            }
        }
    }

    private var leadingToolbarPlacement: ToolbarItemPlacement {
        #if os(iOS)
        return .topBarLeading
        #else
        return .navigation
        #endif
    }

    private var trailingToolbarPlacement: ToolbarItemPlacement {
        #if os(iOS)
        return .topBarTrailing
        #else
        return .primaryAction
        #endif
    }

    @ViewBuilder
    private func transferContent(_ route: MobileTransferRoute) -> some View {
        switch route {
        case .send:
            SendView(
                viewModel: model.send,
                initialFiles: initialSendFiles,
                initialFileAccess: initialSendFileAccess,
                initialPendingSelectionID: initialPendingSendSelectionID,
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

    private var initialSendFiles: [URL] {
        guard preservedSendSelection.items.isEmpty else {
            return preservedSendSelection.items
        }
        return model.pendingSendSelection?.fileURLs ?? []
    }

    private var initialSendFileAccess: AnyObject? {
        guard preservedSendSelection.items.isEmpty else {
            return preservedSendSelection.sourceAccess
        }
        return model.pendingSendSelection?.sourceAccess
    }

    private var initialPendingSendSelectionID: UUID? {
        guard preservedSendSelection.items.isEmpty else {
            return preservedSendSelection.pendingSelectionID
        }
        return model.pendingSendSelection?.id
    }

    private var pageTitle: String {
        pageTitle(navigation.page)
    }

    private func pageTitle(_ page: MobilePage) -> String {
        switch page {
        case .connect: return "Envoix"
        case .room: return flowText(.room)
        case .activity: return flowText(.activity)
        case .settings: return flowText(.settings)
        }
    }

    private func transferTitle(_ route: MobileTransferRoute) -> String {
        switch route {
        case .send: return flowText(.offerFiles)
        case .receive: return flowText(.receiveFiles)
        }
    }

    private func flowText(_ copy: MobileConnectionFlowCopy) -> String {
        MobileConnectionFlowPresentationText.value(copy, language: language)
    }

    private var pendingOfferBinding: Binding<PendingNearbyInvitation?> {
        Binding(
            get: {
                guard runtime.isPresentationOwner(sceneID) else { return nil }
                #if os(iOS)
                guard pendingExternalInvitation == nil else { return nil }
                #endif
                return workflow.nextPendingOffer
            },
            set: { value in
                if runtime.isPresentationOwner(sceneID),
                   value == nil,
                   let pending = workflow.nextPendingOffer {
                    workflow.discardPendingOffer(id: pending.id)
                }
            }
        )
    }

    private var nearbyInviteOffer: NearbyInviteOffer? {
        guard !transferUsesInboundInvite,
              !transferUsesRoomControl,
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
        if usesBluetoothVerification(selection) {
            if workflow.canReuseHostingInvitation(for: selection),
               let verification = outgoingBleVerification {
                deliverRoomInvitation(verification.publicOffer, to: selection)
                return
            }
            guardRoomReplacement {
                do {
                    let verification = try BleVerificationInvitation.make(
                        broker: serverURL,
                        relay: relayURL
                    )
                    guard startHostingRoom(
                        nearbySelection: selection,
                        invitationInput: verification.privateInvitation,
                        verifiedPeerLabel: selection.displayName ?? "Nearby Envoix device"
                    ) else { return }
                    outgoingBleVerification = verification
                    deliverRoomInvitation(verification.publicOffer, to: selection)
                } catch {
                    ToastCenter.shared.show(error.localizedDescription)
                }
            }
            return
        }
        outgoingBleVerification = nil
        if workflow.canReuseHostingInvitation(for: selection),
           let payload = workflow.roomInvitation?.payload {
            deliverRoomInvitation(payload, to: selection)
            return
        }
        guardRoomReplacement {
            guard startHostingRoom(nearbySelection: selection) else { return }
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
                if outgoingBleVerification?.publicOffer == payload {
                    outgoingBleVerification = nil
                    closeRoomNow()
                }
                ToastCenter.shared.show(error)
            }
        }
    }

    private func openPairingRoom(input: String) -> String? {
        let pairingInput = input.trimmed
        guard !pairingInput.isEmpty else {
            return flowText(.connectionInputRequired)
        }

        let classified: ClassifiedConnectionInput
        do {
            classified = try classifyConnectionInput(
                pairingInput,
                fallbackBroker: serverURL,
                fallbackRelay: relayURL,
                allowBareRoomControl: true
            )
        } catch {
            return flowText(.connectionInputInvalid)
        }

        if classified.kind == .roomControl {
            guard !isRoomOccupied else {
                return flowText(.roomOccupied)
            }
            guard let identityPath = roomIdentityPath else {
                return flowText(.applicationSupportUnavailable)
            }
            let error = workflow.joinRoomControl(
                input: classified.normalizedInput,
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
        guard let invitation = classified.pairingInvite else {
            return flowText(.inviteV2Unavailable)
        }
        action = ConnectionWorkflowPolicy.localAction(
            forLocalRole: invitation.joinerRole
        )

        workflow.discardAllPendingOffers()
        workflow.openRoom(
            origin: .pairingCode,
            pairingInput: classified.normalizedInput,
            suggestedAction: action,
            existingActivityIDs: Set(model.activities.map(\.activityId))
        )
        resetRoomTransferHandoff()
        navigation.page = .room
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

    private func offerFilesToRememberedRoom(_ relationshipID: String) {
        guard canPresentRememberedRoomSend() else { return }
        openRememberedRoom(relationshipID) {
            presentedSharedSelectionID = model.pendingSendSelection?.id
            offerRememberedRoomFiles()
        }
    }

    private func offerDroppedItemsToRememberedRoom(
        _ relationshipID: String,
        _ urls: [URL]
    ) {
        guard canPresentRememberedRoomSend() else { return }
        do {
            switch try model.importOpenedSendFiles(urls) {
            case .imported:
                offerFilesToRememberedRoom(relationshipID)
            case .queued:
                ToastCenter.shared.show(flowText(.droppedItemsSendBusy))
            }
        } catch let error as OpenedSendFileError {
            ToastCenter.shared.show(openedSendFileErrorMessage(error))
        } catch {
            ToastCenter.shared.show(error.localizedDescription)
        }
    }

    private func canPresentRememberedRoomSend() -> Bool {
        guard transferRoute == nil, !model.send.isBusy else {
            ToastCenter.shared.show(flowText(.anotherSendBusy))
            return false
        }
        return true
    }

    private func openRememberedRoom(_ relationshipID: String) {
        openRememberedRoom(relationshipID) {
            presentPendingSendSelection()
        }
    }

    private func openRememberedRoom(
        _ relationshipID: String,
        onOpened: @escaping () -> Void
    ) {
        if workflow.activeRememberedRelationshipID == relationshipID
            || workflow.rememberedRoom?.relationshipID == relationshipID {
            openRememberedRoomNow(relationshipID, onOpened: onOpened)
            return
        }
        guardRoomReplacement {
            openRememberedRoomNow(relationshipID, onOpened: onOpened)
        }
    }

    private func openRememberedRoomNow(
        _ relationshipID: String,
        onOpened: @escaping () -> Void
    ) {
        if let error = workflow.openRememberedRoom(
            relationshipID: relationshipID,
            existingActivityIDs: Set(model.activities.map(\.activityId))
        ) {
            ToastCenter.shared.show(error)
            return
        }
        resetRoomTransferHandoff()
        navigation.page = .room
        DispatchQueue.main.async {
            onOpened()
        }
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
            navigation.page = .connect
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
        ToastCenter.shared.show(flowText(.queuedForReconnect))
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
        DispatchQueue.main.async {
            presentPendingSendSelection()
        }
    }

    private func requestCloseRoom() {
        if isEndedOrFailed(workflow.controlPhase) {
            closeRoomNow()
            return
        }
        let authenticatedRoomIsOpen =
            workflow.room?.origin == .roomControl &&
                workflow.controlPhase == .connected
        if authenticatedRoomIsOpen || roomHasActiveTransfers {
            isCloseRoomConfirmationPresented = true
        } else {
            closeRoomNow()
        }
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
        presentedSharedSelectionID = nil
        if isControlRoomOpen || workflow.room?.origin == .roomControl {
            workflow.endControl(reason: .userEnded)
        } else {
            workflow.closeRoom()
        }
        navigation.page = .connect
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

    private func assignActivityToActiveRoom(_ activityID: String) {
        if let room = workflow.rememberedRoom {
            _ = model.assignActivityGroup(
                activityID: activityID,
                groupID: "remembered:\(room.relationshipID)",
                label: room.label
            )
            return
        }
        guard let room = workflow.room else { return }
        _ = model.assignActivityGroup(
            activityID: activityID,
            groupID: "one-time:\(room.id.uuidString.lowercased())",
            label: workflow.peerDisplayName ?? room.nearbySelection?.displayName
        )
    }

    private func selectedPeerIsVisible(_ room: OneTimeRoomSession) -> Bool {
        guard let key = room.nearbySelection?.discoveryPeerKey else { return false }
        return nearbyCoordinator.state.peers.contains { $0.peerKey == key }
    }

    private func showPage(_ destination: MobilePage) {
        navigation.show(destination)
    }

    private func navigateBack() {
        switch navigation.page {
        case .room:
            #if os(macOS)
            if selectedHelperDeviceID != nil {
                selectedHelperDeviceID = nil
                navigation.page = .connect
                return
            }
            #endif
            if workflow.rememberedRoom != nil {
                workflow.unpinRememberedRoom()
                navigation.page = .connect
            } else {
                requestCloseRoom()
            }
        case .activity, .settings:
            navigation.returnToContext(hasActiveRoom: hasNavigableRoom)
        case .connect:
            break
        }
    }

    private var hasNavigableRoom: Bool {
        #if os(macOS)
        return selectedHelperDevice != nil || workflow.activeRoomID != nil
        #else
        return workflow.activeRoomID != nil
        #endif
    }

    #if os(macOS)
    private var selectedHelperDevice: MacOSAgentDevice? {
        guard let selectedHelperDeviceID else { return nil }
        return helperTransfers.devices.first { $0.id == selectedHelperDeviceID }
    }
    #endif

    private func updateRuntimeRequest() {
        let effects = MobileSceneLifecyclePolicy.effects(
            for: MobileSceneLifecycleEvent(scenePhase: scenePhase)
        )
        #if DEBUG
        // XCTest can report an inactive initial scene for the entire launch.
        // Fixture providers are process-local and safe to keep active so UI
        // tests exercise deterministic discovery instead of scene timing.
        let sceneAllowsDiscovery = effects.allowsNearbyDiscovery
            || ProcessInfo.processInfo.arguments.contains("--ui-testing")
        let sceneAllowsPresentation = scenePhase == .active
            || ProcessInfo.processInfo.arguments.contains("--ui-testing")
        #else
        let sceneAllowsDiscovery = effects.allowsNearbyDiscovery
        let sceneAllowsPresentation = scenePhase == .active
        #endif
        let shouldRun = NearbyDiscoveryLeasePolicy.shouldRun(
            sceneAllowsDiscovery: sceneAllowsDiscovery,
            isConnectionPage: navigation.page == .connect,
            discoveryIsEnabled: presence.visibility != .hidden,
            systemPairingIsActive: false
        )
        runtime.updateScene(
            id: sceneID,
            isActive: sceneAllowsPresentation,
            requestsDiscovery: shouldRun,
            keepsRememberedConnected: RememberedRoomLifecyclePolicy.shouldKeepConnected(
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
        updateRuntimeRequest()
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
        guard runtime.isPresentationOwner(sceneID) else { return }
        guard let offer = nearbyCoordinator.state.incomingRendezvousOffer else { return }
        defer { nearbyCoordinator.consumeRendezvousOffer(id: offer.id) }
        if navigation.page == .room,
           let selectedPeerKey = workflow.room?.nearbySelection?.discoveryPeerKey,
           offer.senderPeerKey != selectedPeerKey {
            return
        }
        if BleVerificationInvitation.isPublicOffer(offer.invite) {
            guard offer.source == .bluetooth else { return }
            bleVerificationInput = ""
            pendingBleVerificationOffer = offer
            return
        }
        guard connectionInputKind(
            offer.invite,
            allowBareRoomControl: false
        ) != nil else {
            ToastCenter.shared.show(flowText(.invalidNearbyInvitation))
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
        let continuesCurrentNearbyRoom = connectionInputKind(
            pending.offer.invite,
            allowBareRoomControl: false
        ) == .inviteV2
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
        let selection = nearbySelection(for: pending.offer)
        if connectionInputKind(
            pending.offer.invite,
            allowBareRoomControl: false
        ) == .roomControl {
            guard let identityPath = roomIdentityPath else { return }
            if let error = workflow.joinRoomControl(
                input: pending.offer.invite,
                broker: serverURL,
                relay: relayURL,
                displayName: presence.displayName,
                identityPath: identityPath,
                existingActivityIDs: Set(model.activities.map(\.activityId)),
                nearbySelection: selection
            ) {
                ToastCenter.shared.show(error)
            }
            return
        }
        guard let parsed = try? parsePairingInvite(input: pending.offer.invite) else { return }
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
        navigation.page = .room
        DispatchQueue.main.async {
            switch action {
            case .offerFiles: offerFiles()
            case .receiveFiles: receiveFiles()
            case .choose: break
            }
        }
    }

    private func acceptBleVerificationOffer(
        _ offer: NearbyRendezvousOffer,
        code: String
    ) -> String? {
        guard offer.source == .bluetooth,
              let invitation = BleVerificationInvitation.resolve(
                  publicOffer: offer.invite,
                  verificationCode: code
              ) else {
            return flowText(.currentVerificationCodeRequired)
        }
        guard let identityPath = roomIdentityPath else {
            return flowText(.applicationSupportUnavailable)
        }
        pendingBleVerificationOffer = nil
        guardRoomReplacement {
            if let error = workflow.joinRoomControl(
                input: invitation,
                broker: serverURL,
                relay: relayURL,
                displayName: presence.displayName,
                identityPath: identityPath,
                existingActivityIDs: Set(model.activities.map(\.activityId)),
                nearbySelection: nearbySelection(for: offer),
                verifiedPeerLabel: offer.senderDisplayName ?? "Nearby Envoix device"
            ) {
                ToastCenter.shared.show(error)
            }
        }
        return nil
    }

    private func usesBluetoothVerification(_ selection: NearbyPairingSelection) -> Bool {
        selection.nearbyWifiAwareDeviceID == nil
            && !(selection.sources.contains(.mdns) && selection.nearbyInviteRoute != nil)
            && selection.sources.contains(.bluetooth)
    }

    private func nearbySelection(
        for offer: NearbyRendezvousOffer
    ) -> NearbyPairingSelection {
        let capturedRoute = nearbyCoordinator.state.peers.first { peer in
            peer.peerKey == offer.senderPeerKey
                && peer.inviteRoute?.endpointID == offer.senderInboxEndpointID
        }?.inviteRoute
        return NearbyPairingSelection(
            discoveryPeerKey: offer.senderPeerKey,
            displayName: offer.senderDisplayName,
            sources: [offer.source],
            nearbyInviteRoute: capturedRoute,
            nearbyWifiAwareDeviceID: offer.senderWifiAwareDeviceID
        )
    }

    private func handleIncomingURL(_ url: URL) {
        #if os(iOS)
        if let id = ShareDraftLink.draftID(from: url) {
            presentSharedDraft(preferredID: id)
            return
        }
        #if canImport(CoreNFC)
        if url.absoluteString.hasPrefix(NFCInvitationNDEFCodec.carrierPrefix) {
            do {
                stageExternalInvitation(
                    try NFCInvitationNDEFCodec.invitation(fromCarrierURL: url),
                    origin: .universalLink
                )
            } catch {
                ToastCenter.shared.show(error.localizedDescription)
            }
            return
        }
        #endif
        #endif
        let input = url.absoluteString
        if input.hasPrefix(inviteV2URLPrefix)
            || input.hasPrefix(roomControlURLPrefix) {
            #if os(iOS) && canImport(CoreNFC)
            do {
                stageExternalInvitation(
                    try NFCInvitationNDEFCodec.invitation(fromDirectURL: url),
                    origin: .deepLink
                )
            } catch {
                ToastCenter.shared.show(error.localizedDescription)
            }
            #else
            openConfirmedExternalInvitation(input)
            #endif
            return
        }
        guard url.isFileURL else { return }

        do {
            switch try model.importOpenedSendFile(url) {
            case .imported:
                routePendingSendSelection(notifyWaiting: true)
            case .queued:
                ToastCenter.shared.show(flowText(.openedFileQueued))
            }
        } catch let error as OpenedSendFileError {
            ToastCenter.shared.show(openedSendFileErrorMessage(error))
        } catch {
            ToastCenter.shared.show(error.localizedDescription)
        }
    }

    private func openConfirmedExternalInvitation(_ input: String) {
        guardRoomReplacement {
            if let error = openPairingRoom(input: input) {
                ToastCenter.shared.show(error)
            }
        }
    }

    private var nfcInvitationIsAvailable: Bool {
        #if os(iOS) && canImport(CoreNFC)
        return NFCInvitationExchange.isAvailable
        #else
        return false
        #endif
    }

    private var nfcInvitationIsActive: Bool {
        #if os(iOS) && canImport(CoreNFC)
        return nfcInvitationExchange.isActive
        #else
        return false
        #endif
    }

    #if os(iOS) && canImport(CoreNFC)
    private var automaticNFCPresentationEnvironment:
        AutomaticNFCPresentationEnvironment {
        AutomaticNFCPresentationEnvironment(
            transferSheetIsPresented: transferRoute != nil,
            destinationRepairIsPresented: roomDestinationRepair != nil,
            scannerIsPresented: scannerIsPresented,
            manualEntryIsPresented: manualEntryIsPresented,
            nearbyOfferAlertIsPresented: workflow.nextPendingOffer != nil,
            closeRoomAlertIsPresented: isCloseRoomConfirmationPresented,
            replaceRoomAlertIsPresented: isRoomReplacementPresented,
            roomVerificationIsPresented: workflow.verificationRequested,
            externalConfirmationIsPresented: pendingExternalInvitation != nil,
            systemPairingIsPresented: runtime.isSystemPairingActive,
            connectionHubModalIsPresented: connectionHubModalIsPresented
        )
    }
    #endif

    private func beginOfferGatedNFCReadIfNeeded() {
        #if os(iOS) && canImport(CoreNFC)
        guard runtime.isPresentationOwner(sceneID) else { return }
        guard let offer =
                  nearbyCoordinator.state.incomingNFCReadinessOffer else {
            return
        }
        let nowMilliseconds = Int64(
            ProcessInfo.processInfo.systemUptime * 1_000
        )
        guard offer.isFresh(at: nowMilliseconds) else {
            nearbyCoordinator.consumeNFCReadinessOffer(id: offer.id)
            return
        }
        let eligibleBluetoothPeerKeys = Set(
            nearbyCoordinator.state.peers.lazy
                .filter { $0.sources.contains(.bluetooth) }
                .map(\.peerKey)
        )
        let applicationIsActive =
            UIApplication.shared.applicationState == .active
                && scenePhase == .active
        guard offerGatedNFCReadIsAllowedForProcess,
              NFCInvitationExchange.isAvailable,
              !nfcInvitationExchange.isActive,
              !automaticNFCPresentationEnvironment.hasConflict,
              !isRoomOccupied else {
            return
        }
        var gate = nfcReadinessGate
        guard gate.claim(
            offer: offer,
            nowMilliseconds: nowMilliseconds,
            applicationIsActive: applicationIsActive,
            isConnectPage: navigation.page == .connect,
            eligibleBluetoothPeerKeys: eligibleBluetoothPeerKeys
        ) else {
            return
        }
        nfcReadinessGate = gate
        nearbyCoordinator.consumeNFCReadinessOffer(id: offer.id)
        beginNFCInvitationRead(
            timeout: offer.remainingLifetimeSeconds(at: nowMilliseconds)
        )
        #endif
    }

    #if os(iOS) && canImport(CoreNFC)
    private var offerGatedNFCReadIsAllowedForProcess: Bool {
        #if DEBUG
        let processInfo = ProcessInfo.processInfo
        return !processInfo.arguments.contains("--ui-testing")
            && processInfo.environment["XCTestConfigurationFilePath"] == nil
        #else
        return true
        #endif
    }
    #endif

    private func scanNFCInvitation() {
        #if os(iOS) && canImport(CoreNFC)
        nfcReadinessGate.didBeginManualRead()
        if let offer = nearbyCoordinator.state.incomingNFCReadinessOffer {
            nearbyCoordinator.consumeNFCReadinessOffer(id: offer.id)
        }
        beginNFCInvitationRead(timeout: nil)
        #endif
    }

    #if os(iOS) && canImport(CoreNFC)
    private func beginNFCInvitationRead(timeout: TimeInterval?) {
        nfcInvitationExchange.beginReadingEnvoixPhone(
            prompt: flowText(.nfcReadPrompt),
            timeout: timeout
        ) { result in
            switch result {
            case .success(let invitation):
                stageExternalInvitation(invitation, origin: .nfc)
            case .cancelled:
                break
            case .failure(let error):
                ToastCenter.shared.show(error.localizedDescription)
            }
        }
    }
    #endif

    #if os(iOS)
    private func stageExternalInvitation(
        _ invitation: String,
        origin: ExternalInvitationOrigin
    ) {
        guard ExternalInvitationRoutingPolicy.shouldStage(
            invitation: invitation,
            pendingInvitation: pendingExternalInvitation?.invitation,
            openedInvitation: openedExternalInvitation
        ) else {
            return
        }
        pendingExternalInvitation = PendingExternalInvitation(
            invitation: invitation,
            origin: origin
        )
    }

    private func continueExternalInvitation(_ pending: PendingExternalInvitation) {
        guard pendingExternalInvitation?.id == pending.id else { return }
        pendingExternalInvitation = nil
        guardRoomReplacement {
            if let error = openPairingRoom(input: pending.invitation) {
                ToastCenter.shared.show(error)
            } else {
                openedExternalInvitation = pending.invitation
            }
        }
    }

    private func cancelExternalInvitation(_ pending: PendingExternalInvitation) {
        guard pendingExternalInvitation?.id == pending.id else { return }
        pendingExternalInvitation = nil
    }
    #endif

    private func presentPendingSendSelection() {
        guard runtime.isPresentationOwner(sceneID) else { return }
        #if os(iOS)
        guard pendingExternalInvitation == nil else { return }
        presentSharedDraft(preferredID: nil)
        #else
        routePendingSendSelection(notifyWaiting: false)
        #endif
    }

    #if os(iOS)
    private func presentSharedDraft(preferredID: UUID?) {
        do {
            switch try model.importSharedSendDraft(preferredID: preferredID) {
            case .imported:
                routePendingSendSelection(notifyWaiting: true)
            case .alreadyImported:
                routePendingSendSelection(notifyWaiting: false)
            case .noPendingDraft:
                routePendingSendSelection(notifyWaiting: false)
            case .sendBusy:
                ToastCenter.shared.show(flowText(.sharedItemSendBusy))
            }
        } catch {
            ToastCenter.shared.show(error.localizedDescription)
        }
    }
    #endif

    private func routePendingSendSelection(notifyWaiting: Bool) {
        let selectionID = model.pendingSendSelection?.id
        let destination = ConnectionWorkflowPolicy.pendingSharedSendDestination(
            hasPendingSelection: selectionID != nil,
            sendIsBusy: model.send.isBusy,
            transferIsPresented: transferRoute != nil,
            selectionWasPresented: selectionID == presentedSharedSelectionID,
            hasConnectedOneTimeRoom: workflow.controlPhase == .connected
                && workflow.room?.origin == .roomControl,
            hasConnectedRememberedRoom: workflow.controlPhase == .connected
                && workflow.hasPinnedRememberedRoom
        )
        switch destination {
        case .none:
            break
        case .connectionHub:
            navigation.page = workflow.room == nil && !workflow.hasPinnedRememberedRoom
                ? .connect
                : .room
            if notifyWaiting {
                ToastCenter.shared.show(flowText(.sharedItemsNeedRoom))
            }
        case .oneTimeRoom:
            presentedSharedSelectionID = selectionID
            navigation.page = .room
            offerFiles()
        case .rememberedRoom:
            presentedSharedSelectionID = selectionID
            navigation.page = .room
            offerRememberedRoomFiles()
        }
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
        let wasRevealed = roomInvitationIsRevealed
        if startHostingRoom() {
            roomInvitationIsRevealed = wasRevealed
        }
    }

    @discardableResult
    private func startHostingRoom(
        nearbySelection: NearbyPairingSelection? = nil,
        invitationInput: String? = nil,
        verifiedPeerLabel: String? = nil
    ) -> Bool {
        guard let identityPath = roomIdentityPath else {
            ToastCenter.shared.show(flowText(.applicationSupportUnavailable))
            return false
        }
        let error = workflow.startHosting(
            broker: serverURL,
            relay: relayURL,
            displayName: presence.displayName,
            identityPath: identityPath,
            existingActivityIDs: Set(model.activities.map(\.activityId)),
            nearbySelection: nearbySelection,
            invitationInput: invitationInput,
            verifiedPeerLabel: verifiedPeerLabel
        )
        if let error {
            ToastCenter.shared.show(error)
            return false
        }
        if nearbySelection != nil {
            roomInvitationIsRevealed = false
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

    private func connectionInputKind(
        _ input: String,
        allowBareRoomControl: Bool
    ) -> ConnectionInputKind? {
        try? classifyConnectionInput(
            input,
            fallbackBroker: serverURL,
            fallbackRelay: relayURL,
            allowBareRoomControl: allowBareRoomControl
        ).kind
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
        updateRuntimeRequest()
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
            ToastCenter.shared.show(flowText(.anotherReceiveBusy))
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
                throw RuntimeSettingsError(flowText(.offerRouteMismatch))
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

        // Once the user accepts, destination preparation and the native
        // Wi-Fi Aware listener may legitimately cross the display deadline.
        // Hold the offer before either asynchronous step can re-enter `tick`.
        guard workflow.holdIncomingRoomOfferForDestination(id: offer.id) else {
            return
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
                        destinationAccess: destination.access,
                        nearbyWifiAwareDeviceID: workflow.room?
                            .nearbySelection?.nearbyWifiAwareDeviceID
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
                ToastCenter.shared.show(flowText(.offerUnavailable))
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
                throw RuntimeSettingsError(flowText(.saveFolderInaccessible))
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
                throw RuntimeSettingsError(flowText(.saveFolderPermissionExpired))
            }
            try validateWritableDirectoryAccess(url)
            return (url, access)
        }

        #if os(macOS)
        throw RuntimeSettingsError(flowText(.saveFolderRequiredOnMac))
        #else
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
        #endif
    }

    private func openedSendFileErrorMessage(_ error: OpenedSendFileError) -> String {
        switch error {
        case .unsupportedURL:
            return flowText(.localFilesOnly)
        case .unsupportedItem:
            return flowText(.unsupportedItem)
        case .inaccessible:
            return flowText(.inaccessibleItem)
        case .itemCountExceeded:
            return MobileConnectionFlowPresentationText.itemCountExceeded(
                maximum: ShareDraftStore.maxItemCount,
                language: language
            )
        }
    }

    #if DEBUG && os(iOS)
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
                Text(manualText(.manualEntryDetail))
                .font(.subheadline)
                .foregroundStyle(Theme.muted)
                .fixedSize(horizontal: false, vertical: true)

                HStack(alignment: .top, spacing: 8) {
                    TextField(
                        manualText(.manualEntryTitle),
                        text: Binding(
                            get: { input },
                            set: { input = formatRoomCodeInput($0) }
                        ),
                        axis: .vertical
                    )
                    #if os(iOS)
                    .textInputAutocapitalization(.never)
                    .autocorrectionDisabled()
                    #endif
                    .textFieldStyle(.roundedBorder)
                    .accessibilityIdentifier("manual_pairing_code_input")

                    Button(action: pastePairingInput) {
                        Label(
                            manualText(.paste),
                            systemImage: "doc.on.clipboard"
                        )
                        .frame(minHeight: 36)
                    }
                    .buttonStyle(.bordered)
                    .accessibilityIdentifier("manual_pairing_code_paste")
                }
                .onChange(of: input) { _ in
                    error = nil
                }

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
                    Text(manualText(.continueAction))
                        .frame(maxWidth: .infinity, minHeight: 44)
                }
                .buttonStyle(PrimaryActionButtonStyle())
                .disabled(input.trimmed.isEmpty)
                .accessibilityIdentifier("manual_pairing_code_submit")

                Spacer()
            }
            .padding(20)
            .background(Theme.bg)
            .navigationTitle(manualText(.manualEntryTitle))
            #if os(iOS)
            .navigationBarTitleDisplayMode(.inline)
            #endif
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button(manualText(.close)) {
                        dismiss()
                    }
                }
            }
        }
        #if os(iOS)
        .presentationDetents([.medium, .large])
        #endif
    }

    private func pastePairingInput() {
        guard let value = pasteboardString()?.trimmed, !value.isEmpty else {
            error = manualText(.clipboardEmpty)
            return
        }
        input = value
        error = nil
    }

    private func manualText(_ copy: MobileConnectionFlowCopy) -> String {
        MobileConnectionFlowPresentationText.value(copy, language: language)
    }
}

private struct QRScannerPresentationModifier: ViewModifier {
    @Binding var isPresented: Bool
    let language: String
    let onScan: (String) -> String?

    @ViewBuilder
    func body(content: Content) -> some View {
        #if os(iOS)
        content.fullScreenCover(isPresented: $isPresented) {
            QRCodeScannerSheet(language: language) { value in
                onScan(value)
            }
        }
        #else
        content
        #endif
    }
}
#endif
