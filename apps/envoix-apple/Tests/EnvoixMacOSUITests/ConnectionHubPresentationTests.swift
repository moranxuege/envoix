#if os(macOS)
import XCTest
@testable import Envoix

final class ConnectionHubPresentationTests: XCTestCase {
    func testRoomActionsAndQRCodeUseTheSameSquareFootprint() {
        XCTAssertEqual(RoomInvitationLayout.viewportHeight, 152)
        XCTAssertEqual(RoomInvitationLayout.contentSide(availableWidth: 320), 176)
        XCTAssertEqual(
            RoomInvitationLayout.qrImageSide(contentSide: 176)
                + QRCard.contentPadding * 2,
            176
        )
    }

    func testWholeRoomCardKeepsOneContentHeightAcrossInvitationStates() {
        XCTAssertEqual(RoomInvitationLayout.headerHeight, 44)
        XCTAssertEqual(RoomInvitationLayout.cardSpacing, 14)
        XCTAssertEqual(RoomInvitationLayout.cardContentHeight, 210)
    }

    func testRevealedQRCodeReplacesSupportedConnectionMethods() {
        XCTAssertTrue(RoomInvitationLayout.showsConnectionMethods(revealed: false))
        XCTAssertFalse(RoomInvitationLayout.showsConnectionMethods(revealed: true))
    }

    func testRememberedDeviceSendRequiresAUsableRelationship() {
        XCTAssertTrue(RememberedDeviceSendPolicy.canSend(status: .offline))
        XCTAssertTrue(RememberedDeviceSendPolicy.canSend(status: .available))
        XCTAssertTrue(RememberedDeviceSendPolicy.canSend(status: .connecting))
        XCTAssertTrue(RememberedDeviceSendPolicy.canSend(status: .waiting))
        XCTAssertTrue(RememberedDeviceSendPolicy.canSend(status: .connected))
        XCTAssertFalse(RememberedDeviceSendPolicy.canSend(status: .needsRepair("expired")))
    }

    func testRememberedDeviceDropEnforcesItemBoundaries() {
        XCTAssertFalse(RememberedDeviceSendPolicy.acceptsDrop(
            providerCount: 0,
            status: .connected
        ))
        XCTAssertTrue(RememberedDeviceSendPolicy.acceptsDrop(
            providerCount: ShareDraftStore.maxItemCount,
            status: .offline
        ))
        XCTAssertFalse(RememberedDeviceSendPolicy.acceptsDrop(
            providerCount: ShareDraftStore.maxItemCount + 1,
            status: .connected
        ))
        XCTAssertFalse(RememberedDeviceSendPolicy.acceptsDrop(
            providerCount: 1,
            status: .needsRepair("expired")
        ))
    }

    func testOpenedSendURLsAcceptFilesAndDirectoriesAndDeduplicatePaths() throws {
        let root = FileManager.default.temporaryDirectory
            .appendingPathComponent(UUID().uuidString, isDirectory: true)
        try FileManager.default.createDirectory(
            at: root,
            withIntermediateDirectories: true
        )
        defer { try? FileManager.default.removeItem(at: root) }

        let file = root.appendingPathComponent("message.txt")
        XCTAssertTrue(FileManager.default.createFile(
            atPath: file.path,
            contents: Data("hello".utf8)
        ))
        let folder = root.appendingPathComponent("folder", isDirectory: true)
        try FileManager.default.createDirectory(
            at: folder,
            withIntermediateDirectories: false
        )

        XCTAssertEqual(
            try validatedOpenedSendURLs([file, folder, file]),
            [file.standardizedFileURL, folder.standardizedFileURL]
        )
    }

    func testOpenedSendURLsRejectUnsupportedAndOversizedSelections() throws {
        let root = FileManager.default.temporaryDirectory
            .appendingPathComponent(UUID().uuidString, isDirectory: true)
        try FileManager.default.createDirectory(
            at: root,
            withIntermediateDirectories: true
        )
        defer { try? FileManager.default.removeItem(at: root) }

        let file = root.appendingPathComponent("target.txt")
        XCTAssertTrue(FileManager.default.createFile(atPath: file.path, contents: Data()))
        let link = root.appendingPathComponent("link.txt")
        try FileManager.default.createSymbolicLink(at: link, withDestinationURL: file)

        XCTAssertThrowsError(try validatedOpenedSendURLs([])) { error in
            XCTAssertEqual(error as? OpenedSendFileError, .unsupportedItem)
        }
        XCTAssertThrowsError(try validatedOpenedSendURLs([URL(string: "https://example.com")!])) {
            error in
            XCTAssertEqual(error as? OpenedSendFileError, .unsupportedURL)
        }
        XCTAssertThrowsError(try validatedOpenedSendURLs([link])) { error in
            XCTAssertEqual(error as? OpenedSendFileError, .unsupportedItem)
        }
        XCTAssertThrowsError(try validatedOpenedSendURLs(Array(
            repeating: file,
            count: ShareDraftStore.maxItemCount + 1
        ))) { error in
            XCTAssertEqual(error as? OpenedSendFileError, .itemCountExceeded)
        }
    }

    @MainActor
    func testFinderServiceExposesItsAdvertisedSelector() {
        let selector = NSSelectorFromString("sendWithEnvoix:userData:error:")
        XCTAssertTrue(MacFinderSendService.instancesRespond(to: selector))
    }
}
#endif
