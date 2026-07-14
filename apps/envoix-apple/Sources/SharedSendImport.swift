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

    deinit {
        try? store.discard(id: id)
    }
}

struct PendingSendSelection: Identifiable {
    let id: UUID
    let fileURL: URL
    let sourceAccess: AnyObject
}

enum SharedSendImportOutcome {
    case imported
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
