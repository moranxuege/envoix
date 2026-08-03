#if os(iOS) || os(macOS)
import Combine
import EnvoixCore
import Foundation
import OSLog
#if os(iOS)
import UIKit
#endif

struct NearbyDiscoveryState {
    var localName: String
    var isActive: Bool
    var nowMilliseconds: Int64
    var peers: [NearbyDiscoveredPeer]
    var pairedDevices: [NearbyPairedDevice]
    var statuses: [NearbyDiscoverySource: NearbyProviderStatus]
    var incomingRendezvousOffer: NearbyRendezvousOffer?
    var incomingNFCReadinessOffer: NearbyNFCReadinessOffer?
}

final class NearbyDiscoveryCoordinator: ObservableObject {
    typealias ProviderFactory = (LocalNearbyDiscoveryIdentity) -> [NearbyDiscoveryProvider]
    static let maximumPendingRendezvousOfferCount = 16

    private struct RendezvousOfferKey: Hashable {
        let source: NearbyDiscoverySource
        let senderPeerKey: String
        let requestID: String

        init(_ offer: NearbyRendezvousOffer) {
            source = offer.source
            senderPeerKey = offer.senderPeerKey
            requestID = offer.requestID
        }
    }

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
    private var nfcReadinessOffers = NearbyNFCReadinessOfferRegistry()
    private var refreshTimer: Timer?
    private var lastLoggedAvailability: [NearbyDiscoverySource: NearbyProviderAvailability] = [:]
    private var generation = 0
    private var started = false
    private var advertisingEnabled = false
    private var pendingRendezvousOffers: [NearbyRendezvousOffer] = []
    private var pendingRendezvousOfferKeys = Set<RendezvousOfferKey>()

    init(
        identity: LocalNearbyDiscoveryIdentity? = nil,
        identityFactory: (() -> LocalNearbyDiscoveryIdentity)? = nil,
        registry: NearbyDiscoveryPeerRegistry = NearbyDiscoveryPeerRegistry(),
        clock: @escaping () -> Int64 = NearbyDiscoveryCoordinator.monotonicMilliseconds,
        providerFactory: @escaping ProviderFactory = NearbyDiscoveryCoordinator.defaultProviderFactory
    ) {
        let resolvedIdentity = identity
            ?? identityFactory?()
            ?? NearbyDiscoveryIdentityFactory.create(displayName: Self.defaultDisplayName)
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
            pairedDevices: [],
            statuses: Dictionary(uniqueKeysWithValues: NearbyDiscoverySource.allCases.map { source in
                (source, NearbyProviderStatus(
                    source: source,
                    availability: .stopped,
                    detail: .discoveryStopped
                ))
            }),
            incomingRendezvousOffer: nil,
            incomingNFCReadinessOffer: nil
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
        state.pairedDevices = []
        clearPendingRendezvousOffers()
        state.incomingNFCReadinessOffer = nil
        if providers.isEmpty {
            providers = providerFactory(identity)
        }
        providers.forEach { provider in
            (provider as? NearbyIdentityConfigurable)?
                .setIdentity(identity)
            (provider as? NearbyAdvertisingConfigurable)?
                .setAdvertisingEnabled(advertisingEnabled)
            (provider as? NearbyRendezvousAdmissionConfigurable)?
                .setRendezvousOfferAdmission { [weak self] offer in
                    guard let self,
                          self.started,
                          self.generation == activeGeneration,
                          offer.senderPeerKey != self.identity.peerKey else {
                        return false
                    }
                    return self.enqueueRendezvousOffer(offer)
                }
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
        activeProviders.forEach { $0.stop() }
        registry.clear()
        state.isActive = false
        state.peers = []
        state.pairedDevices = []
        clearPendingRendezvousOffers()
        state.incomingNFCReadinessOffer = nil
        for source in NearbyDiscoverySource.allCases {
            state.statuses[source] = NearbyProviderStatus(
                source: source,
                availability: .stopped,
                detail: .discoveryStopped
            )
        }
        refreshPeers()
    }

    func suspendForSystemPairing() async {
        let quiescingProviders = providers.compactMap {
            $0 as? NearbySystemPairingQuiescing
        }
        stop()
        for provider in quiescingProviders {
            await provider.waitUntilStopped()
        }
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
        let isRoomControlInvite = invite.trimmed.hasPrefix(roomControlURLPrefix)
        guard started else {
            logUnavailableInvite(selection: selection, reason: "discovery_stopped")
            completion("Nearby invitation delivery is not available for this device")
            return
        }
        guard let provider = liveRendezvousProvider(
            to: selection,
            isRoomControlInvite: isRoomControlInvite
        ) else {
            let hasCapturedRoute = selection.nearbyWifiAwareDeviceID != nil
                || (isRoomControlInvite && selection.nearbyInviteRoute != nil)
            logUnavailableInvite(
                selection: selection,
                reason: hasCapturedRoute ? "route_not_ready" : "route_missing"
            )
            completion("Nearby invitation delivery is not available for this device")
            return
        }
        logger.info(
            "RENDEZVOUS action=offer route=\(provider.source.logName, privacy: .public) state=ready"
        )
        let route = provider.source.logName
        let activeGeneration = generation
        provider.offerInvite(to: selection, invite: invite) { [weak self] error in
            DispatchQueue.main.async {
                guard let self else {
                    completion("Nearby discovery stopped")
                    return
                }
                guard self.started, self.generation == activeGeneration else {
                    self.logger.error(
                        "RENDEZVOUS action=offer route=\(route, privacy: .public) result=discarded reason=discovery_stopped"
                    )
                    completion("Nearby discovery stopped")
                    return
                }
                if error == nil {
                    self.logger.info(
                        "RENDEZVOUS action=offer route=\(route, privacy: .public) result=acknowledged"
                    )
                } else {
                    self.logger.error(
                        "RENDEZVOUS action=offer route=\(route, privacy: .public) result=failed"
                    )
                }
                completion(error)
            }
        }
    }

    func canOfferRoomInvite(to selection: NearbyPairingSelection) -> Bool {
        started && liveRendezvousProvider(
            to: selection,
            isRoomControlInvite: true
        ) != nil
    }

    private func liveRendezvousProvider(
        to selection: NearbyPairingSelection,
        isRoomControlInvite: Bool
    ) -> NearbyRendezvousProvider? {
        let rendezvousProviders = providers.compactMap {
            $0 as? NearbyRendezvousProvider
        }
        let hasExactWifiAwareRoute = selection.nearbyWifiAwareDeviceID != nil
        let hasSecureMdnsRoute = isRoomControlInvite
            && selection.sources.contains(.mdns)
            && selection.nearbyInviteRoute != nil
        if hasExactWifiAwareRoute {
            return rendezvousProviders.first {
                $0.source == .wifiAware && $0.canOfferInvite(to: selection)
            }
        }
        if hasSecureMdnsRoute {
            return rendezvousProviders.first {
                $0.source == .mdns && $0.canOfferInvite(to: selection)
            }
        }
        return rendezvousProviders.first {
            $0.source != .wifiAware
                && $0.source != .mdns
                && selection.sources.contains($0.source)
                && $0.canOfferInvite(to: selection)
        }
    }

    private func logUnavailableInvite(
        selection: NearbyPairingSelection,
        reason: String
    ) {
        logger.error(
            "RENDEZVOUS action=offer result=unavailable reason=\(reason, privacy: .public) bluetooth=\(selection.sources.contains(.bluetooth)) mdns=\(selection.sources.contains(.mdns)) wifi_aware=\(selection.sources.contains(.wifiAware)) mdns_route=\(selection.nearbyInviteRoute != nil) wifi_aware_route=\(selection.nearbyWifiAwareDeviceID != nil)"
        )
    }

    func consumeRendezvousOffer(id: String) {
        guard pendingRendezvousOffers.first?.id == id,
              state.incomingRendezvousOffer?.id == id else {
            return
        }
        let consumed = pendingRendezvousOffers.removeFirst()
        pendingRendezvousOfferKeys.remove(RendezvousOfferKey(consumed))
        state.incomingRendezvousOffer = pendingRendezvousOffers.first
    }

    func consumeNFCReadinessOffer(id: String) {
        guard state.incomingNFCReadinessOffer?.id == id else { return }
        state.incomingNFCReadinessOffer = nil
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
        case .pairedDevices(let source, let devices):
            guard started else { return }
            replacePairedDevices(from: source, with: devices)
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
            enqueueRendezvousOffer(offer)
        case .nfcPresenterReadiness(
            let offerID,
            let presenterPeerKey,
            let presenterID
        ):
            guard started, presenterPeerKey != identity.peerKey else { return }
            let now = clock()
            guard let offer = nfcReadinessOffers.observe(
                offerID: offerID,
                presenterPeerKey: presenterPeerKey,
                presenterID: presenterID,
                at: now
            ) else {
                return
            }
            state.nowMilliseconds = now
            state.incomingNFCReadinessOffer = offer
        }
    }

    @discardableResult
    private func enqueueRendezvousOffer(_ offer: NearbyRendezvousOffer) -> Bool {
        let key = RendezvousOfferKey(offer)
        guard !pendingRendezvousOfferKeys.contains(key) else { return true }
        guard pendingRendezvousOffers.count < Self.maximumPendingRendezvousOfferCount else {
            logger.error("RENDEZVOUS rejected=queue_limit")
            return false
        }
        pendingRendezvousOffers.append(offer)
        pendingRendezvousOfferKeys.insert(key)
        if state.incomingRendezvousOffer == nil {
            state.incomingRendezvousOffer = offer
        }
        return true
    }

    private func clearPendingRendezvousOffers() {
        pendingRendezvousOffers.removeAll(keepingCapacity: true)
        pendingRendezvousOfferKeys.removeAll(keepingCapacity: true)
        state.incomingRendezvousOffer = nil
    }

    private func refreshPeers() {
        let now = clock()
        state.nowMilliseconds = now
        state.peers = registry.peers(nowMilliseconds: now)
        if let offer = state.incomingNFCReadinessOffer,
           !offer.isFresh(at: now) {
            state.incomingNFCReadinessOffer = nil
        }
    }

    private func replacePairedDevices(
        from source: NearbyDiscoverySource,
        with devices: [NearbyPairedDevice]
    ) {
        guard devices.count <= NearbyPairedDevice.maximumSnapshotCount else {
            logger.error("PAIRING provider=\(source.logName, privacy: .public) rejected=snapshot_limit")
            return
        }
        guard devices.allSatisfy({ $0.source == source }) else {
            logger.error("PAIRING provider=\(source.logName, privacy: .public) rejected=source_mismatch")
            return
        }
        var seen = Set<String>()
        let replacement = devices
            .filter { seen.insert($0.id).inserted }
            .sorted { $0.id < $1.id }
        state.pairedDevices.removeAll { $0.source == source }
        state.pairedDevices.append(contentsOf: replacement)
        state.pairedDevices.sort { $0.id < $1.id }
    }

    private static func monotonicMilliseconds() -> Int64 {
        Int64(ProcessInfo.processInfo.systemUptime * 1_000)
    }

    private static var defaultDisplayName: String {
        #if os(iOS)
        UIDevice.current.model
        #else
        NearbyDiscoveryPeerRegistry.sanitizeDisplayName(Host.current().localizedName)
            ?? "Mac"
        #endif
    }

    static func defaultProviderFactory(
        identity: LocalNearbyDiscoveryIdentity
    ) -> [NearbyDiscoveryProvider] {
        #if DEBUG
        if ProcessInfo.processInfo.arguments.contains("--ui-testing-discovery-fixtures") {
            #if os(iOS)
            return [
                FixtureNearbyDiscoveryProvider(source: .bluetooth),
                FixtureNearbyDiscoveryProvider(source: .mdns),
                ReservedNearbyDiscoveryProvider(),
            ]
            #else
            return [
                FixtureNearbyDiscoveryProvider(source: .bluetooth),
                FixtureNearbyDiscoveryProvider(source: .mdns),
                UnsupportedWifiAwareDiscoveryProvider(),
            ]
            #endif
        }
        #endif
        #if os(macOS)
        return [
            AppleBluetoothDiscoveryProvider(identity: identity),
            AppleBonjourDiscoveryProvider(identity: identity),
            UnsupportedWifiAwareDiscoveryProvider(),
        ]
        #else
        var providers: [NearbyDiscoveryProvider] = [
            AppleBluetoothDiscoveryProvider(identity: identity),
            AppleBonjourDiscoveryProvider(identity: identity),
        ]
        #if canImport(WiFiAware)
        if #available(iOS 26.0, *) {
            providers.append(AppleWifiAwarePairingProvider(identity: identity))
        } else {
            providers.append(UnsupportedWifiAwareDiscoveryProvider())
        }
        #else
        providers.append(UnsupportedWifiAwareDiscoveryProvider())
        #endif
        return providers
        #endif
    }
}

final class UnsupportedWifiAwareDiscoveryProvider: NearbyDiscoveryProvider {
    let source = NearbyDiscoverySource.wifiAware

    func start(sink: @escaping (NearbyDiscoveryEvent) -> Void) {
        sink(.status(NearbyProviderStatus(
            source: source,
            availability: .unsupported,
            detail: .wifiAwareUnsupported
        )))
    }

    func stop() {}
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
    private static let observationRefreshInterval: TimeInterval = 5

    let source: NearbyDiscoverySource
    private var sink: ((NearbyDiscoveryEvent) -> Void)?
    private var observationRefreshTimer: Timer?

    init(source: NearbyDiscoverySource) {
        self.source = source
    }

    func start(sink: @escaping (NearbyDiscoveryEvent) -> Void) {
        self.sink = sink
        let detail: NearbyProviderDetail = source == .bluetooth ? .bluetoothReady : .localNetworkReady
        sink(.status(NearbyProviderStatus(source: source, availability: .ready, detail: detail)))
        emitObservation()
        let timer = Timer(timeInterval: Self.observationRefreshInterval, repeats: true) { [weak self] _ in
            self?.emitObservation()
        }
        RunLoop.main.add(timer, forMode: .common)
        observationRefreshTimer = timer
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
        observationRefreshTimer?.invalidate()
        observationRefreshTimer = nil
        sink = nil
    }

    private func emitObservation() {
        guard let sink else { return }
        sink(.observation(NearbyDiscoveryObservation(
            peerKey: "0011223344556677",
            source: source,
            seenAtMilliseconds: Int64(ProcessInfo.processInfo.systemUptime * 1_000),
            displayName: source == .mdns ? "Nearby test device" : nil,
            rssi: source == .bluetooth ? -48 : nil,
            inviteRoute: source == .mdns ? Self.fixtureInviteRoute : nil
        )))
    }

    func offerInvite(
        to selection: NearbyPairingSelection,
        invite: String,
        completion: @escaping (String?) -> Void
    ) {
        let validPeer = NearbyDiscoveryPeerRegistry.normalizePeerKey(
            selection.discoveryPeerKey
        ) != nil
        let validInvite = source == .bluetooth
            ? BleRendezvousProtocol.isSupportedBluetoothVerificationOffer(invite.trimmed)
            : BleRendezvousProtocol.isSupportedInvite(invite.trimmed)
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
