import Foundation
import EnvoixCore

let expectedApplicationBindingVersion: UInt32 = 1
let expectedApplicationContractVersion: UInt16 = 6
private let typedApplicationCapability = "typed_application_contract_v6"

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
          core.capabilities.contains(typedApplicationCapability)
    else {
        throw ApplicationEngineAdapterError.incompatibleBinding(
            ffi: core.ffiApiVersion,
            binding: binding.bindingVersion,
            contract: binding.contractVersion
        )
    }
}

/// Serializes access to one Rust application Engine handle.
///
/// These calls reduce state or decide a command and are intentionally short.
/// Long-running network and file operations retain their explicit native
/// cancellation objects outside this actor.
actor ApplicationEngineAdapter {
    private var engine: FfiApplicationEngineProtocol?

    init(
        engine: FfiApplicationEngineProtocol = FfiApplicationEngine(),
        core: FfiCoreInfo = envoixCoreInfo(),
        binding: FfiApplicationBindingInfo = envoixApplicationBindingInfo()
    ) throws {
        try validateApplicationBinding(core: core, binding: binding)
        self.engine = engine
    }

    func snapshot() throws -> FfiApplicationSnapshot {
        try Task.checkCancellation()
        return try requireEngine().snapshot()
    }

    func apply(_ envelope: FfiApplicationEventEnvelope) throws -> FfiApplyOutcome {
        try Task.checkCancellation()
        return try requireEngine().apply(envelope: envelope)
    }

    func decide(
        _ envelope: FfiApplicationCommandEnvelope
    ) throws -> FfiApplicationEffectEnvelope {
        try Task.checkCancellation()
        return try requireEngine().decide(envelope: envelope)
    }

    func close() {
        engine = nil
    }

    private func requireEngine() throws -> FfiApplicationEngineProtocol {
        guard let engine else {
            throw ApplicationEngineAdapterError.closed
        }
        return engine
    }
}
