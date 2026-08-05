import XCTest
@testable import Envoix_iOS

final class ConnectionHubPresentationTests: XCTestCase {
    func testRoomActionsAndQRCodeUseTheSameSquareFootprint() {
        XCTAssertEqual(RoomInvitationLayout.viewportHeight, 240)
        XCTAssertEqual(RoomInvitationLayout.contentSide(availableWidth: 320), 240)
        XCTAssertEqual(RoomInvitationLayout.contentSide(availableWidth: 220), 220)
        XCTAssertEqual(
            RoomInvitationLayout.qrImageSide(contentSide: 240)
                + QRCard.contentPadding * 2,
            240
        )
    }

    func testWholeRoomCardKeepsOneContentHeightAcrossInvitationStates() {
        XCTAssertEqual(RoomInvitationLayout.headerHeight, 44)
        XCTAssertEqual(RoomInvitationLayout.cardSpacing, 14)
        XCTAssertEqual(RoomInvitationLayout.cardContentHeight, 298)
    }

    func testRevealedQRCodeReplacesConnectionMethods() {
        XCTAssertTrue(RoomInvitationLayout.showsConnectionMethods(revealed: false))
        XCTAssertFalse(RoomInvitationLayout.showsConnectionMethods(revealed: true))
    }

    func testSendSelectionUsesStatusCardOnlyAfterTransferStarts() {
        XCTAssertFalse(SendSelectionPresentationPolicy.showsTransferStatus(hasTransferActivity: false))
        XCTAssertTrue(SendSelectionPresentationPolicy.showsTransferStatus(hasTransferActivity: true))
    }
}
