import SwiftUI
import EnvoixCore

struct ReceiveView: View {
    @Environment(\.appLanguage) private var uiLanguage
    @EnvironmentObject private var model: AppModel
    @ObservedObject var viewModel: TransferViewModel
    // Remembered across launches. Empty means "use the platform default".
    @AppStorage("envoix.outputDir") private var outputDirPath: String = ""
    @AppStorage("envoix.outputDirDisplayName") private var outputDirDisplayName: String = ""
    @AppStorage("envoix.token") private var token: String = ""
    @AppStorage("envoix.concurrentTransfers") private var concurrentTransfers = true
    @AppStorage("envoix.language") private var language = "en"
    @AppStorage("envoix.serverURL") private var serverURL = ""
    @AppStorage("envoix.relayURL") private var relayURL = ""
    @AppStorage("envoix.candidatesAllow") private var candidatesAllow = ""
    @AppStorage("envoix.candidatesDeny") private var candidatesDeny = ""
    @AppStorage("envoix.speedLimit") private var speedLimit = 40
    @AppStorage("envoix.destinationSaveMode") private var destinationSaveMode = "direct"
    @State private var mode: PairingMode = .room
    @State private var roomCode = newRoomCode()
    @State private var joinRoomCode = ""
    @State private var joiningInvite = ""
    @State private var pairingInvite: FfiPairingInvite?
    @State private var roomQRCodeImage: PlatformImage?
    @State private var roomQRCodePayload = ""
    @State private var pairingPanel: PairingPanelMode = .show
    @State private var revealAddress = false
    @State private var didApplyInitialPairingInput = false
    private let initialPairingInput: String?
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
        onInitialPairingInputConsumed: (() -> Void)? = nil,
        onSwitchToSend: ((String) -> Void)? = nil
    ) {
        self.viewModel = viewModel
        self.initialPairingInput = initialPairingInput
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
        #else
        VStack(spacing: 0) {
            scrollContent
            footerMessage
            primaryButton
                .padding(.top, 12)
        }
        .onAppear(perform: applyInitialPairingInputIfNeeded)
        #endif
    }

    private var scrollContent: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 16) {
                outputSection
                connectionSection
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
                joinRoomCode = ""
                roomCode = ""
                pairingInvite = nil
                roomQRCodeImage = nil
                roomQRCodePayload = ""
            }
        }
    }

    @ViewBuilder private var connectionSection: some View {
        if mode == .invite {
            inviteSection
        } else if mode == .room {
            roomSection
        } else {
            TokenField(token: $token, disabled: viewModel.isBusy)
                .card(padding: 14)
        }
    }

    @ViewBuilder private var footerMessage: some View {
        if concurrencyBlocked {
            Text(AppText.value("Finish sending before starting a receive.", "请先完成发送任务，再开始接收。", language: uiLanguage))
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
            Text(AppText.value("Save to", "保存到", language: uiLanguage))
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
                        Label(AppText.value("Reset", "重置", language: uiLanguage), systemImage: "arrow.uturn.backward")
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
                    Label(AppText.value("Select", "选择", language: uiLanguage), systemImage: "folder")
                        .frame(minHeight: 34)
                        .contentShape(Rectangle())
                }
                .disabled(viewModel.isBusy)
            }
            #endif

            Divider().overlay(Theme.line.opacity(0.5))
            Text(AppText.value("Save method", "保存方式", language: uiLanguage))
                .font(.body.weight(.semibold))
                .foregroundStyle(Theme.text)
            Picker("Save method", selection: $destinationSaveMode) {
                Text(AppText.value("Save directly", "直接保存", language: uiLanguage)).tag("direct")
                Text(AppText.value("Verify, then copy", "校验后复制", language: uiLanguage)).tag("copy")
            }
            .pickerStyle(.segmented)
            .labelsHidden()
            .disabled(viewModel.isBusy)
            Text(destinationSaveMode == "copy"
                 ? AppText.value(
                    "Uses additional temporary space and saving time for destinations that cannot safely finalize the same object.",
                    "适用于无法安全原地完成保存的目标；会额外占用临时空间和保存时间。",
                    language: uiLanguage
                 )
                 : AppText.value(
                    "Writes once on the selected storage and reveals the verified object when ready.",
                    "在所选存储上只写入一次，校验完成后直接显示文件。",
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
                return AppText.value("Selected Files folder unavailable", "已选 Files 文件夹不可用", language: uiLanguage)
            }
            return outputDir.lastPathComponent.isEmpty ? outputDir.path : outputDir.lastPathComponent
        }
        return AppText.value("On My iPhone / Envoix / Downloads", "我的 iPhone / Envoix / Downloads", language: uiLanguage)
        #else
        if UserDefaults.standard.data(forKey: outputDirBookmarkKey) != nil,
           outputDir == nil {
            return AppText.value(
                "Selected folder unavailable — choose again",
                "已选文件夹不可用——请重新选择",
                language: uiLanguage
            )
        }
        return outputDir?.path
            ?? AppText.value("Choose a save folder", "请选择保存文件夹", language: uiLanguage)
        #endif
    }

    #if os(iOS)
    private var outputFolderChooseLabel: String {
        hasUnavailableCustomOutputDir
            ? AppText.value("Choose Again", "重新选择", language: uiLanguage)
            : AppText.value("Choose", "选择", language: uiLanguage)
    }

    private var outputFolderHelperText: String {
        if hasUnavailableCustomOutputDir {
            return AppText.value(
                "The selected Files folder permission expired. Choose it again or reset to the default folder.",
                "已选择的 Files 文件夹权限已失效。请重新选择，或重置为默认文件夹。",
                language: uiLanguage
            )
        }
        return AppText.value(
            "Default saves to Files > On My iPhone > Envoix > Downloads. Choose a Files folder to save elsewhere.",
            "默认保存到 Files > On My iPhone > Envoix > Downloads。也可以选择其他 Files 文件夹。",
            language: uiLanguage
        )
    }
    #endif

    private var primaryLabel: String {
        if canStartAnotherReceive {
            return AppText.value("Start Another Receive", "再开启一个接收", language: uiLanguage)
        }
        if viewModel.isBusy {
            return AppText.value("Managed in Activity", "请在活动中管理", language: uiLanguage)
        }
        switch mode {
        case .invite:
            return AppText.value("Start Receiving", "开始接收", language: uiLanguage)
        default:
            return AppText.value("Start Receiving", "开始接收", language: uiLanguage)
        }
    }

    @ViewBuilder private var inviteSection: some View {
        VStack(spacing: 12) {
            Image(systemName: "checkmark.shield.fill")
                .font(.system(size: 42))
                .foregroundStyle(Theme.success)
            Text(AppText.value("InviteV2 verified", "InviteV2 已验证", language: uiLanguage))
                .font(.title3.weight(.semibold))
                .foregroundStyle(Theme.text)
            Text(AppText.value(
                "This invitation assigns this device the Receive role.",
                "此邀请已将本设备指定为接收端。",
                language: uiLanguage
            ))
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
            Text(AppText.value(
                "Show your receive QR, or scan the other device's send QR.",
                "可以显示本机接收码，也可以扫描另一台设备的发送码。",
                language: uiLanguage
            ))
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
                        Text(AppText.value("Scan a send QR", "扫描发送码", language: uiLanguage))
                            .font(.headline.weight(.semibold))
                            .foregroundStyle(Theme.text)
                        Button {
                            isQRScannerPresented = true
                        } label: {
                            Label(AppText.value("Open scanner", "打开扫描器", language: uiLanguage), systemImage: "camera")
                                .frame(maxWidth: .infinity, minHeight: 48)
                        }
                        .buttonStyle(.borderedProminent)
                        .tint(Theme.accent)
                        .accessibilityIdentifier("receive_scan_sender_qr")
                    }
                    .frame(maxWidth: .infinity, minHeight: 230)
                } else if !joinRoomCode.trimmed.isEmpty {
                    VStack(spacing: 12) {
                        Image(systemName: "checkmark.circle.fill")
                            .font(.system(size: 42))
                            .foregroundStyle(Theme.success)
                        Text(AppText.value("Ready to join", "已准备加入", language: uiLanguage))
                            .font(.headline.weight(.semibold))
                            .foregroundStyle(Theme.text)
                        Text(joinRoomCode)
                            .font(.body.monospaced().weight(.semibold))
                            .foregroundStyle(Theme.accentStrong)
                            .multilineTextAlignment(.center)
                            .textSelection(.enabled)
                        Button(AppText.value("Clear and show my QR", "清除并显示我的二维码", language: uiLanguage)) {
                            joinRoomCode = ""
                        }
                        .buttonStyle(.bordered)
                    }
                    .frame(maxWidth: .infinity, minHeight: 230)
                } else {
                    VStack(spacing: 12) {
                        if let image = roomQRCodeImage {
                            QRCard(image: image, size: 184)
                                .accessibilityLabel(AppText.value("Receive QR code", "接收二维码", language: uiLanguage))
                        } else {
                            qrPlaceholder
                        }
                        LinkRow(
                            text: roomCode.trimmed.isEmpty ? AppText.value("Receive code", "接收码", language: uiLanguage) : roomCode,
                            textIdentifier: "receive_room_code",
                            displaysFullText: true
                        ) {
                            Button {
                                copyWithToast(roomCode, AppText.value("Room code copied", "接收码已复制", language: uiLanguage), language: uiLanguage)
                            } label: {
                                Label(AppText.value("Copy", "复制", language: uiLanguage), systemImage: "doc.on.doc")
                                    .frame(minHeight: 40)
                            }
                            .disabled(roomCode.trimmed.isEmpty)
                            .accessibilityIdentifier("receive_room_copy")
                        }
                    }
                    .frame(maxWidth: .infinity, minHeight: 230)
                }
            }

            RoomCodeField(
                code: joinRoomCodeBinding,
                disabled: viewModel.isBusy,
                title: AppText.value("Or enter a Room code", "或输入配对码", language: uiLanguage),
                placeholder: AppText.value("Enter Room code", "输入配对码", language: uiLanguage),
                showsCopyAction: false,
                pasteAction: pastePairingInput,
                helper: "",
                accessibilityIdentifier: "receive_join_room_code_input"
            )
        }
        .card(raised: true, padding: 18)
    }
    #endif

    private var desktopRoomSection: some View {
        VStack(alignment: .center, spacing: 16) {
            VStack(spacing: 4) {
                Text(AppText.value("Share this QR or code", "分享二维码或接收码", language: uiLanguage))
                    .font(.title2.weight(.semibold))
                    .foregroundStyle(Theme.text)
                Text(AppText.value("The sender can scan the QR or enter the same Room code.", "发送方可以扫码，或输入同一个配对码。", language: uiLanguage))
                    .font(.body)
                    .foregroundStyle(Theme.muted)
                    .multilineTextAlignment(.center)
            }

            if let image = roomQRCodeImage {
                QRCard(image: image, size: 208)
                    .accessibilityLabel(AppText.value("Receive QR code", "接收二维码", language: uiLanguage))
            } else {
                qrPlaceholder
            }

            LinkRow(
                text: roomCode.trimmed.isEmpty ? AppText.value("Receive code", "接收码", language: uiLanguage) : roomCode,
                textIdentifier: "receive_room_code",
                displaysFullText: true
            ) {
                Button {
                    copyWithToast(roomCode, AppText.value("Room code copied", "接收码已复制", language: uiLanguage), language: uiLanguage)
                } label: {
                    Label(AppText.value("Copy", "复制", language: uiLanguage), systemImage: "doc.on.doc")
                        .frame(minHeight: 34)
                        .contentShape(Rectangle())
                }
                .disabled(roomCode.trimmed.isEmpty)

                Button {
                    refreshPairingInvite()
                } label: {
                    Label(AppText.value("New", "新建", language: uiLanguage), systemImage: "arrow.clockwise")
                        .frame(minHeight: 34)
                        .contentShape(Rectangle())
                }
                .disabled(viewModel.isBusy)
            }

            RoomCodeField(
                code: joinRoomCodeBinding,
                disabled: viewModel.isBusy,
                title: AppText.value("Join sender instead", "改为加入发送端", language: uiLanguage),
                placeholder: AppText.value("Scan QR or enter sender Room code", "扫码或输入发送端配对码", language: uiLanguage),
                helper: "",
                accessibilityIdentifier: "receive_join_room_code_input"
            )

            HStack(spacing: 8) {
                Button {
                    pastePairingInput()
                } label: {
                    Label(AppText.value("Paste", "粘贴", language: uiLanguage), systemImage: "doc.on.clipboard")
                        .frame(maxWidth: .infinity, minHeight: 44)
                        .contentShape(Rectangle())
                }
                .disabled(viewModel.isBusy)

                #if os(iOS)
                Button {
                    isQRScannerPresented = true
                } label: {
                    Label(AppText.value("Scan sender QR", "扫描发送端二维码", language: uiLanguage), systemImage: "qrcode.viewfinder")
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

    private var joinRoomCodeBinding: Binding<String> {
        Binding(
            get: { joinRoomCode },
            set: { value in joinRoomCode = value }
        )
    }

    private var qrPlaceholder: some View {
        VStack(spacing: 10) {
            Image(systemName: "qrcode")
                .font(.system(size: 72, weight: .medium))
                .foregroundStyle(Theme.muted)
            Text(AppText.value("QR code", "二维码", language: uiLanguage))
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
                Label(revealAddress
                      ? AppText.value("Hide address", "隐藏地址", language: uiLanguage)
                      : AppText.value("Show address", "显示地址", language: uiLanguage),
                      systemImage: revealAddress ? "eye.slash" : "eye")
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
        switch mode {
        case .room:
            return !joinRoomCode.trimmed.isEmpty || !roomCode.trimmed.isEmpty
        case .invite:
            return !joiningInvite.isEmpty
        case .token:
            return token.trimmed.count >= minTokenLength
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
        guard mode == .room, pairingInvite == nil else { return }
        refreshPairingInvite()
    }

    private func refreshPairingInviteForSettingsChange() {
        guard mode == .room, !viewModel.isBusy else { return }
        refreshPairingInvite()
    }

    private func refreshPairingInvite() {
        do {
            let invite = try makePairingInvite(role: .receive, broker: serverURL, relay: relayURL)
            pairingInvite = invite
            roomCode = invite.roomCode
            joinRoomCode = ""
            updateRoomQRCode(for: invite.payload)
        } catch {
            viewModel.handleFailed(error.localizedDescription)
        }
    }

    private func activeRoomCode() throws -> String {
        let joinedCode = joinRoomCode.trimmed
        if !joinedCode.isEmpty {
            return try roomCodeFromJoinInput(joinedCode)
        }
        if let invite = pairingInvite {
            let code = invite.roomCode.trimmed
            if !code.isEmpty {
                updateRoomQRCode(for: invite.payload)
                return code
            }
        }
        let invite = try makePairingInvite(role: .receive, broker: serverURL, relay: relayURL)
        pairingInvite = invite
        roomCode = invite.roomCode
        updateRoomQRCode(for: invite.payload)
        return invite.roomCode
    }

    private func roomCodeFromJoinInput(_ input: String) throws -> String {
        try normalizeRoomCode(input: input)
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
            ToastCenter.shared.show(AppText.value("Clipboard is empty", "剪贴板为空", language: uiLanguage))
            return
        }
        _ = applyPairingInput(value, source: .paste)
    }

    @discardableResult
    private func applyPairingInput(_ value: String, source: PairingInputSource) -> String? {
        let input = value.trimmed
        do {
            if input.lowercased().hasPrefix("envoix:") {
                let parsed = try parsePairingInviteForRole(input: input, localRole: .receive)
                applyRuntimeSettings(from: parsed)
                joiningInvite = input
                joinRoomCode = ""
                mode = .invite
            } else {
                joinRoomCode = try roomCodeFromJoinInput(input)
                joiningInvite = ""
                pairingPanel = .show
                mode = .room
            }
            let message = source == .scan
                ? AppText.value("QR scanned", "二维码已扫描", language: uiLanguage)
                : AppText.value("Invitation pasted", "邀请已粘贴", language: uiLanguage)
            ToastCenter.shared.show(message)
            return nil
        } catch {
            let message = if error is RuntimeSettingsError {
                error.localizedDescription
            } else {
                AppText.value(
                    "This is not a valid Envoix pairing code.",
                    "这不是有效的 Envoix 配对码。",
                    language: uiLanguage
                )
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
    }

    private func applyRuntimeSettings(from parsed: FfiPairingInvite) {
        if !parsed.broker.trimmed.isEmpty {
            serverURL = parsed.broker.trimmed
        }
        if let relay = parsed.relayUrls.first, !relay.trimmed.isEmpty {
            relayURL = relay.trimmed
        }
    }

    private func updateRoomQRCode(for payload: String) {
        guard roomQRCodePayload != payload else { return }
        roomQRCodePayload = payload
        roomQRCodeImage = payload.isEmpty ? nil : QRCode.image(from: payload)
    }

    private func primaryAction() {
        guard (!viewModel.isBusy || canStartAnotherReceive), !viewModel.isFinalizing else { return }
        #if os(iOS)
        guard outputDir != nil else {
            shouldStartAfterFolderPick = true
            isFolderPickerPresented = true
            return
        }
        #elseif os(macOS)
        guard ensureMacOutputDirectoryAuthorization() else { return }
        #endif
        startReceive()
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
        do {
            let code = try activeRoomCode()
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
            viewModel.startReceivingWithRoom(
                outputDir: prepared.url.path,
                code: code,
                settings: settings,
                destinationAccess: prepared.access
            )
        } catch {
            viewModel.handleFailed(error.localizedDescription)
        }
    }

    private func startReceiveWithInvite() {
        do {
            _ = try parsePairingInviteForRole(input: joiningInvite, localRole: .receive)
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
            viewModel.startReceivingWithInvite(
                outputDir: prepared.url.path,
                invite: joiningInvite,
                settings: settings,
                destinationAccess: prepared.access
            )
        } catch {
            viewModel.handleFailed(error.localizedDescription)
        }
    }

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
                    startReceive()
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
