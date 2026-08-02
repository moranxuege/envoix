import XCTest
#if os(macOS)
@testable import Envoix
#else
@testable import Envoix_iOS
#endif

final class AppleWifiAwareControlChannelRegistryTests: XCTestCase {
    func testInboundChannelIsSelectableWithoutABrowserEndpoint() throws {
        guard #available(iOS 26.0, *) else {
            throw XCTSkip("Wi-Fi Aware requires iOS or iPadOS 26")
        }
        var registry = AppleWifiAwareControlChannelRegistry<String>()
        let entry = makeEntry(
            id: UUID(uuidString: "00000000-0000-0000-0000-000000000001")!,
            direction: .inboundPublisher,
            value: "inbound"
        )

        registry.register(entry)

        XCTAssertTrue(registry.contains(deviceID: 7))
        XCTAssertEqual(
            registry.selected(for: 7, preferredRole: .publisher)?.value,
            "inbound"
        )
    }

    func testPreferredRoleChoosesComplementaryPhysicalChannel() throws {
        guard #available(iOS 26.0, *) else {
            throw XCTSkip("Wi-Fi Aware requires iOS or iPadOS 26")
        }
        var registry = AppleWifiAwareControlChannelRegistry<String>()
        registry.register(makeEntry(
            id: UUID(uuidString: "00000000-0000-0000-0000-000000000001")!,
            direction: .inboundPublisher,
            value: "inbound"
        ))
        registry.register(makeEntry(
            id: UUID(uuidString: "00000000-0000-0000-0000-000000000002")!,
            direction: .outboundSubscriber,
            value: "outbound"
        ))

        XCTAssertEqual(
            registry.selected(for: 7, preferredRole: .subscriber)?.value,
            "outbound"
        )
        XCTAssertEqual(
            registry.selected(for: 7, preferredRole: .publisher)?.value,
            "inbound"
        )
    }

    func testLateCloseCannotRemoveSameDirectionReplacement() throws {
        guard #available(iOS 26.0, *) else {
            throw XCTSkip("Wi-Fi Aware requires iOS or iPadOS 26")
        }
        var registry = AppleWifiAwareControlChannelRegistry<String>()
        let oldID = UUID(uuidString: "00000000-0000-0000-0000-000000000001")!
        let newID = UUID(uuidString: "00000000-0000-0000-0000-000000000002")!
        registry.register(makeEntry(
            id: oldID,
            direction: .outboundSubscriber,
            value: "old"
        ))
        let replaced = registry.register(makeEntry(
            id: newID,
            direction: .outboundSubscriber,
            value: "new"
        ))

        XCTAssertEqual(replaced?.channelID, oldID)
        XCTAssertNil(registry.remove(deviceID: 7, channelID: oldID))
        XCTAssertEqual(
            registry.selected(for: 7, preferredRole: .subscriber)?.value,
            "new"
        )
    }

    func testRetainRemovesOnlyEntriesForUnpairedDevices() throws {
        guard #available(iOS 26.0, *) else {
            throw XCTSkip("Wi-Fi Aware requires iOS or iPadOS 26")
        }
        var registry = AppleWifiAwareControlChannelRegistry<String>()
        registry.register(makeEntry(
            id: UUID(uuidString: "00000000-0000-0000-0000-000000000001")!,
            deviceID: 7,
            direction: .inboundPublisher,
            value: "retained"
        ))
        registry.register(makeEntry(
            id: UUID(uuidString: "00000000-0000-0000-0000-000000000002")!,
            deviceID: 8,
            direction: .outboundSubscriber,
            value: "removed"
        ))

        let removed = registry.retain(deviceIDs: [7])

        XCTAssertEqual(removed.map(\.value), ["removed"])
        XCTAssertTrue(registry.contains(deviceID: 7))
        XCTAssertFalse(registry.contains(deviceID: 8))
    }

    private func makeEntry(
        id: UUID,
        deviceID: UInt64 = 7,
        direction: AppleWifiAwareControlChannelDirection,
        value: String
    ) -> AppleWifiAwareControlChannelRegistry<String>.Entry {
        AppleWifiAwareControlChannelRegistry<String>.Entry(
            channelID: id,
            deviceID: deviceID,
            direction: direction,
            remoteIdentity: LocalNearbyDiscoveryIdentity(
                peerKey: "8899aabbccddeeff",
                displayName: "Nearby phone"
            ),
            value: value
        )
    }
}
