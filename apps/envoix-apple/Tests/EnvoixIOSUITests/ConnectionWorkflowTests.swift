import EnvoixCore
import XCTest
@testable import Envoix_iOS

@MainActor
final class ConnectionWorkflowTests: XCTestCase {
    func testInviteJoinerRoleSelectsTheLocalTransferAdapter() {
        XCTAssertEqual(ConnectionWorkflowPolicy.localAction(forLocalRole: .send), .offerFiles)
        XCTAssertEqual(ConnectionWorkflowPolicy.localAction(forLocalRole: .receive), .receiveFiles)
    }

    func testLinkedCoreExposesTheExpectedRoomControlContract() {
        let info = envoixCoreInfo()

        XCTAssertEqual(info.ffiApiVersion, expectedCoreFFIAPIVersion)
        XCTAssertEqual(expectedCoreFFIAPIVersion, 8)
        XCTAssertTrue(info.capabilities.contains(expectedRoomControlCoreCapability))
        XCTAssertEqual(expectedRoomControlCoreCapability, "foreground_room_control_v3")
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

        gateway.emit(.connected(
            peerDisplayName: "Other phone",
            creator: true,
            lifetime: lifetime(revision: 1, deadline: Date(timeIntervalSince1970: 1_900))
        ))
        await Task.yield()

        XCTAssertEqual(workflow.controlPhase, .connected)
        XCTAssertEqual(workflow.peerDisplayName, "Other phone")
        XCTAssertTrue(workflow.isRoomCreator)
        XCTAssertEqual(workflow.room?.origin, .roomControl)
    }

    func testFailedHostingRefreshPreservesCurrentInvitation() async {
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
            existingActivityIDs: []
        ))
        let currentInvitation = workflow.roomInvitation
        let closeCount = gateway.closeReasons.count
        gateway.invitationError = RuntimeSettingsError("refresh failed")

        let error = workflow.refreshHosting(
            broker: "",
            relay: "",
            displayName: "My iPhone",
            identityPath: "/tmp/envoix-test-identity",
            existingActivityIDs: []
        )

        XCTAssertEqual(error, "refresh failed")
        XCTAssertEqual(workflow.controlPhase, .hosting)
        XCTAssertEqual(workflow.roomInvitation, currentInvitation)
        XCTAssertEqual(gateway.closeReasons.count, closeCount)
    }

    func testCreatorExpiresOnlyAtAuthoritativeIdleDeadline() async {
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
        let boundary = start.addingTimeInterval(ConnectionWorkflowPolicy.roomIdleLifetime)
        gateway.currentLifetime = lifetime(revision: 4, deadline: boundary)
        gateway.emit(.connected(
            peerDisplayName: "Peer",
            creator: true,
            lifetime: gateway.currentLifetime
        ))
        await Task.yield()

        workflow.tick(now: boundary, hasActiveTransfer: true)
        XCTAssertEqual(workflow.controlPhase, .connected)
        XCTAssertEqual(gateway.idleExpiryAttempts, 0)

        workflow.tick(now: boundary.addingTimeInterval(-1), hasActiveTransfer: false)
        XCTAssertEqual(workflow.controlPhase, .connected)
        XCTAssertEqual(gateway.idleExpiryAttempts, 0)

        workflow.tick(now: boundary, hasActiveTransfer: false)
        await Task.yield()

        XCTAssertEqual(gateway.idleExpiryAttempts, 1)
        XCTAssertFalse(gateway.closeReasons.contains(.idleExpired))
        XCTAssertEqual(workflow.controlPhase, .ended(.idleExpired))
    }

    func testJoinerNeverExpiresTheCreatorsDeadline() async {
        let deadline = Date(timeIntervalSince1970: 3_000)
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
        gateway.currentLifetime = lifetime(revision: 2, deadline: deadline)
        gateway.emit(.connected(
            peerDisplayName: "Creator",
            creator: false,
            lifetime: gateway.currentLifetime
        ))
        await Task.yield()

        workflow.tick(now: deadline.addingTimeInterval(30), hasActiveTransfer: false)
        await Task.yield()

        XCTAssertEqual(gateway.idleExpiryAttempts, 0)
        XCTAssertEqual(workflow.controlPhase, .connected)
    }

    func testJoinerMayCloseRoomWhenAppBackgrounds() async {
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
        gateway.emit(.connected(
            peerDisplayName: "Creator",
            creator: false,
            lifetime: lifetime(revision: 1)
        ))
        await Task.yield()

        workflow.endControl(reason: .backgrounded)

        XCTAssertEqual(workflow.controlPhase, .ended(.backgrounded))
        XCTAssertEqual(gateway.closeReasons.last, .backgrounded)
        XCTAssertEqual(gateway.closeReasons.filter { $0 == .backgrounded }.count, 1)
    }

    func testLifetimeReducerIgnoresStaleRevisions() async {
        let originalDeadline = Date(timeIntervalSince1970: 4_000)
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
        gateway.emit(.connected(
            peerDisplayName: "Peer",
            creator: false,
            lifetime: lifetime(revision: 5, deadline: originalDeadline)
        ))
        await Task.yield()

        gateway.emit(.lifetimeChanged(RoomControlLifetimeState(
            revision: 4,
            policy: .untilForegroundEnds,
            idleDeadline: nil
        )))
        await Task.yield()

        XCTAssertEqual(workflow.roomLifetimePolicy, .idleFifteenMinutes)
        XCTAssertEqual(workflow.idleDeadline, originalDeadline)

        gateway.emit(.lifetimeChanged(RoomControlLifetimeState(
            revision: 6,
            policy: .untilForegroundEnds,
            idleDeadline: nil
        )))
        await Task.yield()

        XCTAssertEqual(workflow.roomLifetimePolicy, .untilForegroundEnds)
        XCTAssertNil(workflow.idleDeadline)
    }

    func testLocalTransferEdgesApplyTheCreatorsReturnedLifetime() async {
        let initialDeadline = Date(timeIntervalSince1970: 4_500)
        let resumedDeadline = initialDeadline.addingTimeInterval(900)
        let gateway = RecordingRoomControlGateway()
        gateway.localTransferLifetime = { active in
            RoomControlLifetimeState(
                revision: active ? 2 : 3,
                policy: .idleFifteenMinutes,
                idleDeadline: active ? nil : resumedDeadline
            )
        }
        let workflow = ConnectionWorkflowState(gateway: gateway)
        _ = workflow.startHosting(
            broker: "",
            relay: "",
            displayName: "My iPhone",
            identityPath: "/tmp/envoix-test-identity",
            existingActivityIDs: []
        )
        await Task.yield()
        gateway.emit(.connected(
            peerDisplayName: "Peer",
            creator: true,
            lifetime: lifetime(revision: 1, deadline: initialDeadline)
        ))
        await Task.yield()

        workflow.setLocalTransferActive(true)
        await Task.yield()
        XCTAssertEqual(gateway.localTransferStates, [true])
        XCTAssertNil(workflow.idleDeadline)

        workflow.setLocalTransferActive(false)
        await Task.yield()
        XCTAssertEqual(gateway.localTransferStates, [true, false])
        XCTAssertEqual(workflow.idleDeadline, resumedDeadline)
    }

    func testRejectedIdleCloseKeepsRoomAndAppliesNewerLifetime() async {
        let deadline = Date(timeIntervalSince1970: 5_000)
        let gateway = RecordingRoomControlGateway()
        gateway.rejectIdleExpiry = true
        let workflow = ConnectionWorkflowState(gateway: gateway)
        _ = workflow.startHosting(
            broker: "",
            relay: "",
            displayName: "My iPhone",
            identityPath: "/tmp/envoix-test-identity",
            existingActivityIDs: []
        )
        await Task.yield()
        gateway.currentLifetime = lifetime(revision: 8, deadline: deadline)
        gateway.emit(.connected(
            peerDisplayName: "Peer",
            creator: true,
            lifetime: gateway.currentLifetime
        ))
        await Task.yield()

        workflow.tick(now: deadline, hasActiveTransfer: false)
        await Task.yield()

        XCTAssertEqual(gateway.idleExpiryAttempts, 1)
        XCTAssertEqual(workflow.controlPhase, .connected)
        XCTAssertEqual(workflow.idleDeadline, deadline)

        workflow.tick(now: deadline.addingTimeInterval(1), hasActiveTransfer: false)
        await Task.yield()
        XCTAssertEqual(gateway.idleExpiryAttempts, 2)

        gateway.currentLifetime = RoomControlLifetimeState(
            revision: 9,
            policy: .idleFifteenMinutes,
            idleDeadline: nil
        )
        workflow.tick(now: deadline.addingTimeInterval(2), hasActiveTransfer: false)
        await Task.yield()

        XCTAssertEqual(gateway.idleExpiryAttempts, 3)
        XCTAssertEqual(workflow.controlPhase, .connected)
        XCTAssertNil(workflow.idleDeadline)
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
        gateway.emit(.connected(
            peerDisplayName: "Late peer",
            creator: true,
            lifetime: lifetime(revision: 1)
        ))
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
        gateway.emit(.connected(
            peerDisplayName: "Peer",
            creator: true,
            lifetime: lifetime(revision: 1)
        ))
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

    func testRoomOfferAcceptanceWaitsForExplicitReceiverLaunchSignal() async {
        let launch = ControlledReceiverLaunch()
        var didAcknowledge = false

        let acceptance = Task { @MainActor in
            await RoomOfferAcceptanceCoordinator.startReceiverThenAccept(
                startReceiver: {
                    await launch.waitForSignal()
                },
                acceptOffer: {
                    didAcknowledge = true
                    return true
                },
                cancelReceiver: { _ in }
            )
        }

        await launch.waitUntilReceiverIsWaiting()
        XCTAssertFalse(didAcknowledge)

        launch.signal(activityID: "receive-activity")
        let result = await acceptance.value

        XCTAssertEqual(result, .accepted(activityID: "receive-activity"))
        XCTAssertTrue(didAcknowledge)
    }

    func testRoomOfferAcceptanceCancelsReceiverWhenOfferDisappears() async {
        var cancelledActivityID: String?

        let result = await RoomOfferAcceptanceCoordinator.startReceiverThenAccept(
            startReceiver: { "receive-activity" },
            acceptOffer: { false },
            cancelReceiver: { cancelledActivityID = $0 }
        )

        XCTAssertEqual(result, .offerUnavailable(activityID: "receive-activity"))
        XCTAssertEqual(cancelledActivityID, "receive-activity")
    }

    func testRoomOfferAcceptanceDoesNotAcknowledgeWhenReceiverCannotStart() async {
        var didAcknowledge = false

        let result = await RoomOfferAcceptanceCoordinator.startReceiverThenAccept(
            startReceiver: { nil },
            acceptOffer: {
                didAcknowledge = true
                return true
            },
            cancelReceiver: { _ in }
        )

        XCTAssertEqual(result, .receiverDidNotStart)
        XCTAssertFalse(didAcknowledge)
    }

    private func offer(id: String, invitationID: String = "default") -> NearbyRendezvousOffer {
        NearbyRendezvousOffer(
            requestID: id,
            senderPeerKey: "0011223344556677",
            senderDisplayName: "Nearby phone",
            invite: "envoix://pair/river-stone-\(invitationID)?role=send"
        )
    }

    private func lifetime(
        revision: UInt64,
        policy: RoomControlLifetimePolicy = .idleFifteenMinutes,
        deadline: Date? = nil
    ) -> RoomControlLifetimeState {
        RoomControlLifetimeState(
            revision: revision,
            policy: policy,
            idleDeadline: deadline
        )
    }
}

@MainActor
private final class ControlledReceiverLaunch {
    private var isWaiting = false
    private var receiverContinuation: CheckedContinuation<String?, Never>?
    private var observerContinuation: CheckedContinuation<Void, Never>?

    func waitForSignal() async -> String? {
        isWaiting = true
        observerContinuation?.resume()
        observerContinuation = nil
        return await withCheckedContinuation { continuation in
            receiverContinuation = continuation
        }
    }

    func waitUntilReceiverIsWaiting() async {
        guard !isWaiting else { return }
        await withCheckedContinuation { continuation in
            observerContinuation = continuation
        }
    }

    func signal(activityID: String?) {
        receiverContinuation?.resume(returning: activityID)
        receiverContinuation = nil
    }
}

@MainActor
private final class RecordingRoomControlGateway: RoomControlGateway {
    private var eventHandler: ((RoomControlEvent) -> Void)?
    private var acceptanceContinuation: CheckedContinuation<Void, Never>?
    var suspendAcceptance = false
    var rejectIdleExpiry = false
    var invitationError: Error?
    var localTransferLifetime: ((Bool) -> RoomControlLifetimeState?)?
    var currentLifetime = RoomControlLifetimeState(
        revision: 0,
        policy: .idleFifteenMinutes,
        idleDeadline: nil
    )
    private(set) var acceptedOfferIDs: [String] = []
    private(set) var rejectedOfferIDs: [String] = []
    private(set) var localTransferStates: [Bool] = []
    private(set) var idleExpiryAttempts = 0
    private(set) var closeReasons: [RoomControlCloseReason] = []

    func makeInvitation(broker: String, relay: String, now: Date) throws -> RoomControlInvitation {
        if let invitationError {
            throw invitationError
        }
        return RoomControlInvitation(
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

    func offerTransfer(
        _ offer: RoomControlTransferOffer
    ) async throws -> RoomControlLifetimeState? {
        nil
    }

    func acceptOffer(id: String) async throws -> RoomControlLifetimeState? {
        acceptedOfferIDs.append(id)
        if suspendAcceptance {
            await withCheckedContinuation { continuation in
                acceptanceContinuation = continuation
            }
        }
        return nil
    }

    func rejectOffer(id: String) async throws -> RoomControlLifetimeState? {
        rejectedOfferIDs.append(id)
        return nil
    }

    func setLifetimePolicy(
        _ policy: RoomControlLifetimePolicy
    ) async throws -> RoomControlLifetimeState? {
        currentLifetime = RoomControlLifetimeState(
            revision: currentLifetime.revision + 1,
            policy: policy,
            idleDeadline: nil
        )
        return currentLifetime
    }

    func setLocalTransferActive(_ active: Bool) async throws -> RoomControlLifetimeState? {
        localTransferStates.append(active)
        return localTransferLifetime?(active)
    }

    func lifetimeSnapshot() -> RoomControlLifetimeState? {
        currentLifetime
    }

    func expireIdleDeadline() async throws {
        idleExpiryAttempts += 1
        if rejectIdleExpiry {
            throw RuntimeSettingsError("authoritative deadline changed")
        }
    }

    func close(reason: RoomControlCloseReason) {
        closeReasons.append(reason)
    }

    func emit(_ event: RoomControlEvent) {
        eventHandler?(event)
    }

    func finishAcceptance() {
        acceptanceContinuation?.resume()
        acceptanceContinuation = nil
    }
}
