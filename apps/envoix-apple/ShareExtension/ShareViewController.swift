import OSLog
import UIKit
import UniformTypeIdentifiers

final class ShareViewController: UIViewController {
    private enum ImportError: LocalizedError {
        case itemCountExceeded
        case livePhotoUnsupported
        case folderUnsupported
        case unsupportedItem

        var errorDescription: String? {
            switch self {
            case .itemCountExceeded:
                return localized(
                    "Select between 1 and \(ShareDraftStore.maxItemCount) items.",
                    "请选择 1 到 \(ShareDraftStore.maxItemCount) 个项目。"
                )
            case .livePhotoUnsupported:
                return localized(
                    "Paired Live Photos are not supported yet. Share a still image or video instead.",
                    "暂不支持成对的 Live Photo，请改为分享静态照片或视频。"
                )
            case .folderUnsupported:
                return localized(
                    "Open folders from the Envoix app. The Share Extension accepts files and Photos.",
                    "请从 Envoix App 内选择文件夹；分享扩展支持文件和照片。"
                )
            case .unsupportedItem:
                return localized(
                    "Envoix could not read this item as a file.",
                    "Envoix 无法将此项目读取为文件。"
                )
            }
        }
    }

    private struct ProviderImport {
        let provider: NSItemProvider
        let selection: ShareProviderSelection
    }

    private static let logger = Logger(
        subsystem: Bundle.main.bundleIdentifier ?? "com.envoix.app.ios.share",
        category: "ShareExtension"
    )

    private let iconView = UIImageView(image: UIImage(systemName: "paperplane.circle.fill"))
    private let titleLabel = UILabel()
    private let detailLabel = UILabel()
    private let activityIndicator = UIActivityIndicatorView(style: .medium)
    private let primaryButton = UIButton(type: .system)
    private let secondaryButton = UIButton(type: .system)
    private let importGate = ShareDraftImportGate()
    private var loadProgress: Progress?
    private var stagedDraft: ShareDraft?
    private var stagingSession: ShareDraftStagingSession?
    private var isFinishing = false
    private var preservesStagedDraftOnClose = false

    override func viewDidLoad() {
        super.viewDidLoad()
        configureView()
        beginImport()
    }

    private func configureView() {
        view.backgroundColor = .systemBackground
        preferredContentSize = CGSize(width: 360, height: 300)

        iconView.tintColor = .systemIndigo
        iconView.contentMode = .scaleAspectFit
        iconView.translatesAutoresizingMaskIntoConstraints = false

        titleLabel.font = .preferredFont(forTextStyle: .title2)
        titleLabel.adjustsFontForContentSizeCategory = true
        titleLabel.textAlignment = .center
        titleLabel.numberOfLines = 0
        titleLabel.accessibilityIdentifier = "share_status_title"

        detailLabel.font = .preferredFont(forTextStyle: .body)
        detailLabel.adjustsFontForContentSizeCategory = true
        detailLabel.textColor = .secondaryLabel
        detailLabel.textAlignment = .center
        detailLabel.numberOfLines = 0

        activityIndicator.hidesWhenStopped = true

        var primaryConfiguration = UIButton.Configuration.filled()
        primaryConfiguration.cornerStyle = .large
        primaryConfiguration.baseBackgroundColor = .systemIndigo
        primaryButton.configuration = primaryConfiguration
        primaryButton.addTarget(self, action: #selector(primaryAction), for: .touchUpInside)
        primaryButton.isHidden = true
        primaryButton.accessibilityIdentifier = "share_primary_action"

        var secondaryConfiguration = UIButton.Configuration.plain()
        secondaryConfiguration.cornerStyle = .large
        secondaryButton.configuration = secondaryConfiguration
        secondaryButton.setTitle(localized("Cancel", "取消"), for: .normal)
        secondaryButton.addTarget(self, action: #selector(cancelAction), for: .touchUpInside)

        let stack = UIStackView(arrangedSubviews: [
            iconView,
            titleLabel,
            detailLabel,
            activityIndicator,
            primaryButton,
            secondaryButton,
        ])
        stack.axis = .vertical
        stack.alignment = .fill
        stack.spacing = 14
        stack.translatesAutoresizingMaskIntoConstraints = false

        let scrollView = UIScrollView()
        scrollView.alwaysBounceVertical = false
        scrollView.translatesAutoresizingMaskIntoConstraints = false
        let contentView = UIView()
        contentView.translatesAutoresizingMaskIntoConstraints = false
        view.addSubview(scrollView)
        scrollView.addSubview(contentView)
        contentView.addSubview(stack)

        NSLayoutConstraint.activate([
            scrollView.leadingAnchor.constraint(equalTo: view.safeAreaLayoutGuide.leadingAnchor),
            scrollView.trailingAnchor.constraint(equalTo: view.safeAreaLayoutGuide.trailingAnchor),
            scrollView.topAnchor.constraint(equalTo: view.safeAreaLayoutGuide.topAnchor),
            scrollView.bottomAnchor.constraint(equalTo: view.safeAreaLayoutGuide.bottomAnchor),
            contentView.leadingAnchor.constraint(equalTo: scrollView.contentLayoutGuide.leadingAnchor),
            contentView.trailingAnchor.constraint(equalTo: scrollView.contentLayoutGuide.trailingAnchor),
            contentView.topAnchor.constraint(equalTo: scrollView.contentLayoutGuide.topAnchor),
            contentView.bottomAnchor.constraint(equalTo: scrollView.contentLayoutGuide.bottomAnchor),
            contentView.widthAnchor.constraint(equalTo: scrollView.frameLayoutGuide.widthAnchor),
            contentView.heightAnchor.constraint(greaterThanOrEqualTo: scrollView.frameLayoutGuide.heightAnchor),
            iconView.heightAnchor.constraint(equalToConstant: 52),
            stack.leadingAnchor.constraint(equalTo: contentView.layoutMarginsGuide.leadingAnchor, constant: 12),
            stack.trailingAnchor.constraint(equalTo: contentView.layoutMarginsGuide.trailingAnchor, constant: -12),
            stack.topAnchor.constraint(greaterThanOrEqualTo: contentView.topAnchor, constant: 24),
            stack.bottomAnchor.constraint(lessThanOrEqualTo: contentView.bottomAnchor, constant: -24),
            stack.centerYAnchor.constraint(equalTo: contentView.centerYAnchor),
            primaryButton.heightAnchor.constraint(greaterThanOrEqualToConstant: 48),
            secondaryButton.heightAnchor.constraint(greaterThanOrEqualToConstant: 44),
        ])

        showLoading(
            title: localized("Preparing to send", "正在准备发送"),
            detail: localized("Reading the selected items…", "正在读取所选项目…")
        )
    }

    private func beginImport() {
        let providers = (extensionContext?.inputItems as? [NSExtensionItem] ?? [])
            .flatMap { $0.attachments ?? [] }
        guard !providers.isEmpty, providers.count <= ShareDraftStore.maxItemCount else {
            showFailure(ImportError.itemCountExceeded)
            return
        }

        do {
            let imports = try providers.map {
                ProviderImport(provider: $0, selection: try selection(for: $0))
            }
            let session = try ShareDraftStore.live().beginStaging(
                expectedItemCount: imports.count
            )
            stagingSession = session
            loadProvider(imports, at: 0, stagingSession: session)
        } catch {
            showFailure(error)
        }
    }

    private func loadProvider(
        _ imports: [ProviderImport],
        at index: Int,
        stagingSession: ShareDraftStagingSession
    ) {
        guard !isFinishing else {
            stagingSession.cancel()
            return
        }
        guard imports.indices.contains(index) else {
            finishImport(.failure(ImportError.unsupportedItem))
            return
        }
        let providerImport = imports[index]
        showLoading(
            title: localized("Preparing to send", "正在准备发送"),
            detail: localized(
                "Copying item \(index + 1) of \(imports.count) securely into Envoix…",
                "正在将第 \(index + 1)/\(imports.count) 个项目安全暂存到 Envoix…"
            )
        )
        if providerImport.selection.loadsFileURL {
            loadProgress = nil
            providerImport.provider.loadItem(
                forTypeIdentifier: providerImport.selection.typeIdentifier,
                options: nil
            ) { [weak self] item, error in
                guard let self else { return }
                guard !isFinishing else {
                    stagingSession.cancel()
                    return
                }
                if let error {
                    failProviderLoad(error, stagingSession: stagingSession)
                    return
                }
                guard let sourceURL = sharedFileURL(fromProviderItem: item) else {
                    failProviderLoad(ImportError.unsupportedItem, stagingSession: stagingSession)
                    return
                }
                let didAccess = sourceURL.startAccessingSecurityScopedResource()
                defer {
                    if didAccess {
                        sourceURL.stopAccessingSecurityScopedResource()
                    }
                }
                stageProviderSource(
                    imports,
                    at: index,
                    sourceURL: sourceURL,
                    contentTypeIdentifier: contentTypeIdentifier(for: sourceURL),
                    preferredFileName: sourceURL.lastPathComponent,
                    stagingSession: stagingSession
                )
            }
            return
        }

        loadProgress = providerImport.provider.loadFileRepresentation(
            forTypeIdentifier: providerImport.selection.typeIdentifier
        ) { [weak self] temporaryURL, error in
            guard let self else { return }
            if let error {
                failProviderLoad(error, stagingSession: stagingSession)
                return
            }
            guard let temporaryURL else {
                failProviderLoad(ImportError.unsupportedItem, stagingSession: stagingSession)
                return
            }

            stageProviderSource(
                imports,
                at: index,
                sourceURL: temporaryURL,
                contentTypeIdentifier: providerImport.selection.typeIdentifier,
                preferredFileName: preferredFileName(
                    providerName: providerImport.provider.suggestedName,
                    temporaryURL: temporaryURL
                ),
                stagingSession: stagingSession
            )
        }
    }

    private func stageProviderSource(
        _ imports: [ProviderImport],
        at index: Int,
        sourceURL: URL,
        contentTypeIdentifier: String,
        preferredFileName: String,
        stagingSession: ShareDraftStagingSession
    ) {
        do {
            try stagingSession.append(ShareDraftStagingItem(
                sourceURL: sourceURL,
                contentTypeIdentifier: contentTypeIdentifier,
                mediaKind: imports[index].selection.mediaKind,
                preferredFileName: preferredFileName
            ))
            if imports.indices.contains(index + 1) {
                DispatchQueue.main.async {
                    self.loadProvider(
                        imports,
                        at: index + 1,
                        stagingSession: stagingSession
                    )
                }
            } else {
                let result = finalize(stagingSession)
                DispatchQueue.main.async { self.finishImport(result) }
            }
        } catch {
            failProviderLoad(error, stagingSession: stagingSession)
        }
    }

    private func failProviderLoad(
        _ error: Error,
        stagingSession: ShareDraftStagingSession
    ) {
        stagingSession.cancel()
        DispatchQueue.main.async { self.finishImport(.failure(error)) }
    }

    private func contentTypeIdentifier(for fileURL: URL) -> String {
        guard !fileURL.pathExtension.isEmpty else { return UTType.data.identifier }
        return UTType(filenameExtension: fileURL.pathExtension)?.identifier ?? UTType.data.identifier
    }

    private func finalize(
        _ stagingSession: ShareDraftStagingSession
    ) -> Result<ShareDraft, Error> {
        do {
            let store = try ShareDraftStore.live()
            let draft = try stagingSession.finalize()
            guard importGate.accept(draft.descriptor.id) else {
                try? store.discard(id: draft.descriptor.id)
                return .failure(CancellationError())
            }
            return .success(draft)
        } catch {
            return .failure(error)
        }
    }

    private func selection(for provider: NSItemProvider) throws -> ShareProviderSelection {
        do {
            return try shareProviderSelection(for: provider)
        } catch ShareProviderSelectionError.livePhotoUnsupported {
            throw ImportError.livePhotoUnsupported
        } catch ShareProviderSelectionError.folderUnsupported {
            throw ImportError.folderUnsupported
        } catch {
            throw ImportError.unsupportedItem
        }
    }

    private func finishImport(_ result: Result<ShareDraft, Error>) {
        loadProgress = nil
        stagingSession = nil
        guard !isFinishing else {
            if case let .success(draft) = result {
                try? ShareDraftStore.live().discard(id: draft.descriptor.id)
            }
            return
        }
        switch result {
        case let .success(draft):
            stagedDraft = draft
            Self.logger.info("Staged share draft id=\(draft.descriptor.id.uuidString, privacy: .public) items=\(draft.descriptor.items.count) bytes=\(draft.descriptor.byteCount)")
            showReadyForContainingApp()
        case let .failure(error):
            Self.logger.error("Share staging failed: \(error.localizedDescription, privacy: .public)")
            showFailure(error)
        }
    }

    private func showReadyForContainingApp() {
        preservesStagedDraftOnClose = true
        activityIndicator.stopAnimating()
        titleLabel.text = localized("Ready in Envoix", "已在 Envoix 中准备好")
        detailLabel.text = localized(
            "Tap Done, then return to Envoix. If a Room is connected, this selection will be ready there.",
            "点击“完成”，然后返回 Envoix。如果房间仍已连接，所选项目会直接在该房间中准备好。"
        )
        primaryButton.setTitle(localized("Done", "完成"), for: .normal)
        primaryButton.isHidden = false
        secondaryButton.isHidden = true
    }

    private func showLoading(title: String, detail: String) {
        titleLabel.text = title
        detailLabel.text = detail
        primaryButton.isHidden = true
        secondaryButton.isHidden = false
        secondaryButton.setTitle(localized("Cancel", "取消"), for: .normal)
        activityIndicator.startAnimating()
    }

    private func showFailure(_ error: Error) {
        activityIndicator.stopAnimating()
        titleLabel.text = localized("Couldn’t prepare this selection", "无法准备所选项目")
        detailLabel.text = localizedError(error)
        primaryButton.isHidden = true
        secondaryButton.isHidden = false
        secondaryButton.setTitle(localized("Close", "关闭"), for: .normal)
    }

    @objc private func primaryAction() {
        guard stagedDraft != nil else { return }
        isFinishing = true
        extensionContext?.completeRequest(returningItems: nil)
    }

    @objc private func cancelAction() {
        isFinishing = true
        let draftID = importGate.cancel() ?? stagedDraft?.descriptor.id
        loadProgress?.cancel()
        stagingSession?.cancel()
        stagingSession = nil
        if !preservesStagedDraftOnClose {
            if let draftID {
                try? ShareDraftStore.live().discard(id: draftID)
            }
        }
        extensionContext?.completeRequest(returningItems: nil)
    }

    private func localizedError(_ error: Error) -> String {
        if let error = error as? ShareDraftStoreError {
            switch error {
            case .itemCountExceeded:
                return ImportError.itemCountExceeded.localizedDescription
            case .sourceIsNotRegularFile:
                return ImportError.folderUnsupported.localizedDescription
            case .sourceIsUnreadable:
                return localized(
                    "Wait for this item to finish downloading, then share it again.",
                    "请等待该项目下载完成，然后重新分享。"
                )
            case let .insufficientStorage(requiredBytes, availableBytes):
                let required = ByteCountFormatter.string(
                    fromByteCount: Int64(clamping: requiredBytes),
                    countStyle: .file
                )
                let available = availableBytes.map {
                    ByteCountFormatter.string(
                        fromByteCount: Int64(clamping: $0),
                        countStyle: .file
                    )
                }
                return localized(
                    available.map { "Envoix needs \(required), but only \($0) is available." }
                        ?? "There is not enough free storage to stage this item.",
                    available.map { "Envoix 需要 \(required) 临时空间，但目前只有 \($0) 可用。" }
                        ?? "设备没有足够的可用空间来暂存此项目。"
                )
            case .appGroupUnavailable, .applicationSupportUnavailable,
                 .invalidDraft, .draftNotFound:
                return localized(
                    "Envoix could not save the shared item. Open the app once, then try again.",
                    "Envoix 无法保存此项目。请先打开一次 App，然后重试。"
                )
            }
        }
        return error.localizedDescription
    }

}

private func preferredFileName(providerName: String?, temporaryURL: URL) -> String {
    let sourceExtension = temporaryURL.pathExtension
    let providerName = (providerName ?? "").trimmingCharacters(in: .whitespacesAndNewlines)
    guard !providerName.isEmpty else { return temporaryURL.lastPathComponent }
    guard (providerName as NSString).pathExtension.isEmpty, !sourceExtension.isEmpty else {
        return providerName
    }
    return "\(providerName).\(sourceExtension)"
}

private func localized(_ english: String, _ chinese: String) -> String {
    Locale.current.language.languageCode?.identifier == "zh" ? chinese : english
}
