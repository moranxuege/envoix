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
    @AppStorage("envoix.configChunkSize") private var configChunkSize = ""
    @AppStorage("envoix.speedLimit") private var speedLimit = 40
    @State private var mode: PairingMode = .room
    @State private var roomCode = newRoomCode()
    @State private var pairingInvite: FfiPairingInvite?
    @State private var revealAddress = false
    #if os(iOS)
    @State private var isFolderPickerPresented = false
    @State private var shouldStartAfterFolderPick = false
    #endif

    init(viewModel: TransferViewModel, initialMode: PairingMode = .room) {
        self.viewModel = viewModel
        _mode = State(initialValue: initialMode)
    }

    private let outputDirBookmarkKey = "envoix.outputDirBookmark"

    /// Defaults to ~/Downloads on macOS and the app's Files-visible
    /// Documents/Downloads folder on iOS. A user-selected Files folder is used
    /// only when a persisted bookmark exists.
    private var outputDir: URL? {
        #if os(iOS)
        if let data = UserDefaults.standard.data(forKey: outputDirBookmarkKey) {
            return try? resolveSecurityScopedFolderBookmark(data)
        }
        return defaultIOSOutputDir
        #else
        if !outputDirPath.isEmpty { return URL(fileURLWithPath: outputDirPath) }
        return FileManager.default.urls(for: .downloadsDirectory, in: .userDomainMask).first
            ?? FileManager.default.homeDirectoryForCurrentUser
        #endif
    }

    #if os(iOS)
    private var hasCustomOutputDir: Bool {
        UserDefaults.standard.data(forKey: outputDirBookmarkKey) != nil
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
                outputSection
                modeSelector

                if mode == .invite {
                    inviteSection
                } else if mode == .room {
                    roomSection
                } else {
                    TokenField(token: $token, disabled: viewModel.isBusy)
                        .card(padding: 14)
                }

                if !viewModel.peerAddress.isEmpty {
                    addressReveal
                }

                TransferStatusView(viewModel: viewModel)
            }
            .padding(.vertical, 12)
            #if os(iOS)
            .padding(.bottom, 88)
            #endif
        }
        .onAppear { refreshPairingInviteIfNeeded() }
        .onChange(of: serverURL) { _ in refreshPairingInviteForSettingsChange() }
        .onChange(of: relayURL) { _ in refreshPairingInviteForSettingsChange() }
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
            Label(primaryLabel, systemImage: viewModel.isBusy ? "xmark" : "tray.and.arrow.down")
                .frame(maxWidth: .infinity, minHeight: 44)
                .contentShape(Rectangle())
        }
        .keyboardShortcut(.defaultAction)
        .buttonStyle(.borderedProminent)
        .controlSize(.large)
        .tint(viewModel.isBusy ? Theme.warning : Theme.accent)
        .disabled((!canStart || concurrencyBlocked) && !viewModel.isBusy)
    }

    #if os(iOS)
    private var bottomActionBar: some View {
        VStack(spacing: 8) {
            footerMessage
            primaryButton
        }
        .padding(.top, 10)
        .padding(.bottom, 8)
        .background(.regularMaterial)
    }
    #endif

    private var modeSelector: some View {
        PairingModeSelector(selection: $mode, role: .receive, disabled: viewModel.isBusy)
    }

    private var outputSection: some View {
        VStack(alignment: .leading, spacing: 8) {
            Text(AppText.value("Save to", "保存到", language: uiLanguage))
                .font(.title3.weight(.semibold))
                .foregroundStyle(Theme.muted)
            #if os(iOS)
            LinkRow(text: outputDirDisplayText) {
                Button {
                    isFolderPickerPresented = true
                } label: {
                    Label(AppText.value("Choose", "选择", language: uiLanguage), systemImage: "folder")
                        .frame(minHeight: 34)
                        .contentShape(Rectangle())
                }
                .disabled(viewModel.isBusy)
                if hasCustomOutputDir {
                    Button {
                        resetOutputFolder()
                    } label: {
                        Label(AppText.value("Reset", "重置", language: uiLanguage), systemImage: "arrow.uturn.backward")
                            .labelStyle(.iconOnly)
                            .frame(width: 30, height: 30)
                            .contentShape(Rectangle())
                    }
                    .disabled(viewModel.isBusy)
                }
            }
            Text(AppText.value("Default saves to Files > On My iPhone > Envoix > Downloads. Choose a Files folder to save elsewhere.", "默认保存到 Files > On My iPhone > Envoix > Downloads。也可以选择其他 Files 文件夹。", language: uiLanguage))
                .font(.footnote)
                .foregroundStyle(Theme.muted)
                .fixedSize(horizontal: false, vertical: true)
            #else
            LinkRow(text: outputDir?.path ?? "") {
                Button {
                    if let url = chooseURL(directory: true) { outputDirPath = url.path }
                } label: {
                    Label(AppText.value("Select", "选择", language: uiLanguage), systemImage: "folder")
                        .frame(minHeight: 34)
                        .contentShape(Rectangle())
                }
                .disabled(viewModel.isBusy)
            }
            #endif
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
        return outputDir?.path ?? ""
        #endif
    }

    private var primaryLabel: String {
        if viewModel.isBusy { return AppText.value("Cancel Transfer", "取消传输", language: uiLanguage) }
        switch viewModel.phase {
        case .completed, .canceled, .failed:
            return AppText.value("Receive Again", "再次接收", language: uiLanguage)
        case .idle where mode == .invite:
            return AppText.value("Create Link and Wait", "创建链接并等待", language: uiLanguage)
        default:
            return AppText.value("Start Receiving", "开始接收", language: uiLanguage)
        }
    }

    /// The invite (QR + string) is the pairing artifact meant to be shared, so
    /// it is shown directly.
    @ViewBuilder private var inviteSection: some View {
        VStack(spacing: 16) {
            VStack(spacing: 4) {
                Text(AppText.value("Share this QR or invite link", "分享二维码或邀请链接", language: uiLanguage))
                    .font(.title2.weight(.semibold))
                    .foregroundStyle(Theme.text)
                Text(AppText.value("Create the invite and keep this receiver waiting for the sender.", "创建邀请，并让此接收端等待发送方连接。", language: uiLanguage))
                    .font(.body)
                    .foregroundStyle(Theme.muted)
                    .multilineTextAlignment(.center)
            }

            if let image = QRCode.image(from: viewModel.invite), !viewModel.invite.isEmpty {
                QRCard(image: image, size: 208)
            } else {
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
            LinkRow(text: viewModel.invite.isEmpty ? AppText.value("Invite link", "邀请链接", language: uiLanguage) : viewModel.invite) {
                Button {
                    copyWithToast(viewModel.invite, AppText.value("Invite copied", "邀请已复制", language: uiLanguage))
                } label: {
                    Label(AppText.value("Copy", "复制", language: uiLanguage), systemImage: "doc.on.doc")
                        .frame(minHeight: 34)
                        .contentShape(Rectangle())
                }
                .disabled(viewModel.invite.isEmpty)
                Button {
                    startReceiveWithInvite()
                    ToastCenter.shared.show(AppText.value("Invite created", "邀请已创建", language: uiLanguage))
                } label: {
                    Label(viewModel.invite.isEmpty
                          ? AppText.value("Create and Wait", "创建并等待", language: uiLanguage)
                          : AppText.value("Create New Invite", "创建新邀请", language: uiLanguage),
                          systemImage: "arrow.clockwise")
                        .frame(minHeight: 34)
                        .contentShape(Rectangle())
                }
                .disabled(viewModel.isBusy)
            }
        }
        .card(raised: true, padding: 18)
    }

    private var roomSection: some View {
        VStack(alignment: .center, spacing: 16) {
            VStack(spacing: 4) {
                Text(AppText.value("Share this QR or code", "分享二维码或接收码", language: uiLanguage))
                    .font(.title2.weight(.semibold))
                    .foregroundStyle(Theme.text)
                Text(AppText.value("The sender can scan the QR or enter the same short code.", "发送方可以扫码，或输入同一个短码。", language: uiLanguage))
                    .font(.body)
                    .foregroundStyle(Theme.muted)
                    .multilineTextAlignment(.center)
            }

            if let payload = pairingInvite?.payload,
               let image = QRCode.image(from: payload) {
                QRCard(image: image, size: 208)
            } else {
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

            LinkRow(text: roomCode.trimmed.isEmpty ? AppText.value("Receive code", "接收码", language: uiLanguage) : roomCode) {
                Button {
                    copyWithToast(roomCode, AppText.value("Room code copied", "接收码已复制", language: uiLanguage))
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
        }
        .card(raised: true, padding: 18)
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
            return !roomCode.trimmed.isEmpty
        case .invite:
            return true
        case .token:
            return token.trimmed.count >= minTokenLength
        }
    }

    private var concurrencyBlocked: Bool {
        !concurrentTransfers && !viewModel.isBusy && model.send.isBusy
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
            roomCode = invite.code
        } catch {
            viewModel.handleFailed(error.localizedDescription)
        }
    }

    private func activeRoomCode() throws -> String {
        if let code = pairingInvite?.code.trimmed, !code.isEmpty {
            return code
        }
        let invite = try makePairingInvite(role: .receive, broker: serverURL, relay: relayURL)
        pairingInvite = invite
        roomCode = invite.code
        return invite.code
    }

    private func primaryAction() {
        if viewModel.isBusy {
            viewModel.cancel()
        } else {
            #if os(iOS)
            guard outputDir != nil else {
                shouldStartAfterFolderPick = true
                isFolderPickerPresented = true
                return
            }
            #endif
            startReceive()
        }
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
                configChunkSize: configChunkSize,
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
            let prepared = try prepareOutputDir()
            let code = try activeRoomCode()
            let settings = try RuntimeSettingsProvider.make(
                concurrentTransfers: concurrentTransfers,
                language: language,
                serverURL: serverURL,
                relayURL: relayURL,
                configChunkSize: configChunkSize,
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
            let prepared = try prepareOutputDir()
            let settings = try RuntimeSettingsProvider.make(
                concurrentTransfers: concurrentTransfers,
                language: language,
                serverURL: serverURL,
                relayURL: relayURL,
                configChunkSize: configChunkSize,
                speedLimit: speedLimit
            )
            viewModel.startReceivingWithInvite(
                outputDir: prepared.url.path,
                settings: settings,
                destinationAccess: prepared.access
            )
        } catch {
            viewModel.handleFailed(error.localizedDescription)
        }
    }

    private func prepareOutputDir() throws -> (url: URL, access: AnyObject?) {
        guard let url = outputDir else {
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
        let access: AnyObject? = nil
        #endif
        try FileManager.default.createDirectory(at: url, withIntermediateDirectories: true)
        return (url, access)
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
