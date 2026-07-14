import SwiftUI
#if os(macOS)
import AppKit
#endif
import UniformTypeIdentifiers
import EnvoixCore

struct SendView: View {
    @Environment(\.appLanguage) private var uiLanguage
    @EnvironmentObject private var model: AppModel
    @ObservedObject var viewModel: TransferViewModel
    @State private var file: URL?
    @AppStorage("envoix.token") private var token: String = ""
    @AppStorage("envoix.concurrentTransfers") private var concurrentTransfers = true
    @AppStorage("envoix.language") private var language = "en"
    @AppStorage("envoix.serverURL") private var serverURL = ""
    @AppStorage("envoix.relayURL") private var relayURL = ""
    @AppStorage("envoix.configChunkSize") private var configChunkSize = ""
    @AppStorage("envoix.candidatesAllow") private var candidatesAllow = ""
    @AppStorage("envoix.candidatesDeny") private var candidatesDeny = ""
    @AppStorage("envoix.speedLimit") private var speedLimit = 40
    @State private var invite: String = ""
    @State private var roomCode = ""
    @State private var pairingInvite: FfiPairingInvite?
    @State private var roomQRCodeImage: PlatformImage?
    @State private var roomQRCodePayload = ""
    @State private var mode: PairingMode = .room
    @State private var pairingPanel: PairingPanelMode = .show
    @State private var dropTargeted = false
    @State private var filePathInput = ""
    @State private var isFileImporterPresented = false
    @State private var isQRScannerPresented = false
    @State private var selectedFileAccess: AnyObject?
    @State private var selectedPendingSelectionID: UUID?

    init(
        viewModel: TransferViewModel,
        initialMode: PairingMode = .room,
        initialFile: URL? = nil,
        initialFileAccess: AnyObject? = nil,
        initialPendingSelectionID: UUID? = nil
    ) {
        self.viewModel = viewModel
        _mode = State(initialValue: initialMode)
        _file = State(initialValue: initialFile)
        _filePathInput = State(initialValue: initialFile?.path ?? "")
        _selectedFileAccess = State(initialValue: initialFileAccess)
        _selectedPendingSelectionID = State(initialValue: initialPendingSelectionID)
    }

    var body: some View {
        #if os(iOS)
        VStack(spacing: 0) {
            scrollContent
        }
        .safeAreaInset(edge: .bottom) { bottomActionBar }
        .sheet(isPresented: $isQRScannerPresented) {
            QRCodeScannerSheet(language: uiLanguage) { value in
                handleScannedInvite(value)
            }
        }
        .onAppear(perform: adoptSharedSelectionIfAvailable)
        .onChange(of: model.pendingSendSelection?.id) { _ in
            adoptSharedSelectionIfAvailable()
        }
        #else
        VStack(spacing: 0) {
            scrollContent
            footerMessage
            primaryButton
                .padding(.top, 12)
        }
        #endif
    }

    private var scrollContent: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 16) {
                #if os(iOS)
                fileSection
                connectionSection
                #else
                connectionSection
                fileSection
                #endif
                modeSelector
                #if os(macOS)
                TransferStatusView(viewModel: viewModel)
                #endif
            }
            .padding(.vertical, 12)
        }
        .onAppear { refreshPairingInviteIfNeeded() }
        .onChange(of: mode) { newMode in
            if newMode == .room {
                refreshPairingInviteIfNeeded()
            }
        }
        .onChange(of: serverURL) { _ in refreshPairingInviteForSettingsChange() }
        .onChange(of: relayURL) { _ in refreshPairingInviteForSettingsChange() }
    }

    @ViewBuilder private var connectionSection: some View {
        if mode == .invite {
            inviteSection
        } else if mode == .room {
            roomModeSection
        } else {
            TokenField(token: $token, disabled: viewModel.isBusy)
                .card(padding: 14)
        }
    }

    @ViewBuilder private var roomModeSection: some View {
        #if os(iOS)
        VStack(alignment: .leading, spacing: 14) {
            PairingPanelSelector(selection: $pairingPanel, disabled: viewModel.isBusy)
            Text(AppText.value(
                "Show your send QR, or scan the other device's receive QR.",
                "可以显示本机发送码，也可以扫描另一台设备的接收码。",
                language: uiLanguage
            ))
            .font(.footnote)
            .foregroundStyle(Theme.muted)
            .fixedSize(horizontal: false, vertical: true)
            .accessibilityIdentifier("send_pairing_guidance")

            Group {
                if pairingPanel == .scan {
                    VStack(spacing: 14) {
                        Image(systemName: "qrcode.viewfinder")
                            .font(.system(size: 48, weight: .medium))
                            .foregroundStyle(Theme.accentStrong)
                        Text(AppText.value("Scan a receive QR", "扫描接收码", language: uiLanguage))
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
                        .accessibilityIdentifier("send_scan_receiver_qr")
                    }
                    .frame(maxWidth: .infinity, minHeight: 230)
                } else if !roomCode.trimmed.isEmpty {
                    VStack(spacing: 12) {
                        Image(systemName: "checkmark.circle.fill")
                            .font(.system(size: 42))
                            .foregroundStyle(Theme.success)
                        Text(AppText.value("Ready to join", "已准备加入", language: uiLanguage))
                            .font(.headline.weight(.semibold))
                            .foregroundStyle(Theme.text)
                        Text(roomCode)
                            .font(.body.monospaced().weight(.semibold))
                            .foregroundStyle(Theme.accentStrong)
                            .multilineTextAlignment(.center)
                            .textSelection(.enabled)
                        Button(AppText.value("Clear and show my QR", "清除并显示我的二维码", language: uiLanguage)) {
                            roomCode = ""
                        }
                        .buttonStyle(.bordered)
                    }
                    .frame(maxWidth: .infinity, minHeight: 230)
                } else {
                    VStack(spacing: 12) {
                        if let image = roomQRCodeImage {
                            QRCard(image: image, size: 184)
                                .accessibilityLabel(AppText.value("Send QR code", "发送二维码", language: uiLanguage))
                                .accessibilityIdentifier("send_room_qr")
                        } else {
                            qrPlaceholder
                        }
                        LinkRow(
                            text: pairingInvite?.code ?? AppText.value("Send code", "发送码", language: uiLanguage),
                            displaysFullText: true
                        ) {
                            Button {
                                copyWithToast(pairingInvite?.code ?? "", AppText.value("Send code copied", "发送码已复制", language: uiLanguage))
                            } label: {
                                Label(AppText.value("Copy", "复制", language: uiLanguage), systemImage: "doc.on.doc")
                                    .frame(minHeight: 40)
                            }
                            .disabled(pairingInvite == nil)
                            .accessibilityIdentifier("send_room_copy")
                        }
                    }
                    .frame(maxWidth: .infinity, minHeight: 230)
                }
            }

            RoomCodeField(
                code: roomCodeBinding,
                disabled: viewModel.isBusy,
                title: AppText.value("Or enter a code", "或输入短码", language: uiLanguage),
                placeholder: AppText.value("Enter code", "输入短码", language: uiLanguage),
                showsCopyAction: false,
                pasteAction: pastePairingInput,
                helper: ""
            )
        }
        .card(raised: true, padding: 18)
        #else
        VStack(alignment: .center, spacing: 16) {
            VStack(spacing: 4) {
                Text(AppText.value("Share this QR or code", "分享二维码或发送码", language: uiLanguage))
                    .font(.title2.weight(.semibold))
                    .foregroundStyle(Theme.text)
                Text(AppText.value(
                    "The receiver can scan this QR or enter the short code. You can also scan a receiver code below.",
                    "接收端可以扫码或输入短码；你也可以在下方扫描接收端的码。",
                    language: uiLanguage
                ))
                .font(.body)
                .foregroundStyle(Theme.muted)
                .multilineTextAlignment(.center)
            }

            if let image = roomQRCodeImage {
                QRCard(image: image, size: 208)
                    .accessibilityLabel(AppText.value("Send QR code", "发送二维码", language: uiLanguage))
                    .accessibilityIdentifier("send_room_qr")
            } else {
                qrPlaceholder
            }

            LinkRow(
                text: pairingInvite?.code ?? AppText.value("Send code", "发送码", language: uiLanguage),
                textIdentifier: "send_room_code",
                displaysFullText: true
            ) {
                Button {
                    copyWithToast(pairingInvite?.code ?? "", AppText.value("Send code copied", "发送码已复制", language: uiLanguage))
                } label: {
                    Label(AppText.value("Copy", "复制", language: uiLanguage), systemImage: "doc.on.doc")
                        .frame(minHeight: 34)
                        .contentShape(Rectangle())
                }
                .disabled(pairingInvite == nil)

                Button {
                    roomCode = ""
                    refreshPairingInvite()
                } label: {
                    Label(AppText.value("New", "新建", language: uiLanguage), systemImage: "arrow.clockwise")
                        .frame(minHeight: 34)
                        .contentShape(Rectangle())
                }
                .disabled(viewModel.isBusy)
            }

            RoomCodeField(
                code: roomCodeBinding,
                disabled: viewModel.isBusy,
                title: AppText.value("Join receiver instead", "改为加入接收端", language: uiLanguage),
                placeholder: AppText.value("Scan QR or enter receiver code", "扫码或输入接收端短码", language: uiLanguage),
                helper: AppText.value("Leave this empty to use your send code above.", "留空则使用上方发送码。", language: uiLanguage)
            )

            HStack(spacing: 8) {
                Button {
                    pastePairingInput()
                } label: {
                    Label(AppText.value("Paste", "粘贴", language: uiLanguage), systemImage: "doc.on.clipboard")
                        .frame(minHeight: 34)
                        .contentShape(Rectangle())
                }
                .disabled(viewModel.isBusy)

                #if os(iOS)
                Button {
                    isQRScannerPresented = true
                } label: {
                    Label(AppText.value("Scan QR", "扫码", language: uiLanguage), systemImage: "qrcode.viewfinder")
                        .frame(minHeight: 34)
                        .contentShape(Rectangle())
                }
                .disabled(viewModel.isBusy)
                #endif
            }
            .buttonStyle(.bordered)
            .controlSize(.small)
        }
        .card(raised: true, padding: 18)
        #endif
    }

    private var roomCodeBinding: Binding<String> {
        Binding(
            get: { roomCode },
            set: { value in roomCode = value }
        )
    }

    @ViewBuilder private var footerMessage: some View {
        if concurrencyBlocked {
            Text(AppText.value("Finish receiving before starting a send.", "请先完成接收任务，再开始发送。", language: uiLanguage))
                .font(.callout)
                .foregroundStyle(Theme.muted)
                .padding(.bottom, 8)
        }
    }

    private var primaryButton: some View {
        Button(action: primaryAction) {
            Label(
                primaryLabel,
                systemImage: viewModel.isBusy ? "list.bullet.rectangle" : "paperplane"
            )
                .frame(maxWidth: .infinity, minHeight: 44)
                .contentShape(Rectangle())
        }
        .keyboardShortcut(.defaultAction)
        .buttonStyle(PrimaryActionButtonStyle())
        .disabled(viewModel.isBusy || viewModel.isFinalizing || !canSend || concurrencyBlocked)
        .accessibilityIdentifier("send_start_button")
    }

    #if os(iOS)
    private var bottomActionBar: some View {
        Group {
            if !viewModel.isBusy {
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

    private var modeSelector: some View {
        PairingModeSelector(selection: $mode, role: .send, disabled: viewModel.isBusy)
    }

    private var fileSection: some View {
        VStack(alignment: .leading, spacing: 12) {
            Text(AppText.value("File to send", "要发送的文件", language: uiLanguage))
                .font(.headline.weight(.semibold))
                .foregroundStyle(Theme.text)
            Button {
                #if os(iOS)
                isFileImporterPresented = true
                #else
                if let url = chooseURL(directory: false) { selectFile(url) }
                #endif
            } label: {
                fileChooserLabel
            }
            .buttonStyle(.plain)
            .disabled(viewModel.isBusy)
            .accessibilityIdentifier("send_file_picker")

            Text(AppText.value(
                "One file at a time. Multiple files and folders are coming with Manifest support.",
                "目前一次只能发送一个文件；多文件和文件夹将在 Manifest 支持后开放。",
                language: uiLanguage
            ))
            .font(.caption)
            .foregroundStyle(Theme.muted)
            .accessibilityIdentifier("send_selection_limit")

            #if os(macOS)
            filePathTools
            #endif
        }
        .padding(18)
        .frame(maxWidth: .infinity)
        .background(dropTargeted ? Theme.accentSoft : Theme.surface)
        .overlay(
            RoundedRectangle(cornerRadius: Theme.cardRadius)
                .strokeBorder(
                    dropTargeted ? Theme.accent : Theme.accent.opacity(0.45),
                    style: StrokeStyle(lineWidth: dropTargeted ? 2 : 1.2, dash: [8])
                )
        )
        .clipShape(RoundedRectangle(cornerRadius: Theme.cardRadius))
        .onDrop(of: [.fileURL], isTargeted: $dropTargeted) { providers in
            guard !viewModel.isBusy else { return false }
            guard providers.count == 1, let provider = providers.first else {
                ToastCenter.shared.show(unsupportedSelectionMessage)
                return false
            }
            _ = provider.loadObject(ofClass: URL.self) { url, _ in
                if let url { DispatchQueue.main.async { selectFile(url) } }
            }
            return true
        }
        #if os(iOS)
        .fileImporter(
            isPresented: $isFileImporterPresented,
            allowedContentTypes: [.item],
            allowsMultipleSelection: false
        ) { result in
            handleImportedFile(result)
        }
        #endif
    }

    @ViewBuilder private var fileChooserLabel: some View {
        #if os(iOS)
        HStack(spacing: 12) {
            Image(systemName: file == nil ? "doc.badge.plus" : "doc.fill")
                .font(.system(size: 28, weight: .semibold))
                .foregroundStyle(Theme.accentStrong)
                .frame(width: 38, height: 38)
                .background(Theme.accentSoft, in: RoundedRectangle(cornerRadius: Theme.cardRadius))

            VStack(alignment: .leading, spacing: 4) {
                Text(file?.lastPathComponent ?? AppText.value("Choose file", "选择文件", language: uiLanguage))
                    .font(.headline.weight(.semibold))
                    .foregroundStyle(file == nil ? Theme.text : Theme.accentStrong)
                    .lineLimit(1)
                    .truncationMode(.middle)
                Text(file == nil
                     ? AppText.value("Tap to open Files.", "点击打开文件。", language: uiLanguage)
                     : AppText.value("Tap to replace.", "点击可替换。", language: uiLanguage))
                    .font(.footnote)
                    .foregroundStyle(Theme.muted)
                    .fixedSize(horizontal: false, vertical: true)
            }

            Spacer(minLength: 8)
            Image(systemName: "chevron.right")
                .font(.footnote.weight(.semibold))
                .foregroundStyle(Theme.muted)
        }
        .frame(maxWidth: .infinity, minHeight: 76, alignment: .leading)
        .contentShape(RoundedRectangle(cornerRadius: Theme.cardRadius))
        #else
        VStack(spacing: 10) {
            Image(systemName: file == nil ? "square.and.arrow.up" : "doc.fill")
                .font(.system(size: 48, weight: .semibold))
                .foregroundStyle(Theme.accentStrong)

            Text(file?.lastPathComponent ?? AppText.value("Drag here or click to choose", "拖到这里或点击选择", language: uiLanguage))
                .font(.title2.weight(.semibold))
                .foregroundStyle(file == nil ? Theme.text : Theme.accentStrong)
                .lineLimit(1)
                .truncationMode(.middle)

            Text(file == nil
                 ? AppText.value("Drop a file into this area, or click anywhere here to select one.", "把文件拖到这里，或点击此区域选择文件。", language: uiLanguage)
                 : AppText.value("Ready to share. Click this area to replace the file.", "已准备好分享。点击此区域可替换文件。", language: uiLanguage))
                .font(.body)
                .foregroundStyle(Theme.muted)
                .multilineTextAlignment(.center)
                .lineLimit(2)
        }
        .frame(maxWidth: .infinity, minHeight: 150)
        .contentShape(RoundedRectangle(cornerRadius: Theme.cardRadius))
        #endif
    }

    private var filePathTools: some View {
        HStack(spacing: 8) {
            Image(systemName: "link")
                .font(.callout.weight(.semibold))
                .foregroundStyle(Theme.muted)

            TextField(AppText.value("Paste an absolute file path here", "在这里粘贴绝对文件路径", language: uiLanguage), text: $filePathInput)
                .textFieldStyle(.plain)
                .font(.callout.monospaced())
                .foregroundStyle(Theme.text)
                .onSubmit(applyPathInput)
                .disabled(viewModel.isBusy)

            Button(action: applyPathInput) {
                Label(AppText.value("Use Path", "使用路径", language: uiLanguage), systemImage: "checkmark")
                    .labelStyle(.iconOnly)
                    .frame(width: 28, height: 28)
                    .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .foregroundStyle(filePathInput.trimmed.isEmpty ? Theme.muted : Theme.accentStrong)
            .disabled(viewModel.isBusy || filePathInput.trimmed.isEmpty)
            .help(AppText.value("Use pasted path", "使用粘贴的路径", language: uiLanguage))

            Button {
                if let file { copyWithToast(file.path, AppText.value("File path copied", "文件路径已复制", language: uiLanguage)) }
            } label: {
                Label(AppText.value("Copy Selected Path", "复制已选路径", language: uiLanguage), systemImage: "doc.on.doc")
                    .labelStyle(.iconOnly)
                    .frame(width: 28, height: 28)
                    .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .foregroundStyle(file == nil ? Theme.muted : Theme.accentStrong)
            .disabled(file == nil)
            .help(AppText.value("Copy selected path", "复制已选择文件的路径", language: uiLanguage))
        }
        .padding(.horizontal, 10)
        .frame(minHeight: 44)
        .background(Theme.surface)
        .overlay(
            RoundedRectangle(cornerRadius: Theme.cardRadius)
                .strokeBorder(Theme.line.opacity(0.75), lineWidth: 0.8)
        )
        .clipShape(RoundedRectangle(cornerRadius: Theme.cardRadius))
    }

    private var inviteSection: some View {
        VStack(alignment: .leading, spacing: 10) {
            VStack(alignment: .leading, spacing: 3) {
                Text(AppText.value("Receiver invite link", "接收端邀请链接", language: uiLanguage))
                    .font(.title3.weight(.semibold))
                    .foregroundStyle(Theme.text)
                Text(AppText.value("Paste the link or QR result from the receiving device.", "粘贴接收端生成的链接或二维码内容。", language: uiLanguage))
                    .font(.body)
                    .foregroundStyle(Theme.muted)
            }
            HStack(alignment: .top, spacing: 8) {
                TextField("envoix:… / envoix://pair/…", text: $invite, axis: .vertical)
                    .textFieldStyle(.plain)
                    .font(.body.monospaced())
                    .foregroundStyle(Theme.text)
                    .lineLimit(1...3)
                    .disabled(viewModel.isBusy)
                Button {
                    invite = pasteboardString()?.trimmed ?? invite
                    ToastCenter.shared.show(AppText.value("Invite pasted", "邀请已粘贴", language: uiLanguage))
                } label: {
                    Label(AppText.value("Paste", "粘贴", language: uiLanguage), systemImage: "doc.on.clipboard")
                        .frame(minHeight: 34)
                        .contentShape(Rectangle())
                }
                .disabled(viewModel.isBusy)
                #if os(iOS)
                Button {
                    isQRScannerPresented = true
                } label: {
                    Label(AppText.value("Scan", "扫码", language: uiLanguage), systemImage: "qrcode.viewfinder")
                        .frame(minHeight: 34)
                        .contentShape(Rectangle())
                }
                .disabled(viewModel.isBusy)
                #endif
            }
            .padding(.horizontal, 10)
            .frame(minHeight: 44)
            .background(Theme.surface)
            .overlay(
                RoundedRectangle(cornerRadius: Theme.cardRadius)
                    .strokeBorder(Theme.line.opacity(0.75), lineWidth: 0.8)
            )
            .clipShape(RoundedRectangle(cornerRadius: Theme.cardRadius))
        }
        .card(padding: 14)
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

    private func handleScannedInvite(_ value: String) {
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
        applyPairingInput(value, source: .paste)
    }

    private func applyPairingInput(_ value: String, source: PairingInputSource) {
        let input = value.trimmed
        let lowercased = input.lowercased()
        if lowercased.hasPrefix("envoix:") && !lowercased.hasPrefix("envoix://pair/") {
            invite = input
            mode = .invite
            ToastCenter.shared.show(AppText.value("Legacy invite loaded", "已载入旧版邀请", language: uiLanguage))
            return
        }

        do {
            let parsed = try parsePairingInvite(input: input)
            guard parsed.role == .receive else {
                ToastCenter.shared.show(AppText.value(
                    "Scan a receiver code or share your send code.",
                    "请扫描接收端的码，或分享你的发送码。",
                    language: uiLanguage
                ))
                return
            }
            roomCode = parsed.code
            pairingPanel = .show
            if !parsed.broker.trimmed.isEmpty {
                serverURL = parsed.broker.trimmed
            }
            if !parsed.relay.trimmed.isEmpty {
                relayURL = parsed.relay.trimmed
            }
            mode = .room
            invite = ""
            let message = source == .scan
                ? AppText.value("QR scanned", "二维码已扫描", language: uiLanguage)
                : AppText.value("Pairing code pasted", "配对码已粘贴", language: uiLanguage)
            ToastCenter.shared.show(message)
        } catch {
            ToastCenter.shared.show(AppText.value("This is not a valid Envoix pairing code.", "这不是有效的 Envoix 配对码。", language: uiLanguage))
        }
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
            let invite = try makePairingInvite(role: .send, broker: serverURL, relay: relayURL)
            pairingInvite = invite
            updateRoomQRCode(for: invite.payload)
        } catch {
            viewModel.handleFailed(error.localizedDescription)
        }
    }

    private func activeSendRoomCode() throws -> String {
        if let invite = pairingInvite {
            let code = invite.code.trimmed
            if !code.isEmpty {
                updateRoomQRCode(for: invite.payload)
                return code
            }
        }
        let invite = try makePairingInvite(role: .send, broker: serverURL, relay: relayURL)
        pairingInvite = invite
        updateRoomQRCode(for: invite.payload)
        return invite.code
    }

    private func updateRoomQRCode(for payload: String) {
        guard roomQRCodePayload != payload else { return }
        roomQRCodePayload = payload
        roomQRCodeImage = payload.isEmpty ? nil : QRCode.image(from: payload)
    }

    private func runtimeSettings(for parsed: FfiPairingInvite) throws -> EnvoixRuntimeSettings {
        try RuntimeSettingsProvider.make(
            concurrentTransfers: concurrentTransfers,
            language: language,
            serverURL: parsed.broker.trimmed.isEmpty ? serverURL : parsed.broker,
            relayURL: parsed.relay.trimmed.isEmpty ? relayURL : parsed.relay,
            configChunkSize: configChunkSize,
            candidatesAllow: candidatesAllow,
            candidatesDeny: candidatesDeny,
            speedLimit: speedLimit
        )
    }

    private var primaryLabel: String {
        if viewModel.isBusy { return AppText.value("Managed in Activity", "请在活动中管理", language: uiLanguage) }
        switch viewModel.phase {
        case .completed, .canceled, .failed: return AppText.value("Send Again", "再次发送", language: uiLanguage)
        default: return AppText.value("Send", "发送", language: uiLanguage)
        }
    }

    private var canSend: Bool {
        guard file != nil else { return false }
        switch mode {
        case .room:
            return !roomCode.trimmed.isEmpty || pairingInvite != nil
        case .invite:
            return !invite.trimmed.isEmpty
        case .token:
            return token.trimmed.count >= minTokenLength
        }
    }

    private var concurrencyBlocked: Bool {
        !concurrentTransfers && !viewModel.isBusy && model.receive.isBusy
    }

    private var unsupportedSelectionMessage: String {
        AppText.value(
            "Multiple files and folders are not supported yet. Manifest support is coming next.",
            "暂不支持多文件和文件夹；将在 Manifest 支持后开放。",
            language: uiLanguage
        )
    }

    @discardableResult
    private func selectFile(
        _ url: URL,
        access: AnyObject? = nil,
        pendingSelectionID: UUID? = nil
    ) -> Bool {
        var isDirectory: ObjCBool = false
        guard FileManager.default.fileExists(atPath: url.path, isDirectory: &isDirectory),
              !isDirectory.boolValue else {
            ToastCenter.shared.show(unsupportedSelectionMessage)
            return false
        }
        selectedFileAccess = access
        selectedPendingSelectionID = pendingSelectionID
        file = url
        filePathInput = url.path
        return true
    }

    #if os(iOS)
    private func adoptSharedSelectionIfAvailable() {
        guard !viewModel.isBusy,
              let selection = model.pendingSendSelection else { return }
        if selection.id == selectedPendingSelectionID {
            model.consumePendingSendSelection(id: selection.id)
            return
        }
        guard selectFile(
            selection.fileURL,
            access: selection.sourceAccess,
            pendingSelectionID: selection.id
        ) else { return }
        model.consumePendingSendSelection(id: selection.id)
        ToastCenter.shared.show(AppText.value(
            "Shared item ready to send",
            "分享项目已准备发送",
            language: uiLanguage
        ))
    }
    #endif

    private func acknowledgedSourceAccess() -> AnyObject? {
        #if os(iOS)
        (selectedFileAccess as? ShareDraftLease)?.acknowledge()
        #endif
        return selectedFileAccess
    }

    private func handleImportedFile(_ result: Result<[URL], Error>) {
        do {
            let urls = try result.get()
            guard urls.count == 1, let url = urls.first else {
                throw RuntimeSettingsError(unsupportedSelectionMessage)
            }
            #if os(iOS)
            let access = SecurityScopedResourceAccess(url: url)
            guard access.isActive || FileManager.default.isReadableFile(atPath: url.path) else {
                throw RuntimeSettingsError(AppText.value(
                    "Envoix could not access the selected file. Choose it again from Files.",
                    "Envoix 无法访问所选文件。请从 Files 中重新选择。",
                    language: uiLanguage
                ))
            }
            guard selectFile(url, access: access) else { return }
            #else
            guard selectFile(url) else { return }
            #endif
            ToastCenter.shared.show(AppText.value("File selected", "已选择文件", language: uiLanguage))
        } catch {
            ToastCenter.shared.show(error.localizedDescription)
        }
    }

    private func applyPathInput() {
        let raw = filePathInput.trimmed
        guard !raw.isEmpty else { return }

        let path = (raw as NSString).expandingTildeInPath
        var isDirectory: ObjCBool = false
        guard FileManager.default.fileExists(atPath: path, isDirectory: &isDirectory), !isDirectory.boolValue else {
            ToastCenter.shared.show(AppText.value("File path not found", "未找到文件路径", language: uiLanguage))
            return
        }

        selectFile(URL(fileURLWithPath: path))
        ToastCenter.shared.show(AppText.value("File path selected", "已选择文件路径", language: uiLanguage))
    }

    private func primaryAction() {
        guard !viewModel.isBusy, !viewModel.isFinalizing else { return }
        guard let file else { return }
        do {
            let settings = try RuntimeSettingsProvider.make(
                concurrentTransfers: concurrentTransfers,
                language: language,
                serverURL: serverURL,
                relayURL: relayURL,
                configChunkSize: configChunkSize,
                candidatesAllow: candidatesAllow,
                candidatesDeny: candidatesDeny,
                speedLimit: speedLimit
            )
            switch mode {
            case .room:
                let input = roomCode.trimmed
                let lowercasedInput = input.lowercased()
                if input.isEmpty {
                    viewModel.startSendingWithRoom(
                        filePath: file.path,
                        code: try activeSendRoomCode(),
                        settings: settings,
                        sourceAccess: acknowledgedSourceAccess()
                    )
                } else if lowercasedInput.hasPrefix("envoix://pair/") {
                    let parsed = try parsePairingInvite(input: input)
                    guard parsed.role == .receive else {
                        throw RuntimeSettingsError(AppText.value(
                            "Scan a receiver code or share your send code.",
                            "请扫描接收端的码，或分享你的发送码。",
                            language: uiLanguage
                        ))
                    }
                    roomCode = parsed.code
                    let roomSettings = try runtimeSettings(for: parsed)
                    viewModel.startSendingWithRoom(
                        filePath: file.path,
                        code: parsed.code,
                        settings: roomSettings,
                        sourceAccess: acknowledgedSourceAccess()
                    )
                } else if lowercasedInput.hasPrefix("envoix:") {
                    invite = input
                    mode = .invite
                    viewModel.startSendingWithInvite(
                        filePath: file.path,
                        invite: input,
                        settings: settings,
                        sourceAccess: acknowledgedSourceAccess()
                    )
                } else {
                    viewModel.startSendingWithRoom(
                        filePath: file.path,
                        code: input,
                        settings: settings,
                        sourceAccess: acknowledgedSourceAccess()
                    )
                }
            case .invite:
                if invite.trimmed.lowercased().hasPrefix("envoix://pair/") {
                    let parsed = try parsePairingInvite(input: invite.trimmed)
                    guard parsed.role == .receive else {
                        throw RuntimeSettingsError(AppText.value(
                            "Scan a receiver code or share your send code.",
                            "请扫描接收端的码，或分享你的发送码。",
                            language: uiLanguage
                        ))
                    }
                    mode = .room
                    roomCode = parsed.code
                    let roomSettings = try runtimeSettings(for: parsed)
                    viewModel.startSendingWithRoom(
                        filePath: file.path,
                        code: parsed.code,
                        settings: roomSettings,
                        sourceAccess: acknowledgedSourceAccess()
                    )
                } else {
                    viewModel.startSendingWithInvite(
                        filePath: file.path,
                        invite: invite.trimmed,
                        settings: settings,
                        sourceAccess: acknowledgedSourceAccess()
                    )
                }
            case .token:
                viewModel.startSendingWithToken(
                    filePath: file.path,
                    token: token.trimmed,
                    settings: settings,
                    sourceAccess: acknowledgedSourceAccess()
                )
            }
        } catch {
            viewModel.handleFailed(error.localizedDescription)
        }
    }
}
