#if os(iOS)
import Combine
import Foundation
import OSLog
import UIKit

struct NearbyDiscoveryState {
    var localName: String
    var isActive: Bool
    var nowMilliseconds: Int64
    var peers: [NearbyDiscoveredPeer]
    var statuses: [NearbyDiscoverySource: NearbyProviderStatus]
    var incomingRendezvousOffer: NearbyRendezvousOffer?
}

final class NearbyDiscoveryCoordinator: ObservableObject {
    typealias ProviderFactory = (LocalNearbyDiscoveryIdentity) -> [NearbyDiscoveryProvider]

    @Published private(set) var state: NearbyDiscoveryState

    private var identity: LocalNearbyDiscoveryIdentity
    private let identityFactory: () -> LocalNearbyDiscoveryIdentity
    private let registry: NearbyDiscoveryPeerRegistry
    private let clock: () -> Int64
    private let providerFactory: ProviderFactory
    private let logger = Logger(
        subsystem: Bundle.main.bundleIdentifier ?? "com.envoix.app",
        category: "discovery"
    )

    private var providers: [NearbyDiscoveryProvider] = []
    private var refreshTimer: Timer?
    private var lastLoggedAvailability: [NearbyDiscoverySource: NearbyProviderAvailability] = [:]
    private var generation = 0
    private var started = false
    private var hasStarted = false

    init(
        identity: LocalNearbyDiscoveryIdentity? = nil,
        identityFactory: (() -> LocalNearbyDiscoveryIdentity)? = nil,
        registry: NearbyDiscoveryPeerRegistry = NearbyDiscoveryPeerRegistry(),
        clock: @escaping () -> Int64 = NearbyDiscoveryCoordinator.monotonicMilliseconds,
        providerFactory: @escaping ProviderFactory = NearbyDiscoveryCoordinator.defaultProviderFactory
    ) {
        let resolvedIdentityFactory: () -> LocalNearbyDiscoveryIdentity
        if let identity {
            resolvedIdentityFactory = { identity }
        } else if let identityFactory {
            resolvedIdentityFactory = identityFactory
        } else {
            resolvedIdentityFactory = {
                NearbyDiscoveryIdentityFactory.create(displayName: UIDevice.current.model)
            }
        }
        let resolvedIdentity = resolvedIdentityFactory()
        self.identity = resolvedIdentity
        self.identityFactory = resolvedIdentityFactory
        self.registry = registry
        self.clock = clock
        self.providerFactory = providerFactory
        let now = clock()
        self.state = NearbyDiscoveryState(
            localName: resolvedIdentity.displayName,
            isActive: false,
            nowMilliseconds: now,
            peers: [],
            statuses: Dictionary(uniqueKeysWithValues: NearbyDiscoverySource.allCases.map { source in
                (source, NearbyProviderStatus(
                    source: source,
                    availability: .stopped,
                    detail: .discoveryStopped
                ))
            }),
            incomingRendezvousOffer: nil
        )
    }

    deinit {
        refreshTimer?.invalidate()
        providers.forEach { $0.stop() }
    }

    func start() {
        start(rotateIdentity: hasStarted)
    }

    private func start(rotateIdentity: Bool) {
        guard !started else { return }
        started = true
        if rotateIdentity {
            identity = identityFactory()
            state.localName = identity.displayName
        }
        hasStarted = true
        generation += 1
        let activeGeneration = generation
        registry.clear()
        state.isActive = true
        state.peers = []
        state.incomingRendezvousOffer = nil
        providers = providerFactory(identity)
        providers.forEach { provider in
            provider.start { [weak self] event in
                guard let self else { return }
                if Thread.isMainThread {
                    self.handle(event, generation: activeGeneration)
                } else {
                    DispatchQueue.main.async { [weak self] in
                        self?.handle(event, generation: activeGeneration)
                    }
                }
            }
        }
        startRefreshTimer()
        refreshPeers()
    }

    func stop() {
        guard started else { return }
        started = false
        generation += 1
        refreshTimer?.invalidate()
        refreshTimer = nil
        let activeProviders = providers
        providers = []
        activeProviders.forEach { $0.stop() }
        registry.clear()
        state.isActive = false
        state.peers = []
        state.incomingRendezvousOffer = nil
        for source in NearbyDiscoverySource.allCases {
            state.statuses[source] = NearbyProviderStatus(
                source: source,
                availability: .stopped,
                detail: .discoveryStopped
            )
        }
        refreshPeers()
    }

    func restart() {
        guard started else {
            start()
            return
        }
        stop()
        start(rotateIdentity: false)
    }

    func offerInvite(
        peerKey: String,
        invite: String,
        completion: @escaping (_ error: String?) -> Void
    ) {
        guard started,
              let provider = providers.compactMap({ $0 as? NearbyRendezvousProvider }).first else {
            completion("Experimental Bluetooth pairing is not available")
            return
        }
        let activeGeneration = generation
        provider.offerInvite(peerKey: peerKey, invite: invite) { [weak self] error in
            DispatchQueue.main.async {
                guard let self, self.started, self.generation == activeGeneration else {
                    completion("Bluetooth discovery stopped")
                    return
                }
                completion(error)
            }
        }
    }

    func consumeRendezvousOffer(id: String) {
        guard state.incomingRendezvousOffer?.id == id else { return }
        state.incomingRendezvousOffer = nil
    }

    private func startRefreshTimer() {
        refreshTimer?.invalidate()
        let timer = Timer(timeInterval: 1, repeats: true) { [weak self] _ in
            self?.refreshPeers()
        }
        RunLoop.main.add(timer, forMode: .common)
        refreshTimer = timer
    }

    private func handle(_ event: NearbyDiscoveryEvent, generation: Int) {
        guard generation == self.generation else { return }
        switch event {
        case .observation(let observation):
            guard started, observation.peerKey != identity.peerKey else { return }
            if registry.upsert(observation) {
                refreshPeers()
            }
        case .status(let status):
            state.statuses[status.source] = status
            if lastLoggedAvailability[status.source] != status.availability {
                lastLoggedAvailability[status.source] = status.availability
                logger.info(
                    "DISCOVERY provider=\(status.source.logName, privacy: .public) state=\(status.availability.logName, privacy: .public)"
                )
            }
        case .rendezvousOffer(let offer):
            guard started, offer.senderPeerKey != identity.peerKey else { return }
            state.incomingRendezvousOffer = offer
        }
    }

    private func refreshPeers() {
        let now = clock()
        state.nowMilliseconds = now
        state.peers = registry.peers(nowMilliseconds: now)
    }

    private static func monotonicMilliseconds() -> Int64 {
        Int64(ProcessInfo.processInfo.systemUptime * 1_000)
    }

    private static func defaultProviderFactory(
        identity: LocalNearbyDiscoveryIdentity
    ) -> [NearbyDiscoveryProvider] {
        #if DEBUG
        if ProcessInfo.processInfo.arguments.contains("--ui-testing-discovery-fixtures") {
            return [
                FixtureNearbyDiscoveryProvider(source: .bluetooth),
                FixtureNearbyDiscoveryProvider(source: .mdns),
                ReservedNearbyDiscoveryProvider(),
            ]
        }
        #endif
        return [
            AppleBluetoothDiscoveryProvider(identity: identity),
            AppleBonjourDiscoveryProvider(identity: identity),
            ReservedNearbyDiscoveryProvider(),
        ]
    }
}

final class ReservedNearbyDiscoveryProvider: NearbyDiscoveryProvider {
    let source = NearbyDiscoverySource.wifiAware
    private var sink: ((NearbyDiscoveryEvent) -> Void)?

    func start(sink: @escaping (NearbyDiscoveryEvent) -> Void) {
        self.sink = sink
        sink(.status(NearbyProviderStatus(
            source: source,
            availability: .reserved,
            detail: .wifiAwareReserved
        )))
    }

    func stop() {
        sink = nil
    }
}

#if DEBUG
private final class FixtureNearbyDiscoveryProvider: NearbyDiscoveryProvider, NearbyRendezvousProvider {
    let source: NearbyDiscoverySource
    private var sink: ((NearbyDiscoveryEvent) -> Void)?

    init(source: NearbyDiscoverySource) {
        self.source = source
    }

    func start(sink: @escaping (NearbyDiscoveryEvent) -> Void) {
        self.sink = sink
        let detail: NearbyProviderDetail = source == .bluetooth ? .bluetoothReady : .localNetworkReady
        sink(.status(NearbyProviderStatus(source: source, availability: .ready, detail: detail)))
        sink(.observation(NearbyDiscoveryObservation(
            peerKey: "0011223344556677",
            source: source,
            seenAtMilliseconds: Int64(ProcessInfo.processInfo.systemUptime * 1_000),
            displayName: source == .mdns ? "Nearby test device" : nil,
            rssi: source == .bluetooth ? -48 : nil
        )))
    }

    func stop() {
        sink = nil
    }

    func offerInvite(
        peerKey: String,
        invite: String,
        completion: @escaping (String?) -> Void
    ) {
        let validPeer = NearbyDiscoveryPeerRegistry.normalizePeerKey(peerKey) != nil
        let validInvite = invite.lowercased().hasPrefix("envoix://pair/")
        completion(validPeer && validInvite ? nil : "Invalid fixture invitation")
    }
}
#endif
#endif
