#if os(macOS)
import XCTest
@testable import Envoix

final class ConnectionHubPresentationTests: XCTestCase {
    func testRoomActionsAndQRCodeUseTheSameSquareFootprint() {
        XCTAssertEqual(RoomInvitationLayout.viewportHeight, 240)
        XCTAssertEqual(RoomInvitationLayout.contentSide(availableWidth: 320), 240)
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

    func testRevealedQRCodeReplacesSupportedConnectionMethods() {
        XCTAssertTrue(RoomInvitationLayout.showsConnectionMethods(revealed: false))
        XCTAssertFalse(RoomInvitationLayout.showsConnectionMethods(revealed: true))
    }
}
#endif
