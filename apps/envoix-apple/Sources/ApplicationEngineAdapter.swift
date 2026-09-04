import Foundation
import EnvoixCore

enum ApplicationEngineAdapterError: Error, Equatable {
    case incompatibleBinding(ffi: UInt32, binding: UInt32, contract: UInt16)
    case closed
}

func validateApplicationBinding(
    core: FfiCoreInfo,
    binding: FfiApplicationBindingInfo
) throws {
    guard core.ffiApiVersion == expectedCoreFFIAPIVersion,
          binding.bindingVersion == expectedApplicationBindingVersion,
          binding.contractVersion == expectedApplicationContractVersion,
          core.capabilities.contains(expectedTypedApplicationCapability),
          core.capabilities.contains(expectedPersistentApplicationEngineCapability),
          core.capabilities.contains(expectedAgentHostControlCapability)
    else {
        throw ApplicationEngineAdapterError.incompatibleBinding(
            ffi: core.ffiApiVersion,
            binding: binding.bindingVersion,
            contract: binding.contractVersion
        )
    }
}

private final class ApplicationEngineHandle: @unchecked Sendable {
    private let lock = NSLock()
    private var engine: FfiApplicationEngineProtocol?

    init(engine: FfiApplicationEngineProtocol) {
        self.engine = engine
    }

    func withEngine<T>(
        _ operation: (FfiApplicationEngineProtocol) throws -> T
    ) throws -> T {
        try lock.withEnvoixLock {
            guard let engine else {
                throw ApplicationEngineAdapterError.closed
            }
            return try operation(engine)
        }
    }

    func close() {
        lock.withEnvoixLock {
            engine = nil
        }
    }
}

final class ApplicationEngineRememberedPeerStore: RememberedPeerStoring, @unchecked Sendable {
    private let handle: ApplicationEngineHandle
    private let sessionLock = NSLock()
    private var activeRelationships = Set<String>()

    fileprivate init(handle: ApplicationEngineHandle) {
        self.handle = handle
    }

    func prepare(label: String, broker: String, relay: String) throws -> PendingRememberedPeer {
        let prepared = try handle.withEngine {
            try $0.prepareRelationship(label: label, broker: broker, relay: relay)
        }
        return PendingRememberedPeer(
            relationshipID: prepared.relationshipId,
            label: prepared.label,
            credentialReference: prepared.relationshipId,
            broker: broker,
            relay: relay
        )
    }

    func discardPrepared(_ pending: PendingRememberedPeer) throws {
        try handle.withEngine {
            try $0.discardPreparedRelationship(relationshipId: pending.relationshipID)
        }
    }

    func peers() throws -> [RememberedPeerSummary] {
        try handle.withEngine {
            try $0.relationships()
                .map(Self.project)
                .sorted {
                    $0.label.localizedCaseInsensitiveCompare($1.label) == .orderedAscending
                }
        }
    }

    func credential(for peer: RememberedPeerSummary) throws -> Data {
        try sessionMaterial(relationshipID: peer.relationshipID).opaqueCredential
    }

    func sessionMaterial(relationshipID: String) throws -> RememberedPeerSessionMaterial {
        try handle.withEngine {
            guard let material = try $0.loadRelationship(relationshipId: relationshipID) else {
                throw RememberedPeerStoreError.missingCredential
            }
            return RememberedPeerSessionMaterial(
                summary: Self.project(material.relationship),
                opaqueCredential: material.opaqueCredential
            )
        }
    }

    func acquireSession(_ relationshipID: String) throws {
        try sessionLock.withEnvoixLock {
            guard activeRelationships.insert(relationshipID).inserted else {
                throw RememberedPeerStoreError.activeTransfer
            }
        }
    }

    func releaseSession(_ relationshipID: String) {
        _ = sessionLock.withEnvoixLock {
            activeRelationships.remove(relationshipID)
        }
    }

    func create(
        _ pending: PendingRememberedPeer,
        opaqueCredential: Data,
        generation: UInt64
    ) throws {
        try sessionLock.withEnvoixLock {
            guard activeRelationships.contains(pending.relationshipID) else {
                throw RememberedPeerStoreError.inactiveSession
            }
            _ = try handle.withEngine {
                try $0.commitRelationship(
                    relationshipId: pending.relationshipID,
                    opaqueCredential: opaqueCredential,
                    generation: generation
                )
            }
        }
    }

    func rotate(
        relationshipID: String,
        opaqueCredential: Data,
        generation: UInt64
    ) throws {
        try sessionLock.withEnvoixLock {
            guard activeRelationships.contains(relationshipID) else {
                throw RememberedPeerStoreError.inactiveSession
            }
            _ = try handle.withEngine {
                try $0.rotateRelationship(
                    relationshipId: relationshipID,
                    opaqueCredential: opaqueCredential,
                    generation: generation
                )
            }
        }
    }

    func delete(_ peer: RememberedPeerSummary) throws {
        try sessionLock.withEnvoixLock {
            guard !activeRelationships.contains(peer.relationshipID) else {
                throw RememberedPeerStoreError.activeTransfer
            }
            _ = try handle.withEngine {
                try $0.revokeRelationship(relationshipId: peer.relationshipID)
            }
        }
    }

    private static func project(
        _ relationship: FfiRememberedRelationship
    ) -> RememberedPeerSummary {
        RememberedPeerSummary(
            relationshipID: relationship.relationshipId,
            label: relationship.label,
            generation: relationship.generation,
            previousGeneration: relationship.previousGeneration,
            broker: relationship.broker,
            relay: relationship.relay
        )
    }
}

/// Serializes access to one Rust application Engine handle.
///
/// These calls reduce state or decide a command and are intentionally short.
/// Long-running network and file operations retain their explicit native
/// cancellation objects outside this actor.
actor ApplicationEngineAdapter {
    private let handle: ApplicationEngineHandle
    nonisolated let relationshipStore: RememberedPeerStoring

    init(
        engine: FfiApplicationEngineProtocol,
        core: FfiCoreInfo = envoixCoreInfo(),
        binding: FfiApplicationBindingInfo = envoixApplicationBindingInfo()
    ) throws {
        try validateApplicationBinding(core: core, binding: binding)
        let handle = ApplicationEngineHandle(engine: engine)
        self.handle = handle
        relationshipStore = ApplicationEngineRememberedPeerStore(handle: handle)
    }

    static func openPersistent(
        stateDirectory: URL,
        vault: FfiApplicationVault,
        core: FfiCoreInfo = envoixCoreInfo(),
        binding: FfiApplicationBindingInfo = envoixApplicationBindingInfo()
    ) throws -> ApplicationEngineAdapter {
        try validateApplicationBinding(core: core, binding: binding)
        guard stateDirectory.isFileURL,
              stateDirectory.path.hasPrefix("/") else {
            throw FfiApplicationError.Failed(
                code: .invalidInput,
                reason: "persistent Engine state directory must be an absolute file URL"
            )
        }
        return try ApplicationEngineAdapter(
            engine: FfiApplicationEngine.openPersistent(
                stateDirectory: stateDirectory.standardizedFileURL.path,
                vault: vault
            ),
            core: core,
            binding: binding
        )
    }

    func snapshot() throws -> FfiApplicationSnapshot {
        try Task.checkCancellation()
        return try handle.withEngine { try $0.snapshot() }
    }

    func apply(_ envelope: FfiApplicationEventEnvelope) throws -> FfiApplyOutcome {
        try Task.checkCancellation()
        return try handle.withEngine { try $0.apply(envelope: envelope) }
    }

    func decide(
        _ envelope: FfiApplicationCommandEnvelope
    ) throws -> FfiApplicationEffectEnvelope {
        try Task.checkCancellation()
        return try handle.withEngine { try $0.decide(envelope: envelope) }
    }

    func close() {
        handle.close()
    }
}
