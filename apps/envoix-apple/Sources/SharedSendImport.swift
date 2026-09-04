import Foundation

final class ShareDraftLease {
    let id: UUID
    private let store: ShareDraftStore

    init(id: UUID, store: ShareDraftStore) {
        self.id = id
        self.store = store
    }

    func acknowledge() {
        store.acknowledgePending(id: id)
    }

    func bind(to activityID: String) throws {
        try store.bindClaim(id: id, activityID: activityID)
    }

    // SwiftUI may release and recreate views while the draft must remain recoverable.
    func discard() throws {
        try store.discard(id: id)
    }
}

final class SelectedResourceAccessGroup {
    private let resources: [AnyObject]

    init(_ resources: [AnyObject]) {
        self.resources = resources
    }
}

struct PendingSendSelection: Identifiable {
    let id: UUID
    let fileURLs: [URL]
    let sourceAccess: AnyObject
}

#if os(iOS)
enum SharedSendImportOutcome {
    case imported
    case alreadyImported
    case noPendingDraft
    case sendBusy
}
#endif

enum OpenedSendFileOutcome {
    case imported
    case queued
}

enum OpenedSendFileError: LocalizedError, Equatable {
    case unsupportedURL
    case unsupportedItem
    case inaccessible
    case itemCountExceeded

    var errorDescription: String? {
        switch self {
        case .unsupportedURL:
            return "Envoix can open local files only."
        case .unsupportedItem:
            return "Choose a regular file or folder."
        case .inaccessible:
            return "Envoix could not access every selected item."
        case .itemCountExceeded:
            return "Choose no more than \(ShareDraftStore.maxItemCount) items."
        }
    }
}

func validatedOpenedSendURLs(_ urls: [URL]) throws -> [URL] {
    guard !urls.isEmpty else { throw OpenedSendFileError.unsupportedItem }
    guard urls.count <= ShareDraftStore.maxItemCount else {
        throw OpenedSendFileError.itemCountExceeded
    }
    var seenPaths = Set<String>()
    var accepted: [URL] = []
    for url in urls {
        guard url.isFileURL else { throw OpenedSendFileError.unsupportedURL }
        let standardized = url.standardizedFileURL
        guard seenPaths.insert(standardized.path).inserted else { continue }
        let values = try standardized.resourceValues(forKeys: [
            .isRegularFileKey,
            .isDirectoryKey,
            .isSymbolicLinkKey,
        ])
        guard values.isSymbolicLink != true,
              values.isRegularFile == true || values.isDirectory == true else {
            throw OpenedSendFileError.unsupportedItem
        }
        accepted.append(standardized)
    }
    guard !accepted.isEmpty else { throw OpenedSendFileError.unsupportedItem }
    return accepted
}
