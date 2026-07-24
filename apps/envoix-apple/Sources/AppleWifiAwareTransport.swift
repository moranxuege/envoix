import Foundation

let envoixWifiAwareTransferService = "_envoix-transfer._tcp"

#if os(iOS) && canImport(WiFiAware)
import EnvoixCore
import Network
import WiFiAware

@available(iOS 26.0, *)
enum AppleWifiAwareTransportError: Error {
    case invalidDeviceID
    case pairedDeviceUnavailable
    case serviceNotDeclared
    case listenerFinishedWithoutResult
    case invalidReadBound
}

/// Keeps the Wi-Fi Aware publisher or subscriber scope alive for the entire
/// canonical Rust transfer session.
@available(iOS 26.0, *)
enum AppleWifiAwareTransportSession {
    private enum PublisherFinished: Error {
        case completed
    }

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
            using: transportParameters()
        )
        .newConnectionLimit(1)
        let result = PublisherResult<Result>()

        do {
            try await listener.run { connection in
                let value = try await operation(AppleWifiAwareNativeTransport(connection))
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
        let endpoint: WAEndpoint = try await browser.run { endpoints in
            guard let endpoint = endpoints.first(where: { $0.device.id == device.id }) else {
                return .continue
            }
            return .finish(endpoint)
        }
        let connection: NetworkConnection<TCP> = NetworkConnection(
            to: endpoint,
            using: transportParameters()
        )
        return try await operation(AppleWifiAwareNativeTransport(connection))
    }

    private static func transportParameters() -> NWParametersBuilder<TCP> {
        .parameters {
            TCP().noDelay(true)
        }
        .wifiAware { $0.performanceMode = .bulk }
        .serviceClass(.background)
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
