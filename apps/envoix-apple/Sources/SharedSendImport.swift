#if os(iOS)
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

struct PendingSendSelection: Identifiable {
    let id: UUID
    let fileURLs: [URL]
    let sourceAccess: AnyObject
}

enum SharedSendImportOutcome {
    case imported
    case alreadyImported
    case noPendingDraft
    case sendBusy
}

enum OpenedSendFileOutcome {
    case imported
    case queued
}

enum OpenedSendFileError: Error, Equatable {
    case unsupportedURL
    case unsupportedItem
    case inaccessible
}
#endif
