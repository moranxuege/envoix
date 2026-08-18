import XCTest
import EnvoixCore
#if os(iOS)
@testable import Envoix_iOS
#elseif os(macOS)
@testable import Envoix
#endif

final class ApplicationEngineAdapterTests: XCTestCase {
    func testBindingNegotiationRejectsEveryMismatchedVersion() throws {
        let core = compatibleCoreInfo()
        let binding = compatibleBindingInfo()
        XCTAssertNoThrow(try validateApplicationBinding(core: core, binding: binding))

        XCTAssertThrowsError(
            try validateApplicationBinding(
                core: FfiCoreInfo(
                    ffiApiVersion: 15,
                    coreVersion: core.coreVersion,
                    capabilities: core.capabilities
                ),
                binding: binding
            )
        )
        XCTAssertThrowsError(
            try validateApplicationBinding(
                core: core,
                binding: FfiApplicationBindingInfo(bindingVersion: 2, contractVersion: 6)
            )
        )
        XCTAssertThrowsError(
            try validateApplicationBinding(
                core: core,
                binding: FfiApplicationBindingInfo(bindingVersion: 1, contractVersion: 5)
            )
        )
        XCTAssertThrowsError(
            try validateApplicationBinding(
                core: FfiCoreInfo(
                    ffiApiVersion: expectedCoreFFIAPIVersion,
                    coreVersion: core.coreVersion,
                    capabilities: []
                ),
                binding: binding
            )
        )
    }

    func testTypedEventsRebuildSnapshotAndReportGaps() async throws {
        let adapter = try ApplicationEngineAdapter()
        let observed = FfiApplicationEventEnvelope(
            contractVersion: expectedApplicationContractVersion,
            sequence: 1,
            event: .deviceObserved(
                deviceId: "device_binding_fixture",
                displayName: "Binding Fixture"
            )
        )

        XCTAssertEqual(try await adapter.apply(observed), .applied)
        XCTAssertEqual(try await adapter.apply(observed), .ignoredDuplicate)
        let snapshot = try await adapter.snapshot()
        XCTAssertEqual(snapshot.lastSequence, 1)
        XCTAssertEqual(snapshot.devices.map(\.id), ["device_binding_fixture"])

        let gap = FfiApplicationEventEnvelope(
            contractVersion: expectedApplicationContractVersion,
            sequence: 3,
            event: .deviceObserved(
                deviceId: "device_gap_fixture",
                displayName: "Gap Fixture"
            )
        )
        do {
            _ = try await adapter.apply(gap)
            XCTFail("event gap should fail")
        } catch let error as FfiApplicationError {
            guard case let .failed(code, _) = error else {
                return XCTFail("unexpected application error: \(error)")
            }
            XCTAssertEqual(code, .eventGap)
        }
        await adapter.close()
    }

    func testActorChecksCancellationBeforeCallingFFI() async throws {
        let fake = FakeApplicationEngine()
        let adapter = try ApplicationEngineAdapter(
            engine: fake,
            core: compatibleCoreInfo(),
            binding: compatibleBindingInfo()
        )
        let operation = Task {
            withUnsafeCurrentTask { $0?.cancel() }
            return try await adapter.snapshot()
        }

        do {
            _ = try await operation.value
            XCTFail("canceled operation should fail")
        } catch is CancellationError {
            XCTAssertEqual(fake.snapshotCalls, 0)
        }
        await adapter.close()
    }

    func testCloseReleasesTheHandleAndRejectsLaterCalls() async throws {
        let fake = FakeApplicationEngine()
        let adapter = try ApplicationEngineAdapter(
            engine: fake,
            core: compatibleCoreInfo(),
            binding: compatibleBindingInfo()
        )
        await adapter.close()
        await adapter.close()

        do {
            _ = try await adapter.snapshot()
            XCTFail("closed adapter should fail")
        } catch let error as ApplicationEngineAdapterError {
            XCTAssertEqual(error, .closed)
        }
    }

    private func compatibleCoreInfo() -> FfiCoreInfo {
        FfiCoreInfo(
            ffiApiVersion: expectedCoreFFIAPIVersion,
            coreVersion: "test",
            capabilities: ["typed_application_contract_v6"]
        )
    }

    private func compatibleBindingInfo() -> FfiApplicationBindingInfo {
        FfiApplicationBindingInfo(
            bindingVersion: expectedApplicationBindingVersion,
            contractVersion: expectedApplicationContractVersion
        )
    }
}

private final class FakeApplicationEngine: FfiApplicationEngineProtocol, @unchecked Sendable {
    private(set) var snapshotCalls = 0

    func snapshot() throws -> FfiApplicationSnapshot {
        snapshotCalls += 1
        return FfiApplicationSnapshot(
            contractVersion: expectedApplicationContractVersion,
            lastSequence: 0,
            capabilities: FfiPlatformCapabilities(values: []),
            devices: [],
            relationships: [],
            rooms: [],
            transfers: []
        )
    }

    func apply(envelope: FfiApplicationEventEnvelope) throws -> FfiApplyOutcome {
        .applied
    }

    func decide(
        envelope: FfiApplicationCommandEnvelope
    ) throws -> FfiApplicationEffectEnvelope {
        FfiApplicationEffectEnvelope(
            contractVersion: expectedApplicationContractVersion,
            commandId: envelope.commandId,
            effect: .createRoom
        )
    }
}
