import XCTest
@testable import Envoix_iOS

final class NearbyDiscoveryTests: XCTestCase {
    private func bleOffer(now: Date = Date()) -> String {
        "envoix://ble/v1/123456?broker=example&expires=\(Int(now.timeIntervalSince1970) + 300)"
    }

    func testBleRendezvousRoundTripsFragmentedInvite() throws {
        let identity = LocalNearbyDiscoveryIdentity(
            peerKey: "0011223344556677",
            displayName: "iPhone"
        )
        let invite = bleOffer()
        let frames = try XCTUnwrap(BleRendezvousProtocol.encodeInvite(
            identity: identity,
            invite: invite,
            requestID: 0x0102030405060708,
            maximumFrameBytes: 31
        ))
        let assembler = BleRendezvousProtocol.Assembler()

        let decoded = try XCTUnwrap(frames.compactMap(assembler.accept).first)

        XCTAssertGreaterThan(frames.count, 1)
        XCTAssertEqual(decoded.requestID, "0102030405060708")
        XCTAssertEqual(decoded.senderPeerKey, identity.peerKey)
        XCTAssertEqual(decoded.senderDisplayName, identity.displayName)
        XCTAssertEqual(decoded.invite, invite)
    }

    func testBleRendezvousRejectsOutOfOrderContinuationAndResets() throws {
        let identity = LocalNearbyDiscoveryIdentity(peerKey: "0011223344556677", displayName: "iPhone")
        let invite = bleOffer()
        let frames = try XCTUnwrap(BleRendezvousProtocol.encodeInvite(
            identity: identity,
            invite: invite,
            requestID: 7,
            maximumFrameBytes: 24
        ))
        let assembler = BleRendezvousProtocol.Assembler()

        XCTAssertNil(assembler.accept(frames[1]))
        XCTAssertNil(assembler.accept(frames[0]))
        XCTAssertNil(assembler.accept(frames[2]))
        let decoded = try XCTUnwrap(frames.compactMap(assembler.accept).first)
        XCTAssertEqual(decoded.invite, invite)
    }

    func testBleRendezvousRejectsInvalidInviteAndSecurityMode() throws {
        let identity = LocalNearbyDiscoveryIdentity(peerKey: "0011223344556677", displayName: "iPhone")
        XCTAssertNil(BleRendezvousProtocol.encodeInvite(
            identity: identity,
            invite: "123456-alpha-bravo",
            requestID: 7,
            maximumFrameBytes: 64
        ))
        var frame = try XCTUnwrap(BleRendezvousProtocol.encodeInvite(
            identity: identity,
            invite: bleOffer(),
            requestID: 7,
            maximumFrameBytes: 512
        )?.first)
        frame[BleRendezvousProtocol.frameHeaderSize] = 1
        XCTAssertNil(BleRendezvousProtocol.Assembler().accept(frame))
    }

    func testBleRendezvousCarriesOnlyPublicVerificationOffer() throws {
        let identity = LocalNearbyDiscoveryIdentity(
            peerKey: "0011223344556677",
            displayName: "iPhone"
        )
        let invite = bleOffer()
        let frames = try XCTUnwrap(BleRendezvousProtocol.encodeInvite(
            identity: identity,
            invite: invite,
            requestID: 9,
            maximumFrameBytes: 31
        ))

        let assembler = BleRendezvousProtocol.Assembler()
        let decoded = try XCTUnwrap(frames.compactMap(assembler.accept).first)
        XCTAssertEqual(decoded.invite, invite)
        XCTAssertNil(BleRendezvousProtocol.encodeInvite(
            identity: identity,
            invite: "envoix://room/123456-a1b2-c3d4?broker=example",
            requestID: 10,
            maximumFrameBytes: 128
        ))
        XCTAssertNil(BleRendezvousProtocol.encodeInvite(
            identity: identity,
            invite: "envoix://invite/v2/secret",
            requestID: 11,
            maximumFrameBytes: 128
        ))
    }

    func testBleVerificationCodeReconstructsPrivatePakeInvitation() throws {
        let now = Date(timeIntervalSince1970: 1_700_000_000)
        let offer = "envoix://ble/v1/123456?broker=example&expires=1700000300"
        let invitation = try XCTUnwrap(BleVerificationInvitation.resolve(
            publicOffer: offer,
            verificationCode: "654321",
            now: now
        ))

        XCTAssertTrue(invitation.hasPrefix("envoix://room/123456-cd5e-bd5d?"))
        XCTAssertTrue(BleVerificationInvitation.isPublicOffer(offer, now: now))
        XCTAssertNil(BleVerificationInvitation.resolve(
            publicOffer: offer,
            verificationCode: "65432",
            now: now
        ))
        XCTAssertFalse(BleVerificationInvitation.isPublicOffer(
            offer,
            now: now.addingTimeInterval(300)
        ))
        XCTAssertNil(BleVerificationInvitation.resolve(
            publicOffer: offer + "&relay",
            verificationCode: "654321",
            now: now
        ))
        let tampered = offer.replacingOccurrences(of: "broker=example", with: "broker=other")
        XCTAssertNotEqual(
            URLComponents(string: invitation)?.path,
            URLComponents(string: try XCTUnwrap(BleVerificationInvitation.resolve(
                publicOffer: tampered,
                verificationCode: "654321",
                now: now
            )))?.path
        )
    }

    func testBleVerificationGeneratorRoundTripsWithDistinctPublicLocator() throws {
        let now = Date(timeIntervalSince1970: 1_700_000_000)
        let verification = try BleVerificationInvitation.make(
            broker: "example",
            relay: "",
            now: now
        )

        XCTAssertEqual(
            verification.privateInvitation,
            BleVerificationInvitation.resolve(
                publicOffer: verification.publicOffer,
                verificationCode: verification.verificationCode,
                now: now
            )
        )
        XCTAssertNotEqual(
            verification.verificationCode,
            URLComponents(string: verification.publicOffer)?.path.split(separator: "/").last.map(String.init)
        )
    }

    func testBleRendezvousRejectsLegacyAndNakedInvitationForms() {
        XCTAssertTrue(BleRendezvousProtocol.isSupportedInvite(
            "envoix://invite/v2/opaque"
        ))
        XCTAssertTrue(BleRendezvousProtocol.isSupportedInvite(
            "envoix://room/123456-a1b2-c3d4?broker=example"
        ))

        for invite in [
            "123456-a1b2-c3d4",
            "envoix://pair/123456-a1b2-c3d4",
            "envoix://room/R123456-a1b2-c3d4",
            "envoix://room/r123456-a1b2-c3d4",
            "envoix://room/%52123456-a1b2-c3d4",
            "envoix://room/123456-A1B2-C3D4",
            "envoix://invite/v2/",
        ] {
            XCTAssertFalse(
                BleRendezvousProtocol.isSupportedInvite(invite),
                "Accepted unsupported BLE invitation \(invite)"
            )
        }
    }

    func testBluetoothUUIDMatchesAndroidWireContract() throws {
        let peerKey = "8899aabbccddeeff"
        let uuid = try XCTUnwrap(NearbyDiscoveryBluetoothUUID.encode(peerKey: peerKey))

        XCTAssertEqual(uuid.uuidString.lowercased(), "d5f3a2d8-8f4a-4b33-8899-aabbccddeeff")
        XCTAssertEqual(NearbyDiscoveryBluetoothUUID.decode(uuid), peerKey)
    }

    func testBluetoothUUIDPreservesUnsignedHighBitAndRejectsOtherNamespaces() throws {
        let peerKey = "ffffffffffffffff"
        XCTAssertEqual(
            NearbyDiscoveryBluetoothUUID.decode(
                try XCTUnwrap(NearbyDiscoveryBluetoothUUID.encode(peerKey: peerKey))
            ),
            peerKey
        )
        XCTAssertNil(NearbyDiscoveryBluetoothUUID.encode(peerKey: "not-a-peer"))
        XCTAssertNil(NearbyDiscoveryBluetoothUUID.decode(UUID(uuidString: "d5f3a2d8-8f4a-4b34-ffff-ffffffffffff")))
    }

    func testNFCReadinessUUIDMatchesAndroidWireContract() throws {
        let offerID = "00112233aabbccdd"
        let uuid = try XCTUnwrap(
            NearbyNFCReadinessBluetoothUUID.encode(offerID: offerID)
        )

        XCTAssertEqual(
            uuid.uuidString.lowercased(),
            "d5f3a2d8-8f4a-4b34-0011-2233aabbccdd"
        )
        XCTAssertEqual(NearbyNFCReadinessBluetoothUUID.decode(uuid), offerID)
        XCTAssertEqual(
            NearbyNFCReadinessBluetoothUUID.normalizeOfferID(offerID),
            offerID
        )
        XCTAssertNil(
            NearbyNFCReadinessBluetoothUUID.normalizeOfferID(
                offerID.uppercased()
            )
        )
        XCTAssertNil(
            NearbyNFCReadinessBluetoothUUID.encode(
                offerID: "0000000000000000"
            )
        )
        XCTAssertNil(
            NearbyNFCReadinessBluetoothUUID.encode(offerID: "not-an-offer-id")
        )
        XCTAssertNil(
            NearbyNFCReadinessBluetoothUUID.decode(
                UUID(uuidString: "d5f3a2d8-8f4a-4b33-0011-223344556677")
            )
        )
        XCTAssertNil(
            NearbyNFCReadinessBluetoothUUID.decode(
                UUID(uuidString: NearbyNFCReadinessBluetoothUUID.baseUUIDString)
            )
        )
    }

    func testNFCReadinessIdentityRequiresOneRecentPeerOnTheSamePresenter() {
        var registry = NearbyNFCReadinessIdentityRegistry()
        let presenterID = UUID(uuidString: "00112233-4455-6677-8899-aabbccddeeff")!

        XCTAssertNil(registry.boundPeerKey(for: presenterID, at: 100))
        XCTAssertTrue(registry.observePresence(
            peerKey: "0011223344556677",
            presenterID: presenterID,
            at: 100
        ))
        XCTAssertEqual(
            registry.boundPeerKey(for: presenterID, at: 101),
            "0011223344556677"
        )
        XCTAssertTrue(registry.observePresence(
            peerKey: "8899aabbccddeeff",
            presenterID: presenterID,
            at: 101
        ))
        XCTAssertNil(registry.boundPeerKey(for: presenterID, at: 102))

        XCTAssertEqual(
            registry.boundPeerKey(
                for: presenterID,
                at: 100 + NearbyNFCReadinessIdentityRegistry
                    .bindingLifetimeMilliseconds + 1
            ),
            "8899aabbccddeeff"
        )
        XCTAssertFalse(registry.observePresence(
            peerKey: "not-a-peer",
            presenterID: presenterID,
            at: 200
        ))
        XCTAssertFalse(registry.observePresence(
            peerKey: "8899aabbccddeeff",
            presenterID: presenterID,
            at: 100
        ))
    }

    func testNFCReadinessRegistryNormalizesDeduplicatesAndExpiresOffers() throws {
        var registry = NearbyNFCReadinessOfferRegistry()
        let presenterID = UUID(uuidString: "00112233-4455-6677-8899-aabbccddeeff")!
        let offer = try XCTUnwrap(registry.observe(
            offerID: "aabbccddeeff0011",
            presenterPeerKey: "8899AABBCCDDEEFF",
            presenterID: presenterID,
            at: 1_000
        ))

        XCTAssertEqual(offer.id, "aabbccddeeff0011")
        XCTAssertEqual(offer.presenterPeerKey, "8899aabbccddeeff")
        XCTAssertEqual(offer.presenterID, presenterID)
        XCTAssertTrue(offer.isFresh(at: 30_999))
        XCTAssertFalse(offer.isFresh(at: 31_000))
        XCTAssertEqual(
            offer.remainingLifetimeSeconds(at: 1_000),
            30,
            accuracy: 0.001
        )
        XCTAssertNil(registry.observe(
            offerID: offer.id,
            presenterPeerKey: "0011223344556677",
            presenterID: UUID(),
            at: 1_001
        ))
        XCTAssertNil(registry.observe(
            offerID: "0000000000000000",
            presenterPeerKey: "8899aabbccddeeff",
            presenterID: presenterID,
            at: 1_001
        ))
    }

    func testBonjourRecordMatchesAndroidKeysAndBoundsName() throws {
        let inboxEndpointID = "2cfu7vzc7zhqv6w3k7m2kkwqvwzppmzvv53lmst6xm7ubjx5qnya"
        let relayURL = "https://relay.example.test"
        let directAddresses = ["192.0.2.10:4242", "[2001:db8::10]:4242"]
        let record = try XCTUnwrap(NearbyDiscoveryBonjourRecord(dictionary: [
            "v": "1",
            "id": "AABBCCDDEEFF0011",
            "name": "  test   device  ",
            "ibox": inboxEndpointID,
            "irelay": relayURL,
            "iaddr0": directAddresses[0],
            "iaddr1": directAddresses[1],
        ]))

        XCTAssertEqual(record.peerKey, "aabbccddeeff0011")
        XCTAssertEqual(record.displayName, "test device")
        XCTAssertEqual(record.inviteRoute?.endpointID, inboxEndpointID)
        XCTAssertEqual(record.inviteRoute?.relayURL, relayURL)
        XCTAssertEqual(record.inviteRoute?.directAddresses, directAddresses)
        XCTAssertEqual(record.dictionary["v"], "1")
        XCTAssertEqual(record.dictionary["id"], "aabbccddeeff0011")
        XCTAssertEqual(record.dictionary["ibox"], inboxEndpointID)
        XCTAssertEqual(record.dictionary["irelay"], relayURL)
        XCTAssertEqual(record.dictionary["iaddr0"], directAddresses[0])
        XCTAssertEqual(record.dictionary["iaddr1"], directAddresses[1])
        XCTAssertNil(NearbyDiscoveryBonjourRecord(dictionary: ["v": "2", "id": record.peerKey]))

        let longName = String(repeating: "x", count: 60)
        XCTAssertEqual(
            NearbyDiscoveryBonjourRecord(dictionary: ["v": "1", "id": record.peerKey, "name": longName])?
                .displayName?.count,
            NearbyDiscoveryPeerRegistry.maximumDisplayNameLength
        )

        let legacy = try XCTUnwrap(NearbyDiscoveryBonjourRecord(dictionary: [
            "v": "1",
            "id": record.peerKey,
        ]))
        XCTAssertNil(legacy.inviteRoute)
        let malformedInbox = try XCTUnwrap(NearbyDiscoveryBonjourRecord(dictionary: [
            "v": "1",
            "id": record.peerKey,
            "ibox": "not an endpoint",
            "iaddr0": directAddresses[0],
        ]))
        XCTAssertNil(malformedInbox.inviteRoute)
        XCTAssertNil(malformedInbox.dictionary["ibox"])
    }

    func testBonjourRouteRequiresCoordinatesAndRejectsAmbiguousRecords() throws {
        let endpointID = "2cfu7vzc7zhqv6w3k7m2kkwqvwzppmzvv53lmst6xm7ubjx5qnya"
        let base = ["v": "1", "id": "aabbccddeeff0011", "ibox": endpointID]

        let endpointOnly = try XCTUnwrap(NearbyDiscoveryBonjourRecord(dictionary: base))
        XCTAssertNil(endpointOnly.inviteRoute)

        var validDictionary = base
        validDictionary["iaddr0"] = "192.0.2.10:4242"
        let valid = try XCTUnwrap(NearbyDiscoveryBonjourRecord(dictionary: validDictionary))
        XCTAssertNotNil(valid.inviteRoute)
        XCTAssertEqual(
            NearbyDiscoveryBonjourRecord.consistentInviteRoute(in: [valid, valid]),
            valid.inviteRoute
        )

        var changedDictionary = validDictionary
        changedDictionary["iaddr0"] = "192.0.2.11:4242"
        let changed = try XCTUnwrap(
            NearbyDiscoveryBonjourRecord(dictionary: changedDictionary)
        )
        XCTAssertNil(
            NearbyDiscoveryBonjourRecord.consistentInviteRoute(in: [valid, changed])
        )
        XCTAssertNil(
            NearbyDiscoveryBonjourRecord.consistentInviteRoute(in: [valid, endpointOnly])
        )

        var oversizedDictionary = base
        oversizedDictionary["iaddr0"] = String(
            repeating: "x",
            count: NearbyInviteRoute.maximumDirectAddressUTF8Bytes + 1
        )
        XCTAssertNil(
            NearbyDiscoveryBonjourRecord(dictionary: oversizedDictionary)?.inviteRoute
        )
    }

    func testRegistryMergesSourcesAndExpiresThemIndependently() {
        let registry = NearbyDiscoveryPeerRegistry()
        XCTAssertTrue(registry.upsert(NearbyDiscoveryObservation(
            peerKey: "0011223344556677",
            source: .bluetooth,
            seenAtMilliseconds: 0,
            rssi: -51
        )))
        XCTAssertTrue(registry.upsert(NearbyDiscoveryObservation(
            peerKey: "0011223344556677",
            source: .mdns,
            seenAtMilliseconds: 5_000,
            displayName: "Phone",
            inviteRoute: testInviteRoute()
        )))

        var peers = registry.peers(nowMilliseconds: 10_000)
        XCTAssertEqual(peers.count, 1)
        XCTAssertEqual(peers[0].sources, [.bluetooth, .mdns])
        XCTAssertEqual(peers[0].displayName, "Phone")
        XCTAssertEqual(peers[0].rssi, -51)
        XCTAssertEqual(peers[0].inviteRoute, testInviteRoute())

        peers = registry.peers(nowMilliseconds: 20_001)
        XCTAssertEqual(peers.count, 1)
        XCTAssertEqual(peers[0].sources, [.mdns])
        XCTAssertNil(peers[0].rssi)
        XCTAssertEqual(peers[0].inviteRoute, testInviteRoute())

        XCTAssertTrue(registry.peers(nowMilliseconds: 25_001).isEmpty)
    }

    func testRegistryRejectsOutOfOrderObservationAndSanitizesInput() throws {
        let registry = NearbyDiscoveryPeerRegistry()
        XCTAssertTrue(registry.upsert(NearbyDiscoveryObservation(
            peerKey: "AABBCCDDEEFF0011",
            source: .mdns,
            seenAtMilliseconds: 20,
            displayName: "  New\nName  "
        )))
        XCTAssertFalse(registry.upsert(NearbyDiscoveryObservation(
            peerKey: "aabbccddeeff0011",
            source: .mdns,
            seenAtMilliseconds: 19,
            displayName: "Old name"
        )))
        XCTAssertFalse(registry.upsert(NearbyDiscoveryObservation(
            peerKey: "invalid",
            source: .bluetooth,
            seenAtMilliseconds: 30
        )))

        let peer = try XCTUnwrap(registry.peers(nowMilliseconds: 20).first)
        XCTAssertEqual(peer.peerKey, "aabbccddeeff0011")
        XCTAssertEqual(peer.displayName, "New Name")
    }

    func testRegistryKeepsFirstSeenOrderAcrossRefreshCallbacks() {
        let registry = NearbyDiscoveryPeerRegistry()
        registry.upsert(NearbyDiscoveryObservation(
            peerKey: "bbbbbbbbbbbbbbbb",
            source: .mdns,
            seenAtMilliseconds: 200,
            displayName: "Zulu"
        ))
        registry.upsert(NearbyDiscoveryObservation(
            peerKey: "aaaaaaaaaaaaaaaa",
            source: .mdns,
            seenAtMilliseconds: 100,
            displayName: "Alpha"
        ))

        XCTAssertEqual(registry.peers(nowMilliseconds: 200).map(\.peerKey), [
            "bbbbbbbbbbbbbbbb",
            "aaaaaaaaaaaaaaaa",
        ])

        registry.upsert(NearbyDiscoveryObservation(
            peerKey: "aaaaaaaaaaaaaaaa",
            source: .bluetooth,
            seenAtMilliseconds: 300,
            rssi: -40
        ))
        XCTAssertEqual(registry.peers(nowMilliseconds: 300).map(\.peerKey), [
            "bbbbbbbbbbbbbbbb",
            "aaaaaaaaaaaaaaaa",
        ])

        registry.upsert(NearbyDiscoveryObservation(
            peerKey: "bbbbbbbbbbbbbbbb",
            source: .mdns,
            seenAtMilliseconds: 400,
            displayName: "Renamed"
        ))
        XCTAssertEqual(registry.peers(nowMilliseconds: 400).map(\.peerKey), [
            "bbbbbbbbbbbbbbbb",
            "aaaaaaaaaaaaaaaa",
        ])
    }

    func testRegistryMergesSamePeerAcrossTransportsWithoutMovingOrLosingName() throws {
        let registry = NearbyDiscoveryPeerRegistry()
        registry.upsert(NearbyDiscoveryObservation(
            peerKey: "1111111111111111",
            source: .mdns,
            seenAtMilliseconds: 10,
            displayName: "Desk Mac"
        ))
        registry.upsert(NearbyDiscoveryObservation(
            peerKey: "2222222222222222",
            source: .mdns,
            seenAtMilliseconds: 20,
            displayName: "Phone"
        ))
        registry.upsert(NearbyDiscoveryObservation(
            peerKey: "1111111111111111",
            source: .bluetooth,
            seenAtMilliseconds: 30,
            rssi: -47
        ))

        let peers = registry.peers(nowMilliseconds: 30)
        XCTAssertEqual(peers.map(\.peerKey), [
            "1111111111111111",
            "2222222222222222",
        ])
        XCTAssertEqual(peers.count, 2)
        let mergedPeer = try XCTUnwrap(peers.first)
        XCTAssertEqual(mergedPeer.sources, [.bluetooth, .mdns])
        XCTAssertEqual(mergedPeer.displayName, "Desk Mac")
        XCTAssertEqual(mergedPeer.rssi, -47)
    }

    func testRegistryKeepsKnownSourceNameWhenNewerRefreshOmitsIt() throws {
        let registry = NearbyDiscoveryPeerRegistry()
        registry.upsert(NearbyDiscoveryObservation(
            peerKey: "0011223344556677",
            source: .mdns,
            seenAtMilliseconds: 10,
            displayName: "Desk Mac"
        ))

        registry.upsert(NearbyDiscoveryObservation(
            peerKey: "0011223344556677",
            source: .mdns,
            seenAtMilliseconds: 20,
            displayName: " \n "
        ))

        let peer = try XCTUnwrap(registry.peers(nowMilliseconds: 20).first)
        XCTAssertEqual(peer.displayName, "Desk Mac")
        XCTAssertEqual(peer.lastSeenAtMilliseconds, 20)
    }

    func testRegistryUsesBLENameUntilACompleteBonjourNameIsAvailable() throws {
        let registry = NearbyDiscoveryPeerRegistry()
        registry.upsert(NearbyDiscoveryObservation(
            peerKey: "0011223344556677",
            source: .bluetooth,
            seenAtMilliseconds: 10,
            displayName: "Nearby Xi"
        ))

        XCTAssertEqual(
            registry.peers(nowMilliseconds: 10).first?.displayName,
            "Nearby Xi"
        )

        registry.upsert(NearbyDiscoveryObservation(
            peerKey: "0011223344556677",
            source: .mdns,
            seenAtMilliseconds: 20,
            displayName: "Nearby Xiaomi Phone"
        ))
        registry.upsert(NearbyDiscoveryObservation(
            peerKey: "0011223344556677",
            source: .bluetooth,
            seenAtMilliseconds: 30,
            displayName: "Nearby Xi"
        ))

        let peer = try XCTUnwrap(registry.peers(nowMilliseconds: 30).first)
        XCTAssertEqual(peer.displayName, "Nearby Xiaomi Phone")
    }

    func testRegistryKeepsFirstSeenOrderForDuplicateAndMissingNames() {
        let registry = NearbyDiscoveryPeerRegistry()
        registry.upsert(NearbyDiscoveryObservation(
            peerKey: "cccccccccccccccc",
            source: .bluetooth,
            seenAtMilliseconds: 300
        ))
        registry.upsert(NearbyDiscoveryObservation(
            peerKey: "bbbbbbbbbbbbbbbb",
            source: .mdns,
            seenAtMilliseconds: 100,
            displayName: "Same name"
        ))
        registry.upsert(NearbyDiscoveryObservation(
            peerKey: "aaaaaaaaaaaaaaaa",
            source: .mdns,
            seenAtMilliseconds: 200,
            displayName: "Same name"
        ))

        XCTAssertEqual(registry.peers(nowMilliseconds: 300).map(\.peerKey), [
            "cccccccccccccccc",
            "bbbbbbbbbbbbbbbb",
            "aaaaaaaaaaaaaaaa",
        ])

        registry.upsert(NearbyDiscoveryObservation(
            peerKey: "cccccccccccccccc",
            source: .mdns,
            seenAtMilliseconds: 400,
            displayName: "Now named"
        ))
        XCTAssertEqual(registry.peers(nowMilliseconds: 400).map(\.peerKey), [
            "cccccccccccccccc",
            "bbbbbbbbbbbbbbbb",
            "aaaaaaaaaaaaaaaa",
        ])
    }

    func testDuplicateNearbyNamesGainStableShortIdentityOnlyWhenNeeded() {
        let first = NearbyDiscoveredPeer(
            peerKey: "0011223344556677",
            displayName: "iPhone",
            sources: [.bluetooth],
            lastSeenAtMilliseconds: 1,
            rssi: nil,
            inviteRoute: nil
        )
        let second = NearbyDiscoveredPeer(
            peerKey: "8899aabbccddeeff",
            displayName: " iPhone ",
            sources: [.mdns],
            lastSeenAtMilliseconds: 2,
            rssi: nil,
            inviteRoute: nil
        )
        let unique = NearbyDiscoveredPeer(
            peerKey: "0123456789abcdef",
            displayName: "iPad",
            sources: [.bluetooth],
            lastSeenAtMilliseconds: 3,
            rssi: nil,
            inviteRoute: nil
        )
        let peers = [first, second, unique]

        XCTAssertEqual(
            nearbyPeerDisplayName(first, among: peers, fallback: "Nearby Envoix device"),
            "iPhone · 6677"
        )
        XCTAssertEqual(
            nearbyPeerDisplayName(second, among: peers, fallback: "Nearby Envoix device"),
            "iPhone · EEFF"
        )
        XCTAssertEqual(
            nearbyPeerDisplayName(unique, among: peers, fallback: "Nearby Envoix device"),
            "iPad"
        )
    }

    func testRegistryAppendsPeerWhenItReappearsAfterFullExpiration() {
        let registry = NearbyDiscoveryPeerRegistry(observationTTLMilliseconds: 100)
        registry.upsert(NearbyDiscoveryObservation(
            peerKey: "aaaaaaaaaaaaaaaa",
            source: .bluetooth,
            seenAtMilliseconds: 0
        ))
        registry.upsert(NearbyDiscoveryObservation(
            peerKey: "bbbbbbbbbbbbbbbb",
            source: .bluetooth,
            seenAtMilliseconds: 50
        ))

        XCTAssertEqual(registry.peers(nowMilliseconds: 101).map(\.peerKey), [
            "bbbbbbbbbbbbbbbb",
        ])

        registry.upsert(NearbyDiscoveryObservation(
            peerKey: "aaaaaaaaaaaaaaaa",
            source: .bluetooth,
            seenAtMilliseconds: 102
        ))
        XCTAssertEqual(registry.peers(nowMilliseconds: 102).map(\.peerKey), [
            "bbbbbbbbbbbbbbbb",
            "aaaaaaaaaaaaaaaa",
        ])
    }

    func testIdentityFactoryCreatesFreshPresenceIdentityForEachSession() {
        let first = NearbyDiscoveryIdentityFactory.create(
            displayName: "  iPhone   test ",
            randomValue: { 0xFFEEDDCCBBAA0099 }
        )
        let second = NearbyDiscoveryIdentityFactory.create(
            displayName: "iPhone",
            randomValue: { 1 }
        )

        XCTAssertEqual(first.peerKey, "ffeeddccbbaa0099")
        XCTAssertEqual(first.displayName, "iPhone test")
        XCTAssertEqual(second.peerKey, "0000000000000001")
        XCTAssertNotEqual(second.peerKey, first.peerKey)
    }

    @MainActor
    func testPresenceDefaultsVisibleSanitizesNameAndExpiresEveryoneMode() {
        let suiteName = "NearbyPresencePreferencesTests"
        let defaults = try! XCTUnwrap(UserDefaults(suiteName: suiteName))
        defaults.removePersistentDomain(forName: suiteName)
        defer { defaults.removePersistentDomain(forName: suiteName) }
        let start = Date(timeIntervalSince1970: 1_000)
        let preferences = NearbyPresencePreferences(defaults: defaults, now: start)

        XCTAssertEqual(preferences.visibility, .whileAppOpen)
        XCTAssertTrue(preferences.isAdvertising(sceneIsActive: true, now: start))
        XCTAssertFalse(preferences.isAdvertising(sceneIsActive: false, now: start))
        XCTAssertTrue(preferences.updateDisplayName("  Jinbin's\n iPhone  "))
        XCTAssertEqual(preferences.displayName, "Jinbin's iPhone")

        defaults.set("future-visibility", forKey: "envoix.nearby.visibility")
        XCTAssertEqual(
            NearbyPresencePreferences(defaults: defaults, now: start).visibility,
            .hidden
        )
        defaults.set(42, forKey: "envoix.nearby.visibility")
        XCTAssertEqual(
            NearbyPresencePreferences(defaults: defaults, now: start).visibility,
            .hidden
        )

        preferences.setVisibility(.everyoneTenMinutes, now: start)
        XCTAssertTrue(preferences.isAdvertising(
            sceneIsActive: true,
            now: start.addingTimeInterval(599)
        ))
        XCTAssertTrue(preferences.expireIfNeeded(
            now: start.addingTimeInterval(NearbyPresencePreferences.visibilityDuration)
        ))
        XCTAssertEqual(preferences.visibility, .hidden)
        XCTAssertEqual(
            NearbyPresencePreferences(defaults: defaults, now: start).visibility,
            .hidden
        )
    }

    func testCoordinatorStartStopAreIdempotentAndIgnoreSelf() {
        var now: Int64 = 100
        let provider = CountingNearbyDiscoveryProvider(source: .bluetooth)
        let identity = LocalNearbyDiscoveryIdentity(peerKey: "0011223344556677", displayName: "iPhone")
        let coordinator = NearbyDiscoveryCoordinator(
            identity: identity,
            clock: { now },
            providerFactory: { _ in [provider] }
        )

        coordinator.start()
        coordinator.start()
        XCTAssertEqual(provider.startCount, 1)
        XCTAssertTrue(coordinator.state.isActive)

        provider.emit(.observation(NearbyDiscoveryObservation(
            peerKey: identity.peerKey,
            source: .bluetooth,
            seenAtMilliseconds: now
        )))
        XCTAssertTrue(coordinator.state.peers.isEmpty)

        now += 1
        provider.emit(.observation(NearbyDiscoveryObservation(
            peerKey: "8899aabbccddeeff",
            source: .bluetooth,
            seenAtMilliseconds: now
        )))
        XCTAssertEqual(coordinator.state.peers.map(\.peerKey), ["8899aabbccddeeff"])

        coordinator.stop()
        coordinator.stop()
        XCTAssertEqual(provider.stopCount, 1)
        XCTAssertFalse(coordinator.state.isActive)
        XCTAssertTrue(coordinator.state.peers.isEmpty)
    }

    func testCoordinatorAppliesAdvertisingPolicyBeforeProviderStarts() {
        let provider = CountingNearbyDiscoveryProvider(source: .bluetooth)
        let coordinator = NearbyDiscoveryCoordinator(
            identity: LocalNearbyDiscoveryIdentity(
                peerKey: "0011223344556677",
                displayName: "iPhone"
            ),
            providerFactory: { _ in [provider] }
        )

        coordinator.configure(displayName: "iPhone", advertisingEnabled: true)
        coordinator.start()

        XCTAssertEqual(provider.advertisingValues, [true])
        XCTAssertEqual(provider.advertisingValueAtStart, true)
    }

    func testCoordinatorSystemPairingSuspensionAwaitsProviderShutdown() async {
        let provider = QuiescingNearbyDiscoveryProvider()
        let coordinator = NearbyDiscoveryCoordinator(
            identity: LocalNearbyDiscoveryIdentity(
                peerKey: "0011223344556677",
                displayName: "iPhone"
            ),
            providerFactory: { _ in [provider] }
        )
        coordinator.start()

        await coordinator.suspendForSystemPairing()

        XCTAssertFalse(coordinator.state.isActive)
        XCTAssertEqual(provider.stopCount, 1)
        XCTAssertEqual(provider.waitCount, 1)
        XCTAssertTrue(provider.finishedWaiting)
    }

    func testCoordinatorRetainsIdentityAcrossPausedPresenceForRoomContinuity() {
        var identityFactoryCalls = 0
        var advertisedPeerKeys: [String] = []
        let provider = CountingNearbyDiscoveryProvider(source: .bluetooth)
        let coordinator = NearbyDiscoveryCoordinator(
            identityFactory: {
                identityFactoryCalls += 1
                return LocalNearbyDiscoveryIdentity(
                    peerKey: "0011223344556677",
                    displayName: "iPhone"
                )
            },
            providerFactory: { identity in
                advertisedPeerKeys.append(identity.peerKey)
                return [provider]
            }
        )

        coordinator.start()
        coordinator.stop()
        coordinator.start()

        XCTAssertEqual(identityFactoryCalls, 1)
        XCTAssertEqual(advertisedPeerKeys, ["0011223344556677"])
        XCTAssertEqual(provider.startCount, 2)
        XCTAssertEqual(provider.stopCount, 1)
        XCTAssertEqual(
            provider.identities.map(\.peerKey),
            ["0011223344556677", "0011223344556677"]
        )
        XCTAssertEqual(coordinator.state.localName, "iPhone")
    }

    func testCoordinatorReconfiguresIdentityWithoutReplacingProvider() {
        var providerFactoryCalls = 0
        let provider = CountingNearbyDiscoveryProvider(source: .bluetooth)
        let coordinator = NearbyDiscoveryCoordinator(
            identity: LocalNearbyDiscoveryIdentity(
                peerKey: "0011223344556677",
                displayName: "iPhone"
            ),
            providerFactory: { _ in
                providerFactoryCalls += 1
                return [provider]
            }
        )

        coordinator.start()
        coordinator.configure(displayName: "Renamed iPhone", advertisingEnabled: false)

        XCTAssertEqual(providerFactoryCalls, 1)
        XCTAssertEqual(provider.startCount, 2)
        XCTAssertEqual(provider.stopCount, 1)
        XCTAssertEqual(
            provider.identities.map(\.displayName),
            ["iPhone", "Renamed iPhone"]
        )
        XCTAssertEqual(coordinator.state.localName, "Renamed iPhone")
    }

    func testRegistryKeepsPeerThroughSourceLossAndMergesReturningSource() throws {
        let registry = NearbyDiscoveryPeerRegistry(observationTTLMilliseconds: 1_000)
        registry.upsert(NearbyDiscoveryObservation(
            peerKey: "0011223344556677",
            source: .bluetooth,
            seenAtMilliseconds: 0,
            rssi: -60
        ))
        registry.upsert(NearbyDiscoveryObservation(
            peerKey: "0011223344556677",
            source: .mdns,
            seenAtMilliseconds: 500
        ))

        registry.upsert(NearbyDiscoveryObservation(
            peerKey: "0011223344556677",
            source: .mdns,
            seenAtMilliseconds: 1_001
        ))
        var peer = try XCTUnwrap(registry.peers(nowMilliseconds: 1_001).first)
        XCTAssertEqual(peer.sources, [.mdns])

        registry.upsert(NearbyDiscoveryObservation(
            peerKey: "0011223344556677",
            source: .bluetooth,
            seenAtMilliseconds: 1_002,
            rssi: -48
        ))
        peer = try XCTUnwrap(registry.peers(nowMilliseconds: 1_002).first)
        XCTAssertEqual(peer.sources, [.bluetooth, .mdns])
        XCTAssertEqual(peer.rssi, -48)
    }

    func testRegistryClearRemovesPreviousPresenceSession() {
        let registry = NearbyDiscoveryPeerRegistry()
        registry.upsert(NearbyDiscoveryObservation(
            peerKey: "aaaaaaaaaaaaaaaa",
            source: .bluetooth,
            seenAtMilliseconds: 1
        ))

        registry.clear()

        XCTAssertTrue(registry.peers(nowMilliseconds: 1).isEmpty)
        registry.upsert(NearbyDiscoveryObservation(
            peerKey: "bbbbbbbbbbbbbbbb",
            source: .bluetooth,
            seenAtMilliseconds: 2
        ))
        registry.upsert(NearbyDiscoveryObservation(
            peerKey: "aaaaaaaaaaaaaaaa",
            source: .bluetooth,
            seenAtMilliseconds: 2
        ))
        XCTAssertEqual(registry.peers(nowMilliseconds: 2).map(\.peerKey), [
            "bbbbbbbbbbbbbbbb",
            "aaaaaaaaaaaaaaaa",
        ])
    }

    func testCoordinatorRejectsCallbacksFromStoppedGeneration() {
        let provider = CountingNearbyDiscoveryProvider(source: .bluetooth)
        var providerFactoryCalls = 0
        var now: Int64 = 1
        let coordinator = NearbyDiscoveryCoordinator(
            identity: LocalNearbyDiscoveryIdentity(
                peerKey: "0011223344556677",
                displayName: "iPhone"
            ),
            clock: { now },
            providerFactory: { _ in
                providerFactoryCalls += 1
                return [provider]
            }
        )

        coordinator.start()
        coordinator.stop()
        coordinator.start()
        provider.emit(.observation(NearbyDiscoveryObservation(
            peerKey: "1111222233334444",
            source: .bluetooth,
            seenAtMilliseconds: 1
        )))
        now = 2
        provider.emitFromStart(0, event: .observation(NearbyDiscoveryObservation(
            peerKey: "aaaabbbbccccdddd",
            source: .bluetooth,
            seenAtMilliseconds: 2
        )))

        XCTAssertEqual(providerFactoryCalls, 1)
        XCTAssertEqual(provider.startCount, 2)
        XCTAssertEqual(coordinator.state.peers.map(\.peerKey), ["1111222233334444"])
    }

    func testCoordinatorQueuesOffersAndDeduplicatesPerSenderRequest() throws {
        let provider = CountingNearbyDiscoveryProvider(source: .bluetooth)
        let coordinator = NearbyDiscoveryCoordinator(
            identity: LocalNearbyDiscoveryIdentity(
                peerKey: "0011223344556677",
                displayName: "iPhone"
            ),
            providerFactory: { _ in [provider] }
        )
        let first = rendezvousOffer(requestID: "first")
        let second = rendezvousOffer(requestID: "second")
        let duplicateFirst = NearbyRendezvousOffer(
            requestID: first.requestID,
            senderPeerKey: first.senderPeerKey,
            senderDisplayName: first.senderDisplayName,
            source: first.source,
            senderInboxEndpointID: first.senderInboxEndpointID,
            invite: "envoix://room/654321-d4c3-b2a1"
        )
        let collidingRequestFromAnotherPeer = NearbyRendezvousOffer(
            requestID: first.requestID,
            senderPeerKey: "1021324354657687",
            senderDisplayName: "Another nearby phone",
            source: first.source,
            senderInboxEndpointID: nil,
            invite: "envoix://room/654321-d4c3-b2a1"
        )

        coordinator.start()
        provider.emit(.rendezvousOffer(first))
        provider.emit(.rendezvousOffer(second))
        provider.emit(.rendezvousOffer(duplicateFirst))
        provider.emit(.rendezvousOffer(collidingRequestFromAnotherPeer))

        XCTAssertNotEqual(
            first.deliveryID,
            collidingRequestFromAnotherPeer.deliveryID
        )
        XCTAssertEqual(coordinator.state.incomingRendezvousOffer, first)
        coordinator.consumeRendezvousOffer(id: first.id)
        XCTAssertEqual(coordinator.state.incomingRendezvousOffer, second)
        coordinator.consumeRendezvousOffer(id: second.id)
        XCTAssertEqual(
            coordinator.state.incomingRendezvousOffer,
            collidingRequestFromAnotherPeer
        )
        coordinator.consumeRendezvousOffer(id: collidingRequestFromAnotherPeer.id)
        XCTAssertNil(coordinator.state.incomingRendezvousOffer)
    }

    func testCoordinatorBoundsPendingRendezvousOfferFIFO() {
        let provider = CountingNearbyDiscoveryProvider(source: .bluetooth)
        let coordinator = NearbyDiscoveryCoordinator(
            identity: LocalNearbyDiscoveryIdentity(
                peerKey: "0011223344556677",
                displayName: "iPhone"
            ),
            providerFactory: { _ in [provider] }
        )
        coordinator.start()

        for index in 0...NearbyDiscoveryCoordinator.maximumPendingRendezvousOfferCount {
            provider.emit(.rendezvousOffer(rendezvousOffer(requestID: "request-\(index)")))
        }

        var consumedIDs: [String] = []
        while let offer = coordinator.state.incomingRendezvousOffer {
            consumedIDs.append(offer.id)
            coordinator.consumeRendezvousOffer(id: offer.id)
        }
        XCTAssertEqual(
            consumedIDs,
            (0..<NearbyDiscoveryCoordinator.maximumPendingRendezvousOfferCount).map {
                "request-\($0)"
            }
        )
    }

    @MainActor
    func testCoordinatorReportsRendezvousAdmissionOnlyAfterQueueAcceptance() async throws {
        let provider = AdmissionNearbyDiscoveryProvider()
        let coordinator = NearbyDiscoveryCoordinator(
            identity: LocalNearbyDiscoveryIdentity(
                peerKey: "0011223344556677",
                displayName: "iPhone"
            ),
            providerFactory: { _ in [provider] }
        )
        coordinator.start()

        for index in 0..<NearbyDiscoveryCoordinator.maximumPendingRendezvousOfferCount {
            let accepted = await provider.admit(
                rendezvousOffer(requestID: "request-\(index)")
            )
            XCTAssertTrue(accepted)
        }
        let duplicateAccepted = await provider.admit(
            rendezvousOffer(requestID: "request-0")
        )
        XCTAssertTrue(duplicateAccepted)
        let overflow = rendezvousOffer(requestID: "overflow")
        let overflowAccepted = await provider.admit(overflow)
        XCTAssertFalse(overflowAccepted)

        let firstID = try XCTUnwrap(
            coordinator.state.incomingRendezvousOffer?.id
        )
        coordinator.consumeRendezvousOffer(id: firstID)
        let retriedOverflowAccepted = await provider.admit(overflow)
        XCTAssertTrue(retriedOverflowAccepted)
    }

    func testCoordinatorClearsPendingRendezvousOffersAcrossStopAndRestart() {
        let provider = CountingNearbyDiscoveryProvider(source: .bluetooth)
        let coordinator = NearbyDiscoveryCoordinator(
            identity: LocalNearbyDiscoveryIdentity(
                peerKey: "0011223344556677",
                displayName: "iPhone"
            ),
            providerFactory: { _ in [provider] }
        )
        coordinator.start()
        provider.emit(.rendezvousOffer(rendezvousOffer(requestID: "first")))
        provider.emit(.rendezvousOffer(rendezvousOffer(requestID: "second")))

        coordinator.stop()
        coordinator.start()

        XCTAssertNil(coordinator.state.incomingRendezvousOffer)
    }

    func testCoordinatorPublishesFreshNFCReadinessOnceAcrossRestarts() throws {
        var now: Int64 = 1_000
        let provider = CountingNearbyDiscoveryProvider(source: .bluetooth)
        let coordinator = NearbyDiscoveryCoordinator(
            identity: LocalNearbyDiscoveryIdentity(
                peerKey: "0011223344556677",
                displayName: "iPhone"
            ),
            clock: { now },
            providerFactory: { _ in [provider] }
        )
        let presenterID = UUID(
            uuidString: "00112233-4455-6677-8899-aabbccddeeff"
        )!

        coordinator.start()
        provider.emit(.nfcPresenterReadiness(
            offerID: "aabbccddeeff0011",
            presenterPeerKey: "8899aabbccddeeff",
            presenterID: presenterID
        ))
        let offer = try XCTUnwrap(
            coordinator.state.incomingNFCReadinessOffer
        )
        XCTAssertEqual(offer.id, "aabbccddeeff0011")
        XCTAssertEqual(offer.presenterPeerKey, "8899aabbccddeeff")

        coordinator.consumeNFCReadinessOffer(id: offer.id)
        provider.emit(.nfcPresenterReadiness(
            offerID: offer.id,
            presenterPeerKey: offer.presenterPeerKey,
            presenterID: presenterID
        ))
        XCTAssertNil(coordinator.state.incomingNFCReadinessOffer)

        coordinator.stop()
        coordinator.start()
        provider.emit(.nfcPresenterReadiness(
            offerID: offer.id,
            presenterPeerKey: offer.presenterPeerKey,
            presenterID: presenterID
        ))
        XCTAssertNil(coordinator.state.incomingNFCReadinessOffer)

        now += 1
        provider.emit(.nfcPresenterReadiness(
            offerID: "aabbccddeeff0022",
            presenterPeerKey: offer.presenterPeerKey,
            presenterID: presenterID
        ))
        XCTAssertEqual(
            coordinator.state.incomingNFCReadinessOffer?.id,
            "aabbccddeeff0022"
        )

        now += NearbyNFCReadinessOffer.lifetimeMilliseconds
        provider.emit(.observation(NearbyDiscoveryObservation(
            peerKey: offer.presenterPeerKey,
            source: .bluetooth,
            seenAtMilliseconds: now
        )))
        XCTAssertNil(coordinator.state.incomingNFCReadinessOffer)
    }

    func testPairedDeviceValidatesScopedIdentityAndBoundsMetadata() throws {
        let device = try XCTUnwrap(NearbyPairedDevice(
            sourceScopedID: " 0000000000000042 ",
            source: .wifiAware,
            displayName: "  Test\n iPad  ",
            model: String(repeating: "m", count: 120)
        ))

        XCTAssertEqual(device.id, "wifi_aware:0000000000000042")
        XCTAssertEqual(device.displayName, "Test iPad")
        XCTAssertEqual(device.model?.count, NearbyDiscoveryPeerRegistry.maximumDeviceDetailLength)
        XCTAssertNil(NearbyPairedDevice(sourceScopedID: "bad id", source: .wifiAware))
        XCTAssertNil(NearbyPairedDevice(sourceScopedID: "", source: .wifiAware))
    }

    func testCoordinatorReplacesAndDeduplicatesPairedDeviceSnapshot() throws {
        let provider = CountingNearbyDiscoveryProvider(source: .wifiAware)
        let coordinator = NearbyDiscoveryCoordinator(
            identity: LocalNearbyDiscoveryIdentity(
                peerKey: "0011223344556677",
                displayName: "iPhone"
            ),
            providerFactory: { _ in [provider] }
        )
        let first = try XCTUnwrap(NearbyPairedDevice(
            sourceScopedID: "0000000000000001",
            source: .wifiAware,
            displayName: "First"
        ))
        let second = try XCTUnwrap(NearbyPairedDevice(
            sourceScopedID: "0000000000000002",
            source: .wifiAware,
            displayName: "Second"
        ))
        let duplicateFirst = try XCTUnwrap(NearbyPairedDevice(
            sourceScopedID: "0000000000000001",
            source: .wifiAware,
            displayName: "Ignored duplicate"
        ))

        coordinator.start()
        provider.emit(.pairedDevices(source: .wifiAware, devices: [second, first, duplicateFirst]))
        XCTAssertEqual(coordinator.state.pairedDevices.map(\.sourceScopedID), [
            "0000000000000001",
            "0000000000000002",
        ])
        XCTAssertEqual(coordinator.state.pairedDevices.first?.displayName, "First")

        provider.emit(.pairedDevices(source: .wifiAware, devices: [second]))
        XCTAssertEqual(coordinator.state.pairedDevices, [second])

        provider.emit(.pairedDevices(source: .wifiAware, devices: []))
        XCTAssertTrue(coordinator.state.pairedDevices.isEmpty)
    }

    func testCoordinatorRejectsMismatchedAndStalePairedSnapshots() throws {
        let provider = CountingNearbyDiscoveryProvider(source: .wifiAware)
        var providerFactoryCalls = 0
        let coordinator = NearbyDiscoveryCoordinator(
            identity: LocalNearbyDiscoveryIdentity(
                peerKey: "0011223344556677",
                displayName: "iPhone"
            ),
            providerFactory: { _ in
                providerFactoryCalls += 1
                return [provider]
            }
        )
        let activeDevice = try XCTUnwrap(NearbyPairedDevice(
            sourceScopedID: "0000000000000001",
            source: .wifiAware
        ))
        let mismatchedDevice = try XCTUnwrap(NearbyPairedDevice(
            sourceScopedID: "0000000000000002",
            source: .mdns
        ))

        coordinator.start()
        coordinator.stop()
        coordinator.start()
        provider.emit(.pairedDevices(source: .wifiAware, devices: [activeDevice]))
        provider.emit(.pairedDevices(source: .wifiAware, devices: [mismatchedDevice]))
        provider.emitFromStart(
            0,
            event: .pairedDevices(source: .wifiAware, devices: [])
        )

        XCTAssertEqual(providerFactoryCalls, 1)
        XCTAssertEqual(provider.startCount, 2)
        XCTAssertEqual(coordinator.state.pairedDevices, [activeDevice])
    }

    func testCoordinatorRejectsOversizedPairedSnapshotWithoutLosingCurrentState() throws {
        let provider = CountingNearbyDiscoveryProvider(source: .wifiAware)
        let coordinator = NearbyDiscoveryCoordinator(
            identity: LocalNearbyDiscoveryIdentity(
                peerKey: "0011223344556677",
                displayName: "iPhone"
            ),
            providerFactory: { _ in [provider] }
        )
        let current = try XCTUnwrap(NearbyPairedDevice(
            sourceScopedID: "current",
            source: .wifiAware
        ))
        let oversized = (0...NearbyPairedDevice.maximumSnapshotCount).compactMap { index in
            NearbyPairedDevice(sourceScopedID: "device-\(index)", source: .wifiAware)
        }
        XCTAssertEqual(oversized.count, NearbyPairedDevice.maximumSnapshotCount + 1)

        coordinator.start()
        provider.emit(.pairedDevices(source: .wifiAware, devices: [current]))
        provider.emit(.pairedDevices(source: .wifiAware, devices: oversized))

        XCTAssertEqual(coordinator.state.pairedDevices, [current])
    }

    func testWifiAwareObservationFreezesExactDeviceIDIntoPeerSelection() throws {
        let provider = CountingNearbyDiscoveryProvider(source: .wifiAware)
        let now: Int64 = 100
        let coordinator = NearbyDiscoveryCoordinator(
            identity: LocalNearbyDiscoveryIdentity(
                peerKey: "0011223344556677",
                displayName: "iPhone"
            ),
            clock: { now },
            providerFactory: { _ in [provider] }
        )
        coordinator.start()

        provider.emit(.observation(NearbyDiscoveryObservation(
            peerKey: "8899aabbccddeeff",
            source: .wifiAware,
            seenAtMilliseconds: now,
            displayName: "Nearby iPad",
            nearbyWifiAwareDeviceID: "0000000000000042"
        )))

        let peer = try XCTUnwrap(coordinator.state.peers.first)
        XCTAssertEqual(peer.nearbyWifiAwareDeviceID, "0000000000000042")
        let selection = NearbyPairingSelection(peer: peer)
        XCTAssertEqual(selection.discoveryPeerKey, "8899aabbccddeeff")
        XCTAssertEqual(selection.nearbyWifiAwareDeviceID, "0000000000000042")
    }

    func testWifiAwareObservationKeepsDeviceIDsScopedToTheirPeers() throws {
        let provider = CountingNearbyDiscoveryProvider(source: .wifiAware)
        let coordinator = NearbyDiscoveryCoordinator(
            identity: LocalNearbyDiscoveryIdentity(
                peerKey: "0011223344556677",
                displayName: "iPhone"
            ),
            clock: { 100 },
            providerFactory: { _ in [provider] }
        )
        coordinator.start()

        provider.emit(.observation(NearbyDiscoveryObservation(
            peerKey: "1111222233334444",
            source: .wifiAware,
            seenAtMilliseconds: 100,
            nearbyWifiAwareDeviceID: "0000000000000001"
        )))
        provider.emit(.observation(NearbyDiscoveryObservation(
            peerKey: "aaaabbbbccccdddd",
            source: .wifiAware,
            seenAtMilliseconds: 100,
            nearbyWifiAwareDeviceID: "0000000000000002"
        )))

        let peers = Dictionary(uniqueKeysWithValues: coordinator.state.peers.map {
            ($0.peerKey, $0)
        })
        XCTAssertEqual(
            NearbyPairingSelection(
                peer: try XCTUnwrap(peers["1111222233334444"])
            ).nearbyWifiAwareDeviceID,
            "0000000000000001"
        )
        XCTAssertEqual(
            NearbyPairingSelection(
                peer: try XCTUnwrap(peers["aaaabbbbccccdddd"])
            ).nearbyWifiAwareDeviceID,
            "0000000000000002"
        )
    }

    func testCoordinatorPrefersExactWifiAwareRendezvousOverMdnsAndBluetooth() {
        let bluetooth = RoutingNearbyDiscoveryProvider(source: .bluetooth, canOffer: true)
        let mdns = RoutingNearbyDiscoveryProvider(source: .mdns, canOffer: true)
        let wifiAware = RoutingNearbyDiscoveryProvider(
            source: .wifiAware,
            canOffer: true,
            requiredWifiAwareDeviceID: "0000000000000042"
        )
        let coordinator = NearbyDiscoveryCoordinator(
            identity: LocalNearbyDiscoveryIdentity(
                peerKey: "0011223344556677",
                displayName: "iPhone"
            ),
            providerFactory: { _ in [bluetooth, mdns, wifiAware] }
        )
        coordinator.start()

        var result: String?
        coordinator.offerInvite(
            to: routingSelection(
                nearbyWifiAwareDeviceID: "0000000000000042"
            ),
            invite: "envoix://room/123456-a1b2-c3d4"
        ) { result = $0 }

        XCTAssertNil(result)
        XCTAssertEqual(wifiAware.offeredPeerKeys, ["8899aabbccddeeff"])
        XCTAssertEqual(
            wifiAware.offeredWifiAwareDeviceIDs,
            ["0000000000000042"]
        )
        XCTAssertTrue(mdns.offeredPeerKeys.isEmpty)
        XCTAssertTrue(bluetooth.offeredPeerKeys.isEmpty)
    }

    func testCoordinatorDoesNotDowngradeExactWifiAwareRouteToMdns() {
        let bluetooth = RoutingNearbyDiscoveryProvider(source: .bluetooth, canOffer: true)
        let mdns = RoutingNearbyDiscoveryProvider(source: .mdns, canOffer: true)
        let wifiAware = RoutingNearbyDiscoveryProvider(
            source: .wifiAware,
            canOffer: true,
            requiredWifiAwareDeviceID: "0000000000000099"
        )
        let coordinator = NearbyDiscoveryCoordinator(
            identity: LocalNearbyDiscoveryIdentity(
                peerKey: "0011223344556677",
                displayName: "iPhone"
            ),
            providerFactory: { _ in [bluetooth, mdns, wifiAware] }
        )
        coordinator.start()
        let selection = routingSelection(
            nearbyWifiAwareDeviceID: "0000000000000042"
        )

        XCTAssertFalse(coordinator.canOfferRoomInvite(to: selection))

        var result: String?
        coordinator.offerInvite(
            to: selection,
            invite: "envoix://room/123456-a1b2-c3d4"
        ) { result = $0 }

        XCTAssertEqual(
            result,
            "Nearby invitation delivery is not available for this device"
        )
        XCTAssertTrue(wifiAware.offeredPeerKeys.isEmpty)
        XCTAssertTrue(mdns.offeredPeerKeys.isEmpty)
        XCTAssertTrue(bluetooth.offeredPeerKeys.isEmpty)
    }

    func testCoordinatorDoesNotDowngradeUnavailableExactWifiAwareRouteToBluetooth() {
        let bluetooth = RoutingNearbyDiscoveryProvider(source: .bluetooth, canOffer: true)
        let mdns = RoutingNearbyDiscoveryProvider(source: .mdns, canOffer: false)
        let wifiAware = RoutingNearbyDiscoveryProvider(source: .wifiAware, canOffer: false)
        let coordinator = NearbyDiscoveryCoordinator(
            identity: LocalNearbyDiscoveryIdentity(
                peerKey: "0011223344556677",
                displayName: "iPhone"
            ),
            providerFactory: { _ in [bluetooth, mdns, wifiAware] }
        )
        coordinator.start()
        let selection = routingSelection(
            hasInviteRoute: false,
            nearbyWifiAwareDeviceID: "0000000000000042"
        )

        XCTAssertFalse(coordinator.canOfferRoomInvite(to: selection))

        var result: String?
        coordinator.offerInvite(
            to: selection,
            invite: "envoix://room/123456-a1b2-c3d4"
        ) { result = $0 }

        XCTAssertEqual(
            result,
            "Nearby invitation delivery is not available for this device"
        )
        XCTAssertTrue(wifiAware.offeredPeerKeys.isEmpty)
        XCTAssertTrue(mdns.offeredPeerKeys.isEmpty)
        XCTAssertTrue(bluetooth.offeredPeerKeys.isEmpty)
    }

    func testCoordinatorPrefersSecureMdnsRendezvousWhenPeerAdvertisesInbox() {
        let bluetooth = RoutingNearbyDiscoveryProvider(source: .bluetooth, canOffer: true)
        let mdns = RoutingNearbyDiscoveryProvider(source: .mdns, canOffer: true)
        let coordinator = NearbyDiscoveryCoordinator(
            identity: LocalNearbyDiscoveryIdentity(
                peerKey: "0011223344556677",
                displayName: "iPhone"
            ),
            providerFactory: { _ in [bluetooth, mdns] }
        )
        coordinator.start()

        XCTAssertTrue(coordinator.canOfferRoomInvite(to: routingSelection()))

        var result: String?
        coordinator.offerInvite(
            to: routingSelection(),
            invite: "envoix://room/123456-a1b2-c3d4"
        ) { result = $0 }

        XCTAssertNil(result)
        XCTAssertEqual(mdns.offeredPeerKeys, ["8899aabbccddeeff"])
        XCTAssertTrue(bluetooth.offeredPeerKeys.isEmpty)
    }

    func testCoordinatorKeepsDirectInviteV2OffRoomOnlyMdnsInbox() {
        let bluetooth = RoutingNearbyDiscoveryProvider(source: .bluetooth, canOffer: true)
        let mdns = RoutingNearbyDiscoveryProvider(source: .mdns, canOffer: true)
        let coordinator = NearbyDiscoveryCoordinator(
            identity: LocalNearbyDiscoveryIdentity(
                peerKey: "0011223344556677",
                displayName: "iPhone"
            ),
            providerFactory: { _ in [bluetooth, mdns] }
        )
        coordinator.start()

        var result: String?
        coordinator.offerInvite(
            to: routingSelection(),
            invite: "envoix://invite/v2/opaque"
        ) { result = $0 }

        XCTAssertNil(result)
        XCTAssertEqual(bluetooth.offeredPeerKeys, ["8899aabbccddeeff"])
        XCTAssertTrue(mdns.offeredPeerKeys.isEmpty)
    }

    func testCoordinatorDoesNotUseRoomOnlyMdnsAsDirectInviteFallback() {
        let mdns = RoutingNearbyDiscoveryProvider(source: .mdns, canOffer: true)
        let coordinator = NearbyDiscoveryCoordinator(
            identity: LocalNearbyDiscoveryIdentity(
                peerKey: "0011223344556677",
                displayName: "iPhone"
            ),
            providerFactory: { _ in [mdns] }
        )
        coordinator.start()

        var result: String?
        coordinator.offerInvite(
            to: routingSelection(),
            invite: "envoix://invite/v2/opaque"
        ) { result = $0 }

        XCTAssertEqual(
            result,
            "Nearby invitation delivery is not available for this device"
        )
        XCTAssertTrue(mdns.offeredPeerKeys.isEmpty)
    }

    func testCoordinatorDoesNotDowngradeFailedSecureMdnsDeliveryToBluetooth() {
        let bluetooth = RoutingNearbyDiscoveryProvider(source: .bluetooth, canOffer: true)
        let mdns = RoutingNearbyDiscoveryProvider(
            source: .mdns,
            canOffer: true,
            deliveryError: "secure mDNS delivery failed"
        )
        let coordinator = NearbyDiscoveryCoordinator(
            identity: LocalNearbyDiscoveryIdentity(
                peerKey: "0011223344556677",
                displayName: "iPhone"
            ),
            providerFactory: { _ in [bluetooth, mdns] }
        )
        coordinator.start()

        var result: String?
        let completed = expectation(description: "secure mDNS delivery completed")
        coordinator.offerInvite(
            to: routingSelection(),
            invite: "envoix://room/123456-a1b2-c3d4"
        ) {
            result = $0
            completed.fulfill()
        }
        wait(for: [completed], timeout: 1)

        XCTAssertEqual(result, "secure mDNS delivery failed")
        XCTAssertEqual(mdns.offeredPeerKeys, ["8899aabbccddeeff"])
        XCTAssertTrue(bluetooth.offeredPeerKeys.isEmpty)
    }

    func testCoordinatorRejectsStaleMdnsRouteWithoutInvokingAnyProvider() {
        let bluetooth = RoutingNearbyDiscoveryProvider(source: .bluetooth, canOffer: true)
        let mdns = RoutingNearbyDiscoveryProvider(
            source: .mdns,
            canOffer: false
        )
        let coordinator = NearbyDiscoveryCoordinator(
            identity: LocalNearbyDiscoveryIdentity(
                peerKey: "0011223344556677",
                displayName: "iPhone"
            ),
            providerFactory: { _ in [bluetooth, mdns] }
        )
        coordinator.start()
        let selection = routingSelection()

        XCTAssertFalse(coordinator.canOfferRoomInvite(to: selection))

        var result: String?
        coordinator.offerInvite(
            to: selection,
            invite: "envoix://room/123456-a1b2-c3d4"
        ) { result = $0 }

        XCTAssertEqual(
            result,
            "Nearby invitation delivery is not available for this device"
        )
        XCTAssertTrue(mdns.offeredPeerKeys.isEmpty)
        XCTAssertTrue(bluetooth.offeredPeerKeys.isEmpty)
    }

    func testCoordinatorUsesBluetoothWhenMdnsPeerHasNoInboxCapability() {
        let bluetooth = RoutingNearbyDiscoveryProvider(source: .bluetooth, canOffer: true)
        let mdns = RoutingNearbyDiscoveryProvider(source: .mdns, canOffer: false)
        let coordinator = NearbyDiscoveryCoordinator(
            identity: LocalNearbyDiscoveryIdentity(
                peerKey: "0011223344556677",
                displayName: "iPhone"
            ),
            providerFactory: { _ in [bluetooth, mdns] }
        )
        coordinator.start()

        XCTAssertTrue(coordinator.canOfferRoomInvite(
            to: routingSelection(hasInviteRoute: false)
        ))

        coordinator.offerInvite(
            to: routingSelection(hasInviteRoute: false),
            invite: "envoix://room/123456-a1b2-c3d4"
        ) { _ in }

        XCTAssertEqual(bluetooth.offeredPeerKeys, ["8899aabbccddeeff"])
        XCTAssertTrue(mdns.offeredPeerKeys.isEmpty)
    }

    func testCoordinatorDoesNotInvokeBluetoothWhenSelectionLacksBluetoothSource() {
        let bluetooth = RoutingNearbyDiscoveryProvider(source: .bluetooth, canOffer: true)
        let coordinator = NearbyDiscoveryCoordinator(
            identity: LocalNearbyDiscoveryIdentity(
                peerKey: "0011223344556677",
                displayName: "iPhone"
            ),
            providerFactory: { _ in [bluetooth] }
        )
        coordinator.start()

        let selection = NearbyPairingSelection(
            discoveryPeerKey: "8899aabbccddeeff",
            displayName: "Nearby phone",
            sources: [.mdns],
            nearbyInviteRoute: nil
        )

        XCTAssertFalse(coordinator.canOfferRoomInvite(to: selection))

        var result: String?
        coordinator.offerInvite(
            to: selection,
            invite: "envoix://room/123456-a1b2-c3d4"
        ) { result = $0 }

        XCTAssertEqual(
            result,
            "Nearby invitation delivery is not available for this device"
        )
        XCTAssertTrue(bluetooth.offeredPeerKeys.isEmpty)
    }

    func testPairingSelectionFreezesCompleteUntrustedRouteWithoutTreatingItAsCredential() {
        let route = testInviteRoute()
        let selection = NearbyPairingSelection(
            peer: NearbyDiscoveredPeer(
                peerKey: "0011223344556677",
                displayName: "Nearby phone",
                sources: [.bluetooth, .mdns],
                lastSeenAtMilliseconds: 42,
                rssi: -36,
                inviteRoute: route
            ),
            nearbyWifiAwareDeviceID: "0000000000000042"
        )

        XCTAssertEqual(selection.discoveryPeerKey, "0011223344556677")
        XCTAssertEqual(selection.displayName, "Nearby phone")
        XCTAssertEqual(selection.sources, [.bluetooth, .mdns])
        XCTAssertEqual(selection.nearbyInviteRoute, route)
        XCTAssertEqual(selection.nearbyWifiAwareDeviceID, "0000000000000042")
    }

    private func routingSelection(
        hasInviteRoute: Bool = true,
        nearbyWifiAwareDeviceID: String? = nil
    ) -> NearbyPairingSelection {
        var sources: Set<NearbyDiscoverySource> = [.bluetooth, .mdns]
        if nearbyWifiAwareDeviceID != nil {
            sources.insert(.wifiAware)
        }
        return NearbyPairingSelection(
            discoveryPeerKey: "8899aabbccddeeff",
            displayName: "Nearby phone",
            sources: sources,
            nearbyInviteRoute: hasInviteRoute ? testInviteRoute() : nil,
            nearbyWifiAwareDeviceID: nearbyWifiAwareDeviceID
        )
    }

    private func testInviteRoute() -> NearbyInviteRoute {
        NearbyInviteRoute(
            endpointID: "2cfu7vzc7zhqv6w3k7m2kkwqvwzppmzvv53lmst6xm7ubjx5qnya",
            relayURL: "https://relay.example.test",
            directAddresses: ["192.0.2.10:4242"]
        )!
    }

    private func rendezvousOffer(requestID: String) -> NearbyRendezvousOffer {
        NearbyRendezvousOffer(
            requestID: requestID,
            senderPeerKey: "8899aabbccddeeff",
            senderDisplayName: "Nearby phone",
            source: .bluetooth,
            senderInboxEndpointID: nil,
            invite: "envoix://room/123456-a1b2-c3d4"
        )
    }

    func testWifiAwareRouteRequiresExactlyOnePairedDevice() throws {
        let wifiAware = try XCTUnwrap(NearbyPairedDevice(
            sourceScopedID: "0000000000000001",
            source: .wifiAware
        ))
        let secondWifiAware = try XCTUnwrap(NearbyPairedDevice(
            sourceScopedID: "0000000000000002",
            source: .wifiAware
        ))
        let bluetooth = try XCTUnwrap(NearbyPairedDevice(
            sourceScopedID: "ble-peer",
            source: .bluetooth
        ))

        XCTAssertNil(uniqueNearbyWifiAwareDeviceID(in: []))
        XCTAssertEqual(
            uniqueNearbyWifiAwareDeviceID(in: [bluetooth, wifiAware]),
            wifiAware.sourceScopedID
        )
        XCTAssertNil(uniqueNearbyWifiAwareDeviceID(in: [wifiAware, secondWifiAware]))
    }
}

private final class AdmissionNearbyDiscoveryProvider:
    NearbyDiscoveryProvider,
    NearbyRendezvousAdmissionConfigurable {
    let source = NearbyDiscoverySource.wifiAware
    private var admission:
        (@MainActor (NearbyRendezvousOffer) -> Bool)?

    func setRendezvousOfferAdmission(
        _ admission: @escaping @MainActor (NearbyRendezvousOffer) -> Bool
    ) {
        self.admission = admission
    }

    func start(sink: @escaping (NearbyDiscoveryEvent) -> Void) {}
    func stop() {}

    func admit(_ offer: NearbyRendezvousOffer) async -> Bool {
        await admission?(offer) ?? false
    }
}

private final class CountingNearbyDiscoveryProvider:
    NearbyDiscoveryProvider,
    NearbyAdvertisingConfigurable,
    NearbyIdentityConfigurable {
    let source: NearbyDiscoverySource
    private var sink: ((NearbyDiscoveryEvent) -> Void)?
    private var startSinks: [((NearbyDiscoveryEvent) -> Void)] = []
    private(set) var startCount = 0
    private(set) var stopCount = 0
    private(set) var advertisingValues: [Bool] = []
    private(set) var advertisingValueAtStart: Bool?
    private(set) var identities: [LocalNearbyDiscoveryIdentity] = []

    init(source: NearbyDiscoverySource) {
        self.source = source
    }

    func start(sink: @escaping (NearbyDiscoveryEvent) -> Void) {
        startCount += 1
        advertisingValueAtStart = advertisingValues.last
        self.sink = sink
        startSinks.append(sink)
    }

    func setAdvertisingEnabled(_ enabled: Bool) {
        advertisingValues.append(enabled)
    }

    func setIdentity(_ identity: LocalNearbyDiscoveryIdentity) {
        identities.append(identity)
    }

    func stop() {
        stopCount += 1
        sink = nil
    }

    func emit(_ event: NearbyDiscoveryEvent) {
        sink?(event)
    }

    func emitFromStart(_ index: Int, event: NearbyDiscoveryEvent) {
        startSinks[index](event)
    }
}

private final class QuiescingNearbyDiscoveryProvider:
    NearbyDiscoveryProvider,
    NearbySystemPairingQuiescing {
    let source = NearbyDiscoverySource.wifiAware
    private(set) var stopCount = 0
    private(set) var waitCount = 0
    private(set) var finishedWaiting = false

    func start(sink: @escaping (NearbyDiscoveryEvent) -> Void) {}

    func stop() {
        stopCount += 1
    }

    func waitUntilStopped() async {
        waitCount += 1
        await Task.yield()
        finishedWaiting = true
    }
}

private final class RoutingNearbyDiscoveryProvider: NearbyRendezvousProvider {
    let source: NearbyDiscoverySource
    private let canOffer: Bool
    private let deliveryError: String?
    private let requiredWifiAwareDeviceID: String?
    private(set) var offeredPeerKeys: [String] = []
    private(set) var offeredWifiAwareDeviceIDs: [String] = []

    init(
        source: NearbyDiscoverySource,
        canOffer: Bool,
        deliveryError: String? = nil,
        requiredWifiAwareDeviceID: String? = nil
    ) {
        self.source = source
        self.canOffer = canOffer
        self.deliveryError = deliveryError
        self.requiredWifiAwareDeviceID = requiredWifiAwareDeviceID
    }

    func start(sink: @escaping (NearbyDiscoveryEvent) -> Void) {}
    func stop() {}

    func canOfferInvite(to selection: NearbyPairingSelection) -> Bool {
        guard canOffer,
              NearbyDiscoveryPeerRegistry.normalizePeerKey(
                  selection.discoveryPeerKey
              ) != nil else {
            return false
        }
        return requiredWifiAwareDeviceID == nil
            || selection.nearbyWifiAwareDeviceID == requiredWifiAwareDeviceID
    }

    func offerInvite(
        to selection: NearbyPairingSelection,
        invite: String,
        completion: @escaping (String?) -> Void
    ) {
        offeredPeerKeys.append(selection.discoveryPeerKey)
        if let nearbyWifiAwareDeviceID = selection.nearbyWifiAwareDeviceID {
            offeredWifiAwareDeviceIDs.append(nearbyWifiAwareDeviceID)
        }
        completion(deliveryError)
    }
}
