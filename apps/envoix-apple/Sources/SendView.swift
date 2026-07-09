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
    @AppStorage("envoix.speedLimit") private var speedLimit = 40
    @State private var invite: String = ""
    @State private var roomCode = ""
    @State private var pairingInvite: FfiPairingInvite?
    @State private var roomQRCodeImage: PlatformImage?
    @State private var roomQRCodePayload = ""
    @State private var mode: PairingMode = .room
    @State private var dropTargeted = false
    @State private var filePathInput = ""
    @State private var isFileImporterPresented = false
    @State private var isQRScannerPresented = false
    @State private var selectedFileAccess: AnyObject?

    init(viewModel: TransferViewModel, initialMode: PairingMode = .room) {
        self.viewModel = viewModel
        _mode = State(initialValue: initialMode)
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
                modeSelector

                if mode == .invite {
                    inviteSection
                } else if mode == .room {
                    roomModeSection
                } else {
                    TokenField(token: $token, disabled: viewModel.isBusy)
                        .card(padding: 14)
                }

                fileSection
                TransferStatusView(viewModel: viewModel)
            }
            .padding(.vertical, 12)
            #if os(iOS)
            .padding(.bottom, 88)
            #endif
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

    @ViewBuilder private var roomModeSection: some View {
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
                    .accessibilityIdentifier("send_room_qr")
            } else {
                qrPlaceholder
            }

            LinkRow(
                text: pairingInvite?.code ?? AppText.value("Send code", "发送码", language: uiLanguage),
                textIdentifier: "send_room_code"
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
            Label(primaryLabel, systemImage: viewModel.isBusy ? "xmark" : "paperplane")
                .frame(maxWidth: .infinity, minHeight: 44)
                .contentShape(Rectangle())
        }
        .keyboardShortcut(.defaultAction)
        .buttonStyle(.borderedProminent)
        .controlSize(.large)
        .tint(viewModel.isBusy ? Theme.warning : Theme.accent)
        .disabled((!canSend || concurrencyBlocked) && !viewModel.isBusy)
        .accessibilityIdentifier("send_start_button")
    }

    #if os(iOS)
    private var bottomActionBar: some View {
        VStack(spacing: 8) {
            footerMessage
            primaryButton
        }
        .padding(.horizontal, 16)
        .padding(.top, 10)
        .padding(.bottom, 8)
        .background(.regularMaterial)
    }
    #endif

    private var modeSelector: some View {
        PairingModeSelector(selection: $mode, role: .send, disabled: viewModel.isBusy)
    }

    private var fileSection: some View {
        VStack(spacing: 12) {
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
            guard !viewModel.isBusy, let provider = providers.first else { return false }
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
                    .lineLimit(1)
            }

            Spacer(minLength: 8)
            Image(systemName: "chevron.right")
                .font(.footnote.weight(.semibold))
                .foregroundStyle(Theme.muted)
        }
        .frame(maxWidth: .infinity, minHeight: 68, alignment: .leading)
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
            guard parsed.role != .send else {
                ToastCenter.shared.show(AppText.value(
                    "This QR belongs to another sender. Switch to Receive or scan a receiver code.",
                    "这是另一个发送端的二维码。请切到接收，或扫描接收端的码。",
                    language: uiLanguage
                ))
                return
            }
            roomCode = parsed.code
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
            speedLimit: speedLimit
        )
    }

    private var primaryLabel: String {
        if viewModel.isBusy { return AppText.value("Cancel Transfer", "取消传输", language: uiLanguage) }
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

    private func selectFile(_ url: URL, access: AnyObject? = nil) {
        selectedFileAccess = access
        file = url
        filePathInput = url.path
    }

    private func handleImportedFile(_ result: Result<[URL], Error>) {
        do {
            guard let url = try result.get().first else { return }
            #if os(iOS)
            let access = SecurityScopedResourceAccess(url: url)
            guard access.isActive || FileManager.default.isReadableFile(atPath: url.path) else {
                throw RuntimeSettingsError(AppText.value(
                    "Envoix could not access the selected file. Choose it again from Files.",
                    "Envoix 无法访问所选文件。请从 Files 中重新选择。",
                    language: uiLanguage
                ))
            }
            selectFile(url, access: access)
            #else
            selectFile(url)
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
        if viewModel.isBusy {
            viewModel.cancel()
            return
        }
        guard let file else { return }
        do {
            let settings = try RuntimeSettingsProvider.make(
                concurrentTransfers: concurrentTransfers,
                language: language,
                serverURL: serverURL,
                relayURL: relayURL,
                configChunkSize: configChunkSize,
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
                        sourceAccess: selectedFileAccess
                    )
                } else if lowercasedInput.hasPrefix("envoix://pair/") {
                    let parsed = try parsePairingInvite(input: input)
                    guard parsed.role != .send else {
                        throw RuntimeSettingsError(AppText.value(
                            "This QR belongs to another sender. Switch to Receive or scan a receiver code.",
                            "这是另一个发送端的二维码。请切到接收，或扫描接收端的码。",
                            language: uiLanguage
                        ))
                    }
                    roomCode = parsed.code
                    let roomSettings = try runtimeSettings(for: parsed)
                    viewModel.startSendingWithRoom(
                        filePath: file.path,
                        code: parsed.code,
                        settings: roomSettings,
                        sourceAccess: selectedFileAccess
                    )
                } else if lowercasedInput.hasPrefix("envoix:") {
                    invite = input
                    mode = .invite
                    viewModel.startSendingWithInvite(
                        filePath: file.path,
                        invite: input,
                        settings: settings,
                        sourceAccess: selectedFileAccess
                    )
                } else {
                    viewModel.startSendingWithRoom(
                        filePath: file.path,
                        code: input,
                        settings: settings,
                        sourceAccess: selectedFileAccess
                    )
                }
            case .invite:
                if invite.trimmed.lowercased().hasPrefix("envoix://pair/") {
                    let parsed = try parsePairingInvite(input: invite.trimmed)
                    guard parsed.role != .send else {
                        throw RuntimeSettingsError(AppText.value(
                            "This QR belongs to another sender. Switch to Receive or scan a receiver code.",
                            "这是另一个发送端的二维码。请切到接收，或扫描接收端的码。",
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
                        sourceAccess: selectedFileAccess
                    )
                } else {
                    viewModel.startSendingWithInvite(
                        filePath: file.path,
                        invite: invite.trimmed,
                        settings: settings,
                        sourceAccess: selectedFileAccess
                    )
                }
            case .token:
                viewModel.startSendingWithToken(
                    filePath: file.path,
                    token: token.trimmed,
                    settings: settings,
                    sourceAccess: selectedFileAccess
                )
            }
        } catch {
            viewModel.handleFailed(error.localizedDescription)
        }
    }
}
