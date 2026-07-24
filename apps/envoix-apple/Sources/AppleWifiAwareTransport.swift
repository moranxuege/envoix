import Foundation

let envoixWifiAwareTransferService = "_envoix-transfer._tcp"

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
    case noWifiAwarePath
}

/// Apple requires matching performance modes on both endpoints. Keep the
/// system-recommended bulk mode explicit and retain the default best-effort
/// service class for file transfer.
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
    private enum PublisherFinished: Error {
        case completed
    }

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

    static func send(
        sourceScopedDeviceID: String,
        job: FfiTransferJobV2,
        pairingToken: String,
        stateDirectory: String,
        cancellation: FfiManifestV2Cancellation,
        observer: TransferObserver
    ) async throws -> FfiNativeManifestV2Completion {
        let device = try await pairedDevice(sourceScopedID: sourceScopedDeviceID)
        return try await withSenderTransport(device: device) { transport in
            try await sendTransferJobV2OverNativeTransport(
                job: job,
                pairingToken: pairingToken,
                stateDirectory: stateDirectory,
                transport: transport,
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
        destinationDecision: @escaping @Sendable (
            FfiPendingManifestV2Receive
        ) async throws -> FfiDestinationRequestV2
    ) async throws -> FfiManifestV2Completion {
        let device = try await pairedDevice(sourceScopedID: sourceScopedDeviceID)
        return try await withReceiverTransport(device: device) { transport in
            let pending = try await receiveTransferOfferV2OverNativeTransport(
                pairingToken: pairingToken,
                stateDirectory: stateDirectory,
                transport: transport,
                cancellation: cancellation,
                observer: observer
            )
            let destination = try await destinationDecision(pending)
            return try await pending.receive(destination: destination, observer: observer)
        }
    }

    /// Receiver role: publish the declared TCP service and retain the listener
    /// until Rust has completed or failed the Manifest v2 session.
    static func withReceiverTransport<Result: Sendable>(
        device: WAPairedDevice,
        operation: @escaping @Sendable (FfiNativeDuplexTransport) async throws -> Result
    ) async throws -> Result {
        guard let service = WAPublishableService.allServices[envoixWifiAwareTransferService] else {
            throw AppleWifiAwareTransportError.serviceNotDeclared
        }
        let listener: NetworkListener<TCP> = try NetworkListener(
            for: .wifiAware(.connecting(to: service, from: .selected([device]))),
            using: envoixWifiAwareTCPParameters()
        )
        .newConnectionLimit(1)
        .onStateUpdate { _, state in
            Self.logListenerState(state)
        }
        let result = PublisherResult<Result>()

        do {
            try await listener.run { connection in
                connection.onStateUpdate { connection, state in
                    Self.logConnectionState(
                        state,
                        role: "receiver",
                        connection: connection
                    )
                }
                Self.logConnectionState(
                    connection.state,
                    role: "receiver",
                    connection: connection
                )
                let value = try await operation(AppleWifiAwareNativeTransport(connection))
                try await requireWifiAwarePath(connection)
                await result.store(value)
                throw PublisherFinished.completed
            }
        } catch PublisherFinished.completed {
            guard let value = await result.take() else {
                throw AppleWifiAwareTransportError.listenerFinishedWithoutResult
            }
            return value
        }
        throw AppleWifiAwareTransportError.listenerFinishedWithoutResult
    }

    /// Sender role: browse the selected paired device, then retain its TCP
    /// connection until Rust has completed or failed the Manifest v2 session.
    static func withSenderTransport<Result: Sendable>(
        device: WAPairedDevice,
        operation: @escaping @Sendable (FfiNativeDuplexTransport) async throws -> Result
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
        let connection: NetworkConnection<TCP> = NetworkConnection(
            to: endpoint,
            using: envoixWifiAwareTCPParameters()
        )
        .onStateUpdate { connection, state in
            Self.logConnectionState(
                state,
                role: "sender",
                connection: connection
            )
        }
        let value = try await operation(AppleWifiAwareNativeTransport(connection))
        try await requireWifiAwarePath(connection)
        return value
    }

    private static func logListenerState(_ state: NetworkListener<TCP>.State) {
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
        _ state: NetworkChannel<TCP>.State,
        role: String,
        connection: NetworkConnection<TCP>
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
        _ connection: NetworkConnection<TCP>
    ) async throws {
        guard let path = connection.currentPath,
              try await path.wifiAware != nil
        else {
            throw AppleWifiAwareTransportError.noWifiAwarePath
        }
    }
}

@available(iOS 26.0, *)
private actor PublisherResult<Value: Sendable> {
    private var value: Value?

    func store(_ value: Value) {
        self.value = value
    }

    func take() -> Value? {
        defer { value = nil }
        return value
    }
}

/// Raw TCP adapter only. Rust owns TLS, SPAKE2, Manifest v2 framing, recovery,
/// and final delivery authority.
@available(iOS 26.0, *)
private actor AppleWifiAwareNativeTransport: FfiNativeDuplexTransport {
    private let connection: NetworkConnection<TCP>
    private var closed = false

    init(_ connection: NetworkConnection<TCP>) {
        self.connection = connection
    }

    func send(bytes: Data) async throws {
        guard !closed else {
            throw FfiNativeTransportError.Operation(reason: "Wi-Fi Aware transport is closed")
        }
        do {
            try await connection.send(bytes)
        } catch {
            throw Self.project(error)
        }
    }

    func receive(maxBytes: UInt32) async throws -> FfiNativeTransportRead {
        guard !closed else {
            return FfiNativeTransportRead(bytes: Data(), endOfStream: true)
        }
        guard maxBytes > 0, let bound = Int(exactly: maxBytes) else {
            throw FfiNativeTransportError.Operation(
                reason: String(describing: AppleWifiAwareTransportError.invalidReadBound)
            )
        }
        do {
            let message = try await connection.receive(atMost: bound)
            return FfiNativeTransportRead(
                bytes: message.content,
                endOfStream: message.metadata.endOfStream
            )
        } catch {
            throw Self.project(error)
        }
    }

    func close() async throws {
        guard !closed else { return }
        closed = true
        do {
            try await connection.send(Data(), endOfStream: true)
        } catch {
            throw Self.project(error)
        }
    }

    nonisolated private static func project(_ error: Error) -> FfiNativeTransportError {
        .Operation(reason: String(describing: error))
    }
}
#endif
