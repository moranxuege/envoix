#if os(iOS)
import Foundation
import PhotosUI
import SwiftUI
import UniformTypeIdentifiers

struct PhotoPickerSheet: UIViewControllerRepresentable {
    let onPick: ([NSItemProvider]) -> Void
    let onCancel: () -> Void

    func makeCoordinator() -> Coordinator {
        Coordinator(parent: self)
    }

    func makeUIViewController(context: Context) -> PHPickerViewController {
        var configuration = PHPickerConfiguration(photoLibrary: .shared())
        configuration.filter = .any(of: [.images, .videos])
        configuration.selectionLimit = 0
        configuration.selection = .ordered
        configuration.preferredAssetRepresentationMode = .current
        let controller = PHPickerViewController(configuration: configuration)
        controller.delegate = context.coordinator
        return controller
    }

    func updateUIViewController(_ uiViewController: PHPickerViewController, context: Context) {}

    final class Coordinator: NSObject, PHPickerViewControllerDelegate {
        private let parent: PhotoPickerSheet

        init(parent: PhotoPickerSheet) {
            self.parent = parent
        }

        func picker(_ picker: PHPickerViewController, didFinishPicking results: [PHPickerResult]) {
            guard !results.isEmpty else {
                parent.onCancel()
                return
            }
            parent.onPick(results.map(\.itemProvider))
        }
    }
}

final class PhotoDraftImporter {
    struct ImportedDraft {
        let draft: ShareDraft
        let store: ShareDraftStore
    }

    private struct ProviderImport {
        let provider: NSItemProvider
        let selection: ShareProviderSelection
    }

    private let store: ShareDraftStore
    private var imports: [ProviderImport] = []
    private var session: ShareDraftStagingSession?
    private var providerProgress: Progress?
    private var runID: UUID?
    private var progressHandler: ((Int, Int) -> Void)?
    private var completion: ((Result<ImportedDraft, Error>) -> Void)?

    var isRunning: Bool { runID != nil }

    init(store: ShareDraftStore) {
        self.store = store
    }

    func start(
        providers: [NSItemProvider],
        onProgress: @escaping (Int, Int) -> Void,
        completion: @escaping (Result<ImportedDraft, Error>) -> Void
    ) throws {
        guard !isRunning else { throw CancellationError() }
        guard !providers.isEmpty, providers.count <= ShareDraftStore.maxItemCount else {
            throw ShareDraftStoreError.itemCountExceeded(limit: ShareDraftStore.maxItemCount)
        }

        let imports = try providers.map { provider in
            let selection = try shareProviderSelection(for: provider)
            guard selection.mediaKind != .file else {
                throw ShareProviderSelectionError.unsupportedItem
            }
            return ProviderImport(provider: provider, selection: selection)
        }
        let session = try store.beginStaging(expectedItemCount: imports.count)
        let runID = UUID()
        self.imports = imports
        self.session = session
        self.runID = runID
        progressHandler = onProgress
        self.completion = completion
        loadProvider(at: 0, runID: runID, session: session)
    }

    func cancel() {
        guard isRunning else { return }
        providerProgress?.cancel()
        session?.cancel()
        clearState()
    }

    private func loadProvider(
        at index: Int,
        runID: UUID,
        session: ShareDraftStagingSession
    ) {
        guard self.runID == runID, imports.indices.contains(index) else {
            session.cancel()
            return
        }
        progressHandler?(index + 1, imports.count)
        let item = imports[index]
        providerProgress = item.provider.loadFileRepresentation(
            forTypeIdentifier: item.selection.typeIdentifier
        ) { [weak self] temporaryURL, error in
            let result: Result<Void, Error>
            if let temporaryURL, error == nil {
                do {
                    try session.append(ShareDraftStagingItem(
                        sourceURL: temporaryURL,
                        contentTypeIdentifier: item.selection.typeIdentifier,
                        mediaKind: item.selection.mediaKind,
                        preferredFileName: Self.preferredFileName(
                            provider: item.provider,
                            selection: item.selection,
                            temporaryURL: temporaryURL
                        )
                    ))
                    result = .success(())
                } catch {
                    result = .failure(error)
                }
            } else {
                result = .failure(error ?? ShareProviderSelectionError.unsupportedItem)
            }

            DispatchQueue.main.async { [weak self] in
                guard let self else {
                    session.cancel()
                    return
                }
                self.handleProviderResult(
                    result,
                    at: index,
                    runID: runID,
                    session: session
                )
            }
        }
    }

    private func handleProviderResult(
        _ result: Result<Void, Error>,
        at index: Int,
        runID: UUID,
        session: ShareDraftStagingSession
    ) {
        guard self.runID == runID else {
            session.cancel()
            return
        }
        switch result {
        case .failure(let error):
            fail(error, session: session)
        case .success:
            if imports.indices.contains(index + 1) {
                loadProvider(at: index + 1, runID: runID, session: session)
            } else {
                finalize(session: session)
            }
        }
    }

    private func finalize(session: ShareDraftStagingSession) {
        do {
            let draft = try session.finalize()
            let completion = completion
            clearState()
            completion?(.success(ImportedDraft(draft: draft, store: store)))
        } catch {
            fail(error, session: session)
        }
    }

    private func fail(_ error: Error, session: ShareDraftStagingSession) {
        session.cancel()
        try? store.discard(id: session.id)
        let completion = completion
        clearState()
        completion?(.failure(error))
    }

    private func clearState() {
        providerProgress = nil
        imports = []
        session = nil
        runID = nil
        progressHandler = nil
        completion = nil
    }

    private static func preferredFileName(
        provider: NSItemProvider,
        selection: ShareProviderSelection,
        temporaryURL: URL
    ) -> String {
        let suggested = provider.suggestedName?.trimmingCharacters(in: .whitespacesAndNewlines)
        let baseName = suggested.flatMap { $0.isEmpty ? nil : $0 } ?? "Photo"
        guard URL(fileURLWithPath: baseName).pathExtension.isEmpty else { return baseName }
        let temporaryExtension = temporaryURL.pathExtension.trimmingCharacters(
            in: .whitespacesAndNewlines
        )
        let fileExtension = temporaryExtension.isEmpty
            ? UTType(selection.typeIdentifier)?.preferredFilenameExtension
            : temporaryExtension
        guard let fileExtension, !fileExtension.isEmpty else { return baseName }
        return "\(baseName).\(fileExtension)"
    }
}
#endif
