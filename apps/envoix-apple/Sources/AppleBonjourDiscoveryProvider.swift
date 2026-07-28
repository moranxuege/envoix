#if os(iOS)
import EnvoixCore
import Foundation
import Network

final class AppleBonjourDiscoveryProvider: NearbyRendezvousProvider,
    NearbyAdvertisingConfigurable {
    let source = NearbyDiscoverySource.mdns

    private enum OperationState {
        case setup
        case ready
        case waiting
        case failed
        case cancelled

        var isSettledUnavailable: Bool {
            switch self {
            case .waiting, .failed, .cancelled: return true
            case .setup, .ready: return false
            }
        }
    }

    private static let observationRefreshInterval: TimeInterval = 5

    private let identity: LocalNearbyDiscoveryIdentity
    private var sink: ((NearbyDiscoveryEvent) -> Void)?
    private var browser: NWBrowser?
    private var listener: NWListener?
    private var refreshTimer: Timer?
    private var recordsByResult: [NWBrowser.Result: NearbyDiscoveryBonjourRecord] = [:]
    private var inboxRoutesByPeerKey: [String: NearbyInviteRoute] = [:]
    private var inbox: FfiNearbyInviteInbox?
    private var inboxStartTask: Task<Void, Never>?
    private var inboxEventTask: Task<Void, Never>?
    private var outboundTask: Task<Void, Never>?
    private var outboundCompletion: ((String?) -> Void)?
    private var outboundGeneration: Int?
    private var browserState = OperationState.setup
    private var listenerState = OperationState.setup
    private var inboxState = OperationState.setup
    private var generation = 0
    private var active = false
    private var advertisingEnabled = false

    init(identity: LocalNearbyDiscoveryIdentity) {
        self.identity = identity
    }

    func start(sink: @escaping (NearbyDiscoveryEvent) -> Void) {
        self.sink = sink
        guard !active else {
            emitOperationalStatus()
            return
        }

        active = true
        generation += 1
        let activeGeneration = generation
        browserState = .setup
        listenerState = advertisingEnabled ? .setup : .cancelled
        inboxState = .setup
        recordsByResult.removeAll()
        inboxRoutesByPeerKey.removeAll()
        emitStatus(.starting, .startingLocalNetwork)

        let browserParameters = NWParameters.udp
        browserParameters.includePeerToPeer = true
        let browser = NWBrowser(
            for: .bonjourWithTXTRecord(type: NearbyDiscoveryBonjourRecord.serviceType, domain: nil),
            using: browserParameters
        )
        browser.stateUpdateHandler = { [weak self] state in
            self?.handleBrowserState(state)
        }
        browser.browseResultsChangedHandler = { [weak self] results, _ in
            self?.accept(results: results)
        }
        self.browser = browser

        browser.start(queue: .main)
        startInbox(generation: activeGeneration)
        startRefreshTimer()
        emitOperationalStatus()
    }

    func setAdvertisingEnabled(_ enabled: Bool) {
        precondition(!active, "Advertising policy must be configured before discovery starts")
        advertisingEnabled = enabled
    }

    func stop() {
        guard active else {
            sink = nil
            return
        }
        active = false
        generation += 1
        inboxStartTask?.cancel()
        inboxStartTask = nil
        inboxEventTask?.cancel()
        inboxEventTask = nil
        outboundTask?.cancel()
        outboundTask = nil
        let pendingCompletion = outboundCompletion
        outboundCompletion = nil
        outboundGeneration = nil
        let activeInbox = inbox
        inbox = nil
        if let activeInbox {
            Task {
                try? await activeInbox.close()
            }
        }
        refreshTimer?.invalidate()
        refreshTimer = nil
        recordsByResult.removeAll()
        inboxRoutesByPeerKey.removeAll()
        browser?.stateUpdateHandler = nil
        browser?.browseResultsChangedHandler = nil
        browser?.cancel()
        stopListener()
        browser = nil
        browserState = .cancelled
        listenerState = .cancelled
        inboxState = .cancelled
        emitStatus(.stopped, .discoveryStopped)
        sink = nil
        pendingCompletion?("Nearby discovery stopped")
    }

    func canOfferInvite(to selection: NearbyPairingSelection) -> Bool {
        guard active,
              inbox != nil,
              let peerKey = NearbyDiscoveryPeerRegistry.normalizePeerKey(
                  selection.discoveryPeerKey
              ),
              let capturedRoute = selection.nearbyInviteRoute else {
            return false
        }
        return inboxRoutesByPeerKey[peerKey] == capturedRoute
    }

    func offerInvite(
        to selection: NearbyPairingSelection,
        invite: String,
        completion: @escaping (String?) -> Void
    ) {
        if !Thread.isMainThread {
            DispatchQueue.main.async { [weak self] in
                self?.offerInvite(to: selection, invite: invite, completion: completion)
            }
            return
        }
        guard active,
              let peerKey = NearbyDiscoveryPeerRegistry.normalizePeerKey(
                  selection.discoveryPeerKey
              ),
              let route = selection.nearbyInviteRoute,
              let inbox else {
            completion("The selected device no longer exposes secure local pairing")
            return
        }
        guard inboxRoutesByPeerKey[peerKey] == route else {
            completion("The selected device's secure endpoint changed. Refresh Nearby and try again")
            return
        }
        guard outboundTask == nil else {
            completion("Another local-network invitation is already being delivered")
            return
        }

        let activeGeneration = generation
        outboundGeneration = activeGeneration
        outboundCompletion = completion
        outboundTask = Task { @MainActor [weak self] in
            do {
                try await inbox.sendInvite(
                    endpoint: FfiNearbyInviteEndpoint(
                        endpointId: route.endpointID,
                        relayUrl: route.relayURL,
                        directAddresses: route.directAddresses
                    ),
                    invite: invite
                )
                self?.finishOutbound(generation: activeGeneration, error: nil)
            } catch {
                self?.finishOutbound(
                    generation: activeGeneration,
                    error: error.localizedDescription
                )
            }
        }
    }

    private func startRefreshTimer() {
        refreshTimer?.invalidate()
        let timer = Timer(timeInterval: Self.observationRefreshInterval, repeats: true) { [weak self] _ in
            self?.emitCurrentObservations()
        }
        RunLoop.main.add(timer, forMode: .common)
        refreshTimer = timer
    }

    private func startInbox(generation: Int) {
        inboxStartTask?.cancel()
        inboxStartTask = Task { @MainActor [weak self] in
            guard let self else { return }
            do {
                let inbox = try await startNearbyInviteInbox(
                    relay: "",
                    peerKey: self.identity.peerKey,
                    displayName: self.identity.displayName
                )
                guard self.active,
                      self.generation == generation,
                      !Task.isCancelled else {
                    try? await inbox.close()
                    return
                }
                let endpoint = inbox.endpoint()
                guard let route = NearbyInviteRoute(
                    endpointID: endpoint.endpointId,
                    relayURL: endpoint.relayUrl,
                    directAddresses: Array(
                        endpoint.directAddresses.prefix(
                            NearbyInviteRoute.maximumDirectAddressCount
                        )
                    )
                ) else {
                    try? await inbox.close()
                    self.inboxStartTask = nil
                    self.inboxState = .failed
                    self.listenerState = self.advertisingEnabled ? .failed : .cancelled
                    self.emitOperationalStatus()
                    return
                }
                self.inbox = inbox
                self.inboxState = .ready
                self.inboxStartTask = nil
                if self.advertisingEnabled {
                    self.startListener(inviteRoute: route)
                }
                self.startInboxEventLoop(inbox, generation: generation)
                self.emitOperationalStatus()
            } catch {
                guard self.active,
                      self.generation == generation,
                      !Task.isCancelled else {
                    return
                }
                self.inboxStartTask = nil
                self.inboxState = .failed
                self.listenerState = self.advertisingEnabled ? .failed : .cancelled
                self.emitOperationalStatus()
            }
        }
    }

    private func startInboxEventLoop(
        _ inbox: FfiNearbyInviteInbox,
        generation: Int
    ) {
        inboxEventTask?.cancel()
        inboxEventTask = Task { @MainActor [weak self] in
            while let self,
                  self.active,
                  self.generation == generation,
                  !Task.isCancelled {
                do {
                    let offer = try await inbox.nextInvite()
                    guard self.active,
                          self.generation == generation,
                          self.inbox === inbox,
                          let senderPeerKey = NearbyDiscoveryPeerRegistry.normalizePeerKey(
                              offer.senderPeerKey
                          ),
                          senderPeerKey != self.identity.peerKey else {
                        continue
                    }
                    let senderEndpointID = NearbyDiscoveryPeerRegistry
                        .normalizeInboxEndpointID(offer.senderEndpointId)
                    guard self.acceptsIncomingInvite(
                        senderPeerKey: senderPeerKey,
                        senderEndpointID: senderEndpointID
                    ) else {
                        continue
                    }
                    self.sink?(.rendezvousOffer(NearbyRendezvousOffer(
                        requestID: String(format: "%016llx", offer.requestId),
                        senderPeerKey: senderPeerKey,
                        senderDisplayName: NearbyDiscoveryPeerRegistry.sanitizeDisplayName(
                            offer.senderDisplayName
                        ),
                        source: self.source,
                        senderInboxEndpointID: senderEndpointID,
                        invite: offer.invite
                    )))
                } catch {
                    guard self.active,
                          self.generation == generation,
                          !Task.isCancelled else {
                        return
                    }
                    self.failInbox(inbox, generation: generation)
                    return
                }
            }
        }
    }

    private func startListener(inviteRoute: NearbyInviteRoute) {
        guard active, advertisingEnabled, listener == nil else { return }
        do {
            let listenerParameters = NWParameters.udp
            listenerParameters.includePeerToPeer = true
            let listener = try NWListener(using: listenerParameters, on: .any)
            let record = NearbyDiscoveryBonjourRecord(
                identity: identity,
                inviteRoute: inviteRoute
            )
            listener.service = NWListener.Service(
                name: "Envoix-\(identity.peerKey.prefix(8))",
                type: NearbyDiscoveryBonjourRecord.serviceType,
                domain: nil,
                txtRecord: NWTXTRecord(record.dictionary)
            )
            listener.newConnectionHandler = { connection in
                connection.cancel()
            }
            listener.stateUpdateHandler = { [weak self] state in
                self?.handleListenerState(state)
            }
            self.listener = listener
            listener.start(queue: .main)
        } catch {
            listenerState = .failed
        }
    }

    private func stopListener() {
        listener?.stateUpdateHandler = nil
        listener?.newConnectionHandler = nil
        listener?.cancel()
        listener = nil
    }

    private func failInbox(_ failedInbox: FfiNearbyInviteInbox, generation: Int) {
        guard active, self.generation == generation, inbox === failedInbox else { return }
        Task {
            try? await failedInbox.close()
        }
        inbox = nil
        inboxState = .failed
        inboxEventTask = nil
        outboundTask?.cancel()
        outboundTask = nil
        let completion = outboundCompletion
        outboundCompletion = nil
        outboundGeneration = nil
        if advertisingEnabled {
            stopListener()
            listenerState = .failed
        }
        emitOperationalStatus()
        completion?("Secure local pairing stopped unexpectedly")
    }

    private func finishOutbound(generation: Int, error: String?) {
        guard outboundGeneration == generation else { return }
        outboundTask = nil
        outboundGeneration = nil
        let completion = outboundCompletion
        outboundCompletion = nil
        guard active, self.generation == generation else {
            completion?("Nearby discovery stopped")
            return
        }
        completion?(error)
    }

    private func handleBrowserState(_ state: NWBrowser.State) {
        guard active else { return }
        switch state {
        case .setup:
            browserState = .setup
        case .ready:
            browserState = .ready
        case .waiting:
            browserState = .waiting
            recordsByResult.removeAll()
            inboxRoutesByPeerKey.removeAll()
        case .failed:
            browserState = .failed
            recordsByResult.removeAll()
            inboxRoutesByPeerKey.removeAll()
        case .cancelled:
            browserState = .cancelled
            recordsByResult.removeAll()
            inboxRoutesByPeerKey.removeAll()
        @unknown default:
            browserState = .failed
            recordsByResult.removeAll()
            inboxRoutesByPeerKey.removeAll()
        }
        emitOperationalStatus()
    }

    private func handleListenerState(_ state: NWListener.State) {
        guard active else { return }
        switch state {
        case .setup:
            listenerState = .setup
        case .ready:
            listenerState = .ready
        case .waiting:
            listenerState = .waiting
        case .failed:
            listenerState = .failed
        case .cancelled:
            listenerState = .cancelled
        @unknown default:
            listenerState = .failed
        }
        emitOperationalStatus()
    }

    private func accept(results: Set<NWBrowser.Result>) {
        guard active else { return }
        var accepted: [NWBrowser.Result: NearbyDiscoveryBonjourRecord] = [:]
        for result in results {
            guard case .bonjour(let txtRecord) = result.metadata,
                  let record = NearbyDiscoveryBonjourRecord(dictionary: txtRecord.dictionary),
                  record.peerKey != identity.peerKey else {
                continue
            }
            accepted[result] = record
        }
        recordsByResult = accepted
        refreshInboxRoutes()
        emitCurrentObservations()
    }

    private func refreshInboxRoutes() {
        let grouped = Dictionary(grouping: recordsByResult.values, by: \.peerKey)
        inboxRoutesByPeerKey = grouped.compactMapValues { records in
            NearbyDiscoveryBonjourRecord.consistentInviteRoute(in: records)
        }
    }

    private func acceptsIncomingInvite(
        senderPeerKey: String,
        senderEndpointID: String?
    ) -> Bool {
        let claimedRecords = recordsByResult.values.filter {
            $0.peerKey == senderPeerKey
        }
        guard !claimedRecords.isEmpty else { return true }
        guard let senderEndpointID else { return false }
        return claimedRecords.contains {
            $0.inviteRoute?.endpointID == senderEndpointID
        }
    }

    private func emitCurrentObservations() {
        guard active else { return }
        let now = Int64(ProcessInfo.processInfo.systemUptime * 1_000)
        for record in recordsByResult.values {
            sink?(.observation(NearbyDiscoveryObservation(
                peerKey: record.peerKey,
                source: source,
                seenAtMilliseconds: now,
                displayName: record.displayName,
                inviteRoute: inboxRoutesByPeerKey[record.peerKey]
            )))
        }
    }

    private func emitOperationalStatus() {
        guard active else { return }
        if browserState == .ready && !advertisingEnabled && inboxState == .ready {
            emitStatus(.ready, .localNetworkScanningOnly)
        } else if browserState == .ready && listenerState == .ready && inboxState == .ready {
            emitStatus(.ready, .localNetworkReady)
        } else if browserState == .ready
                    && (listenerState.isSettledUnavailable || inboxState.isSettledUnavailable) {
            emitStatus(.degraded, .localNetworkScanningOnly)
        } else if listenerState == .ready
                    && inboxState == .ready
                    && browserState.isSettledUnavailable {
            emitStatus(.degraded, .localNetworkVisibleOnly)
        } else if browserState == .setup || listenerState == .setup || inboxState == .setup {
            emitStatus(.starting, .startingLocalNetwork)
        } else {
            emitStatus(.temporarilyUnavailable, .localNetworkPermissionOrUnavailable)
        }
    }

    private func emitStatus(_ availability: NearbyProviderAvailability, _ detail: NearbyProviderDetail) {
        sink?(.status(NearbyProviderStatus(source: source, availability: availability, detail: detail)))
    }
}
#endif
