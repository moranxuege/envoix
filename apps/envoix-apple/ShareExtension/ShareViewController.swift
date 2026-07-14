import OSLog
import UIKit
import UniformTypeIdentifiers

final class ShareViewController: UIViewController {
    private enum ImportError: LocalizedError {
        case selectOneItem
        case livePhotoUnsupported
        case folderUnsupported
        case unsupportedItem

        var errorDescription: String? {
            switch self {
            case .selectOneItem:
                return localized(
                    "Select one item. Multiple files are coming with Manifest support.",
                    "请选择一个项目；多文件将在 Manifest 支持后开放。"
                )
            case .livePhotoUnsupported:
                return localized(
                    "Paired Live Photos are not supported yet. Share a still image or video instead.",
                    "暂不支持成对的 Live Photo，请改为分享静态照片或视频。"
                )
            case .folderUnsupported:
                return localized(
                    "Folders are coming with Manifest support. Choose one file for now.",
                    "文件夹将在 Manifest 支持后开放，目前请选择一个文件。"
                )
            case .unsupportedItem:
                return localized(
                    "Envoix could not read this item as a file.",
                    "Envoix 无法将此项目读取为文件。"
                )
            }
        }
    }

    private struct ProviderSelection {
        let typeIdentifier: String
        let mediaKind: ShareDraftDescriptor.MediaKind
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
            detail: localized("Copying one item securely into Envoix…", "正在将一个项目安全暂存到 Envoix…")
        )
    }

    private func beginImport() {
        let providers = (extensionContext?.inputItems as? [NSExtensionItem] ?? [])
            .flatMap { $0.attachments ?? [] }
        guard providers.count == 1, let provider = providers.first else {
            showFailure(ImportError.selectOneItem)
            return
        }

        do {
            let selection = try selection(for: provider)
            loadProgress = provider.loadFileRepresentation(
                forTypeIdentifier: selection.typeIdentifier
            ) { [weak self] temporaryURL, error in
                guard let self else { return }
                let result: Result<ShareDraft, Error>
                if let error {
                    result = .failure(error)
                } else if let temporaryURL {
                    do {
                        let store = try ShareDraftStore.live()
                        let draft = try store.stage(
                            sourceURL: temporaryURL,
                            contentTypeIdentifier: selection.typeIdentifier,
                            mediaKind: selection.mediaKind,
                            preferredFileName: preferredFileName(
                                providerName: provider.suggestedName,
                                temporaryURL: temporaryURL
                            )
                        )
                        if self.importGate.accept(draft.descriptor.id) {
                            result = .success(draft)
                        } else {
                            try? store.discard(id: draft.descriptor.id)
                            result = .failure(CancellationError())
                        }
                    } catch {
                        result = .failure(error)
                    }
                } else {
                    result = .failure(ImportError.unsupportedItem)
                }
                DispatchQueue.main.async { self.finishImport(result) }
            }
        } catch {
            showFailure(error)
        }
    }

    private func selection(for provider: NSItemProvider) throws -> ProviderSelection {
        let identifiers = provider.registeredTypeIdentifiers
        let types = identifiers.compactMap { identifier in
            UTType(identifier).map { (identifier, $0) }
        }
        if types.contains(where: { $0.1.conforms(to: .livePhoto) }) {
            throw ImportError.livePhotoUnsupported
        }
        if types.contains(where: { $0.1.conforms(to: .directory) }) {
            throw ImportError.folderUnsupported
        }
        if let movie = types.first(where: { $0.1.conforms(to: .movie) }) {
            return ProviderSelection(typeIdentifier: movie.0, mediaKind: .video)
        }
        if let image = types.first(where: { $0.1.conforms(to: .image) }) {
            return ProviderSelection(typeIdentifier: image.0, mediaKind: .image)
        }
        if let file = types.first(where: {
            $0.1.conforms(to: .data) && !$0.1.conforms(to: .text)
        }) ?? types.first(where: { $0.1.conforms(to: .item) && $0.1 != .url }) {
            return ProviderSelection(typeIdentifier: file.0, mediaKind: .file)
        }
        throw ImportError.unsupportedItem
    }

    private func finishImport(_ result: Result<ShareDraft, Error>) {
        loadProgress = nil
        guard !isFinishing else {
            if case let .success(draft) = result {
                try? ShareDraftStore.live().discard(id: draft.descriptor.id)
            }
            return
        }
        switch result {
        case let .success(draft):
            stagedDraft = draft
            Self.logger.info("Staged one share draft id=\(draft.descriptor.id.uuidString, privacy: .public) bytes=\(draft.descriptor.byteCount)")
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
            "Tap Done, then open Envoix to choose the receiving device and send.",
            "点击“完成”，然后打开 Envoix 选择接收设备并发送。"
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
        titleLabel.text = localized("Couldn’t prepare this item", "无法准备此项目")
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
        loadProgress?.cancel()
        if !preservesStagedDraftOnClose {
            let draftID = importGate.cancel() ?? stagedDraft?.descriptor.id
            if let draftID {
                try? ShareDraftStore.live().discard(id: draftID)
            }
        }
        extensionContext?.completeRequest(returningItems: nil)
    }

    private func localizedError(_ error: Error) -> String {
        if let error = error as? ShareDraftStoreError {
            switch error {
            case .sourceIsNotRegularFile:
                return ImportError.folderUnsupported.localizedDescription
            case .sourceIsUnreadable:
                return localized(
                    "Wait for this item to finish downloading, then share it again.",
                    "请等待该项目下载完成，然后重新分享。"
                )
            case let .quotaExceeded(limitBytes):
                let limit = ByteCountFormatter.string(fromByteCount: Int64(limitBytes), countStyle: .file)
                return localized(
                    "This item exceeds Envoix's \(limit) temporary sharing limit.",
                    "此项目超过 Envoix 的 \(limit) 临时分享上限。"
                )
            case .appGroupUnavailable, .invalidDraft, .draftNotFound:
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
