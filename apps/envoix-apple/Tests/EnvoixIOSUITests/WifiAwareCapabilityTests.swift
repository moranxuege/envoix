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
        XCTAssertEqual(envoixWifiAwareProbeService, "_envoix-probe._tcp")
        XCTAssertEqual(envoixWifiAwareTransferService, "_envoix._udp")
    }

    func testProbeProtocolRoundTripAndRejectsCorruption() throws {
        let nonce = Data(0 ..< UInt8(WifiAwareProbeProtocol.nonceLength))
        let request = try WifiAwareProbeProtocol.makeRequest(nonce: nonce)
        let response = try WifiAwareProbeProtocol.makeResponse(for: request)

        XCTAssertEqual(request.count, WifiAwareProbeProtocol.frameLength)
        XCTAssertNoThrow(try WifiAwareProbeProtocol.validateResponse(response, nonce: nonce))

        var corrupted = response
        corrupted[corrupted.index(before: corrupted.endIndex)] ^= 0xff
        XCTAssertThrowsError(try WifiAwareProbeProtocol.validateResponse(corrupted, nonce: nonce)) {
            XCTAssertEqual($0 as? WifiAwareProbeProtocolError, .nonceMismatch)
        }
    }

    func testProbeProtocolRejectsInvalidFrameAndNonceLengths() {
        XCTAssertThrowsError(try WifiAwareProbeProtocol.makeRequest(nonce: Data())) {
            XCTAssertEqual($0 as? WifiAwareProbeProtocolError, .invalidNonceLength)
        }
        XCTAssertThrowsError(try WifiAwareProbeProtocol.makeResponse(for: Data())) {
            XCTAssertEqual($0 as? WifiAwareProbeProtocolError, .invalidFrameLength)
        }
    }

    func testProbeFrameAccumulatorHandlesFragmentedInput() throws {
        let nonce = Data(0 ..< UInt8(WifiAwareProbeProtocol.nonceLength))
        let frame = try WifiAwareProbeProtocol.makeRequest(nonce: nonce)
        var accumulator = WifiAwareProbeFrameAccumulator()

        XCTAssertNil(try accumulator.append(Data(frame.prefix(7))))
        XCTAssertNil(try accumulator.append(Data(frame.dropFirst(7).prefix(11))))
        XCTAssertEqual(
            try accumulator.append(Data(frame.dropFirst(18))),
            frame
        )
        XCTAssertEqual(try accumulator.finish(), frame)
    }

    func testProbeFrameAccumulatorRejectsTruncatedAndOversizedInput() throws {
        var truncated = WifiAwareProbeFrameAccumulator()
        XCTAssertNil(try truncated.append(Data(repeating: 0, count: WifiAwareProbeProtocol.frameLength - 1)))
        XCTAssertThrowsError(try truncated.finish()) {
            XCTAssertEqual($0 as? WifiAwareProbeProtocolError, .invalidFrameLength)
        }

        var oversized = WifiAwareProbeFrameAccumulator()
        XCTAssertThrowsError(
            try oversized.append(Data(repeating: 0, count: WifiAwareProbeProtocol.frameLength + 1))
        ) {
            XCTAssertEqual($0 as? WifiAwareProbeProtocolError, .invalidFrameLength)
        }
    }

    func testProbeAttemptGateRejectsCancelledCompletedAndLateCallbacks() {
        var gate = WifiAwareProbeAttemptGate()
        let first = gate.begin()
        XCTAssertTrue(gate.accepts(first))

        gate.cancel()
        let tokenAfterCancel = gate.currentToken
        gate.cancel()
        XCTAssertEqual(gate.currentToken, tokenAfterCancel, "Stop must be idempotent")
        XCTAssertFalse(gate.accepts(first))

        let second = gate.begin()
        XCTAssertFalse(gate.accepts(first))
        XCTAssertTrue(gate.accepts(second))
        XCTAssertTrue(gate.complete(second))
        XCTAssertFalse(gate.accepts(second))
        XCTAssertFalse(gate.complete(second), "A late completion must be ignored")
    }

    func testProbeTimeoutAndCallerCancellation() async throws {
        guard #available(iOS 26.0, *) else {
            throw XCTSkip("The Network.framework probe requires iOS 26")
        }

        do {
            _ = try await withProbeTimeout(.milliseconds(20)) {
                try await Task<Never, Never>.sleep(for: .seconds(5))
                return 1
            }
            XCTFail("The probe operation should time out")
        } catch {
            XCTAssertEqual(error as? AppleWifiAwareProbeError, .timedOut)
        }

        let operation = Task {
            try await withProbeTimeout(.seconds(5)) {
                try await Task<Never, Never>.sleep(for: .seconds(5))
                return 1
            }
        }
        operation.cancel()
        do {
            _ = try await operation.value
            XCTFail("Caller cancellation should stop the probe")
        } catch is CancellationError {
            // Expected.
        } catch {
            XCTFail("Expected CancellationError, received \(type(of: error))")
        }
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
