#if os(iOS) && canImport(WiFiAware)
import Foundation
import Network
import OSLog
import WiFiAware

@available(iOS 26.0, *)
final class AppleWifiAwarePairingProvider: NearbyRendezvousProvider,
    NearbyAdvertisingConfigurable,
    NearbyIdentityConfigurable,
    NearbyRendezvousAdmissionConfigurable,
    NearbySystemPairingQuiescing {
    let source = NearbyDiscoverySource.wifiAware

    private static let retryDelay: Duration = .seconds(1)

    private var identity: LocalNearbyDiscoveryIdentity
    private let networkingMode: AppleWifiAwareRendezvousNetworkingMode
    private let lock = NSLock()
    private let logger = Logger(
        subsystem: Bundle.main.bundleIdentifier ?? "com.envoix.app",
        category: "wifi-aware-pairing"
    )

    private var generation = 0
    private var sink: ((NearbyDiscoveryEvent) -> Void)?
    private var observationTask: Task<Void, Never>?
    private var rendezvousSession: AppleWifiAwareRendezvousSession?
    private var rendezvousLifecycleTask: Task<Void, Never>?
    private var rendezvousRoutes: [String: String] = [:]
    private var outboundID: UUID?
    private var outboundTask: Task<Void, Never>?
    private var outboundCompletion: ((String?) -> Void)?
    private var lastPublishedDevices: [NearbyPairedDevice]?
    private var pairedDeviceRevision: UInt64 = 0
    private var rendezvousOfferAdmission:
        (@MainActor (NearbyRendezvousOffer) -> Bool)?

    init(
        identity: LocalNearbyDiscoveryIdentity,
        networkingMode: AppleWifiAwareRendezvousNetworkingMode = .automatic
    ) {
        self.identity = identity
        self.networkingMode = networkingMode
    }

    func start(sink: @escaping (NearbyDiscoveryEvent) -> Void) {
        stop()

        lock.lock()
        generation += 1
        let activeGeneration = generation
        self.sink = sink
        lastPublishedDevices = nil
        lock.unlock()

        emit(.status(NearbyProviderStatus(
            source: source,
            availability: .starting,
            detail: .startingWifiAware
        )), generation: activeGeneration)

        guard WACapabilities.supportedFeatures.contains(.wifiAware) else {
            emit(.status(NearbyProviderStatus(
                source: source,
                availability: .unsupported,
                detail: .wifiAwareUnsupported
            )), generation: activeGeneration)
            return
        }
        guard WAPublishableService.allServices[envoixWifiAwareService] != nil,
              WASubscribableService.allServices[envoixWifiAwareService] != nil else {
            emit(.status(NearbyProviderStatus(
                source: source,
                availability: .error,
                detail: .wifiAwareServiceMissing
            )), generation: activeGeneration)
            return
        }

        if #available(iOS 26.4, *) {
            let session = AppleWifiAwareRendezvousSession(
                identity: identity,
                networkingMode: networkingMode,
                emit: { [weak self] event in
                    self?.emit(event, generation: activeGeneration)
                },
                updateRoutes: { [weak self] routes in
                    self?.replaceRendezvousRoutes(
                        routes,
                        generation: activeGeneration
                    )
                },
                admitOffer: { [weak self] offer in
                    await self?.admitRendezvousOffer(
                        offer,
                        generation: activeGeneration
                    ) ?? false
                }
            )
            lock.lock()
            if generation == activeGeneration, self.sink != nil {
                let predecessor = rendezvousLifecycleTask
                rendezvousSession = session
                let lifecycleTask = Task { [weak self] in
                    _ = await predecessor?.result
                    guard !Task.isCancelled,
                          self?.isCurrentRendezvousSession(
                              session,
                              generation: activeGeneration
                          ) == true else {
                        return
                    }
                    await session.start()
                    if Task.isCancelled
                        || self?.isCurrentRendezvousSession(
                            session,
                            generation: activeGeneration
                        ) != true {
                        await session.stop()
                    }
                }
                rendezvousLifecycleTask = lifecycleTask
                lock.unlock()
            } else {
                lock.unlock()
            }
        }

        let task = Task { [weak self] in
            guard let self else { return }
            await self.observePairedDevices(generation: activeGeneration)
        }
        lock.lock()
        if generation == activeGeneration, self.sink != nil {
            observationTask = task
            lock.unlock()
        } else {
            lock.unlock()
            task.cancel()
        }
    }

    func stop() {
        lock.lock()
        generation += 1
        let task = observationTask
        let session = rendezvousSession
        let lifecycleTask = rendezvousLifecycleTask
        let outboundTask = outboundTask
        let outboundCompletion = outboundCompletion
        task?.cancel()
        lifecycleTask?.cancel()
        outboundTask?.cancel()
        observationTask = nil
        rendezvousSession = nil
        rendezvousRoutes = [:]
        outboundID = nil
        self.outboundTask = nil
        self.outboundCompletion = nil
        sink = nil
        lastPublishedDevices = nil
        pairedDeviceRevision = 0
        rendezvousLifecycleTask = Task {
            _ = await lifecycleTask?.result
            _ = await outboundTask?.result
            if let session {
                await session.stop()
            }
            _ = await task?.result
        }
        lock.unlock()
        outboundCompletion?("Wi-Fi Aware discovery stopped")
    }

    func setAdvertisingEnabled(_: Bool) {
        lock.lock()
        let isStopped = sink == nil
        lock.unlock()
        precondition(isStopped, "Advertising policy must be configured before discovery starts")
        // Kept for provider interface compatibility. Wi-Fi Aware networking is
        // scoped to OS-paired devices and does not use public visibility.
    }

    func setIdentity(_ identity: LocalNearbyDiscoveryIdentity) {
        lock.lock()
        let isStopped = sink == nil
        if isStopped {
            self.identity = identity
        }
        lock.unlock()
        precondition(isStopped, "Identity must be configured before discovery starts")
    }

    func waitUntilStopped() async {
        let task = currentRendezvousLifecycleTask()
        _ = await task?.result
    }

    private func currentRendezvousLifecycleTask() -> Task<Void, Never>? {
        lock.lock()
        let task = rendezvousLifecycleTask
        lock.unlock()
        return task
    }

    func setRendezvousOfferAdmission(
        _ admission: @escaping @MainActor (NearbyRendezvousOffer) -> Bool
    ) {
        lock.lock()
        precondition(sink == nil, "Rendezvous admission must be configured before discovery starts")
        rendezvousOfferAdmission = admission
        lock.unlock()
    }

    func canOfferInvite(to selection: NearbyPairingSelection) -> Bool {
        guard let deviceID = Self.normalizeDeviceID(selection.nearbyWifiAwareDeviceID),
              let peerKey = NearbyDiscoveryPeerRegistry.normalizePeerKey(
                  selection.discoveryPeerKey
              ) else {
            return false
        }
        lock.lock()
        let available = sink != nil && rendezvousRoutes[peerKey] == deviceID
        lock.unlock()
        return available
    }

    func offerInvite(
        to selection: NearbyPairingSelection,
        invite: String,
        completion: @escaping (String?) -> Void
    ) {
        guard let deviceID = Self.normalizeDeviceID(selection.nearbyWifiAwareDeviceID),
              let peerKey = NearbyDiscoveryPeerRegistry.normalizePeerKey(
                  selection.discoveryPeerKey
              ),
              BleRendezvousProtocol.isSupportedInvite(invite.trimmed) else {
            completion("The Wi-Fi Aware invitation is invalid")
            return
        }

        lock.lock()
        guard let session = rendezvousSession,
              sink != nil,
              rendezvousRoutes[peerKey] == deviceID else {
            lock.unlock()
            completion("The selected device is no longer available over Wi-Fi Aware")
            return
        }
        guard outboundID == nil else {
            lock.unlock()
            completion("Another Wi-Fi Aware invitation is already being delivered")
            return
        }
        let activeGeneration = generation
        let operationID = UUID()
        outboundID = operationID
        outboundCompletion = completion
        let task = Task { [weak self] in
            let error = await session.offerInvite(
                peerKey: peerKey,
                sourceScopedDeviceID: deviceID,
                invite: invite.trimmed
            )
            self?.finishOutbound(
                id: operationID,
                generation: activeGeneration,
                error: error
            )
        }
        outboundTask = task
        lock.unlock()
    }

    private func observePairedDevices(generation: Int) async {
        while !Task.isCancelled {
            do {
                let sequence = WAPairedDevice.allDevices
                do {
                    if let devices = try await sequence.current() {
                        publish(devices, generation: generation)
                    } else {
                        emitTemporarilyUnavailable(generation: generation)
                    }
                } catch let error as WAError where error.isNoPairedDevices {
                    publish([:], generation: generation)
                }

                for try await devices in sequence {
                    try Task.checkCancellation()
                    publish(devices, generation: generation)
                }
            } catch is CancellationError {
                return
            } catch let error as WAError {
                if !handle(error, generation: generation) {
                    return
                }
            } catch {
                emitTemporarilyUnavailable(generation: generation)
            }

            do {
                try await Task<Never, Never>.sleep(for: Self.retryDelay)
            } catch {
                return
            }
        }
    }

    private func publish(_ devices: WAPairedDevice.Devices, generation: Int) {
        let projected = devices.values.compactMap(Self.project).sorted { $0.id < $1.id }
        guard projected.count <= NearbyPairedDevice.maximumSnapshotCount else {
            emitStatus(.error, .wifiAwarePairedDeviceLimitExceeded, generation: generation)
            return
        }

        lock.lock()
        guard self.generation == generation, sink != nil else {
            lock.unlock()
            return
        }
        pairedDeviceRevision &+= 1
        let revision = pairedDeviceRevision
        let session = rendezvousSession
        let changed = projected != lastPublishedDevices
        if changed {
            lastPublishedDevices = projected
        }
        lock.unlock()

        if let session {
            Task {
                await session.replacePairedDevices(devices, revision: revision)
            }
        }
        guard changed else { return }

        let availability: NearbyProviderAvailability
        let detail: NearbyProviderDetail
        if projected.isEmpty {
            availability = .pairingRequired
            detail = .wifiAwarePairingRequired
        } else if WifiAwareRendezvousRuntimePolicy.authenticatedControlPlaneSupported {
            availability = .paired
            detail = .wifiAwarePairedDevices(projected.count)
        } else {
            availability = .temporarilyUnavailable
            detail = .wifiAwareTemporarilyUnavailable
        }
        emit(.pairedDevices(source: source, devices: projected), generation: generation)
        emit(.status(NearbyProviderStatus(
            source: source,
            availability: availability,
            detail: detail
        )), generation: generation)
        logger.info("PAIRING provider=wifi_aware paired_device_count=\(projected.count, privacy: .public)")
    }

    private func handle(_ error: WAError, generation: Int) -> Bool {
        switch error {
        case .wifiAwareUnsupported:
            emitStatus(.unsupported, .wifiAwareUnsupported, generation: generation)
            return false
        case .entitlementMissing:
            emitStatus(.error, .wifiAwareEntitlementMissing, generation: generation)
            return false
        case .serviceNotDeclared:
            emitStatus(.error, .wifiAwareServiceMissing, generation: generation)
            return false
        case .noPairedDevices:
            publish([:], generation: generation)
            return true
        default:
            emitTemporarilyUnavailable(generation: generation)
            return true
        }
    }

    private func emitTemporarilyUnavailable(generation: Int) {
        emitStatus(
            .temporarilyUnavailable,
            .wifiAwareTemporarilyUnavailable,
            generation: generation
        )
    }

    private func emitStatus(
        _ availability: NearbyProviderAvailability,
        _ detail: NearbyProviderDetail,
        generation: Int
    ) {
        emit(.status(NearbyProviderStatus(
            source: source,
            availability: availability,
            detail: detail
        )), generation: generation)
    }

    private func emit(_ event: NearbyDiscoveryEvent, generation: Int) {
        lock.lock()
        let activeSink = self.generation == generation ? sink : nil
        lock.unlock()
        activeSink?(event)
    }

    private func isCurrentRendezvousSession(
        _ session: AppleWifiAwareRendezvousSession,
        generation: Int
    ) -> Bool {
        lock.lock()
        let isCurrent = self.generation == generation
            && sink != nil
            && rendezvousSession === session
        lock.unlock()
        return isCurrent
    }

    private func replaceRendezvousRoutes(
        _ routes: [String: String],
        generation: Int
    ) {
        var accepted: [String: String] = [:]
        var claimedDeviceIDs = Set<String>()
        for (rawPeerKey, rawDeviceID) in routes {
            guard let peerKey = NearbyDiscoveryPeerRegistry.normalizePeerKey(rawPeerKey),
                  let deviceID = Self.normalizeDeviceID(rawDeviceID),
                  peerKey != identity.peerKey,
                  claimedDeviceIDs.insert(deviceID).inserted else {
                continue
            }
            accepted[peerKey] = deviceID
        }

        lock.lock()
        guard self.generation == generation, sink != nil else {
            lock.unlock()
            return
        }
        rendezvousRoutes = accepted
        lock.unlock()
    }

    private func admitRendezvousOffer(
        _ offer: NearbyRendezvousOffer,
        generation: Int
    ) async -> Bool {
        let admission = lock.withLock {
            self.generation == generation && sink != nil
                ? rendezvousOfferAdmission
                : nil
        }
        guard let admission, await admission(offer) else { return false }

        return lock.withLock {
            self.generation == generation && sink != nil
        }
    }

    private func finishOutbound(
        id: UUID,
        generation: Int,
        error: String?
    ) {
        lock.lock()
        guard outboundID == id else {
            lock.unlock()
            return
        }
        outboundID = nil
        outboundTask = nil
        let completion = outboundCompletion
        outboundCompletion = nil
        let stillActive = self.generation == generation && sink != nil
        lock.unlock()
        completion?(stillActive ? error : "Wi-Fi Aware discovery stopped")
    }

    private static func normalizeDeviceID(_ value: String?) -> String? {
        guard let value else { return nil }
        let normalized = value.trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
        guard normalized.count == 16,
              let deviceID = UInt64(normalized, radix: 16) else {
            return nil
        }
        return String(format: "%016llx", deviceID)
    }

    private static func project(_ device: WAPairedDevice) -> NearbyPairedDevice? {
        let pairingInfo = device.pairingInfo
        let displayName = device.name ?? pairingInfo?.pairingName
        let model = [pairingInfo?.vendorName, pairingInfo?.modelName]
            .compactMap { NearbyDiscoveryPeerRegistry.sanitizeDeviceDetail($0) }
            .filter { !$0.isEmpty }
            .joined(separator: " · ")
        return NearbyPairedDevice(
            sourceScopedID: String(format: "%016llx", device.id),
            source: .wifiAware,
            displayName: displayName,
            model: model.isEmpty ? nil : model
        )
    }
}

@available(iOS 26.0, *)
private actor AppleWifiAwareRendezvousSession {
    private typealias EndpointAttemptState =
        WifiAwareEndpointAttemptState<WAEndpoint>

    private enum ConnectionBootstrapRole {
        case initiator
        case acceptor
    }

    private struct InboundRequestKey: Hashable {
        let deviceID: UInt64
        let requestID: UInt64
    }

    private struct RememberedInboundRequest {
        let invite: String
        let observedAtMilliseconds: Int64
    }

    private static let retryDelay: Duration = .seconds(1)
    private static let outboundCoveragePollInterval: Duration = .milliseconds(100)
    private static let observationRefreshInterval: Duration = .seconds(5)
    private static let heartbeatInterval: Duration = .seconds(10)
    private static let roleRecoveryDelay: Duration = .seconds(12)
    private static let pendingChannelPromotionTimeout: Duration = .seconds(30)
    private static let connectionReadyTimeout: Duration = .seconds(10)
    private static let connectionReadyPollInterval: Duration = .milliseconds(50)
    private static let bootstrapRetryInterval: Duration = .milliseconds(500)
    private static let inboundDeduplicationMilliseconds: Int64 = 60_000
    private static let maximumRememberedInboundRequests = 128
    private static let sharedSecretProtocolName = "envoix-nearby-v1"
    private static let connectionBootstrapDatagram = Data(
        "envoix-wifi-aware-control-hello-v1".utf8
    )
    private static let connectionBootstrapAcknowledgement = Data(
        "envoix-wifi-aware-control-ready-v1".utf8
    )

    private let identity: LocalNearbyDiscoveryIdentity
    private let networkingMode: AppleWifiAwareRendezvousNetworkingMode
    private let emit: @Sendable (NearbyDiscoveryEvent) -> Void
    private let updateRoutes: @Sendable ([String: String]) -> Void
    private let admitOffer: @Sendable (NearbyRendezvousOffer) async -> Bool
    private let logger = Logger(
        subsystem: Bundle.main.bundleIdentifier ?? "com.envoix.app",
        category: "wifi-aware-rendezvous"
    )

    private var active = false
    private var networkGeneration = 0
    private var latestPairedDeviceRevision: UInt64 = 0
    private var pairedDevices: WAPairedDevice.Devices = [:]
    private var endpointAttemptStates: [UInt64: EndpointAttemptState] = [:]
    private var remoteIdentities: [UInt64: LocalNearbyDiscoveryIdentity] = [:]
    private var conflictedDeviceIDs = Set<UInt64>()
    private var publishedRoutes: [String: String] = [:]
    private var browserTask: Task<Void, Never>?
    private var listenerTask: Task<Void, Never>?
    private var browserDeviceIDs = Set<UInt64>()
    private var listenerDeviceIDs = Set<UInt64>()
    private var refreshTask: Task<Void, Never>?
    private var networkPlanReconciliationTask: Task<Void, Never>?
    private var networkPlanReconciliationID: UUID?
    private var serviceLease: AppleWifiAwareServiceCoordinator.Lease?
    private var serviceLeaseRequestID: UUID?
    private var identityTasks: [UInt64: Task<Void, Never>] = [:]
    private var controlRoleStates: [UInt64: AppleWifiAwareControlRoleState] = [:]
    private var roleRecoveryTasks: [UInt64: Task<Void, Never>] = [:]
    private var controlChannels = AppleWifiAwareControlChannelRegistry<
        AppleWifiAwareRendezvousChannel
    >()
    private var pendingControlChannels: [UUID: AppleWifiAwareRendezvousChannel] = [:]
    private var heartbeatTasks: [UUID: Task<Void, Never>] = [:]
    private var rememberedInboundRequests: [
        InboundRequestKey: RememberedInboundRequest
    ] = [:]
    private var inboundConnectionAdmission =
        WifiAwareInboundConnectionAdmission()

    init(
        identity: LocalNearbyDiscoveryIdentity,
        networkingMode: AppleWifiAwareRendezvousNetworkingMode,
        emit: @escaping @Sendable (NearbyDiscoveryEvent) -> Void,
        updateRoutes: @escaping @Sendable ([String: String]) -> Void,
        admitOffer: @escaping @Sendable (NearbyRendezvousOffer) async -> Bool
    ) {
        self.identity = identity
        self.networkingMode = networkingMode
        self.emit = emit
        self.updateRoutes = updateRoutes
        self.admitOffer = admitOffer
    }

    func start() async {
        guard !active else { return }
        active = true
        networkGeneration += 1
        let requestID = UUID()
        serviceLeaseRequestID = requestID
        let lease: AppleWifiAwareServiceCoordinator.Lease
        do {
            lease = try await AppleWifiAwareServiceCoordinator.shared.acquire(
                .control
            )
        } catch {
            if serviceLeaseRequestID == requestID {
                serviceLeaseRequestID = nil
                active = false
            }
            logger.error(
                "RENDEZVOUS provider=wifi_aware state=stopped reason=lease_acquire_failed"
            )
            return
        }
        guard active, serviceLeaseRequestID == requestID else {
            await AppleWifiAwareServiceCoordinator.shared.release(lease)
            return
        }
        serviceLeaseRequestID = nil
        serviceLease = lease
        startRefreshLoop()
        startNetworking(generation: networkGeneration)
    }

    func stop() async {
        guard active else { return }
        active = false
        networkGeneration += 1
        serviceLeaseRequestID = nil
        refreshTask?.cancel()
        refreshTask = nil
        await cancelNetworkPlanReconciliation()
        await cancelRoleRecoveryTasks()
        await stopControlChannels()
        await cancelIdentityTasks()
        await cancelNetworkTasks()
        pairedDevices = [:]
        endpointAttemptStates = [:]
        remoteIdentities = [:]
        conflictedDeviceIDs = []
        controlRoleStates = [:]
        rememberedInboundRequests = [:]
        publishRoutesAndObservations(emitObservations: false)
        if let serviceLease {
            self.serviceLease = nil
            await AppleWifiAwareServiceCoordinator.shared.release(serviceLease)
        }
    }

    func replacePairedDevices(
        _ devices: WAPairedDevice.Devices,
        revision: UInt64
    ) async {
        guard revision > latestPairedDeviceRevision else { return }
        latestPairedDeviceRevision = revision
        let previousDevices = pairedDevices
        let nextIDs = Set(devices.keys)
        pairedDevices = devices
        guard previousDevices != devices else { return }

        networkGeneration += 1
        let replacementGeneration = networkGeneration
        await cancelNetworkPlanReconciliation()
        AppleWifiAwareControlRoleStore.shared.retain(deviceIDs: nextIDs)
        await cancelRoleRecoveryTasks()
        controlRoleStates = Dictionary(uniqueKeysWithValues: nextIDs.map {
            deviceID in
            let existing = controlRoleStates[deviceID]
            return (
                deviceID,
                existing ?? AppleWifiAwareControlRoleState(
                    persistedRole: AppleWifiAwareControlRoleStore.shared.role(
                        for: deviceID
                    )
                )
            )
        })
        await stopControlChannels()
        await cancelIdentityTasks()
        guard networkGeneration == replacementGeneration else { return }
        endpointAttemptStates = endpointAttemptStates.filter {
            nextIDs.contains($0.key)
        }
        for deviceID in Array(endpointAttemptStates.keys) {
            endpointAttemptStates[deviceID]?.updateEndpoint(nil)
        }
        remoteIdentities = remoteIdentities.filter { nextIDs.contains($0.key) }
        conflictedDeviceIDs = conflictedDeviceIDs.intersection(nextIDs)
        publishRoutesAndObservations(emitObservations: false)
        await cancelNetworkTasks()
        guard active, networkGeneration == replacementGeneration else { return }
        startNetworking(generation: replacementGeneration)
    }

    func offerInvite(
        peerKey: String,
        sourceScopedDeviceID: String,
        invite: String
    ) async -> String? {
        guard active,
              let deviceID = Self.parseDeviceID(sourceScopedDeviceID),
              let remoteIdentity = remoteIdentities[deviceID],
              remoteIdentity.peerKey == peerKey,
              isIdentityUnique(remoteIdentity, for: deviceID),
              publishedRoutes[peerKey] == Self.formatDeviceID(deviceID),
              let channelEntry = selectedControlChannel(for: deviceID),
              channelEntry.remoteIdentity.peerKey == peerKey else {
            return "The selected device is no longer available over Wi-Fi Aware"
        }
        let generation = networkGeneration

        do {
            try await channelEntry.value.sendInvite(
                invite,
                localIdentity: identity,
                expectedPeerKey: remoteIdentity.peerKey
            )
            guard active,
                  generation == networkGeneration,
                  publishedRoutes[peerKey] == Self.formatDeviceID(deviceID) else {
                return "The selected Wi-Fi Aware route changed during delivery"
            }
            return nil
        } catch is CancellationError {
            return "Wi-Fi Aware invitation delivery was cancelled"
        } catch let error as AppleWifiAwareRendezvousError {
            return error.errorDescription
        } catch let error as AppleWifiAwareRendezvousChannelError {
            return error.errorDescription
        } catch {
            return "Wi-Fi Aware invitation delivery failed"
        }
    }

    private func startRefreshLoop() {
        refreshTask?.cancel()
        refreshTask = Task { [weak self] in
            while !Task.isCancelled {
                guard let self else { return }
                await self.publishRoutesAndObservations(emitObservations: true)
                do {
                    try await Task<Never, Never>.sleep(
                        for: Self.observationRefreshInterval
                    )
                } catch {
                    return
                }
            }
        }
    }

    private func startNetworking(generation: Int) {
        guard active, serviceLease != nil, !pairedDevices.isEmpty else {
            endpointAttemptStates = [:]
            remoteIdentities = [:]
            conflictedDeviceIDs = []
            browserDeviceIDs = []
            listenerDeviceIDs = []
            publishRoutesAndObservations(emitObservations: false)
            return
        }
        ensureControlRoleStates()
        let plan = desiredNetworkingDevices()
        startBrowser(devices: plan.browser, generation: generation)
        // This listener is restricted to OS-paired devices. Public Nearby
        // visibility continues to gate BLE and Bonjour advertising only.
        startListener(devices: plan.listener, generation: generation)
        refreshRoleRecoveryTasks(generation: generation)
    }

    private func ensureControlRoleStates() {
        let activeDeviceIDs = Set(pairedDevices.keys)
        controlRoleStates = controlRoleStates.filter {
            activeDeviceIDs.contains($0.key)
        }
        for deviceID in activeDeviceIDs where controlRoleStates[deviceID] == nil {
            controlRoleStates[deviceID] = AppleWifiAwareControlRoleState(
                persistedRole: AppleWifiAwareControlRoleStore.shared.role(
                    for: deviceID
                )
            )
        }
    }

    private func desiredNetworkingDevices() -> (
        browser: WAPairedDevice.Devices,
        listener: WAPairedDevice.Devices
    ) {
        switch networkingMode {
        case .publisherOnly:
            return ([:], pairedDevices)
        case .subscriberOnly:
            return (pairedDevices, [:])
        case .automatic:
            let browser = pairedDevices.filter { deviceID, _ in
                controlRoleStates[deviceID]?.startsBrowser ?? true
            }
            let listener = pairedDevices.filter { deviceID, _ in
                controlRoleStates[deviceID]?.startsListener ?? true
            }
            return (browser, listener)
        }
    }

    private func startBrowser(
        devices: WAPairedDevice.Devices,
        generation: Int
    ) {
        browserDeviceIDs = Set(devices.keys)
        guard !devices.isEmpty else {
            browserTask = nil
            return
        }
        browserTask = Task { [weak self] in
            await self?.runBrowserLoop(
                devices: devices,
                generation: generation
            )
        }
    }

    private func startListener(
        devices: WAPairedDevice.Devices,
        generation: Int
    ) {
        listenerDeviceIDs = Set(devices.keys)
        guard !devices.isEmpty else {
            listenerTask = nil
            return
        }
        listenerTask = Task { [weak self] in
            await self?.runListenerLoop(
                devices: devices,
                generation: generation
            )
        }
    }

    private func scheduleNetworkPlanReconciliation(generation: Int) {
        let predecessor = networkPlanReconciliationTask
        predecessor?.cancel()
        let reconciliationID = UUID()
        networkPlanReconciliationID = reconciliationID
        networkPlanReconciliationTask = Task { [weak self] in
            _ = await predecessor?.result
            guard !Task.isCancelled else { return }
            await self?.reconcileNetworkingPlan(
                generation: generation,
                reconciliationID: reconciliationID
            )
            await self?.finishNetworkPlanReconciliation(
                reconciliationID: reconciliationID
            )
        }
    }

    private func finishNetworkPlanReconciliation(
        reconciliationID: UUID
    ) {
        guard networkPlanReconciliationID == reconciliationID else { return }
        networkPlanReconciliationTask = nil
        networkPlanReconciliationID = nil
    }

    private func cancelNetworkPlanReconciliation() async {
        let task = networkPlanReconciliationTask
        networkPlanReconciliationTask = nil
        networkPlanReconciliationID = nil
        task?.cancel()
        _ = await task?.result
    }

    private func reconcileNetworkingPlan(
        generation: Int,
        reconciliationID: UUID
    ) async {
        guard isCurrentReconciliation(
            generation: generation,
            reconciliationID: reconciliationID
        ) else {
            return
        }
        ensureControlRoleStates()
        let plan = desiredNetworkingDevices()
        let desiredBrowserIDs = Set(plan.browser.keys)
        let desiredListenerIDs = Set(plan.listener.keys)

        if desiredBrowserIDs != browserDeviceIDs {
            let previousBrowserIDs = browserDeviceIDs
            let previousTask = browserTask
            browserTask = nil
            browserDeviceIDs = []
            previousTask?.cancel()
            _ = await previousTask?.result
            guard isCurrentReconciliation(
                generation: generation,
                reconciliationID: reconciliationID
            ) else {
                return
            }
            await cancelIdentityTasks(
                for: previousBrowserIDs.subtracting(desiredBrowserIDs)
            )
            guard isCurrentReconciliation(
                generation: generation,
                reconciliationID: reconciliationID
            ) else {
                return
            }
            startBrowser(devices: plan.browser, generation: generation)
        }

        if desiredListenerIDs != listenerDeviceIDs {
            let previousTask = listenerTask
            listenerTask = nil
            listenerDeviceIDs = []
            previousTask?.cancel()
            _ = await previousTask?.result
            guard isCurrentReconciliation(
                generation: generation,
                reconciliationID: reconciliationID
            ) else {
                return
            }
            startListener(devices: plan.listener, generation: generation)
        }
        refreshRoleRecoveryTasks(generation: generation)
    }

    private func isCurrentReconciliation(
        generation: Int,
        reconciliationID: UUID
    ) -> Bool {
        isCurrent(generation)
            && networkPlanReconciliationID == reconciliationID
            && !Task.isCancelled
    }

    private func refreshRoleRecoveryTasks(generation: Int) {
        guard networkingMode == .automatic, isCurrent(generation) else {
            for task in roleRecoveryTasks.values {
                task.cancel()
            }
            roleRecoveryTasks = [:]
            return
        }
        let eligibleDeviceIDs = Set(pairedDevices.keys.filter { deviceID in
            controlRoleStates[deviceID]?.role != nil
                && !controlChannels.contains(deviceID: deviceID)
        })
        for deviceID in Array(roleRecoveryTasks.keys)
        where !eligibleDeviceIDs.contains(deviceID) {
            roleRecoveryTasks.removeValue(forKey: deviceID)?.cancel()
        }
        for deviceID in eligibleDeviceIDs
        where roleRecoveryTasks[deviceID] == nil {
            roleRecoveryTasks[deviceID] = Task { [weak self] in
                do {
                    try await Task<Never, Never>.sleep(
                        for: Self.roleRecoveryDelay
                    )
                } catch {
                    return
                }
                await self?.beginRoleRecovery(
                    for: deviceID,
                    generation: generation
                )
            }
        }
    }

    private func beginRoleRecovery(
        for deviceID: UInt64,
        generation: Int
    ) {
        roleRecoveryTasks.removeValue(forKey: deviceID)
        guard isCurrent(generation), pairedDevices[deviceID] != nil,
              !controlChannels.contains(deviceID: deviceID),
              var state = controlRoleStates[deviceID],
              state.beginRecovery(hasReadyChannel: false) else {
            return
        }
        controlRoleStates[deviceID] = state
        AppleWifiAwareControlRoleStore.shared.remove(for: deviceID)
        logger.info(
            "RENDEZVOUS provider=wifi_aware role=recovery reason=ready_timeout"
        )
        scheduleNetworkPlanReconciliation(generation: generation)
    }

    private func cancelRoleRecoveryTask(for deviceID: UInt64) {
        roleRecoveryTasks.removeValue(forKey: deviceID)?.cancel()
    }

    private func cancelRoleRecoveryTasks() async {
        let tasks = Array(roleRecoveryTasks.values)
        roleRecoveryTasks = [:]
        for task in tasks {
            task.cancel()
        }
        for task in tasks {
            _ = await task.result
        }
    }

    private func cancelNetworkTasks() async {
        let browserTask = self.browserTask
        let listenerTask = self.listenerTask
        self.browserTask = nil
        self.listenerTask = nil
        browserDeviceIDs = []
        listenerDeviceIDs = []
        browserTask?.cancel()
        listenerTask?.cancel()
        _ = await browserTask?.result
        _ = await listenerTask?.result
    }

    private func cancelIdentityTasks(for deviceIDs: Set<UInt64>) async {
        let tasks = deviceIDs.compactMap { deviceID -> Task<Void, Never>? in
            endpointAttemptStates[deviceID]?.cancelAttempt()
            return identityTasks.removeValue(forKey: deviceID)
        }
        for task in tasks {
            task.cancel()
        }
        for task in tasks {
            _ = await task.result
        }
    }

    private func cancelIdentityTasks() async {
        let tasks = Array(identityTasks.values)
        identityTasks = [:]
        for deviceID in Array(endpointAttemptStates.keys) {
            endpointAttemptStates[deviceID]?.cancelAttempt()
        }
        for task in tasks {
            task.cancel()
        }
        for task in tasks {
            _ = await task.result
        }
    }

    private func runBrowserLoop(
        devices: WAPairedDevice.Devices,
        generation: Int
    ) async {
        guard let service = WASubscribableService
            .allServices[envoixWifiAwareService] else {
            return
        }
        while isCurrent(generation), !Task.isCancelled {
            let logger = self.logger
            let browser = NetworkBrowser(
                for: WASubscriberBrowser.wifiAware(
                    .connecting(
                        to: .selected(Array(devices.values)),
                        from: service
                    )
                )
            )
            .onStateUpdate { _, state in
                let detail = Self.describeBrowserState(state)
                logger.info(
                    "RENDEZVOUS provider=wifi_aware browser_state=\(detail, privacy: .public)"
                )
            }
            do {
                // End the subscriber browse before opening data paths. On
                // current iOS, a connection created by the active browser's
                // callback remains in setup instead of joining the NAN path.
                let discoveredEndpoints: [WAEndpoint] = try await browser.run {
                    endpoints in
                    let containsPairedDevice = endpoints.contains {
                        devices[$0.device.id] != nil
                    }
                    return containsPairedDevice
                        ? .finish(endpoints)
                        : .continue
                }
                await accept(discoveredEndpoints, generation: generation)
                await waitUntilOutboundDiscoveryIsNeeded(
                    for: Set(devices.keys),
                    generation: generation
                )
            } catch is CancellationError {
                return
            } catch {
                await browserFailed(error: error, generation: generation)
            }

            guard isCurrent(generation), !Task.isCancelled else { return }
            do {
                try await Task<Never, Never>.sleep(for: Self.retryDelay)
            } catch {
                return
            }
        }
    }

    private func waitUntilOutboundDiscoveryIsNeeded(
        for deviceIDs: Set<UInt64>,
        generation: Int
    ) async {
        while isCurrent(generation), !Task.isCancelled {
            let requiredDeviceIDs = deviceIDs.filter { deviceID in
                shouldMaintainOutboundChannel(for: deviceID)
                    && !conflictedDeviceIDs.contains(deviceID)
            }
            guard !requiredDeviceIDs.isEmpty else { return }
            let missingDeviceRequiresDiscovery = requiredDeviceIDs.contains {
                deviceID in
                !hasControlChannel(
                    for: deviceID,
                    direction: .outboundSubscriber
                ) && identityTasks[deviceID] == nil
            }
            guard !missingDeviceRequiresDiscovery else { return }
            do {
                try await Task<Never, Never>.sleep(
                    for: Self.outboundCoveragePollInterval
                )
            } catch {
                return
            }
        }
    }

    private func runListenerLoop(
        devices: WAPairedDevice.Devices,
        generation: Int
    ) async {
        guard let service = WAPublishableService
            .allServices[envoixWifiAwareService] else {
            return
        }
        while isCurrent(generation), !Task.isCancelled {
            do {
                let logger = self.logger
                let listener: NetworkListener<UDP> = try NetworkListener(
                    for: .wifiAware(
                        .connecting(
                            to: service,
                            from: .selected(Array(devices.values))
                        )
                    ),
                    using: envoixWifiAwareUDPParameters()
                )
                .onStateUpdate { _, state in
                    let detail = Self.describeListenerState(state)
                    logger.info(
                        "RENDEZVOUS provider=wifi_aware listener_state=\(detail, privacy: .public)"
                    )
                }
                try await listener.run { [weak self] connection in
                    await self?.handleIncoming(
                        connection,
                        generation: generation
                    )
                }
            } catch is CancellationError {
                return
            } catch {
                let detail = Self.describeError(error)
                logger.error(
                    "RENDEZVOUS provider=wifi_aware listener=failed error=\(detail, privacy: .public)"
                )
            }

            guard isCurrent(generation), !Task.isCancelled else { return }
            do {
                try await Task<Never, Never>.sleep(for: Self.retryDelay)
            } catch {
                return
            }
        }
    }

    private func isCurrent(_ generation: Int) -> Bool {
        active && networkGeneration == generation
    }

    private func browserFailed(error: Error, generation: Int) async {
        guard isCurrent(generation) else { return }
        for deviceID in Array(endpointAttemptStates.keys) {
            endpointAttemptStates[deviceID]?.updateEndpoint(nil)
        }
        let detail = Self.describeError(error)
        logger.error(
            "RENDEZVOUS provider=wifi_aware browser=failed error=\(detail, privacy: .public)"
        )
    }

    private static func describeBrowserState(
        _ state: NetworkBrowser<WASubscriberBrowser>.State
    ) -> String {
        switch state {
        case .setup: "setup"
        case .waiting(let error): "waiting:\(describeError(error))"
        case .ready: "ready"
        case .failed(let error): "failed:\(describeError(error))"
        case .cancelled: "cancelled"
        @unknown default: "unknown"
        }
    }

    private static func describeListenerState(
        _ state: NetworkListener<UDP>.State
    ) -> String {
        switch state {
        case .setup: "setup"
        case .waiting(let error): "waiting:\(describeError(error))"
        case .ready: "ready"
        case .failed(let error): "failed:\(describeError(error))"
        case .cancelled: "cancelled"
        @unknown default: "unknown"
        }
    }

    private static func describeConnectionState(
        _ state: NetworkConnection<UDP>.State
    ) -> String {
        switch state {
        case .setup: "setup"
        case .preparing: "preparing"
        case .waiting(let error): "waiting:\(describeError(error))"
        case .ready: "ready"
        case .failed(let error): "failed:\(describeError(error))"
        case .cancelled: "cancelled"
        @unknown default: "unknown"
        }
    }

    private static func describeError(_ error: Error) -> String {
        if let error = error as? NWError {
            return error.wifiAware?.wireName ?? "network"
        }
        if let error = error as? WAError {
            return error.wireName
        }
        if let error = error as? AppleWifiAwareRendezvousError {
            return String(describing: error)
        }
        if let error = error as? AppleWifiAwareRendezvousChannelError {
            return String(describing: error)
        }
        return "other"
    }

    private func accept(
        _ discoveredEndpoints: [WAEndpoint],
        generation: Int
    ) async {
        guard isCurrent(generation) else { return }
        var accepted: [UInt64: WAEndpoint] = [:]
        for endpoint in discoveredEndpoints
        where pairedDevices[endpoint.device.id] != nil {
            accepted[endpoint.device.id] = endpoint
        }
        logger.info(
            "RENDEZVOUS provider=wifi_aware browse_update total=\(discoveredEndpoints.count, privacy: .public) matched=\(accepted.count, privacy: .public)"
        )
        for deviceID in pairedDevices.keys {
            var state = endpointAttemptStates[deviceID]
                ?? EndpointAttemptState()
            state.updateEndpoint(accepted[deviceID])
            endpointAttemptStates[deviceID] = state
        }

        for deviceID in accepted.keys
        where shouldMaintainOutboundChannel(for: deviceID)
            && !hasControlChannel(
                for: deviceID,
                direction: .outboundSubscriber
            )
            && !conflictedDeviceIDs.contains(deviceID)
            && endpointAttemptStates[deviceID]?.hasActiveAttempt == false
            && identityTasks[deviceID] == nil {
            startIdentityResolution(
                deviceID: deviceID,
                generation: generation
            )
        }
        publishRoutesAndObservations(emitObservations: true)
    }

    private func startIdentityResolution(
        deviceID: UInt64,
        generation: Int
    ) {
        guard var state = endpointAttemptStates[deviceID],
              let endpoint = state.currentEndpoint,
              let attemptToken = state.beginAttempt() else {
            return
        }
        endpointAttemptStates[deviceID] = state
        identityTasks[deviceID] = Task { [weak self] in
            guard let self else { return }
            await self.runOutboundControlConnection(
                endpoint: endpoint,
                deviceID: deviceID,
                generation: generation
            )
            await self.completeIdentityResolution(
                deviceID: deviceID,
                attemptToken: attemptToken
            )
        }
    }

    private func completeIdentityResolution(
        deviceID: UInt64,
        attemptToken: EndpointAttemptState.Token
    ) {
        guard var state = endpointAttemptStates[deviceID],
              state.finishAttempt(attemptToken) else {
            return
        }
        endpointAttemptStates[deviceID] = state
        identityTasks.removeValue(forKey: deviceID)
    }

    private func runOutboundControlConnection(
        endpoint: WAEndpoint,
        deviceID: UInt64,
        generation: Int
    ) async {
        guard #available(iOS 26.4, *), isCurrent(generation) else { return }
        var channel: AppleWifiAwareRendezvousChannel?
        var runTask: Task<Void, Error>?
        do {
            let logger = self.logger
            let connection: NetworkConnection<UDP> = NetworkConnection(
                to: endpoint,
                using: envoixWifiAwareUDPParameters()
            )
            .onStateUpdate { _, state in
                let detail = Self.describeConnectionState(state)
                logger.info(
                    "RENDEZVOUS provider=wifi_aware outbound_state=\(detail, privacy: .public)"
                )
            }
            let context = try await Self.authenticatedContext(
                for: connection,
                expectedDeviceID: deviceID,
                bootstrapRole: .initiator
            )
            logger.info(
                "RENDEZVOUS provider=wifi_aware outbound_stage=authenticated"
            )
            guard isCurrent(generation), pairedDevices[deviceID] != nil else {
                return
            }
            let channelID = UUID()
            let activeChannel = try AppleWifiAwareRendezvousChannel(
                connection: connection,
                derivedKey: context.key,
                localIdentity: identity,
                channelID: channelID,
                deviceID: deviceID,
                maximumFrameBytes: context.maximumFrameBytes,
                handleHello: { [weak self] message in
                    await self?.handleControlHello(
                        message,
                        deviceID: deviceID,
                        channelID: channelID,
                        direction: .outboundSubscriber,
                        generation: generation
                    ) ?? false
                },
                handleInvite: { [weak self] message in
                    await self?.handleControlInvite(
                        message,
                        deviceID: deviceID,
                        channelID: channelID,
                        generation: generation
                    ) ?? false
                }
            )
            channel = activeChannel
            pendingControlChannels[channelID] = activeChannel
            let activeRunTask = Task {
                try await activeChannel.run()
            }
            runTask = activeRunTask
            let remoteIdentity = try await activeChannel.identify(
                localIdentity: identity
            )
            logger.info(
                "RENDEZVOUS provider=wifi_aware outbound_stage=hello_ack"
            )
            guard await registerControlChannel(
                activeChannel,
                direction: .outboundSubscriber,
                remoteIdentity: remoteIdentity,
                generation: generation
            ) else {
                throw AppleWifiAwareRendezvousChannelError.peerIdentityChanged
            }
            try await activeRunTask.value
        } catch is CancellationError {
            // Normal during provider teardown or endpoint replacement.
        } catch {
            let detail = Self.describeError(error)
            logger.error(
                "RENDEZVOUS provider=wifi_aware outbound_stage=failed error=\(detail, privacy: .public)"
            )
        }
        if let channel {
            await channel.stop()
            runTask?.cancel()
            _ = await runTask?.result
            controlChannelClosed(
                deviceID: deviceID,
                channelID: channel.channelID,
                generation: generation
            )
        }
    }

    private func handleIncoming(
        _ connection: NetworkConnection<UDP>,
        generation: Int
    ) async {
        guard #available(iOS 26.4, *), isCurrent(generation) else { return }
        guard let admissionToken = inboundConnectionAdmission.acquire() else {
            logger.error("RENDEZVOUS provider=wifi_aware rejected=connection_limit")
            return
        }
        defer { inboundConnectionAdmission.release(admissionToken) }
        var channel: AppleWifiAwareRendezvousChannel?
        var promotionDeadlineTask: Task<Void, Never>?
        do {
            let logger = self.logger
            connection.onStateUpdate { _, state in
                let detail = Self.describeConnectionState(state)
                logger.info(
                    "RENDEZVOUS provider=wifi_aware inbound_state=\(detail, privacy: .public)"
                )
            }
            let context = try await Self.authenticatedContext(
                for: connection,
                expectedDeviceID: nil,
                bootstrapRole: .acceptor
            )
            logger.info(
                "RENDEZVOUS provider=wifi_aware inbound_stage=authenticated"
            )
            guard isCurrent(generation),
                  pairedDevices[context.deviceID] != nil else {
                return
            }
            let channelID = UUID()
            let activeChannel = try AppleWifiAwareRendezvousChannel(
                connection: connection,
                derivedKey: context.key,
                localIdentity: identity,
                channelID: channelID,
                deviceID: context.deviceID,
                maximumFrameBytes: context.maximumFrameBytes,
                bootstrapReplay: (
                    request: Self.connectionBootstrapDatagram,
                    response: Self.connectionBootstrapAcknowledgement
                ),
                handleHello: { [weak self] message in
                    await self?.handleControlHello(
                        message,
                        deviceID: context.deviceID,
                        channelID: channelID,
                        direction: .inboundPublisher,
                        generation: generation
                    ) ?? false
                },
                handleInvite: { [weak self] message in
                    await self?.handleControlInvite(
                        message,
                        deviceID: context.deviceID,
                        channelID: channelID,
                        generation: generation
                    ) ?? false
                }
            )
            channel = activeChannel
            pendingControlChannels[channelID] = activeChannel
            guard inboundConnectionAdmission.markPending(
                admissionToken,
                for: channelID
            ) else {
                pendingControlChannels.removeValue(forKey: channelID)
                logger.error(
                    "RENDEZVOUS provider=wifi_aware rejected=admission_state"
                )
                await activeChannel.stop()
                return
            }
            promotionDeadlineTask = Task { [weak self] in
                do {
                    try await Task<Never, Never>.sleep(
                        for: Self.pendingChannelPromotionTimeout
                    )
                } catch {
                    return
                }
                await self?.expirePendingControlChannel(
                    deviceID: context.deviceID,
                    channelID: channelID,
                    generation: generation
                )
            }
            try await activeChannel.run()
        } catch is CancellationError {
            // Normal during listener or provider teardown.
        } catch {
            let detail = Self.describeError(error)
            logger.error(
                "RENDEZVOUS provider=wifi_aware inbound_stage=failed error=\(detail, privacy: .public)"
            )
        }
        promotionDeadlineTask?.cancel()
        _ = await promotionDeadlineTask?.result
        if let channel {
            await channel.stop()
            controlChannelClosed(
                deviceID: channel.deviceID,
                channelID: channel.channelID,
                generation: generation
            )
        }
    }

    private func expirePendingControlChannel(
        deviceID: UInt64,
        channelID: UUID,
        generation: Int
    ) async {
        guard isCurrent(generation),
              registeredControlChannel(
                  deviceID: deviceID,
                  channelID: channelID
              ) == nil,
              let channel = pendingControlChannels.removeValue(
                  forKey: channelID
              ) else {
            return
        }
        releasePendingAdmission(for: channelID)
        logger.info(
            "RENDEZVOUS provider=wifi_aware channel=rejected reason=hello_timeout"
        )
        await channel.stop()
    }

    private func handleControlHello(
        _ message: WifiAwareRendezvousProtocol.Message,
        deviceID: UInt64,
        channelID: UUID,
        direction: AppleWifiAwareControlChannelDirection,
        generation: Int
    ) async -> Bool {
        guard message.type == .hello,
              isCurrent(generation),
              message.senderPeerKey != identity.peerKey else {
            return false
        }
        if let existing = registeredControlChannel(
            deviceID: deviceID,
            channelID: channelID
        ) {
            return existing.direction == direction
                && existing.remoteIdentity.peerKey == message.senderPeerKey
                && !conflictedDeviceIDs.contains(deviceID)
        }
        guard let channel = pendingControlChannels[channelID],
              channel.deviceID == deviceID else {
            return false
        }
        return await registerControlChannel(
            channel,
            direction: direction,
            remoteIdentity: message.senderIdentity,
            generation: generation
        )
    }

    private func handleControlInvite(
        _ message: WifiAwareRendezvousProtocol.Message,
        deviceID: UInt64,
        channelID: UUID,
        generation: Int
    ) async -> Bool {
        guard message.type == .invite,
              isCurrent(generation),
              let entry = registeredControlChannel(
                  deviceID: deviceID,
                  channelID: channelID
              ),
              entry.remoteIdentity.peerKey == message.senderPeerKey,
              !conflictedDeviceIDs.contains(deviceID),
              BleRendezvousProtocol.isSupportedInvite(message.content) else {
            return false
        }
        if let rememberedInviteMatches = rememberedInboundInviteMatches(
            deviceID: deviceID,
            requestID: message.requestID,
            invite: message.content
        ) {
            if !rememberedInviteMatches {
                logger.error(
                    "RENDEZVOUS provider=wifi_aware rejected=request_id_reuse"
                )
            }
            return rememberedInviteMatches
        }
        let offer = NearbyRendezvousOffer(
            requestID: String(format: "%016llx", message.requestID),
            senderPeerKey: entry.remoteIdentity.peerKey,
            senderDisplayName: entry.remoteIdentity.displayName,
            source: .wifiAware,
            senderInboxEndpointID: nil,
            senderWifiAwareDeviceID: Self.formatDeviceID(deviceID),
            invite: message.content
        )
        guard await admitOffer(offer), isCurrent(generation) else {
            logger.error("RENDEZVOUS provider=wifi_aware rejected=admission")
            return false
        }
        rememberInboundRequest(
            deviceID: deviceID,
            requestID: message.requestID,
            invite: message.content
        )
        return true
    }

    private func registerControlChannel(
        _ channel: AppleWifiAwareRendezvousChannel,
        direction: AppleWifiAwareControlChannelDirection,
        remoteIdentity: LocalNearbyDiscoveryIdentity,
        generation: Int
    ) async -> Bool {
        let deviceID = channel.deviceID
        if let existing = registeredControlChannel(
            deviceID: deviceID,
            channelID: channel.channelID
        ) {
            return existing.direction == direction
                && existing.remoteIdentity.peerKey == remoteIdentity.peerKey
                && !conflictedDeviceIDs.contains(deviceID)
        }
        guard isCurrent(generation),
              pairedDevices[deviceID] != nil,
              pendingControlChannels[channel.channelID] === channel else {
            return false
        }
        guard bind(remoteIdentity, to: deviceID),
              let normalizedIdentity = remoteIdentities[deviceID] else {
            await rejectPendingControlChannel(channel)
            return false
        }
        if networkingMode == .automatic {
            var roleState = controlRoleStates[deviceID]
                ?? AppleWifiAwareControlRoleState(
                    persistedRole: AppleWifiAwareControlRoleStore.shared.role(
                        for: deviceID
                    )
                )
            let authentication = roleState.authenticate(
                localPeerKey: identity.peerKey,
                remotePeerKey: normalizedIdentity.peerKey,
                direction: direction
            )
            controlRoleStates[deviceID] = roleState
            switch authentication {
            case .identityCollision:
                logger.error(
                    "RENDEZVOUS provider=wifi_aware channel=rejected reason=identity_collision"
                )
                await rejectPendingControlChannel(channel)
                return false
            case .roleMismatch(let role, let roleChanged):
                if roleChanged {
                    AppleWifiAwareControlRoleStore.shared.set(
                        role,
                        for: deviceID
                    )
                    if role == .subscriber,
                       direction == .inboundPublisher {
                        // A simultaneous bootstrap can leave this device's
                        // first outbound attempt waiting behind the peer's
                        // now-rejected connection. Restart that attempt after
                        // the authenticated identities select this device as
                        // the subscriber.
                        identityTasks[deviceID]?.cancel()
                    }
                    scheduleNetworkPlanReconciliation(generation: generation)
                }
                logger.info(
                    "RENDEZVOUS provider=wifi_aware channel=rejected reason=role_mismatch"
                )
                await rejectPendingControlChannel(channel)
                return false
            case .accepted(let role, let roleChanged):
                if roleChanged {
                    AppleWifiAwareControlRoleStore.shared.set(
                        role,
                        for: deviceID
                    )
                }
                cancelRoleRecoveryTask(for: deviceID)
                if roleChanged {
                    scheduleNetworkPlanReconciliation(generation: generation)
                }
            }
        }

        let entry = AppleWifiAwareControlChannelRegistry<
            AppleWifiAwareRendezvousChannel
        >.Entry(
            channelID: channel.channelID,
            deviceID: deviceID,
            direction: direction,
            remoteIdentity: normalizedIdentity,
            value: channel
        )
        let replaced = controlChannels.register(entry)
        pendingControlChannels.removeValue(forKey: channel.channelID)
        releasePendingAdmission(for: channel.channelID)
        startHeartbeat(for: entry, generation: generation)
        publishRoutesAndObservations(emitObservations: true)
        if let replaced {
            await retireReplacedControlChannel(replaced)
        }
        logger.info(
            "RENDEZVOUS provider=wifi_aware channel=ready direction=\(String(describing: direction), privacy: .public)"
        )
        return true
    }

    private func rejectPendingControlChannel(
        _ channel: AppleWifiAwareRendezvousChannel
    ) async {
        if pendingControlChannels[channel.channelID] === channel {
            pendingControlChannels.removeValue(forKey: channel.channelID)
        }
        releasePendingAdmission(for: channel.channelID)
        reconcileRemoteIdentity(for: channel.deviceID)
        publishRoutesAndObservations(emitObservations: false)
        await channel.stop()
    }

    private func startHeartbeat(
        for entry: AppleWifiAwareControlChannelRegistry<
            AppleWifiAwareRendezvousChannel
        >.Entry,
        generation: Int
    ) {
        heartbeatTasks.removeValue(forKey: entry.channelID)?.cancel()
        let localIdentity = identity
        heartbeatTasks[entry.channelID] = Task { [weak self] in
            while !Task.isCancelled {
                do {
                    try await Task<Never, Never>.sleep(
                        for: Self.heartbeatInterval
                    )
                    try await entry.value.heartbeat(
                        localIdentity: localIdentity,
                        expectedPeerKey: entry.remoteIdentity.peerKey
                    )
                } catch is CancellationError {
                    return
                } catch {
                    await self?.controlChannelHeartbeatFailed(
                        deviceID: entry.deviceID,
                        channelID: entry.channelID,
                        generation: generation
                    )
                    return
                }
            }
        }
    }

    private func controlChannelHeartbeatFailed(
        deviceID: UInt64,
        channelID: UUID,
        generation: Int
    ) async {
        guard isCurrent(generation),
              registeredControlChannel(
                  deviceID: deviceID,
                  channelID: channelID
              ) != nil else {
            return
        }
        logger.info(
            "RENDEZVOUS provider=wifi_aware channel=stale reason=heartbeat_failed"
        )
        await removeControlChannel(
            deviceID: deviceID,
            channelID: channelID,
            generation: generation,
            stop: true
        )
    }

    private func retireReplacedControlChannel(
        _ entry: AppleWifiAwareControlChannelRegistry<
            AppleWifiAwareRendezvousChannel
        >.Entry
    ) async {
        heartbeatTasks.removeValue(forKey: entry.channelID)?.cancel()
        await entry.value.stop()
    }

    private func removeControlChannel(
        deviceID: UInt64,
        channelID: UUID,
        generation: Int,
        stop: Bool
    ) async {
        pendingControlChannels.removeValue(forKey: channelID)
        releasePendingAdmission(for: channelID)
        guard let removed = controlChannels.remove(
            deviceID: deviceID,
            channelID: channelID
        ) else {
            return
        }
        heartbeatTasks.removeValue(forKey: channelID)?.cancel()
        controlChannelDidBecomeUnavailable(
            deviceID: deviceID,
            generation: generation
        )
        reconcileRemoteIdentity(for: deviceID)
        publishRoutesAndObservations(emitObservations: false)
        if stop {
            await removed.value.stop()
        }
    }

    private func controlChannelClosed(
        deviceID: UInt64,
        channelID: UUID,
        generation: Int
    ) {
        pendingControlChannels.removeValue(forKey: channelID)
        releasePendingAdmission(for: channelID)
        guard controlChannels.remove(
            deviceID: deviceID,
            channelID: channelID
        ) != nil else {
            return
        }
        heartbeatTasks.removeValue(forKey: channelID)?.cancel()
        controlChannelDidBecomeUnavailable(
            deviceID: deviceID,
            generation: generation
        )
        reconcileRemoteIdentity(for: deviceID)
        publishRoutesAndObservations(emitObservations: false)
    }

    private func controlChannelDidBecomeUnavailable(
        deviceID: UInt64,
        generation: Int
    ) {
        guard networkingMode == .automatic,
              isCurrent(generation),
              var state = controlRoleStates[deviceID] else {
            return
        }
        state.channelClosed(
            hasReadyChannel: controlChannels.contains(deviceID: deviceID)
        )
        controlRoleStates[deviceID] = state
        refreshRoleRecoveryTasks(generation: generation)
    }

    private func stopControlChannels() async {
        let entries = controlChannels.removeAll()
        let pending = Array(pendingControlChannels.values)
        pendingControlChannels = [:]
        // Also clear handshakes that have not authenticated far enough to be
        // associated with a channel ID. Their deferred releases are idempotent.
        inboundConnectionAdmission.reset()
        let heartbeatTasks = Array(self.heartbeatTasks.values)
        self.heartbeatTasks = [:]
        for task in heartbeatTasks {
            task.cancel()
        }

        var seen = Set<ObjectIdentifier>()
        let channels = (entries.map(\.value) + pending).filter { channel in
            seen.insert(ObjectIdentifier(channel)).inserted
        }
        for channel in channels {
            await channel.stop()
        }
        for task in heartbeatTasks {
            _ = await task.result
        }
        remoteIdentities = [:]
        publishRoutesAndObservations(emitObservations: false)
    }

    private func releasePendingAdmission(for channelID: UUID) {
        inboundConnectionAdmission.releasePending(for: channelID)
    }

    private func hasControlChannel(
        for deviceID: UInt64,
        direction: AppleWifiAwareControlChannelDirection
    ) -> Bool {
        controlChannels.entries(for: deviceID).contains {
            $0.direction == direction
        }
    }

    private func shouldMaintainOutboundChannel(for deviceID: UInt64) -> Bool {
        switch networkingMode {
        case .publisherOnly:
            return false
        case .subscriberOnly:
            return true
        case .automatic:
            return controlRoleStates[deviceID]?.startsBrowser ?? true
        }
    }

    private func registeredControlChannel(
        deviceID: UInt64,
        channelID: UUID
    ) -> AppleWifiAwareControlChannelRegistry<
        AppleWifiAwareRendezvousChannel
    >.Entry? {
        controlChannels.entries(for: deviceID).first {
            $0.channelID == channelID
        }
    }

    private func selectedControlChannel(
        for deviceID: UInt64
    ) -> AppleWifiAwareControlChannelRegistry<
        AppleWifiAwareRendezvousChannel
    >.Entry? {
        controlChannels.selected(
            for: deviceID,
            preferredRole: controlRoleStates[deviceID]?.role
        )
    }

    private func reconcileRemoteIdentity(for deviceID: UInt64) {
        if let selected = selectedControlChannel(for: deviceID) {
            remoteIdentities[deviceID] = selected.remoteIdentity
        } else if !conflictedDeviceIDs.contains(deviceID) {
            remoteIdentities.removeValue(forKey: deviceID)
        }
    }

    @discardableResult
    private func bind(
        _ candidate: LocalNearbyDiscoveryIdentity,
        to deviceID: UInt64
    ) -> Bool {
        guard active,
              pairedDevices[deviceID] != nil,
              let peerKey = NearbyDiscoveryPeerRegistry.normalizePeerKey(
                  candidate.peerKey
              ),
              peerKey != identity.peerKey,
              let displayName = NearbyDiscoveryPeerRegistry.sanitizeDisplayName(
                  candidate.displayName
              ) else {
            return false
        }
        let normalized = LocalNearbyDiscoveryIdentity(
            peerKey: peerKey,
            displayName: displayName
        )
        if conflictedDeviceIDs.contains(deviceID) {
            return false
        }
        if let existing = remoteIdentities[deviceID],
           existing.peerKey != normalized.peerKey {
            remoteIdentities.removeValue(forKey: deviceID)
            conflictedDeviceIDs.insert(deviceID)
            publishRoutesAndObservations(emitObservations: false)
            logger.error("RENDEZVOUS provider=wifi_aware identity=conflict")
            return false
        }
        let duplicateDeviceIDs = remoteIdentities.compactMap {
            claimedDeviceID, claimedIdentity in
            claimedDeviceID != deviceID
                && pairedDevices[claimedDeviceID] != nil
                && claimedIdentity.peerKey == normalized.peerKey
                ? claimedDeviceID
                : nil
        }
        if !duplicateDeviceIDs.isEmpty {
            remoteIdentities[deviceID] = normalized
            conflictedDeviceIDs.insert(deviceID)
            conflictedDeviceIDs.formUnion(duplicateDeviceIDs)
            publishRoutesAndObservations(emitObservations: false)
            logger.error(
                "RENDEZVOUS provider=wifi_aware identity=duplicate_claim"
            )
            return false
        }
        remoteIdentities[deviceID] = normalized
        return true
    }

    private func isIdentityUnique(
        _ candidate: LocalNearbyDiscoveryIdentity,
        for deviceID: UInt64
    ) -> Bool {
        guard !conflictedDeviceIDs.contains(deviceID),
              let selected = selectedControlChannel(for: deviceID),
              selected.remoteIdentity.peerKey == candidate.peerKey else {
            return false
        }
        let claimants = pairedDevices.keys.filter { claimedDeviceID in
            guard !conflictedDeviceIDs.contains(claimedDeviceID),
                  let claimed = selectedControlChannel(for: claimedDeviceID)
            else {
                return false
            }
            return claimed.remoteIdentity.peerKey == candidate.peerKey
        }
        return claimants.count == 1 && claimants[0] == deviceID
    }

    private func publishRoutesAndObservations(emitObservations: Bool) {
        let eligibleIdentities: [(UInt64, LocalNearbyDiscoveryIdentity)] = active
            ? pairedDevices.keys.compactMap { deviceID in
                guard !conflictedDeviceIDs.contains(deviceID),
                      let entry = selectedControlChannel(for: deviceID) else {
                    return nil
                }
                return (deviceID, entry.remoteIdentity)
            }
            : []
        let grouped = Dictionary(
            grouping: eligibleIdentities,
            by: { $0.1.peerKey }
        )
        var routes: [String: String] = [:]
        var visible: [(UInt64, LocalNearbyDiscoveryIdentity)] = []
        for (peerKey, claims) in grouped where claims.count == 1 {
            let claim = claims[0]
            routes[peerKey] = Self.formatDeviceID(claim.0)
            visible.append(claim)
        }

        if routes != publishedRoutes {
            publishedRoutes = routes
            updateRoutes(routes)
        }
        guard emitObservations, active else { return }
        let now = Self.monotonicMilliseconds()
        for (deviceID, remoteIdentity) in visible {
            emit(.observation(NearbyDiscoveryObservation(
                peerKey: remoteIdentity.peerKey,
                source: .wifiAware,
                seenAtMilliseconds: now,
                displayName: remoteIdentity.displayName,
                nearbyWifiAwareDeviceID: Self.formatDeviceID(deviceID)
            )))
        }
    }

    private func rememberedInboundInviteMatches(
        deviceID: UInt64,
        requestID: UInt64,
        invite: String
    ) -> Bool? {
        pruneRememberedInboundRequests()
        return rememberedInboundRequests[
            InboundRequestKey(deviceID: deviceID, requestID: requestID)
        ].map { $0.invite == invite }
    }

    private func rememberInboundRequest(
        deviceID: UInt64,
        requestID: UInt64,
        invite: String
    ) {
        pruneRememberedInboundRequests()
        let now = Self.monotonicMilliseconds()
        let key = InboundRequestKey(deviceID: deviceID, requestID: requestID)
        guard rememberedInboundRequests[key] == nil else { return }
        if rememberedInboundRequests.count >= Self.maximumRememberedInboundRequests,
           let oldest = rememberedInboundRequests.min(by: {
               $0.value.observedAtMilliseconds < $1.value.observedAtMilliseconds
           })?.key {
            rememberedInboundRequests.removeValue(forKey: oldest)
        }
        rememberedInboundRequests[key] = RememberedInboundRequest(
            invite: invite,
            observedAtMilliseconds: now
        )
    }

    private func pruneRememberedInboundRequests() {
        let now = Self.monotonicMilliseconds()
        rememberedInboundRequests = rememberedInboundRequests.filter {
            now >= $0.value.observedAtMilliseconds
                && now - $0.value.observedAtMilliseconds
                    < Self.inboundDeduplicationMilliseconds
        }
    }

    @available(iOS 26.4, *)
    private static func authenticatedContext(
        for connection: NetworkConnection<UDP>,
        expectedDeviceID: UInt64?,
        bootstrapRole: ConnectionBootstrapRole
    ) async throws -> (
        deviceID: UInt64,
        key: Data,
        maximumFrameBytes: Int
    ) {
        // One-to-one UDP connections start lazily. An outstanding receive
        // activates the subscriber without risking a pre-ready datagram being
        // discarded. The explicit acknowledgement keeps control frames out of
        // the bootstrap exchange on both endpoints.
        switch bootstrapRole {
        case .initiator:
            let received = try await exchangeBootstrap(over: connection)
            guard received == connectionBootstrapAcknowledgement else {
                throw AppleWifiAwareRendezvousError.invalidAcknowledgement(
                    byteCount: received.count,
                    firstByte: received.first
                )
            }
        case .acceptor:
            let bootstrap = try await receiveBootstrap(from: connection)
            guard bootstrap == connectionBootstrapDatagram else {
                throw AppleWifiAwareRendezvousError.invalidBootstrap(
                    byteCount: bootstrap.count,
                    firstByte: bootstrap.first
                )
            }
            try await awaitReady(connection)
            try await connection.send(connectionBootstrapAcknowledgement)
        }
        guard let path = connection.currentPath,
              let wifiAwarePath = try? await path.wifiAware else {
            throw AppleWifiAwareRendezvousError.routeUnavailable
        }
        let deviceID = wifiAwarePath.endpoint.device.id
        if let expectedDeviceID, expectedDeviceID != deviceID {
            throw AppleWifiAwareRendezvousError.routeChanged
        }
        let maximumFrameBytes = try await awaitDatagramCapacity(connection)
        guard let protocolName = WASharedSecret.ProtocolName(
                  sharedSecretProtocolName
              ),
              let wifiAwareConnection = connection.wifiAware,
              let sharedSecret = await wifiAwareConnection.deriveSharedSecret(
                  for: protocolName,
                  method: .kdfHash256,
                  context: .bundleID
              ),
              !sharedSecret.data.isEmpty else {
            throw AppleWifiAwareRendezvousError.sharedSecretUnavailable
        }
        return (deviceID, sharedSecret.data, maximumFrameBytes)
    }

    private static func receiveBootstrap(
        from connection: NetworkConnection<UDP>
    ) async throws -> Data {
        try await withThrowingTaskGroup(of: Data.self) { group in
            group.addTask {
                try await connection.receive().content
            }
            group.addTask {
                try await Task<Never, Never>.sleep(
                    for: connectionReadyTimeout
                )
                throw AppleWifiAwareRendezvousError.connectionTimedOut
            }
            defer { group.cancelAll() }
            guard let bootstrap = try await group.next() else {
                throw AppleWifiAwareRendezvousError.connectionTimedOut
            }
            return bootstrap
        }
    }

    private static func exchangeBootstrap(
        over connection: NetworkConnection<UDP>
    ) async throws -> Data {
        try await withThrowingTaskGroup(of: Data.self) { group in
            group.addTask {
                try await connection.receive().content
            }
            group.addTask {
                try await awaitReady(connection)
                while !Task.isCancelled {
                    try await connection.send(connectionBootstrapDatagram)
                    try await Task<Never, Never>.sleep(
                        for: bootstrapRetryInterval
                    )
                }
                throw CancellationError()
            }
            group.addTask {
                try await Task<Never, Never>.sleep(
                    for: connectionReadyTimeout
                )
                throw AppleWifiAwareRendezvousError.connectionTimedOut
            }
            defer { group.cancelAll() }
            guard let acknowledgement = try await group.next() else {
                throw AppleWifiAwareRendezvousError.connectionTimedOut
            }
            return acknowledgement
        }
    }

    private static func awaitReady(
        _ connection: NetworkConnection<UDP>
    ) async throws {
        let clock = ContinuousClock()
        let deadline = clock.now.advanced(by: connectionReadyTimeout)
        while clock.now < deadline {
            try Task.checkCancellation()
            switch connection.state {
            case .ready:
                return
            case .failed:
                throw AppleWifiAwareRendezvousError.routeUnavailable
            case .cancelled:
                throw CancellationError()
            case .setup, .waiting, .preparing:
                try await Task<Never, Never>.sleep(
                    for: connectionReadyPollInterval
                )
            @unknown default:
                try await Task<Never, Never>.sleep(
                    for: connectionReadyPollInterval
                )
            }
        }
        throw AppleWifiAwareRendezvousError.connectionTimedOut
    }

    private static func awaitDatagramCapacity(
        _ connection: NetworkConnection<UDP>
    ) async throws -> Int {
        let clock = ContinuousClock()
        let deadline = clock.now.advanced(by: connectionReadyTimeout)
        while clock.now < deadline {
            try Task.checkCancellation()
            let capacity = connection.maximumDatagramSize
            if capacity > WifiAwareRendezvousProtocol.frameHeaderSize {
                return capacity
            }
            switch connection.state {
            case .failed:
                throw AppleWifiAwareRendezvousError.routeUnavailable
            case .cancelled:
                throw CancellationError()
            case .setup, .waiting, .preparing, .ready:
                try await Task<Never, Never>.sleep(
                    for: connectionReadyPollInterval
                )
            @unknown default:
                try await Task<Never, Never>.sleep(
                    for: connectionReadyPollInterval
                )
            }
        }
        throw AppleWifiAwareRendezvousError.insufficientDatagramSize
    }

    private static func monotonicMilliseconds() -> Int64 {
        Int64(ProcessInfo.processInfo.systemUptime * 1_000)
    }

    private static func formatDeviceID(_ deviceID: UInt64) -> String {
        String(format: "%016llx", deviceID)
    }

    private static func parseDeviceID(_ value: String) -> UInt64? {
        let normalized = value.trimmingCharacters(in: .whitespacesAndNewlines)
            .lowercased()
        guard normalized.count == 16 else { return nil }
        return UInt64(normalized, radix: 16)
    }
}

private enum AppleWifiAwareRendezvousError: LocalizedError {
    case connectionTimedOut
    case routeUnavailable
    case routeChanged
    case sharedSecretUnavailable
    case invalidBootstrap(byteCount: Int, firstByte: UInt8?)
    case invalidAcknowledgement(byteCount: Int, firstByte: UInt8?)
    case insufficientDatagramSize

    var errorDescription: String? {
        switch self {
        case .connectionTimedOut:
            return "Wi-Fi Aware connection timed out"
        case .routeUnavailable:
            return "The selected Wi-Fi Aware route is unavailable"
        case .routeChanged:
            return "The selected Wi-Fi Aware route changed"
        case .sharedSecretUnavailable:
            return "The Wi-Fi Aware connection could not be authenticated"
        case .invalidBootstrap:
            return "The Wi-Fi Aware connection used an unsupported handshake"
        case .invalidAcknowledgement:
            return "The Wi-Fi Aware connection returned an invalid handshake acknowledgement"
        case .insufficientDatagramSize:
            return "The Wi-Fi Aware connection cannot carry invitation frames"
        }
    }
}

@available(iOS 26.0, *)
private extension WAError {
    var isNoPairedDevices: Bool {
        if case .noPairedDevices = self {
            return true
        }
        return false
    }
}

#if canImport(DeviceDiscoveryUI)
import DeviceDiscoveryUI
import SwiftUI

@available(iOS 26.0, *)
struct AppleWifiAwarePairingControls: View {
    let language: String

    @State private var observation = WifiAwarePairingDeviceObservation.loading
    @State private var baselineDeviceIDs: Set<UInt64>?
    @State private var pickerSelectedDeviceID: UInt64?
    @State private var pickerSelectedDisplayName: String?
    @State private var observationAttempt = 0

    private static let logger = Logger(
        subsystem: Bundle.main.bundleIdentifier ?? "com.envoix.app",
        category: "wifi-aware-pairing-ui"
    )

    var body: some View {
        Group {
            if let publishable = WAPublishableService
                .allServices[envoixWifiAwareService],
               let subscribable = WASubscribableService
                .allServices[envoixWifiAwareService] {
                VStack(alignment: .leading, spacing: 12) {
                    pairingStatus

                    if pairingControlsAreAvailable {
                        DevicePairingView(
                            .wifiAware(
                                .connecting(
                                    to: publishable,
                                    from: .userSpecifiedDevices
                                )
                            ),
                            access: .permanent
                        ) {
                            Label(
                                AppText.value(
                                    "Show this device and code",
                                    "显示本机与验证码",
                                    language: language
                                ),
                                systemImage: "number.circle"
                            )
                            .frame(maxWidth: .infinity)
                        } fallback: {
                            pairingUnavailable
                        }
                        .buttonStyle(.bordered)
                        .accessibilityIdentifier("nearby_wifi_aware_allow")

                        DevicePicker(
                            .wifiAware(
                                .connecting(
                                    to: .userSpecifiedDevices,
                                    from: subscribable
                                )
                            ),
                            access: .permanent
                        ) { endpoint in
                            pickerSelectedDeviceID = endpoint.device.id
                            // The picker is the subscriber side of Apple's
                            // pairing UI. Its explicit result is authoritative
                            // if the paired-device observer raced ahead and
                            // provisionally classified the new device.
                            AppleWifiAwareControlRoleStore.shared.set(
                                .subscriber,
                                for: endpoint.device.id
                            )
                            pickerSelectedDisplayName = endpoint.device.name
                                ?? endpoint.device.pairingInfo?.pairingName
                            Self.logger.info(
                                "PAIRING provider=wifi_aware event=picker_selected"
                            )
                        } label: {
                            Label(
                                AppText.value(
                                    "Find the other device",
                                    "查找另一台设备",
                                    language: language
                                ),
                                systemImage: "magnifyingglass"
                            )
                            .frame(maxWidth: .infinity)
                        } fallback: {
                            pairingUnavailable
                        }
                        .buttonStyle(PrimaryActionButtonStyle())
                        .accessibilityIdentifier("nearby_wifi_aware_add")
                    }
                }
            } else {
                Text(AppText.value(
                    "Nearby pairing is unavailable in this build.",
                    "此版本无法使用附近设备配对。",
                    language: language
                ))
                .font(.footnote)
                .foregroundStyle(Theme.danger)
                .fixedSize(horizontal: false, vertical: true)
            }
        }
        .task(id: observationAttempt) {
            await observePairedDevices()
        }
    }

    @ViewBuilder
    private var pairingStatus: some View {
        switch WifiAwarePairingPresentationPolicy.evaluate(
            observation: observation,
            pickerSelectedDeviceID: pickerSelectedDeviceID
        ) {
        case .loading:
            HStack(spacing: 8) {
                ProgressView()
                    .controlSize(.small)
                Text(AppText.value(
                    "Checking Apple paired devices…",
                    "正在检查 Apple 配对设备…",
                    language: language
                ))
            }
            .font(.footnote)
            .foregroundStyle(Theme.muted)

        case .guidance(.newPair):
            Text(AppText.value(
                "For the first pair, tap “Show this device and code” on one device. On the other, tap “Find the other device”, select it, then enter or confirm the six-digit code. The two devices must use opposite buttons.",
                "首次配对时，请在一台设备上点“显示本机与验证码”；在另一台设备上点“查找另一台设备”，选择前一台设备，再输入或确认六位码。两台设备必须使用不同的按钮。",
                language: language
            ))
            .font(.footnote)
            .foregroundStyle(Theme.muted)
            .fixedSize(horizontal: false, vertical: true)

        case .guidance(.existingPairs(let count)):
            Text(AppText.value(
                "\(count) Apple-paired device\(count == 1 ? "" : "s") already available. Existing pairs do not show another six-digit code; tap Done to resume automatic discovery. Use the controls below only to add a new device.",
                "已有 \(count) 台 Apple 系统配对设备。已有配对不会再次显示六位码；点“完成”即可恢复自动发现。仅在添加新设备时使用下方按钮。",
                language: language
            ))
            .font(.footnote)
            .foregroundStyle(Theme.muted)
            .fixedSize(horizontal: false, vertical: true)

        case .success(.pairedDevicesObserved(_, let totalCount)):
            Label(
                AppText.value(
                    "New pairing detected · \(totalCount) total",
                    "已检测到新配对 · 共 \(totalCount) 台",
                    language: language
                ),
                systemImage: "checkmark.circle.fill"
            )
            .font(.footnote.weight(.semibold))
            .foregroundStyle(Theme.success)

        case .success(.pickerSelected(_, let snapshotConfirmed)):
            Label(
                pickerSuccessText(snapshotConfirmed: snapshotConfirmed),
                systemImage: snapshotConfirmed
                    ? "checkmark.circle.fill"
                    : "hourglass.circle"
            )
            .font(.footnote.weight(.semibold))
            .foregroundStyle(snapshotConfirmed ? Theme.success : Theme.accentStrong)

        case .observationFailed:
            VStack(alignment: .leading, spacing: 8) {
                Text(AppText.value(
                    "Envoix could not read Apple's paired-device list. No pairing record was changed.",
                    "Envoix 无法读取 Apple 的配对设备列表；现有配对记录未被更改。",
                    language: language
                ))
                .font(.footnote)
                .foregroundStyle(Theme.danger)
                Button(AppText.value("Retry", "重试", language: language)) {
                    observationAttempt += 1
                }
                .buttonStyle(.bordered)
                .accessibilityIdentifier("nearby_wifi_aware_pairing_retry")
            }
        }
    }

    private var pairingControlsAreAvailable: Bool {
        switch observation {
        case .snapshot:
            return true
        case .loading, .failed:
            return pickerSelectedDeviceID != nil
        }
    }

    private func pickerSuccessText(snapshotConfirmed: Bool) -> String {
        let name = pickerSelectedDisplayName?.trimmingCharacters(
            in: .whitespacesAndNewlines
        )
        let resolvedName: String
        if let name, !name.isEmpty {
            resolvedName = name
        } else {
            resolvedName = AppText.value("device", "设备", language: language)
        }
        if snapshotConfirmed {
            return AppText.value(
                "\(resolvedName) is paired and ready",
                "\(resolvedName) 已配对并可使用",
                language: language
            )
        }
        return AppText.value(
            "\(resolvedName) selected; waiting for Apple's pairing list",
            "已选择 \(resolvedName)；正在等待 Apple 配对列表更新",
            language: language
        )
    }

    @MainActor
    private func observePairedDevices() async {
        if baselineDeviceIDs == nil {
            observation = .loading
        }
        while !Task.isCancelled {
            let sequence = WAPairedDevice.allDevices
            do {
                let devices: WAPairedDevice.Devices
                do {
                    guard let current = try await sequence.current() else {
                        Self.logger.error(
                            "PAIRING provider=wifi_aware event=observation_unavailable"
                        )
                        observation = .failed
                        return
                    }
                    devices = current
                } catch let error as WAError where error.isNoPairedDevices {
                    devices = [:]
                }
                let currentIDs = Set(devices.keys)
                if baselineDeviceIDs == nil {
                    baselineDeviceIDs = currentIDs
                }
                publishPairingSnapshot(currentDeviceIDs: currentIDs)

                do {
                    for try await updatedDevices in sequence {
                        try Task.checkCancellation()
                        publishPairingSnapshot(
                            currentDeviceIDs: Set(updatedDevices.keys)
                        )
                    }
                } catch let error as WAError where error.isNoPairedDevices {
                    publishPairingSnapshot(currentDeviceIDs: [])
                }
            } catch is CancellationError {
                return
            } catch {
                Self.logger.error(
                    "PAIRING provider=wifi_aware event=observation_failed"
                )
                observation = .failed
                return
            }

            do {
                try await Task<Never, Never>.sleep(for: .seconds(1))
            } catch {
                return
            }
        }
    }

    @MainActor
    private func publishPairingSnapshot(currentDeviceIDs: Set<UInt64>) {
        let baseline = baselineDeviceIDs ?? currentDeviceIDs
        for deviceID in currentDeviceIDs.subtracting(baseline) {
            AppleWifiAwareControlRoleStore.shared.setIfAbsent(
                .publisher,
                for: deviceID
            )
        }
        if case .snapshot(_, let previousIDs) = observation,
           previousIDs != currentDeviceIDs {
            Self.logger.info(
                "PAIRING provider=wifi_aware event=snapshot_changed count=\(currentDeviceIDs.count, privacy: .public)"
            )
        }
        observation = .snapshot(
            baselineDeviceIDs: baseline,
            currentDeviceIDs: currentDeviceIDs
        )
    }

    private var pairingUnavailable: some View {
        Text(AppText.value("Pairing unavailable", "配对不可用", language: language))
            .frame(maxWidth: .infinity)
    }
}
#endif
#endif
