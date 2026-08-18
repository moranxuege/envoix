import EnvoixCore
import XCTest
@testable import Envoix_iOS

@MainActor
final class ConnectionWorkflowTests: XCTestCase {
    func testRoomControlReceiverInviteRetainsRoomRendezvousForWifiAware() {
        let invite = TransferViewModel.rendezvousPlan(for: .invite)
        XCTAssertTrue(invite.useRoom)
        XCTAssertFalse(invite.useMdns)
        XCTAssertTrue(invite.internetAvailable)

        let room = TransferViewModel.rendezvousPlan(for: .room)
        XCTAssertTrue(room.useRoom)
        XCTAssertFalse(room.useMdns)

        let mdns = TransferViewModel.rendezvousPlan(for: .mdns)
        XCTAssertFalse(mdns.useRoom)
        XCTAssertTrue(mdns.useMdns)
    }

    func testInviteJoinerRoleSelectsTheLocalTransferAdapter() {
        XCTAssertEqual(ConnectionWorkflowPolicy.localAction(forLocalRole: .send), .offerFiles)
        XCTAssertEqual(ConnectionWorkflowPolicy.localAction(forLocalRole: .receive), .receiveFiles)
    }

    func testPendingSharedSendWaitsForAnAuthenticatedRoom() {
        XCTAssertEqual(
            ConnectionWorkflowPolicy.pendingSharedSendDestination(
                hasPendingSelection: true,
                sendIsBusy: false,
                transferIsPresented: false,
                selectionWasPresented: false,
                hasConnectedOneTimeRoom: false,
                hasConnectedRememberedRoom: false
            ),
            .connectionHub
        )
    }

    func testPendingSharedSendUsesTheConnectedRoomWithoutCreatingAnotherOne() {
        XCTAssertEqual(
            ConnectionWorkflowPolicy.pendingSharedSendDestination(
                hasPendingSelection: true,
                sendIsBusy: false,
                transferIsPresented: false,
                selectionWasPresented: false,
                hasConnectedOneTimeRoom: true,
                hasConnectedRememberedRoom: false
            ),
            .oneTimeRoom
        )
        XCTAssertEqual(
            ConnectionWorkflowPolicy.pendingSharedSendDestination(
                hasPendingSelection: true,
                sendIsBusy: false,
                transferIsPresented: false,
                selectionWasPresented: false,
                hasConnectedOneTimeRoom: false,
                hasConnectedRememberedRoom: true
            ),
            .rememberedRoom
        )
    }

    func testPendingSharedSendDoesNotPresentTwiceOrInterruptASend() {
        XCTAssertEqual(
            ConnectionWorkflowPolicy.pendingSharedSendDestination(
                hasPendingSelection: true,
                sendIsBusy: false,
                transferIsPresented: true,
                selectionWasPresented: false,
                hasConnectedOneTimeRoom: true,
                hasConnectedRememberedRoom: false
            ),
            .none
        )
        XCTAssertEqual(
            ConnectionWorkflowPolicy.pendingSharedSendDestination(
                hasPendingSelection: true,
                sendIsBusy: true,
                transferIsPresented: false,
                selectionWasPresented: false,
                hasConnectedOneTimeRoom: true,
                hasConnectedRememberedRoom: false
            ),
            .none
        )
        XCTAssertEqual(
            ConnectionWorkflowPolicy.pendingSharedSendDestination(
                hasPendingSelection: true,
                sendIsBusy: false,
                transferIsPresented: false,
                selectionWasPresented: true,
                hasConnectedOneTimeRoom: true,
                hasConnectedRememberedRoom: false
            ),
            .none
        )
    }

    func testExternalInvitationRequiresOneExplicitConfirmationAndDeduplicates() {
        let invitation = "envoix://room/123456-a1b2-c3d4"
        XCTAssertTrue(ExternalInvitationRoutingPolicy.shouldStage(
            invitation: invitation,
            pendingInvitation: nil,
            openedInvitation: nil
        ))
        XCTAssertFalse(ExternalInvitationRoutingPolicy.shouldStage(
            invitation: invitation,
            pendingInvitation: invitation,
            openedInvitation: nil
        ))
        XCTAssertFalse(ExternalInvitationRoutingPolicy.shouldStage(
            invitation: invitation,
            pendingInvitation: nil,
            openedInvitation: invitation
        ))
        XCTAssertTrue(ExternalInvitationRoutingPolicy.shouldStage(
            invitation: "envoix://room/654321-d4c3-b2a1",
            pendingInvitation: nil,
            openedInvitation: invitation
        ))
    }

    func testNFCAutomaticReadRequiresForegroundConnectAndBoundBluetoothPeer() {
        var gate = NFCInvitationReadinessGate()
        let offer = nfcReadinessOffer(
            id: "0011223344556677",
            peerKey: "8899aabbccddeeff",
            firstSeenAtMilliseconds: 100
        )

        XCTAssertFalse(gate.claim(
            offer: offer,
            nowMilliseconds: 100,
            applicationIsActive: false,
            isConnectPage: true,
            eligibleBluetoothPeerKeys: [offer.presenterPeerKey]
        ))
        XCTAssertFalse(gate.claim(
            offer: offer,
            nowMilliseconds: 100,
            applicationIsActive: true,
            isConnectPage: false,
            eligibleBluetoothPeerKeys: [offer.presenterPeerKey]
        ))
        XCTAssertFalse(gate.claim(
            offer: offer,
            nowMilliseconds: 100,
            applicationIsActive: true,
            isConnectPage: true,
            eligibleBluetoothPeerKeys: ["0011223344556677"]
        ))
        XCTAssertTrue(gate.claim(
            offer: offer,
            nowMilliseconds: 100,
            applicationIsActive: true,
            isConnectPage: true,
            eligibleBluetoothPeerKeys: [offer.presenterPeerKey]
        ))
    }

    func testNFCAutomaticReadIsOneShotPerConnectActivationAndRateLimited() {
        var gate = NFCInvitationReadinessGate()
        let first = nfcReadinessOffer(
            id: "0011223344556677",
            firstSeenAtMilliseconds: 1_000
        )
        let second = nfcReadinessOffer(
            id: "8899aabbccddeeff",
            firstSeenAtMilliseconds: 1_001
        )

        XCTAssertTrue(claimNFCRead(
            with: &gate,
            offer: first,
            nowMilliseconds: 1_000
        ))
        XCTAssertFalse(claimNFCRead(
            with: &gate,
            offer: second,
            nowMilliseconds: 1_001
        ))

        gate.didLeaveConnectPage()
        let afterCooldown = nfcReadinessOffer(
            id: "1021324354657687",
            firstSeenAtMilliseconds:
                1_000
                + NFCInvitationReadinessGate
                    .automaticReadCooldownMilliseconds - 1_000
        )
        XCTAssertFalse(claimNFCRead(
            with: &gate,
            offer: afterCooldown,
            nowMilliseconds:
                1_000
                + NFCInvitationReadinessGate
                    .automaticReadCooldownMilliseconds - 1
        ))
        XCTAssertTrue(claimNFCRead(
            with: &gate,
            offer: afterCooldown,
            nowMilliseconds:
                1_000
                + NFCInvitationReadinessGate
                    .automaticReadCooldownMilliseconds
        ))
    }

    func testNFCReadinessRejectsReplayStaleOfferAndManualReadUsesActivation() {
        var gate = NFCInvitationReadinessGate()
        let first = nfcReadinessOffer(
            id: "0011223344556677",
            firstSeenAtMilliseconds: 100
        )

        XCTAssertTrue(claimNFCRead(
            with: &gate,
            offer: first,
            nowMilliseconds: 100
        ))
        gate.didLeaveConnectPage()
        let replay = nfcReadinessOffer(
            id: first.id,
            firstSeenAtMilliseconds:
                100
                + NFCInvitationReadinessGate.automaticReadCooldownMilliseconds
        )
        XCTAssertFalse(claimNFCRead(
            with: &gate,
            offer: replay,
            nowMilliseconds:
                100
                + NFCInvitationReadinessGate.automaticReadCooldownMilliseconds
        ))

        var manualGate = NFCInvitationReadinessGate()
        manualGate.didBeginManualRead()
        XCTAssertFalse(claimNFCRead(
            with: &manualGate,
            offer: first,
            nowMilliseconds: 100
        ))

        let stale = nfcReadinessOffer(
            id: "8899aabbccddeeff",
            firstSeenAtMilliseconds: 100
        )
        var staleGate = NFCInvitationReadinessGate()
        XCTAssertFalse(claimNFCRead(
            with: &staleGate,
            offer: stale,
            nowMilliseconds:
                100 + NearbyNFCReadinessOffer.lifetimeMilliseconds
        ))
    }

    func testRememberedGenerationSweepNeverProbesCurrentTwice() {
        XCTAssertEqual(
            ConnectionWorkflowPolicy.rememberedGenerationSchedule(
                current: 9,
                previous: 8,
                mode: .connector
            ),
            [9, 8]
        )
        XCTAssertEqual(
            ConnectionWorkflowPolicy.rememberedGenerationSchedule(
                current: 9,
                previous: 9,
                mode: .responder
            ),
            [9]
        )
        XCTAssertEqual(
            ConnectionWorkflowPolicy.rememberedGenerationSchedule(
                current: 9,
                previous: nil,
                mode: .responder
            ),
            [9]
        )
    }

    private func nfcReadinessOffer(
        id: String,
        peerKey: String = "8899aabbccddeeff",
        firstSeenAtMilliseconds: Int64
    ) -> NearbyNFCReadinessOffer {
        NearbyNFCReadinessOffer(
            id: id,
            presenterPeerKey: peerKey,
            presenterID: UUID(
                uuidString: "00112233-4455-6677-8899-aabbccddeeff"
            )!,
            firstSeenAtMilliseconds: firstSeenAtMilliseconds
        )
    }

    private func claimNFCRead(
        with gate: inout NFCInvitationReadinessGate,
        offer: NearbyNFCReadinessOffer,
        nowMilliseconds: Int64
    ) -> Bool {
        gate.claim(
            offer: offer,
            nowMilliseconds: nowMilliseconds,
            applicationIsActive: true,
            isConnectPage: true,
            eligibleBluetoothPeerKeys: [offer.presenterPeerKey]
        )
    }

    func testRememberedReconnectPolicyUsesPositiveBoundedJitter() {
        let policy = RememberedRoomReconnectPolicy.live

        XCTAssertEqual(policy.connectorAttemptTimeout, 75)
        XCTAssertEqual(policy.responderAttemptTimeout, 240)
        XCTAssertEqual(policy.sameLocatorCooldown, 6)
        XCTAssertEqual(policy.passiveConnectedDwell, 45)
        XCTAssertEqual(policy.delay(failureCount: 1, jitterUnit: 0), 30)
        XCTAssertGreaterThan(policy.delay(failureCount: 1, jitterUnit: 1), 30)
        XCTAssertEqual(policy.delay(failureCount: 7, jitterUnit: 1), 300)
        XCTAssertEqual(policy.collisionDelay(jitterUnit: 0), 1)
        XCTAssertEqual(policy.collisionDelay(jitterUnit: 1), 6)
        XCTAssertEqual(
            policy.requiredCooldown(
                failureCode: .roomExpired,
                retryAfterSeconds: nil
            ),
            300
        )
        XCTAssertEqual(
            policy.requiredCooldown(
                failureCode: .serverBusy,
                retryAfterSeconds: 17
            ),
            17
        )
        XCTAssertEqual(
            policy.requiredCooldown(
                failureCode: .serverBusy,
                retryAfterSeconds: UInt64.max
            ),
            300
        )
    }

    func testRememberedStoreReturnsConsistentMaterialAndProtectsActiveLease() throws {
        let credentials = InMemoryRememberedCredentialStore()
        let metadataURL = FileManager.default.temporaryDirectory
            .appendingPathComponent(UUID().uuidString, isDirectory: true)
            .appendingPathComponent("remembered.json")
        let store = RememberedPeerStore(
            credentialStore: credentials,
            metadataFileURL: metadataURL
        )
        let pending = try store.prepare(
            label: "Other phone",
            broker: "udp://broker.example:8555",
            relay: ""
        )
        let credential = Data([1, 2, 3, 4])

        XCTAssertThrowsError(try store.create(
            pending,
            opaqueCredential: credential,
            generation: 7
        )) { error in
            guard case RememberedPeerStoreError.inactiveSession = error else {
                return XCTFail("Expected inactive-session protection, got \(error)")
            }
        }

        try store.acquireSession(pending.relationshipID)
        try store.create(pending, opaqueCredential: credential, generation: 7)
        XCTAssertEqual(
            try store.sessionMaterial(relationshipID: pending.relationshipID),
            RememberedPeerSessionMaterial(
                summary: RememberedPeerSummary(
                    relationshipID: pending.relationshipID,
                    label: "Other phone",
                    generation: 7,
                    previousGeneration: nil,
                    broker: "udp://broker.example:8555",
                    relay: ""
                ),
                opaqueCredential: credential
            )
        )

        try store.rotate(
            relationshipID: pending.relationshipID,
            opaqueCredential: credential,
            generation: 8
        )
        let rotated = try store.sessionMaterial(relationshipID: pending.relationshipID)
        XCTAssertEqual(rotated.summary.generation, 8)
        XCTAssertEqual(rotated.summary.previousGeneration, 7)
        XCTAssertEqual(rotated.opaqueCredential, credential)
        XCTAssertThrowsError(try store.delete(rotated.summary)) { error in
            guard case RememberedPeerStoreError.activeTransfer = error else {
                return XCTFail("Expected active-lease protection, got \(error)")
            }
        }

        store.releaseSession(pending.relationshipID)
        try store.delete(rotated.summary)
        XCTAssertTrue(try store.peers().isEmpty)
        try? FileManager.default.removeItem(
            at: metadataURL.deletingLastPathComponent()
        )
    }

    func testRememberedRoomRotatesGenerationBeforePublishingConnected() async throws {
        let credentials = InMemoryRememberedCredentialStore()
        let metadataURL = FileManager.default.temporaryDirectory
            .appendingPathComponent(UUID().uuidString, isDirectory: true)
            .appendingPathComponent("remembered.json")
        let store = RememberedPeerStore(
            credentialStore: credentials,
            metadataFileURL: metadataURL
        )
        let pending = try store.prepare(
            label: "Other phone",
            broker: "udp://broker.example:8555",
            relay: ""
        )
        let credential = validRememberedCredential(secretByte: 4)
        try store.acquireSession(pending.relationshipID)
        try store.create(pending, opaqueCredential: credential, generation: 11)
        store.releaseSession(pending.relationshipID)

        let gateway = RecordingRoomControlGateway()
        gateway.rememberedConnectHandler = { attempt, _, beforeConnected, onEvent in
            try beforeConnected(attempt.generation)
            let persisted = try store.sessionMaterial(
                relationshipID: pending.relationshipID
            )
            XCTAssertEqual(persisted.summary.generation, 12)
            XCTAssertEqual(persisted.summary.previousGeneration, 11)
            onEvent(.connected(
                peerDisplayName: "Other phone",
                creator: false,
                lifetime: RoomControlLifetimeState(
                    revision: 1,
                    policy: .untilForegroundEnds,
                    idleDeadline: nil
                )
            ))
            try await Task.sleep(nanoseconds: 60_000_000_000)
        }
        let workflow = ConnectionWorkflowState(
            gateway: gateway,
            rememberedStore: store,
            jitterUnit: { 0.5 }
        )
        workflow.refreshRememberedRooms()
        XCTAssertNil(workflow.openRememberedRoom(
            relationshipID: pending.relationshipID,
            existingActivityIDs: []
        ))
        workflow.setRememberedReconnectEnabled(
            true,
            displayName: "My iPhone",
            identityPath: "/tmp/envoix-test-identity"
        )

        for _ in 0..<100 where workflow.controlPhase != .connected {
            try await Task.sleep(nanoseconds: 10_000_000)
        }

        XCTAssertEqual(workflow.controlPhase, .connected)
        XCTAssertEqual(
            workflow.activeRememberedRelationshipID,
            pending.relationshipID
        )
        workflow.setRememberedReconnectEnabled(
            false,
            displayName: "My iPhone",
            identityPath: "/tmp/envoix-test-identity"
        )
        try? FileManager.default.removeItem(
            at: metadataURL.deletingLastPathComponent()
        )
    }

    func testPassiveRememberedConnectionRotatesToAnotherSavedRoom() async throws {
        let credentials = InMemoryRememberedCredentialStore()
        let metadataURL = FileManager.default.temporaryDirectory
            .appendingPathComponent(UUID().uuidString, isDirectory: true)
            .appendingPathComponent("remembered.json")
        let store = RememberedPeerStore(
            credentialStore: credentials,
            metadataFileURL: metadataURL
        )
        for (label, broker, byte) in [
            ("Phone A", "udp://a.example:8555", UInt8(1)),
            ("Phone B", "udp://b.example:8555", UInt8(2)),
        ] {
            let pending = try store.prepare(label: label, broker: broker, relay: "")
            try store.acquireSession(pending.relationshipID)
            try store.create(
                pending,
                opaqueCredential: validRememberedCredential(secretByte: byte),
                generation: 1
            )
            store.releaseSession(pending.relationshipID)
        }

        let gateway = RecordingRoomControlGateway()
        gateway.rememberedConnectHandler = { attempt, _, beforeConnected, onEvent in
            try beforeConnected(attempt.generation)
            onEvent(.connected(
                peerDisplayName: attempt.endpoint.broker,
                creator: false,
                lifetime: RoomControlLifetimeState(
                    revision: 1,
                    policy: .untilForegroundEnds,
                    idleDeadline: nil
                )
            ))
            try await Task.sleep(nanoseconds: 60_000_000_000)
        }
        let policy = RememberedRoomReconnectPolicy(
            connectorAttemptTimeout: 1,
            responderAttemptTimeout: 1,
            sameLocatorCooldown: 0.001,
            minimumBackoff: 0.001,
            maximumBackoff: 0.01,
            passiveConnectedDwell: 0.03
        )
        let workflow = ConnectionWorkflowState(
            gateway: gateway,
            rememberedStore: store,
            reconnectPolicy: policy,
            jitterUnit: { 0 }
        )
        workflow.refreshRememberedRooms()
        workflow.setRememberedReconnectEnabled(
            true,
            displayName: "My iPhone",
            identityPath: "/tmp/envoix-test-identity"
        )

        for _ in 0..<200 {
            let brokers = Set(gateway.rememberedAttempts.map(\.endpoint.broker))
            if brokers.contains("udp://a.example:8555"),
               brokers.contains("udp://b.example:8555") {
                break
            }
            try await Task.sleep(nanoseconds: 10_000_000)
        }

        let attemptedBrokers = Set(gateway.rememberedAttempts.map(\.endpoint.broker))
        XCTAssertTrue(attemptedBrokers.contains("udp://a.example:8555"))
        XCTAssertTrue(attemptedBrokers.contains("udp://b.example:8555"))
        XCTAssertTrue(gateway.closeReasons.contains(.idleExpired))

        workflow.setRememberedReconnectEnabled(
            false,
            displayName: "My iPhone",
            identityPath: "/tmp/envoix-test-identity"
        )
        try? FileManager.default.removeItem(
            at: metadataURL.deletingLastPathComponent()
        )
    }

    func testQueuedWorkPreemptsDifferentIdlePassiveRoom() async throws {
        let credentials = InMemoryRememberedCredentialStore()
        let metadataURL = FileManager.default.temporaryDirectory
            .appendingPathComponent(UUID().uuidString, isDirectory: true)
            .appendingPathComponent("remembered.json")
        let store = RememberedPeerStore(
            credentialStore: credentials,
            metadataFileURL: metadataURL
        )
        var relationshipIDs: [String] = []
        for (label, broker, byte) in [
            ("Phone A", "udp://a.example:8555", UInt8(1)),
            ("Phone B", "udp://b.example:8555", UInt8(2)),
        ] {
            let pending = try store.prepare(label: label, broker: broker, relay: "")
            relationshipIDs.append(pending.relationshipID)
            try store.acquireSession(pending.relationshipID)
            try store.create(
                pending,
                opaqueCredential: validRememberedCredential(secretByte: byte),
                generation: 1
            )
            store.releaseSession(pending.relationshipID)
        }

        let gateway = RecordingRoomControlGateway()
        gateway.rememberedConnectHandler = { attempt, _, beforeConnected, onEvent in
            try beforeConnected(attempt.generation)
            onEvent(.connected(
                peerDisplayName: attempt.endpoint.broker,
                creator: false,
                lifetime: RoomControlLifetimeState(
                    revision: 1,
                    policy: .untilForegroundEnds,
                    idleDeadline: nil
                )
            ))
            try await Task.sleep(nanoseconds: 60_000_000_000)
        }
        let workflow = ConnectionWorkflowState(
            gateway: gateway,
            rememberedStore: store,
            reconnectPolicy: RememberedRoomReconnectPolicy(
                connectorAttemptTimeout: 1,
                responderAttemptTimeout: 1,
                sameLocatorCooldown: 0.001,
                minimumBackoff: 0.001,
                maximumBackoff: 0.01,
                passiveConnectedDwell: 30
            ),
            jitterUnit: { 0 }
        )
        workflow.refreshRememberedRooms()
        workflow.setRememberedReconnectEnabled(
            true,
            displayName: "My iPhone",
            identityPath: "/tmp/envoix-test-identity"
        )

        for _ in 0..<100 where workflow.controlPhase != .connected {
            try await Task.sleep(nanoseconds: 10_000_000)
        }
        let passiveRelationshipID = try XCTUnwrap(
            workflow.activeRememberedRelationshipID
        )
        let queuedRelationshipID = try XCTUnwrap(
            relationshipIDs.first { $0 != passiveRelationshipID }
        )
        workflow.setQueuedRememberedRelationships([queuedRelationshipID])

        for _ in 0..<100 where
            workflow.activeRememberedRelationshipID != queuedRelationshipID
                || workflow.controlPhase != .connected {
            try await Task.sleep(nanoseconds: 10_000_000)
        }

        XCTAssertEqual(
            workflow.activeRememberedRelationshipID,
            queuedRelationshipID
        )
        XCTAssertEqual(workflow.controlPhase, .connected)
        XCTAssertTrue(gateway.closeReasons.contains(.idleExpired))

        workflow.setRememberedReconnectEnabled(
            false,
            displayName: "My iPhone",
            identityPath: "/tmp/envoix-test-identity"
        )
        try? FileManager.default.removeItem(
            at: metadataURL.deletingLastPathComponent()
        )
    }

    func testRoomExpiredCurrentGenerationStillProbesPreviousGeneration() async throws {
        let credentials = InMemoryRememberedCredentialStore()
        let metadataURL = FileManager.default.temporaryDirectory
            .appendingPathComponent(UUID().uuidString, isDirectory: true)
            .appendingPathComponent("remembered.json")
        let store = RememberedPeerStore(
            credentialStore: credentials,
            metadataFileURL: metadataURL
        )
        let pending = try store.prepare(
            label: "Other phone",
            broker: "udp://broker.example:8555",
            relay: ""
        )
        let credential = validRememberedCredential(secretByte: 9)
        try store.acquireSession(pending.relationshipID)
        try store.create(pending, opaqueCredential: credential, generation: 10)
        try store.rotate(
            relationshipID: pending.relationshipID,
            opaqueCredential: credential,
            generation: 11
        )
        store.releaseSession(pending.relationshipID)

        let gateway = RecordingRoomControlGateway()
        gateway.rememberedConnectHandler = { attempt, _, _, _ in
            throw RememberedRoomConnectFailure(
                reason: attempt.generation == 11 ? "expired" : "not found",
                peerAuthenticated: false,
                failureCode: attempt.generation == 11
                    ? .roomExpired
                    : .roomNotFound
            )
        }
        let workflow = ConnectionWorkflowState(
            gateway: gateway,
            rememberedStore: store,
            jitterUnit: { 0.5 }
        )
        workflow.refreshRememberedRooms()
        XCTAssertNil(workflow.openRememberedRoom(
            relationshipID: pending.relationshipID,
            existingActivityIDs: []
        ))
        workflow.setRememberedReconnectEnabled(
            true,
            displayName: "My iPhone",
            identityPath: "/tmp/envoix-test-identity"
        )

        for _ in 0..<100 where gateway.rememberedAttempts.count < 2 {
            try await Task.sleep(nanoseconds: 10_000_000)
        }

        XCTAssertEqual(
            gateway.rememberedAttempts.map(\.generation),
            [11, 10]
        )
        workflow.setRememberedReconnectEnabled(
            false,
            displayName: "My iPhone",
            identityPath: "/tmp/envoix-test-identity"
        )
        try? FileManager.default.removeItem(
            at: metadataURL.deletingLastPathComponent()
        )
    }

    func testLinkedCoreExposesTheExpectedRoomControlContract() {
        let info = envoixCoreInfo()

        XCTAssertEqual(info.ffiApiVersion, expectedCoreFFIAPIVersion)
        XCTAssertEqual(expectedCoreFFIAPIVersion, 15)
        XCTAssertTrue(info.capabilities.contains(expectedRoomControlCoreCapability))
        XCTAssertEqual(expectedRoomControlCoreCapability, "foreground_room_control_v5")
        XCTAssertTrue(info.capabilities.contains(expectedNearbyInviteCoreCapability))
        XCTAssertEqual(expectedNearbyInviteCoreCapability, "nearby_invite_inbox_v1")
        XCTAssertTrue(info.capabilities.contains(expectedFailureProjectionCoreCapability))
        XCTAssertEqual(
            expectedFailureProjectionCoreCapability,
            "canonical_failure_projection_v1"
        )
    }

    func testBackgroundScenePreservesRoomWhileHidingInvitationAndDiscovery() {
        let effects = MobileSceneLifecyclePolicy.effects(for: .background)

        XCTAssertTrue(effects.shouldHideRoomInvitation)
        XCTAssertFalse(effects.allowsNearbyDiscovery)
        XCTAssertFalse(effects.shouldPresentPendingSendSelection)
    }

    func testActiveSceneRestoresForegroundPresentationAndDiscovery() {
        let effects = MobileSceneLifecyclePolicy.effects(for: .active)

        XCTAssertFalse(effects.shouldHideRoomInvitation)
        XCTAssertTrue(effects.allowsNearbyDiscovery)
        XCTAssertTrue(effects.shouldPresentPendingSendSelection)
    }

    func testSystemPairingKeepsNearbyDiscoverySuspendedInActiveScene() {
        XCTAssertFalse(NearbyDiscoveryLeasePolicy.shouldRun(
            sceneAllowsDiscovery: true,
            isConnectionPage: true,
            discoveryIsEnabled: true,
            systemPairingIsActive: true
        ))
        XCTAssertTrue(NearbyDiscoveryLeasePolicy.shouldRun(
            sceneAllowsDiscovery: true,
            isConnectionPage: true,
            discoveryIsEnabled: true,
            systemPairingIsActive: false
        ))
        XCTAssertFalse(NearbyDiscoveryLeasePolicy.shouldRun(
            sceneAllowsDiscovery: true,
            isConnectionPage: true,
            discoveryIsEnabled: false,
            systemPairingIsActive: false
        ))
    }

    func testRememberedRoomSurvivesAnExternalFilePicker() {
        XCTAssertTrue(RememberedRoomLifecyclePolicy.shouldKeepConnected(
            sceneIsActive: false,
            externalActivityActive: true
        ))
        XCTAssertFalse(RememberedRoomLifecyclePolicy.shouldKeepConnected(
            sceneIsActive: false,
            externalActivityActive: false
        ))
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
            pairingInput: "envoix://invite/v2/river-stone-next?role=send",
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
                for: FfiConnectionPathEvent(pathKind: .directIpv4, eventKind: .selected),
                language: "en"
            ),
            "Data path · Direct · IPv4"
        )
        XCTAssertEqual(
            ConnectionPathPresentationPolicy.label(
                for: FfiConnectionPathEvent(pathKind: .directIpv6, eventKind: .selected),
                language: "zh-Hans"
            ),
            "数据路径 · 直连 · IPv6"
        )
        XCTAssertEqual(
            ConnectionPathPresentationPolicy.label(
                for: FfiConnectionPathEvent(pathKind: .relay, eventKind: .changed),
                language: "zh-Hans"
            ),
            "数据路径 · 中继 · 已切换"
        )
        XCTAssertEqual(
            ConnectionPathPresentationPolicy.label(
                for: FfiConnectionPathEvent(pathKind: .wifiAware, eventKind: .selected),
                language: "en"
            ),
            "Data path · Wi‑Fi Aware"
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
        XCTAssertNil(workflow.room?.nearbySelection)
    }

    func testConnectedRoomRetainsItsAuthoritativeEndpoint() async {
        let gateway = RecordingRoomControlGateway()
        let workflow = ConnectionWorkflowState(gateway: gateway)
        let endpoint = RoomControlEndpoint(
            broker: "udp://room.example.test:8555",
            relay: ""
        )

        XCTAssertNil(workflow.startHosting(
            broker: endpoint.broker,
            relay: endpoint.relay,
            displayName: "My iPhone",
            identityPath: "/tmp/envoix-test-identity",
            existingActivityIDs: []
        ))
        await Task.yield()
        gateway.emit(.connected(
            peerDisplayName: "Other phone",
            creator: true,
            lifetime: lifetime(revision: 1)
        ))
        await Task.yield()

        XCTAssertEqual(workflow.room?.endpoint, endpoint)
        XCTAssertEqual(workflow.room?.endpoint?.relay, "")
    }

    func testNearbyRoomControlHostRetainsSelectionAndEndpointAfterConnected() async {
        let gateway = RecordingRoomControlGateway()
        let workflow = ConnectionWorkflowState(gateway: gateway)
        let selection = wifiAwareSelection()
        let endpoint = RoomControlEndpoint(
            broker: "udp://room.example.test:8555",
            relay: "https://relay.example.test"
        )

        XCTAssertNil(workflow.startHosting(
            broker: endpoint.broker,
            relay: endpoint.relay,
            displayName: "My iPhone",
            identityPath: "/tmp/envoix-test-identity",
            existingActivityIDs: [],
            nearbySelection: selection
        ))
        await Task.yield()
        gateway.emit(.connected(
            peerDisplayName: "Nearby iPad",
            creator: true,
            lifetime: lifetime(revision: 1)
        ))
        await Task.yield()

        XCTAssertEqual(workflow.room?.origin, .roomControl)
        XCTAssertEqual(workflow.room?.nearbySelection, selection)
        XCTAssertEqual(workflow.room?.endpoint, endpoint)
    }

    func testVerifiedNearbyHostPreparesProtectedPersistence() async {
        let gateway = RecordingRoomControlGateway()
        let workflow = ConnectionWorkflowState(gateway: gateway)
        let endpoint = RoomControlEndpoint(
            broker: "udp://room.example.test:8555",
            relay: "https://relay.example.test"
        )

        XCTAssertNil(workflow.startHosting(
            broker: endpoint.broker,
            relay: endpoint.relay,
            displayName: "My iPhone",
            identityPath: "/tmp/envoix-test-identity",
            existingActivityIDs: [],
            nearbySelection: wifiAwareSelection(),
            invitationInput: "envoix://room/123456-v165-4321",
            verifiedPeerLabel: "Nearby iPad"
        ))
        XCTAssertEqual(gateway.preparedVerification?.label, "Nearby iPad")
        XCTAssertEqual(gateway.preparedVerification?.endpoint, endpoint)
        await Task.yield()
    }

    func testOrdinaryRoomCanAuthorizePersistenceWithOneVerificationRequest() async {
        let gateway = RecordingRoomControlGateway()
        let workflow = ConnectionWorkflowState(gateway: gateway)
        let endpoint = RoomControlEndpoint(
            broker: "udp://room.example.test:8555",
            relay: "https://relay.example.test"
        )

        XCTAssertNil(workflow.joinRoomControl(
            input: "123456-test-room",
            broker: endpoint.broker,
            relay: endpoint.relay,
            displayName: "My Mac",
            identityPath: "/tmp/envoix-test-identity",
            existingActivityIDs: []
        ))
        await Task.yield()
        gateway.emit(.connected(
            peerDisplayName: "WSL",
            creator: false,
            lifetime: lifetime(revision: 1)
        ))
        await Task.yield()
        gateway.emit(.verificationRequested)
        await Task.yield()

        XCTAssertTrue(workflow.verificationRequested)
        XCTAssertNil(workflow.submitDeviceVerification("012345"))
        await Task.yield()

        XCTAssertEqual(gateway.preparedVerification?.label, "WSL")
        XCTAssertEqual(gateway.preparedVerification?.endpoint, endpoint)
        XCTAssertEqual(gateway.submittedVerificationCodes, ["012345"])
        XCTAssertFalse(workflow.verificationRequested)
    }

    func testHostingInvitationScopeCannotBeReassignedBeforeConnected() async {
        let gateway = RecordingRoomControlGateway()
        let workflow = ConnectionWorkflowState(gateway: gateway)
        let selectedPeer = wifiAwareSelection()
        let differentPeer = NearbyPairingSelection(
            discoveryPeerKey: "1021324354657687",
            displayName: "Different nearby device",
            sources: [.wifiAware],
            nearbyWifiAwareDeviceID: "0000000000000043"
        )
        let conflictingRoute = NearbyPairingSelection(
            discoveryPeerKey: selectedPeer.discoveryPeerKey,
            displayName: selectedPeer.displayName,
            sources: [.wifiAware],
            nearbyWifiAwareDeviceID: "0000000000000043"
        )

        XCTAssertNil(workflow.startHosting(
            broker: "udp://room.example.test:8555",
            relay: "",
            displayName: "My iPhone",
            identityPath: "/tmp/envoix-test-identity",
            existingActivityIDs: [],
            nearbySelection: selectedPeer
        ))
        XCTAssertTrue(workflow.canReuseHostingInvitation(for: selectedPeer))
        XCTAssertFalse(workflow.canReuseHostingInvitation(for: differentPeer))
        XCTAssertFalse(workflow.canReuseHostingInvitation(for: conflictingRoute))

        await Task.yield()
        gateway.emit(.connected(
            peerDisplayName: "Nearby iPad",
            creator: true,
            lifetime: lifetime(revision: 1)
        ))
        await Task.yield()

        XCTAssertEqual(workflow.room?.nearbySelection, selectedPeer)
    }

    func testGenericHostingInvitationCannotBeAnnotatedAsNearby() async {
        let gateway = RecordingRoomControlGateway()
        let workflow = ConnectionWorkflowState(gateway: gateway)
        let selection = wifiAwareSelection()

        XCTAssertNil(workflow.startHosting(
            broker: "udp://room.example.test:8555",
            relay: "",
            displayName: "My iPhone",
            identityPath: "/tmp/envoix-test-identity",
            existingActivityIDs: []
        ))
        XCTAssertFalse(workflow.canReuseHostingInvitation(for: selection))

        await Task.yield()
        gateway.emit(.connected(
            peerDisplayName: "Nearby iPad",
            creator: true,
            lifetime: lifetime(revision: 1)
        ))
        await Task.yield()

        XCTAssertNil(workflow.room?.nearbySelection)
    }

    func testNearbyRoomControlJoinRetainsSelectionAndEndpointAfterConnected() async {
        let gateway = RecordingRoomControlGateway()
        let workflow = ConnectionWorkflowState(gateway: gateway)
        let selection = wifiAwareSelection()
        let endpoint = RoomControlEndpoint(
            broker: "udp://room.example.test:8555",
            relay: "https://relay.example.test"
        )

        XCTAssertNil(workflow.joinRoomControl(
            input: "envoix://room/123456-test-room",
            broker: endpoint.broker,
            relay: endpoint.relay,
            displayName: "My iPhone",
            identityPath: "/tmp/envoix-test-identity",
            existingActivityIDs: [],
            nearbySelection: selection
        ))
        await Task.yield()
        gateway.emit(.connected(
            peerDisplayName: "Nearby iPad",
            creator: false,
            lifetime: lifetime(revision: 1)
        ))
        await Task.yield()

        XCTAssertEqual(workflow.room?.origin, .roomControl)
        XCTAssertEqual(workflow.room?.nearbySelection, selection)
        XCTAssertEqual(workflow.room?.endpoint, endpoint)
    }

    func testTransferInvitationEndpointPreservesAnEmptyRelay() {
        let invitation = FfiPairingInvite(
            roomCode: "123456-test-room",
            payload: "envoix://invite/v2/test",
            broker: "udp://room.example.test:8555",
            relayUrls: [],
            creatorRole: .send,
            joinerRole: .receive,
            expiresAt: 2_000
        )

        XCTAssertEqual(
            RoomControlEndpoint(transferInvitation: invitation),
            RoomControlEndpoint(
                broker: "udp://room.example.test:8555",
                relay: ""
            )
        )
    }

    func testTransferInvitationUsesItsEndpointWithoutChangingConfiguredEndpoint() throws {
        let suiteName = "TransferInvitationEndpointTests-\(UUID().uuidString)"
        let defaults = try XCTUnwrap(UserDefaults(suiteName: suiteName))
        defer { defaults.removePersistentDomain(forName: suiteName) }
        let configuredBroker = "configured-a.example.test:8555"
        let configuredRelay = "https://configured-a.example.test:8444"
        defaults.set(configuredBroker, forKey: "envoix.serverURL")
        defaults.set(configuredRelay, forKey: "envoix.relayURL")
        let invitation = FfiPairingInvite(
            roomCode: "123456-test-room",
            payload: "envoix://invite/v2/opaque",
            broker: "invite-b.example.test:8555",
            relayUrls: ["https://invite-b.example.test:8444"],
            creatorRole: .send,
            joinerRole: .receive,
            expiresAt: 2_000
        )

        let settings = try RuntimeSettingsProvider.make(
            transferInvitation: invitation,
            concurrentTransfers: false,
            language: "en",
            speedLimit: 0
        )

        XCTAssertEqual(settings.serverUrl, invitation.broker)
        XCTAssertEqual(settings.relayUrl, invitation.relayUrls[0])
        XCTAssertEqual(
            defaults.string(forKey: "envoix.serverURL"),
            configuredBroker
        )
        XCTAssertEqual(
            defaults.string(forKey: "envoix.relayURL"),
            configuredRelay
        )
    }

    func testDestinationRepairRequiresTheSameOfferAndRoom() {
        let roomID = UUID()
        let request = RoomDestinationRepairRequest(
            offerID: "offer-1",
            roomID: roomID
        )

        XCTAssertTrue(request.matches(offerID: "offer-1", roomID: roomID))
        XCTAssertFalse(request.matches(offerID: "offer-2", roomID: roomID))
        XCTAssertFalse(request.matches(offerID: "offer-1", roomID: UUID()))
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

    func testExplicitBackgroundCloseReasonStillClosesRoom() async {
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
            transferInvite: "envoix://invite/v2/river-stone-test?role=send",
            rootNames: ["report.pdf"],
            itemCount: 1,
            directoryCount: 0,
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

    func testAcceptedOfferIsHeldWhileReceiverListenerStarts() async throws {
        let start = Date(timeIntervalSince1970: 4_000)
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
        gateway.emit(.connected(
            peerDisplayName: "Peer",
            creator: true,
            lifetime: lifetime(revision: 1)
        ))
        await Task.yield()
        gateway.emit(.incomingOffer(RoomControlTransferOffer(
            id: "offer-being-prepared",
            transferInvite: "envoix://invite/v2/river-stone-test?role=send",
            rootNames: ["report.pdf"],
            itemCount: 1,
            directoryCount: 0,
            totalBytes: 1_024
        )))
        await Task.yield()

        XCTAssertTrue(
            workflow.holdIncomingRoomOfferForDestination(
                id: "offer-being-prepared"
            )
        )
        workflow.tick(
            now: start.addingTimeInterval(
                ConnectionWorkflowPolicy.roomOfferLifetime + 1
            ),
            hasActiveTransfer: false
        )
        await Task.yield()

        XCTAssertEqual(workflow.incomingRoomOffer?.id, "offer-being-prepared")
        XCTAssertTrue(gateway.rejectedOfferIDs.isEmpty)
        let accepted = await workflow.acceptIncomingRoomOffer()
        XCTAssertEqual(accepted?.id, "offer-being-prepared")
        XCTAssertEqual(gateway.acceptedOfferIDs, ["offer-being-prepared"])
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
            source: .bluetooth,
            senderInboxEndpointID: nil,
            invite: "envoix://invite/v2/river-stone-\(invitationID)?role=send"
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

    private func wifiAwareSelection() -> NearbyPairingSelection {
        NearbyPairingSelection(
            discoveryPeerKey: "8899aabbccddeeff",
            displayName: "Nearby iPad",
            sources: [.wifiAware],
            nearbyWifiAwareDeviceID: "0000000000000042"
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
    typealias RememberedConnectHandler = (
        RememberedRoomConnectAttempt,
        RememberedRoomConnectMode,
        (UInt64) throws -> Void,
        (RoomControlEvent) -> Void
    ) async throws -> Void

    private var eventHandler: ((RoomControlEvent) -> Void)?
    private var acceptanceContinuation: CheckedContinuation<Void, Never>?
    var suspendAcceptance = false
    var rejectIdleExpiry = false
    var invitationError: Error?
    var localTransferLifetime: ((Bool) -> RoomControlLifetimeState?)?
    var rememberedConnectHandler: RememberedConnectHandler?
    var currentLifetime = RoomControlLifetimeState(
        revision: 0,
        policy: .idleFifteenMinutes,
        idleDeadline: nil
    )
    private(set) var acceptedOfferIDs: [String] = []
    private(set) var rejectedOfferIDs: [String] = []
    private(set) var localTransferStates: [Bool] = []
    private(set) var rememberedAttempts: [RememberedRoomConnectAttempt] = []
    private(set) var idleExpiryAttempts = 0
    private(set) var closeReasons: [RoomControlCloseReason] = []
    private(set) var preparedVerification: (label: String, endpoint: RoomControlEndpoint)?
    private(set) var submittedVerificationCodes: [String] = []

    func makeInvitation(broker: String, relay: String, now: Date) throws -> RoomControlInvitation {
        if let invitationError {
            throw invitationError
        }
        return RoomControlInvitation(
            code: "123456-test-room",
            payload: "envoix://room/123456-test-room",
            endpoint: RoomControlEndpoint(broker: broker, relay: relay),
            expiresAt: now.addingTimeInterval(ConnectionWorkflowPolicy.roomInvitationLifetime)
        )
    }

    func parseInvitation(
        _ input: String,
        broker: String,
        relay: String,
        now: Date
    ) throws -> RoomControlInvitation {
        try makeInvitation(broker: broker, relay: relay, now: now)
    }

    func prepareDeviceVerification(
        label: String,
        endpoint: RoomControlEndpoint
    ) throws {
        preparedVerification = (label, endpoint)
    }

    func submitVerificationCode(_ code: String) async throws {
        submittedVerificationCodes.append(code)
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

    func connectRemembered(
        attempt: RememberedRoomConnectAttempt,
        mode: RememberedRoomConnectMode,
        timeout: TimeInterval?,
        beforeConnected: @escaping (UInt64) throws -> Void,
        onEvent: @escaping (RoomControlEvent) -> Void
    ) async throws {
        rememberedAttempts.append(attempt)
        guard let rememberedConnectHandler else {
            throw RememberedRoomConnectFailure(
                reason: "No remembered-room fixture was configured.",
                peerAuthenticated: false
            )
        }
        try await rememberedConnectHandler(
            attempt,
            mode,
            beforeConnected,
            onEvent
        )
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

private final class InMemoryRememberedCredentialStore: RememberedCredentialStoring {
    private var values: [String: Data] = [:]

    func put(_ reference: String, _ credential: Data) throws {
        values[reference] = credential
    }

    func get(_ reference: String) throws -> Data {
        guard let value = values[reference] else {
            throw RememberedPeerStoreError.missingCredential
        }
        return value
    }

    func delete(_ reference: String) throws {
        values.removeValue(forKey: reference)
    }
}

private func validRememberedCredential(secretByte: UInt8) -> Data {
    Data(
        Array("ENVR".utf8)
            + [1]
            + Array(repeating: secretByte, count: 32)
    )
}
