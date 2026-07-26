import XCTest
@testable import Envoix_iOS

final class NearbyDiscoveryTests: XCTestCase {
    func testBleRendezvousRoundTripsFragmentedInvite() throws {
        let identity = LocalNearbyDiscoveryIdentity(
            peerKey: "0011223344556677",
            displayName: "iPhone"
        )
        let invite = "envoix://pair/123456-alpha-bravo?broker=https%3A%2F%2Fexample.test&role=send"
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
        let invite = "envoix://pair/123456-alpha-bravo?role=receive"
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
            invite: "envoix://pair/123456-alpha-bravo?role=send",
            requestID: 7,
            maximumFrameBytes: 512
        )?.first)
        frame[BleRendezvousProtocol.frameHeaderSize] = 1
        XCTAssertNil(BleRendezvousProtocol.Assembler().accept(frame))
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

    func testBonjourRecordMatchesAndroidKeysAndBoundsName() throws {
        let record = try XCTUnwrap(NearbyDiscoveryBonjourRecord(dictionary: [
            "v": "1",
            "id": "AABBCCDDEEFF0011",
            "name": "  test   device  ",
        ]))

        XCTAssertEqual(record.peerKey, "aabbccddeeff0011")
        XCTAssertEqual(record.displayName, "test device")
        XCTAssertEqual(record.dictionary["v"], "1")
        XCTAssertEqual(record.dictionary["id"], "aabbccddeeff0011")
        XCTAssertNil(NearbyDiscoveryBonjourRecord(dictionary: ["v": "2", "id": record.peerKey]))

        let longName = String(repeating: "x", count: 60)
        XCTAssertEqual(
            NearbyDiscoveryBonjourRecord(dictionary: ["v": "1", "id": record.peerKey, "name": longName])?
                .displayName?.count,
            NearbyDiscoveryPeerRegistry.maximumDisplayNameLength
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
            displayName: "Phone"
        )))

        var peers = registry.peers(nowMilliseconds: 10_000)
        XCTAssertEqual(peers.count, 1)
        XCTAssertEqual(peers[0].sources, [.bluetooth, .mdns])
        XCTAssertEqual(peers[0].displayName, "Phone")
        XCTAssertEqual(peers[0].rssi, -51)

        peers = registry.peers(nowMilliseconds: 20_001)
        XCTAssertEqual(peers.count, 1)
        XCTAssertEqual(peers[0].sources, [.mdns])
        XCTAssertNil(peers[0].rssi)

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

    func testCoordinatorRotatesIdentityAcrossPresenceSessions() {
        var identities = [
            LocalNearbyDiscoveryIdentity(peerKey: "0011223344556677", displayName: "first"),
            LocalNearbyDiscoveryIdentity(peerKey: "8899aabbccddeeff", displayName: "second"),
        ]
        var advertisedPeerKeys: [String] = []
        let coordinator = NearbyDiscoveryCoordinator(
            identityFactory: { identities.removeFirst() },
            providerFactory: { identity in
                advertisedPeerKeys.append(identity.peerKey)
                return [CountingNearbyDiscoveryProvider(source: .bluetooth)]
            }
        )

        coordinator.start()
        coordinator.stop()
        coordinator.start()

        XCTAssertEqual(advertisedPeerKeys, ["0011223344556677", "8899aabbccddeeff"])
        XCTAssertEqual(coordinator.state.localName, "second")
    }

    func testCoordinatorRestartKeepsPresenceIdentityAndDoesNotAccumulateSelfGhosts() {
        let firstIdentity = LocalNearbyDiscoveryIdentity(
            peerKey: "0011223344556677",
            displayName: "first"
        )
        var identities = [
            firstIdentity,
            LocalNearbyDiscoveryIdentity(peerKey: "8899aabbccddeeff", displayName: "second"),
            LocalNearbyDiscoveryIdentity(peerKey: "1111222233334444", displayName: "third"),
            LocalNearbyDiscoveryIdentity(peerKey: "aaaabbbbccccdddd", displayName: "fourth"),
        ]
        let initialProvider = CountingNearbyDiscoveryProvider(source: .mdns)
        let firstRefreshProvider = CountingNearbyDiscoveryProvider(source: .mdns)
        let secondRefreshProvider = CountingNearbyDiscoveryProvider(source: .mdns)
        let resumedProvider = CountingNearbyDiscoveryProvider(source: .mdns)
        var providers = [initialProvider, firstRefreshProvider, secondRefreshProvider, resumedProvider]
        var advertisedPeerKeys: [String] = []
        let now: Int64 = 100
        let coordinator = NearbyDiscoveryCoordinator(
            identityFactory: { identities.removeFirst() },
            clock: { now },
            providerFactory: { identity in
                advertisedPeerKeys.append(identity.peerKey)
                return [providers.removeFirst()]
            }
        )

        coordinator.start()
        coordinator.restart()
        coordinator.restart()
        secondRefreshProvider.emit(.observation(NearbyDiscoveryObservation(
            peerKey: firstIdentity.peerKey,
            source: .mdns,
            seenAtMilliseconds: now,
            displayName: firstIdentity.displayName
        )))

        XCTAssertEqual(advertisedPeerKeys, [
            firstIdentity.peerKey,
            firstIdentity.peerKey,
            firstIdentity.peerKey,
        ])
        XCTAssertTrue(coordinator.state.peers.isEmpty)

        coordinator.stop()
        coordinator.start()
        XCTAssertEqual(advertisedPeerKeys.last, "8899aabbccddeeff")
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
            peerKey: "0011223344556677",
            source: .bluetooth,
            seenAtMilliseconds: 1
        ))

        registry.clear()

        XCTAssertTrue(registry.peers(nowMilliseconds: 1).isEmpty)
    }

    func testCoordinatorRejectsCallbacksFromStoppedGeneration() {
        let staleProvider = CountingNearbyDiscoveryProvider(source: .bluetooth)
        let activeProvider = CountingNearbyDiscoveryProvider(source: .bluetooth)
        var startCount = 0
        var now: Int64 = 1
        let coordinator = NearbyDiscoveryCoordinator(
            identity: LocalNearbyDiscoveryIdentity(
                peerKey: "0011223344556677",
                displayName: "iPhone"
            ),
            clock: { now },
            providerFactory: { _ in
                defer { startCount += 1 }
                return [startCount == 0 ? staleProvider : activeProvider]
            }
        )

        coordinator.start()
        coordinator.stop()
        coordinator.start()
        activeProvider.emit(.observation(NearbyDiscoveryObservation(
            peerKey: "1111222233334444",
            source: .bluetooth,
            seenAtMilliseconds: 1
        )))
        now = 2
        staleProvider.emitAfterStop(.observation(NearbyDiscoveryObservation(
            peerKey: "aaaabbbbccccdddd",
            source: .bluetooth,
            seenAtMilliseconds: 2
        )))

        XCTAssertEqual(coordinator.state.peers.map(\.peerKey), ["1111222233334444"])
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
        let staleProvider = CountingNearbyDiscoveryProvider(source: .wifiAware)
        let activeProvider = CountingNearbyDiscoveryProvider(source: .wifiAware)
        var providerIndex = 0
        let coordinator = NearbyDiscoveryCoordinator(
            identity: LocalNearbyDiscoveryIdentity(
                peerKey: "0011223344556677",
                displayName: "iPhone"
            ),
            providerFactory: { _ in
                defer { providerIndex += 1 }
                return [providerIndex == 0 ? staleProvider : activeProvider]
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
        activeProvider.emit(.pairedDevices(source: .wifiAware, devices: [activeDevice]))
        activeProvider.emit(.pairedDevices(source: .wifiAware, devices: [mismatchedDevice]))
        staleProvider.emitAfterStop(.pairedDevices(source: .wifiAware, devices: []))

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

    func testPairingSelectionCarriesOnlyUntrustedDisplayContext() {
        let selection = NearbyPairingSelection(peer: NearbyDiscoveredPeer(
            peerKey: "0011223344556677",
            displayName: "Nearby phone",
            sources: [.bluetooth, .mdns],
            lastSeenAtMilliseconds: 42,
            rssi: -36,
            endpoint: "192.0.2.10:4242"
        ))

        XCTAssertEqual(selection.discoveryPeerKey, "0011223344556677")
        XCTAssertEqual(selection.displayName, "Nearby phone")
        XCTAssertEqual(selection.sources, [.bluetooth, .mdns])
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

private final class CountingNearbyDiscoveryProvider: NearbyDiscoveryProvider {
    let source: NearbyDiscoverySource
    private var sink: ((NearbyDiscoveryEvent) -> Void)?
    private var lastSink: ((NearbyDiscoveryEvent) -> Void)?
    private(set) var startCount = 0
    private(set) var stopCount = 0

    init(source: NearbyDiscoverySource) {
        self.source = source
    }

    func start(sink: @escaping (NearbyDiscoveryEvent) -> Void) {
        startCount += 1
        self.sink = sink
        lastSink = sink
    }

    func stop() {
        stopCount += 1
        sink = nil
    }

    func emit(_ event: NearbyDiscoveryEvent) {
        sink?(event)
    }

    func emitAfterStop(_ event: NearbyDiscoveryEvent) {
        lastSink?(event)
    }
}
