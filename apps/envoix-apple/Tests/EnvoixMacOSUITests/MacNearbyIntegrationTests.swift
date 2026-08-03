#if os(macOS)
import Foundation
import XCTest
@testable import Envoix

final class MacNearbyIntegrationTests: XCTestCase {
    func testBluetoothIdentityMatchesCrossPlatformWireVector() throws {
        let identity = LocalNearbyDiscoveryIdentity(
            peerKey: "0011223344556677",
            displayName: "设备"
        )

        let encoded = try XCTUnwrap(
            BleRendezvousProtocol.encodeIdentity(identity: identity)
        )

        XCTAssertEqual(
            encoded.map { String(format: "%02x", $0) }.joined(),
            "01303031313232333334343535363637370006e8aebee5a487"
        )
        XCTAssertEqual(
            BleRendezvousProtocol.decodeIdentity(encoded),
            identity
        )
    }

    func testBluetoothIdentityRejectsMalformedPayloadsAndControlNames() throws {
        let identity = LocalNearbyDiscoveryIdentity(
            peerKey: "0011223344556677",
            displayName: "Nearby Mac"
        )
        let encoded = try XCTUnwrap(
            BleRendezvousProtocol.encodeIdentity(identity: identity)
        )

        var wrongVersion = encoded
        wrongVersion[0] = 2
        XCTAssertNil(BleRendezvousProtocol.decodeIdentity(wrongVersion))

        var wrongPeerKey = encoded
        wrongPeerKey[1] = 0x7a
        XCTAssertNil(BleRendezvousProtocol.decodeIdentity(wrongPeerKey))

        XCTAssertNil(BleRendezvousProtocol.decodeIdentity(Data(encoded.dropLast())))

        var invalidUTF8 = encoded
        invalidUTF8[invalidUTF8.index(before: invalidUTF8.endIndex)] = 0xff
        XCTAssertNil(BleRendezvousProtocol.decodeIdentity(invalidUTF8))

        var oversizedName = Data([1])
        oversizedName.append(contentsOf: identity.peerKey.utf8)
        oversizedName.append(contentsOf: [0, 193])
        oversizedName.append(Data(repeating: 0x61, count: 193))
        XCTAssertNil(BleRendezvousProtocol.decodeIdentity(oversizedName))

        let controlledIdentity = LocalNearbyDiscoveryIdentity(
            peerKey: identity.peerKey,
            displayName: "Nearby\u{0000}Mac"
        )
        XCTAssertNil(
            BleRendezvousProtocol.encodeIdentity(identity: controlledIdentity)
        )
    }

    func testBluetoothProvisionalNameUsesStrictServiceDataThenLocalName() {
        XCTAssertEqual(
            BleRendezvousProtocol.decodeProvisionalDisplayName(
                serviceData: Data("设备".utf8),
                localName: "Fallback"
            ),
            "设备"
        )
        XCTAssertEqual(
            BleRendezvousProtocol.decodeProvisionalDisplayName(
                serviceData: Data("abcdefghijklm".utf8),
                localName: "Fallback"
            ),
            "abcdefghijklm"
        )
        XCTAssertEqual(
            BleRendezvousProtocol.decodeProvisionalDisplayName(
                serviceData: Data(repeating: 0x61, count: 14),
                localName: "  Nearby   Android "
            ),
            "Nearby Android"
        )
        XCTAssertNil(
            BleRendezvousProtocol.decodeProvisionalDisplayName(
                serviceData: Data([0xff]),
                localName: "Bad\u{0000}Name"
            )
        )
    }

    func testRegistryKeepsCompleteBonjourNameAfterBluetoothRefresh() throws {
        let registry = NearbyDiscoveryPeerRegistry()
        registry.upsert(NearbyDiscoveryObservation(
            peerKey: "0011223344556677",
            source: .bluetooth,
            seenAtMilliseconds: 10,
            displayName: "Nearby Andr"
        ))
        registry.upsert(NearbyDiscoveryObservation(
            peerKey: "0011223344556677",
            source: .mdns,
            seenAtMilliseconds: 20,
            displayName: "Nearby Android Phone"
        ))
        registry.upsert(NearbyDiscoveryObservation(
            peerKey: "0011223344556677",
            source: .bluetooth,
            seenAtMilliseconds: 30,
            displayName: "Nearby Andr"
        ))

        let peer = try XCTUnwrap(registry.peers(nowMilliseconds: 30).first)
        XCTAssertEqual(peer.displayName, "Nearby Android Phone")
        XCTAssertEqual(peer.sources, [.bluetooth, .mdns])
    }

    func testRegistryCapsPeersButKeepsUpdatesAndReclaimsExpiredCapacity() throws {
        let registry = NearbyDiscoveryPeerRegistry(observationTTLMilliseconds: 100)
        for index in 0..<NearbyDiscoveryPeerRegistry.maximumPeerCount {
            XCTAssertTrue(registry.upsert(NearbyDiscoveryObservation(
                peerKey: String(format: "%016llx", UInt64(index)),
                source: .bluetooth,
                seenAtMilliseconds: 0
            )))
        }

        let overflowPeerKey = String(
            format: "%016llx",
            UInt64(NearbyDiscoveryPeerRegistry.maximumPeerCount)
        )
        XCTAssertFalse(registry.upsert(NearbyDiscoveryObservation(
            peerKey: overflowPeerKey,
            source: .bluetooth,
            seenAtMilliseconds: 1
        )))
        XCTAssertTrue(registry.upsert(NearbyDiscoveryObservation(
            peerKey: "0000000000000000",
            source: .bluetooth,
            seenAtMilliseconds: 1,
            displayName: "Updated"
        )))
        XCTAssertEqual(registry.peers(nowMilliseconds: 1).count, 64)

        XCTAssertTrue(registry.peers(nowMilliseconds: 102).isEmpty)
        XCTAssertTrue(registry.upsert(NearbyDiscoveryObservation(
            peerKey: overflowPeerKey,
            source: .bluetooth,
            seenAtMilliseconds: 102
        )))
        XCTAssertEqual(registry.peers(nowMilliseconds: 102).map(\.peerKey), [
            overflowPeerKey,
        ])
    }

    func testBluetoothIdentityReadLimiterBacksOffOnlyTheSamePeer() {
        let limiter = AppleBluetoothIdentityReadAttemptLimiter(
            maximumAttempts: 16,
            windowMilliseconds: 30_000,
            peerBackoffMilliseconds: 5_000
        )

        XCTAssertTrue(limiter.tryAcquire(peerKey: "peer-a", nowMilliseconds: 0))
        XCTAssertFalse(limiter.tryAcquire(peerKey: "peer-a", nowMilliseconds: 4_999))
        XCTAssertTrue(limiter.tryAcquire(peerKey: "peer-b", nowMilliseconds: 4_999))
        XCTAssertTrue(limiter.tryAcquire(peerKey: "peer-a", nowMilliseconds: 5_000))
    }

    func testBluetoothIdentityReadLimiterBoundsRollingWindow() {
        let limiter = AppleBluetoothIdentityReadAttemptLimiter(
            maximumAttempts: 16,
            windowMilliseconds: 30_000,
            peerBackoffMilliseconds: 5_000
        )

        for index in 0..<16 {
            XCTAssertTrue(limiter.tryAcquire(
                peerKey: "peer-\(index)",
                nowMilliseconds: Int64(index)
            ))
        }
        XCTAssertFalse(limiter.tryAcquire(peerKey: "overflow", nowMilliseconds: 29_999))
        XCTAssertTrue(limiter.tryAcquire(peerKey: "retry", nowMilliseconds: 30_000))
        XCTAssertFalse(limiter.tryAcquire(peerKey: "still-full", nowMilliseconds: 30_000))
        XCTAssertTrue(limiter.tryAcquire(peerKey: "next-slot", nowMilliseconds: 30_001))
    }

    func testBluetoothIdentityReadLimiterRetriesFailedPeerAcrossWindows() {
        let limiter = AppleBluetoothIdentityReadAttemptLimiter(
            maximumAttempts: 1,
            windowMilliseconds: 30_000,
            peerBackoffMilliseconds: 5_000
        )

        for window in 0..<1_000 {
            XCTAssertTrue(limiter.tryAcquire(
                peerKey: "failed-peer",
                nowMilliseconds: Int64(window) * 30_000
            ))
        }
    }

    func testWifiAwareProviderReportsUnsupported() throws {
        let provider = UnsupportedWifiAwareDiscoveryProvider()
        var reportedStatus: NearbyProviderStatus?

        provider.start { event in
            guard case .status(let status) = event else { return }
            reportedStatus = status
        }

        let status = try XCTUnwrap(reportedStatus)
        XCTAssertEqual(status.source, .wifiAware)
        XCTAssertEqual(status.availability, .unsupported)
        XCTAssertEqual(status.detail, .wifiAwareUnsupported)
    }

    func testAppDeclaresBonjourDiscoveryService() {
        XCTAssertEqual(
            Bundle.main.object(forInfoDictionaryKey: "NSBonjourServices") as? [String],
            ["_envoix-disc._udp"]
        )
    }

    func testAppDeclaresBluetoothUsageDescription() {
        XCTAssertEqual(
            Bundle.main.object(forInfoDictionaryKey: "NSBluetoothAlwaysUsageDescription")
                as? String,
            "Envoix uses Bluetooth to find and be visible to nearby Envoix devices while the Nearby page is open."
        )
    }

    func testDefaultProviderFactoryIncludesBluetoothAndWifiAwareFallback() {
        let providers = NearbyDiscoveryCoordinator.defaultProviderFactory(
            identity: LocalNearbyDiscoveryIdentity(
                peerKey: "0011223344556677",
                displayName: "Test Mac"
            )
        )

        XCTAssertEqual(providers.map(\.source), [.bluetooth, .mdns, .wifiAware])
        XCTAssertTrue(providers[0] is AppleBluetoothDiscoveryProvider)
        XCTAssertTrue(providers[2] is UnsupportedWifiAwareDiscoveryProvider)
    }

    func testAppDeclaresRoomURLSchemeAndEncryptionClassification() throws {
        let urlTypes = try XCTUnwrap(
            Bundle.main.object(forInfoDictionaryKey: "CFBundleURLTypes")
                as? [[String: Any]]
        )
        let schemes = urlTypes.flatMap {
            $0["CFBundleURLSchemes"] as? [String] ?? []
        }

        XCTAssertTrue(schemes.contains("envoix"))
        XCTAssertEqual(
            Bundle.main.object(forInfoDictionaryKey: "ITSAppUsesNonExemptEncryption")
                as? Bool,
            false
        )
    }
}
#endif
