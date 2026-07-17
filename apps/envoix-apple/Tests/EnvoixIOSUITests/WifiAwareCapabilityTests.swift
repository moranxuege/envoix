import XCTest
@testable import Envoix_iOS

final class WifiAwareCapabilityTests: XCTestCase {
    func testAvailabilityPolicyCoversEveryStructuredState() {
        let cases: [(WifiAwareCapabilityFacts, WifiAwareAvailability)] = [
            (readyFacts(osSupported: false), .unsupportedOS),
            (readyFacts(hardwareSupported: false), .unsupportedHardware),
            (readyFacts(pairingSupported: false), .unsupportedHardware),
            (readyFacts(entitlementPresent: false), .entitlementMissing),
            (readyFacts(permissionState: .required), .permissionRequired),
            (readyFacts(permissionState: .denied), .permissionDenied),
            (readyFacts(wifiEnabled: false), .wifiDisabled),
            (
                readyFacts(
                    wifiEnabled: false,
                    temporarilyAvailable: false,
                    pairingSupported: nil
                ),
                .wifiDisabled
            ),
            (readyFacts(serviceDeclared: false), .temporarilyUnavailable),
            (readyFacts(temporarilyAvailable: false), .temporarilyUnavailable),
            (readyFacts(pairingSupported: nil), .temporarilyUnavailable),
            (readyFacts(pairedDeviceCount: nil), .temporarilyUnavailable),
            (readyFacts(pairedDeviceCount: 0), .pairingRequired),
            (readyFacts(), .ready),
        ]

        for (facts, expected) in cases {
            XCTAssertEqual(WifiAwareCapabilityPolicy.evaluate(facts).availability, expected)
        }
    }

    func testWireNamesAndServiceIdentifierAreStable() {
        XCTAssertEqual(
            WifiAwareAvailability.allCases.map(\.rawValue),
            [
                "unsupported_os",
                "unsupported_hardware",
                "entitlement_missing",
                "permission_required",
                "permission_denied",
                "wifi_disabled",
                "temporarily_unavailable",
                "pairing_required",
                "ready",
            ]
        )
        XCTAssertEqual(envoixWifiAwareService, "_envoix._udp")
    }

    func testPhysicalDeviceReportsWifiAwarePairingCapability() async throws {
        #if targetEnvironment(simulator)
        throw XCTSkip("Wi-Fi Aware requires supported physical hardware")
        #else
        guard #available(iOS 26.0, *) else {
            throw XCTSkip("Wi-Fi Aware requires iOS or iPadOS 26")
        }

        let snapshot = await AppleWifiAwareCapabilityProbe.read()
        let pairing = snapshot.pairingSupported.map { String($0) } ?? "unknown"
        let pairedDevices = snapshot.pairedDeviceCount.map { String($0) } ?? "unknown"
        let evidence =
            "availability=\(snapshot.availability.rawValue) " +
            "pairing_supported=\(pairing) paired_device_count=\(pairedDevices)"
        XCTContext.runActivity(named: "Wi-Fi Aware gate: \(evidence)") { _ in }

        XCTAssertEqual(snapshot.pairingSupported, true, evidence)
        XCTAssertNotNil(snapshot.pairedDeviceCount, evidence)
        XCTAssertTrue(
            snapshot.availability == .pairingRequired || snapshot.availability == .ready,
            evidence
        )
        #endif
    }

    private func readyFacts(
        osSupported: Bool = true,
        hardwareSupported: Bool = true,
        entitlementPresent: Bool = true,
        permissionState: WifiAwarePermissionState = .granted,
        wifiEnabled: Bool = true,
        serviceDeclared: Bool = true,
        temporarilyAvailable: Bool = true,
        pairingSupported: Bool? = true,
        pairedDeviceCount: Int? = 1
    ) -> WifiAwareCapabilityFacts {
        WifiAwareCapabilityFacts(
            osSupported: osSupported,
            hardwareSupported: hardwareSupported,
            entitlementPresent: entitlementPresent,
            permissionState: permissionState,
            wifiEnabled: wifiEnabled,
            serviceDeclared: serviceDeclared,
            temporarilyAvailable: temporarilyAvailable,
            pairingSupported: pairingSupported,
            pairedDeviceCount: pairedDeviceCount
        )
    }
}
