import CryptoKit
import Foundation

#if os(iOS) && canImport(WiFiAware)
import EnvoixCore
import Network
import OSLog
import WiFiAware

@available(iOS 26.0, *)
enum AppleWifiAwareTransportError: Error {
    case invalidDeviceID
    case pairedDeviceUnavailable
    case serviceNotDeclared
    case listenerFinishedWithoutResult
    case receiverListenerReadyTimedOut
    case receiverConnectionReadyTimedOut
    case browserReadyTimedOut
    case peerHelloTimedOut
    case invalidPeerHelloDatagram
    case invalidTransferAuthenticator
    case peerReadyTimedOut
    case invalidPeerReadyDatagram
    case invalidReadBound
    case datagramExceedsBound
    case datagramChannelClosed
    case concurrentReceive
    case connectionReadyTimedOut
    case insufficientDatagramSize
    case noWifiAwarePath
}

@available(iOS 26.0, *)
extension AppleWifiAwareTransportError: LocalizedError {
    var errorDescription: String? {
        switch self {
        case .invalidDeviceID:
            "The selected Wi-Fi Aware device identifier is invalid."
        case .pairedDeviceUnavailable:
            "The selected Apple-paired device is no longer available."
        case .serviceNotDeclared:
            "This build does not declare the Wi-Fi Aware transfer service."
        case .listenerFinishedWithoutResult:
            "The Wi-Fi Aware receiver stopped before the transfer completed."
        case .receiverListenerReadyTimedOut:
            "The Wi-Fi Aware receiver could not start in time."
        case .receiverConnectionReadyTimedOut:
            "The Wi-Fi Aware sender did not connect in time."
        case .browserReadyTimedOut:
            "The paired Wi-Fi Aware device was not found in time."
        case .peerHelloTimedOut:
            "The Wi-Fi Aware peer did not identify this transfer in time."
        case .invalidPeerHelloDatagram:
            "The Wi-Fi Aware connection belongs to a different transfer."
        case .invalidTransferAuthenticator:
            "The Wi-Fi Aware transfer invitation has no valid matching scope."
        case .peerReadyTimedOut:
            "The Wi-Fi Aware receiver did not acknowledge the transfer in time."
        case .invalidPeerReadyDatagram:
            "The Wi-Fi Aware receiver returned an invalid acknowledgement."
        case .invalidReadBound:
            "The Wi-Fi Aware transfer requested an invalid datagram size."
        case .datagramExceedsBound:
            "A Wi-Fi Aware datagram exceeded the negotiated path capacity."
        case .datagramChannelClosed:
            "The Wi-Fi Aware connection closed during transfer."
        case .concurrentReceive:
            "The Wi-Fi Aware transport received overlapping read requests."
        case .connectionReadyTimedOut:
            "The Wi-Fi Aware connection could not become ready in time."
        case .insufficientDatagramSize:
            "The Wi-Fi Aware path cannot carry the required QUIC datagrams."
        case .noWifiAwarePath:
            "The connection did not use the selected Wi-Fi Aware path."
        }
    }
}

@available(iOS 26.0, *)
enum AppleWifiAwarePerformanceStage: String, Sendable {
    case ready
    case completed
}

@available(iOS 26.0, *)
struct AppleWifiAwarePerformanceSample: Sendable {
    let stage: AppleWifiAwarePerformanceStage
    let maximumDatagramSize: Int
    let throughputCeilingMbps: Double?
    let throughputCapacityMbps: Double?
    let throughputCapacityRatio: Double?
    let signalStrength: Double?
}

@available(iOS 26.0, *)
typealias AppleWifiAwarePerformanceObserver =
    @Sendable (AppleWifiAwarePerformanceSample) -> Void

@available(iOS 26.0, *)
enum AppleWifiAwareFallbackBoundary {
    static func crosses(for phase: FfiManifestV2Phase) -> Bool {
        switch phase {
        case .waitingForPeer, .pairing, .connecting:
            false
        case .transferring, .verifying, .saving, .waitingForReceiverSave,
             .finalizingDelivery, .delivered:
            true
        }
    }

    static func crosses(for recoveryAction: FfiRecoveryAction) -> Bool {
        recoveryAction == .rePair
    }
}

/// Apple requires matching performance modes on both endpoints. Keep the
/// system-recommended bulk mode explicit and retain the default best-effort
/// service class for file transfer.
@available(iOS 26.0, *)
func envoixWifiAwareUDPParameters() -> NWParametersBuilder<UDP> {
    .parameters {
        UDP()
    }
    .wifiAware { $0.performanceMode = .bulk }
}

/// Diagnostic-only parameters for reproducing Apple's Wi-Fi Aware TCP
/// `ENOBUFS` failure. Canonical transfers use UDP above.
@available(iOS 26.0, *)
func envoixWifiAwareTCPParameters() -> NWParametersBuilder<TCP> {
    .parameters {
        TCP()
    }
    .wifiAware { $0.performanceMode = .bulk }
}

/// Keeps the Wi-Fi Aware publisher or subscriber scope alive for the entire
/// canonical Rust transfer session.
@available(iOS 26.0, *)
enum AppleWifiAwareTransportSession {
    private static let logger = Logger(
        subsystem: "com.envoix.app.ios",
        category: "wifi-aware-transport"
    )

    static func pairedDevice(sourceScopedID: String) async throws -> WAPairedDevice {
        guard sourceScopedID.count <= NearbyPairedDevice.maximumSourceScopedIDLength,
              let identifier = UInt64(sourceScopedID, radix: 16)
        else {
            throw AppleWifiAwareTransportError.invalidDeviceID
        }
        let devices = try await WAPairedDevice.allDevices.current() ?? [:]
        guard let device = devices[identifier] else {
            throw AppleWifiAwareTransportError.pairedDeviceUnavailable
        }
        return device
    }

    static func sendNearbyHybrid(
        sourceScopedDeviceID: String,
        job: FfiTransferJobV2,
        settings: EnvoixRuntimeSettings,
        request: FfiTransferRequest,
        stateDirectory: String,
        cancellation: FfiManifestV2Cancellation,
        observer: TransferObserver,
        performanceObserver: AppleWifiAwarePerformanceObserver? = nil
    ) async throws -> FfiManifestV2Completion {
        let fallbackObserver = AppleWifiAwareFallbackObserver(downstream: observer)
        let wifiAwareStartedAt = ContinuousClock().now
        do {
            let device = try await pairedDevice(sourceScopedID: sourceScopedDeviceID)
            let authenticator = try peerHelloAuthenticator(for: request)
            return try await withSenderTransport(
                device: device,
                peerHello: peerHelloDatagram(authenticator: authenticator),
                performanceObserver: performanceObserver
            ) { transport, maximumDatagramSize in
                try await sendTransferJobV2NearbyHybrid(
                    job: job,
                    settings: settings,
                    request: request,
                    stateDirectory: stateDirectory,
                    transport: transport,
                    maximumDatagramSize: maximumDatagramSize,
                    cancellation: cancellation,
                    observer: fallbackObserver
                )
            }
        } catch {
            guard shouldFallbackToIroh(
                after: error,
                cancellation: cancellation,
                observer: fallbackObserver
            ) else {
                fallbackObserver.forwardSuppressedFailure()
                throw error
            }
            try await awaitFallbackAlignment(
                since: wifiAwareStartedAt,
                cancellation: cancellation
            )
            observer.onDiagnostic(message: Self.fallbackDiagnostic)
            fallbackObserver.activateFallback()
            return try await sendTransferJobV2(
                job: job,
                settings: settings,
                request: request,
                stateDirectory: stateDirectory,
                cancellation: cancellation,
                observer: fallbackObserver
            )
        }
    }

    static func receiveNearbyHybrid(
        sourceScopedDeviceID: String,
        settings: EnvoixRuntimeSettings,
        request: FfiTransferRequest,
        stateDirectory: String,
        cancellation: FfiManifestV2Cancellation,
        observer: TransferObserver,
        performanceObserver: AppleWifiAwarePerformanceObserver? = nil,
        onListenerReady: (@MainActor @Sendable () -> Void)? = nil,
        destinationDecision: @escaping @Sendable (
            FfiPendingManifestV2Receive
        ) async throws -> FfiDestinationRequestV2
    ) async throws -> FfiManifestV2Completion {
        let fallbackObserver = AppleWifiAwareFallbackObserver(downstream: observer)
        let wifiAwareStartedAt = ContinuousClock().now
        do {
            let device = try await pairedDevice(sourceScopedID: sourceScopedDeviceID)
            let authenticator = try peerHelloAuthenticator(for: request)
            return try await withReceiverTransport(
                device: device,
                peerHello: peerHelloDatagram(authenticator: authenticator),
                performanceObserver: performanceObserver,
                onListenerReady: onListenerReady
            ) { transport, maximumDatagramSize in
                let pending = try await receiveTransferOfferV2NearbyHybrid(
                    settings: settings,
                    request: request,
                    stateDirectory: stateDirectory,
                    transport: transport,
                    maximumDatagramSize: maximumDatagramSize,
                    cancellation: cancellation,
                    observer: fallbackObserver
                )
                fallbackObserver.crossFallbackBoundary()
                let destination = try await destinationDecision(pending)
                return try await pending.receive(
                    destination: destination,
                    observer: fallbackObserver
                )
            }
        } catch {
            guard shouldFallbackToIroh(
                after: error,
                cancellation: cancellation,
                observer: fallbackObserver
            ) else {
                fallbackObserver.forwardSuppressedFailure()
                throw error
            }
            try await awaitFallbackAlignment(
                since: wifiAwareStartedAt,
                cancellation: cancellation
            )
            observer.onDiagnostic(message: Self.fallbackDiagnostic)
            fallbackObserver.activateFallback()
            let pending = try await receiveTransferOfferV2(
                settings: settings,
                request: request,
                stateDirectory: stateDirectory,
                cancellation: cancellation,
                observer: fallbackObserver
            )
            let destination = try await destinationDecision(pending)
            return try await pending.receive(
                destination: destination,
                observer: fallbackObserver
            )
        }
    }

    static func isRecoverableWifiAwareFailure(_ error: Error) -> Bool {
        guard !(error is CancellationError) else { return false }
        if let transportError = error as? AppleWifiAwareTransportError {
            if case .invalidTransferAuthenticator = transportError {
                return false
            }
            return true
        }
        if error is NWError || error is WAError {
            return true
        }
        if let ffiError = error as? EnvoixError,
           case .Operation(reason: let reason) = ffiError,
           reason == nearbyHybridPreAuthTransportFailureMarker
            || reason.hasPrefix(
                nearbyHybridPreAuthTransportFailureMarker + ": "
            ) {
            return true
        }
        let description = String(reflecting: error)
        return Self.recoverableFailureMarkers.contains {
            description.localizedCaseInsensitiveContains($0)
        }
    }

    private static func shouldFallbackToIroh(
        after error: Error,
        cancellation: FfiManifestV2Cancellation,
        observer: AppleWifiAwareFallbackObserver
    ) -> Bool {
        !cancellation.isCancelled() &&
            !observer.sawTerminalCancellation &&
            !observer.crossedFallbackBoundary &&
            isRecoverableWifiAwareFailure(error)
    }

    private static func awaitFallbackAlignment(
        since startedAt: ContinuousClock.Instant,
        cancellation: FfiManifestV2Cancellation
    ) async throws {
        let clock = ContinuousClock()
        let deadline = startedAt.advanced(by: connectionReadyTimeout)
        while clock.now < deadline {
            guard !cancellation.isCancelled() else {
                throw CancellationError()
            }
            try await Task<Never, Never>.sleep(for: connectionReadyPollInterval)
        }
    }

    private static let fallbackDiagnostic =
        "Wi-Fi Aware path unavailable; continuing over authenticated iroh direct/relay"
    static let nearbyHybridPreAuthTransportFailureMarker =
        "nearby_hybrid_pre_auth_transport_failure"
    private static let recoverableFailureMarkers = [
        "Wi-Fi Aware datagram bootstrap timed out",
        "platform Wi-Fi Aware datagram transport failed",
    ]

    static func send(
        sourceScopedDeviceID: String,
        job: FfiTransferJobV2,
        pairingToken: String,
        stateDirectory: String,
        cancellation: FfiManifestV2Cancellation,
        observer: TransferObserver,
        performanceObserver: AppleWifiAwarePerformanceObserver? = nil
    ) async throws -> FfiNativeManifestV2Completion {
        let device = try await pairedDevice(sourceScopedID: sourceScopedDeviceID)
        return try await withSenderTransport(
            device: device,
            peerHello: peerHelloDatagram(authenticator: pairingToken),
            performanceObserver: performanceObserver
        ) { transport, maximumDatagramSize in
            try await sendTransferJobV2OverDatagramTransport(
                job: job,
                pairingToken: pairingToken,
                stateDirectory: stateDirectory,
                transport: transport,
                maximumDatagramSize: maximumDatagramSize,
                cancellation: cancellation,
                observer: observer
            )
        }
    }

    static func receive(
        sourceScopedDeviceID: String,
        pairingToken: String,
        stateDirectory: String,
        cancellation: FfiManifestV2Cancellation,
        observer: TransferObserver,
        performanceObserver: AppleWifiAwarePerformanceObserver? = nil,
        destinationDecision: @escaping @Sendable (
            FfiPendingManifestV2Receive
        ) async throws -> FfiDestinationRequestV2
    ) async throws -> FfiManifestV2Completion {
        let device = try await pairedDevice(sourceScopedID: sourceScopedDeviceID)
        return try await withReceiverTransport(
            device: device,
            peerHello: peerHelloDatagram(authenticator: pairingToken),
            performanceObserver: performanceObserver
        ) { transport, maximumDatagramSize in
            let pending = try await receiveTransferOfferV2OverDatagramTransport(
                pairingToken: pairingToken,
                stateDirectory: stateDirectory,
                transport: transport,
                maximumDatagramSize: maximumDatagramSize,
                cancellation: cancellation,
                observer: observer
            )
            let destination = try await destinationDecision(pending)
            return try await pending.receive(destination: destination, observer: observer)
        }
    }

    /// Receiver role: publish the declared UDP service and retain the listener
    /// until Rust has completed or failed the Manifest v2 session.
    /// `onListenerReady` runs exactly once after the listener is bound and
    /// before this method waits for the sender's connection.
    static func withReceiverTransport<Result: Sendable>(
        device: WAPairedDevice,
        peerHello: Data = defaultPeerHelloDatagram,
        performanceObserver: AppleWifiAwarePerformanceObserver? = nil,
        onListenerReady: (@MainActor @Sendable () -> Void)? = nil,
        operation: @escaping @Sendable (
            FfiNativeDatagramTransport,
            UInt32
        ) async throws -> Result
    ) async throws -> Result {
        try await withServiceLease(.transferReceiver) {
            try await withReceiverTransportWhileLeased(
                device: device,
                peerHello: peerHello,
                performanceObserver: performanceObserver,
                onListenerReady: onListenerReady,
                operation: operation
            )
        }
    }

    private static func withReceiverTransportWhileLeased<Result: Sendable>(
        device: WAPairedDevice,
        peerHello: Data,
        performanceObserver: AppleWifiAwarePerformanceObserver?,
        onListenerReady: (@MainActor @Sendable () -> Void)?,
        operation: @escaping @Sendable (
            FfiNativeDatagramTransport,
            UInt32
        ) async throws -> Result
    ) async throws -> Result {
        guard let service = WAPublishableService.allServices[envoixWifiAwareService] else {
            throw AppleWifiAwareTransportError.serviceNotDeclared
        }
        let (readyListeners, listenerReadyContinuation) =
            AsyncThrowingStream<Void, Error>.makeStream(
                bufferingPolicy: .bufferingNewest(1)
            )
        let listener: NetworkListener<UDP> = try NetworkListener(
            for: .wifiAware(.connecting(to: service, from: .selected([device]))),
            using: envoixWifiAwareUDPParameters()
        )
        .onStateUpdate { _, state in
            Self.logListenerState(state)
            switch state {
            case .ready:
                listenerReadyContinuation.yield()
                listenerReadyContinuation.finish()
            case .failed(let error):
                listenerReadyContinuation.finish(throwing: error)
            case .cancelled:
                listenerReadyContinuation.finish(throwing: CancellationError())
            case .setup, .waiting:
                break
            @unknown default:
                break
            }
        }
        let (results, continuation) = AsyncThrowingStream<Result, Error>.makeStream()
        let (readyConnections, readyContinuation) = AsyncThrowingStream<Void, Error>.makeStream(
            bufferingPolicy: .bufferingNewest(1)
        )
        let admission = AppleWifiAwareReceiverAdmission()

        let listenerTask = Task {
            do {
                try await listener.run { connection in
                    connection.onStateUpdate { connection, state in
                        Self.logConnectionState(
                            state,
                            role: "receiver",
                            connection: connection
                        )
                    }
                    var claimed = false
                    var transport: AppleWifiAwareDatagramTransport?
                    do {
                        // Listener-accepted one-to-one connections are lazy.
                        // Receiving the required peer hello starts this data
                        // path without introducing a synthetic protocol frame.
                        try await awaitPeerHello(from: connection, expected: peerHello)
                        try await awaitReady(connection, role: "receiver")
                        try await requireWifiAwarePath(connection)
                        let maximumDatagramSize = try requireDatagramCapacity(connection)
                        guard await admission.claim() else {
                            logger.notice(
                                "role=receiver handshake=duplicate_authenticated_connection_rejected"
                            )
                            return
                        }
                        claimed = true
                        let activeTransport = AppleWifiAwareDatagramTransport(
                            connection,
                            role: "receiver",
                            interceptingControlDatagram: peerHello,
                            onControlDatagram: {
                                do {
                                    try await connection.send(peerReadyDatagram)
                                    logger.info(
                                        "role=receiver handshake=peer_ready_resent"
                                    )
                                } catch {
                                    logger.notice(
                                        "role=receiver handshake=peer_ready_resend_failed error=\(String(describing: error), privacy: .public)"
                                    )
                                }
                            }
                        )
                        transport = activeTransport
                        try await connection.send(peerReadyDatagram)
                        logger.info("role=receiver handshake=peer_ready_sent")
                        readyContinuation.yield()
                        readyContinuation.finish()
                        await reportPerformance(
                            connection,
                            role: "receiver",
                            stage: .ready,
                            observer: performanceObserver
                        )
                        let result = try await operation(
                            activeTransport,
                            maximumDatagramSize
                        )
                        await reportPerformance(
                            connection,
                            role: "receiver",
                            stage: .completed,
                            observer: performanceObserver
                        )
                        try? await activeTransport.close()
                        continuation.yield(result)
                        continuation.finish()
                    } catch {
                        if let transport {
                            try? await transport.close()
                        }
                        if claimed {
                            readyContinuation.finish(throwing: error)
                            continuation.finish(throwing: error)
                        } else if !(error is CancellationError) {
                            logger.notice(
                                "role=receiver handshake=connection_rejected error=\(String(describing: error), privacy: .public)"
                            )
                        }
                    }
                }
                listenerReadyContinuation.finish()
                readyContinuation.finish()
                continuation.finish()
            } catch is CancellationError {
                listenerReadyContinuation.finish(throwing: CancellationError())
                readyContinuation.finish(throwing: CancellationError())
                continuation.finish(throwing: CancellationError())
            } catch {
                listenerReadyContinuation.finish(throwing: error)
                readyContinuation.finish(throwing: error)
                continuation.finish(throwing: error)
            }
        }

        do {
            try await awaitReceiverListenerReady(
                readyListeners,
                onListenerReady: onListenerReady
            )
            try await awaitReceiverConnectionReady(readyConnections)
            for try await value in results {
                listenerTask.cancel()
                _ = await listenerTask.result
                return value
            }
        } catch {
            listenerTask.cancel()
            _ = await listenerTask.result
            throw error
        }
        listenerTask.cancel()
        _ = await listenerTask.result
        throw AppleWifiAwareTransportError.listenerFinishedWithoutResult
    }

    static func awaitReceiverListenerReady(
        _ readyListeners: AsyncThrowingStream<Void, Error>,
        onListenerReady: (@MainActor @Sendable () -> Void)?
    ) async throws {
        try Task.checkCancellation()
        try await withThrowingTaskGroup(of: Bool.self) { group in
            group.addTask {
                for try await _ in readyListeners {
                    return true
                }
                return false
            }
            group.addTask {
                try await Task<Never, Never>.sleep(for: connectionReadyTimeout)
                return false
            }
            defer { group.cancelAll() }
            guard try await group.next() == true else {
                try Task.checkCancellation()
                throw AppleWifiAwareTransportError.receiverListenerReadyTimedOut
            }
        }
        try Task.checkCancellation()
        if let onListenerReady {
            await onListenerReady()
        }
    }

    private static func awaitReceiverConnectionReady(
        _ readyConnections: AsyncThrowingStream<Void, Error>
    ) async throws {
        try await withThrowingTaskGroup(of: Bool.self) { group in
            group.addTask {
                for try await _ in readyConnections {
                    return true
                }
                return false
            }
            group.addTask {
                try await Task<Never, Never>.sleep(for: connectionReadyTimeout)
                return false
            }
            defer { group.cancelAll() }
            guard try await group.next() == true else {
                throw AppleWifiAwareTransportError.receiverConnectionReadyTimedOut
            }
        }
    }

    /// Sender role: browse the selected paired device, then retain its UDP
    /// connection until Rust has completed or failed the Manifest v2 session.
    static func withSenderTransport<Result: Sendable>(
        device: WAPairedDevice,
        peerHello: Data = defaultPeerHelloDatagram,
        performanceObserver: AppleWifiAwarePerformanceObserver? = nil,
        operation: @escaping @Sendable (
            FfiNativeDatagramTransport,
            UInt32
        ) async throws -> Result
    ) async throws -> Result {
        try await withServiceLease(.transferSender) {
            try await withSenderTransportWhileLeased(
                device: device,
                peerHello: peerHello,
                performanceObserver: performanceObserver,
                operation: operation
            )
        }
    }

    private static func withSenderTransportWhileLeased<Result: Sendable>(
        device: WAPairedDevice,
        peerHello: Data,
        performanceObserver: AppleWifiAwarePerformanceObserver?,
        operation: @escaping @Sendable (
            FfiNativeDatagramTransport,
            UInt32
        ) async throws -> Result
    ) async throws -> Result {
        guard let service = WASubscribableService.allServices[envoixWifiAwareService] else {
            throw AppleWifiAwareTransportError.serviceNotDeclared
        }
        let browser = NetworkBrowser(
            for: WASubscriberBrowser.wifiAware(
                .connecting(to: .selected([device]), from: service)
            )
        )
        .onStateUpdate { _, state in
            Self.logBrowserState(state)
        }
        let endpoint = try await discoverEndpoint(for: device, using: browser)
        let connection: NetworkConnection<UDP> = NetworkConnection(
            to: endpoint,
            using: envoixWifiAwareUDPParameters()
        )
        .onStateUpdate { connection, state in
            Self.logConnectionState(
                state,
                role: "sender",
                connection: connection
            )
        }
        let (peerReadyEvents, peerReadyContinuation) = AsyncStream<Void>.makeStream(
            bufferingPolicy: .bufferingNewest(1)
        )
        let transport = AppleWifiAwareDatagramTransport(
            connection,
            role: "sender",
            interceptingControlDatagram: peerReadyDatagram,
            onControlDatagram: {
                peerReadyContinuation.yield()
            }
        )
        defer { peerReadyContinuation.finish() }
        do {
            try await awaitReady(connection, role: "sender")
            try await requireWifiAwarePath(connection)
            let maximumDatagramSize = try requireDatagramCapacity(connection)
            try await awaitPeerReady(
                peerReadyEvents,
                sendPeerHello: {
                    try await connection.send(peerHello)
                }
            )
            logger.info("role=sender handshake=peer_ready_received")
            await reportPerformance(
                connection,
                role: "sender",
                stage: .ready,
                observer: performanceObserver
            )
            let result = try await operation(transport, maximumDatagramSize)
            await reportPerformance(
                connection,
                role: "sender",
                stage: .completed,
                observer: performanceObserver
            )
            try? await transport.close()
            return result
        } catch {
            try? await transport.close()
            throw error
        }
    }

    private static func withServiceLease<Result: Sendable>(
        _ purpose: AppleWifiAwareServiceCoordinator.Purpose,
        operation: () async throws -> Result
    ) async throws -> Result {
        let lease = try await AppleWifiAwareServiceCoordinator.shared.acquire(
            purpose
        )
        do {
            let result = try await operation()
            await AppleWifiAwareServiceCoordinator.shared.release(lease)
            return result
        } catch {
            await AppleWifiAwareServiceCoordinator.shared.release(lease)
            throw error
        }
    }

    private static func awaitReady(
        _ connection: NetworkConnection<UDP>,
        role: String
    ) async throws {
        let clock = ContinuousClock()
        let deadline = clock.now.advanced(by: connectionReadyTimeout)
        while clock.now < deadline {
            switch connection.state {
            case .ready:
                let message = "role=\(role) max_datagram=\(connection.maximumDatagramSize)"
                logger.info("\(message, privacy: .public)")
                return
            case .failed(let error):
                throw error
            case .cancelled:
                throw CancellationError()
            case .setup, .waiting, .preparing:
                try await Task<Never, Never>.sleep(for: connectionReadyPollInterval)
            @unknown default:
                try await Task<Never, Never>.sleep(for: connectionReadyPollInterval)
            }
        }
        throw AppleWifiAwareTransportError.connectionReadyTimedOut
    }

    private static func discoverEndpoint(
        for device: WAPairedDevice,
        using browser: NetworkBrowser<WASubscriberBrowser>
    ) async throws -> WAEndpoint {
        try await withThrowingTaskGroup(of: WAEndpoint.self) { group in
            group.addTask {
                try await browser.run { endpoints in
                    guard let endpoint = endpoints.first(where: { $0.device.id == device.id }) else {
                        return .continue
                    }
                    return .finish(endpoint)
                }
            }
            group.addTask {
                try await Task<Never, Never>.sleep(for: connectionReadyTimeout)
                throw AppleWifiAwareTransportError.browserReadyTimedOut
            }
            defer { group.cancelAll() }
            guard let endpoint = try await group.next() else {
                throw AppleWifiAwareTransportError.browserReadyTimedOut
            }
            return endpoint
        }
    }

    static func awaitPeerReady(
        _ peerReadyEvents: AsyncStream<Void>,
        timeout: Duration = peerReadyTimeout,
        retryInterval: Duration = peerHandshakeRetryInterval,
        sendPeerHello: @escaping @Sendable () async throws -> Void
    ) async throws {
        try await withThrowingTaskGroup(of: Bool.self) { group in
            group.addTask {
                for await _ in peerReadyEvents {
                    return true
                }
                return false
            }
            group.addTask {
                var attempt = 0
                while true {
                    try Task.checkCancellation()
                    attempt += 1
                    try await sendPeerHello()
                    if attempt == 1 || attempt.isMultiple(of: 10) {
                        logger.info(
                            "role=sender handshake=peer_hello_sent attempt=\(attempt, privacy: .public)"
                        )
                    }
                    try await Task<Never, Never>.sleep(for: retryInterval)
                }
            }
            group.addTask {
                try await Task<Never, Never>.sleep(for: timeout)
                throw AppleWifiAwareTransportError.peerReadyTimedOut
            }
            defer { group.cancelAll() }
            guard try await group.next() == true else {
                throw AppleWifiAwareTransportError.peerReadyTimedOut
            }
        }
    }

    private static func awaitPeerHello(
        from connection: NetworkConnection<UDP>,
        expected: Data
    ) async throws {
        let received = try await withThrowingTaskGroup(of: Data.self) { group in
            group.addTask {
                try await connection.receive().content
            }
            group.addTask {
                try await Task<Never, Never>.sleep(for: peerHelloTimeout)
                throw AppleWifiAwareTransportError.peerHelloTimedOut
            }
            defer { group.cancelAll() }
            guard let datagram = try await group.next() else {
                throw AppleWifiAwareTransportError.peerHelloTimedOut
            }
            return datagram
        }
        guard received == expected else {
            throw AppleWifiAwareTransportError.invalidPeerHelloDatagram
        }
    }

    static func peerHelloAuthenticator(
        for request: FfiTransferRequest
    ) throws -> String {
        switch request.mode {
        case .room, .invite:
            break
        default:
            throw AppleWifiAwareTransportError.invalidTransferAuthenticator
        }
        guard let roomID = try? transferInvitationRoomId(request: request),
              !roomID.isEmpty else {
            throw AppleWifiAwareTransportError.invalidTransferAuthenticator
        }
        return roomID
    }

    static func peerHelloDatagram(authenticator: String) -> Data {
        guard !authenticator.isEmpty else { return defaultPeerHelloDatagram }
        var transcript = Data("envoix-wifi-aware-session-v1\0".utf8)
        transcript.append(contentsOf: authenticator.utf8)
        var datagram = defaultPeerHelloDatagram
        datagram.append(contentsOf: SHA256.hash(data: transcript))
        return datagram
    }

    private static func requireDatagramCapacity(
        _ connection: NetworkConnection<UDP>
    ) throws -> UInt32 {
        guard connection.maximumDatagramSize >= minimumQUICDatagramSize,
              let maximumDatagramSize = UInt32(exactly: connection.maximumDatagramSize)
        else {
            throw AppleWifiAwareTransportError.insufficientDatagramSize
        }
        return maximumDatagramSize
    }

    private static let minimumQUICDatagramSize = 1_200
    private static let connectionReadyTimeout: Duration = .seconds(20)
    private static let peerHelloTimeout: Duration = .seconds(5)
    static let peerReadyTimeout: Duration = .seconds(20)
    static let peerHandshakeRetryInterval: Duration = .milliseconds(500)
    private static let connectionReadyPollInterval: Duration = .milliseconds(50)
    static let defaultPeerHelloDatagram = Data("envoix-wifi-aware-hello-v1".utf8)
    private static let peerReadyDatagram = Data("envoix-wifi-aware-ready-v1".utf8)

    private static func reportPerformance(
        _ connection: NetworkConnection<UDP>,
        role: String,
        stage: AppleWifiAwarePerformanceStage,
        observer: AppleWifiAwarePerformanceObserver?
    ) async {
        guard let path = connection.currentPath,
              let wifiAwarePath = try? await path.wifiAware
        else {
            let message = "role=\(role) performance_stage=\(stage.rawValue) report=unavailable"
            logger.info("\(message, privacy: .public)")
            return
        }
        let report = wifiAwarePath.performance
        let sample = AppleWifiAwarePerformanceSample(
            stage: stage,
            maximumDatagramSize: connection.maximumDatagramSize,
            throughputCeilingMbps: report.throughputCeiling,
            throughputCapacityMbps: report.throughputCapacity,
            throughputCapacityRatio: report.throughputCapacityRatio,
            signalStrength: report.signalStrength
        )
        let message = "role=\(role) performance_stage=\(stage.rawValue) " +
            "max_datagram=\(sample.maximumDatagramSize) " +
            "ceiling_mbps=\(metric(sample.throughputCeilingMbps)) " +
            "capacity_mbps=\(metric(sample.throughputCapacityMbps)) " +
            "capacity_ratio=\(metric(sample.throughputCapacityRatio)) " +
            "signal_strength=\(metric(sample.signalStrength))"
        logger.info("\(message, privacy: .public)")
        observer?(sample)
    }

    private static func metric(_ value: Double?) -> String {
        value.map { String($0) } ?? "unavailable"
    }

    private static func logListenerState(_ state: NetworkListener<UDP>.State) {
        let detail: String
        switch state {
        case .setup: detail = "setup"
        case .waiting(let error): detail = "waiting:\(error.wifiAware?.wireName ?? "network")"
        case .ready: detail = "ready"
        case .failed(let error): detail = "failed:\(error.wifiAware?.wireName ?? "network")"
        case .cancelled: detail = "cancelled"
        @unknown default: detail = "unknown"
        }
        logger.info("role=receiver listener_state=\(detail, privacy: .public)")
    }

    private static func logBrowserState(_ state: NetworkBrowser<WASubscriberBrowser>.State) {
        let detail: String
        switch state {
        case .setup: detail = "setup"
        case .waiting(let error): detail = "waiting:\(error.wifiAware?.wireName ?? "network")"
        case .ready: detail = "ready"
        case .failed(let error): detail = "failed:\(error.wifiAware?.wireName ?? "network")"
        case .cancelled: detail = "cancelled"
        @unknown default: detail = "unknown"
        }
        logger.info("role=sender browser_state=\(detail, privacy: .public)")
    }

    private static func logConnectionState(
        _ state: NetworkChannel<UDP>.State,
        role: String,
        connection: NetworkConnection<UDP>
    ) {
        let path = connection.currentPath
        let detail: String
        switch state {
        case .setup: detail = "setup"
        case .waiting(let error): detail = "waiting:\(error.wifiAware?.wireName ?? "network")"
        case .preparing: detail = "preparing"
        case .ready: detail = "ready"
        case .failed(let error): detail = "failed:\(error.wifiAware?.wireName ?? "network")"
        case .cancelled: detail = "cancelled"
        @unknown default: detail = "unknown"
        }
        let pathStatus: String
        switch path?.status {
        case .satisfied: pathStatus = "satisfied"
        case .unsatisfied: pathStatus = "unsatisfied"
        case .requiresConnection: pathStatus = "requires_connection"
        case nil: pathStatus = "missing"
        @unknown default: pathStatus = "unknown"
        }
        let interfaceNames = path?.availableInterfaces
            .map(\.name)
            .sorted()
        let interfaces = interfaceNames
            .flatMap { $0.isEmpty ? nil : $0.joined(separator: ",") }
            ?? "none"
        let localEndpoint = path?.localEndpoint == nil ? "missing" : "present"
        let remoteEndpoint = path?.remoteEndpoint == nil ? "missing" : "present"
        let wifiAwareConnection: String
        if #available(iOS 26.4, *) {
            wifiAwareConnection = connection.wifiAware == nil ? "missing" : "present"
        } else {
            wifiAwareConnection = "api_unavailable"
        }
        let message = "role=\(role) connection_state=\(detail) " +
            "path_status=\(pathStatus) interfaces=\(interfaces) " +
            "local_endpoint=\(localEndpoint) remote_endpoint=\(remoteEndpoint) " +
            "wifi_aware_connection=\(wifiAwareConnection)"
        logger.info("\(message, privacy: .public)")
    }

    private static func requireWifiAwarePath(
        _ connection: NetworkConnection<UDP>
    ) async throws {
        guard let path = connection.currentPath,
              try await path.wifiAware != nil
        else {
            throw AppleWifiAwareTransportError.noWifiAwarePath
        }
    }
}

@available(iOS 26.0, *)
actor AppleWifiAwareReceiverAdmission {
    private var claimed = false

    func claim() -> Bool {
        guard !claimed else { return false }
        claimed = true
        return true
    }
}

@available(iOS 26.0, *)
enum AppleWifiAwareDatagramRouter {
    static func shouldForward(
        _ datagram: Data,
        intercepting controlDatagram: Data?
    ) -> Bool {
        datagram != controlDatagram
    }
}

/// Message-preserving UDP adapter. Rust owns iroh QUIC, SPAKE2, Manifest v2,
/// recovery, and final delivery authority.
@available(iOS 26.0, *)
private actor AppleWifiAwareDatagramTransport: FfiNativeDatagramTransport {
    private static let bootstrapMagic = Data("ENVXWA02".utf8)
    private static let logger = Logger(
        subsystem: "com.envoix.app.ios",
        category: "wifi-aware-transport"
    )

    private let connection: NetworkConnection<UDP>
    private let inbox: AppleWifiAwareDatagramInbox
    private let role: String
    private var receiveTask: Task<Void, Never>?
    private var bootstrapSendCount = 0
    private var closed = false

    init(
        _ connection: NetworkConnection<UDP>,
        role: String,
        interceptingControlDatagram: Data? = nil,
        onControlDatagram: (@Sendable () async -> Void)? = nil
    ) {
        self.connection = connection
        self.role = role
        let inbox = AppleWifiAwareDatagramInbox()
        self.inbox = inbox
        receiveTask = Task {
            var bootstrapReceiveCount = 0
            do {
                for try await message in connection.messages {
                    try Task.checkCancellation()
                    if !AppleWifiAwareDatagramRouter.shouldForward(
                        message.content,
                        intercepting: interceptingControlDatagram
                    ) {
                        Self.logger.info(
                            "role=\(role, privacy: .public) control_datagram=intercepted"
                        )
                        await onControlDatagram?()
                        continue
                    }
                    if message.content.starts(with: Self.bootstrapMagic) {
                        bootstrapReceiveCount += 1
                        let attempt = bootstrapReceiveCount
                        if attempt == 1 || attempt.isMultiple(of: 10) {
                            Self.logger.info(
                                "role=\(role, privacy: .public) iroh_bootstrap=received attempt=\(attempt, privacy: .public) bytes=\(message.content.count, privacy: .public)"
                            )
                        }
                    }
                    await inbox.deliver(message.content)
                }
                await inbox.finish(reason: nil)
            } catch is CancellationError {
                await inbox.finish(reason: nil)
            } catch {
                await inbox.finish(reason: String(describing: error))
            }
        }
    }

    func sendDatagram(bytes: Data) async throws {
        guard !closed else {
            throw FfiNativeTransportError.Operation(reason: "Wi-Fi Aware transport is closed")
        }
        guard bytes.count <= connection.maximumDatagramSize else {
            throw Self.project(AppleWifiAwareTransportError.datagramExceedsBound)
        }
        let isBootstrap = bytes.starts(with: Self.bootstrapMagic)
        let bootstrapAttempt: Int?
        if isBootstrap {
            bootstrapSendCount += 1
            bootstrapAttempt = bootstrapSendCount
            if bootstrapSendCount == 1 || bootstrapSendCount.isMultiple(of: 10) {
                Self.logger.info(
                    "role=\(self.role, privacy: .public) iroh_bootstrap=sending attempt=\(self.bootstrapSendCount, privacy: .public) bytes=\(bytes.count, privacy: .public)"
                )
            }
        } else {
            bootstrapAttempt = nil
        }
        do {
            try await connection.send(bytes)
            if let bootstrapAttempt,
               bootstrapAttempt == 1 || bootstrapAttempt.isMultiple(of: 10)
            {
                Self.logger.info(
                    "role=\(self.role, privacy: .public) iroh_bootstrap=sent attempt=\(bootstrapAttempt, privacy: .public)"
                )
            }
        } catch {
            throw Self.project(error)
        }
    }

    func receiveDatagram(maxBytes: UInt32) async throws -> FfiNativeDatagram {
        guard !closed else {
            throw Self.project(AppleWifiAwareTransportError.datagramChannelClosed)
        }
        guard maxBytes > 0, let bound = Int(exactly: maxBytes) else {
            throw Self.project(AppleWifiAwareTransportError.invalidReadBound)
        }
        do {
            return FfiNativeDatagram(bytes: try await inbox.receive(maxBytes: bound))
        } catch {
            throw Self.project(error)
        }
    }

    func close() async throws {
        guard !closed else { return }
        closed = true
        let task = receiveTask
        receiveTask = nil
        await inbox.finish(reason: nil)
        task?.cancel()
        // A failed Wi-Fi Aware path can leave NetworkConnection.messages
        // suspended after cancellation. The inbox is already terminal, so
        // waiting here would turn bounded handshake failures into hung calls.
    }

    nonisolated private static func project(_ error: Error) -> FfiNativeTransportError {
        .Operation(reason: String(describing: error))
    }
}

@available(iOS 26.0, *)
private final class AppleWifiAwareFallbackObserver: TransferObserver, @unchecked Sendable {
    private let downstream: TransferObserver
    private let lock = NSLock()
    private var suppressedFailure: FfiTransferFailure?
    private var terminalCancellation = false
    private var fallbackBoundaryCrossed = false
    private var fallbackActive = false
    private var startedForwarded = false

    init(downstream: TransferObserver) {
        self.downstream = downstream
    }

    var sawTerminalCancellation: Bool {
        lock.lock()
        defer { lock.unlock() }
        return terminalCancellation
    }

    var crossedFallbackBoundary: Bool {
        lock.lock()
        defer { lock.unlock() }
        return fallbackBoundaryCrossed
    }

    func crossFallbackBoundary() {
        lock.lock()
        fallbackBoundaryCrossed = true
        lock.unlock()
    }

    func activateFallback() {
        lock.lock()
        fallbackActive = true
        suppressedFailure = nil
        lock.unlock()
    }

    func forwardSuppressedFailure() {
        lock.lock()
        let failure = suppressedFailure
        suppressedFailure = nil
        lock.unlock()
        if let failure {
            downstream.onTransferFailed(failure: failure)
        }
    }

    func onInviteReady(invite: String) {
        downstream.onInviteReady(invite: invite)
    }

    func onStarted(itemCount: UInt32, totalBytes: UInt64) {
        lock.lock()
        let shouldForward = !startedForwarded
        startedForwarded = true
        lock.unlock()
        if shouldForward {
            downstream.onStarted(itemCount: itemCount, totalBytes: totalBytes)
        }
    }

    func onPhase(phase: FfiManifestV2Phase) {
        if AppleWifiAwareFallbackBoundary.crosses(for: phase) {
            crossFallbackBoundary()
        }
        downstream.onPhase(phase: phase)
    }

    func onProgress(transferred: UInt64, total: UInt64) {
        downstream.onProgress(transferred: transferred, total: total)
    }

    func onCompleted(bytes: UInt64) {
        downstream.onCompleted(bytes: bytes)
    }

    func onTransferFailed(failure: FfiTransferFailure) {
        if AppleWifiAwareFallbackBoundary.crosses(for: failure.recoveryAction) {
            crossFallbackBoundary()
        }
        if failure.code == .userCanceled || failure.code == .senderCanceled {
            lock.lock()
            terminalCancellation = true
            lock.unlock()
            downstream.onTransferFailed(failure: failure)
            return
        }
        lock.lock()
        let shouldForward = fallbackActive
        if !shouldForward {
            suppressedFailure = failure
        }
        lock.unlock()
        if shouldForward {
            downstream.onTransferFailed(failure: failure)
        }
    }

    func onConnectionPath(event: FfiConnectionPathEvent) {
        downstream.onConnectionPath(event: event)
    }

    func onStageTiming(event: FfiTransferStageTiming) {
        downstream.onStageTiming(event: event)
    }

    func onDiagnostic(message: String) {
        if message.hasPrefix("connected via ") {
            crossFallbackBoundary()
        }
        downstream.onDiagnostic(message: message)
    }

    func onRememberedCredential(opaqueCredential: Data, generation: UInt64) -> Bool {
        downstream.onRememberedCredential(
            opaqueCredential: opaqueCredential,
            generation: generation
        )
    }
}

@available(iOS 26.0, *)
actor AppleWifiAwareDatagramInbox {
    private struct Waiter {
        let id: UInt64
        let maxBytes: Int
        let continuation: CheckedContinuation<Data, Error>
    }

    private struct PendingDelivery {
        let datagram: Data
        let continuation: CheckedContinuation<Void, Never>
    }

    private static let queueCapacity = 256

    private var queued: [Data] = []
    private var waiter: Waiter?
    private var pendingDelivery: PendingDelivery?
    private var finished = false
    private var failureReason: String?
    private var nextWaiterID: UInt64 = 0

    func deliver(_ datagram: Data) async {
        guard !finished else { return }
        guard let waiter else {
            if queued.count < Self.queueCapacity {
                queued.append(datagram)
                return
            }
            await withCheckedContinuation { continuation in
                pendingDelivery = PendingDelivery(
                    datagram: datagram,
                    continuation: continuation
                )
            }
            return
        }
        self.waiter = nil
        if datagram.count <= waiter.maxBytes {
            waiter.continuation.resume(returning: datagram)
        } else {
            waiter.continuation.resume(
                throwing: AppleWifiAwareTransportError.datagramExceedsBound
            )
        }
    }

    func finish(reason: String?) {
        guard !finished else { return }
        finished = true
        failureReason = reason
        if let waiter {
            self.waiter = nil
            waiter.continuation.resume(throwing: terminalError())
        }
        if let pendingDelivery {
            self.pendingDelivery = nil
            pendingDelivery.continuation.resume()
        }
    }

    func receive(maxBytes: Int) async throws -> Data {
        if !queued.isEmpty {
            let datagram = queued.removeFirst()
            resumePendingDelivery()
            guard datagram.count <= maxBytes else {
                throw AppleWifiAwareTransportError.datagramExceedsBound
            }
            return datagram
        }
        if finished {
            throw terminalError()
        }
        guard waiter == nil else {
            throw AppleWifiAwareTransportError.concurrentReceive
        }
        let waiterID = nextWaiterID
        nextWaiterID &+= 1
        return try await withTaskCancellationHandler {
            try await withCheckedThrowingContinuation { continuation in
                guard !Task.isCancelled else {
                    continuation.resume(throwing: CancellationError())
                    return
                }
                waiter = Waiter(
                    id: waiterID,
                    maxBytes: maxBytes,
                    continuation: continuation
                )
            }
        } onCancel: {
            Task {
                await self.cancelWaiter(id: waiterID)
            }
        }
    }

    private func cancelWaiter(id: UInt64) {
        guard let waiter, waiter.id == id else { return }
        self.waiter = nil
        waiter.continuation.resume(throwing: CancellationError())
    }

    private func resumePendingDelivery() {
        guard !finished,
              queued.count < Self.queueCapacity,
              let pendingDelivery
        else {
            return
        }
        self.pendingDelivery = nil
        queued.append(pendingDelivery.datagram)
        pendingDelivery.continuation.resume()
    }

    private func terminalError() -> FfiNativeTransportError {
        if let failureReason {
            return .Operation(reason: failureReason)
        }
        return .Operation(
            reason: String(describing: AppleWifiAwareTransportError.datagramChannelClosed)
        )
    }
}
#endif
