#if os(iOS)
import SwiftUI
import UIKit
import UniformTypeIdentifiers

struct FolderPickerSheet: UIViewControllerRepresentable {
    let initialDirectoryURL: URL?
    let onPick: (URL) -> Void
    let onCancel: () -> Void

    init(
        initialDirectoryURL: URL? = nil,
        onPick: @escaping (URL) -> Void,
        onCancel: @escaping () -> Void
    ) {
        self.initialDirectoryURL = initialDirectoryURL
        self.onPick = onPick
        self.onCancel = onCancel
    }

    func makeCoordinator() -> Coordinator {
        Coordinator(parent: self)
    }

    func makeUIViewController(context: Context) -> UIDocumentPickerViewController {
        let controller = UIDocumentPickerViewController(forOpeningContentTypes: [.folder], asCopy: false)
        controller.allowsMultipleSelection = false
        controller.directoryURL = initialDirectoryURL
        controller.delegate = context.coordinator
        return controller
    }

    func updateUIViewController(_ uiViewController: UIDocumentPickerViewController, context: Context) {}

    final class Coordinator: NSObject, UIDocumentPickerDelegate {
        private let parent: FolderPickerSheet

        init(parent: FolderPickerSheet) {
            self.parent = parent
        }

        func documentPicker(_ controller: UIDocumentPickerViewController, didPickDocumentsAt urls: [URL]) {
            guard let url = urls.first else {
                parent.onCancel()
                return
            }
            parent.onPick(url)
        }

        func documentPickerWasCancelled(_ controller: UIDocumentPickerViewController) {
            parent.onCancel()
        }
    }
}

final class SecurityScopedResourceAccess {
    let url: URL
    private let didStart: Bool

    init(url: URL) {
        self.url = url
        self.didStart = url.startAccessingSecurityScopedResource()
    }

    var isActive: Bool { didStart }

    deinit {
        if didStart {
            url.stopAccessingSecurityScopedResource()
        }
    }
}

func makeSecurityScopedFolderBookmark(for url: URL) throws -> Data {
    let didStart = url.startAccessingSecurityScopedResource()
    defer {
        if didStart {
            url.stopAccessingSecurityScopedResource()
        }
    }
    // `.withSecurityScope` is macOS-only; iOS document-picker bookmarks use the
    // default options and access is re-opened on the resolved URL.
    return try url.bookmarkData(
        options: [],
        includingResourceValuesForKeys: nil,
        relativeTo: nil
    )
}

func resolveSecurityScopedFolderBookmark(_ data: Data) throws -> URL {
    var isStale = false
    let url = try URL(
        resolvingBookmarkData: data,
        options: [],
        relativeTo: nil,
        bookmarkDataIsStale: &isStale
    )
    if isStale {
        throw RuntimeSettingsError("The selected save folder permission expired. Choose the folder again.")
    }
    return url
}
#endif
