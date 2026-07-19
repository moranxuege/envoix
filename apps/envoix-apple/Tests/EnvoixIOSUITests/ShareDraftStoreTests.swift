import EnvoixCore
import Foundation
import UniformTypeIdentifiers
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
        XCTAssertEqual(staged.descriptor.items.count, 1)
        XCTAssertEqual(staged.fileURLs.count, 1)
        XCTAssertEqual(try Data(contentsOf: staged.fileURLs[0]), payload)
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

    func testProviderSelectionAcceptsPhotosAndRejectsLivePhotosAndFolders() throws {
        let image = NSItemProvider(item: Data() as NSData, typeIdentifier: UTType.jpeg.identifier)
        XCTAssertEqual(
            try shareProviderSelection(for: image),
            ShareProviderSelection(typeIdentifier: UTType.jpeg.identifier, mediaKind: .image)
        )

        let movie = NSItemProvider(item: Data() as NSData, typeIdentifier: UTType.movie.identifier)
        XCTAssertEqual(
            try shareProviderSelection(for: movie),
            ShareProviderSelection(typeIdentifier: UTType.movie.identifier, mediaKind: .video)
        )

        let livePhoto = NSItemProvider(
            item: Data() as NSData,
            typeIdentifier: UTType.livePhoto.identifier
        )
        XCTAssertThrowsError(try shareProviderSelection(for: livePhoto)) { error in
            XCTAssertEqual(error as? ShareProviderSelectionError, .livePhotoUnsupported)
        }

        let folder = NSItemProvider(item: Data() as NSData, typeIdentifier: UTType.folder.identifier)
        XCTAssertThrowsError(try shareProviderSelection(for: folder)) { error in
            XCTAssertEqual(error as? ShareProviderSelectionError, .folderUnsupported)
        }
    }

    func testProviderSelectionLoadsFileURLsAsReferences() throws {
        let source = root.appendingPathComponent("shared.txt")
        let provider = NSItemProvider(
            item: source as NSURL,
            typeIdentifier: UTType.fileURL.identifier
        )

        let selection = try shareProviderSelection(for: provider)

        XCTAssertEqual(selection.typeIdentifier, UTType.fileURL.identifier)
        XCTAssertEqual(selection.mediaKind, .file)
        XCTAssertTrue(selection.loadsFileURL)
        XCTAssertEqual(sharedFileURL(fromProviderItem: source as NSURL), source)
    }

    func testSharedFileURLDecodesFilesHostPropertyList() throws {
        let source = root.appendingPathComponent("shared.txt")
        let payload: [Any] = [source.absoluteString, "", [String: String]()]
        let data = try PropertyListSerialization.data(
            fromPropertyList: payload,
            format: .binary,
            options: 0
        )

        XCTAssertEqual(sharedFileURL(fromProviderItem: data as NSData), source)
    }

    func testSharedFileURLRejectsNonFileURLsAndMalformedPropertyLists() throws {
        let webURL = try XCTUnwrap(URL(string: "https://example.com/shared.txt"))
        let webPayload = try PropertyListSerialization.data(
            fromPropertyList: [webURL.absoluteString],
            format: .binary,
            options: 0
        )

        XCTAssertNil(sharedFileURL(fromProviderItem: webURL as NSURL))
        XCTAssertNil(sharedFileURL(fromProviderItem: webPayload as NSData))
        XCTAssertNil(sharedFileURL(fromProviderItem: Data("not a plist".utf8) as NSData))
    }

    func testPhotoDraftImporterCopiesProviderRepresentationIntoDraft() throws {
        let source = root.appendingPathComponent("provider-photo.jpg")
        let payload = Data("photo provider payload".utf8)
        try payload.write(to: source)
        let provider = NSItemProvider()
        provider.suggestedName = "Selected Photo"
        provider.registerFileRepresentation(
            forTypeIdentifier: UTType.jpeg.identifier,
            fileOptions: [],
            visibility: .all
        ) { completion in
            completion(source, false, nil)
            return nil
        }

        let store = ShareDraftStore(rootDirectory: root.appendingPathComponent("drafts"))
        let importer = PhotoDraftImporter(store: store)
        let completed = expectation(description: "Photo representation is staged")
        var result: Result<PhotoDraftImporter.ImportedDraft, Error>?
        var progress: [(Int, Int)] = []

        try importer.start(
            providers: [provider],
            onProgress: { progress.append(($0, $1)) },
            completion: {
                result = $0
                completed.fulfill()
            }
        )
        wait(for: [completed], timeout: 5)

        let imported = try XCTUnwrap(result).get()
        defer { try? store.discard(id: imported.draft.descriptor.id) }
        XCTAssertEqual(progress.map { [$0.0, $0.1] }, [[1, 1]])
        XCTAssertEqual(imported.draft.descriptor.mediaKind, .image)
        XCTAssertEqual(
            imported.draft.descriptor.fileName,
            "Selected Photo.\(try XCTUnwrap(UTType.jpeg.preferredFilenameExtension))"
        )
        XCTAssertEqual(try Data(contentsOf: imported.draft.fileURLs[0]), payload)
        XCTAssertEqual(try store.pending()?.descriptor.id, imported.draft.descriptor.id)
    }

    func testAppModelImportsPendingDraftForSendSheet() throws {
        let firstSource = root.appendingPathComponent("shared-photo.jpg")
        let secondSource = root.appendingPathComponent("shared-video.mov")
        try Data("shared photo through app group".utf8).write(to: firstSource)
        try Data("shared video through app group".utf8).write(to: secondSource)
        let store = try ShareDraftStore.live()
        let staged = try store.stage(items: [
            ShareDraftStagingItem(
                sourceURL: firstSource,
                contentTypeIdentifier: "public.jpeg",
                mediaKind: .image,
                preferredFileName: nil
            ),
            ShareDraftStagingItem(
                sourceURL: secondSource,
                contentTypeIdentifier: "com.apple.quicktime-movie",
                mediaKind: .video,
                preferredFileName: nil
            ),
        ])
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
        XCTAssertEqual(AppModel.shared.pendingSendSelection?.fileURLs, staged.fileURLs)
        XCTAssertTrue(sendSelectionRequiresManifest(staged.fileURLs))
        XCTAssertEqual(try store.pending()?.descriptor.id, staged.descriptor.id)
    }

    func testReleasingShareDraftLeasePreservesDurableDraft() throws {
        let source = root.appendingPathComponent("durable-share.mov")
        let payload = Data("durable share payload".utf8)
        try payload.write(to: source)
        let store = ShareDraftStore(rootDirectory: root.appendingPathComponent("drafts"))
        let staged = try store.stage(
            sourceURL: source,
            contentTypeIdentifier: "com.apple.quicktime-movie",
            mediaKind: .video
        )

        weak var releasedLease: ShareDraftLease?
        autoreleasepool {
            let lease = ShareDraftLease(id: staged.descriptor.id, store: store)
            releasedLease = lease
        }

        XCTAssertNil(releasedLease)
        XCTAssertEqual(try store.load(id: staged.descriptor.id), staged)

        let cleanupLease = ShareDraftLease(id: staged.descriptor.id, store: store)
        try cleanupLease.discard()
        XCTAssertThrowsError(try store.load(id: staged.descriptor.id)) { error in
            XCTAssertEqual(error as? ShareDraftStoreError, .draftNotFound)
        }
    }

    func testAppModelBindsShareDraftBeforeAcknowledgingAndExplicitlyCleansIt() throws {
        let source = root.appendingPathComponent("bound-share.mov")
        try Data("bound share payload".utf8).write(to: source)
        let store = ShareDraftStore(rootDirectory: root.appendingPathComponent("drafts"))
        let staged = try store.stage(
            sourceURL: source,
            contentTypeIdentifier: "com.apple.quicktime-movie",
            mediaKind: .video
        )
        let activityID = "share-draft-test-\(UUID().uuidString)"
        defer {
            AppModel.shared.removeActivity(activityID)
            try? store.discard(id: staged.descriptor.id)
        }

        let lease = ShareDraftLease(id: staged.descriptor.id, store: store)
        AppModel.shared.retainResourceAccess(lease, for: activityID)

        XCTAssertNil(try store.pending())
        XCTAssertEqual(
            try store.claimedDraftsByActivityID()[activityID],
            staged.descriptor.id
        )

        AppModel.shared.handleCoreActivity(Self.activity(id: activityID, state: .queued))
        XCTAssertTrue(AppModel.shared.removeActivity(activityID))
        XCTAssertThrowsError(try store.load(id: staged.descriptor.id)) { error in
            XCTAssertEqual(error as? ShareDraftStoreError, .draftNotFound)
        }
    }

    func testStageMultipleFilesKeepsOrderAndRenamesCollisions() throws {
        let first = root.appendingPathComponent("first.jpg")
        let second = root.appendingPathComponent("second.jpg")
        let firstPayload = Data("first photo".utf8)
        let secondPayload = Data("second photo".utf8)
        try firstPayload.write(to: first)
        try secondPayload.write(to: second)
        let store = ShareDraftStore(rootDirectory: root.appendingPathComponent("drafts"))

        let staged = try store.stage(items: [
            ShareDraftStagingItem(
                sourceURL: first,
                contentTypeIdentifier: "public.jpeg",
                mediaKind: .image,
                preferredFileName: "Photo.jpg"
            ),
            ShareDraftStagingItem(
                sourceURL: second,
                contentTypeIdentifier: "public.jpeg",
                mediaKind: .image,
                preferredFileName: "photo.jpg"
            ),
        ])

        XCTAssertEqual(staged.descriptor.items.map(\.fileName), ["Photo.jpg", "photo (2).jpg"])
        XCTAssertEqual(staged.descriptor.byteCount, UInt64(firstPayload.count + secondPayload.count))
        XCTAssertEqual(try Data(contentsOf: staged.fileURLs[0]), firstPayload)
        XCTAssertEqual(try Data(contentsOf: staged.fileURLs[1]), secondPayload)
        XCTAssertEqual(try store.load(id: staged.descriptor.id), staged)
    }

    func testLoadsLegacySingleItemDescriptor() throws {
        let drafts = root.appendingPathComponent("drafts", isDirectory: true)
        let id = UUID()
        let draftDirectory = drafts.appendingPathComponent(id.uuidString, isDirectory: true)
        let payloadURL = draftDirectory.appendingPathComponent("legacy.txt")
        let payload = Data("legacy share draft".utf8)
        try FileManager.default.createDirectory(at: draftDirectory, withIntermediateDirectories: true)
        try payload.write(to: payloadURL)
        let descriptor = ShareDraftDescriptor(
            schemaVersion: ShareDraftDescriptor.legacySchemaVersion,
            id: id,
            mediaKind: .file,
            contentTypeIdentifier: "public.plain-text",
            fileName: "legacy.txt",
            byteCount: UInt64(payload.count),
            createdAtMilliseconds: 1_000,
            stagedRelativePath: "\(id.uuidString)/legacy.txt"
        )
        try JSONEncoder().encode(descriptor).write(
            to: draftDirectory.appendingPathComponent("draft.json"),
            options: .atomic
        )

        let loaded = try ShareDraftStore(rootDirectory: drafts).load(id: id)
        XCTAssertEqual(loaded.descriptor.schemaVersion, ShareDraftDescriptor.legacySchemaVersion)
        XCTAssertEqual(loaded.fileURLs, [payloadURL])
        XCTAssertEqual(try Data(contentsOf: loaded.fileURLs[0]), payload)
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
        XCTAssertEqual(selection.fileURLs, [source])
        XCTAssertTrue(selection.sourceAccess is SecurityScopedResourceAccess)
    }

    func testAppModelImportsDirectoryOpenedBySystem() throws {
        let directory = root.appendingPathComponent("opened-folder", isDirectory: true)
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)

        switch try AppModel.shared.importOpenedSendFile(directory) {
        case .imported:
            break
        case .queued:
            XCTFail("An opened folder should be immediately available while the sender is idle")
        }
        guard let selection = AppModel.shared.pendingSendSelection else {
            return XCTFail("The opened folder should become the pending send selection")
        }
        defer { AppModel.shared.consumePendingSendSelection(id: selection.id) }
        XCTAssertEqual(selection.fileURLs, [directory])
        XCTAssertTrue(selection.sourceAccess is SecurityScopedResourceAccess)
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

    func testShareExtensionActivationMatchesDraftItemLimit() throws {
        let extensionURL = try XCTUnwrap(Bundle.main.builtInPlugInsURL)
            .appendingPathComponent("EnvoixShare.appex", isDirectory: true)
        let extensionBundle = try XCTUnwrap(Bundle(url: extensionURL))
        let extensionInfo = try XCTUnwrap(extensionBundle.infoDictionary)
        let extensionConfiguration = try XCTUnwrap(
            extensionInfo["NSExtension"] as? [String: Any]
        )
        let attributes = try XCTUnwrap(
            extensionConfiguration["NSExtensionAttributes"] as? [String: Any]
        )
        let activation = try XCTUnwrap(
            attributes["NSExtensionActivationRule"] as? [String: Any]
        )
        for key in [
            "NSExtensionActivationSupportsAttachmentsWithMaxCount",
            "NSExtensionActivationSupportsFileWithMaxCount",
            "NSExtensionActivationSupportsImageWithMaxCount",
            "NSExtensionActivationSupportsMovieWithMaxCount",
        ] {
            XCTAssertEqual(activation[key] as? Int, ShareDraftStore.maxItemCount, key)
        }
    }

    func testRejectsDirectoryAndInsufficientStorage() throws {
        let sourceDirectory = root.appendingPathComponent("folder", isDirectory: true)
        try FileManager.default.createDirectory(at: sourceDirectory, withIntermediateDirectories: true)
        let store = ShareDraftStore(
            rootDirectory: root.appendingPathComponent("drafts"),
            availableCapacity: { _ in 3 }
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
            XCTAssertEqual(
                error as? ShareDraftStoreError,
                .insufficientStorage(requiredBytes: 4, availableBytes: 3)
            )
        }

        let twoBytes = root.appendingPathComponent("two-bytes.bin")
        try Data([0, 1]).write(to: twoBytes)
        var availableBytes: Int64 = 3
        let aggregateStore = ShareDraftStore(
            rootDirectory: root.appendingPathComponent("aggregate-drafts"),
            availableCapacity: { _ in
                defer { availableBytes -= 2 }
                return availableBytes
            }
        )
        XCTAssertThrowsError(try aggregateStore.stage(items: [
            ShareDraftStagingItem(
                sourceURL: twoBytes,
                contentTypeIdentifier: "public.data",
                mediaKind: .file,
                preferredFileName: nil
            ),
            ShareDraftStagingItem(
                sourceURL: twoBytes,
                contentTypeIdentifier: "public.data",
                mediaKind: .file,
                preferredFileName: nil
            ),
        ])) { error in
            XCTAssertEqual(
                error as? ShareDraftStoreError,
                .insufficientStorage(requiredBytes: 2, availableBytes: 1)
            )
        }
    }

    func testIncrementalStagingNeedsOnlyTheAppGroupCopy() throws {
        let source = root.appendingPathComponent("provider-temporary.bin")
        let payload = Data("provider callback payload".utf8)
        try payload.write(to: source)
        let store = ShareDraftStore(rootDirectory: root.appendingPathComponent("drafts"))
        let staging = try store.beginStaging(expectedItemCount: 1)

        try staging.append(ShareDraftStagingItem(
            sourceURL: source,
            contentTypeIdentifier: "public.data",
            mediaKind: .file,
            preferredFileName: "payload.bin"
        ))
        try FileManager.default.removeItem(at: source)
        let draft = try staging.finalize()

        XCTAssertEqual(draft.descriptor.items.map(\.fileName), ["payload.bin"])
        XCTAssertEqual(try Data(contentsOf: draft.fileURLs[0]), payload)
    }

    func testShareDraftMaterializationUsesCopyOnWriteCloneOnAPFS() throws {
        let source = root.appendingPathComponent("clone-source.bin")
        let destination = root.appendingPathComponent("clone-destination.bin")
        let payload = Data(repeating: 0x5a, count: 1_048_576)
        try payload.write(to: source)

        let materialization = try materializeShareDraftFile(
            at: source,
            to: destination
        )

        XCTAssertEqual(materialization, .cloned)
        try FileManager.default.removeItem(at: source)
        XCTAssertEqual(try Data(contentsOf: destination), payload)
    }

    func testRejectsEmptyAndOversizedItemLists() throws {
        let store = ShareDraftStore(rootDirectory: root.appendingPathComponent("drafts"))
        XCTAssertThrowsError(try store.stage(items: [])) { error in
            XCTAssertEqual(
                error as? ShareDraftStoreError,
                .itemCountExceeded(limit: ShareDraftStore.maxItemCount)
            )
        }

        let source = root.appendingPathComponent("payload.bin")
        try Data([1]).write(to: source)
        let item = ShareDraftStagingItem(
            sourceURL: source,
            contentTypeIdentifier: "public.data",
            mediaKind: .file,
            preferredFileName: nil
        )
        XCTAssertThrowsError(
            try store.stage(items: Array(repeating: item, count: ShareDraftStore.maxItemCount + 1))
        ) { error in
            XCTAssertEqual(
                error as? ShareDraftStoreError,
                .itemCountExceeded(limit: ShareDraftStore.maxItemCount)
            )
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

    func testCleanupProtectsClaimedResumableDraft() throws {
        var currentDate = Date(timeIntervalSince1970: 10_000)
        let source = root.appendingPathComponent("payload.bin")
        try Data([1, 2]).write(to: source)
        let store = ShareDraftStore(
            rootDirectory: root.appendingPathComponent("drafts"),
            timeToLive: 60,
            now: { currentDate }
        )
        let draft = try store.stage(
            sourceURL: source,
            contentTypeIdentifier: "public.data",
            mediaKind: .file
        )
        try store.claim(id: draft.descriptor.id, activityID: "resume-activity")

        currentDate = currentDate.addingTimeInterval(61)
        try store.cleanupExpired()
        XCTAssertEqual(try store.load(id: draft.descriptor.id), draft)

        try store.reconcileCache(
            protectingDraftIDs: [],
            protectingActivityIDs: ["resume-activity"]
        )
        XCTAssertEqual(try store.load(id: draft.descriptor.id), draft)

        try store.reconcileCache(protectingDraftIDs: [], protectingActivityIDs: [])
        XCTAssertThrowsError(try store.load(id: draft.descriptor.id)) { error in
            XCTAssertEqual(error as? ShareDraftStoreError, .draftNotFound)
        }
    }

    func testManualCleanupKeepsOnlyProtectedDrafts() throws {
        let firstSource = root.appendingPathComponent("first.bin")
        let secondSource = root.appendingPathComponent("second.bin")
        try Data([1, 2]).write(to: firstSource)
        try Data([3, 4, 5]).write(to: secondSource)
        let store = ShareDraftStore(rootDirectory: root.appendingPathComponent("drafts"))
        let protected = try store.stage(
            sourceURL: firstSource,
            contentTypeIdentifier: "public.data",
            mediaKind: .file
        )
        try store.claim(id: protected.descriptor.id, activityID: "paused-activity")
        let removable = try store.stage(
            sourceURL: secondSource,
            contentTypeIdentifier: "public.data",
            mediaKind: .file
        )

        let before = try store.cacheSummary(
            protectingDraftIDs: [],
            protectingActivityIDs: ["paused-activity"]
        )
        XCTAssertGreaterThan(before.totalBytes, before.protectedBytes)
        XCTAssertGreaterThan(before.removableBytes, 0)

        try store.cleanUnprotected(
            protectingDraftIDs: [],
            protectingActivityIDs: ["paused-activity"]
        )
        XCTAssertEqual(try store.load(id: protected.descriptor.id), protected)
        XCTAssertThrowsError(try store.load(id: removable.descriptor.id))
    }

    func testReceiveCacheCleanupKeepsPausedActivityData() throws {
        let support = root.appendingPathComponent("Application Support", isDirectory: true)
        let staging = support.appendingPathComponent("envoix/receive-staging", isDirectory: true)
        let paused = staging.appendingPathComponent("paused-activity", isDirectory: true)
        let orphan = staging.appendingPathComponent("orphan-activity", isDirectory: true)
        try FileManager.default.createDirectory(at: paused, withIntermediateDirectories: true)
        try FileManager.default.createDirectory(at: orphan, withIntermediateDirectories: true)
        try Data([1, 2, 3]).write(to: paused.appendingPathComponent("paused.bin"))
        try Data([4, 5]).write(to: orphan.appendingPathComponent("orphan.bin"))
        let store = TransferCacheStore(
            applicationSupportDirectory: support,
            includeSharedDrafts: false
        )

        let before = try store.summary(
            protectingDraftIDs: [],
            protectingActivityIDs: ["paused-activity"]
        )
        XCTAssertEqual(before.totalBytes, 5)
        XCTAssertEqual(before.protectedBytes, 3)

        try store.cleanUnprotected(
            protectingDraftIDs: [],
            protectingActivityIDs: ["paused-activity"],
            createdBefore: .distantFuture
        )
        XCTAssertTrue(FileManager.default.fileExists(atPath: paused.path))
        XCTAssertFalse(FileManager.default.fileExists(atPath: orphan.path))
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

    private static func activity(
        id: String,
        state: FfiTransferActivityState
    ) -> FfiTransferActivityRecord {
        FfiTransferActivityRecord(
            activityId: id,
            sequence: 1,
            attemptId: "attempt-1",
            state: state,
            direction: .send,
            mode: .room,
            transferId: "transfer-\(id)",
            fileName: "bound-share.mov",
            totalBytes: 19,
            bytesTransferred: 0,
            bytesResumed: 0,
            speedBps: 0,
            averageSpeedBps: 0,
            createdAtMs: 1,
            updatedAtMs: 1,
            startedAtMs: 0,
            completedAtMs: 0,
            completedFilePath: "",
            dataPathKind: .none,
            dataPathDetail: "",
            invite: "",
            token: "",
            peerDescriptor: "",
            diagnosticMessage: "",
            failureCode: .unknown,
            failureCategory: .unknown,
            failurePhase: .setup,
            failureOrigin: .unknown,
            userMessageKey: "",
            retryable: false,
            recoveryAction: .none,
            limits: FfiTransferLimits(
                maxParallelTransfers: 1,
                maxParallelFiles: 1,
                maxParallelChunksPerFile: 1,
                speedLimitBps: 0
            )
        )
    }
}
