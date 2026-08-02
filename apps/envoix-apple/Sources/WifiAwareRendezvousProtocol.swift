import CryptoKit
import Foundation

/// Authenticated control messages carried by Wi-Fi Aware datagrams.
///
/// Each frame uses this big-endian layout:
/// `EW | version | type | request ID | total length | chunk offset | chunk`.
/// The assembled payload contains a fixed-width peer key, UTF-8 name and
/// content lengths, the two UTF-8 values, and an HMAC-SHA256 tag. The tag
/// covers every invariant header field plus the complete payload body.
enum WifiAwareRendezvousProtocol {
    enum MessageType: UInt8, Equatable {
        case hello = 1
        case helloAck = 2
        case invite = 3
        case inviteAck = 4
    }

    struct Message: Equatable {
        let type: MessageType
        let requestID: UInt64
        let senderIdentity: LocalNearbyDiscoveryIdentity
        let content: String

        var senderPeerKey: String { senderIdentity.peerKey }
        var senderDisplayName: String { senderIdentity.displayName }
    }

    static let frameHeaderSize = 16
    static let maximumWirePayloadBytes = 4_096
    static let maximumInviteBytes = 2_048
    static let maximumDisplayNameBytes = BleRendezvousProtocol.maximumDisplayNameBytes
    static let maximumConcurrentAssemblies = 4
    static let maximumFragmentsPerAssembly = 64
    static let assemblyTimeoutMilliseconds: Int64 = 5_000

    private static let frameMagic: [UInt8] = [0x45, 0x57]
    private static let frameVersion: UInt8 = 1
    private static let peerKeyBytes = 16
    private static let authenticationTagBytes = 32
    private static let bodyFixedBytes = peerKeyBytes + 4
    private static let minimumWirePayloadBytes =
        bodyFixedBytes + 1 + authenticationTagBytes

    static func encodeIdentity(
        identity: LocalNearbyDiscoveryIdentity,
        requestID: UInt64,
        key: SymmetricKey,
        maximumFrameBytes: Int
    ) -> [Data]? {
        encode(
            type: .hello,
            identity: identity,
            content: "",
            requestID: requestID,
            key: key,
            maximumFrameBytes: maximumFrameBytes
        )
    }

    static func encodeIdentity(
        identity: LocalNearbyDiscoveryIdentity,
        requestID: UInt64,
        key: Data,
        maximumFrameBytes: Int
    ) -> [Data]? {
        guard !key.isEmpty else { return nil }
        return encodeIdentity(
            identity: identity,
            requestID: requestID,
            key: SymmetricKey(data: key),
            maximumFrameBytes: maximumFrameBytes
        )
    }

    static func encodeInvite(
        identity: LocalNearbyDiscoveryIdentity,
        invite: String,
        requestID: UInt64,
        key: SymmetricKey,
        maximumFrameBytes: Int
    ) -> [Data]? {
        let normalizedInvite = invite.trimmingCharacters(in: .whitespacesAndNewlines)
        let inviteBytes = Array(normalizedInvite.utf8)
        guard !inviteBytes.isEmpty,
              inviteBytes.count <= maximumInviteBytes,
              BleRendezvousProtocol.isSupportedInvite(normalizedInvite) else {
            return nil
        }
        return encode(
            type: .invite,
            identity: identity,
            content: normalizedInvite,
            requestID: requestID,
            key: key,
            maximumFrameBytes: maximumFrameBytes
        )
    }

    static func encodeInvite(
        identity: LocalNearbyDiscoveryIdentity,
        invite: String,
        requestID: UInt64,
        key: Data,
        maximumFrameBytes: Int
    ) -> [Data]? {
        guard !key.isEmpty else { return nil }
        return encodeInvite(
            identity: identity,
            invite: invite,
            requestID: requestID,
            key: SymmetricKey(data: key),
            maximumFrameBytes: maximumFrameBytes
        )
    }

    static func encodeAck(
        identity: LocalNearbyDiscoveryIdentity,
        acknowledging requestID: UInt64,
        kind: MessageType,
        key: SymmetricKey,
        maximumFrameBytes: Int
    ) -> [Data]? {
        let acknowledgementType: MessageType
        switch kind {
        case .hello:
            acknowledgementType = .helloAck
        case .invite:
            acknowledgementType = .inviteAck
        case .helloAck, .inviteAck:
            return nil
        }
        return encode(
            type: acknowledgementType,
            identity: identity,
            content: "",
            requestID: requestID,
            key: key,
            maximumFrameBytes: maximumFrameBytes
        )
    }

    static func encodeAck(
        identity: LocalNearbyDiscoveryIdentity,
        acknowledging requestID: UInt64,
        kind: MessageType,
        key: Data,
        maximumFrameBytes: Int
    ) -> [Data]? {
        guard !key.isEmpty else { return nil }
        return encodeAck(
            identity: identity,
            acknowledging: requestID,
            kind: kind,
            key: SymmetricKey(data: key),
            maximumFrameBytes: maximumFrameBytes
        )
    }

    final class Assembler {
        private let key: SymmetricKey?
        private var assemblies: [UInt64: PartialAssembly] = [:]

        var inFlightCount: Int { assemblies.count }

        init(key: SymmetricKey) {
            self.key = key.bitCount > 0 ? key : nil
        }

        init(key: Data) {
            self.key = key.isEmpty ? nil : SymmetricKey(data: key)
        }

        func accept(_ data: Data) -> Message? {
            accept(
                data,
                nowMilliseconds: Int64(Date().timeIntervalSince1970 * 1_000)
            )
        }

        func accept(_ data: Data, nowMilliseconds: Int64) -> Message? {
            guard nowMilliseconds >= 0, let key else { return nil }
            expireAssemblies(nowMilliseconds: nowMilliseconds)

            let frame = Array(data)
            guard frame.count > frameHeaderSize,
                  Array(frame[0..<frameMagic.count]) == frameMagic,
                  frame[2] == frameVersion,
                  let type = MessageType(rawValue: frame[3]) else {
                return nil
            }

            let requestID = uint64(frame, offset: 4)
            let totalLength = unsignedShort(frame, offset: 12)
            let offset = unsignedShort(frame, offset: 14)
            let chunk = Data(frame[frameHeaderSize...])
            guard totalLength >= minimumWirePayloadBytes,
                  totalLength <= maximumWirePayloadBytes,
                  offset < totalLength,
                  chunk.count <= totalLength - offset else {
                assemblies.removeValue(forKey: requestID)
                return nil
            }

            var assembly: PartialAssembly
            if let existing = assemblies[requestID] {
                guard existing.type == type,
                      existing.totalLength == totalLength else {
                    assemblies.removeValue(forKey: requestID)
                    return nil
                }
                assembly = existing
            } else {
                guard assemblies.count < maximumConcurrentAssemblies else {
                    return nil
                }
                assembly = PartialAssembly(
                    type: type,
                    totalLength: totalLength,
                    startedAtMilliseconds: nowMilliseconds
                )
            }

            switch assembly.insert(chunk, at: offset) {
            case .inserted, .duplicate:
                break
            case .conflict, .fragmentLimitExceeded:
                assemblies.removeValue(forKey: requestID)
                return nil
            }

            guard assembly.isComplete else {
                assemblies[requestID] = assembly
                return nil
            }
            assemblies.removeValue(forKey: requestID)
            guard let payload = assembly.payload() else { return nil }
            return WifiAwareRendezvousProtocol.decode(
                payload,
                type: type,
                requestID: requestID,
                key: key
            )
        }

        func reset() {
            assemblies.removeAll()
        }

        private func expireAssemblies(nowMilliseconds: Int64) {
            assemblies = assemblies.filter { _, assembly in
                guard nowMilliseconds >= assembly.startedAtMilliseconds else {
                    return false
                }
                return nowMilliseconds - assembly.startedAtMilliseconds
                    < assemblyTimeoutMilliseconds
            }
        }
    }

    private enum InsertionResult {
        case inserted
        case duplicate
        case conflict
        case fragmentLimitExceeded
    }

    private struct PartialAssembly {
        let type: MessageType
        let totalLength: Int
        let startedAtMilliseconds: Int64
        private(set) var chunks: [Int: Data] = [:]
        private(set) var receivedByteCount = 0

        var isComplete: Bool {
            receivedByteCount == totalLength
        }

        mutating func insert(_ chunk: Data, at offset: Int) -> InsertionResult {
            if let existing = chunks[offset] {
                return existing == chunk ? .duplicate : .conflict
            }
            guard chunks.count < maximumFragmentsPerAssembly else {
                return .fragmentLimitExceeded
            }

            let incomingRange = offset..<(offset + chunk.count)
            for (existingOffset, existing) in chunks {
                let existingRange =
                    existingOffset..<(existingOffset + existing.count)
                if incomingRange.overlaps(existingRange) {
                    return .conflict
                }
            }

            chunks[offset] = chunk
            receivedByteCount += chunk.count
            return .inserted
        }

        func payload() -> Data? {
            guard isComplete else { return nil }
            var payload = Data()
            payload.reserveCapacity(totalLength)
            var expectedOffset = 0
            for (offset, chunk) in chunks.sorted(by: { $0.key < $1.key }) {
                guard offset == expectedOffset else { return nil }
                payload.append(chunk)
                expectedOffset += chunk.count
            }
            return expectedOffset == totalLength ? payload : nil
        }
    }

    private static func encode(
        type: MessageType,
        identity: LocalNearbyDiscoveryIdentity,
        content: String,
        requestID: UInt64,
        key: SymmetricKey,
        maximumFrameBytes: Int
    ) -> [Data]? {
        guard key.bitCount > 0,
              maximumFrameBytes > frameHeaderSize,
              let peerKey = NearbyDiscoveryPeerRegistry.normalizePeerKey(
                  identity.peerKey
              ),
              let displayName = normalizedDisplayName(identity.displayName) else {
            return nil
        }

        let nameBytes = boundedUTF8(
            displayName,
            maximumBytes: maximumDisplayNameBytes
        )
        let contentBytes = Array(content.utf8)
        guard !nameBytes.isEmpty,
              nameBytes.count <= maximumDisplayNameBytes else {
            return nil
        }

        var body = Data(peerKey.utf8)
        body.append(contentsOf: unsignedShort(nameBytes.count))
        body.append(contentsOf: unsignedShort(contentBytes.count))
        body.append(contentsOf: nameBytes)
        body.append(contentsOf: contentBytes)

        let totalLength = body.count + authenticationTagBytes
        guard totalLength <= maximumWirePayloadBytes else { return nil }
        let authenticated = authenticationData(
            type: type,
            requestID: requestID,
            totalLength: totalLength,
            body: body
        )
        let authenticationTag = HMAC<SHA256>.authenticationCode(
            for: authenticated,
            using: key
        )
        var payload = body
        payload.append(contentsOf: authenticationTag)

        let payloadBytes = Array(payload)
        let chunkCapacity = maximumFrameBytes - frameHeaderSize
        let fragmentCount =
            (payloadBytes.count + chunkCapacity - 1) / chunkCapacity
        guard fragmentCount <= maximumFragmentsPerAssembly else { return nil }
        var frames: [Data] = []
        var offset = 0
        while offset < payloadBytes.count {
            let count = min(chunkCapacity, payloadBytes.count - offset)
            var frame = frameMagic + [frameVersion, type.rawValue]
            frame.append(contentsOf: uint64Bytes(requestID))
            frame.append(contentsOf: unsignedShort(payloadBytes.count))
            frame.append(contentsOf: unsignedShort(offset))
            frame.append(contentsOf: payloadBytes[offset..<(offset + count)])
            frames.append(Data(frame))
            offset += count
        }
        return frames
    }

    private static func decode(
        _ payload: Data,
        type: MessageType,
        requestID: UInt64,
        key: SymmetricKey
    ) -> Message? {
        guard payload.count >= minimumWirePayloadBytes else { return nil }
        let bodyLength = payload.count - authenticationTagBytes
        let body = Data(payload.prefix(bodyLength))
        let authenticationTag = payload.suffix(authenticationTagBytes)
        let authenticated = authenticationData(
            type: type,
            requestID: requestID,
            totalLength: payload.count,
            body: body
        )
        guard HMAC<SHA256>.isValidAuthenticationCode(
            authenticationTag,
            authenticating: authenticated,
            using: key
        ) else {
            return nil
        }

        let bytes = Array(body)
        guard bytes.count >= bodyFixedBytes + 1,
              let rawPeerKey = String(
                  bytes: bytes[0..<peerKeyBytes],
                  encoding: .ascii
              ),
              let peerKey = NearbyDiscoveryPeerRegistry.normalizePeerKey(
                  rawPeerKey
              ) else {
            return nil
        }
        let nameLength = unsignedShort(bytes, offset: peerKeyBytes)
        let contentLength = unsignedShort(bytes, offset: peerKeyBytes + 2)
        guard nameLength > 0,
              nameLength <= maximumDisplayNameBytes,
              bodyFixedBytes + nameLength + contentLength == bytes.count else {
            return nil
        }

        let nameStart = bodyFixedBytes
        let contentStart = nameStart + nameLength
        guard let rawName = String(
            bytes: bytes[nameStart..<contentStart],
            encoding: .utf8
        ),
              let displayName = normalizedDisplayName(rawName),
              displayName == rawName else {
            return nil
        }

        let rawContentBytes = bytes[contentStart...]
        let content: String
        switch type {
        case .hello, .helloAck, .inviteAck:
            guard contentLength == 0 else { return nil }
            content = ""
        case .invite:
            guard contentLength > 0,
                  contentLength <= maximumInviteBytes,
                  let rawValue = String(
                      bytes: rawContentBytes,
                      encoding: .utf8
                  ) else {
                return nil
            }
            let value = rawValue.trimmingCharacters(
                in: .whitespacesAndNewlines
            )
            guard value == rawValue,
                  !value.isEmpty,
                  BleRendezvousProtocol.isSupportedInvite(value) else {
                return nil
            }
            content = value
        }

        return Message(
            type: type,
            requestID: requestID,
            senderIdentity: LocalNearbyDiscoveryIdentity(
                peerKey: peerKey,
                displayName: displayName
            ),
            content: content
        )
    }

    private static func authenticationData(
        type: MessageType,
        requestID: UInt64,
        totalLength: Int,
        body: Data
    ) -> Data {
        var data = Data(frameMagic)
        data.append(frameVersion)
        data.append(type.rawValue)
        data.append(contentsOf: uint64Bytes(requestID))
        data.append(contentsOf: unsignedShort(totalLength))
        data.append(body)
        return data
    }

    private static func normalizedDisplayName(_ value: String) -> String? {
        guard let displayName = NearbyDiscoveryPeerRegistry.sanitizeDisplayName(
            value
        ),
              displayName.unicodeScalars.allSatisfy({
                  !CharacterSet.controlCharacters.contains($0)
              }) else {
            return nil
        }
        return displayName
    }

    private static func boundedUTF8(
        _ value: String,
        maximumBytes: Int
    ) -> [UInt8] {
        var result: [UInt8] = []
        for character in value {
            let bytes = Array(String(character).utf8)
            guard result.count + bytes.count <= maximumBytes else { break }
            result.append(contentsOf: bytes)
        }
        return result
    }

    private static func unsignedShort(_ value: Int) -> [UInt8] {
        [UInt8((value >> 8) & 0xff), UInt8(value & 0xff)]
    }

    private static func unsignedShort(_ bytes: [UInt8], offset: Int) -> Int {
        (Int(bytes[offset]) << 8) | Int(bytes[offset + 1])
    }

    private static func uint64(_ bytes: [UInt8], offset: Int) -> UInt64 {
        bytes[offset..<(offset + MemoryLayout<UInt64>.size)]
            .reduce(0) { ($0 << 8) | UInt64($1) }
    }

    private static func uint64Bytes(_ value: UInt64) -> [UInt8] {
        (0..<MemoryLayout<UInt64>.size).map { index in
            UInt8((value >> UInt64(56 - index * 8)) & 0xff)
        }
    }
}
