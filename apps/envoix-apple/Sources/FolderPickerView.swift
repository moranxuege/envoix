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

#if DEBUG
enum FolderPickerUITestFixture {
    static let payloadArgument = "--ui-testing-folder-payload"
    static let cleanupArgument = "--ui-testing-clean-folder-payload"
    static let runIDEnvironmentKey = "ENVOIX_CROSS_DEVICE_RUN_ID"

    static func initialDirectoryURL() -> URL? {
        guard let documents = FileManager.default.urls(
            for: .documentDirectory,
            in: .userDomainMask
        ).first else { return nil }
        guard ProcessInfo.processInfo.arguments.contains(payloadArgument) else {
            return documents
        }
        guard let fixture = fixture(in: documents) else { return documents }
        do {
            try FileManager.default.createDirectory(
                at: fixture.directory,
                withIntermediateDirectories: true
            )
            if (try? Data(contentsOf: fixture.file)) != fixture.payload {
                try fixture.payload.write(to: fixture.file, options: .atomic)
            }
            return fixture.directory
        } catch {
            assertionFailure("Could not prepare Folder picker UI fixture: \(error)")
            return documents
        }
    }

    static func cleanIfRequested() {
        guard ProcessInfo.processInfo.arguments.contains(cleanupArgument),
              let documents = FileManager.default.urls(
                for: .documentDirectory,
                in: .userDomainMask
              ).first,
              let fixture = fixture(in: documents) else { return }
        try? FileManager.default.removeItem(at: fixture.directory)
    }

    private static func fixture(in documents: URL) -> (directory: URL, file: URL, payload: Data)? {
        guard let runID = ProcessInfo.processInfo.environment[runIDEnvironmentKey],
              runID.count <= 80,
              runID.range(of: "^[A-Za-z0-9_-]+$", options: .regularExpression) != nil else {
            assertionFailure("\(runIDEnvironmentKey) must contain only letters, digits, '-' or '_'")
            return nil
        }
        let directory = documents.appendingPathComponent("envoix-\(runID)-folder", isDirectory: true)
        return (
            directory,
            directory.appendingPathComponent("payload.txt"),
            Data("envoix folder picker payload \(runID)\n".utf8)
        )
    }
}
#endif

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
