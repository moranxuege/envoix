import Foundation

#if os(iOS)
import SwiftUI
import UIKit
import UniformTypeIdentifiers

private struct DocumentPickerSheet: UIViewControllerRepresentable {
    let contentTypes: [UTType]
    let allowsMultipleSelection: Bool
    let initialDirectoryURL: URL?
    let onPick: ([URL]) -> Void
    let onCancel: () -> Void

    func makeCoordinator() -> Coordinator {
        Coordinator(parent: self)
    }

    func makeUIViewController(context: Context) -> UIDocumentPickerViewController {
        let controller = UIDocumentPickerViewController(forOpeningContentTypes: contentTypes, asCopy: false)
        controller.allowsMultipleSelection = allowsMultipleSelection
        controller.directoryURL = initialDirectoryURL
        controller.delegate = context.coordinator
        return controller
    }

    func updateUIViewController(_ uiViewController: UIDocumentPickerViewController, context: Context) {}

    final class Coordinator: NSObject, UIDocumentPickerDelegate {
        private let parent: DocumentPickerSheet

        init(parent: DocumentPickerSheet) {
            self.parent = parent
        }

        func documentPicker(_ controller: UIDocumentPickerViewController, didPickDocumentsAt urls: [URL]) {
            guard !urls.isEmpty else {
                parent.onCancel()
                return
            }
            parent.onPick(urls)
        }

        func documentPickerWasCancelled(_ controller: UIDocumentPickerViewController) {
            parent.onCancel()
        }
    }
}

struct FolderPickerSheet: View {
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

    var body: some View {
        DocumentPickerSheet(
            contentTypes: [.folder],
            allowsMultipleSelection: false,
            initialDirectoryURL: initialDirectoryURL,
            onPick: { urls in
                guard let url = urls.first else {
                    onCancel()
                    return
                }
                onPick(url)
            },
            onCancel: onCancel
        )
    }
}

struct MultiFolderPickerSheet: View {
    let initialDirectoryURL: URL?
    let onPick: ([URL]) -> Void
    let onCancel: () -> Void

    init(
        initialDirectoryURL: URL? = nil,
        onPick: @escaping ([URL]) -> Void,
        onCancel: @escaping () -> Void
    ) {
        self.initialDirectoryURL = initialDirectoryURL
        self.onPick = onPick
        self.onCancel = onCancel
    }

    var body: some View {
        DocumentPickerSheet(
            contentTypes: [.folder],
            allowsMultipleSelection: true,
            initialDirectoryURL: initialDirectoryURL,
            onPick: onPick,
            onCancel: onCancel
        )
    }
}

struct FilePickerSheet: View {
    let initialDirectoryURL: URL?
    let onPick: ([URL]) -> Void
    let onCancel: () -> Void

    init(
        initialDirectoryURL: URL? = nil,
        onPick: @escaping ([URL]) -> Void,
        onCancel: @escaping () -> Void
    ) {
        self.initialDirectoryURL = initialDirectoryURL
        self.onPick = onPick
        self.onCancel = onCancel
    }

    var body: some View {
        DocumentPickerSheet(
            contentTypes: [.data],
            allowsMultipleSelection: true,
            initialDirectoryURL: initialDirectoryURL,
            onPick: onPick,
            onCancel: onCancel
        )
    }
}

#if DEBUG
private enum PickerUITestFixtureRunID {
    static let environmentKey = "ENVOIX_CROSS_DEVICE_RUN_ID"

    static func current() -> String? {
        guard let runID = ProcessInfo.processInfo.environment[environmentKey],
              runID.count <= 80,
              runID.range(of: "^[A-Za-z0-9_-]+$", options: .regularExpression) != nil else {
            assertionFailure("\(environmentKey) must contain only letters, digits, '-' or '_'")
            return nil
        }
        return runID
    }
}

enum FolderPickerUITestFixture {
    static let payloadArgument = "--ui-testing-folder-payload"
    static let cleanupArgument = "--ui-testing-clean-folder-payload"

    static func initialDirectoryURL() -> URL? {
        guard let documents = FileManager.default.urls(
            for: .documentDirectory,
            in: .userDomainMask
        ).first else { return nil }
        guard ProcessInfo.processInfo.arguments.contains(payloadArgument) else {
            return compatibleInitialDirectory(documents)
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
            return compatibleInitialDirectory(fixture.directory)
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

    private static func compatibleInitialDirectory(_ directory: URL) -> URL? {
        if #available(iOS 26.0, *) {
            return nil
        }
        return directory
    }

    private static func fixture(in documents: URL) -> (directory: URL, file: URL, payload: Data)? {
        guard let runID = PickerUITestFixtureRunID.current() else { return nil }
        let directory = documents.appendingPathComponent("envoix-\(runID)-folder", isDirectory: true)
        return (
            directory,
            directory.appendingPathComponent("payload.txt"),
            Data("envoix folder picker payload \(runID)\n".utf8)
        )
    }
}

enum FilePickerUITestFixture {
    static let payloadArgument = "--ui-testing-file-payload"
    static let cleanupArgument = "--ui-testing-clean-file-payload"

    static func initialDirectoryURL() -> URL? {
        let documents = prepareDocuments()
        if #available(iOS 26.0, *) {
            return nil
        }
        return documents
    }

    static func stageIfRequested() {
        guard ProcessInfo.processInfo.arguments.contains(payloadArgument) else { return }
        _ = prepareDocuments()
    }

    private static func prepareDocuments() -> URL? {
        guard let documents = FileManager.default.urls(
            for: .documentDirectory,
            in: .userDomainMask
        ).first else { return nil }
        guard ProcessInfo.processInfo.arguments.contains(payloadArgument),
              let files = files(in: documents) else { return documents }
        do {
            for file in files where (try? Data(contentsOf: file.url)) != file.payload {
                try file.payload.write(to: file.url, options: .atomic)
            }
            return documents
        } catch {
            assertionFailure("Could not prepare Files picker UI fixture: \(error)")
            return documents
        }
    }

    static func cleanIfRequested() {
        guard ProcessInfo.processInfo.arguments.contains(cleanupArgument),
              let documents = FileManager.default.urls(
                for: .documentDirectory,
                in: .userDomainMask
              ).first,
              let files = files(in: documents) else { return }
        for file in files {
            try? FileManager.default.removeItem(at: file.url)
        }
    }

    private static func files(in documents: URL) -> [(url: URL, payload: Data)]? {
        guard let runID = PickerUITestFixtureRunID.current() else { return nil }
        return [
            (
                documents.appendingPathComponent("envoix-\(runID)-file-first.txt"),
                Data("envoix file picker payload first \(runID)\n".utf8)
            ),
            (
                documents.appendingPathComponent("envoix-\(runID)-file-second.txt"),
                Data("envoix file picker payload second \(runID)\n".utf8)
            ),
        ]
    }
}

enum OpenInUITestFixture {
    static let payloadArgument = "--ui-testing-open-in-payload"
    static let cleanupArgument = "--ui-testing-clean-open-in-payload"

    static func stageIfRequested() -> URL? {
        guard ProcessInfo.processInfo.arguments.contains(payloadArgument),
              let fixture = fixture() else { return nil }
        do {
            if (try? Data(contentsOf: fixture.url)) != fixture.payload {
                try fixture.payload.write(to: fixture.url, options: .atomic)
            }
            return fixture.url
        } catch {
            assertionFailure("Could not prepare Open In UI fixture: \(error)")
            return nil
        }
    }

    static func cleanIfRequested() {
        guard ProcessInfo.processInfo.arguments.contains(cleanupArgument),
              let fixture = fixture() else { return }
        try? FileManager.default.removeItem(at: fixture.url)
    }

    private static func fixture() -> (url: URL, payload: Data)? {
        guard let runID = PickerUITestFixtureRunID.current(),
              let documents = FileManager.default.urls(
                for: .documentDirectory,
                in: .userDomainMask
              ).first else { return nil }
        return (
            documents.appendingPathComponent("envoix-\(runID)-open-in.txt"),
            Data("envoix Open In payload \(runID)\n".utf8)
        )
    }
}
#endif
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
    #if os(macOS)
    let options: URL.BookmarkCreationOptions = [.withSecurityScope]
    #else
    let options: URL.BookmarkCreationOptions = []
    #endif
    return try url.bookmarkData(
        options: options,
        includingResourceValuesForKeys: nil,
        relativeTo: nil
    )
}

func resolveSecurityScopedFolderBookmark(_ data: Data) throws -> URL {
    var isStale = false
    #if os(macOS)
    let options: URL.BookmarkResolutionOptions = [.withSecurityScope]
    #else
    let options: URL.BookmarkResolutionOptions = []
    #endif
    let url = try URL(
        resolvingBookmarkData: data,
        options: options,
        relativeTo: nil,
        bookmarkDataIsStale: &isStale
    )
    if isStale {
        throw RuntimeSettingsError("The selected save folder permission expired. Choose the folder again.")
    }
    return url
}

#if os(macOS)
/// Resolves the persisted destination without silently falling back when a
/// bookmark exists but is no longer usable. A stale bookmark must be renewed
/// through NSOpenPanel; otherwise a legacy path can bypass the user's current
/// Files & Folders authorization and fail only after pairing has started.
func resolveRememberedOutputDirectory(
    bookmarkData: Data?,
    legacyPath: String,
    defaultURL: URL
) -> URL? {
    if let bookmarkData {
        return try? resolveSecurityScopedFolderBookmark(bookmarkData)
    }
    if !legacyPath.isEmpty {
        return URL(fileURLWithPath: legacyPath, isDirectory: true)
    }
    return defaultURL
}
#endif

/// Performs a real create/write/remove cycle so folder authorization failures
/// surface before pairing starts instead of after the peer begins sending.
func validateWritableDirectoryAccess(
    _ directory: URL,
    fileManager: FileManager = .default
) throws {
    var isDirectory: ObjCBool = false
    if fileManager.fileExists(atPath: directory.path, isDirectory: &isDirectory),
       !isDirectory.boolValue {
        throw RuntimeSettingsError("The selected save location is not a folder.")
    }

    do {
        try fileManager.createDirectory(at: directory, withIntermediateDirectories: true)
        let probe = directory.appendingPathComponent(".envoix-write-probe-\(UUID().uuidString)")
        defer { try? fileManager.removeItem(at: probe) }
        try Data("envoix".utf8).write(to: probe, options: .atomic)
        try fileManager.removeItem(at: probe)
    } catch {
        throw RuntimeSettingsError("Envoix cannot write to the selected save folder: \(error.localizedDescription)")
    }
}
