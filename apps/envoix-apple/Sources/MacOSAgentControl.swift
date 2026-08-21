#if os(macOS)
import Combine
import EnvoixCore
import Foundation
import ServiceManagement

protocol MacOSHelperControlClient: AnyObject, Sendable {
    func call(request: FfiAgentRequest) async throws -> FfiAgentResponse
}

enum MacOSAgentControlClientError: Error, Equatable {
    case unavailable
    case invalidEndpoint
}

final class MacOSAgentControlClient: MacOSHelperControlClient, @unchecked Sendable {
    private let client: FfiAgentControlClientProtocol

    init(controlEndpoint: URL) throws {
        guard controlEndpoint.isFileURL,
              controlEndpoint.path.hasPrefix("/") else {
            throw MacOSAgentControlClientError.invalidEndpoint
        }
        client = try FfiAgentControlClient(controlEndpoint: controlEndpoint.path)
    }

    init(client: FfiAgentControlClientProtocol) {
        self.client = client
    }

    func call(request: FfiAgentRequest) async throws -> FfiAgentResponse {
        try await client.call(request: request)
    }
}

final class UnavailableMacOSAgentControlClient:
    MacOSHelperControlClient, @unchecked Sendable
{
    func call(request: FfiAgentRequest) async throws -> FfiAgentResponse {
        throw MacOSAgentControlClientError.unavailable
    }
}

enum MacOSAgentRegistrationState: Equatable {
    case unknown
    case notRegistered
    case enabled
    case requiresApproval
    case helperNotFound
    case failed
}

enum MacOSAgentConnectionState: Equatable {
    case idle
    case checking
    case ready(pairedDevices: UInt64)
    case unavailable(FfiAgentControlErrorCode?)
    case incompatible
}

@MainActor
protocol MacOSAgentServiceRegistering: AnyObject {
    var registrationState: MacOSAgentRegistrationState { get }
    func register() throws
    func unregister() throws
}

@MainActor
final class SystemMacOSAgentService: MacOSAgentServiceRegistering {
    private let service: SMAppService

    init(identifier: String = MacOSAgentBoundary.helperBundleIdentifier) {
        service = .loginItem(identifier: identifier)
    }

    var registrationState: MacOSAgentRegistrationState {
        switch service.status {
        case .notRegistered:
            return .notRegistered
        case .enabled:
            return .enabled
        case .requiresApproval:
            return .requiresApproval
        case .notFound:
            return .helperNotFound
        @unknown default:
            return .failed
        }
    }

    func register() throws {
        try service.register()
    }

    func unregister() throws {
        try service.unregister()
    }
}

@MainActor
final class MacOSAgentServiceController: ObservableObject {
    @Published private(set) var registrationState: MacOSAgentRegistrationState = .unknown
    @Published private(set) var connectionState: MacOSAgentConnectionState = .idle

    private let service: MacOSAgentServiceRegistering
    private let controlClient: MacOSHelperControlClient
    private var generation = 0

    init(
        service: MacOSAgentServiceRegistering? = nil,
        controlClient: MacOSHelperControlClient
    ) {
        self.service = service ?? SystemMacOSAgentService()
        self.controlClient = controlClient
    }

    var isRequestedEnabled: Bool {
        registrationState == .enabled || registrationState == .requiresApproval
    }

    func setEnabled(_ enabled: Bool) async {
        generation += 1
        do {
            if enabled {
                try service.register()
            } else {
                try service.unregister()
            }
        } catch {
            registrationState = .failed
            connectionState = .unavailable(nil)
            return
        }
        await refresh()
    }

    func refresh() async {
        generation += 1
        let refreshGeneration = generation
        let registration = service.registrationState
        registrationState = registration
        guard registration == .enabled else {
            connectionState = .idle
            return
        }

        connectionState = .checking
        do {
            let response = try await controlClient.call(request: .status)
            guard refreshGeneration == generation,
                  registrationState == .enabled else { return }
            guard case let .status(status) = response,
                  status.protocolVersion == expectedAgentProtocolVersion else {
                connectionState = .incompatible
                return
            }
            connectionState = .ready(pairedDevices: status.pairedDevices)
        } catch let FfiAgentControlError.Failed(code, _) {
            guard refreshGeneration == generation else { return }
            connectionState = code == .incompatibleProtocol
                ? .incompatible
                : .unavailable(code)
        } catch {
            guard refreshGeneration == generation else { return }
            connectionState = .unavailable(nil)
        }
    }
}
#endif
