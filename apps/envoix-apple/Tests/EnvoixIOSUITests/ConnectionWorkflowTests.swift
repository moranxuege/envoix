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

    func testRoomControlDoesNotOpenRoomBeforeConnectedEvent() async throws {
        let gateway = RecordingRoomControlGateway()
        let workflow = ConnectionWorkflowState(
            gateway: gateway,
            clock: { Date(timeIntervalSince1970: 1_000) }
        )

        XCTAssertNil(workflow.startHosting(
            broker: "",
            relay: "",
            displayName: "My iPhone",
            identityPath: "/tmp/envoix-test-identity",
            existingActivityIDs: ["old"]
        ))
        XCTAssertEqual(workflow.controlPhase, .hosting)
        XCTAssertNil(workflow.room)
        await Task.yield()

        gateway.emit(.connected(peerDisplayName: "Other phone", creator: true))
        await Task.yield()

        XCTAssertEqual(workflow.controlPhase, .connected)
        XCTAssertEqual(workflow.peerDisplayName, "Other phone")
        XCTAssertTrue(workflow.isRoomCreator)
        XCTAssertEqual(workflow.room?.origin, .roomControl)
    }

    func testRoomIdleExpirySuspendsForActiveTransferAndHonorsKeepOpen() async {
        let start = Date(timeIntervalSince1970: 2_000)
        let gateway = RecordingRoomControlGateway()
        let workflow = ConnectionWorkflowState(gateway: gateway, clock: { start })
        _ = workflow.startHosting(
            broker: "",
            relay: "",
            displayName: "My iPhone",
            identityPath: "/tmp/envoix-test-identity",
            existingActivityIDs: []
        )
        await Task.yield()
        gateway.emit(.connected(peerDisplayName: "Peer", creator: true))
        await Task.yield()
        let boundary = start.addingTimeInterval(ConnectionWorkflowPolicy.roomIdleLifetime)

        workflow.tick(now: boundary, hasActiveTransfer: true)
        XCTAssertEqual(workflow.controlPhase, .connected)

        workflow.setKeepOpen(true)
        workflow.tick(now: boundary.addingTimeInterval(1), hasActiveTransfer: false)
        XCTAssertEqual(workflow.controlPhase, .connected)
        XCTAssertNil(workflow.idleDeadline)

        workflow.setKeepOpen(false)
        workflow.noteRoomActivity(now: start.addingTimeInterval(1))
        workflow.tick(now: boundary, hasActiveTransfer: false)
        XCTAssertEqual(workflow.controlPhase, .connected)
        workflow.tick(now: boundary.addingTimeInterval(1), hasActiveTransfer: false)
        XCTAssertEqual(workflow.controlPhase, .ended(.idleExpired))
    }

    func testEndedRoomIgnoresLateGatewayEventsAndLegacyRoomStartsClean() async {
        let gateway = RecordingRoomControlGateway()
        let workflow = ConnectionWorkflowState(gateway: gateway)
        _ = workflow.startHosting(
            broker: "",
            relay: "",
            displayName: "My iPhone",
            identityPath: "/tmp/envoix-test-identity",
            existingActivityIDs: []
        )
        await Task.yield()

        workflow.endControl(reason: .userEnded)
        gateway.emit(.connected(peerDisplayName: "Late peer", creator: true))
        await Task.yield()
        XCTAssertNil(workflow.room)
        XCTAssertEqual(workflow.controlPhase, .ended(.userEnded))

        workflow.openRoom(
            origin: .pairingCode,
            pairingInput: "123456-alpha-bravo",
            existingActivityIDs: []
        )
        XCTAssertEqual(workflow.controlPhase, .idle)
        XCTAssertEqual(workflow.room?.origin, .pairingCode)
    }

    func testAcceptClaimsIncomingOfferBeforeDeadlineTickCanRejectIt() async throws {
        let start = Date(timeIntervalSince1970: 3_000)
        let gateway = RecordingRoomControlGateway()
        gateway.suspendAcceptance = true
        let workflow = ConnectionWorkflowState(gateway: gateway, clock: { start })
        _ = workflow.startHosting(
            broker: "",
            relay: "",
            displayName: "My iPhone",
            identityPath: "/tmp/envoix-test-identity",
            existingActivityIDs: []
        )
        await Task.yield()
        gateway.emit(.connected(peerDisplayName: "Peer", creator: true))
        await Task.yield()
        gateway.emit(.incomingOffer(RoomControlTransferOffer(
            id: "offer-at-deadline",
            transferInvite: "envoix://pair/river-stone-test?role=send",
            rootNames: ["report.pdf"],
            itemCount: 1,
            totalBytes: 1_024
        )))
        await Task.yield()

        let acceptance = Task { await workflow.acceptIncomingRoomOffer() }
        await Task.yield()
        XCTAssertNil(workflow.incomingRoomOffer)

        workflow.tick(
            now: start.addingTimeInterval(ConnectionWorkflowPolicy.roomOfferLifetime),
            hasActiveTransfer: false
        )
        XCTAssertEqual(gateway.acceptedOfferIDs, ["offer-at-deadline"])
        XCTAssertTrue(gateway.rejectedOfferIDs.isEmpty)

        gateway.finishAcceptance()
        let accepted = await acceptance.value
        XCTAssertEqual(accepted?.id, "offer-at-deadline")
        XCTAssertEqual(workflow.controlPhase, .connected)
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

@MainActor
private final class RecordingRoomControlGateway: RoomControlGateway {
    private var eventHandler: ((RoomControlEvent) -> Void)?
    private var acceptanceContinuation: CheckedContinuation<Void, Never>?
    var suspendAcceptance = false
    private(set) var acceptedOfferIDs: [String] = []
    private(set) var rejectedOfferIDs: [String] = []

    func makeInvitation(broker: String, relay: String, now: Date) throws -> RoomControlInvitation {
        RoomControlInvitation(
            code: "R123456-test-room",
            payload: "envoix://room/R123456-test-room",
            expiresAt: now.addingTimeInterval(ConnectionWorkflowPolicy.roomInvitationLifetime)
        )
    }

    func parseInvitation(
        _ input: String,
        broker: String,
        relay: String,
        now: Date
    ) throws -> RoomControlInvitation {
        try makeInvitation(broker: "", relay: "", now: now)
    }

    func host(
        invitation: RoomControlInvitation,
        displayName: String,
        identityPath: String,
        onEvent: @escaping (RoomControlEvent) -> Void
    ) async throws {
        eventHandler = onEvent
    }

    func join(
        invitation: RoomControlInvitation,
        displayName: String,
        identityPath: String,
        onEvent: @escaping (RoomControlEvent) -> Void
    ) async throws {
        eventHandler = onEvent
    }

    func offerTransfer(_ offer: RoomControlTransferOffer) async throws {}
    func acceptOffer(id: String) async throws {
        acceptedOfferIDs.append(id)
        if suspendAcceptance {
            await withCheckedContinuation { continuation in
                acceptanceContinuation = continuation
            }
        }
    }

    func rejectOffer(id: String) async throws {
        rejectedOfferIDs.append(id)
    }
    func setLifetimePolicy(_ policy: RoomControlLifetimePolicy) async throws {}
    func close(reason: RoomControlCloseReason) {}

    func emit(_ event: RoomControlEvent) {
        eventHandler?(event)
    }

    func finishAcceptance() {
        acceptanceContinuation?.resume()
        acceptanceContinuation = nil
    }
}
