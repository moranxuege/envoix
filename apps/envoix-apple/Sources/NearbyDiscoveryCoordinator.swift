#if os(iOS)
import Combine
import EnvoixCore
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
    private var advertisingEnabled = false
    init(
        identity: LocalNearbyDiscoveryIdentity? = nil,
        identityFactory: (() -> LocalNearbyDiscoveryIdentity)? = nil,
        registry: NearbyDiscoveryPeerRegistry = NearbyDiscoveryPeerRegistry(),
        clock: @escaping () -> Int64 = NearbyDiscoveryCoordinator.monotonicMilliseconds,
        providerFactory: @escaping ProviderFactory = NearbyDiscoveryCoordinator.defaultProviderFactory
    ) {
        let resolvedIdentity = identity
            ?? identityFactory?()
            ?? NearbyDiscoveryIdentityFactory.create(displayName: UIDevice.current.model)
        self.identity = resolvedIdentity
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
        guard !started else { return }
        started = true
        generation += 1
        let activeGeneration = generation
        registry.clear()
        state.isActive = true
        state.peers = []
        state.incomingRendezvousOffer = nil
        providers = providerFactory(identity)
        providers.forEach { provider in
            (provider as? NearbyAdvertisingConfigurable)?
                .setAdvertisingEnabled(advertisingEnabled)
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
        stop()
        start()
    }

    func configure(displayName: String, advertisingEnabled: Bool) {
        let resolvedName = NearbyDiscoveryPeerRegistry.sanitizeDisplayName(displayName)
            ?? identity.displayName
        let needsRestart = resolvedName != identity.displayName
            || advertisingEnabled != self.advertisingEnabled
        guard needsRestart else { return }

        let wasStarted = started
        if wasStarted {
            stop()
        }
        identity = LocalNearbyDiscoveryIdentity(
            peerKey: identity.peerKey,
            displayName: resolvedName
        )
        self.advertisingEnabled = advertisingEnabled
        state.localName = resolvedName
        if wasStarted {
            start()
        }
    }

    func offerInvite(
        to selection: NearbyPairingSelection,
        invite: String,
        completion: @escaping (_ error: String?) -> Void
    ) {
        let rendezvousProviders = providers.compactMap { $0 as? NearbyRendezvousProvider }
        let isRoomControlInvite = invite.trimmed.lowercased().hasPrefix("envoix://room/")
        let selectedSecureMdns = isRoomControlInvite
            && selection.sources.contains(.mdns)
            && selection.nearbyInviteRoute != nil
        let provider = selectedSecureMdns
            ? rendezvousProviders.first { $0.source == .mdns }
            : rendezvousProviders.first {
                $0.source != .mdns && $0.canOfferInvite(to: selection)
            }
        guard started, let provider else {
            completion("Nearby invitation delivery is not available for this device")
            return
        }
        let activeGeneration = generation
        provider.offerInvite(to: selection, invite: invite) { [weak self] error in
            DispatchQueue.main.async {
                guard let self, self.started, self.generation == activeGeneration else {
                    completion("Nearby discovery stopped")
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
private final class FixtureNearbyDiscoveryProvider: NearbyRendezvousProvider {
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
            rssi: source == .bluetooth ? -48 : nil,
            inviteRoute: source == .mdns ? Self.fixtureInviteRoute : nil
        )))
        if source == .bluetooth,
           ProcessInfo.processInfo.arguments.contains("--ui-testing-incoming-nearby-offer"),
           let invite = try? makePairingInvite(role: .send, broker: "", relay: "") {
            sink(.rendezvousOffer(NearbyRendezvousOffer(
                requestID: "ui-test-incoming-offer",
                senderPeerKey: "0011223344556677",
                senderDisplayName: "Nearby test device",
                source: source,
                senderInboxEndpointID: source == .mdns ? Self.fixtureInboxEndpointID : nil,
                invite: invite.payload
            )))
        }
    }

    func stop() {
        sink = nil
    }

    func offerInvite(
        to selection: NearbyPairingSelection,
        invite: String,
        completion: @escaping (String?) -> Void
    ) {
        let validPeer = NearbyDiscoveryPeerRegistry.normalizePeerKey(
            selection.discoveryPeerKey
        ) != nil
        let normalizedInvite = invite.lowercased()
        let validInvite = normalizedInvite.hasPrefix("envoix://pair/")
            || normalizedInvite.hasPrefix("envoix://room/")
        completion(validPeer && validInvite ? nil : "Invalid fixture invitation")
    }

    func canOfferInvite(to selection: NearbyPairingSelection) -> Bool {
        guard NearbyDiscoveryPeerRegistry.normalizePeerKey(
            selection.discoveryPeerKey
        ) != nil else {
            return false
        }
        return source != .mdns || selection.nearbyInviteRoute != nil
    }

    private static let fixtureInboxEndpointID =
        "2cfu7vzc7zhqv6w3k7m2kkwqvwzppmzvv53lmst6xm7ubjx5qnya"
    private static let fixtureInviteRoute = NearbyInviteRoute(
        endpointID: fixtureInboxEndpointID,
        relayURL: "https://relay.example.test",
        directAddresses: ["192.0.2.1:4242"]
    )!
}
#endif
#endif
