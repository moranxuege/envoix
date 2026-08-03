#if os(iOS) && canImport(WiFiAware)
import Foundation
import Network
import OSLog

enum AppleWifiAwareRendezvousChannelError: Error, Equatable, LocalizedError {
    case invalidConfiguration
    case alreadyRunning
    case closed
    case duplicateRequest
    case responseTimedOut
    case peerIdentityChanged
    case protocolFailure

    var errorDescription: String? {
        switch self {
        case .invalidConfiguration:
            return "The Wi-Fi Aware control channel configuration is invalid"
        case .alreadyRunning:
            return "The Wi-Fi Aware control channel is already running"
        case .closed:
            return "The Wi-Fi Aware control channel is closed"
        case .duplicateRequest:
            return "A matching Wi-Fi Aware request is already pending"
        case .responseTimedOut:
            return "The nearby device did not acknowledge the Wi-Fi Aware request"
        case .peerIdentityChanged:
            return "The selected Wi-Fi Aware device identity changed"
        case .protocolFailure:
            return "The Wi-Fi Aware control message could not be encoded"
        }
    }
}

struct AppleWifiAwareRendezvousResponseWaiter {
    let key: AppleWifiAwareRendezvousChannelState.ResponseKey

    private let responses: AsyncThrowingStream<
        WifiAwareRendezvousProtocol.Message,
        Error
    >
    private let cancelWait: @Sendable () -> Void

    init(
        key: AppleWifiAwareRendezvousChannelState.ResponseKey,
        responses: AsyncThrowingStream<
            WifiAwareRendezvousProtocol.Message,
            Error
        >,
        cancelWait: @escaping @Sendable () -> Void
    ) {
        self.key = key
        self.responses = responses
        self.cancelWait = cancelWait
    }

    func value() async throws -> WifiAwareRendezvousProtocol.Message {
        try await withTaskCancellationHandler {
            var iterator = responses.makeAsyncIterator()
            let response = try await iterator.next()
            try Task.checkCancellation()
            guard let response else {
                throw AppleWifiAwareRendezvousChannelError.closed
            }
            return response
        } onCancel: {
            cancelWait()
        }
    }
}

actor AppleWifiAwareRendezvousChannelState {
    struct ResponseKey: Hashable {
        let typeRawValue: UInt8
        let requestID: UInt64

        init(
            type: WifiAwareRendezvousProtocol.MessageType,
            requestID: UInt64
        ) {
            typeRawValue = type.rawValue
            self.requestID = requestID
        }
    }

    enum Route: Equatable {
        case handledResponse
        case request(WifiAwareRendezvousProtocol.Message)
        case ignored
    }

    private struct PendingResponse {
        let expectedPeerKey: String?
        let continuation: AsyncThrowingStream<
            WifiAwareRendezvousProtocol.Message,
            Error
        >.Continuation
        let timeoutTask: Task<Void, Never>
    }

    private let closeEvents: AsyncStream<Void>
    private let closeContinuation: AsyncStream<Void>.Continuation
    private var pendingResponses: [ResponseKey: PendingResponse] = [:]
    private var runStarted = false
    private var closed = false

    init() {
        let pair = AsyncStream<Void>.makeStream(
            bufferingPolicy: .bufferingNewest(1)
        )
        closeEvents = pair.stream
        closeContinuation = pair.continuation
    }

    var pendingResponseCount: Int { pendingResponses.count }

    func beginRun() throws {
        guard !closed else {
            throw AppleWifiAwareRendezvousChannelError.closed
        }
        guard !runStarted else {
            throw AppleWifiAwareRendezvousChannelError.alreadyRunning
        }
        runStarted = true
    }

    func requireOpen() throws {
        guard !closed else {
            throw AppleWifiAwareRendezvousChannelError.closed
        }
    }

    func waitUntilClosed() async {
        guard !closed else { return }
        for await _ in closeEvents {
            return
        }
    }

    func registerResponse(
        type: WifiAwareRendezvousProtocol.MessageType,
        requestID: UInt64,
        expectedPeerKey: String?,
        timeout: Duration
    ) throws -> AppleWifiAwareRendezvousResponseWaiter {
        try requireOpen()
        let key = ResponseKey(type: type, requestID: requestID)
        guard pendingResponses[key] == nil else {
            throw AppleWifiAwareRendezvousChannelError.duplicateRequest
        }

        let pair = AsyncThrowingStream<
            WifiAwareRendezvousProtocol.Message,
            Error
        >.makeStream(bufferingPolicy: .bufferingNewest(1))
        let timeoutTask = Task { [weak self] in
            do {
                try await Task<Never, Never>.sleep(for: timeout)
            } catch {
                return
            }
            await self?.expireResponse(key)
        }
        pendingResponses[key] = PendingResponse(
            expectedPeerKey: expectedPeerKey,
            continuation: pair.continuation,
            timeoutTask: timeoutTask
        )
        return AppleWifiAwareRendezvousResponseWaiter(
            key: key,
            responses: pair.stream,
            cancelWait: { [weak self] in
                Task {
                    await self?.cancelResponse(key)
                }
            }
        )
    }

    func route(
        _ message: WifiAwareRendezvousProtocol.Message
    ) -> Route {
        switch message.type {
        case .hello, .invite:
            return .request(message)
        case .helloAck, .inviteAck:
            let key = ResponseKey(
                type: message.type,
                requestID: message.requestID
            )
            guard let pending = pendingResponses.removeValue(forKey: key) else {
                return .ignored
            }
            pending.timeoutTask.cancel()
            if let expectedPeerKey = pending.expectedPeerKey,
               message.senderPeerKey != expectedPeerKey {
                pending.continuation.finish(
                    throwing: AppleWifiAwareRendezvousChannelError
                        .peerIdentityChanged
                )
            } else {
                pending.continuation.yield(message)
                pending.continuation.finish()
            }
            return .handledResponse
        }
    }

    static func acknowledgementKind(
        for message: WifiAwareRendezvousProtocol.Message,
        accepted: Bool
    ) -> WifiAwareRendezvousProtocol.MessageType? {
        guard accepted else { return nil }
        switch message.type {
        case .hello:
            return .hello
        case .invite:
            return .invite
        case .helloAck, .inviteAck:
            return nil
        }
    }

    func abandonResponse(_ key: ResponseKey) {
        guard let pending = pendingResponses.removeValue(forKey: key) else {
            return
        }
        pending.timeoutTask.cancel()
        pending.continuation.finish(
            throwing: AppleWifiAwareRendezvousChannelError.closed
        )
    }

    func close() {
        guard !closed else { return }
        closed = true
        let pending = pendingResponses.values
        pendingResponses.removeAll()
        for response in pending {
            response.timeoutTask.cancel()
            response.continuation.finish(
                throwing: AppleWifiAwareRendezvousChannelError.closed
            )
        }
        closeContinuation.finish()
    }

    private func expireResponse(_ key: ResponseKey) {
        guard let pending = pendingResponses.removeValue(forKey: key) else {
            return
        }
        pending.continuation.finish(
            throwing: AppleWifiAwareRendezvousChannelError.responseTimedOut
        )
    }

    private func cancelResponse(_ key: ResponseKey) {
        guard let pending = pendingResponses.removeValue(forKey: key) else {
            return
        }
        pending.timeoutTask.cancel()
        pending.continuation.finish(throwing: CancellationError())
    }
}

/// A persistent, authenticated, bidirectional control channel over one
/// Wi-Fi Aware UDP connection. `run()` is the only datagram receive loop;
/// outbound requests register response waiters instead of receiving directly.
@available(iOS 26.0, *)
final class AppleWifiAwareRendezvousChannel: @unchecked Sendable {
    typealias InboundHandler = @Sendable (
        WifiAwareRendezvousProtocol.Message
    ) async -> Bool

    static let defaultResponseTimeout: Duration = .seconds(8)
    static let maximumSendAttempts = 3

    let channelID: UUID
    let deviceID: UInt64
    let maximumFrameBytes: Int

    var id: UUID { channelID }

    private let connection: NetworkConnection<UDP>
    private let derivedKey: Data
    private let localIdentity: LocalNearbyDiscoveryIdentity
    private let responseTimeout: Duration
    private let bootstrapReplay: (request: Data, response: Data)?
    private let handleHello: InboundHandler
    private let handleInvite: InboundHandler
    private let state = AppleWifiAwareRendezvousChannelState()
    private let assembler: WifiAwareRendezvousProtocol.Assembler
    private let logger = Logger(
        subsystem: Bundle.main.bundleIdentifier ?? "com.envoix.app",
        category: "wifi-aware-control-channel"
    )

    init(
        connection: NetworkConnection<UDP>,
        derivedKey: Data,
        localIdentity: LocalNearbyDiscoveryIdentity,
        channelID: UUID = UUID(),
        deviceID: UInt64,
        maximumFrameBytes: Int,
        responseTimeout: Duration = defaultResponseTimeout,
        bootstrapReplay: (request: Data, response: Data)? = nil,
        handleHello: @escaping InboundHandler,
        handleInvite: @escaping InboundHandler
    ) throws {
        guard !derivedKey.isEmpty,
              maximumFrameBytes
                > WifiAwareRendezvousProtocol.frameHeaderSize,
              responseTimeout > .zero,
              bootstrapReplay?.request.isEmpty != true,
              bootstrapReplay?.response.isEmpty != true,
              WifiAwareRendezvousProtocol.encodeIdentity(
                  identity: localIdentity,
                  requestID: 0,
                  key: derivedKey,
                  maximumFrameBytes: maximumFrameBytes
              ) != nil else {
            throw AppleWifiAwareRendezvousChannelError.invalidConfiguration
        }
        self.connection = connection
        self.derivedKey = derivedKey
        self.localIdentity = localIdentity
        self.channelID = channelID
        self.deviceID = deviceID
        self.maximumFrameBytes = maximumFrameBytes
        self.responseTimeout = responseTimeout
        self.bootstrapReplay = bootstrapReplay
        self.handleHello = handleHello
        self.handleInvite = handleInvite
        assembler = WifiAwareRendezvousProtocol.Assembler(key: derivedKey)
    }

    func run() async throws {
        try await state.beginRun()
        logger.info(
            "WFA_CHANNEL state=running channel=\(self.channelID.uuidString, privacy: .public)"
        )
        do {
            try await withThrowingTaskGroup(of: Void.self) { group in
                group.addTask { [self] in
                    try await receiveLoop()
                }
                group.addTask { [state] in
                    await state.waitUntilClosed()
                    throw AppleWifiAwareRendezvousChannelError.closed
                }
                defer { group.cancelAll() }
                guard let _ = try await group.next() else {
                    throw AppleWifiAwareRendezvousChannelError.closed
                }
            }
        } catch {
            await state.close()
            logger.info(
                "WFA_CHANNEL state=closed channel=\(self.channelID.uuidString, privacy: .public)"
            )
            if Task.isCancelled {
                throw CancellationError()
            }
            throw error
        }
        await state.close()
    }

    func stop() async {
        await state.close()
    }

    func identify(
        localIdentity: LocalNearbyDiscoveryIdentity
    ) async throws -> LocalNearbyDiscoveryIdentity {
        let requestID = UInt64.random(in: UInt64.min...UInt64.max)
        guard let frames = WifiAwareRendezvousProtocol.encodeIdentity(
            identity: localIdentity,
            requestID: requestID,
            key: derivedKey,
            maximumFrameBytes: maximumFrameBytes
        ) else {
            throw AppleWifiAwareRendezvousChannelError.protocolFailure
        }
        let response = try await sendRequest(
            frames: frames,
            responseType: .helloAck,
            requestID: requestID,
            expectedPeerKey: nil
        )
        return response.senderIdentity
    }

    func heartbeat(
        localIdentity: LocalNearbyDiscoveryIdentity,
        expectedPeerKey: String
    ) async throws {
        let peerKey = try normalizedPeerKey(expectedPeerKey)
        let requestID = UInt64.random(in: UInt64.min...UInt64.max)
        guard let frames = WifiAwareRendezvousProtocol.encodeIdentity(
            identity: localIdentity,
            requestID: requestID,
            key: derivedKey,
            maximumFrameBytes: maximumFrameBytes
        ) else {
            throw AppleWifiAwareRendezvousChannelError.protocolFailure
        }
        _ = try await sendRequest(
            frames: frames,
            responseType: .helloAck,
            requestID: requestID,
            expectedPeerKey: peerKey
        )
    }

    func sendInvite(
        _ invite: String,
        localIdentity: LocalNearbyDiscoveryIdentity,
        expectedPeerKey: String
    ) async throws {
        let peerKey = try normalizedPeerKey(expectedPeerKey)
        let requestID = UInt64.random(in: UInt64.min...UInt64.max)
        guard let frames = WifiAwareRendezvousProtocol.encodeInvite(
            identity: localIdentity,
            invite: invite,
            requestID: requestID,
            key: derivedKey,
            maximumFrameBytes: maximumFrameBytes
        ) else {
            throw AppleWifiAwareRendezvousChannelError.protocolFailure
        }
        _ = try await sendRequest(
            frames: frames,
            responseType: .inviteAck,
            requestID: requestID,
            expectedPeerKey: peerKey
        )
    }

    private func receiveLoop() async throws {
        while !Task.isCancelled {
            let datagram = try await connection.receive().content
            if let bootstrapReplay,
               datagram == bootstrapReplay.request {
                try await connection.send(bootstrapReplay.response)
                continue
            }
            guard let message = assembler.accept(
                datagram,
                nowMilliseconds: Self.monotonicMilliseconds()
            ) else {
                continue
            }
            switch await state.route(message) {
            case .handledResponse, .ignored:
                continue
            case .request(let request):
                try await handleInboundRequest(request)
            }
        }
        throw CancellationError()
    }

    private func handleInboundRequest(
        _ message: WifiAwareRendezvousProtocol.Message
    ) async throws {
        let accepted: Bool
        switch message.type {
        case .hello:
            accepted = await handleHello(message)
        case .invite:
            accepted = await handleInvite(message)
        case .helloAck, .inviteAck:
            return
        }
        guard let kind = AppleWifiAwareRendezvousChannelState
            .acknowledgementKind(for: message, accepted: accepted) else {
            return
        }
        guard let frames = WifiAwareRendezvousProtocol.encodeAck(
            identity: localIdentity,
            acknowledging: message.requestID,
            kind: kind,
            key: derivedKey,
            maximumFrameBytes: maximumFrameBytes
        ) else {
            throw AppleWifiAwareRendezvousChannelError.protocolFailure
        }
        try await send(frames)
    }

    private func sendRequest(
        frames: [Data],
        responseType: WifiAwareRendezvousProtocol.MessageType,
        requestID: UInt64,
        expectedPeerKey: String?
    ) async throws -> WifiAwareRendezvousProtocol.Message {
        var lastTimeout: AppleWifiAwareRendezvousChannelError?
        for _ in 0..<Self.maximumSendAttempts {
            try Task.checkCancellation()
            let waiter = try await state.registerResponse(
                type: responseType,
                requestID: requestID,
                expectedPeerKey: expectedPeerKey,
                timeout: responseTimeout
            )
            do {
                try await send(frames)
            } catch {
                await state.abandonResponse(waiter.key)
                throw error
            }
            do {
                let response = try await waiter.value()
                try Task.checkCancellation()
                return response
            } catch AppleWifiAwareRendezvousChannelError.responseTimedOut {
                lastTimeout = .responseTimedOut
            }
        }
        throw lastTimeout ?? AppleWifiAwareRendezvousChannelError.responseTimedOut
    }

    private func send(_ frames: [Data]) async throws {
        for frame in frames {
            try Task.checkCancellation()
            try await state.requireOpen()
            try await connection.send(frame)
        }
    }

    private func normalizedPeerKey(_ value: String) throws -> String {
        guard let peerKey = NearbyDiscoveryPeerRegistry.normalizePeerKey(value) else {
            throw AppleWifiAwareRendezvousChannelError.protocolFailure
        }
        return peerKey
    }

    private static func monotonicMilliseconds() -> Int64 {
        Int64(ProcessInfo.processInfo.systemUptime * 1_000)
    }
}
#endif
