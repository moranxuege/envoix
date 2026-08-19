import SwiftUI
import EnvoixCore

struct ReceiveView: View {
    @Environment(\.appLanguage) private var uiLanguage
    @EnvironmentObject private var model: AppModel
    @ObservedObject var viewModel: TransferViewModel
    // Remembered across launches. Empty means "use the platform default".
    @AppStorage("envoix.outputDir") private var outputDirPath: String = ""
    @AppStorage("envoix.outputDirDisplayName") private var outputDirDisplayName: String = ""
    @State private var token = ""
    @AppStorage("envoix.concurrentTransfers") private var concurrentTransfers = true
    @AppStorage("envoix.language") private var language = "en"
    @AppStorage("envoix.serverURL") private var serverURL = ""
    @AppStorage("envoix.relayURL") private var relayURL = ""
    @AppStorage("envoix.candidatesAllow") private var candidatesAllow = ""
    @AppStorage("envoix.candidatesDeny") private var candidatesDeny = ""
    @AppStorage("envoix.speedLimit") private var speedLimit = 40
    @AppStorage("envoix.destinationSaveMode") private var destinationSaveMode = "direct"
    @State private var mode: PairingMode = .room
    #if os(macOS)
    @State private var rememberedPeers: [RememberedPeerSummary] = []
    @State private var selectedRememberedPeer: RememberedPeerSummary?
    #endif
    @State private var rememberAfterPairing = false
    @State private var rememberLabel = ""
    @State private var joiningInvite = ""
    @State private var pairingInvite: FfiPairingInvite?
    @State private var roomQRCodeImage: PlatformImage?
    @State private var roomQRCodePayload = ""
    @State private var pairingPanel: PairingPanelMode = .show
    @State private var revealAddress = false
    @State private var didApplyInitialPairingInput = false
    @State private var didAutoStartRoomControl = false
    @State private var isAcceptingRoomOffer = false
    @StateObject private var nearbyInviteDelivery = NearbyInviteDeliveryController()
    private let initialPairingInput: String?
    private let nearbySelection: NearbyPairingSelection?
    private let nearbyInviteOffer: NearbyInviteOffer?
    private let roomControlTransfer: Bool
    private let roomControlAccept: (() async -> Bool)?
    private let onInitialPairingInputConsumed: (() -> Void)?
    private let onSwitchToSend: ((String) -> Void)?
    #if os(iOS)
    @State private var isFolderPickerPresented = false
    @State private var isQRScannerPresented = false
    @State private var shouldStartAfterFolderPick = false
    #endif

    init(
        viewModel: TransferViewModel,
        initialMode: PairingMode = .room,
        initialPairingInput: String? = nil,
        nearbySelection: NearbyPairingSelection? = nil,
        nearbyInviteOffer: NearbyInviteOffer? = nil,
        roomControlTransfer: Bool = false,
        roomControlAccept: (() async -> Bool)? = nil,
        onInitialPairingInputConsumed: (() -> Void)? = nil,
        onSwitchToSend: ((String) -> Void)? = nil
    ) {
        self.viewModel = viewModel
        self.initialPairingInput = initialPairingInput
        self.nearbySelection = nearbySelection
        self.nearbyInviteOffer = nearbyInviteOffer
        self.roomControlTransfer = roomControlTransfer
        self.roomControlAccept = roomControlAccept
        self.onInitialPairingInputConsumed = onInitialPairingInputConsumed
        self.onSwitchToSend = onSwitchToSend
        _mode = State(initialValue: initialMode)
    }

    private let outputDirBookmarkKey = "envoix.outputDirBookmark"

    /// Suggests ~/Downloads on macOS, but requires one explicit system folder
    /// selection before receiving so authorization can be persisted. iOS uses
    /// the app's Files-visible Documents/Downloads folder by default.
    private var outputDir: URL? {
        #if os(iOS)
        if let data = UserDefaults.standard.data(forKey: outputDirBookmarkKey) {
            return try? resolveSecurityScopedFolderBookmark(data)
        }
        return defaultIOSOutputDir
        #else
        let defaultURL = FileManager.default.urls(for: .downloadsDirectory, in: .userDomainMask).first
            ?? FileManager.default.homeDirectoryForCurrentUser
        return resolveRememberedOutputDirectory(
            bookmarkData: UserDefaults.standard.data(forKey: outputDirBookmarkKey),
            legacyPath: outputDirPath,
            defaultURL: defaultURL
        )
        #endif
    }

    #if os(iOS)
    private var hasCustomOutputDir: Bool {
        UserDefaults.standard.data(forKey: outputDirBookmarkKey) != nil
    }

    private var hasUnavailableCustomOutputDir: Bool {
        hasCustomOutputDir && outputDir == nil
    }

    private var defaultIOSOutputDir: URL {
        let documents = FileManager.default.urls(for: .documentDirectory, in: .userDomainMask).first
            ?? URL(fileURLWithPath: NSHomeDirectory(), isDirectory: true)
                .appendingPathComponent("Documents", isDirectory: true)
        return documents.appendingPathComponent("Downloads", isDirectory: true)
    }
    #endif

    var body: some View {
        #if os(iOS)
        VStack(spacing: 0) {
            scrollContent
        }
        .safeAreaInset(edge: .bottom) { bottomActionBar }
        .sheet(isPresented: $isFolderPickerPresented) {
            FolderPickerSheet(
                onPick: handlePickedOutputFolder,
                onCancel: {
                    shouldStartAfterFolderPick = false
                    isFolderPickerPresented = false
                }
            )
        }
        .sheet(isPresented: $isQRScannerPresented) {
            QRCodeScannerSheet(language: uiLanguage) { value in
                handleScannedInvite(value)
            }
        }
        .onAppear(perform: applyInitialPairingInputIfNeeded)
        .onDisappear(perform: cancelNearbyInviteDelivery)
        #else
        VStack(spacing: 0) {
            scrollContent
            footerMessage
            primaryButton
                .padding(.top, 12)
        }
        .onAppear(perform: applyInitialPairingInputIfNeeded)
        .onAppear(perform: refreshRememberedPeers)
        #endif
    }

    private var scrollContent: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 16) {
                #if os(iOS)
                if let nearbySelection {
                    NearbyTransferContextView(
                        selection: nearbySelection,
                        deliversInvitationOnStart: nearbyInviteOffer != nil,
                        isDelivering: nearbyInviteDelivery.isDelivering,
                        error: nearbyInviteDelivery.error
                    )
                }
                #endif
                outputSection
                if !roomControlTransfer {
                    connectionSection
                }
                #if os(macOS)
                if !viewModel.peerAddress.isEmpty {
                    addressReveal
                }
                #endif

                TransferStatusView(viewModel: viewModel)
            }
            .padding(.vertical, 12)
        }
        .accessibilityIdentifier("receive_content_scroll")
        .onAppear { refreshPairingInviteIfNeeded() }
        .onChange(of: mode) { newMode in
            if newMode == .room {
                refreshPairingInviteIfNeeded()
            }
        }
        .onChange(of: serverURL) { _ in refreshPairingInviteForSettingsChange() }
        .onChange(of: relayURL) { _ in refreshPairingInviteForSettingsChange() }
        .onChange(of: viewModel.isBusy) { isBusy in
            if isBusy {
                joiningInvite = ""
                pairingInvite = nil
                roomQRCodeImage = nil
                roomQRCodePayload = ""
            }
        }
    }

    @ViewBuilder private var connectionSection: some View {
        VStack(alignment: .leading, spacing: 12) {
            #if os(macOS)
            if !rememberedPeers.isEmpty {
                rememberedPeerSection
            }
            #endif
            if mode == .invite {
                inviteSection
            } else if mode == .room {
                roomSection
            } else if mode == .remembered {
                EmptyView()
            } else {
                TokenField(token: $token, disabled: viewModel.isBusy)
                    .card(padding: 14)
            }
            if mode == .room || mode == .invite {
                rememberConsentSection
            }
        }
    }

    #if os(macOS)
    private var rememberedPeerSection: some View {
        VStack(alignment: .leading, spacing: 10) {
            Text(AppText.localized(
                "transfer.pairing.remembered_devices",
                language: uiLanguage
            ))
                .font(.headline.weight(.semibold))
            ForEach(rememberedPeers) { peer in
                HStack {
                    Button {
                        selectedRememberedPeer = peer
                        mode = .remembered
                    } label: {
                        Label(peer.label, systemImage: selectedRememberedPeer?.id == peer.id
                            ? "checkmark.circle.fill"
                            : "laptopcomputer.and.iphone")
                            .frame(maxWidth: .infinity, alignment: .leading)
                    }
                    .buttonStyle(.plain)
                    .disabled(viewModel.isBusy)
                    Button(role: .destructive) {
                        try? RememberedPeerStore.shared.delete(peer)
                        refreshRememberedPeers()
                    } label: {
                        Image(systemName: "trash")
                    }
                    .disabled(viewModel.isBusy)
                    .accessibilityLabel(AppText.localized(
                        "transfer.pairing.forget_device",
                        language: uiLanguage
                    ))
                }
            }
        }
        .card(padding: 14)
    }
    #endif

    private var rememberConsentSection: some View {
        VStack(alignment: .leading, spacing: 8) {
            Toggle(
                AppText.localized("transfer.pairing.remember_device", language: uiLanguage),
                isOn: $rememberAfterPairing
            )
            .disabled(viewModel.isBusy)
            if rememberAfterPairing {
                TextField(
                    AppText.localized("transfer.pairing.device_label", language: uiLanguage),
                    text: $rememberLabel
                )
                .textFieldStyle(.roundedBorder)
                .disabled(viewModel.isBusy)
            }
        }
        .card(padding: 14)
    }

    @ViewBuilder private var footerMessage: some View {
        if concurrencyBlocked {
            Text(AppText.localized(
                "receive.concurrent.finish_send",
                language: uiLanguage
            ))
                .font(.callout)
                .foregroundStyle(Theme.muted)
                .padding(.bottom, 8)
        }
    }

    private var primaryButton: some View {
        Button(action: primaryAction) {
            Label(
                primaryLabel,
                systemImage: canStartAnotherReceive ? "plus.circle" : "tray.and.arrow.down"
            )
                .frame(maxWidth: .infinity, minHeight: 44)
                .contentShape(Rectangle())
        }
        .keyboardShortcut(.defaultAction)
        .buttonStyle(PrimaryActionButtonStyle())
        .disabled(
            (viewModel.isBusy && !canStartAnotherReceive)
                || viewModel.isFinalizing
                || nearbyInviteDelivery.isDelivering
                || isAcceptingRoomOffer
                || !canStart
                || concurrencyBlocked
        )
        .accessibilityIdentifier("receive_start_button")
    }

    #if os(iOS)
    private var bottomActionBar: some View {
        Group {
            if !viewModel.isBusy || canStartAnotherReceive {
                VStack(spacing: 8) {
                    footerMessage
                    primaryButton
                }
                .padding(.horizontal, 16)
                .padding(.top, 10)
                .padding(.bottom, 8)
                .background(.regularMaterial)
            }
        }
    }
    #endif

    private var outputSection: some View {
        VStack(alignment: .leading, spacing: 8) {
            Text(AppText.localized("receive.destination.title", language: uiLanguage))
                .font(.headline.weight(.semibold))
                .foregroundStyle(Theme.text)
            #if os(iOS)
            HStack(spacing: 10) {
                Button {
                    isFolderPickerPresented = true
                } label: {
                    HStack(spacing: 12) {
                        Image(systemName: "folder.fill")
                            .font(.title3.weight(.semibold))
                            .foregroundStyle(Theme.accentStrong)
                            .frame(width: 38, height: 38)
                            .background(Theme.accentSoft, in: RoundedRectangle(cornerRadius: Theme.cardRadius))
                        VStack(alignment: .leading, spacing: 3) {
                            Text(outputDirDisplayText)
                                .font(.body.weight(.semibold))
                                .foregroundStyle(Theme.text)
                                .fixedSize(horizontal: false, vertical: true)
                            Text(outputFolderChooseLabel)
                                .font(.footnote)
                                .foregroundStyle(Theme.muted)
                        }
                        Spacer(minLength: 8)
                        Image(systemName: "chevron.right")
                            .font(.footnote.weight(.semibold))
                            .foregroundStyle(Theme.muted)
                    }
                    .frame(maxWidth: .infinity, minHeight: 72, alignment: .leading)
                    .contentShape(RoundedRectangle(cornerRadius: Theme.cardRadius))
                }
                .buttonStyle(.plain)
                .disabled(viewModel.isBusy)
                .accessibilityIdentifier("receive_destination_picker")
                if hasCustomOutputDir {
                    Button {
                        resetOutputFolder()
                    } label: {
                        Label(
                            AppText.localized("receive.destination.reset", language: uiLanguage),
                            systemImage: "arrow.uturn.backward"
                        )
                            .labelStyle(.iconOnly)
                            .frame(width: 30, height: 30)
                            .contentShape(Rectangle())
                    }
                    .buttonStyle(.bordered)
                    .disabled(viewModel.isBusy)
                }
            }
            .padding(12)
            .background(Theme.surface)
            .overlay(
                RoundedRectangle(cornerRadius: Theme.cardRadius)
                    .strokeBorder(Theme.accent.opacity(0.45), lineWidth: 1)
            )
            .clipShape(RoundedRectangle(cornerRadius: Theme.cardRadius))
            #else
            LinkRow(text: outputDirDisplayText) {
                Button {
                    selectMacOutputFolder(startAfterSelection: false)
                } label: {
                    Label(
                        AppText.localized("receive.destination.select", language: uiLanguage),
                        systemImage: "folder"
                    )
                        .frame(minHeight: 34)
                        .contentShape(Rectangle())
                }
                .disabled(viewModel.isBusy)
            }
            #endif

            Divider().overlay(Theme.line.opacity(0.5))
            Text(AppText.localized(
                "receive.destination.method_title",
                language: uiLanguage
            ))
                .font(.body.weight(.semibold))
                .foregroundStyle(Theme.text)
            Picker(
                AppText.localized("receive.destination.method_title", language: uiLanguage),
                selection: $destinationSaveMode
            ) {
                Text(AppText.localized(
                    "receive.destination.method_direct",
                    language: uiLanguage
                )).tag("direct")
                Text(AppText.localized(
                    "receive.destination.method_copy",
                    language: uiLanguage
                )).tag("copy")
            }
            .pickerStyle(.segmented)
            .labelsHidden()
            .disabled(viewModel.isBusy)
            Text(ReceivePresentationText.saveMethodDetail(
                usesCopy: destinationSaveMode == "copy",
                language: uiLanguage
            ))
                .font(.footnote)
                .foregroundStyle(Theme.muted)
                .fixedSize(horizontal: false, vertical: true)
        }
        .card(padding: 14)
    }

    private var outputDirDisplayText: String {
        #if os(iOS)
        if hasCustomOutputDir {
            if !outputDirDisplayName.trimmed.isEmpty {
                return outputDirDisplayName
            }
            guard let outputDir else {
                return AppText.localized(
                    "receive.destination.ios_unavailable",
                    language: uiLanguage
                )
            }
            return outputDir.lastPathComponent.isEmpty ? outputDir.path : outputDir.lastPathComponent
        }
        return AppText.localized("receive.destination.ios_default", language: uiLanguage)
        #else
        if UserDefaults.standard.data(forKey: outputDirBookmarkKey) != nil,
           outputDir == nil {
            return AppText.localized(
                "receive.destination.macos_unavailable",
                language: uiLanguage
            )
        }
        return outputDir?.path
            ?? AppText.localized("receive.destination.macos_choose", language: uiLanguage)
        #endif
    }

    #if os(iOS)
    private var outputFolderChooseLabel: String {
        ReceivePresentationText.folderAction(
            isUnavailable: hasUnavailableCustomOutputDir,
            language: uiLanguage
        )
    }

    private var outputFolderHelperText: String {
        ReceivePresentationText.folderHelper(
            isUnavailable: hasUnavailableCustomOutputDir,
            language: uiLanguage
        )
    }
    #endif

    private var primaryLabel: String {
        ReceivePresentationText.primaryAction(
            isAcceptingOffer: isAcceptingRoomOffer,
            isDeliveringInvitation: nearbyInviteDelivery.isDelivering,
            canStartAnother: canStartAnotherReceive,
            isBusy: viewModel.isBusy,
            language: uiLanguage
        )
    }

    @ViewBuilder private var inviteSection: some View {
        VStack(spacing: 12) {
            Image(systemName: "checkmark.shield.fill")
                .font(.system(size: 42))
                .foregroundStyle(Theme.success)
            Text(AppText.localized("receive.invite.verified", language: uiLanguage))
                .font(.title3.weight(.semibold))
                .foregroundStyle(Theme.text)
            Text(AppText.localized("receive.invite.role", language: uiLanguage))
            .font(.body)
            .foregroundStyle(Theme.muted)
            .multilineTextAlignment(.center)
        }
        .card(raised: true, padding: 18)
    }

    @ViewBuilder private var roomSection: some View {
        #if os(iOS)
        mobileRoomSection
        #else
        desktopRoomSection
        #endif
    }

    #if os(iOS)
    private var mobileRoomSection: some View {
        VStack(alignment: .leading, spacing: 14) {
            PairingPanelSelector(selection: $pairingPanel, disabled: viewModel.isBusy)
            Text(TransferPairingText.guidance(direction: .receive, language: uiLanguage))
            .font(.footnote)
            .foregroundStyle(Theme.muted)
            .fixedSize(horizontal: false, vertical: true)
            .accessibilityIdentifier("receive_pairing_guidance")

            Group {
                if pairingPanel == .scan {
                    VStack(spacing: 14) {
                        Image(systemName: "qrcode.viewfinder")
                            .font(.system(size: 48, weight: .medium))
                            .foregroundStyle(Theme.accentStrong)
                        Text(TransferPairingText.scanPrompt(
                            direction: .receive,
                            language: uiLanguage
                        ))
                            .font(.headline.weight(.semibold))
                            .foregroundStyle(Theme.text)
                        Button {
                            isQRScannerPresented = true
                        } label: {
                            Label(
                                AppText.localized(
                                    "transfer.pairing.open_scanner",
                                    language: uiLanguage
                                ),
                                systemImage: "camera"
                            )
                                .frame(maxWidth: .infinity, minHeight: 48)
                        }
                        .buttonStyle(.borderedProminent)
                        .tint(Theme.accent)
                        .accessibilityIdentifier("receive_scan_sender_qr")
                    }
                    .frame(maxWidth: .infinity, minHeight: 230)
                } else if !joiningInvite.trimmed.isEmpty {
                    VStack(spacing: 12) {
                        Image(systemName: "checkmark.circle.fill")
                            .font(.system(size: 42))
                            .foregroundStyle(Theme.success)
                        Text(AppText.localized(
                            "transfer.pairing.link_ready",
                            language: uiLanguage
                        ))
                            .font(.headline.weight(.semibold))
                            .foregroundStyle(Theme.text)
                        Button(AppText.localized(
                            "transfer.pairing.clear_show_qr",
                            language: uiLanguage
                        )) {
                            joiningInvite = ""
                        }
                        .buttonStyle(.bordered)
                    }
                    .frame(maxWidth: .infinity, minHeight: 230)
                } else {
                    VStack(spacing: 12) {
                        if let image = roomQRCodeImage {
                            QRCard(image: image, size: 184)
                                .accessibilityLabel(TransferPairingText.qrAccessibility(
                                    direction: .receive,
                                    language: uiLanguage
                                ))
                        } else {
                            qrPlaceholder
                        }
                        Button {
                            copyWithToast(
                                pairingInvite?.payload ?? "",
                                AppText.localized(
                                    "transfer.pairing.link_copied",
                                    language: uiLanguage
                                ),
                                language: uiLanguage
                            )
                        } label: {
                            Label(
                                AppText.localized(
                                    "transfer.pairing.copy_link",
                                    language: uiLanguage
                                ),
                                systemImage: "doc.on.doc"
                            )
                            .frame(maxWidth: .infinity, minHeight: 40)
                        }
                        .buttonStyle(.bordered)
                        .disabled(pairingInvite == nil)
                        .accessibilityIdentifier("receive_invite_copy")
                    }
                    .frame(maxWidth: .infinity, minHeight: 230)
                }
            }

            RoomCodeField(
                code: joiningInviteBinding,
                disabled: viewModel.isBusy,
                title: AppText.localized(
                    "transfer.pairing.enter_complete_link",
                    language: uiLanguage
                ),
                placeholder: "envoix://invite/v2/…",
                pasteAction: pastePairingInput,
                helper: "",
                accessibilityIdentifier: "receive_invite_input"
            )
        }
        .card(raised: true, padding: 18)
    }
    #endif

    private var desktopRoomSection: some View {
        VStack(alignment: .center, spacing: 16) {
            VStack(spacing: 4) {
                Text(AppText.localized(
                    "transfer.pairing.share_title",
                    language: uiLanguage
                ))
                    .font(.title2.weight(.semibold))
                    .foregroundStyle(Theme.text)
                Text(TransferPairingText.desktopDetail(
                    direction: .receive,
                    language: uiLanguage
                ))
                    .font(.body)
                    .foregroundStyle(Theme.muted)
                    .multilineTextAlignment(.center)
            }

            if let image = roomQRCodeImage {
                QRCard(image: image, size: 208)
                    .accessibilityLabel(TransferPairingText.qrAccessibility(
                        direction: .receive,
                        language: uiLanguage
                    ))
            } else {
                qrPlaceholder
            }

            HStack(spacing: 8) {
                Button {
                    copyWithToast(
                        pairingInvite?.payload ?? "",
                        AppText.localized(
                            "transfer.pairing.link_copied",
                            language: uiLanguage
                        ),
                        language: uiLanguage
                    )
                } label: {
                    Label(
                        AppText.localized("transfer.pairing.copy_link", language: uiLanguage),
                        systemImage: "doc.on.doc"
                    )
                        .frame(minHeight: 34)
                        .contentShape(Rectangle())
                }
                .disabled(pairingInvite == nil)

                Button {
                    joiningInvite = ""
                    refreshPairingInvite()
                } label: {
                    Label(
                        AppText.localized("common.new", language: uiLanguage),
                        systemImage: "arrow.clockwise"
                    )
                        .frame(minHeight: 34)
                        .contentShape(Rectangle())
                }
                .disabled(viewModel.isBusy)
            }
            .buttonStyle(.bordered)

            RoomCodeField(
                code: joiningInviteBinding,
                disabled: viewModel.isBusy,
                title: TransferPairingText.joinOtherTitle(
                    direction: .receive,
                    language: uiLanguage
                ),
                placeholder: "envoix://invite/v2/…",
                helper: "",
                accessibilityIdentifier: "receive_invite_input"
            )

            HStack(spacing: 8) {
                Button {
                    pastePairingInput()
                } label: {
                    Label(
                        AppText.localized("common.paste", language: uiLanguage),
                        systemImage: "doc.on.clipboard"
                    )
                        .frame(maxWidth: .infinity, minHeight: 44)
                        .contentShape(Rectangle())
                }
                .disabled(viewModel.isBusy)

                #if os(iOS)
                Button {
                    isQRScannerPresented = true
                } label: {
                    Label(
                        TransferPairingText.scanAction(
                            direction: .receive,
                            language: uiLanguage
                        ),
                        systemImage: "qrcode.viewfinder"
                    )
                        .frame(maxWidth: .infinity, minHeight: 44)
                        .contentShape(Rectangle())
                }
                .disabled(viewModel.isBusy)
                #endif
            }
            .buttonStyle(.bordered)
            .controlSize(.regular)
        }
        .card(raised: true, padding: 18)
    }

    private var joiningInviteBinding: Binding<String> {
        Binding(
            get: { joiningInvite },
            set: { value in joiningInvite = value }
        )
    }

    private var qrPlaceholder: some View {
        VStack(spacing: 10) {
            Image(systemName: "qrcode")
                .font(.system(size: 72, weight: .medium))
                .foregroundStyle(Theme.muted)
            Text(AppText.localized("transfer.pairing.qr_placeholder", language: uiLanguage))
                .font(.title3.weight(.semibold))
                .foregroundStyle(Theme.muted)
        }
        .frame(width: 236, height: 236)
        .background(Theme.surface)
        .overlay(
            RoundedRectangle(cornerRadius: Theme.cardRadius)
                .strokeBorder(Theme.line.opacity(0.75), lineWidth: 0.8)
        )
        .clipShape(RoundedRectangle(cornerRadius: Theme.cardRadius))
    }


    /// The raw network address carries the real IP, so it stays hidden until the
    /// user explicitly reveals it.
    @ViewBuilder private var addressReveal: some View {
        VStack(alignment: .leading, spacing: 8) {
            Button {
                revealAddress.toggle()
            } label: {
                Label(
                    ReceivePresentationText.addressAction(
                        isRevealed: revealAddress,
                        language: uiLanguage
                    ),
                    systemImage: revealAddress ? "eye.slash" : "eye"
                )
                    .contentShape(Rectangle())
            }
            .controlSize(.small)

            if revealAddress {
                Text(viewModel.peerAddress)
                    .font(.body.monospaced())
                    .foregroundStyle(Theme.muted)
                    .textSelection(.enabled)
                    .lineLimit(1)
                    .truncationMode(.middle)
            }
        }
        .card(raised: true, padding: 14)
    }

    private var canStart: Bool {
        if roomControlTransfer {
            return mode == .invite && !joiningInvite.isEmpty
        }
        if rememberAfterPairing,
           (mode == .room || mode == .invite),
           rememberLabel.trimmed.isEmpty {
            return false
        }
        switch mode {
        case .room:
            return !joiningInvite.trimmed.isEmpty || pairingInvite != nil
        case .invite:
            return !joiningInvite.isEmpty
        case .token:
            return token.trimmed.count >= minTokenLength
        case .remembered:
            #if os(macOS)
            return selectedRememberedPeer != nil
            #else
            return false
            #endif
        }
    }

    private var concurrencyBlocked: Bool {
        !concurrentTransfers
            && !viewModel.isBusy
            && (model.send.isBusy || model.hasExecutingActivity)
    }

    private var canStartAnotherReceive: Bool {
        concurrentTransfers && viewModel.isBusy && mode == .room && activeReceiveCount < 2
    }

    private var activeReceiveCount: Int {
        model.activities.filter { record in
            guard record.direction == .receive else { return false }
            return ActivityExecutionPolicy.occupiesExecutionSlot(record.state)
        }.count
    }

    private func refreshPairingInviteIfNeeded() {
        guard !roomControlTransfer, mode == .room, pairingInvite == nil else { return }
        refreshPairingInvite()
    }

    private func refreshPairingInviteForSettingsChange() {
        guard !roomControlTransfer, mode == .room, !viewModel.isBusy else { return }
        refreshPairingInvite()
    }

    private func refreshPairingInvite() {
        do {
            let invite = try makePairingInvite(role: .receive, broker: serverURL, relay: relayURL)
            pairingInvite = invite
            joiningInvite = ""
            updateRoomQRCode(for: invite.payload)
        } catch {
            viewModel.handleFailed(error.localizedDescription)
        }
    }

    private func activeReceivePairingInvite() throws -> FfiPairingInvite {
        if let invite = pairingInvite {
            let code = invite.roomCode.trimmed
            if !code.isEmpty {
                updateRoomQRCode(for: invite.payload)
                return invite
            }
        }
        let invite = try makePairingInvite(role: .receive, broker: serverURL, relay: relayURL)
        pairingInvite = invite
        updateRoomQRCode(for: invite.payload)
        return invite
    }

    private func handleScannedInvite(_ value: String) -> String? {
        applyPairingInput(value, source: .scan)
    }

    private enum PairingInputSource {
        case paste
        case scan
    }

    private func pastePairingInput() {
        guard let value = pasteboardString()?.trimmed, !value.isEmpty else {
            ToastCenter.shared.show(AppText.localized(
                "transfer.pairing.clipboard_empty",
                language: uiLanguage
            ))
            return
        }
        _ = applyPairingInput(value, source: .paste)
    }

    @discardableResult
    private func applyPairingInput(_ value: String, source: PairingInputSource) -> String? {
        let input = value.trimmed
        do {
            guard input.hasPrefix(inviteV2URLPrefix) else {
                throw RuntimeSettingsError("Enter a complete InviteV2 link.")
            }
            _ = try parsePairingInviteForRole(input: input, localRole: .receive)
            joiningInvite = input
            mode = .invite
            let message = TransferPairingText.inputAccepted(
                scanned: source == .scan,
                language: uiLanguage
            )
            ToastCenter.shared.show(message)
            return nil
        } catch {
            let message = if error is RuntimeSettingsError {
                error.localizedDescription
            } else {
                AppText.localized("transfer.pairing.invalid_link", language: uiLanguage)
            }
            ToastCenter.shared.show(message)
            return message
        }
    }

    private func applyInitialPairingInputIfNeeded() {
        guard !didApplyInitialPairingInput,
              let initialPairingInput,
              !initialPairingInput.trimmed.isEmpty else { return }
        didApplyInitialPairingInput = true
        _ = applyPairingInput(initialPairingInput, source: .scan)
        onInitialPairingInputConsumed?()
        if roomControlTransfer {
            DispatchQueue.main.async {
                autoStartRoomControlReceiveIfReady()
            }
        }
    }

    private func autoStartRoomControlReceiveIfReady() {
        guard roomControlTransfer,
              !didAutoStartRoomControl,
              mode == .invite,
              !joiningInvite.isEmpty,
              !viewModel.isBusy else { return }
        didAutoStartRoomControl = true
        primaryAction()
    }

    private func updateRoomQRCode(for payload: String) {
        guard roomQRCodePayload != payload else { return }
        roomQRCodePayload = payload
        roomQRCodeImage = payload.isEmpty ? nil : QRCode.image(from: payload)
    }

    private func primaryAction() {
        guard (!viewModel.isBusy || canStartAnotherReceive),
              !viewModel.isFinalizing,
              !nearbyInviteDelivery.isDelivering else { return }
        #if os(iOS)
        guard outputDir != nil else {
            shouldStartAfterFolderPick = true
            isFolderPickerPresented = true
            return
        }
        #elseif os(macOS)
        guard ensureMacOutputDirectoryAuthorization() else { return }
        #endif
        if roomControlAccept != nil {
            startRoomControlReceive()
        } else {
            startReceive()
        }
    }

    private func startRoomControlReceive() {
        #if os(iOS)
        guard let roomControlAccept else { return }
        do {
            let prepared = try prepareOutputDir()
            isAcceptingRoomOffer = true
            let parsed: FfiPairingInvite
            switch mode {
            case .invite:
                parsed = try parsePairingInviteForRole(
                    input: joiningInvite,
                    localRole: .receive
                )
            case .room, .remembered, .token:
                throw RuntimeSettingsError(AppText.value(
                    "This room offer needs a new InviteV2 invitation.",
                    "此房间邀请需要新的 InviteV2 邀请。",
                    language: uiLanguage
                ))
            }
            let settings = try runtimeSettings(for: parsed)
            Task { @MainActor in
                let result = await RoomOfferAcceptanceCoordinator.startReceiverThenAccept(
                    startReceiver: {
                        viewModel.startReceivingWithInvite(
                            outputDir: prepared.url.path,
                            invite: joiningInvite,
                            settings: settings,
                            destinationAccess: prepared.access,
                            nearbyWifiAwareDeviceID: nearbySelection?.nearbyWifiAwareDeviceID
                        )
                        guard let activity = viewModel.transferActivity,
                              activity.state != .failed else {
                            return nil
                        }
                        return activity.activityId
                    },
                    acceptOffer: roomControlAccept,
                    cancelReceiver: { activityID in
                        if viewModel.transferActivity?.activityId == activityID {
                            _ = viewModel.cancel()
                        }
                    }
                )
                isAcceptingRoomOffer = false
                if case .offerUnavailable = result {
                    ToastCenter.shared.show(AppText.value(
                        "The file offer is no longer available.",
                        "此文件邀请已不可用。",
                        language: uiLanguage
                    ))
                }
            }
        } catch {
            isAcceptingRoomOffer = false
            viewModel.handleFailed(error.localizedDescription)
        }
        #else
        viewModel.handleFailed("Room control receive is unavailable on this platform.")
        #endif
    }

    /// Starts (or restarts, for "Regenerate") the receive session.
    private func startReceive() {
        switch mode {
        case .room:
            startReceiveWithRoom()
        case .invite:
            startReceiveWithInvite()
        case .token:
            startReceiveWithToken()
        case .remembered:
            #if os(macOS)
            startReceiveWithRememberedPeer()
            #else
            return
            #endif
        }
    }

    private func startReceiveWithToken() {
        do {
            let prepared = try prepareOutputDir()
            let settings = try RuntimeSettingsProvider.make(
                concurrentTransfers: concurrentTransfers,
                language: language,
                serverURL: serverURL,
                relayURL: relayURL,
                candidatesAllow: candidatesAllow,
                candidatesDeny: candidatesDeny,
                speedLimit: speedLimit
            )
            viewModel.startReceivingWithToken(
                outputDir: prepared.url.path,
                token: token.trimmed,
                settings: settings,
                destinationAccess: prepared.access
            )
        } catch {
            viewModel.handleFailed(error.localizedDescription)
        }
    }

    private func startReceiveWithRoom() {
        if !joiningInvite.trimmed.isEmpty {
            startReceiveWithInvite()
            return
        }
        do {
            let prepared = try prepareOutputDir()
            let settings = try RuntimeSettingsProvider.make(
                concurrentTransfers: concurrentTransfers,
                language: language,
                serverURL: serverURL,
                relayURL: relayURL,
                candidatesAllow: candidatesAllow,
                candidatesDeny: candidatesDeny,
                speedLimit: speedLimit
            )
            let start: (String) -> Void = { code in
                viewModel.startReceivingWithRoom(
                    outputDir: prepared.url.path,
                    code: code,
                    settings: settings,
                    destinationAccess: prepared.access,
                    nearbyWifiAwareDeviceID: nearbySelection?.nearbyWifiAwareDeviceID,
                    rememberLabel: rememberAfterPairing ? rememberLabel : nil
                )
            }

            let pairingInvite = try activeReceivePairingInvite()
            nearbyInviteDelivery.deliver(
                invite: pairingInvite.payload,
                using: nearbyInviteOffer
            ) {
                start(pairingInvite.roomCode)
            }
        } catch {
            viewModel.handleFailed(error.localizedDescription)
        }
    }

    private func cancelNearbyInviteDelivery() {
        nearbyInviteDelivery.cancel()
    }

    private func startReceiveWithInvite() {
        do {
            guard joiningInvite.trimmed.hasPrefix(inviteV2URLPrefix) else {
                throw RuntimeSettingsError("Enter a complete InviteV2 link.")
            }
            let parsed = try parsePairingInviteForRole(
                input: joiningInvite,
                localRole: .receive
            )
            let prepared = try prepareOutputDir()
            let settings = try runtimeSettings(for: parsed)
            viewModel.startReceivingWithInvite(
                outputDir: prepared.url.path,
                invite: joiningInvite,
                settings: settings,
                destinationAccess: prepared.access,
                nearbyWifiAwareDeviceID: nearbySelection?.nearbyWifiAwareDeviceID,
                rememberLabel: rememberAfterPairing ? rememberLabel : nil
            )
        } catch {
            viewModel.handleFailed(error.localizedDescription)
        }
    }

    private func runtimeSettings(for parsed: FfiPairingInvite) throws -> EnvoixRuntimeSettings {
        return try RuntimeSettingsProvider.make(
            transferInvitation: parsed,
            concurrentTransfers: concurrentTransfers,
            language: language,
            candidatesAllow: candidatesAllow,
            candidatesDeny: candidatesDeny,
            speedLimit: speedLimit
        )
    }

    #if os(macOS)
    private func startReceiveWithRememberedPeer() {
        guard let peer = selectedRememberedPeer else { return }
        do {
            let prepared = try prepareOutputDir()
            let settings = try RuntimeSettingsProvider.make(
                concurrentTransfers: concurrentTransfers,
                language: language,
                serverURL: peer.broker,
                relayURL: peer.relay,
                candidatesAllow: candidatesAllow,
                candidatesDeny: candidatesDeny,
                speedLimit: speedLimit
            )
            viewModel.startReceivingFromRememberedPeer(
                outputDir: prepared.url.path,
                peer: peer,
                settings: settings,
                destinationAccess: prepared.access
            )
        } catch {
            viewModel.handleFailed(error.localizedDescription)
        }
    }

    private func refreshRememberedPeers() {
        rememberedPeers = (try? RememberedPeerStore.shared.peers()) ?? []
        if let selectedRememberedPeer,
           !rememberedPeers.contains(where: { $0.id == selectedRememberedPeer.id }) {
            self.selectedRememberedPeer = nil
            if mode == .remembered { mode = .room }
        }
    }
    #endif

    #if os(macOS)
    /// A raw ~/Downloads path is not durable authorization. Require a valid
    /// user-selected bookmark and prove write access before advertising or
    /// joining, so a TCC denial cannot interrupt an active transfer.
    private func ensureMacOutputDirectoryAuthorization() -> Bool {
        guard let bookmark = UserDefaults.standard.data(forKey: outputDirBookmarkKey),
              let url = try? resolveSecurityScopedFolderBookmark(bookmark) else {
            selectMacOutputFolder(startAfterSelection: true)
            return false
        }

        let access = SecurityScopedResourceAccess(url: url)
        guard access.isActive || FileManager.default.isWritableFile(atPath: url.path) else {
            selectMacOutputFolder(startAfterSelection: true)
            return false
        }
        do {
            try validateWritableDirectoryAccess(url)
            model.retainDestinationAccessForAppLifetime(access)
            return true
        } catch {
            selectMacOutputFolder(startAfterSelection: true)
            return false
        }
    }

    private func selectMacOutputFolder(startAfterSelection: Bool) {
        guard let url = chooseURL(directory: true) else { return }
        do {
            let access = SecurityScopedResourceAccess(url: url)
            guard access.isActive || FileManager.default.isWritableFile(atPath: url.path) else {
                throw RuntimeSettingsError(AppText.value(
                    "macOS did not grant access to the selected folder. Choose it again and confirm the system prompt.",
                    "macOS 未授予所选文件夹访问权限。请重新选择并确认系统授权提示。",
                    language: uiLanguage
                ))
            }
            try validateWritableDirectoryAccess(url)
            let bookmark = try makeSecurityScopedFolderBookmark(for: url)
            UserDefaults.standard.set(bookmark, forKey: outputDirBookmarkKey)
            outputDirPath = url.path
            outputDirDisplayName = url.lastPathComponent.isEmpty ? url.path : url.lastPathComponent
            model.retainDestinationAccessForAppLifetime(access)
            ToastCenter.shared.show(AppText.value(
                "Save folder authorized",
                "保存文件夹已授权",
                language: uiLanguage
            ))
            if startAfterSelection {
                DispatchQueue.main.async {
                    primaryAction()
                }
            }
        } catch {
            viewModel.handleFailed(error.localizedDescription)
        }
    }
    #endif

    private func prepareOutputDir() throws -> (url: URL, access: AnyObject?) {
        guard let url = outputDir else {
            #if os(iOS)
            if hasCustomOutputDir {
                throw RuntimeSettingsError(AppText.value(
                    "The selected Files folder is unavailable. Choose it again or reset to the default save folder.",
                    "已选择的 Files 文件夹不可用。请重新选择，或重置为默认保存位置。",
                    language: uiLanguage
                ))
            }
            #endif
            throw RuntimeSettingsError(AppText.value("Choose a save folder first.", "请先选择保存文件夹。", language: uiLanguage))
        }
        #if os(iOS)
        let access: AnyObject?
        if hasCustomOutputDir {
            let scopedAccess = SecurityScopedResourceAccess(url: url)
            guard scopedAccess.isActive || FileManager.default.isWritableFile(atPath: url.path) else {
                throw RuntimeSettingsError(AppText.value(
                    "Envoix cannot write to the selected Files folder. Choose it again or reset to the default save folder.",
                    "Envoix 无法写入已选择的 Files 文件夹。请重新选择，或重置为默认保存位置。",
                    language: uiLanguage
                ))
            }
            access = scopedAccess
        } else {
            access = nil
        }
        #else
        guard UserDefaults.standard.data(forKey: outputDirBookmarkKey) != nil else {
            throw RuntimeSettingsError(AppText.value(
                "Choose the save folder once to grant macOS access before receiving.",
                "接收前请先选择一次保存文件夹，以授予 macOS 访问权限。",
                language: uiLanguage
            ))
        }
        let scopedAccess = SecurityScopedResourceAccess(url: url)
        guard scopedAccess.isActive || FileManager.default.isWritableFile(atPath: url.path) else {
            throw RuntimeSettingsError(AppText.value(
                "The save-folder permission is unavailable. Choose the folder again.",
                "保存文件夹权限不可用。请重新选择该文件夹。",
                language: uiLanguage
            ))
        }
        let access: AnyObject? = scopedAccess
        #endif
        do {
            try validateWritableDirectoryAccess(url)
        } catch {
            throw RuntimeSettingsError(AppText.value(
                "Envoix cannot write to the selected save folder. Choose it again or check its permissions.",
                "Envoix 无法写入所选保存文件夹。请重新选择或检查文件夹权限。",
                language: uiLanguage
            ))
        }
        #if os(iOS)
        return (url, access)
        #else
        model.retainDestinationAccessForAppLifetime(scopedAccess)
        return (url, access)
        #endif
    }

    #if os(iOS)
    private func handlePickedOutputFolder(_ url: URL) {
        do {
            let bookmark = try makeSecurityScopedFolderBookmark(for: url)
            UserDefaults.standard.set(bookmark, forKey: outputDirBookmarkKey)
            outputDirDisplayName = url.lastPathComponent.isEmpty ? url.path : url.lastPathComponent
            isFolderPickerPresented = false
            ToastCenter.shared.show(AppText.value("Save folder selected", "已选择保存文件夹", language: uiLanguage))
            if shouldStartAfterFolderPick {
                shouldStartAfterFolderPick = false
                DispatchQueue.main.async {
                    primaryAction()
                }
            }
        } catch {
            shouldStartAfterFolderPick = false
            isFolderPickerPresented = false
            viewModel.handleFailed(error.localizedDescription)
        }
    }

    private func resetOutputFolder() {
        UserDefaults.standard.removeObject(forKey: outputDirBookmarkKey)
        outputDirDisplayName = ""
        ToastCenter.shared.show(AppText.value("Default save folder restored", "已恢复默认保存位置", language: uiLanguage))
    }
    #endif
}
