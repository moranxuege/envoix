import EnvoixCore
import XCTest
@testable import Envoix_iOS

@MainActor
final class ConnectionWorkflowTests: XCTestCase {
    func testInviteRoleSelectsOnlyTheNecessaryTransferAdapter() {
        XCTAssertEqual(ConnectionWorkflowPolicy.localAction(for: .send), .receiveFiles)
        XCTAssertEqual(ConnectionWorkflowPolicy.localAction(for: .receive), .offerFiles)
        XCTAssertEqual(ConnectionWorkflowPolicy.localAction(for: .unknown), .choose)
    }

    func testIncomingOfferQueueDeduplicatesBoundsAndExpires() {
        let workflow = ConnectionWorkflowState()
        let start = Date(timeIntervalSince1970: 1_000)

        XCTAssertTrue(workflow.enqueue(
            offer(id: "duplicate", invitationID: "same"),
            receivedAt: start,
            now: start
        ))
        XCTAssertFalse(workflow.enqueue(
            offer(id: "different-request", invitationID: "same"),
            receivedAt: start,
            now: start
        ))

        for index in 0...4 {
            XCTAssertTrue(workflow.enqueue(
                offer(id: "offer-\(index)", invitationID: "\(index)"),
                receivedAt: start.addingTimeInterval(Double(index)),
                now: start.addingTimeInterval(Double(index))
            ))
        }

        XCTAssertEqual(workflow.pendingOffers.count, ConnectionWorkflowPolicy.maximumPendingOffers)
        XCTAssertEqual(workflow.pendingOffers.map(\.id), ["offer-1", "offer-2", "offer-3", "offer-4"])

        workflow.discardExpiredOffers(
            now: start.addingTimeInterval(ConnectionWorkflowPolicy.offerLifetime + 5)
        )
        XCTAssertTrue(workflow.pendingOffers.isEmpty)
    }

    func testRoomTimelineCapturesOnlyActivitiesCreatedAfterRoomOpened() {
        let workflow = ConnectionWorkflowState()
        workflow.openRoom(
            origin: .pairingCode,
            existingActivityIDs: ["existing"]
        )

        workflow.captureActivity("new-send")
        workflow.captureActivity("new-receive")
        workflow.captureActivity("existing")

        XCTAssertEqual(workflow.room?.activityIDs, ["new-send", "new-receive"])
        workflow.closeRoom()
        XCTAssertNil(workflow.room)
    }

    func testSamePeerOfferPreservesRoomIdentityAndTimeline() throws {
        let workflow = ConnectionWorkflowState()
        let selection = NearbyPairingSelection(
            discoveryPeerKey: "0011223344556677",
            displayName: "Nearby phone",
            sources: [.bluetooth]
        )
        workflow.openRoom(
            origin: .nearby(selection),
            existingActivityIDs: ["older"]
        )
        workflow.captureActivity("room-transfer")
        let originalID = try XCTUnwrap(workflow.room?.id)

        workflow.acceptNearbyOffer(
            selection: selection,
            pairingInput: "envoix://pair/river-stone-next?role=send",
            suggestedAction: .receiveFiles,
            existingActivityIDs: ["older", "unrelated"]
        )

        XCTAssertEqual(workflow.room?.id, originalID)
        XCTAssertEqual(workflow.room?.activityIDs, ["room-transfer"])
        XCTAssertEqual(workflow.room?.suggestedAction, .receiveFiles)
    }

    func testPathPresentationIsStructuredAndPrivacySafe() {
        XCTAssertEqual(
            ConnectionPathPresentationPolicy.label(
                for: FfiConnectionPathEvent(pathKind: .direct, eventKind: .selected),
                language: "en"
            ),
            "Direct path"
        )
        XCTAssertEqual(
            ConnectionPathPresentationPolicy.label(
                for: FfiConnectionPathEvent(pathKind: .relay, eventKind: .changed),
                language: "zh-Hans"
            ),
            "中继链路 · 已切换"
        )
    }

    private func offer(id: String, invitationID: String = "default") -> NearbyRendezvousOffer {
        NearbyRendezvousOffer(
            requestID: id,
            senderPeerKey: "0011223344556677",
            senderDisplayName: "Nearby phone",
            invite: "envoix://pair/river-stone-\(invitationID)?role=send"
        )
    }
}
