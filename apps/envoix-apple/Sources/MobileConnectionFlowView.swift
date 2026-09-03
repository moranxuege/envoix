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
                        AppText.value("Cancel", "取消", language: language),
                        action: onCancel
                    )
                    .buttonStyle(.bordered)
                    .accessibilityIdentifier("external_invitation_cancel")

                    Button(
                        AppText.value("Continue", "继续", language: language),
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
        if origin == .nfc {
            return AppText.value(
                isRoomInvitation
                    ? "Nearby Envoix room found"
                    : "Nearby Envoix invitation found",
                isRoomInvitation
                    ? "发现附近的 Envoix 房间"
                    : "发现附近的 Envoix 邀请",
                language: language
            )
        }
        return AppText.value(
            isRoomInvitation ? "Join this room?" : "Open invitation?",
            isRoomInvitation ? "加入此房间？" : "打开邀请？",
            language: language
        )
    }

    private var confirmationMessage: String {
        if origin == .nfc {
            return AppText.value(
                "NFC confirms touch-range proximity, not the other phone's identity. Continue to validate this one-time invitation and connect.",
                "NFC 仅确认另一台手机处于触碰距离内，并不代表其身份已经验证。继续后将验证此一次性邀请并连接。",
                language: language
            )
        }
        return AppText.value(
            isRoomInvitation
                ? "This external room invitation is untrusted. Continue to validate it and connect; it does not authenticate the other device."
                : "This external invitation is untrusted. Continue to validate it and choose the normal transfer action; it does not authenticate the other device.",
            isRoomInvitation
                ? "此房间邀请来自外部且未经信任。继续后将验证并连接；它不会认证另一台设备。"
                : "此外部邀请未经信任。继续后仍需验证并选择常规传输操作；它不会认证另一台设备。",
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
                            .accessibilityLabel(AppText.value("Close", "关闭", language: language))
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
            AppText.value("Verify nearby device", "验证附近设备", language: language),
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
            Button(AppText.value("Cancel verification", "取消验证", language: language), role: .destructive) {
                outgoingBleVerification = nil
                closeRoomNow()
            }
        } message: {
            Text(AppText.value(
                "Enter \(outgoingBleVerification?.verificationCode ?? "") on the other device. The code is never sent over Bluetooth.",
                "请在另一台设备上输入 \(outgoingBleVerification?.verificationCode ?? "")。验证码不会通过蓝牙发送。",
                language: language
            ))
            .privacySensitive()
        }
        .alert(
            AppText.value("Enter verification code", "输入验证码", language: language),
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
            Button(AppText.value("Verify and connect", "验证并连接", language: language)) {
                guard let offer = pendingBleVerificationOffer else { return }
                if let error = acceptBleVerificationOffer(offer, code: bleVerificationInput) {
                    ToastCenter.shared.show(error)
                }
                bleVerificationInput = ""
            }
            .disabled(bleVerificationInput.count != 6)
            Button(AppText.value("Cancel", "取消", language: language), role: .cancel) {
                pendingBleVerificationOffer = nil
                bleVerificationInput = ""
            }
        } message: {
            Text(AppText.value(
                "Ask the other person for the six-digit code shown in Envoix.",
                "请向对方确认 Envoix 中显示的六位验证码。",
                language: language
            ))
        }
        .alert(
            AppText.value("Verify this device", "验证此设备", language: language),
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
            Button(AppText.value("Verify device", "验证设备", language: language)) {
                if let error = workflow.submitDeviceVerification(roomVerificationInput) {
                    ToastCenter.shared.show(error)
                }
                roomVerificationInput = ""
            }
            .disabled(roomVerificationInput.count != 6)
            .accessibilityIdentifier("room_device_verification_submit")
            Button(AppText.value("Cancel", "取消", language: language), role: .cancel) {
                workflow.cancelDeviceVerification()
                roomVerificationInput = ""
            }
        } message: {
            Text(AppText.value(
                "Enter the six-digit code shown by \(workflow.peerDisplayName ?? "the other device"). A successful match saves this device for future rooms.",
                "请输入 \(workflow.peerDisplayName ?? "另一台设备") 显示的六位验证码。匹配成功后会保存此设备，以便以后自动连接。",
                language: language
            ))
        }
        .alert(item: pendingOfferBinding) { pending in
            let isRoomInvite = connectionInputKind(
                pending.offer.invite,
                allowBareRoomControl: false
            ) == .roomControl
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
            Button(AppText.value("Keep room", "保留房间", language: language), role: .cancel) {}
            Button(AppText.value("End room", "结束房间", language: language), role: .destructive) {
                closeRoomNow()
            }
        } message: {
            Text(AppText.value(
                "New file offers will stop. Transfers already in progress will continue in Activity.",
                "结束后将无法发送新文件。已经开始的传输会继续显示在“活动”中。",
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
                    navigation.page = .room
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
            ToastCenter.shared.show(AppText.value(
                "\(label) is now paired through the background helper.",
                "已通过后台 helper 与 \(label) 完成配对。",
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
                        AppText.value("New Window", "新建窗口", language: language),
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
            ToastCenter.shared.show(AppText.value(
                "Refresh paired devices and try again.",
                "请刷新已配对设备后重试。",
                language: language
            ))
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
            ToastCenter.shared.show(AppText.value(
                "Refresh paired devices and try again.",
                "请刷新已配对设备后重试。",
                language: language
            ))
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
                ToastCenter.shared.show(AppText.value(
                    "Queued for \(device.label).",
                    "已加入发送队列：\(device.label)。",
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
                    Text(AppText.value(
                        "Background helper",
                        "后台 helper",
                        language: language
                    ))
                    .tag(false)
                    Text(AppText.value(
                        "One-time transfers",
                        "一次性传输",
                        language: language
                    ))
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
                .accessibilityLabel(AppText.value("Activity", "活动", language: language))
                .accessibilityIdentifier("open_activity")
            }
        } else {
            ToolbarItem(placement: leadingToolbarPlacement) {
                Button(action: navigateBack) {
                    Image(systemName: "chevron.left")
                        .font(.body.weight(.semibold))
                        .frame(width: 40, height: 40)
                }
                .accessibilityLabel(AppText.value("Back", "返回", language: language))
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
                .accessibilityLabel(AppText.value("Activity", "活动", language: language))
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
                .accessibilityLabel(AppText.value("Settings", "设置", language: language))
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
            return AppText.value(
                "Enter an Envoix InviteV2 link, Room link, or Room code.",
                "请输入 Envoix InviteV2 链接、房间链接或房间码。",
                language: language
            )
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
            return AppText.value(
                "This is not a valid Envoix InviteV2 link, Room link, or current Room code.",
                "这不是有效的 Envoix InviteV2 链接、房间链接或当前房间码。",
                language: language
            )
        }

        if classified.kind == .roomControl {
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
            return AppText.value(
                "This InviteV2 link could not be opened.",
                "无法打开此 InviteV2 链接。",
                language: language
            )
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
                ToastCenter.shared.show(AppText.value(
                    "Finish the current send, then drop the items again.",
                    "请先完成当前发送，再重新拖入这些项目。",
                    language: language
                ))
            }
        } catch let error as OpenedSendFileError {
            ToastCenter.shared.show(openedSendFileErrorMessage(error))
        } catch {
            ToastCenter.shared.show(error.localizedDescription)
        }
    }

    private func canPresentRememberedRoomSend() -> Bool {
        guard transferRoute == nil, !model.send.isBusy else {
            ToastCenter.shared.show(AppText.value(
                "Finish the current send before starting another one.",
                "请先完成当前发送，再开始新的发送。",
                language: language
            ))
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
            return AppText.value(
                "Enter the current six-digit code shown on the other device.",
                "请输入另一台设备当前显示的六位验证码。",
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
            prompt: AppText.value(
                "Hold the top of this iPhone near one Android phone sharing an Envoix invitation.",
                "请将这台 iPhone 顶部靠近一台正在共享 Envoix 邀请的 Android 手机。",
                language: language
            ),
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
                ToastCenter.shared.show(AppText.value(
                    "Files are ready. Connect to a device to offer them in a Room.",
                    "文件已准备好。请连接设备，并在房间中发送。",
                    language: language
                ))
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

        #if os(macOS)
        throw RuntimeSettingsError(AppText.value(
            "Choose a save folder before accepting files on Mac.",
            "在 Mac 上接收文件前，请先选择保存文件夹。",
            language: language
        ))
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
        case .itemCountExceeded:
            return AppText.value(
                "Choose no more than \(ShareDraftStore.maxItemCount) items.",
                "一次最多选择 \(ShareDraftStore.maxItemCount) 个项目。",
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
                Text(AppText.value(
                    "A current Room code opens a foreground room. A complete InviteV2 link opens a one-time transfer. Neither identifies or trusts the other device.",
                    "当前房间码会打开前台房间；完整 InviteV2 链接会打开一次性传输。两者都不代表设备身份或信任关系。",
                    language: language
                ))
                .font(.subheadline)
                .foregroundStyle(Theme.muted)
                .fixedSize(horizontal: false, vertical: true)

                HStack(alignment: .top, spacing: 8) {
                    TextField(
                        AppText.value("Room code or invite link", "房间码或邀请链接", language: language),
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
                            AppText.value("Paste", "粘贴", language: language),
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
                    Text(AppText.value("Continue", "继续", language: language))
                        .frame(maxWidth: .infinity, minHeight: 44)
                }
                .buttonStyle(PrimaryActionButtonStyle())
                .disabled(input.trimmed.isEmpty)
                .accessibilityIdentifier("manual_pairing_code_submit")

                Spacer()
            }
            .padding(20)
            .background(Theme.bg)
            .navigationTitle(AppText.value(
                "Room code or invite link",
                "房间码或邀请链接",
                language: language
            ))
            #if os(iOS)
            .navigationBarTitleDisplayMode(.inline)
            #endif
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button(AppText.value("Close", "关闭", language: language)) {
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
            error = AppText.value(
                "Clipboard is empty",
                "剪贴板为空",
                language: language
            )
            return
        }
        input = value
        error = nil
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
