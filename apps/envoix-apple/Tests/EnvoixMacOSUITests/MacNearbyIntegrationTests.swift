#if os(macOS)
import XCTest
@testable import Envoix

final class MacNearbyIntegrationTests: XCTestCase {
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

    @MainActor
    func testDefaultCoordinatorReportsWifiAwareUnsupported() {
        let coordinator = NearbyDiscoveryCoordinator()
        coordinator.start()
        defer { coordinator.stop() }

        XCTAssertEqual(
            coordinator.state.statuses[.wifiAware]?.availability,
            .unsupported
        )
        XCTAssertEqual(
            coordinator.state.statuses[.wifiAware]?.detail,
            .wifiAwareUnsupported
        )
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
