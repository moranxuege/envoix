import EnvoixCore
import Foundation
import Network
import WiFiAware
import XCTest
@testable import Envoix_iOS

final class WifiAwarePhysicalTransferTests: XCTestCase {
    func testRawUDPServicePath() async throws {
        let context = try requirePhysicalContext()
        guard #available(iOS 26.0, *) else {
            throw XCTSkip("Wi-Fi Aware requires iOS or iPadOS 26")
        }
        let device = try await selectedPeer(
            matching: context.peerHint,
            exactID: context.peerID
        )
        let nonce = ProbeNonce.bytes(for: context.runID)
        let request = try WifiAwareProbeProtocol.makeRequest(nonce: nonce)

        switch context.role {
        case .send:
            let response = try await withProbeTimeout(Self.rawTimeout) {
                try await Self.withUDPSender(device: device) { connection in
                    try await Self.exchangeUDPProbe(request, over: connection)
                }
            }
            try WifiAwareProbeProtocol.validateResponse(response, nonce: nonce)
            Self.marker("udp sender completed run=\(context.runID) bytes=\(response.count)")
        case .receive:
            Self.marker("udp receiver starting run=\(context.runID)")
            try await withProbeTimeout(Self.rawTimeout) {
                try await Self.withUDPReceiver(device: device) { connection in
                    let received = try await Self.receiveUDPMessage(from: connection)
                    let response = try WifiAwareProbeProtocol.makeResponse(for: received)
                    try await connection.send(response)
                    try await Self.requireWifiAwarePath(connection)
                }
            }
            Self.marker("udp receiver completed run=\(context.runID) bytes=\(request.count)")
        }
    }

    func testRawTransferServicePath() async throws {
        let context = try requirePhysicalContext()
        guard #available(iOS 26.0, *) else {
            throw XCTSkip("Wi-Fi Aware requires iOS or iPadOS 26")
        }
        let device = try await selectedPeer(
            matching: context.peerHint,
            exactID: context.peerID
        )
        let nonce = ProbeNonce.bytes(for: context.runID)
        let request = try WifiAwareProbeProtocol.makeRequest(nonce: nonce)

        switch context.role {
        case .send:
            let response = try await withProbeTimeout(Self.rawTimeout) {
                try await AppleWifiAwareTransportSession.withSenderTransport(device: device) {
                    transport, _ in
                    try await transport.sendDatagram(bytes: request)
                    let response = try await Self.receiveDatagram(from: transport)
                    try await transport.close()
                    return response
                }
            }
            try WifiAwareProbeProtocol.validateResponse(response, nonce: nonce)
            Self.marker("raw sender completed run=\(context.runID) bytes=\(response.count)")
        case .receive:
            Self.marker("raw receiver starting run=\(context.runID)")
            try await withProbeTimeout(Self.rawTimeout) {
                try await AppleWifiAwareTransportSession.withReceiverTransport(device: device) {
                    transport, _ in
                    let received = try await Self.receiveDatagram(from: transport)
                    let response = try WifiAwareProbeProtocol.makeResponse(for: received)
                    try await transport.sendDatagram(bytes: response)
                    try await transport.close()
                }
            }
            Self.marker("raw receiver completed run=\(context.runID) bytes=\(request.count)")
        }
    }

    func testManifestV2TransferServicePath() async throws {
        let context = try requirePhysicalContext()
        let timeline = WifiAwarePhysicalTimeline(
            runID: context.runID,
            role: context.role.rawValue
        )
        timeline.mark("test_started payload_bytes=\(context.payloadBytes)")
        guard #available(iOS 26.0, *) else {
            throw XCTSkip("Wi-Fi Aware requires iOS or iPadOS 26")
        }
        let peer = try await selectedPeer(
            matching: context.peerHint,
            exactID: context.peerID
        )
        timeline.mark("peer_selected peer_id=\(String(peer.id, radix: 16))")
        let sourceScopedID = String(peer.id, radix: 16)
        let root = try makeTestRoot(context)
        defer { try? FileManager.default.removeItem(at: root) }
        let stateDirectory = root.appendingPathComponent("state", isDirectory: true)
        try FileManager.default.createDirectory(at: stateDirectory, withIntermediateDirectories: true)
        let observer = WifiAwarePhysicalObserver(timeline: timeline)

        switch context.role {
        case .send:
            let source = root.appendingPathComponent("wifi-aware-\(context.runID).bin")
            try Self.writePayload(
                to: source,
                runID: context.runID,
                byteCount: context.payloadBytes
            )
            timeline.mark("payload_materialized")
            let jobStore = root.appendingPathComponent("jobs", isDirectory: true)
            try FileManager.default.createDirectory(at: jobStore, withIntermediateDirectories: true)
            let job = try await createTransferJobV2(
                storeDirectory: jobStore.path,
                compressionPolicy: .never
            )
            let prepared = try await job.addLocalPaths(paths: [source.path])
            XCTAssertEqual(prepared.state, .readyToSend)
            _ = try await job.sealForSend()
            timeline.mark("job_sealed")

            timeline.mark("session_start")
            let completion = try await withProbeTimeout(context.manifestTimeout) {
                try await AppleWifiAwareTransportSession.send(
                    sourceScopedDeviceID: sourceScopedID,
                    job: job,
                    pairingToken: context.pairingToken,
                    stateDirectory: stateDirectory.path,
                    cancellation: FfiManifestV2Cancellation(),
                    observer: observer,
                    performanceObserver: { sample in
                        timeline.record(sample)
                    }
                )
            }
            timeline.mark("session_completed")
            XCTAssertEqual(completion.selectedPath, .wifiAware)
            XCTAssertEqual(completion.transfer.entryCount, 1)
            XCTAssertEqual(completion.transfer.totalPlaintextBytes, context.payloadBytes)
            XCTAssertEqual(completion.transfer.deliveryProofDigest.count, Self.deliveryProofDigestBytes)
            XCTAssertTrue(completion.transfer.savedPaths.isEmpty)
            XCTAssertNil(observer.failure)
            Self.marker(
                "manifest sender completed run=\(context.runID) " +
                    "bytes=\(completion.transfer.totalPlaintextBytes) path=wifi_aware"
            )
        case .receive:
            let destination = root.appendingPathComponent("received", isDirectory: true)
            try FileManager.default.createDirectory(at: destination, withIntermediateDirectories: true)
            timeline.mark("receiver_waiting")
            let completion = try await withProbeTimeout(context.manifestTimeout) {
                try await AppleWifiAwareTransportSession.receive(
                    sourceScopedDeviceID: sourceScopedID,
                    pairingToken: context.pairingToken,
                    stateDirectory: stateDirectory.path,
                    cancellation: FfiManifestV2Cancellation(),
                    observer: observer,
                    performanceObserver: { sample in
                        timeline.record(sample)
                    }
                ) { pending in
                    guard pending.selectedPath() == .wifiAware else {
                        throw WifiAwarePhysicalTestError.unexpectedPath
                    }
                    let summary = pending.summary()
                    guard summary.fileCount == 1,
                          summary.totalPlaintextBytes == context.payloadBytes
                    else {
                        throw WifiAwarePhysicalTestError.unexpectedOffer
                    }
                    timeline.mark("offer_received")
                    return FfiDestinationRequestV2(
                        targetDirectory: destination.path,
                        copyStagingDirectory: nil,
                        decision: .saveDirectly,
                        targetAllocatableBytes: try Self.availableCapacity(at: destination),
                        stagingAllocatableBytes: nil,
                        stableObjectIdentity: true,
                        exceptionalTransferApproved: false
                    )
                }
            }
            timeline.mark("session_completed")
            XCTAssertEqual(completion.entryCount, 1)
            XCTAssertEqual(completion.deliveryProofDigest.count, Self.deliveryProofDigestBytes)
            let savedPath = try XCTUnwrap(completion.savedPaths.first)
            try Self.verifyPayload(
                at: URL(fileURLWithPath: savedPath),
                runID: context.runID,
                byteCount: context.payloadBytes
            )
            timeline.mark("payload_verified")
            XCTAssertNil(observer.failure)
            Self.marker(
                "manifest receiver saved run=\(context.runID) " +
                    "bytes=\(completion.totalPlaintextBytes) path=wifi_aware"
            )
        }
    }

    private func requirePhysicalContext() throws -> WifiAwarePhysicalContext {
        let environment = ProcessInfo.processInfo.environment
        guard environment[Self.enabledEnvironment] == "1" else {
            throw XCTSkip("Wi-Fi Aware physical tests require \(Self.enabledEnvironment)=1")
        }
        guard let role = WifiAwarePhysicalRole(rawValue: environment[Self.roleEnvironment] ?? "") else {
            throw WifiAwarePhysicalTestError.invalidRole
        }
        let peerHint = environment[Self.peerHintEnvironment]?
            .trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
        let peerIDText = environment[Self.peerIDEnvironment]?
            .trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
        let runID = environment[Self.runIDEnvironment]?
            .trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
        let pairingToken = environment[Self.pairingTokenEnvironment] ?? ""
        let payloadMiBText = environment[Self.payloadMiBEnvironment]?
            .trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
        let timeoutSecondsText = environment[Self.timeoutSecondsEnvironment]?
            .trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
        guard !peerHint.isEmpty else { throw WifiAwarePhysicalTestError.missingPeerHint }
        let peerID: UInt64?
        if peerIDText.isEmpty {
            peerID = nil
        } else {
            guard let parsedID = UInt64(peerIDText, radix: 16) else {
                throw WifiAwarePhysicalTestError.invalidPeerID
            }
            peerID = parsedID
        }
        guard runID.range(of: #"^[A-Za-z0-9_-]{1,48}$"#, options: .regularExpression) != nil else {
            throw WifiAwarePhysicalTestError.invalidRunID
        }
        guard pairingToken.count >= 16 else {
            throw WifiAwarePhysicalTestError.invalidPairingToken
        }
        let payloadMiB: UInt64
        if payloadMiBText.isEmpty {
            payloadMiB = Self.defaultManifestPayloadMiB
        } else {
            guard let parsedPayloadMiB = UInt64(payloadMiBText),
                  (1 ... Self.maximumManifestPayloadMiB).contains(parsedPayloadMiB)
            else {
                throw WifiAwarePhysicalTestError.invalidPayloadMiB
            }
            payloadMiB = parsedPayloadMiB
        }
        let timeoutSeconds: Int64
        if timeoutSecondsText.isEmpty {
            timeoutSeconds = max(
                Self.defaultManifestTimeoutSeconds,
                Int64(payloadMiB) * Self.manifestTimeoutSecondsPerMiB
            )
        } else {
            guard let parsedTimeoutSeconds = Int64(timeoutSecondsText),
                  (Self.minimumManifestTimeoutSeconds ... Self.maximumManifestTimeoutSeconds)
                      .contains(parsedTimeoutSeconds)
            else {
                throw WifiAwarePhysicalTestError.invalidManifestTimeout
            }
            timeoutSeconds = parsedTimeoutSeconds
        }
        return WifiAwarePhysicalContext(
            role: role,
            peerHint: peerHint,
            peerID: peerID,
            runID: runID,
            pairingToken: pairingToken,
            payloadBytes: payloadMiB * Self.bytesPerMiB,
            manifestTimeout: .seconds(timeoutSeconds)
        )
    }

    @available(iOS 26.0, *)
    private func selectedPeer(
        matching hint: String,
        exactID: WAPairedDevice.ID?
    ) async throws -> WAPairedDevice {
        let devices = try await WAPairedDevice.allDevices.current() ?? [:]
        let matches = devices.values.filter { device in
            [device.name, device.pairingInfo?.pairingName]
                .compactMap { $0?.lowercased() }
                .contains { $0.contains(hint.lowercased()) }
        }
        let peer: WAPairedDevice
        if let exactID {
            guard let exactMatch = devices[exactID], matches.contains(exactMatch) else {
                throw WifiAwarePhysicalTestError.peerIDNotFound
            }
            peer = exactMatch
        } else if matches.count == 1, let uniqueMatch = matches.first {
            peer = uniqueMatch
        } else {
            for candidate in matches.sorted(by: { $0.id < $1.id }) {
                Self.marker(
                    "peer candidate id=\(String(candidate.id, radix: 16)) " +
                        "name=\(candidate.name ?? "<unknown>")"
                )
            }
            throw WifiAwarePhysicalTestError.peerMatchCount(matches.count)
        }
        await MainActor.run {
            XCTContext.runActivity(
                named: "Wi-Fi Aware peer selected: hint=\(hint) " +
                    "id=\(String(peer.id, radix: 16)) paired_device_count=\(devices.count)"
            ) { _ in }
        }
        return peer
    }

    private func makeTestRoot(_ context: WifiAwarePhysicalContext) throws -> URL {
        let root = FileManager.default.temporaryDirectory
            .appendingPathComponent(
                "envoix-wifi-aware-\(context.runID)-\(context.role.rawValue)",
                isDirectory: true
            )
        try? FileManager.default.removeItem(at: root)
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        return root
    }

    @available(iOS 26.0, *)
    private static func receiveDatagram(
        from transport: FfiNativeDatagramTransport
    ) async throws -> Data {
        let datagram = try await transport.receiveDatagram(
            maxBytes: UInt32(WifiAwareProbeProtocol.frameLength)
        )
        guard datagram.bytes.count == WifiAwareProbeProtocol.frameLength else {
            throw WifiAwarePhysicalTestError.unexpectedEndOfStream
        }
        return datagram.bytes
    }

    @available(iOS 26.0, *)
    private static func withUDPReceiver<Result: Sendable>(
        device: WAPairedDevice,
        operation: @escaping @Sendable (NetworkConnection<UDP>) async throws -> Result
    ) async throws -> Result {
        guard let service = WAPublishableService.allServices[envoixWifiAwareService] else {
            throw AppleWifiAwareTransportError.serviceNotDeclared
        }
        let listener: NetworkListener<UDP> = try NetworkListener(
            for: .wifiAware(.connecting(to: service, from: .selected([device]))),
            using: udpParameters()
        )
        .newConnectionLimit(1)
        .onStateUpdate { _, state in
            marker("udp receiver listener_state=\(state)")
        }
        let (results, continuation) = AsyncThrowingStream<Result, Error>.makeStream()

        let listenerTask = Task {
            do {
                try await listener.run { connection in
                    connection.onStateUpdate { _, state in
                        marker("udp receiver connection_state=\(state)")
                    }
                    do {
                        continuation.yield(try await operation(connection))
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
        throw WifiAwarePhysicalTestError.missingUDPResult
    }

    @available(iOS 26.0, *)
    private static func withUDPSender<Result: Sendable>(
        device: WAPairedDevice,
        operation: @escaping @Sendable (NetworkConnection<UDP>) async throws -> Result
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
            marker("udp sender browser_state=\(state)")
        }
        let endpoint: WAEndpoint = try await browser.run { endpoints in
            guard let endpoint = endpoints.first(where: { $0.device.id == device.id }) else {
                return .continue
            }
            return .finish(endpoint)
        }
        let connection = NetworkConnection(
            to: endpoint,
            using: udpParameters()
        )
        .onStateUpdate { _, state in
            marker("udp sender connection_state=\(state)")
        }
        return try await operation(connection)
    }

    @available(iOS 26.0, *)
    private static func awaitUDPReady(
        _ connection: NetworkConnection<UDP>,
        role: String
    ) async throws {
        let clock = ContinuousClock()
        let deadline = clock.now.advanced(by: udpReadyTimeout)
        while clock.now < deadline {
            switch connection.state {
            case .ready:
                marker(
                    "udp \(role) ready max_datagram=\(connection.maximumDatagramSize) " +
                        "connection=\(connection.debugDescription)"
                )
                return
            case .failed(let error):
                throw error
            case .cancelled:
                throw CancellationError()
            case .setup, .waiting, .preparing:
                try await Task<Never, Never>.sleep(for: udpReadyPollInterval)
            @unknown default:
                try await Task<Never, Never>.sleep(for: udpReadyPollInterval)
            }
        }
        throw AppleWifiAwareProbeError.timedOut
    }

    @available(iOS 26.0, *)
    private static func receiveUDPMessage(from connection: NetworkConnection<UDP>) async throws -> Data {
        for try await message in connection.messages {
            return message.content
        }
        throw WifiAwarePhysicalTestError.unexpectedEndOfStream
    }

    @available(iOS 26.0, *)
    private static func exchangeUDPProbe(
        _ request: Data,
        over connection: NetworkConnection<UDP>
    ) async throws -> Data {
        try await withThrowingTaskGroup(of: WifiAwareUDPProbeTaskResult.self) { group in
            group.addTask {
                .response(try await receiveUDPMessage(from: connection))
            }
            group.addTask {
                try await awaitUDPReady(connection, role: "sender")
                try await requireWifiAwarePath(connection)
                var attempt = 0
                while !Task.isCancelled {
                    attempt += 1
                    try await connection.send(request)
                    if attempt == 1 || attempt.isMultiple(of: 10) {
                        marker("udp sender sent attempt=\(attempt) bytes=\(request.count)")
                    }
                    try await Task<Never, Never>.sleep(for: udpRetryInterval)
                }
                return .senderStopped
            }

            defer { group.cancelAll() }
            guard let result = try await group.next() else {
                throw WifiAwarePhysicalTestError.missingUDPResult
            }
            switch result {
            case .response(let response):
                return response
            case .senderStopped:
                throw CancellationError()
            }
        }
    }

    @available(iOS 26.0, *)
    private static func requireWifiAwarePath(_ connection: NetworkConnection<UDP>) async throws {
        guard let path = connection.currentPath,
              try await path.wifiAware != nil
        else {
            throw AppleWifiAwareTransportError.noWifiAwarePath
        }
    }

    @available(iOS 26.0, *)
    private static func udpParameters() -> NWParametersBuilder<UDP> {
        .parameters {
            UDP()
        }
        .wifiAware { $0.performanceMode = .bulk }
    }

    private static func availableCapacity(at directory: URL) throws -> UInt64 {
        let values = try directory.resourceValues(forKeys: [.volumeAvailableCapacityForImportantUsageKey])
        guard let capacity = values.volumeAvailableCapacityForImportantUsage, capacity > 0 else {
            throw WifiAwarePhysicalTestError.missingCapacity
        }
        return UInt64(capacity)
    }

    private static func writePayload(
        to url: URL,
        runID: String,
        byteCount: UInt64
    ) throws {
        guard FileManager.default.createFile(atPath: url.path, contents: nil) else {
            throw WifiAwarePhysicalTestError.payloadFileCreationFailed
        }
        let handle = try FileHandle(forWritingTo: url)
        defer { try? handle.close() }
        let chunk = payloadChunk(for: runID)
        var remaining = byteCount
        while remaining > 0 {
            let count = Int(min(UInt64(chunk.count), remaining))
            let data = count == chunk.count ? chunk : Data(chunk.prefix(count))
            try handle.write(contentsOf: data)
            remaining -= UInt64(count)
        }
    }

    private static func verifyPayload(
        at url: URL,
        runID: String,
        byteCount: UInt64
    ) throws {
        let handle = try FileHandle(forReadingFrom: url)
        defer { try? handle.close() }
        let chunk = payloadChunk(for: runID)
        var remaining = byteCount
        while remaining > 0 {
            let count = Int(min(UInt64(chunk.count), remaining))
            let actual = try handle.read(upToCount: count) ?? Data()
            guard actual.count == count,
                  actual.elementsEqual(chunk.prefix(count))
            else {
                throw WifiAwarePhysicalTestError.payloadMismatch
            }
            remaining -= UInt64(count)
        }
        let trailing = try handle.read(upToCount: 1) ?? Data()
        guard trailing.isEmpty else {
            throw WifiAwarePhysicalTestError.payloadMismatch
        }
    }

    private static func payloadChunk(for runID: String) -> Data {
        let prefix = Data("Envoix Apple Wi-Fi Aware Manifest v2 \(runID)\n".utf8)
        var chunk = Data(capacity: payloadChunkBytes)
        while chunk.count < payloadChunkBytes {
            chunk.append(prefix)
        }
        return Data(chunk.prefix(payloadChunkBytes))
    }

    private static func marker(_ message: String) {
        FileHandle.standardError.write(Data("[wifi-aware-physical] \(message)\n".utf8))
    }

    private static let enabledEnvironment = "ENVOIX_WIFI_AWARE_PHYSICAL"
    private static let roleEnvironment = "ENVOIX_WIFI_AWARE_ROLE"
    private static let peerHintEnvironment = "ENVOIX_WIFI_AWARE_PEER_HINT"
    private static let peerIDEnvironment = "ENVOIX_WIFI_AWARE_PEER_ID"
    private static let runIDEnvironment = "ENVOIX_WIFI_AWARE_RUN_ID"
    private static let pairingTokenEnvironment = "ENVOIX_WIFI_AWARE_PAIRING_TOKEN"
    private static let payloadMiBEnvironment = "ENVOIX_WIFI_AWARE_PAYLOAD_MIB"
    private static let timeoutSecondsEnvironment = "ENVOIX_WIFI_AWARE_TIMEOUT_SECONDS"
    private static let rawTimeout: Duration = .seconds(120)
    private static let udpReadyTimeout: Duration = .seconds(20)
    private static let udpReadyPollInterval: Duration = .milliseconds(50)
    private static let udpRetryInterval: Duration = .milliseconds(500)
    private static let bytesPerMiB: UInt64 = 1_024 * 1_024
    private static let defaultManifestPayloadMiB: UInt64 = 8
    private static let maximumManifestPayloadMiB: UInt64 = 1_024
    private static let minimumManifestTimeoutSeconds: Int64 = 30
    private static let defaultManifestTimeoutSeconds: Int64 = 180
    private static let maximumManifestTimeoutSeconds: Int64 = 7_200
    private static let manifestTimeoutSecondsPerMiB: Int64 = 2
    private static let payloadChunkBytes = 1 * 1_024 * 1_024
    private static let deliveryProofDigestBytes = 32
}

private enum WifiAwarePhysicalRole: String {
    case send
    case receive
}

private struct WifiAwarePhysicalContext {
    let role: WifiAwarePhysicalRole
    let peerHint: String
    let peerID: UInt64?
    let runID: String
    let pairingToken: String
    let payloadBytes: UInt64
    let manifestTimeout: Duration
}

private enum WifiAwarePhysicalTestError: Error {
    case invalidRole
    case missingPeerHint
    case invalidPeerID
    case peerIDNotFound
    case invalidRunID
    case invalidPairingToken
    case invalidPayloadMiB
    case invalidManifestTimeout
    case peerMatchCount(Int)
    case unexpectedEndOfStream
    case unexpectedPath
    case unexpectedOffer
    case missingCapacity
    case missingUDPResult
    case payloadFileCreationFailed
    case payloadMismatch
}

private enum WifiAwareUDPProbeTaskResult: Sendable {
    case response(Data)
    case senderStopped
}

private final class WifiAwarePhysicalTimeline: @unchecked Sendable {
    private let runID: String
    private let role: String
    private let startedAt = ProcessInfo.processInfo.systemUptime
    private let lock = NSLock()

    init(runID: String, role: String) {
        self.runID = runID
        self.role = role
    }

    func mark(_ event: String) {
        lock.lock()
        let elapsedMilliseconds = (ProcessInfo.processInfo.systemUptime - startedAt) * 1_000
        let message = String(
            format: "[wifi-aware-benchmark] run=%@ role=%@ elapsed_ms=%.3f %@\n",
            runID,
            role,
            elapsedMilliseconds,
            event
        )
        FileHandle.standardError.write(Data(message.utf8))
        lock.unlock()
    }

    @available(iOS 26.0, *)
    func record(_ sample: AppleWifiAwarePerformanceSample) {
        mark(
            "performance stage=\(sample.stage.rawValue) " +
                "max_datagram=\(sample.maximumDatagramSize) " +
                "ceiling_mbps=\(metric(sample.throughputCeilingMbps)) " +
                "capacity_mbps=\(metric(sample.throughputCapacityMbps)) " +
                "capacity_ratio=\(metric(sample.throughputCapacityRatio)) " +
                "signal_strength=\(metric(sample.signalStrength))"
        )
    }

    private func metric(_ value: Double?) -> String {
        value.map { String(format: "%.3f", $0) } ?? "unavailable"
    }
}

private final class WifiAwarePhysicalObserver: TransferObserver, @unchecked Sendable {
    private let lock = NSLock()
    private let timeline: WifiAwarePhysicalTimeline
    private var recordedFailure: String?
    private var firstPayloadProgressAt: TimeInterval?
    private var nextProgressPercent = 0

    init(timeline: WifiAwarePhysicalTimeline) {
        self.timeline = timeline
    }

    var failure: String? {
        lock.lock()
        defer { lock.unlock() }
        return recordedFailure
    }

    func onInviteReady(invite _: String) {}
    func onStarted(itemCount: UInt32, totalBytes: UInt64) {
        timeline.mark("started items=\(itemCount) bytes=\(totalBytes)")
    }
    func onPhase(phase: FfiManifestV2Phase) {
        timeline.mark("phase=\(phase)")
    }
    func onProgress(transferred: UInt64, total: UInt64) {
        let now = ProcessInfo.processInfo.systemUptime
        lock.lock()
        if transferred > 0, firstPayloadProgressAt == nil {
            firstPayloadProgressAt = now
        }
        let percent = total > 0 ? Int((transferred * 100) / total) : 100
        let shouldLog = percent >= nextProgressPercent || transferred == total
        while nextProgressPercent <= percent {
            nextProgressPercent += 25
        }
        let firstProgressAt = firstPayloadProgressAt
        lock.unlock()

        if shouldLog {
            timeline.mark("progress=\(transferred)/\(total) percent=\(percent)")
        }
        if transferred == total,
           total > 0,
           let firstProgressAt,
           now > firstProgressAt
        {
            let payloadSeconds = now - firstProgressAt
            let goodputMbps = Double(total) * 8 / payloadSeconds / 1_000_000
            timeline.mark(
                String(
                    format: "payload_completed seconds=%.6f goodput_mbps=%.3f",
                    payloadSeconds,
                    goodputMbps
                )
            )
        }
    }
    func onCompleted(bytes: UInt64) {
        timeline.mark("completed bytes=\(bytes)")
    }
    func onTransferFailed(failure: FfiTransferFailure) {
        lock.lock()
        recordedFailure = failure.diagnosticMessage
        lock.unlock()
        timeline.mark("failed code=\(failure.code) detail=\(failure.diagnosticMessage)")
    }
    func onDiagnostic(message: String) {
        timeline.mark("diagnostic=\(message)")
    }
}

private enum ProbeNonce {
    static func bytes(for value: String) -> Data {
        var state = [UInt8](repeating: 0, count: 32)
        for (index, byte) in value.utf8.enumerated() {
            state[index % state.count] &+= byte &+ UInt8(truncatingIfNeeded: index)
            state[(index * 7) % state.count] ^= byte
        }
        return Data(state)
    }
}
