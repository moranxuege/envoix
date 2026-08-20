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

#if os(iOS)
enum AppleApplicationEngineLocation {
    private static let rootDirectoryName = "envoix"
    private static let engineDirectoryName = "application-engine-v2"

    static func persistentStateDirectory(
        fileManager: FileManager = .default
    ) throws -> URL {
        guard let supportDirectory = fileManager.urls(
            for: .applicationSupportDirectory,
            in: .userDomainMask
        ).first else {
            throw CocoaError(.fileNoSuchFile)
        }
        return supportDirectory
            .appendingPathComponent(rootDirectoryName, isDirectory: true)
            .appendingPathComponent(engineDirectoryName, isDirectory: true)
            .standardizedFileURL
    }
}
#elseif os(macOS)
/// Stage-A ownership seam. The concrete IPC client is supplied by the signed
/// helper work in stage B; this GUI-side marker owns no Engine or vault handle.
protocol MacOSHelperControlClient: AnyObject, Sendable {}

final class PendingMacOSHelperControlClient: MacOSHelperControlClient, @unchecked Sendable {}
#endif

private enum AppleApplicationProcessOwner {
    #if os(iOS)
    case applicationEngine(ApplicationEngineAdapter)
    #elseif os(macOS)
    case helperControlClient(MacOSHelperControlClient)
    #endif
}

/// Owns process-wide Apple platform effects while each window keeps its own
/// presentation and navigation state.
@MainActor
final class AppleApplicationRuntime: ObservableObject {
    static let shared = AppleApplicationRuntime()

    private let processOwner: AppleApplicationProcessOwner
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

    #if os(iOS)
    /// The single process-wide durable Engine owner shared by every scene.
    var applicationEngine: ApplicationEngineAdapter {
        guard case let .applicationEngine(engine) = processOwner else {
            preconditionFailure("The iOS runtime has no application Engine owner")
        }
        return engine
    }

    var rememberedRelationshipStore: RememberedPeerStoring {
        applicationEngine.relationshipStore
    }
    #elseif os(macOS)
    /// The GUI-side helper boundary. It deliberately exposes no Engine or vault.
    var helperControlClient: MacOSHelperControlClient {
        guard case let .helperControlClient(client) = processOwner else {
            preconditionFailure("The macOS runtime has no helper control client")
        }
        return client
    }
    #endif

    convenience init() {
        #if os(iOS)
        do {
            try self.init(applicationEngine: Self.makePersistentApplicationEngine())
        } catch {
            preconditionFailure("The persistent application Engine could not open: \(error)")
        }
        #elseif os(macOS)
        self.init(helperControlClient: PendingMacOSHelperControlClient())
        #endif
    }

    #if os(iOS)
    convenience init(applicationEngine: ApplicationEngineAdapter) {
        let rememberedStore = applicationEngine.relationshipStore
        self.init(
            processOwner: .applicationEngine(applicationEngine),
            nearbyCoordinator: NearbyDiscoveryCoordinator(),
            presence: NearbyPresencePreferences(),
            workflow: ConnectionWorkflowState(
                gateway: RoomControlGatewayFactory.make(rememberedStore: rememberedStore),
                rememberedStore: rememberedStore
            ),
            rememberedOutbox: RememberedRoomOutboxController(),
            wifiAwareServices: .shared
        )
    }
    #elseif os(macOS)
    convenience init(helperControlClient: MacOSHelperControlClient) {
        self.init(
            processOwner: .helperControlClient(helperControlClient),
            nearbyCoordinator: NearbyDiscoveryCoordinator(),
            presence: NearbyPresencePreferences(),
            workflow: ConnectionWorkflowState(
                gateway: RoomControlGatewayFactory.make()
            ),
            rememberedOutbox: RememberedRoomOutboxController(),
            wifiAwareServices: .shared
        )
    }
    #endif

    private init(
        processOwner: AppleApplicationProcessOwner,
        nearbyCoordinator: NearbyDiscoveryCoordinator,
        presence: NearbyPresencePreferences,
        workflow: ConnectionWorkflowState,
        rememberedOutbox: RememberedRoomOutboxController,
        wifiAwareServices: AppleWifiAwareServiceCoordinator
    ) {
        self.processOwner = processOwner
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

    #if os(iOS)
    private static func makePersistentApplicationEngine() throws -> ApplicationEngineAdapter {
        try ApplicationEngineAdapter.openPersistent(
            stateDirectory: AppleApplicationEngineLocation.persistentStateDirectory(),
            vault: AppleApplicationVault(configuration: .iOSApplication)
        )
    }
    #endif
}
#endif
