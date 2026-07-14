import Foundation
import XCTest
@testable import Envoix_iOS

final class ShareDraftStoreTests: XCTestCase {
    private var root: URL!

    override func setUpWithError() throws {
        continueAfterFailure = false
        root = FileManager.default.temporaryDirectory
            .appendingPathComponent("envoix-share-draft-tests-\(UUID().uuidString)", isDirectory: true)
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
    }

    override func tearDownWithError() throws {
        try? FileManager.default.removeItem(at: root)
        root = nil
    }

    func testStageLoadAndDiscardSingleFile() throws {
        let source = root.appendingPathComponent("source/photo.jpg")
        try FileManager.default.createDirectory(
            at: source.deletingLastPathComponent(),
            withIntermediateDirectories: true
        )
        let payload = Data("share draft payload".utf8)
        try payload.write(to: source)

        let store = ShareDraftStore(rootDirectory: root.appendingPathComponent("drafts"))
        let staged = try store.stage(
            sourceURL: source,
            contentTypeIdentifier: "public.jpeg",
            mediaKind: .image
        )

        XCTAssertEqual(staged.descriptor.schemaVersion, ShareDraftDescriptor.currentSchemaVersion)
        XCTAssertEqual(staged.descriptor.fileName, "photo.jpg")
        XCTAssertEqual(staged.descriptor.byteCount, UInt64(payload.count))
        XCTAssertEqual(staged.descriptor.mediaKind, .image)
        XCTAssertEqual(try Data(contentsOf: staged.fileURL), payload)
        XCTAssertEqual(try store.load(id: staged.descriptor.id), staged)
        XCTAssertEqual(try store.pending(), staged)
        XCTAssertEqual(try store.claimPending(), staged)
        XCTAssertNil(try store.claimPending())

        try store.discard(id: staged.descriptor.id)
        XCTAssertThrowsError(try store.load(id: staged.descriptor.id)) { error in
            XCTAssertEqual(error as? ShareDraftStoreError, .draftNotFound)
        }
    }

    func testShareDraftLinkRoundTripsOnlyShareURLs() {
        let id = UUID()
        XCTAssertEqual(ShareDraftLink.draftID(from: ShareDraftLink.url(for: id)), id)
        XCTAssertNil(ShareDraftLink.draftID(from: URL(string: "https://share/\(id)")!))
        XCTAssertNil(ShareDraftLink.draftID(from: URL(string: "envoix://pair/\(id)")!))
        XCTAssertNil(ShareDraftLink.draftID(from: URL(string: "envoix://share/not-a-uuid")!))
        XCTAssertNil(ShareDraftLink.draftID(from: URL(string: "envoix://share/\(id)/extra")!))
        XCTAssertNil(ShareDraftLink.draftID(from: URL(string: "envoix://share/\(id)?source=other")!))
    }

    func testAppModelImportsPendingDraftForSendSheet() throws {
        let source = root.appendingPathComponent("shared-from-files.txt")
        try Data("shared through app group".utf8).write(to: source)
        let store = try ShareDraftStore.live()
        let staged = try store.stage(
            sourceURL: source,
            contentTypeIdentifier: "public.plain-text",
            mediaKind: .file
        )
        defer {
            AppModel.shared.consumePendingSendSelection(id: staged.descriptor.id)
            try? store.discard(id: staged.descriptor.id)
        }

        switch try AppModel.shared.importSharedSendDraft(preferredID: staged.descriptor.id) {
        case .imported:
            break
        case .noPendingDraft, .sendBusy:
            XCTFail("A staged share draft should be imported while the sender is idle")
        }
        XCTAssertEqual(AppModel.shared.pendingSendSelection?.id, staged.descriptor.id)
        XCTAssertEqual(AppModel.shared.pendingSendSelection?.fileURL, staged.fileURL)
        XCTAssertEqual(try store.pending()?.descriptor.id, staged.descriptor.id)
    }

    func testAppModelImportsFileOpenedBySystem() throws {
        let source = root.appendingPathComponent("opened-document.pdf")
        try Data("opened through document handling".utf8).write(to: source)

        switch try AppModel.shared.importOpenedSendFile(source) {
        case .imported:
            break
        case .queued:
            XCTFail("An opened file should be immediately available while the sender is idle")
        }
        guard let selection = AppModel.shared.pendingSendSelection else {
            return XCTFail("The opened file should become the pending send selection")
        }
        defer { AppModel.shared.consumePendingSendSelection(id: selection.id) }
        XCTAssertEqual(selection.fileURL, source)
        XCTAssertTrue(selection.sourceAccess is SecurityScopedResourceAccess)
    }

    func testAppModelRejectsOpenedDirectory() throws {
        let directory = root.appendingPathComponent("opened-folder", isDirectory: true)
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)

        XCTAssertThrowsError(try AppModel.shared.importOpenedSendFile(directory)) { error in
            XCTAssertEqual(error as? OpenedSendFileError, .unsupportedItem)
        }
    }

    func testAppDeclaresGenericDataDocumentType() {
        let documentTypes = Bundle.main.object(forInfoDictionaryKey: "CFBundleDocumentTypes")
            as? [[String: Any]]
        let supportsGenericData = documentTypes?.contains { documentType in
            documentType["LSHandlerRank"] as? String == "Alternate"
                && (documentType["LSItemContentTypes"] as? [String])?.contains("public.data") == true
        } == true

        XCTAssertTrue(supportsGenericData)
    }

    func testRejectsDirectoryAndQuotaOverflow() throws {
        let sourceDirectory = root.appendingPathComponent("folder", isDirectory: true)
        try FileManager.default.createDirectory(at: sourceDirectory, withIntermediateDirectories: true)
        let store = ShareDraftStore(
            rootDirectory: root.appendingPathComponent("drafts"),
            quotaBytes: 3
        )

        XCTAssertThrowsError(try store.stage(
            sourceURL: sourceDirectory,
            contentTypeIdentifier: "public.folder",
            mediaKind: .file
        )) { error in
            XCTAssertEqual(error as? ShareDraftStoreError, .sourceIsNotRegularFile)
        }

        let source = root.appendingPathComponent("four-bytes.bin")
        try Data([0, 1, 2, 3]).write(to: source)
        XCTAssertThrowsError(try store.stage(
            sourceURL: source,
            contentTypeIdentifier: "public.data",
            mediaKind: .file
        )) { error in
            XCTAssertEqual(error as? ShareDraftStoreError, .quotaExceeded(limitBytes: 3))
        }
    }

    func testCleanupRemovesExpiredDraftAndKeepsFreshDraft() throws {
        var currentDate = Date(timeIntervalSince1970: 10_000)
        let source = root.appendingPathComponent("payload.bin")
        try Data([1, 2]).write(to: source)
        let store = ShareDraftStore(
            rootDirectory: root.appendingPathComponent("drafts"),
            timeToLive: 60,
            now: { currentDate }
        )

        let expired = try store.stage(
            sourceURL: source,
            contentTypeIdentifier: "public.data",
            mediaKind: .file
        )
        currentDate = currentDate.addingTimeInterval(61)
        let fresh = try store.stage(
            sourceURL: source,
            contentTypeIdentifier: "public.data",
            mediaKind: .file
        )

        XCTAssertThrowsError(try store.load(id: expired.descriptor.id))
        XCTAssertEqual(try store.load(id: fresh.descriptor.id), fresh)
    }

    func testLoadRejectsDescriptorPathOutsideDraftRoot() throws {
        let source = root.appendingPathComponent("payload.bin")
        try Data([1]).write(to: source)
        let drafts = root.appendingPathComponent("drafts")
        let store = ShareDraftStore(rootDirectory: drafts)
        let staged = try store.stage(
            sourceURL: source,
            contentTypeIdentifier: "public.data",
            mediaKind: .file
        )

        let descriptorURL = drafts
            .appendingPathComponent(staged.descriptor.id.uuidString, isDirectory: true)
            .appendingPathComponent("draft.json")
        let escaped = ShareDraftDescriptor(
            schemaVersion: ShareDraftDescriptor.currentSchemaVersion,
            id: staged.descriptor.id,
            mediaKind: .file,
            contentTypeIdentifier: "public.data",
            fileName: "payload.bin",
            byteCount: 1,
            createdAtMilliseconds: staged.descriptor.createdAtMilliseconds,
            stagedRelativePath: "../payload.bin"
        )
        try JSONEncoder().encode(escaped).write(to: descriptorURL, options: .atomic)

        XCTAssertThrowsError(try store.load(id: staged.descriptor.id)) { error in
            XCTAssertEqual(error as? ShareDraftStoreError, .invalidDraft)
        }
    }

    func testLoadRejectsDescriptorPathIntoAnotherDraft() throws {
        let source = root.appendingPathComponent("payload.bin")
        try Data([1]).write(to: source)
        let drafts = root.appendingPathComponent("drafts")
        let store = ShareDraftStore(rootDirectory: drafts)
        let first = try store.stage(
            sourceURL: source,
            contentTypeIdentifier: "public.data",
            mediaKind: .file
        )
        let second = try store.stage(
            sourceURL: source,
            contentTypeIdentifier: "public.data",
            mediaKind: .file
        )

        let descriptorURL = drafts
            .appendingPathComponent(first.descriptor.id.uuidString, isDirectory: true)
            .appendingPathComponent("draft.json")
        let aliased = ShareDraftDescriptor(
            schemaVersion: ShareDraftDescriptor.currentSchemaVersion,
            id: first.descriptor.id,
            mediaKind: .file,
            contentTypeIdentifier: "public.data",
            fileName: second.descriptor.fileName,
            byteCount: second.descriptor.byteCount,
            createdAtMilliseconds: first.descriptor.createdAtMilliseconds,
            stagedRelativePath: second.descriptor.stagedRelativePath
        )
        try JSONEncoder().encode(aliased).write(to: descriptorURL, options: .atomic)

        XCTAssertThrowsError(try store.load(id: first.descriptor.id)) { error in
            XCTAssertEqual(error as? ShareDraftStoreError, .invalidDraft)
        }
    }

    func testImportGateCleansDraftAcceptedBeforeCancellation() {
        let gate = ShareDraftImportGate()
        let id = UUID()

        XCTAssertTrue(gate.accept(id))
        XCTAssertEqual(gate.cancel(), id)
        XCTAssertFalse(gate.accept(UUID()))
        XCTAssertNil(gate.cancel())
    }

    func testImportGateRejectsDraftProducedAfterCancellation() {
        let gate = ShareDraftImportGate()

        XCTAssertNil(gate.cancel())
        XCTAssertFalse(gate.accept(UUID()))
    }
}
