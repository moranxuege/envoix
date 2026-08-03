import CryptoKit
import Foundation
import XCTest
#if os(macOS)
@testable import Envoix
#else
@testable import Envoix_iOS
#endif

final class WifiAwareRendezvousProtocolTests: XCTestCase {
    private static let keyData = Data((0..<32).map(UInt8.init))
    private let identity = LocalNearbyDiscoveryIdentity(
        peerKey: "0011223344556677",
        displayName: "Sender"
    )

    private var key: SymmetricKey {
        SymmetricKey(data: Self.keyData)
    }

    func testRoundTripsAllMessageTypesAndBigEndianHeader() throws {
        let requestID: UInt64 = 0x0102030405060708
        let invite = "envoix://invite/v2/authenticated-round-trip"
        let helloFrames = try XCTUnwrap(
            WifiAwareRendezvousProtocol.encodeIdentity(
                identity: identity,
                requestID: requestID,
                key: key,
                maximumFrameBytes: 27
            )
        )
        let hello = try XCTUnwrap(assemble(helloFrames))

        XCTAssertGreaterThan(helloFrames.count, 1)
        XCTAssertEqual(Array(helloFrames[0].prefix(4)), [0x45, 0x57, 1, 1])
        XCTAssertEqual(
            Array(helloFrames[0][4..<12]),
            [1, 2, 3, 4, 5, 6, 7, 8]
        )
        XCTAssertEqual(hello.type, .hello)
        XCTAssertEqual(hello.requestID, requestID)
        XCTAssertEqual(hello.senderIdentity, identity)
        XCTAssertEqual(hello.senderPeerKey, identity.peerKey)
        XCTAssertEqual(hello.senderDisplayName, identity.displayName)
        XCTAssertEqual(hello.content, "")

        let helloAck = try XCTUnwrap(assemble(try XCTUnwrap(
            WifiAwareRendezvousProtocol.encodeAck(
                identity: identity,
                acknowledging: requestID,
                kind: .hello,
                key: key,
                maximumFrameBytes: 31
            )
        )))
        XCTAssertEqual(helloAck.type, .helloAck)
        XCTAssertEqual(helloAck.requestID, requestID)
        XCTAssertEqual(helloAck.content, "")

        let inviteMessage = try XCTUnwrap(assemble(try XCTUnwrap(
            WifiAwareRendezvousProtocol.encodeInvite(
                identity: identity,
                invite: invite,
                requestID: requestID + 1,
                key: key,
                maximumFrameBytes: 31
            )
        )))
        XCTAssertEqual(inviteMessage.type, .invite)
        XCTAssertEqual(inviteMessage.requestID, requestID + 1)
        XCTAssertEqual(inviteMessage.content, invite)

        let inviteAck = try XCTUnwrap(assemble(try XCTUnwrap(
            WifiAwareRendezvousProtocol.encodeAck(
                identity: identity,
                acknowledging: requestID + 1,
                kind: .invite,
                key: key,
                maximumFrameBytes: 31
            )
        )))
        XCTAssertEqual(inviteAck.type, .inviteAck)
        XCTAssertEqual(inviteAck.requestID, requestID + 1)
        XCTAssertEqual(inviteAck.content, "")
    }

    func testAssemblerAcceptsOutOfOrderFrames() throws {
        let invite = "envoix://invite/v2/out-of-order-" + String(
            repeating: "x",
            count: 80
        )
        let frames = try XCTUnwrap(
            WifiAwareRendezvousProtocol.encodeInvite(
                identity: identity,
                invite: invite,
                requestID: 2,
                key: key,
                maximumFrameBytes: 25
            )
        )

        let decoded = try XCTUnwrap(assemble(Array(frames.reversed())))

        XCTAssertGreaterThan(frames.count, 3)
        XCTAssertEqual(decoded.content, invite)
    }

    func testAssemblerAcceptsExactlyIdenticalDuplicateFrame() throws {
        let frames = try XCTUnwrap(
            WifiAwareRendezvousProtocol.encodeInvite(
                identity: identity,
                invite: "envoix://invite/v2/duplicate-frame",
                requestID: 3,
                key: key,
                maximumFrameBytes: 28
            )
        )
        let assembler = WifiAwareRendezvousProtocol.Assembler(key: key)

        XCTAssertNil(assembler.accept(frames[1], nowMilliseconds: 0))
        XCTAssertNil(assembler.accept(frames[1], nowMilliseconds: 1))
        var decoded: WifiAwareRendezvousProtocol.Message?
        for (index, frame) in frames.enumerated() where index != 1 {
            decoded = assembler.accept(
                frame,
                nowMilliseconds: Int64(index + 2)
            ) ?? decoded
        }

        XCTAssertEqual(decoded?.content, "envoix://invite/v2/duplicate-frame")
        XCTAssertEqual(assembler.inFlightCount, 0)
    }

    func testAssemblerRejectsConflictingOverlapAndRecovers() throws {
        let frames = try XCTUnwrap(
            WifiAwareRendezvousProtocol.encodeInvite(
                identity: identity,
                invite: "envoix://invite/v2/conflicting-overlap",
                requestID: 4,
                key: key,
                maximumFrameBytes: 28
            )
        )
        let assembler = WifiAwareRendezvousProtocol.Assembler(key: key)
        var overlapping = frames[1]
        setUnsignedShort(1, in: &overlapping, offset: 14)

        XCTAssertNil(assembler.accept(frames[0], nowMilliseconds: 0))
        XCTAssertNil(assembler.accept(overlapping, nowMilliseconds: 1))
        XCTAssertEqual(assembler.inFlightCount, 0)

        var decoded: WifiAwareRendezvousProtocol.Message?
        for (index, frame) in frames.enumerated() {
            decoded = assembler.accept(
                frame,
                nowMilliseconds: Int64(index + 2)
            ) ?? decoded
        }
        XCTAssertEqual(decoded?.content, "envoix://invite/v2/conflicting-overlap")
    }

    func testAssemblerRejectsParameterDriftAndOutOfBoundsChunk() throws {
        let frames = try XCTUnwrap(
            WifiAwareRendezvousProtocol.encodeIdentity(
                identity: identity,
                requestID: 5,
                key: key,
                maximumFrameBytes: 24
            )
        )
        let assembler = WifiAwareRendezvousProtocol.Assembler(key: key)
        var changedTotal = frames[1]
        let totalLength = unsignedShort(changedTotal, offset: 12)
        setUnsignedShort(totalLength + 1, in: &changedTotal, offset: 12)

        XCTAssertNil(assembler.accept(frames[0], nowMilliseconds: 0))
        XCTAssertNil(assembler.accept(changedTotal, nowMilliseconds: 1))
        XCTAssertEqual(assembler.inFlightCount, 0)

        var outOfBounds = frames[0]
        setUnsignedShort(totalLength, in: &outOfBounds, offset: 14)
        XCTAssertNil(assembler.accept(outOfBounds, nowMilliseconds: 2))
        XCTAssertEqual(assembler.inFlightCount, 0)
    }

    func testAssemblerCapsConcurrencyAndExpiresAfterFiveSeconds() throws {
        let assembler = WifiAwareRendezvousProtocol.Assembler(key: key)
        var framesByRequestID: [UInt64: [Data]] = [:]
        for requestID in UInt64(10)...UInt64(14) {
            framesByRequestID[requestID] = try XCTUnwrap(
                WifiAwareRendezvousProtocol.encodeIdentity(
                    identity: identity,
                    requestID: requestID,
                    key: key,
                    maximumFrameBytes: 24
                )
            )
        }

        for requestID in UInt64(10)...UInt64(13) {
            let frame = try XCTUnwrap(framesByRequestID[requestID]?.first)
            XCTAssertNil(assembler.accept(frame, nowMilliseconds: 0))
        }
        XCTAssertEqual(
            assembler.inFlightCount,
            WifiAwareRendezvousProtocol.maximumConcurrentAssemblies
        )

        let fifthFrames = try XCTUnwrap(framesByRequestID[14])
        XCTAssertNil(assembler.accept(fifthFrames[0], nowMilliseconds: 4_999))
        XCTAssertEqual(assembler.inFlightCount, 4)

        XCTAssertNil(assembler.accept(fifthFrames[0], nowMilliseconds: 5_000))
        XCTAssertEqual(assembler.inFlightCount, 1)
        var decoded: WifiAwareRendezvousProtocol.Message?
        for frame in fifthFrames.dropFirst() {
            decoded = assembler.accept(
                frame,
                nowMilliseconds: 5_000
            ) ?? decoded
        }
        XCTAssertEqual(decoded?.type, .hello)
        XCTAssertEqual(decoded?.requestID, 14)
        XCTAssertEqual(assembler.inFlightCount, 0)
    }

    func testAssemblerRejectsExcessiveFragmentCount() throws {
        let completeFrame = try XCTUnwrap(
            WifiAwareRendezvousProtocol.encodeInvite(
                identity: identity,
                invite: "envoix://invite/v2/" + String(
                    repeating: "x",
                    count: WifiAwareRendezvousProtocol.maximumFragmentsPerAssembly
                ),
                requestID: 15,
                key: key,
                maximumFrameBytes: 4_096
            )?.first
        )
        let totalLength = unsignedShort(completeFrame, offset: 12)
        XCTAssertGreaterThan(
            totalLength,
            WifiAwareRendezvousProtocol.maximumFragmentsPerAssembly
        )

        let assembler = WifiAwareRendezvousProtocol.Assembler(key: key)
        for offset in 0..<WifiAwareRendezvousProtocol.maximumFragmentsPerAssembly {
            XCTAssertNil(assembler.accept(
                oneByteFragment(completeFrame, offset: offset),
                nowMilliseconds: Int64(offset)
            ))
        }
        XCTAssertEqual(assembler.inFlightCount, 1)

        XCTAssertNil(assembler.accept(
            oneByteFragment(
                completeFrame,
                offset: WifiAwareRendezvousProtocol.maximumFragmentsPerAssembly
            ),
            nowMilliseconds: Int64(
                WifiAwareRendezvousProtocol.maximumFragmentsPerAssembly
            )
        ))
        XCTAssertEqual(assembler.inFlightCount, 0)
        XCTAssertNil(WifiAwareRendezvousProtocol.encodeInvite(
            identity: identity,
            invite: "envoix://invite/v2/" + String(
                repeating: "x",
                count: WifiAwareRendezvousProtocol.maximumFragmentsPerAssembly
            ),
            requestID: 16,
            key: key,
            maximumFrameBytes: WifiAwareRendezvousProtocol.frameHeaderSize + 1
        ))
    }

    func testRejectsAuthenticationTagMutation() throws {
        var frame = try XCTUnwrap(
            WifiAwareRendezvousProtocol.encodeInvite(
                identity: identity,
                invite: "envoix://invite/v2/mac-mutation",
                requestID: 20,
                key: key,
                maximumFrameBytes: 4_096
            )?.first
        )
        frame[frame.count - 1] ^= 0xff

        let assembler = WifiAwareRendezvousProtocol.Assembler(key: key)
        XCTAssertNil(assembler.accept(frame, nowMilliseconds: 0))
        XCTAssertEqual(assembler.inFlightCount, 0)
    }

    func testRejectsOversizedPayloadsAndFrames() throws {
        let prefix = "envoix://invite/v2/"
        let oversizedInvite = prefix + String(
            repeating: "x",
            count: WifiAwareRendezvousProtocol.maximumInviteBytes
                - prefix.utf8.count + 1
        )
        XCTAssertNil(WifiAwareRendezvousProtocol.encodeInvite(
            identity: identity,
            invite: oversizedInvite,
            requestID: 21,
            key: key,
            maximumFrameBytes: 4_096
        ))
        XCTAssertNil(WifiAwareRendezvousProtocol.encodeIdentity(
            identity: identity,
            requestID: 22,
            key: key,
            maximumFrameBytes: WifiAwareRendezvousProtocol.frameHeaderSize
        ))

        var frame = try XCTUnwrap(
            WifiAwareRendezvousProtocol.encodeIdentity(
                identity: identity,
                requestID: 23,
                key: key,
                maximumFrameBytes: 4_096
            )?.first
        )
        setUnsignedShort(
            WifiAwareRendezvousProtocol.maximumWirePayloadBytes + 1,
            in: &frame,
            offset: 12
        )
        XCTAssertNil(
            WifiAwareRendezvousProtocol.Assembler(key: key)
                .accept(frame, nowMilliseconds: 0)
        )
    }

    func testPreservesValidUTF8NameAndInvite() throws {
        let utf8Identity = LocalNearbyDiscoveryIdentity(
            peerKey: "AABBCCDDEEFF0011",
            displayName: "研究设备 🚀"
        )
        let invite = "envoix://invite/v2/邀请-🚀"
        let frames = try XCTUnwrap(
            WifiAwareRendezvousProtocol.encodeInvite(
                identity: utf8Identity,
                invite: invite,
                requestID: 24,
                key: Self.keyData,
                maximumFrameBytes: 29
            )
        )
        let assembler = WifiAwareRendezvousProtocol.Assembler(key: Self.keyData)
        var decoded: WifiAwareRendezvousProtocol.Message?
        for (index, frame) in frames.reversed().enumerated() {
            decoded = assembler.accept(
                frame,
                nowMilliseconds: Int64(index)
            ) ?? decoded
        }

        XCTAssertEqual(decoded?.senderPeerKey, "aabbccddeeff0011")
        XCTAssertEqual(decoded?.senderDisplayName, utf8Identity.displayName)
        XCTAssertEqual(decoded?.content, invite)
    }

    func testRejectsUnsupportedInviteAndInvalidAckKind() {
        for invite in [
            "123456-a1b2-c3d4",
            "envoix://pair/123456-a1b2-c3d4",
            "envoix://invite/v2/",
        ] {
            XCTAssertNil(
                WifiAwareRendezvousProtocol.encodeInvite(
                    identity: identity,
                    invite: invite,
                    requestID: 25,
                    key: key,
                    maximumFrameBytes: 512
                ),
                "Accepted unsupported invite \(invite)"
            )
        }
        XCTAssertNil(WifiAwareRendezvousProtocol.encodeAck(
            identity: identity,
            acknowledging: 25,
            kind: .helloAck,
            key: key,
            maximumFrameBytes: 512
        ))
    }

    func testInboundConnectionAdmissionBoundsConcurrentWork() {
        var admission = WifiAwareInboundConnectionAdmission()
        var tokens: [WifiAwareInboundConnectionAdmission.Token] = []

        for _ in 0..<WifiAwareInboundConnectionAdmission.maximumConcurrentConnections {
            tokens.append(admission.acquire()!)
        }
        XCTAssertNil(admission.acquire())

        admission.release(tokens[0])
        admission.release(tokens[0])
        XCTAssertNotNil(admission.acquire())
        XCTAssertEqual(
            admission.activeConnectionCount,
            WifiAwareInboundConnectionAdmission.maximumConcurrentConnections
        )
    }

    func testInboundConnectionAdmissionPromotionIsIdempotent() throws {
        var admission = WifiAwareInboundConnectionAdmission()
        let token = try XCTUnwrap(admission.acquire())
        let channelID = UUID()

        XCTAssertTrue(admission.markPending(token, for: channelID))
        XCTAssertEqual(admission.activeConnectionCount, 1)
        XCTAssertEqual(admission.pendingConnectionCount, 1)
        XCTAssertFalse(admission.markPending(token, for: UUID()))

        XCTAssertTrue(admission.releasePending(for: channelID))
        XCTAssertEqual(admission.activeConnectionCount, 0)
        XCTAssertEqual(admission.pendingConnectionCount, 0)
        XCTAssertFalse(admission.releasePending(for: channelID))
        admission.release(token)
        XCTAssertEqual(admission.activeConnectionCount, 0)
    }

    func testInboundConnectionAdmissionResetReleasesPreAuthenticationWork() {
        var admission = WifiAwareInboundConnectionAdmission()
        guard let pendingToken = admission.acquire(),
              let preAuthenticationToken = admission.acquire() else {
            return XCTFail("Could not allocate stale admission tokens")
        }
        XCTAssertTrue(admission.markPending(pendingToken, for: UUID()))

        admission.reset()
        XCTAssertEqual(admission.activeConnectionCount, 0)
        XCTAssertEqual(admission.pendingConnectionCount, 0)

        let maximum = WifiAwareInboundConnectionAdmission
            .maximumConcurrentConnections
        let replacementTokens = (0..<maximum).compactMap { _ in
            admission.acquire()
        }
        XCTAssertEqual(replacementTokens.count, maximum)
        admission.release(pendingToken)
        admission.release(preAuthenticationToken)
        XCTAssertEqual(admission.activeConnectionCount, maximum)
        XCTAssertNil(admission.acquire())
    }

    func testRendezvousNetworkingModesSelectExpectedRoles() {
        XCTAssertTrue(AppleWifiAwareRendezvousNetworkingMode.automatic.startsBrowser)
        XCTAssertTrue(AppleWifiAwareRendezvousNetworkingMode.automatic.startsListener)

        XCTAssertFalse(
            AppleWifiAwareRendezvousNetworkingMode.publisherOnly.startsBrowser
        )
        XCTAssertTrue(
            AppleWifiAwareRendezvousNetworkingMode.publisherOnly.startsListener
        )

        XCTAssertTrue(
            AppleWifiAwareRendezvousNetworkingMode.subscriberOnly.startsBrowser
        )
        XCTAssertFalse(
            AppleWifiAwareRendezvousNetworkingMode.subscriberOnly.startsListener
        )
    }

    private func assemble(
        _ frames: [Data]
    ) -> WifiAwareRendezvousProtocol.Message? {
        let assembler = WifiAwareRendezvousProtocol.Assembler(key: key)
        var decoded: WifiAwareRendezvousProtocol.Message?
        for (index, frame) in frames.enumerated() {
            decoded = assembler.accept(
                frame,
                nowMilliseconds: Int64(index)
            ) ?? decoded
        }
        return decoded
    }

    private func unsignedShort(_ data: Data, offset: Int) -> Int {
        (Int(data[offset]) << 8) | Int(data[offset + 1])
    }

    private func setUnsignedShort(
        _ value: Int,
        in data: inout Data,
        offset: Int
    ) {
        data[offset] = UInt8((value >> 8) & 0xff)
        data[offset + 1] = UInt8(value & 0xff)
    }

    private func oneByteFragment(_ completeFrame: Data, offset: Int) -> Data {
        var fragment = Data(completeFrame.prefix(
            WifiAwareRendezvousProtocol.frameHeaderSize
        ))
        setUnsignedShort(offset, in: &fragment, offset: 14)
        fragment.append(
            completeFrame[WifiAwareRendezvousProtocol.frameHeaderSize + offset]
        )
        return fragment
    }
}
