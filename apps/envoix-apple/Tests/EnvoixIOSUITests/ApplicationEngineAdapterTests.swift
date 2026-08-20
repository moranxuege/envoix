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
                    capabilities: [expectedTypedApplicationCapability]
                ),
                binding: binding
            )
        )
        XCTAssertThrowsError(
            try validateApplicationBinding(
                core: FfiCoreInfo(
                    ffiApiVersion: expectedCoreFFIAPIVersion,
                    coreVersion: core.coreVersion,
                    capabilities: [expectedPersistentApplicationEngineCapability]
                ),
                binding: binding
            )
        )
    }

    func testTypedEventsRebuildSnapshotAndReportGaps() async throws {
        let adapter = try ApplicationEngineAdapter(engine: FfiApplicationEngine())
        let observed = FfiApplicationEventEnvelope(
            contractVersion: expectedApplicationContractVersion,
            sequence: 1,
            event: .deviceObserved(
                deviceId: "device_binding_fixture",
                displayName: "Binding Fixture"
            )
        )

        let applied = try await adapter.apply(observed)
        let duplicate = try await adapter.apply(observed)
        XCTAssertEqual(applied, .applied)
        XCTAssertEqual(duplicate, .ignoredDuplicate)
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
            guard case let .Failed(code, _) = error else {
                return XCTFail("unexpected application error: \(error)")
            }
            XCTAssertEqual(code, .eventGap)
        }
        await adapter.close()
    }

    func testExplicitInMemoryAdaptersOwnIndependentEngines() async throws {
        let first = try ApplicationEngineAdapter(engine: FfiApplicationEngine())
        let second = try ApplicationEngineAdapter(engine: FfiApplicationEngine())
        let observed = FfiApplicationEventEnvelope(
            contractVersion: expectedApplicationContractVersion,
            sequence: 1,
            event: .deviceObserved(
                deviceId: "first_process_device",
                displayName: "First Process Device"
            )
        )

        let applied = try await first.apply(observed)
        let firstSnapshot = try await first.snapshot()
        let secondSnapshot = try await second.snapshot()

        XCTAssertEqual(applied, .applied)
        XCTAssertEqual(firstSnapshot.devices.map(\.id), ["first_process_device"])
        XCTAssertTrue(secondSnapshot.devices.isEmpty)

        await first.close()
        await second.close()
    }

    @MainActor
    func testSharedRuntimeOwnsOneProcessOwnerAndControlWorkflow() {
        let firstSceneLookup = AppleApplicationRuntime.shared
        let secondSceneLookup = AppleApplicationRuntime.shared

        XCTAssertTrue(firstSceneLookup === secondSceneLookup)
        #if os(iOS)
        XCTAssertTrue(
            firstSceneLookup.applicationEngine === secondSceneLookup.applicationEngine
        )
        #elseif os(macOS)
        XCTAssertTrue(
            firstSceneLookup.helperControlClient === secondSceneLookup.helperControlClient
        )
        #endif
        XCTAssertTrue(firstSceneLookup.workflow === secondSceneLookup.workflow)
    }

    @MainActor
    func testMultipleScenesKeepIndependentPresentationAndOneProcessOwner() async throws {
        #if os(iOS)
        let fake = FakeApplicationEngine()
        let adapter = try ApplicationEngineAdapter(
            engine: fake,
            core: compatibleCoreInfo(),
            binding: compatibleBindingInfo()
        )
        let runtime = AppleApplicationRuntime(applicationEngine: adapter)
        let engineOwner = runtime.applicationEngine
        #elseif os(macOS)
        let helperControlClient = FakeMacOSHelperControlClient()
        let runtime = AppleApplicationRuntime(helperControlClient: helperControlClient)
        let helperOwner = runtime.helperControlClient
        #endif
        let controlOwner = runtime.workflow
        let firstScene = UUID()
        let secondScene = UUID()
        let firstPresentation = MobileSceneNavigationState()
        let secondPresentation = MobileSceneNavigationState(initialPage: .activity)

        firstPresentation.show(.settings)
        runtime.updateScene(
            id: firstScene,
            isActive: true,
            requestsDiscovery: false,
            keepsRememberedConnected: false,
            displayName: "First scene",
            identityPath: ""
        )
        runtime.updateScene(
            id: secondScene,
            isActive: true,
            requestsDiscovery: false,
            keepsRememberedConnected: false,
            displayName: "Second scene",
            identityPath: ""
        )

        XCTAssertEqual(firstPresentation.page, .settings)
        XCTAssertEqual(secondPresentation.page, .activity)
        XCTAssertEqual(runtime.presentationOwnerSceneID, firstScene)
        #if os(iOS)
        XCTAssertTrue(runtime.applicationEngine === engineOwner)
        #elseif os(macOS)
        XCTAssertTrue(runtime.helperControlClient === helperOwner)
        #endif
        XCTAssertTrue(runtime.workflow === controlOwner)

        runtime.removeScene(id: firstScene)

        XCTAssertEqual(runtime.presentationOwnerSceneID, secondScene)
        #if os(iOS)
        XCTAssertTrue(runtime.applicationEngine === engineOwner)
        #elseif os(macOS)
        XCTAssertTrue(runtime.helperControlClient === helperOwner)
        #endif
        XCTAssertTrue(runtime.workflow === controlOwner)
        #if os(iOS)
        runtime.workflow.refreshRememberedRooms()
        XCTAssertEqual(fake.relationshipListCalls, 1)
        _ = try await runtime.applicationEngine.snapshot()
        XCTAssertEqual(fake.snapshotCalls, 1)
        #endif

        runtime.removeScene(id: secondScene)
        #if os(iOS)
        await adapter.close()
        #endif
    }

    func testPersistentOwnerRejectsSecondOpenAndReopensAfterClose() async throws {
        let directory = FileManager.default.temporaryDirectory
            .appendingPathComponent(UUID().uuidString, isDirectory: true)
        defer { try? FileManager.default.removeItem(at: directory) }
        let vault = MemoryApplicationVault()
        let first = try ApplicationEngineAdapter.openPersistent(
            stateDirectory: directory,
            vault: vault
        )

        XCTAssertThrowsError(
            try ApplicationEngineAdapter.openPersistent(
                stateDirectory: directory,
                vault: vault
            )
        ) { error in
            guard case let FfiApplicationError.Failed(code, _) = error else {
                return XCTFail("unexpected error: \(error)")
            }
            XCTAssertEqual(code, .stateAlreadyOwned)
        }

        await first.close()
        let reopened = try ApplicationEngineAdapter.openPersistent(
            stateDirectory: directory,
            vault: vault
        )
        await reopened.close()
    }

    func testPersistentRelationshipsCommitRotateAndSurviveRestart() async throws {
        let directory = FileManager.default.temporaryDirectory
            .appendingPathComponent(UUID().uuidString, isDirectory: true)
        defer { try? FileManager.default.removeItem(at: directory) }
        let vault = MemoryApplicationVault()
        let firstCredential = opaqueCredential(seed: 0x41)
        let rotatedCredential = opaqueCredential(seed: 0x42)
        let first = try ApplicationEngineAdapter.openPersistent(
            stateDirectory: directory,
            vault: vault
        )
        let store = first.relationshipStore
        let pending = try store.prepare(
            label: " iPad ",
            broker: "broker",
            relay: "https://relay"
        )

        do {
            let persistence = try RememberPersistenceContext(pending: pending, store: store)
            XCTAssertTrue(persistence.persist(firstCredential, generation: 7))
            let committed = try XCTUnwrap(store.peers().first)
            XCTAssertEqual(committed.label, "iPad")
            XCTAssertEqual(committed.generation, 7)
            XCTAssertNil(committed.previousGeneration)
            XCTAssertThrowsError(try store.acquireSession(committed.relationshipID)) { error in
                XCTAssertEqual(error as? RememberedPeerStoreError, .activeTransfer)
            }
            XCTAssertThrowsError(try store.delete(committed)) { error in
                XCTAssertEqual(error as? RememberedPeerStoreError, .activeTransfer)
            }
            XCTAssertTrue(persistence.persist(rotatedCredential, generation: 8))
        }

        let rotated = try XCTUnwrap(store.peers().first)
        XCTAssertEqual(rotated.generation, 8)
        XCTAssertEqual(rotated.previousGeneration, 7)
        XCTAssertEqual(
            try store.sessionMaterial(relationshipID: rotated.relationshipID).opaqueCredential,
            rotatedCredential
        )
        let state = try Data(
            contentsOf: directory.appendingPathComponent("engine-state-v2.json")
        )
        XCTAssertNil(state.range(of: firstCredential))
        XCTAssertNil(state.range(of: rotatedCredential))

        await first.close()
        let reopened = try ApplicationEngineAdapter.openPersistent(
            stateDirectory: directory,
            vault: vault
        )
        let reopenedStore = reopened.relationshipStore
        XCTAssertEqual(try reopenedStore.peers(), [rotated])
        XCTAssertEqual(
            try reopenedStore.sessionMaterial(
                relationshipID: rotated.relationshipID
            ).opaqueCredential,
            rotatedCredential
        )
        await reopened.close()
    }

    func testCredentialVaultCallbacksCommitThenRotateOnTheSameOwner() async throws {
        let directory = FileManager.default.temporaryDirectory
            .appendingPathComponent(UUID().uuidString, isDirectory: true)
        defer { try? FileManager.default.removeItem(at: directory) }
        let adapter = try ApplicationEngineAdapter.openPersistent(
            stateDirectory: directory,
            vault: MemoryApplicationVault()
        )
        let store = adapter.relationshipStore
        let pending = try store.prepare(label: "iPad", broker: "broker", relay: "")
        let persistence = try RememberPersistenceContext(pending: pending, store: store)
        let callback = RememberedCredentialVault(persistence)
        let firstCredential = opaqueCredential(seed: 0x44)
        let rotatedCredential = opaqueCredential(seed: 0x45)

        XCTAssertTrue(
            callback.storeRememberedCredential(
                opaqueCredential: firstCredential,
                generation: 4
            )
        )
        XCTAssertTrue(
            callback.storeRememberedCredential(
                opaqueCredential: rotatedCredential,
                generation: 5
            )
        )

        let relationship = try XCTUnwrap(store.peers().first)
        XCTAssertEqual(relationship.generation, 5)
        XCTAssertEqual(relationship.previousGeneration, 4)
        XCTAssertEqual(
            try store.sessionMaterial(
                relationshipID: relationship.relationshipID
            ).opaqueCredential,
            rotatedCredential
        )
        await adapter.close()
    }

    func testRoomControlCredentialVaultAcceptsOnlyInitialGeneration() async throws {
        let directory = FileManager.default.temporaryDirectory
            .appendingPathComponent(UUID().uuidString, isDirectory: true)
        defer { try? FileManager.default.removeItem(at: directory) }
        let adapter = try ApplicationEngineAdapter.openPersistent(
            stateDirectory: directory,
            vault: MemoryApplicationVault()
        )
        let store = adapter.relationshipStore
        let callback = try RoomControlCredentialVault(
            label: "iPad",
            endpoint: RoomControlEndpoint(broker: "broker", relay: ""),
            store: store
        )

        XCTAssertFalse(
            callback.storeRememberedCredential(
                opaqueCredential: opaqueCredential(seed: 0x46),
                generation: 1
            )
        )
        XCTAssertTrue(try store.peers().isEmpty)
        XCTAssertTrue(
            callback.storeRememberedCredential(
                opaqueCredential: opaqueCredential(seed: 0x47),
                generation: 0
            )
        )
        XCTAssertEqual(try store.peers().first?.generation, 0)
        await adapter.close()
    }

    func testPersistentRelationshipRevocationRemovesOnlyEngineVaultMaterial() async throws {
        let directory = FileManager.default.temporaryDirectory
            .appendingPathComponent(UUID().uuidString, isDirectory: true)
        defer { try? FileManager.default.removeItem(at: directory) }
        let vault = MemoryApplicationVault()
        let adapter = try ApplicationEngineAdapter.openPersistent(
            stateDirectory: directory,
            vault: vault
        )
        let store = adapter.relationshipStore
        let relationship = try commitRelationship(in: store)

        XCTAssertFalse(vault.isEmpty)
        try store.delete(relationship)

        XCTAssertTrue(try store.peers().isEmpty)
        XCTAssertTrue(vault.isEmpty)
        XCTAssertThrowsError(
            try store.sessionMaterial(relationshipID: relationship.relationshipID)
        ) { error in
            XCTAssertEqual(error as? RememberedPeerStoreError, .missingCredential)
        }
        await adapter.close()
    }

    func testPersistentRelationshipVaultFailuresStayTypedAndDoNotRetry() async throws {
        let directory = FileManager.default.temporaryDirectory
            .appendingPathComponent(UUID().uuidString, isDirectory: true)
        defer { try? FileManager.default.removeItem(at: directory) }
        let vault = MemoryApplicationVault()
        let adapter = try ApplicationEngineAdapter.openPersistent(
            stateDirectory: directory,
            vault: vault
        )
        let store = adapter.relationshipStore
        let relationship = try commitRelationship(in: store)
        let failures: [(FfiApplicationVaultError, FfiApplicationErrorCode)] = [
            (.CorruptData, .vaultCorrupt),
            (.InteractionRequired, .vaultInteractionRequired),
            (.PermissionDenied, .vaultPermissionDenied),
            (.Canceled, .vaultCanceled),
            (.Unavailable, .vaultUnavailable),
        ]

        for (vaultError, expectedCode) in failures {
            vault.setLoadFailure(vaultError)
            let callsBefore = vault.loadCallCount
            assertApplicationErrorCode(expectedCode) {
                _ = try store.sessionMaterial(relationshipID: relationship.relationshipID)
            }
            XCTAssertEqual(vault.loadCallCount, callsBefore + 1)
            XCTAssertEqual(try store.peers(), [relationship])
        }

        vault.setLoadFailure(nil)
        vault.removeAll()
        let callsBeforeMissing = vault.loadCallCount
        assertApplicationErrorCode(.vaultCorrupt) {
            _ = try store.sessionMaterial(relationshipID: relationship.relationshipID)
        }
        XCTAssertEqual(vault.loadCallCount, callsBeforeMissing + 1)
        XCTAssertEqual(try store.peers(), [relationship])
        await adapter.close()
    }

    func testRelationshipPersistenceDiscardsPendingAndFailsClosedAfterClose() async throws {
        let fake = FakeApplicationEngine()
        let adapter = try ApplicationEngineAdapter(
            engine: fake,
            core: compatibleCoreInfo(),
            binding: compatibleBindingInfo()
        )
        let store = adapter.relationshipStore
        let discarded = try store.prepare(label: "Discarded", broker: "broker", relay: "")
        do {
            _ = try RememberPersistenceContext(pending: discarded, store: store)
        }
        XCTAssertEqual(fake.discardedRelationshipIDs, [discarded.relationshipID])

        let closed = try store.prepare(label: "Closed", broker: "broker", relay: "")
        let context = try RememberPersistenceContext(pending: closed, store: store)
        await adapter.close()
        XCTAssertFalse(context.persist(Data([1]), generation: 0))
    }

    func testPersistentRelationshipsLeaveLegacySentinelsUntouched() async throws {
        let root = FileManager.default.temporaryDirectory
            .appendingPathComponent(UUID().uuidString, isDirectory: true)
        defer { try? FileManager.default.removeItem(at: root) }
        let legacyDirectory = root.appendingPathComponent(
            "envoix/relationships",
            isDirectory: true
        )
        try FileManager.default.createDirectory(
            at: legacyDirectory,
            withIntermediateDirectories: true
        )
        let metadataURL = legacyDirectory.appendingPathComponent("remembered-peers-v1.json")
        let credentialURL = legacyDirectory.appendingPathComponent("legacy-credential")
        let metadataSentinel = Data("not-valid-legacy-json".utf8)
        let credentialSentinel = Data("legacy-secret-sentinel".utf8)
        try metadataSentinel.write(to: metadataURL)
        try credentialSentinel.write(to: credentialURL)
        let metadataDate = try modificationDate(of: metadataURL)
        let credentialDate = try modificationDate(of: credentialURL)

        let adapter = try ApplicationEngineAdapter.openPersistent(
            stateDirectory: root.appendingPathComponent("application-engine-v2"),
            vault: MemoryApplicationVault()
        )
        let store = adapter.relationshipStore
        let relationship = try commitRelationship(in: store)
        _ = try store.sessionMaterial(relationshipID: relationship.relationshipID)
        try store.acquireSession(relationship.relationshipID)
        try store.rotate(
            relationshipID: relationship.relationshipID,
            opaqueCredential: opaqueCredential(seed: 0x52),
            generation: relationship.generation + 1
        )
        store.releaseSession(relationship.relationshipID)
        try store.delete(try XCTUnwrap(store.peers().first))
        await adapter.close()

        XCTAssertEqual(try Data(contentsOf: metadataURL), metadataSentinel)
        XCTAssertEqual(try Data(contentsOf: credentialURL), credentialSentinel)
        XCTAssertEqual(try modificationDate(of: metadataURL), metadataDate)
        XCTAssertEqual(try modificationDate(of: credentialURL), credentialDate)
    }

    #if os(iOS)
    func testIOSPersistentStateDirectoryIsStableAndAbsolute() throws {
        let first = try AppleApplicationEngineLocation.persistentStateDirectory()
        let second = try AppleApplicationEngineLocation.persistentStateDirectory()

        XCTAssertEqual(first, second)
        XCTAssertTrue(first.isFileURL)
        XCTAssertTrue(first.path.hasPrefix("/"))
        XCTAssertTrue(first.path.hasSuffix("/envoix/application-engine-v2"))
    }
    #endif

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
        XCTAssertThrowsError(try adapter.relationshipStore.peers()) { error in
            XCTAssertEqual(error as? ApplicationEngineAdapterError, .closed)
        }
    }

    private func compatibleCoreInfo() -> FfiCoreInfo {
        FfiCoreInfo(
            ffiApiVersion: expectedCoreFFIAPIVersion,
            coreVersion: "test",
            capabilities: [
                expectedTypedApplicationCapability,
                expectedPersistentApplicationEngineCapability,
            ]
        )
    }

    private func compatibleBindingInfo() -> FfiApplicationBindingInfo {
        FfiApplicationBindingInfo(
            bindingVersion: expectedApplicationBindingVersion,
            contractVersion: expectedApplicationContractVersion
        )
    }

    private func commitRelationship(
        in store: RememberedPeerStoring
    ) throws -> RememberedPeerSummary {
        let pending = try store.prepare(label: "iPad", broker: "broker", relay: "")
        let persistence = try RememberPersistenceContext(pending: pending, store: store)
        XCTAssertTrue(persistence.persist(
            opaqueCredential(seed: 0x43),
            generation: 0
        ))
        return try XCTUnwrap(store.peers().first)
    }

    private func opaqueCredential(seed: UInt8) -> Data {
        var credential = Data([0x45, 0x4E, 0x56, 0x52, 0x01])
        credential.append(Data(repeating: seed, count: 32))
        return credential
    }

    private func assertApplicationErrorCode(
        _ expectedCode: FfiApplicationErrorCode,
        operation: () throws -> Void,
        file: StaticString = #filePath,
        line: UInt = #line
    ) {
        XCTAssertThrowsError(try operation(), file: file, line: line) { error in
            guard case let FfiApplicationError.Failed(code, _) = error else {
                return XCTFail("unexpected error: \(error)", file: file, line: line)
            }
            XCTAssertEqual(code, expectedCode, file: file, line: line)
        }
    }

    private func modificationDate(of url: URL) throws -> Date {
        try XCTUnwrap(
            FileManager.default.attributesOfItem(atPath: url.path)[.modificationDate] as? Date
        )
    }
}

private final class FakeApplicationEngine: FfiApplicationEngineProtocol, @unchecked Sendable {
    private(set) var snapshotCalls = 0
    private(set) var relationshipListCalls = 0
    private(set) var discardedRelationshipIDs: [String] = []
    private var nextRelationshipID = 0

    func commitRelationship(
        relationshipId _: String,
        opaqueCredential _: Data,
        generation _: UInt64
    ) throws -> FfiRememberedRelationship {
        throw FakeApplicationEngineError.unsupported
    }

    func discardPreparedRelationship(relationshipId: String) throws {
        discardedRelationshipIDs.append(relationshipId)
    }

    func loadRelationship(
        relationshipId _: String
    ) throws -> FfiRememberedRelationshipMaterial? {
        throw FakeApplicationEngineError.unsupported
    }

    func prepareRelationship(
        label: String,
        broker _: String,
        relay _: String
    ) throws -> FfiPreparedRelationship {
        nextRelationshipID += 1
        return FfiPreparedRelationship(
            relationshipId: "prepared-\(nextRelationshipID)",
            label: label.trimmingCharacters(in: .whitespacesAndNewlines)
        )
    }

    func relationships() throws -> [FfiRememberedRelationship] {
        relationshipListCalls += 1
        return []
    }

    func renameRelationship(
        relationshipId _: String,
        label _: String
    ) throws -> FfiRememberedRelationship {
        throw FakeApplicationEngineError.unsupported
    }

    func revokeRelationship(
        relationshipId _: String
    ) throws -> FfiRememberedRelationship {
        throw FakeApplicationEngineError.unsupported
    }

    func rotateRelationship(
        relationshipId _: String,
        opaqueCredential _: Data,
        generation _: UInt64
    ) throws -> FfiRememberedRelationship {
        throw FakeApplicationEngineError.unsupported
    }

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

private enum FakeApplicationEngineError: Error {
    case unsupported
}

private final class MemoryApplicationVault: FfiApplicationVault, @unchecked Sendable {
    private let lock = NSLock()
    private var values: [String: Data] = [:]
    private var forcedLoadFailure: FfiApplicationVaultError?
    private var loadCalls = 0

    var isEmpty: Bool {
        lock.withEnvoixLock { values.isEmpty }
    }

    var loadCallCount: Int {
        lock.withEnvoixLock { loadCalls }
    }

    func setLoadFailure(_ error: FfiApplicationVaultError?) {
        lock.withEnvoixLock {
            forcedLoadFailure = error
        }
    }

    func removeAll() {
        lock.withEnvoixLock {
            values.removeAll()
        }
    }

    func contains(reference: String) throws -> Bool {
        lock.withEnvoixLock { values[reference] != nil }
    }

    func store(reference: String, opaqueCredential: Data) throws {
        lock.withEnvoixLock { values[reference] = opaqueCredential }
    }

    func load(reference: String) throws -> Data? {
        try lock.withEnvoixLock {
            loadCalls += 1
            if let forcedLoadFailure {
                throw forcedLoadFailure
            }
            return values[reference]
        }
    }

    func delete(reference: String) throws {
        _ = lock.withEnvoixLock { values.removeValue(forKey: reference) }
    }
}

#if os(macOS)
private final class FakeMacOSHelperControlClient:
    MacOSHelperControlClient, @unchecked Sendable {}
#endif
