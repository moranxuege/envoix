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

enum MacOSAgentPairingError: LocalizedError, Equatable {
    case unexpectedResponse
    case rejected(code: String, reason: String)

    var errorDescription: String? {
        switch self {
        case .unexpectedResponse:
            return "The helper returned an incompatible pairing response."
        case let .rejected(code, reason):
            return "Agent \(code): \(reason)"
        }
    }
}

typealias MacOSAgentControlClientFactory =
    @Sendable (String) throws -> FfiAgentControlClientProtocol

final class MacOSAgentControlClient: MacOSHelperControlClient, @unchecked Sendable {
    private let controlEndpoint: String
    private let clientFactory: MacOSAgentControlClientFactory

    init(
        controlEndpoint: URL,
        clientFactory: @escaping MacOSAgentControlClientFactory = {
            try FfiAgentControlClient(controlEndpoint: $0)
        }
    ) throws {
        guard controlEndpoint.isFileURL,
              controlEndpoint.path.hasPrefix("/") else {
            throw MacOSAgentControlClientError.invalidEndpoint
        }
        self.controlEndpoint = controlEndpoint.path
        self.clientFactory = clientFactory
    }

    init(client: FfiAgentControlClientProtocol) {
        controlEndpoint = ""
        clientFactory = { _ in client }
    }

    func call(request: FfiAgentRequest) async throws -> FfiAgentResponse {
        // Opening the Unix socket is deliberately deferred until each call.
        // The GUI commonly starts before an explicitly enabled helper has
        // created its socket; caching that initial failure would otherwise
        // leave the process disconnected until the whole app restarts.
        let client = try clientFactory(controlEndpoint)
        return try await client.call(request: request)
    }
}

final class UnavailableMacOSAgentControlClient:
    MacOSHelperControlClient, @unchecked Sendable
{
    func call(request: FfiAgentRequest) async throws -> FfiAgentResponse {
        throw MacOSAgentControlClientError.unavailable
    }
}

@MainActor
final class MacOSAgentPairingCoordinator: DurablePairingCoordinating {
    private let controlClient: MacOSHelperControlClient

    init(controlClient: MacOSHelperControlClient) {
        self.controlClient = controlClient
    }

    func joinPairing(
        label: String,
        invitation: String,
        verificationCode: String
    ) async throws -> DurablePairedDevice {
        let response = try await controlClient.call(request: .joinPairing(
            pairing: FfiAgentPairingInput(
                label: label,
                invitation: invitation,
                verificationCode: verificationCode
            )
        ))
        switch response {
        case let .devicePaired(device):
            return DurablePairedDevice(id: device.id, label: device.label)
        case let .error(code, message):
            throw MacOSAgentPairingError.rejected(code: code, reason: message)
        default:
            throw MacOSAgentPairingError.unexpectedResponse
        }
    }
}

struct MacOSAgentDevice: Equatable, Identifiable {
    let id: String
    let label: String
}

enum MacOSAgentTransferError: LocalizedError, Equatable {
    case alreadyPreparing
    case unexpectedResponse
    case rejected(code: String, reason: String)

    var errorDescription: String? {
        switch self {
        case .alreadyPreparing:
            return "A transfer for this device is already being prepared."
        case .unexpectedResponse:
            return "The helper returned an incompatible transfer response."
        case let .rejected(code, reason):
            return "Agent \(code): \(reason)"
        }
    }
}

@MainActor
final class MacOSAgentTransferController: ObservableObject {
    @Published private(set) var devices: [MacOSAgentDevice] = []
    @Published private(set) var transfers: [FfiApplicationTransfer] = []
    @Published private(set) var activePaths: [FfiAgentTransferPath] = []
    @Published private(set) var preparingDeviceIDs = Set<String>()
    @Published private(set) var hasLoadedSnapshot = false
    @Published private(set) var loadError: String?

    private static let snapshotInboxLimit: UInt64 = 20

    private let controlClient: MacOSHelperControlClient
    private var isRefreshingSnapshot = false
    private var nextTransferObservationOrder: UInt64 = 0
    private var transferObservationOrder: [String: UInt64] = [:]

    init(controlClient: MacOSHelperControlClient) {
        self.controlClient = controlClient
    }

    func isPreparing(deviceID: String) -> Bool {
        preparingDeviceIDs.contains(deviceID)
    }

    func transfers(deviceID: String) -> [FfiApplicationTransfer] {
        transfers.filter { $0.relationshipId == deviceID }
    }

    func activePath(transferID: String) -> FfiAgentPathKind? {
        activePaths.first { $0.transferId == transferID }?.path
    }

    var hasPendingTransfers: Bool {
        transfers.contains { !Self.isTerminal($0.state) }
    }

    func refresh() async {
        await refreshDevices()
        await refreshSnapshot()
    }

    func refreshDevices() async {
        do {
            let response = try await controlClient.call(request: .listDevices)
            guard case let .devices(agentDevices) = response else {
                throw MacOSAgentTransferError.unexpectedResponse
            }
            devices = agentDevices
                .map { MacOSAgentDevice(id: $0.id, label: $0.label) }
                .sorted {
                    $0.label.localizedCaseInsensitiveCompare($1.label) == .orderedAscending
                }
            loadError = nil
        } catch {
            // Preserve the last good list while the helper is temporarily unavailable.
            loadError = error.localizedDescription
        }
    }

    func refreshSnapshot() async {
        guard !isRefreshingSnapshot else { return }
        isRefreshingSnapshot = true
        defer { isRefreshingSnapshot = false }

        do {
            let response = try await controlClient.call(request: .snapshot(
                inboxLimit: Self.snapshotInboxLimit
            ))
            guard case let .snapshot(snapshot) = response else {
                throw MacOSAgentTransferError.unexpectedResponse
            }
            replaceTransfers(snapshot.engine.transfers)
            activePaths = snapshot.activePaths
            hasLoadedSnapshot = true
            loadError = nil
        } catch {
            // Keep the last coherent snapshot visible during a transient helper restart.
            loadError = error.localizedDescription
        }
    }

    func createTransfer(deviceID: String, urls: [URL]) async throws -> String {
        guard preparingDeviceIDs.insert(deviceID).inserted else {
            throw MacOSAgentTransferError.alreadyPreparing
        }
        defer { preparingDeviceIDs.remove(deviceID) }

        let accesses = urls.map(SecurityScopedResourceAccess.init)
        defer { withExtendedLifetime(accesses) {} }
        for access in accesses {
            guard access.isActive
                    || FileManager.default.isReadableFile(atPath: access.url.path) else {
                throw OpenedSendFileError.inaccessible
            }
        }
        let paths = try validatedOpenedSendURLs(urls).map(\.path)
        let response = try await controlClient.call(request: .createTransfer(
            device: deviceID,
            paths: paths
        ))
        switch response {
        case let .transferCreated(transfer):
            replaceTransfers(upserting: transfer)
            return transfer.id
        case let .error(code, message):
            throw MacOSAgentTransferError.rejected(code: code, reason: message)
        default:
            throw MacOSAgentTransferError.unexpectedResponse
        }
    }

    func pauseTransfer(id: String) async throws {
        try await updateTransfer(request: .pauseTransfer(transferId: id))
    }

    func resumeTransfer(id: String) async throws {
        try await updateTransfer(request: .resumeTransfer(transferId: id))
    }

    func retryTransfer(id: String) async throws {
        try await updateTransfer(request: .recoverTransfer(transferId: id))
    }

    func cancelTransfer(id: String) async throws {
        try await updateTransfer(request: .cancelTransfer(transferId: id))
    }

    func removeTransfer(id: String) async throws {
        let response = try await controlClient.call(
            request: .removeTransfer(transferId: id)
        )
        switch response {
        case let .transferRemoved(removedID) where removedID == id:
            replaceTransfers(transfers.filter { $0.id != id })
        case let .error(code, message):
            throw MacOSAgentTransferError.rejected(code: code, reason: message)
        default:
            throw MacOSAgentTransferError.unexpectedResponse
        }
    }

    private func updateTransfer(request: FfiAgentRequest) async throws {
        let response = try await controlClient.call(request: request)
        switch response {
        case let .transfer(transfer):
            replaceTransfers(upserting: transfer)
        case let .error(code, message):
            throw MacOSAgentTransferError.rejected(code: code, reason: message)
        default:
            throw MacOSAgentTransferError.unexpectedResponse
        }
    }

    private func replaceTransfers(upserting transfer: FfiApplicationTransfer) {
        var updated = transfers.filter { $0.id != transfer.id }
        updated.append(transfer)
        replaceTransfers(updated)
    }

    private func replaceTransfers(_ updated: [FfiApplicationTransfer]) {
        let liveIDs = Set(updated.map(\.id))
        transferObservationOrder = transferObservationOrder.filter {
            liveIDs.contains($0.key)
        }
        for transfer in updated where transferObservationOrder[transfer.id] == nil {
            nextTransferObservationOrder &+= 1
            transferObservationOrder[transfer.id] = nextTransferObservationOrder
        }
        transfers = updated.sorted { left, right in
            let leftTerminal = Self.isTerminal(left.state)
            let rightTerminal = Self.isTerminal(right.state)
            if leftTerminal != rightTerminal { return !leftTerminal }
            let leftOrder = transferObservationOrder[left.id] ?? 0
            let rightOrder = transferObservationOrder[right.id] ?? 0
            if leftOrder != rightOrder { return leftOrder > rightOrder }
            return left.id < right.id
        }
    }

    private static func isTerminal(_ state: FfiApplicationTransferState) -> Bool {
        switch state {
        case .delivered, .rejected, .failed, .canceled:
            return true
        case .offered, .queued, .connecting, .transferring, .paused,
             .awaitingDeliveryProof:
            return false
        }
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

private enum MacOSAgentStartupPolicy {
    static let retryDelaysNanoseconds: [UInt64] = [
        150_000_000,
        350_000_000,
        750_000_000,
        1_500_000_000,
        3_000_000_000,
    ]
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
    private let startupRetryDelaysNanoseconds: [UInt64]
    private var generation = 0

    init(
        service: MacOSAgentServiceRegistering? = nil,
        controlClient: MacOSHelperControlClient,
        startupRetryDelaysNanoseconds: [UInt64] =
            MacOSAgentStartupPolicy.retryDelaysNanoseconds
    ) {
        self.service = service ?? SystemMacOSAgentService()
        self.controlClient = controlClient
        self.startupRetryDelaysNanoseconds = startupRetryDelaysNanoseconds
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
        if enabled {
            await retryHelperStartupIfNeeded()
        }
    }

    private func retryHelperStartupIfNeeded() async {
        for delay in startupRetryDelaysNanoseconds {
            guard registrationState == .enabled,
                  case .unavailable = connectionState else { return }
            do {
                try await Task.sleep(nanoseconds: delay)
            } catch {
                return
            }
            guard registrationState == .enabled else { return }
            await refresh()
        }
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
