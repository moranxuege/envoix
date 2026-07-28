import Foundation

protocol BleRendezvousSecurity {
    var mode: UInt8 { get }
    var logName: String { get }

    func seal(_ plaintext: Data) -> Data
    func open(_ payload: Data) -> Data?
}

/// Experimental carrier only. This mode provides no peer authentication or confidentiality.
struct InsecureBleRendezvousSecurity: BleRendezvousSecurity {
    let mode: UInt8 = 0
    let logName = "none"

    func seal(_ plaintext: Data) -> Data { plaintext }
    func open(_ payload: Data) -> Data? { payload }
}

struct BleRendezvousInvite: Equatable {
    let requestID: String
    let senderPeerKey: String
    let senderDisplayName: String?
    let invite: String
}

enum BleRendezvousProtocol {
    static let serviceUUID = UUID(uuidString: "d5f3a2d8-8f4a-4b33-8a01-000000000001")!
    static let writeCharacteristicUUID = UUID(uuidString: "d5f3a2d8-8f4a-4b33-8a01-000000000002")!

    static let frameHeaderSize = 16
    static let maximumWirePayloadBytes = 4_096
    static let maximumInviteBytes = 2_048
    static let maximumDisplayNameBytes = 192
    static let minimumGATTWriteBytes = 20

    private static let frameMagic: [UInt8] = [0x45, 0x58]
    private static let frameVersion: UInt8 = 1
    private static let frameTypeInvite: UInt8 = 1
    private static let envelopeVersion: UInt8 = 1
    private static let envelopeTypeInvite: UInt8 = 1
    private static let peerKeyBytes = 16
    private static let envelopeFixedBytes = 6 + peerKeyBytes
    private static let invitePrefixes = ["envoix://pair/", "envoix://room/"]

    static func encodeInvite(
        identity: LocalNearbyDiscoveryIdentity,
        invite: String,
        requestID: UInt64,
        maximumFrameBytes: Int,
        security: any BleRendezvousSecurity = InsecureBleRendezvousSecurity()
    ) -> [Data]? {
        guard let peerKey = NearbyDiscoveryPeerRegistry.normalizePeerKey(identity.peerKey) else { return nil }
        let normalizedInvite = invite.trimmingCharacters(in: .whitespacesAndNewlines)
        let inviteBytes = Array(normalizedInvite.utf8)
        guard isSupportedInvite(normalizedInvite),
              !inviteBytes.isEmpty,
              inviteBytes.count <= maximumInviteBytes else {
            return nil
        }
        let nameBytes = boundedUTF8(identity.displayName, maximumBytes: maximumDisplayNameBytes)
        var plaintext: [UInt8] = [envelopeVersion, envelopeTypeInvite]
        plaintext.append(contentsOf: peerKey.utf8)
        plaintext.append(contentsOf: unsignedShort(nameBytes.count))
        plaintext.append(contentsOf: unsignedShort(inviteBytes.count))
        plaintext.append(contentsOf: nameBytes)
        plaintext.append(contentsOf: inviteBytes)
        let sealed = security.seal(Data(plaintext))
        let wirePayload = Data([security.mode]) + sealed
        guard wirePayload.count <= maximumWirePayloadBytes,
              maximumFrameBytes > frameHeaderSize else {
            return nil
        }

        let payloadBytes = Array(wirePayload)
        let chunkCapacity = maximumFrameBytes - frameHeaderSize
        var frames: [Data] = []
        var offset = 0
        while offset < payloadBytes.count {
            let count = min(chunkCapacity, payloadBytes.count - offset)
            var frame = frameMagic + [frameVersion, frameTypeInvite]
            frame.append(contentsOf: requestID.bigEndianBytes)
            frame.append(contentsOf: unsignedShort(payloadBytes.count))
            frame.append(contentsOf: unsignedShort(offset))
            frame.append(contentsOf: payloadBytes[offset..<(offset + count)])
            frames.append(Data(frame))
            offset += count
        }
        return frames
    }

    final class Assembler {
        private let security: any BleRendezvousSecurity
        private var requestID: UInt64?
        private var totalLength = 0
        private var bytes: [UInt8] = []

        init(security: any BleRendezvousSecurity = InsecureBleRendezvousSecurity()) {
            self.security = security
        }

        func accept(_ data: Data) -> BleRendezvousInvite? {
            let frame = Array(data)
            guard frame.count > frameHeaderSize,
                  Array(frame[0..<2]) == frameMagic,
                  frame[2] == frameVersion,
                  frame[3] == frameTypeInvite else {
                reset()
                return nil
            }
            let incomingRequestID = uint64(frame, offset: 4)
            let incomingTotal = unsignedShort(frame, offset: 12)
            let incomingOffset = unsignedShort(frame, offset: 14)
            let chunk = frame[frameHeaderSize...]
            guard incomingTotal > 1,
                  incomingTotal <= maximumWirePayloadBytes,
                  incomingOffset + chunk.count <= incomingTotal else {
                reset()
                return nil
            }
            if incomingOffset == 0 {
                requestID = incomingRequestID
                totalLength = incomingTotal
                bytes = []
                bytes.reserveCapacity(incomingTotal)
            } else if requestID != incomingRequestID || totalLength != incomingTotal || bytes.count != incomingOffset {
                reset()
                return nil
            }
            bytes.append(contentsOf: chunk)
            guard bytes.count == totalLength, let completedRequestID = requestID else { return nil }
            let payload = bytes
            reset()
            guard payload.first == security.mode,
                  let plaintext = security.open(Data(payload.dropFirst())) else {
                return nil
            }
            return decodeEnvelope(requestID: completedRequestID, bytes: Array(plaintext))
        }

        func reset() {
            requestID = nil
            totalLength = 0
            bytes = []
        }
    }

    private static func decodeEnvelope(requestID: UInt64, bytes: [UInt8]) -> BleRendezvousInvite? {
        guard bytes.count >= envelopeFixedBytes,
              bytes[0] == envelopeVersion,
              bytes[1] == envelopeTypeInvite,
              let rawPeerKey = String(bytes: bytes[2..<(2 + peerKeyBytes)], encoding: .ascii),
              let peerKey = NearbyDiscoveryPeerRegistry.normalizePeerKey(rawPeerKey) else {
            return nil
        }
        let nameLength = unsignedShort(bytes, offset: 2 + peerKeyBytes)
        let inviteLength = unsignedShort(bytes, offset: 4 + peerKeyBytes)
        guard nameLength <= maximumDisplayNameBytes,
              inviteLength > 0,
              inviteLength <= maximumInviteBytes,
              envelopeFixedBytes + nameLength + inviteLength == bytes.count else {
            return nil
        }
        let nameStart = envelopeFixedBytes
        let inviteStart = nameStart + nameLength
        guard let name = String(bytes: bytes[nameStart..<inviteStart], encoding: .utf8),
              let invite = String(bytes: bytes[inviteStart...], encoding: .utf8)?
                .trimmingCharacters(in: .whitespacesAndNewlines),
              isSupportedInvite(invite) else {
            return nil
        }
        return BleRendezvousInvite(
            requestID: String(format: "%016llx", requestID),
            senderPeerKey: peerKey,
            senderDisplayName: NearbyDiscoveryPeerRegistry.sanitizeDisplayName(name),
            invite: invite
        )
    }

    private static func isSupportedInvite(_ value: String) -> Bool {
        let normalized = value.lowercased()
        return invitePrefixes.contains { normalized.hasPrefix($0) }
    }

    private static func boundedUTF8(_ value: String, maximumBytes: Int) -> [UInt8] {
        var result: [UInt8] = []
        for character in value.trimmingCharacters(in: .whitespacesAndNewlines) {
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
        bytes[offset..<(offset + MemoryLayout<UInt64>.size)].reduce(0) { ($0 << 8) | UInt64($1) }
    }
}

private extension UInt64 {
    var bigEndianBytes: [UInt8] {
        (0..<MemoryLayout<UInt64>.size).map { index in
            UInt8((self >> UInt64(56 - index * 8)) & 0xff)
        }
    }
}
