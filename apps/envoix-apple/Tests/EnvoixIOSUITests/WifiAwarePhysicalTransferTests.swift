import EnvoixCore
import Foundation
import WiFiAware
import XCTest
@testable import Envoix_iOS

final class WifiAwarePhysicalTransferTests: XCTestCase {
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
                try await AppleWifiAwareTransportSession.withSenderTransport(device: device) { transport in
                    try await transport.send(bytes: request)
                    let response = try await Self.receiveFrame(from: transport)
                    try await transport.close()
                    return response
                }
            }
            try WifiAwareProbeProtocol.validateResponse(response, nonce: nonce)
            Self.marker("raw sender completed run=\(context.runID) bytes=\(response.count)")
        case .receive:
            Self.marker("raw receiver starting run=\(context.runID)")
            try await withProbeTimeout(Self.rawTimeout) {
                try await AppleWifiAwareTransportSession.withReceiverTransport(device: device) { transport in
                    let received = try await Self.receiveFrame(from: transport)
                    let response = try WifiAwareProbeProtocol.makeResponse(for: received)
                    try await transport.send(bytes: response)
                    try await transport.close()
                }
            }
            Self.marker("raw receiver completed run=\(context.runID) bytes=\(request.count)")
        }
    }

    func testManifestV2TransferServicePath() async throws {
        let context = try requirePhysicalContext()
        guard #available(iOS 26.0, *) else {
            throw XCTSkip("Wi-Fi Aware requires iOS or iPadOS 26")
        }
        let peer = try await selectedPeer(
            matching: context.peerHint,
            exactID: context.peerID
        )
        let sourceScopedID = String(peer.id, radix: 16)
        let root = try makeTestRoot(context)
        defer { try? FileManager.default.removeItem(at: root) }
        let stateDirectory = root.appendingPathComponent("state", isDirectory: true)
        try FileManager.default.createDirectory(at: stateDirectory, withIntermediateDirectories: true)
        let observer = WifiAwarePhysicalObserver()

        switch context.role {
        case .send:
            let source = root.appendingPathComponent("wifi-aware-\(context.runID).bin")
            try Self.payload(for: context.runID).write(to: source, options: .atomic)
            let jobStore = root.appendingPathComponent("jobs", isDirectory: true)
            try FileManager.default.createDirectory(at: jobStore, withIntermediateDirectories: true)
            let job = try await createTransferJobV2(
                storeDirectory: jobStore.path,
                compressionPolicy: .never
            )
            let prepared = try await job.addLocalPaths(paths: [source.path])
            XCTAssertEqual(prepared.state, .readyToSend)
            _ = try await job.sealForSend()

            let completion = try await withProbeTimeout(Self.manifestTimeout) {
                try await AppleWifiAwareTransportSession.send(
                    sourceScopedDeviceID: sourceScopedID,
                    job: job,
                    pairingToken: context.pairingToken,
                    stateDirectory: stateDirectory.path,
                    cancellation: FfiManifestV2Cancellation(),
                    observer: observer
                )
            }
            XCTAssertEqual(completion.selectedPath, .wifiAware)
            XCTAssertEqual(completion.transfer.entryCount, 1)
            XCTAssertEqual(completion.transfer.totalPlaintextBytes, UInt64(Self.payload(for: context.runID).count))
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
            Self.marker("manifest receiver starting run=\(context.runID)")
            let completion = try await withProbeTimeout(Self.manifestTimeout) {
                try await AppleWifiAwareTransportSession.receive(
                    sourceScopedDeviceID: sourceScopedID,
                    pairingToken: context.pairingToken,
                    stateDirectory: stateDirectory.path,
                    cancellation: FfiManifestV2Cancellation(),
                    observer: observer
                ) { pending in
                    guard pending.selectedPath() == .wifiAware else {
                        throw WifiAwarePhysicalTestError.unexpectedPath
                    }
                    let summary = pending.summary()
                    guard summary.fileCount == 1, summary.totalPlaintextBytes > 0 else {
                        throw WifiAwarePhysicalTestError.unexpectedOffer
                    }
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
            XCTAssertEqual(completion.entryCount, 1)
            XCTAssertEqual(completion.deliveryProofDigest.count, Self.deliveryProofDigestBytes)
            let savedPath = try XCTUnwrap(completion.savedPaths.first)
            XCTAssertEqual(try Data(contentsOf: URL(fileURLWithPath: savedPath)), Self.payload(for: context.runID))
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
        return WifiAwarePhysicalContext(
            role: role,
            peerHint: peerHint,
            peerID: peerID,
            runID: runID,
            pairingToken: pairingToken
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
    private static func receiveFrame(from transport: FfiNativeDuplexTransport) async throws -> Data {
        var frame = Data()
        while frame.count < WifiAwareProbeProtocol.frameLength {
            let remaining = WifiAwareProbeProtocol.frameLength - frame.count
            let read = try await transport.receive(maxBytes: UInt32(remaining))
            frame.append(read.bytes)
            if read.endOfStream { break }
        }
        guard frame.count == WifiAwareProbeProtocol.frameLength else {
            throw WifiAwarePhysicalTestError.unexpectedEndOfStream
        }
        return frame
    }

    private static func availableCapacity(at directory: URL) throws -> UInt64 {
        let values = try directory.resourceValues(forKeys: [.volumeAvailableCapacityForImportantUsageKey])
        guard let capacity = values.volumeAvailableCapacityForImportantUsage, capacity > 0 else {
            throw WifiAwarePhysicalTestError.missingCapacity
        }
        return UInt64(capacity)
    }

    private static func payload(for runID: String) -> Data {
        let prefix = Data("Envoix Apple Wi-Fi Aware Manifest v2 \(runID)\n".utf8)
        var payload = Data(capacity: 128 * 1_024)
        while payload.count < 128 * 1_024 {
            payload.append(prefix)
        }
        return payload.prefix(128 * 1_024)
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
    private static let rawTimeout: Duration = .seconds(120)
    private static let manifestTimeout: Duration = .seconds(180)
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
}

private enum WifiAwarePhysicalTestError: Error {
    case invalidRole
    case missingPeerHint
    case invalidPeerID
    case peerIDNotFound
    case invalidRunID
    case invalidPairingToken
    case peerMatchCount(Int)
    case unexpectedEndOfStream
    case unexpectedPath
    case unexpectedOffer
    case missingCapacity
}

private final class WifiAwarePhysicalObserver: TransferObserver, @unchecked Sendable {
    private let lock = NSLock()
    private var recordedFailure: String?

    var failure: String? {
        lock.lock()
        defer { lock.unlock() }
        return recordedFailure
    }

    func onInviteReady(invite _: String) {}
    func onStarted(itemCount: UInt32, totalBytes: UInt64) {
        marker("started items=\(itemCount) bytes=\(totalBytes)")
    }
    func onPhase(phase: FfiManifestV2Phase) { marker("phase=\(phase)") }
    func onProgress(transferred: UInt64, total: UInt64) {
        marker("progress=\(transferred)/\(total)")
    }
    func onCompleted(bytes: UInt64) { marker("completed bytes=\(bytes)") }
    func onTransferFailed(failure: FfiTransferFailure) {
        lock.lock()
        recordedFailure = failure.diagnosticMessage
        lock.unlock()
        marker("failed code=\(failure.code) detail=\(failure.diagnosticMessage)")
    }
    func onDiagnostic(message: String) { marker("diagnostic=\(message)") }

    private func marker(_ message: String) {
        FileHandle.standardError.write(Data("[wifi-aware-physical] \(message)\n".utf8))
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
