#if os(iOS) || os(macOS)
import Combine
import Foundation

struct AppleSceneRuntimeRequest: Equatable {
    let order: Int
    var isActive: Bool
    var requestsDiscovery: Bool
    var keepsRememberedConnected: Bool
    var displayName: String
    var identityPath: String
}

enum AppleApplicationRuntimePolicy {
    static func presentationOwner(
        current: UUID?,
        requests: [UUID: AppleSceneRuntimeRequest]
    ) -> UUID? {
        if let current, requests[current]?.isActive == true {
            return current
        }
        return requests
            .filter { $0.value.isActive }
            .min { $0.value.order < $1.value.order }?
            .key
    }

    static func requestsDiscovery(
        _ requests: [UUID: AppleSceneRuntimeRequest]
    ) -> Bool {
        requests.values.contains(where: \.requestsDiscovery)
    }

    static func keepsRememberedConnected(
        _ requests: [UUID: AppleSceneRuntimeRequest]
    ) -> Bool {
        requests.values.contains(where: \.keepsRememberedConnected)
    }
}

/// Owns process-wide Apple platform effects while each window keeps its own
/// presentation and navigation state.
@MainActor
final class AppleApplicationRuntime: ObservableObject {
    static let shared = AppleApplicationRuntime()

    /// The process-wide application Engine owner. The injected adapter keeps
    /// scene composition independent from the concrete durable Engine binding.
    let applicationEngine: ApplicationEngineAdapter
    let nearbyCoordinator: NearbyDiscoveryCoordinator
    let presence: NearbyPresencePreferences
    let workflow: ConnectionWorkflowState
    let rememberedOutbox: RememberedRoomOutboxController

    @Published private(set) var presentationOwnerSceneID: UUID?
    @Published private(set) var systemPairingOwnerSceneID: UUID?

    private let wifiAwareServices: AppleWifiAwareServiceCoordinator
    private var sceneRequests: [UUID: AppleSceneRuntimeRequest] = [:]
    private var nextSceneOrder = 0
    private var systemPairingLease: AppleWifiAwareServiceCoordinator.Lease?

    convenience init() {
        self.init(applicationEngine: Self.makeInMemoryApplicationEngine())
    }

    convenience init(applicationEngine: ApplicationEngineAdapter) {
        self.init(
            applicationEngine: applicationEngine,
            nearbyCoordinator: NearbyDiscoveryCoordinator(),
            presence: NearbyPresencePreferences(),
            workflow: ConnectionWorkflowState(
                gateway: RoomControlGatewayFactory.make()
            ),
            rememberedOutbox: RememberedRoomOutboxController(),
            wifiAwareServices: .shared
        )
    }

    init(
        applicationEngine: ApplicationEngineAdapter,
        nearbyCoordinator: NearbyDiscoveryCoordinator,
        presence: NearbyPresencePreferences,
        workflow: ConnectionWorkflowState,
        rememberedOutbox: RememberedRoomOutboxController,
        wifiAwareServices: AppleWifiAwareServiceCoordinator
    ) {
        self.applicationEngine = applicationEngine
        self.nearbyCoordinator = nearbyCoordinator
        self.presence = presence
        self.workflow = workflow
        self.rememberedOutbox = rememberedOutbox
        self.wifiAwareServices = wifiAwareServices
    }

    var isSystemPairingActive: Bool {
        systemPairingOwnerSceneID != nil
    }

    func isPresentationOwner(_ sceneID: UUID) -> Bool {
        presentationOwnerSceneID == sceneID
    }

    func updateScene(
        id: UUID,
        isActive: Bool,
        requestsDiscovery: Bool,
        keepsRememberedConnected: Bool,
        displayName: String,
        identityPath: String
    ) {
        let order: Int
        if let current = sceneRequests[id] {
            order = current.order
        } else {
            order = nextSceneOrder
            nextSceneOrder += 1
        }
        sceneRequests[id] = AppleSceneRuntimeRequest(
            order: order,
            isActive: isActive,
            requestsDiscovery: requestsDiscovery,
            keepsRememberedConnected: keepsRememberedConnected,
            displayName: displayName,
            identityPath: identityPath
        )
        reconcile()
    }

    func removeScene(id: UUID) {
        sceneRequests.removeValue(forKey: id)
        if systemPairingOwnerSceneID == id {
            let lease = systemPairingLease
            systemPairingLease = nil
            systemPairingOwnerSceneID = nil
            if let lease {
                Task { await wifiAwareServices.release(lease) }
            }
        }
        reconcile()
    }

    func beginSystemPairing(for sceneID: UUID) async -> Bool {
        guard systemPairingOwnerSceneID == nil else {
            return systemPairingOwnerSceneID == sceneID && systemPairingLease != nil
        }
        systemPairingOwnerSceneID = sceneID
        reconcile()
        await nearbyCoordinator.suspendForSystemPairing()
        do {
            let lease = try await wifiAwareServices.acquire(.systemPairing)
            guard systemPairingOwnerSceneID == sceneID else {
                await wifiAwareServices.release(lease)
                return false
            }
            systemPairingLease = lease
            return true
        } catch {
            if systemPairingOwnerSceneID == sceneID {
                systemPairingOwnerSceneID = nil
                reconcile()
            }
            return false
        }
    }

    func finishSystemPairing(for sceneID: UUID) async {
        guard systemPairingOwnerSceneID == sceneID else { return }
        let lease = systemPairingLease
        systemPairingLease = nil
        systemPairingOwnerSceneID = nil
        if let lease {
            await wifiAwareServices.release(lease)
        }
        reconcile()
    }

    private func reconcile() {
        presentationOwnerSceneID = AppleApplicationRuntimePolicy.presentationOwner(
            current: presentationOwnerSceneID,
            requests: sceneRequests
        )

        let configuration = sceneRequests.values.min { $0.order < $1.order }
        let discoveryRequested = AppleApplicationRuntimePolicy.requestsDiscovery(
            sceneRequests
        ) && !isSystemPairingActive
        nearbyCoordinator.configure(
            displayName: configuration?.displayName ?? presence.displayName,
            advertisingEnabled: presence.isAdvertising(
                sceneIsActive: discoveryRequested
            )
        )
        if discoveryRequested {
            nearbyCoordinator.start()
        } else {
            nearbyCoordinator.stop()
        }

        workflow.setRememberedReconnectEnabled(
            AppleApplicationRuntimePolicy.keepsRememberedConnected(sceneRequests),
            displayName: configuration?.displayName ?? presence.displayName,
            identityPath: configuration?.identityPath ?? ""
        )
    }

    private static func makeInMemoryApplicationEngine() -> ApplicationEngineAdapter {
        do {
            return try ApplicationEngineAdapter()
        } catch {
            preconditionFailure("The bundled application Engine is incompatible: \(error)")
        }
    }
}
#endif
