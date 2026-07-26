import Foundation

let envoixWifiAwareTransferService = "_envoix._udp"

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
    case invalidReadBound
    case datagramExceedsBound
    case datagramChannelClosed
    case concurrentReceive
    case connectionReadyTimedOut
    case insufficientDatagramSize
    case noWifiAwarePath
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
        let device = try await pairedDevice(sourceScopedID: sourceScopedDeviceID)
        return try await withSenderTransport(
            device: device,
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
                observer: observer
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
        destinationDecision: @escaping @Sendable (
            FfiPendingManifestV2Receive
        ) async throws -> FfiDestinationRequestV2
    ) async throws -> FfiManifestV2Completion {
        let device = try await pairedDevice(sourceScopedID: sourceScopedDeviceID)
        return try await withReceiverTransport(
            device: device,
            performanceObserver: performanceObserver
        ) { transport, maximumDatagramSize in
            let pending = try await receiveTransferOfferV2NearbyHybrid(
                settings: settings,
                request: request,
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
    static func withReceiverTransport<Result: Sendable>(
        device: WAPairedDevice,
        performanceObserver: AppleWifiAwarePerformanceObserver? = nil,
        operation: @escaping @Sendable (
            FfiNativeDatagramTransport,
            UInt32
        ) async throws -> Result
    ) async throws -> Result {
        guard let service = WAPublishableService.allServices[envoixWifiAwareTransferService] else {
            throw AppleWifiAwareTransportError.serviceNotDeclared
        }
        let listener: NetworkListener<UDP> = try NetworkListener(
            for: .wifiAware(.connecting(to: service, from: .selected([device]))),
            using: envoixWifiAwareUDPParameters()
        )
        .newConnectionLimit(1)
        .onStateUpdate { _, state in
            Self.logListenerState(state)
        }
        let (results, continuation) = AsyncThrowingStream<Result, Error>.makeStream()

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
                    let transport = AppleWifiAwareDatagramTransport(connection)
                    do {
                        try await awaitReady(connection, role: "receiver")
                        try await requireWifiAwarePath(connection)
                        let maximumDatagramSize = try requireDatagramCapacity(connection)
                        await reportPerformance(
                            connection,
                            role: "receiver",
                            stage: .ready,
                            observer: performanceObserver
                        )
                        let result = try await operation(transport, maximumDatagramSize)
                        await reportPerformance(
                            connection,
                            role: "receiver",
                            stage: .completed,
                            observer: performanceObserver
                        )
                        continuation.yield(result)
                        continuation.finish()
                    } catch {
                        continuation.finish(throwing: error)
                    }
                }
                continuation.finish()
            } catch is CancellationError {
                continuation.finish()
            } catch {
                continuation.finish(throwing: error)
            }
        }

        do {
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

    /// Sender role: browse the selected paired device, then retain its UDP
    /// connection until Rust has completed or failed the Manifest v2 session.
    static func withSenderTransport<Result: Sendable>(
        device: WAPairedDevice,
        performanceObserver: AppleWifiAwarePerformanceObserver? = nil,
        operation: @escaping @Sendable (
            FfiNativeDatagramTransport,
            UInt32
        ) async throws -> Result
    ) async throws -> Result {
        guard let service = WASubscribableService.allServices[envoixWifiAwareTransferService] else {
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
        let endpoint: WAEndpoint = try await browser.run { endpoints in
            guard let endpoint = endpoints.first(where: { $0.device.id == device.id }) else {
                return .continue
            }
            return .finish(endpoint)
        }
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
        let transport = AppleWifiAwareDatagramTransport(connection)
        try await awaitReady(connection, role: "sender")
        try await requireWifiAwarePath(connection)
        let maximumDatagramSize = try requireDatagramCapacity(connection)
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
        return result
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
    private static let connectionReadyPollInterval: Duration = .milliseconds(50)

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

/// Message-preserving UDP adapter. Rust owns iroh QUIC, SPAKE2, Manifest v2,
/// recovery, and final delivery authority.
@available(iOS 26.0, *)
private actor AppleWifiAwareDatagramTransport: FfiNativeDatagramTransport {
    private let connection: NetworkConnection<UDP>
    private let inbox: AppleWifiAwareDatagramInbox
    private var receiveTask: Task<Void, Never>?
    private var closed = false

    init(_ connection: NetworkConnection<UDP>) {
        self.connection = connection
        let inbox = AppleWifiAwareDatagramInbox()
        self.inbox = inbox
        receiveTask = Task {
            do {
                for try await message in connection.messages {
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
        do {
            try await connection.send(bytes)
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
        receiveTask?.cancel()
        receiveTask = nil
        await inbox.finish(reason: nil)
    }

    nonisolated private static func project(_ error: Error) -> FfiNativeTransportError {
        .Operation(reason: String(describing: error))
    }
}

@available(iOS 26.0, *)
private actor AppleWifiAwareDatagramInbox {
    private struct Waiter {
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
        return try await withCheckedThrowingContinuation { continuation in
            waiter = Waiter(maxBytes: maxBytes, continuation: continuation)
        }
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
