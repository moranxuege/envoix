import SwiftUI
#if os(macOS)
import AppKit
#endif
import UniformTypeIdentifiers
import EnvoixCore

private final class SelectedResourceAccessGroup {
    private let resources: [AnyObject]

    init(_ resources: [AnyObject]) {
        self.resources = resources
    }
}

func sendSelectionRequiresManifest(_ urls: [URL]) -> Bool {
    guard urls.count == 1, let url = urls.first else { return urls.count > 1 }
    let values = try? url.resourceValues(forKeys: [.isDirectoryKey])
    return values?.isDirectory == true
}

struct SendView: View {
    #if os(iOS)
    private static let mobileScrollBottomClearance: CGFloat = 32
    #endif

    @Environment(\.appLanguage) private var uiLanguage
    @EnvironmentObject private var model: AppModel
    @ObservedObject var viewModel: TransferViewModel
    @State private var selectedItems: [URL]
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
    @State private var selectedSourceAccess: AnyObject?
    @State private var selectedPendingSelectionID: UUID?
    #if os(iOS)
    @State private var isFolderPickerPresented = false
    @State private var isPhotoPickerPresented = false
    @State private var photoImporter: PhotoDraftImporter?
    @State private var photoImportItemNumber = 0
    @State private var photoImportItemCount = 0
    #endif

    init(
        viewModel: TransferViewModel,
        initialMode: PairingMode = .room,
        initialFiles: [URL] = [],
        initialFileAccess: AnyObject? = nil,
        initialPendingSelectionID: UUID? = nil
    ) {
        self.viewModel = viewModel
        _mode = State(initialValue: initialMode)
        _selectedItems = State(initialValue: initialFiles)
        _filePathInput = State(initialValue: initialFiles.count == 1 ? initialFiles[0].path : "")
        _selectedSourceAccess = State(initialValue: initialFileAccess)
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
        .sheet(isPresented: $isFileImporterPresented) {
            FilePickerSheet(
                initialDirectoryURL: filePickerInitialDirectoryURL,
                onPick: { urls in
                    isFileImporterPresented = false
                    handleImportedItems(.success(urls))
                },
                onCancel: { isFileImporterPresented = false }
            )
        }
        .sheet(isPresented: $isFolderPickerPresented) {
            FolderPickerSheet(
                initialDirectoryURL: folderPickerInitialDirectoryURL,
                onPick: { url in
                    isFolderPickerPresented = false
                    handleImportedFolder(url)
                },
                onCancel: { isFolderPickerPresented = false }
            )
        }
        .sheet(isPresented: $isPhotoPickerPresented) {
            PhotoPickerSheet(
                onPick: { providers in
                    isPhotoPickerPresented = false
                    beginPhotoImport(providers)
                },
                onCancel: { isPhotoPickerPresented = false }
            )
        }
        .onAppear(perform: adoptSharedSelectionIfAvailable)
        .onChange(of: model.pendingSendSelection?.id) { _ in
            adoptSharedSelectionIfAvailable()
        }
        .onDisappear(perform: cancelPhotoImport)
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
                #if os(macOS)
                TransferStatusView(viewModel: viewModel)
                #endif
            }
            .padding(.vertical, 12)
            #if os(iOS)
            .padding(.bottom, Self.mobileScrollBottomClearance)
            #endif
        }
        .accessibilityIdentifier("send_content_scroll")
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
                title: AppText.value("Or enter a Room code", "或输入配对码", language: uiLanguage),
                placeholder: AppText.value("Enter Room code", "输入配对码", language: uiLanguage),
                showsCopyAction: false,
                pasteAction: pastePairingInput,
                helper: "",
                accessibilityIdentifier: "send_room_code_input"
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
                    "The receiver can scan this QR or enter the same Room code. You can also scan a receiver code below.",
                    "接收端可以扫码或输入同一个配对码；你也可以在下方扫描接收端的码。",
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
                placeholder: AppText.value("Scan QR or enter receiver Room code", "扫码或输入接收端配对码", language: uiLanguage),
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
        Button(action: primaryButtonAction) {
            Label(
                primaryLabel,
                systemImage: viewModel.isPreparingManifest
                    ? "xmark"
                    : (viewModel.isBusy ? "list.bullet.rectangle" : "paperplane")
            )
                .frame(maxWidth: .infinity, minHeight: 44)
                .contentShape(Rectangle())
        }
        .keyboardShortcut(.defaultAction)
        .buttonStyle(PrimaryActionButtonStyle())
        .disabled(
            !viewModel.isPreparingManifest
                && (viewModel.isBusy || viewModel.isFinalizing || isPhotoImporting || !canSend || concurrencyBlocked)
        )
        .accessibilityIdentifier("send_start_button")
    }

    #if os(iOS)
    private var bottomActionBar: some View {
        Group {
            if viewModel.isPreparingManifest || !viewModel.isBusy {
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

    private var fileSection: some View {
        VStack(alignment: .leading, spacing: 12) {
            Text(AppText.value("Items to send", "要发送的项目", language: uiLanguage))
                .font(.headline.weight(.semibold))
                .foregroundStyle(Theme.text)
            #if os(iOS)
            selectionSourceActions

            if isPhotoImporting {
                HStack(spacing: 10) {
                    ProgressView()
                        .controlSize(.small)
                    Text(AppText.value(
                        "Preparing photo \(photoImportItemNumber) of \(photoImportItemCount)…",
                        "正在准备第 \(photoImportItemNumber)/\(photoImportItemCount) 个照片项目…",
                        language: uiLanguage
                    ))
                    .font(.footnote)
                    .foregroundStyle(Theme.muted)
                    .fixedSize(horizontal: false, vertical: true)
                }
                .accessibilityIdentifier("send_photo_import_progress")
            }

            if !selectedItems.isEmpty {
                fileChooserLabel
                    .accessibilityElement(children: .combine)
                    .accessibilityIdentifier("send_selection_summary")
                    .accessibilityValue(String(selectedItems.count))
            }
            #else
            Button {
                let urls = chooseSendItems()
                if !urls.isEmpty { selectItems(urls) }
            } label: {
                fileChooserLabel
            }
            .buttonStyle(.plain)
            .disabled(viewModel.isBusy)
            .accessibilityIdentifier("send_file_picker")
            .accessibilityValue(String(selectedItems.count))
            #endif

            Text(selectionGuidance)
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
            loadDroppedItems(providers)
            return true
        }
    }

    private var selectionGuidance: String {
        #if os(iOS)
        AppText.value(
            "Choose Photos, one or more files, or one folder. In the folder picker, Open uploads the current folder.",
            "可选择照片、一个或多个文件，或一个文件夹；在文件夹选择器中点“打开”即上传当前文件夹。",
            language: uiLanguage
        )
        #else
        AppText.value(
            "Choose one or more files, or a folder. Folder structure is preserved.",
            "可选择一个或多个文件，也可选择文件夹；目录结构会完整保留。",
            language: uiLanguage
        )
        #endif
    }

    #if os(iOS)
    private var filePickerInitialDirectoryURL: URL? {
        #if DEBUG
        guard ProcessInfo.processInfo.arguments.contains("--ui-testing-file-picker") else {
            return nil
        }
        return FilePickerUITestFixture.initialDirectoryURL()
        #else
        return nil
        #endif
    }

    private var folderPickerInitialDirectoryURL: URL? {
        #if DEBUG
        guard ProcessInfo.processInfo.arguments.contains("--ui-testing-folder-picker") else {
            return nil
        }
        return FolderPickerUITestFixture.initialDirectoryURL()
        #else
        return nil
        #endif
    }

    private var selectionSourceActions: some View {
        HStack(spacing: 10) {
            selectionSourceAction(
                AppText.value("Photos", "照片", language: uiLanguage),
                systemImage: "photo.on.rectangle",
                identifier: "send_photo_picker"
            ) {
                isPhotoPickerPresented = true
            }
            selectionSourceAction(
                AppText.value("Files", "文件", language: uiLanguage),
                systemImage: "doc.badge.plus",
                identifier: "send_file_picker"
            ) {
                isFileImporterPresented = true
            }
            selectionSourceAction(
                AppText.value("Folder", "文件夹", language: uiLanguage),
                systemImage: "folder.badge.plus",
                identifier: "send_folder_picker"
            ) {
                isFolderPickerPresented = true
            }
        }
    }

    private func selectionSourceAction(
        _ title: String,
        systemImage: String,
        identifier: String,
        action: @escaping () -> Void
    ) -> some View {
        Button(action: action) {
            VStack(spacing: 8) {
                Image(systemName: systemImage)
                    .font(.title3.weight(.semibold))
                Text(title)
                    .font(.subheadline.weight(.semibold))
                    .lineLimit(1)
                    .minimumScaleFactor(0.8)
            }
            .foregroundStyle(Theme.accentStrong)
            .frame(maxWidth: .infinity, minHeight: 72)
            .background(Theme.accentSoft.opacity(0.65), in: RoundedRectangle(cornerRadius: Theme.cardRadius))
            .contentShape(RoundedRectangle(cornerRadius: Theme.cardRadius))
        }
        .buttonStyle(.plain)
        .disabled(viewModel.isBusy || viewModel.isPreparingManifest || isPhotoImporting)
        .accessibilityIdentifier(identifier)
    }
    #endif

    @ViewBuilder private var fileChooserLabel: some View {
        #if os(iOS)
        HStack(spacing: 12) {
            Image(systemName: selectionIcon)
                .font(.system(size: 28, weight: .semibold))
                .foregroundStyle(Theme.accentStrong)
                .frame(width: 38, height: 38)
                .background(Theme.accentSoft, in: RoundedRectangle(cornerRadius: Theme.cardRadius))

            VStack(alignment: .leading, spacing: 4) {
                Text(selectionTitle)
                    .font(.headline.weight(.semibold))
                    .foregroundStyle(selectedItems.isEmpty ? Theme.text : Theme.accentStrong)
                    .lineLimit(1)
                    .truncationMode(.middle)
                Text(selectionSubtitle)
                    .font(.footnote)
                    .foregroundStyle(Theme.muted)
                    .fixedSize(horizontal: false, vertical: true)
            }

            Spacer(minLength: 8)
            Image(systemName: "checkmark.circle.fill")
                .font(.body.weight(.semibold))
                .foregroundStyle(Theme.success)
        }
        .frame(maxWidth: .infinity, minHeight: 76, alignment: .leading)
        .contentShape(RoundedRectangle(cornerRadius: Theme.cardRadius))
        #else
        VStack(spacing: 10) {
            Image(systemName: selectionIcon)
                .font(.system(size: 48, weight: .semibold))
                .foregroundStyle(Theme.accentStrong)

            Text(selectionTitle)
                .font(.title2.weight(.semibold))
                .foregroundStyle(selectedItems.isEmpty ? Theme.text : Theme.accentStrong)
                .lineLimit(1)
                .truncationMode(.middle)

            Text(selectionSubtitle)
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
                let paths = selectedItems.map(\.path).joined(separator: "\n")
                copyWithToast(
                    paths,
                    AppText.value("Selected paths copied", "已复制所选路径", language: uiLanguage)
                )
            } label: {
                Label(AppText.value("Copy Selected Paths", "复制已选路径", language: uiLanguage), systemImage: "doc.on.doc")
                    .labelStyle(.iconOnly)
                    .frame(width: 28, height: 28)
                    .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .foregroundStyle(selectedItems.isEmpty ? Theme.muted : Theme.accentStrong)
            .disabled(selectedItems.isEmpty)
            .help(AppText.value("Copy selected paths", "复制所选路径", language: uiLanguage))
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
        let lowercased = input.lowercased()
        if lowercased.hasPrefix("envoix:") && !lowercased.hasPrefix("envoix://pair/") {
            invite = input
            mode = .invite
            ToastCenter.shared.show(AppText.value("Legacy invite loaded", "已载入旧版邀请", language: uiLanguage))
            return nil
        }

        do {
            let parsed = try parsePairingInvite(input: input)
            guard parsed.role != .send else {
                let message = AppText.value(
                    "Scan a receiver code or share your send code.",
                    "请扫描接收端的码，或分享你的发送码。",
                    language: uiLanguage
                )
                ToastCenter.shared.show(message)
                return message
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
            return nil
        } catch {
            let message = AppText.value(
                "This is not a valid Envoix pairing code.",
                "这不是有效的 Envoix 配对码。",
                language: uiLanguage
            )
            ToastCenter.shared.show(message)
            return message
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
        if viewModel.isPreparingManifest {
            return AppText.value("Cancel Preparation", "取消准备", language: uiLanguage)
        }
        if viewModel.isBusy { return AppText.value("Managed in Activity", "请在活动中管理", language: uiLanguage) }
        switch viewModel.phase {
        case .completed, .canceled, .failed: return AppText.value("Send Again", "再次发送", language: uiLanguage)
        default: return AppText.value("Send", "发送", language: uiLanguage)
        }
    }

    private var isPhotoImporting: Bool {
        #if os(iOS)
        photoImporter?.isRunning == true
        #else
        false
        #endif
    }

    private var canSend: Bool {
        guard !selectedItems.isEmpty else { return false }
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
        !concurrentTransfers
            && !viewModel.isBusy
            && (model.receive.isBusy || model.hasExecutingActivity)
    }

    private var invalidSelectionMessage: String {
        AppText.value(
            "Choose regular files or folders. Links and special items are not supported.",
            "请选择普通文件或文件夹；暂不支持链接和特殊项目。",
            language: uiLanguage
        )
    }

    private var selectionTitle: String {
        switch selectedItems.count {
        case 0:
            return AppText.value("Choose files or folder", "选择文件或文件夹", language: uiLanguage)
        case 1:
            return selectedItems[0].lastPathComponent
        default:
            return AppText.value(
                "\(selectedItems.count) items selected",
                "已选择 \(selectedItems.count) 个项目",
                language: uiLanguage
            )
        }
    }

    private var selectionSubtitle: String {
        switch selectedItems.count {
        case 0:
            #if os(iOS)
            return AppText.value("Tap to open Files.", "点击打开文件。", language: uiLanguage)
            #else
            return AppText.value(
                "Drop files or folders here, or click to choose.",
                "把文件或文件夹拖到这里，或点击选择。",
                language: uiLanguage
            )
            #endif
        case 1 where sendSelectionRequiresManifest(selectedItems):
            return AppText.value("Folder structure will be preserved.", "将完整保留文件夹结构。", language: uiLanguage)
        case 1:
            #if os(iOS)
            return AppText.value("Ready to send. Tap to replace.", "已准备发送，点击可替换。", language: uiLanguage)
            #else
            return AppText.value("Ready to send. Click to replace.", "已准备发送，点击可替换。", language: uiLanguage)
            #endif
        default:
            return AppText.value("These items will be sent together.", "这些项目将作为一批发送。", language: uiLanguage)
        }
    }

    private var selectionIcon: String {
        guard !selectedItems.isEmpty else {
            #if os(iOS)
            return "doc.badge.plus"
            #else
            return "square.and.arrow.up"
            #endif
        }
        if selectedItems.count > 1 { return "square.stack.3d.up.fill" }
        return sendSelectionRequiresManifest(selectedItems) ? "folder.fill" : "doc.fill"
    }

    @discardableResult
    private func selectItems(
        _ urls: [URL],
        access: AnyObject? = nil,
        pendingSelectionID: UUID? = nil
    ) -> Bool {
        guard !urls.isEmpty else { return false }
        var seenPaths = Set<String>()
        var accepted: [URL] = []
        for url in urls {
            let standardized = url.standardizedFileURL
            guard seenPaths.insert(standardized.path).inserted else { continue }
            guard let values = try? standardized.resourceValues(
                forKeys: [.isRegularFileKey, .isDirectoryKey, .isSymbolicLinkKey]
            ),
                values.isSymbolicLink != true,
                values.isRegularFile == true || values.isDirectory == true else {
                ToastCenter.shared.show(invalidSelectionMessage)
                return false
            }
            accepted.append(standardized)
        }
        guard !accepted.isEmpty else { return false }
        selectedSourceAccess = access
        selectedPendingSelectionID = pendingSelectionID
        selectedItems = accepted
        filePathInput = accepted.count == 1 ? accepted[0].path : ""
        return true
    }

    #if os(iOS)
    private func handleImportedFolder(_ url: URL) {
        do {
            guard try url.resourceValues(forKeys: [.isDirectoryKey]).isDirectory == true else {
                throw RuntimeSettingsError(AppText.value(
                    "Choose a folder, not a file.",
                    "请选择文件夹，而不是文件。",
                    language: uiLanguage
                ))
            }
            guard try adoptUserSelectedItems([url]) else { return }
            ToastCenter.shared.show(AppText.value(
                "Folder ready to upload",
                "文件夹已准备上传",
                language: uiLanguage
            ))
        } catch {
            ToastCenter.shared.show(error.localizedDescription)
        }
    }

    private func beginPhotoImport(_ providers: [NSItemProvider]) {
        guard !providers.isEmpty else { return }
        guard providers.count <= ShareDraftStore.maxItemCount else {
            ToastCenter.shared.show(AppText.value(
                "Select no more than \(ShareDraftStore.maxItemCount) Photos items.",
                "照片项目不能超过 \(ShareDraftStore.maxItemCount) 个。",
                language: uiLanguage
            ))
            return
        }

        do {
            let store = try ShareDraftStore.live()
            let importer = PhotoDraftImporter(store: store)
            photoImporter = importer
            try importer.start(
                providers: providers,
                onProgress: { itemNumber, itemCount in
                    photoImportItemNumber = itemNumber
                    photoImportItemCount = itemCount
                },
                completion: finishPhotoImport
            )
        } catch let error as ShareProviderSelectionError {
            photoImporter = nil
            ToastCenter.shared.show(photoSelectionErrorMessage(error))
        } catch {
            photoImporter = nil
            ToastCenter.shared.show(error.localizedDescription)
        }
    }

    private func finishPhotoImport(_ result: Result<PhotoDraftImporter.ImportedDraft, Error>) {
        photoImporter = nil
        photoImportItemNumber = 0
        photoImportItemCount = 0
        switch result {
        case .success(let imported):
            let draft = imported.draft
            let store = imported.store
            do {
                try store.claim(id: draft.descriptor.id)
                store.acknowledgePending(id: draft.descriptor.id)
                let lease = ShareDraftLease(id: draft.descriptor.id, store: store)
                guard selectItems(
                    draft.fileURLs,
                    access: lease,
                    pendingSelectionID: draft.descriptor.id
                ) else {
                    try? store.discard(id: draft.descriptor.id)
                    return
                }
                ToastCenter.shared.show(AppText.value(
                    draft.fileURLs.count == 1
                        ? "Photo ready to send"
                        : "\(draft.fileURLs.count) Photos items ready to send",
                    draft.fileURLs.count == 1
                        ? "照片已准备发送"
                        : "\(draft.fileURLs.count) 个照片项目已准备发送",
                    language: uiLanguage
                ))
            } catch {
                try? store.discard(id: draft.descriptor.id)
                ToastCenter.shared.show(error.localizedDescription)
            }
        case .failure(let error):
            if let selectionError = error as? ShareProviderSelectionError {
                ToastCenter.shared.show(photoSelectionErrorMessage(selectionError))
            } else {
                ToastCenter.shared.show(error.localizedDescription)
            }
        }
    }

    private func cancelPhotoImport() {
        photoImporter?.cancel()
        photoImporter = nil
        photoImportItemNumber = 0
        photoImportItemCount = 0
    }

    private func photoSelectionErrorMessage(_ error: ShareProviderSelectionError) -> String {
        switch error {
        case .livePhotoUnsupported:
            return AppText.value(
                "Paired Live Photos are not supported yet. Choose a still image or video instead.",
                "暂不支持成对的 Live Photo，请改选静态照片或视频。",
                language: uiLanguage
            )
        case .folderUnsupported, .unsupportedItem:
            return AppText.value(
                "Envoix could not read this Photos item as an image or video.",
                "Envoix 无法将这个照片项目读取为图片或视频。",
                language: uiLanguage
            )
        }
    }

    private func adoptSharedSelectionIfAvailable() {
        guard !viewModel.isBusy,
              let selection = model.pendingSendSelection else { return }
        if selection.id == selectedPendingSelectionID {
            model.consumePendingSendSelection(id: selection.id)
            return
        }
        guard selectItems(
            selection.fileURLs,
            access: selection.sourceAccess,
            pendingSelectionID: selection.id
        ) else { return }
        model.consumePendingSendSelection(id: selection.id)
        ToastCenter.shared.show(AppText.value(
            selection.fileURLs.count == 1
                ? "Shared item ready to send"
                : "\(selection.fileURLs.count) shared items ready to send",
            selection.fileURLs.count == 1
                ? "分享项目已准备发送"
                : "\(selection.fileURLs.count) 个分享项目已准备发送",
            language: uiLanguage
        ))
    }
    #endif

    private func acknowledgedSourceAccess() -> AnyObject? {
        #if os(iOS)
        (selectedSourceAccess as? ShareDraftLease)?.acknowledge()
        #endif
        return selectedSourceAccess
    }

    private func handleImportedItems(_ result: Result<[URL], Error>) {
        do {
            let urls = try result.get()
            guard !urls.isEmpty else { return }
            for url in urls {
                guard try url.resourceValues(forKeys: [.isRegularFileKey]).isRegularFile == true else {
                    throw RuntimeSettingsError(AppText.value(
                        "Use the Folder button to upload a folder.",
                        "请使用“文件夹”按钮上传文件夹。",
                        language: uiLanguage
                    ))
                }
            }
            guard try adoptUserSelectedItems(urls) else { return }
            ToastCenter.shared.show(AppText.value("Files selected", "已选择文件", language: uiLanguage))
        } catch {
            ToastCenter.shared.show(error.localizedDescription)
        }
    }

    private func adoptUserSelectedItems(_ urls: [URL]) throws -> Bool {
        #if os(iOS)
        let accesses = urls.map(SecurityScopedResourceAccess.init)
        for (url, access) in zip(urls, accesses) {
            guard access.isActive || FileManager.default.isReadableFile(atPath: url.path) else {
                throw RuntimeSettingsError(AppText.value(
                    "Envoix could not access every selected item. Choose them again from Files.",
                    "Envoix 无法访问全部所选项目。请从 Files 中重新选择。",
                    language: uiLanguage
                ))
            }
        }
        return selectItems(urls, access: SelectedResourceAccessGroup(accesses))
        #else
        return selectItems(urls)
        #endif
    }

    private func applyPathInput() {
        let raw = filePathInput.trimmed
        guard !raw.isEmpty else { return }

        let path = (raw as NSString).expandingTildeInPath
        guard FileManager.default.fileExists(atPath: path) else {
            ToastCenter.shared.show(AppText.value("Path not found", "未找到路径", language: uiLanguage))
            return
        }

        guard selectItems([URL(fileURLWithPath: path)]) else { return }
        ToastCenter.shared.show(AppText.value("Path selected", "已选择路径", language: uiLanguage))
    }

    private func loadDroppedItems(_ providers: [NSItemProvider]) {
        guard !providers.isEmpty else { return }
        let group = DispatchGroup()
        let lock = NSLock()
        var loaded = Array<URL?>(repeating: nil, count: providers.count)
        for (index, provider) in providers.enumerated() {
            group.enter()
            _ = provider.loadObject(ofClass: URL.self) { url, _ in
                lock.lock()
                loaded[index] = url
                lock.unlock()
                group.leave()
            }
        }
        group.notify(queue: .main) {
            let urls = loaded.compactMap { $0 }
            guard urls.count == providers.count else {
                ToastCenter.shared.show(invalidSelectionMessage)
                return
            }
            do {
                _ = try adoptUserSelectedItems(urls)
            } catch {
                ToastCenter.shared.show(error.localizedDescription)
            }
        }
    }

    #if os(macOS)
    private func chooseSendItems() -> [URL] {
        let panel = NSOpenPanel()
        panel.canChooseFiles = true
        panel.canChooseDirectories = true
        panel.allowsMultipleSelection = true
        return panel.runModal() == .OK ? panel.urls : []
    }
    #endif

    private func startRoomSend(code: String, settings: EnvoixRuntimeSettings) {
        let access = acknowledgedSourceAccess()
        if sendSelectionRequiresManifest(selectedItems) {
            viewModel.startSendingManifestWithRoom(
                selectedPaths: selectedItems.map(\.path),
                code: code,
                settings: settings,
                sourceAccess: access
            )
        } else if let file = selectedItems.first {
            viewModel.startSendingWithRoom(
                filePath: file.path,
                code: code,
                settings: settings,
                sourceAccess: access
            )
        }
    }

    private func startInviteSend(invite: String, settings: EnvoixRuntimeSettings) {
        let access = acknowledgedSourceAccess()
        if sendSelectionRequiresManifest(selectedItems) {
            viewModel.startSendingManifestWithInvite(
                selectedPaths: selectedItems.map(\.path),
                invite: invite,
                settings: settings,
                sourceAccess: access
            )
        } else if let file = selectedItems.first {
            viewModel.startSendingWithInvite(
                filePath: file.path,
                invite: invite,
                settings: settings,
                sourceAccess: access
            )
        }
    }

    private func startTokenSend(token: String, settings: EnvoixRuntimeSettings) {
        let access = acknowledgedSourceAccess()
        if sendSelectionRequiresManifest(selectedItems) {
            viewModel.startSendingManifestWithToken(
                selectedPaths: selectedItems.map(\.path),
                token: token,
                settings: settings,
                sourceAccess: access
            )
        } else if let file = selectedItems.first {
            viewModel.startSendingWithToken(
                filePath: file.path,
                token: token,
                settings: settings,
                sourceAccess: access
            )
        }
    }

    private func primaryButtonAction() {
        if viewModel.isPreparingManifest {
            _ = viewModel.cancelManifestPreparation()
        } else {
            primaryAction()
        }
    }

    private func primaryAction() {
        guard !viewModel.isBusy, !viewModel.isFinalizing else { return }
        guard !selectedItems.isEmpty else { return }
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
                    startRoomSend(
                        code: try activeSendRoomCode(),
                        settings: settings
                    )
                } else if lowercasedInput.hasPrefix("envoix:")
                    && !lowercasedInput.hasPrefix("envoix://pair/") {
                    invite = input
                    mode = .invite
                    startInviteSend(
                        invite: input,
                        settings: settings
                    )
                } else {
                    let parsed = try parsePairingInvite(input: input)
                    guard parsed.role != .send else {
                        throw RuntimeSettingsError(AppText.value(
                            "Scan a receiver code or share your send code.",
                            "请扫描接收端的码，或分享你的发送码。",
                            language: uiLanguage
                        ))
                    }
                    roomCode = parsed.code
                    startRoomSend(
                        code: parsed.code,
                        settings: try runtimeSettings(for: parsed)
                    )
                }
            case .invite:
                if invite.trimmed.lowercased().hasPrefix("envoix://pair/") {
                    let parsed = try parsePairingInvite(input: invite.trimmed)
                    guard parsed.role != .send else {
                        throw RuntimeSettingsError(AppText.value(
                            "Scan a receiver code or share your send code.",
                            "请扫描接收端的码，或分享你的发送码。",
                            language: uiLanguage
                        ))
                    }
                    mode = .room
                    roomCode = parsed.code
                    let roomSettings = try runtimeSettings(for: parsed)
                    startRoomSend(
                        code: parsed.code,
                        settings: roomSettings
                    )
                } else {
                    startInviteSend(
                        invite: invite.trimmed,
                        settings: settings
                    )
                }
            case .token:
                startTokenSend(
                    token: token.trimmed,
                    settings: settings
                )
            }
        } catch {
            viewModel.handleFailed(error.localizedDescription)
        }
    }
}
