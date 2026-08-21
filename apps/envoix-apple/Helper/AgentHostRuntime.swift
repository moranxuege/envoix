import EnvoixCore
import Foundation

final class EnvoixEngineHelperHost: @unchecked Sendable {
    private let host: FfiAgentHostProtocol
    private let configuration: FfiAgentHostConfiguration

    init(
        host: FfiAgentHostProtocol,
        configuration: FfiAgentHostConfiguration
    ) {
        self.host = host
        self.configuration = configuration
    }

    static func start(
        configuration: FfiAgentHostConfiguration,
        vault: FfiApplicationVault,
        core: FfiCoreInfo = envoixCoreInfo()
    ) throws -> EnvoixEngineHelperHost {
        guard coreMatchesExpectedRoomControlContract(core) else {
            throw MacOSAgentBoundaryError.incompatibleCore
        }
        return try EnvoixEngineHelperHost(
            host: FfiAgentHost.start(
                configuration: configuration,
                vault: vault
            ),
            configuration: configuration
        )
    }

    func waitUntilReady() async throws -> FfiAgentHostReady {
        let readiness = try await host.waitUntilReady()
        try MacOSAgentBoundary.validateReadiness(
            readiness,
            configuration: configuration
        )
        return readiness
    }

    func lifecycle() -> FfiAgentHostLifecycleState {
        host.lifecycle()
    }

    func shutdown() async throws -> FfiAgentHostLifecycleState {
        try await host.shutdown()
    }
}
