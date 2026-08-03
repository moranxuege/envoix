import Foundation
import XCTest
@testable import Envoix_iOS

@MainActor
final class ShareDraftHandoffTests: XCTestCase {
    func testShareExtensionIsEmbeddedAndAcceptsPhotos() throws {
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

        XCTAssertEqual(
            activation["NSExtensionActivationSupportsImageWithMaxCount"] as? Int,
            ShareDraftStore.maxItemCount
        )
    }

    func testNewShareSupersedesAnUnstartedPreparedShare() async throws {
        let root = FileManager.default.temporaryDirectory
            .appendingPathComponent("envoix-share-handoff-\(UUID().uuidString)", isDirectory: true)
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: root) }

        let source = root.appendingPathComponent("first-photo.jpg", isDirectory: false)
        try Data("first photo".utf8).write(to: source, options: .atomic)
        let store = ShareDraftStore(rootDirectory: root.appendingPathComponent("drafts"))
        let first = try store.stage(
            sourceURL: source,
            contentTypeIdentifier: "public.jpeg",
            mediaKind: .image
        )
        let sender = TransferViewModel()
        sender.prepareManifestSelection(
            selectedPaths: first.fileURLs.map(\.path),
            sourceAccess: ShareDraftLease(id: first.descriptor.id, store: store)
        )

        let deadline = Date().addingTimeInterval(10)
        while sender.isPreparingManifest, Date() < deadline {
            try await Task.sleep(nanoseconds: 20_000_000)
        }
        XCTAssertTrue(sender.isManifestSelectionReady)
        XCTAssertEqual(sender.preparedShareDraftID, first.descriptor.id)

        XCTAssertTrue(sender.supersedePreparedShareDraft(with: UUID(), store: store))
        XCTAssertNil(sender.preparedShareDraftID)
        XCTAssertThrowsError(try store.load(id: first.descriptor.id)) { error in
            XCTAssertEqual(error as? ShareDraftStoreError, .draftNotFound)
        }
    }
}
