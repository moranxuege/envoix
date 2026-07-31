import CoreNFC
import EnvoixCore
import XCTest
@testable import Envoix_iOS

final class NFCInvitationNDEFTests: XCTestCase {
    func testCanonicalInviteV2RoundTripsThroughExactHTTPSCarrier() throws {
        let invitation = try makePairingInvite(
            role: .send,
            broker: defaultRendezvousBroker,
            relay: defaultRelayURL
        ).payload
        let encoded = base64URLString(for: Data(invitation.utf8))
        let carrier = NFCInvitationNDEFCodec.carrierPrefix + encoded

        let message = try NFCInvitationNDEFCodec.message(for: invitation)
        let record = try XCTUnwrap(message.records.first)

        XCTAssertEqual(message.records.count, 1)
        XCTAssertEqual(record.typeNameFormat, .nfcWellKnown)
        XCTAssertEqual(record.type, Data([0x55]))
        XCTAssertEqual(record.identifier, Data())
        XCTAssertEqual(record.payload.first, 0)
        XCTAssertEqual(Data(record.payload.dropFirst()), Data(carrier.utf8))
        XCTAssertEqual(
            try NFCInvitationNDEFCodec.invitation(from: [message]),
            invitation
        )
        XCTAssertEqual(
            try NFCInvitationNDEFCodec.invitation(
                fromCarrierURL: XCTUnwrap(URL(string: carrier))
            ),
            invitation
        )
    }

    func testRoomInvitationRoundTripsWithoutNormalization() throws {
        let invitation =
            "envoix://room/R123456-a1b2-c3d4?broker=https%3A%2F%2Fexample.test%2Froom&expires=9999999999"
        let message = try NFCInvitationNDEFCodec.message(for: invitation)
        let carrier = try NFCInvitationNDEFCodec.carrierURL(for: invitation)

        XCTAssertEqual(
            try NFCInvitationNDEFCodec.invitation(from: message),
            invitation
        )
        XCTAssertEqual(
            carrier.absoluteString,
            NFCInvitationNDEFCodec.carrierPrefix
                + base64URLString(for: Data(invitation.utf8))
        )
    }

    func testDecodeStillAcceptsLegacyDirectEnvoixURIRecord() throws {
        let invitation = "envoix://room/R123456-legacy-tag"

        XCTAssertEqual(
            try NFCInvitationNDEFCodec.invitation(
                from: message(containingURI: invitation)
            ),
            invitation
        )
        XCTAssertEqual(
            try NFCInvitationNDEFCodec.invitation(
                fromDirectURL: XCTUnwrap(URL(string: invitation))
            ),
            invitation
        )
    }

    func testRejectsPrefixOnlyLegacyAndHTTPSCarrierInvitations() {
        for prefix in ["envoix://invite/v2/", "envoix://room/"] {
            XCTAssertThrowsError(
                try NFCInvitationNDEFCodec.message(for: prefix)
            ) { error in
                XCTAssertEqual(
                    error as? NFCInvitationContractError,
                    .unsupportedInvitation
                )
            }
            XCTAssertThrowsError(
                try NFCInvitationNDEFCodec.invitation(
                    from: message(containingURI: prefix)
                )
            ) { error in
                XCTAssertEqual(
                    error as? NFCInvitationContractError,
                    .unsupportedInvitation
                )
            }
            XCTAssertThrowsError(
                try NFCInvitationNDEFCodec.invitation(
                    fromDirectURL: XCTUnwrap(URL(string: prefix))
                )
            ) { error in
                XCTAssertEqual(
                    error as? NFCInvitationContractError,
                    .unsupportedInvitation
                )
            }

            let carrier = NFCInvitationNDEFCodec.carrierPrefix
                + base64URLString(for: Data(prefix.utf8))
            XCTAssertThrowsError(
                try NFCInvitationNDEFCodec.invitation(
                    fromCarrierURL: XCTUnwrap(URL(string: carrier))
                )
            ) { error in
                XCTAssertEqual(
                    error as? NFCInvitationContractError,
                    .unsupportedInvitation
                )
            }
            XCTAssertThrowsError(
                try NFCInvitationNDEFCodec.invitation(
                    from: message(containingURI: carrier)
                )
            ) { error in
                XCTAssertEqual(
                    error as? NFCInvitationContractError,
                    .unsupportedInvitation
                )
            }
        }
    }

    func testDirectExternalURLRejectsNonCanonicalInvitationCase() {
        XCTAssertThrowsError(
            try NFCInvitationNDEFCodec.invitation(
                fromDirectURL: XCTUnwrap(
                    URL(string: "Envoix://room/R123456-wrong-case")
                )
            )
        ) { error in
            XCTAssertEqual(
                error as? NFCInvitationContractError,
                .unsupportedInvitation
            )
        }
    }

    func testDecodeRejectsMultipleMessagesAndMultipleRecords() throws {
        let message = try NFCInvitationNDEFCodec.message(
            for: "envoix://room/R123456-test-room"
        )
        XCTAssertThrowsError(
            try NFCInvitationNDEFCodec.invitation(from: [message, message])
        ) { error in
            XCTAssertEqual(error as? NFCInvitationContractError, .messageCount)
        }

        let record = try XCTUnwrap(message.records.first)
        let multipleRecords = NFCNDEFMessage(records: [record, record])
        XCTAssertThrowsError(
            try NFCInvitationNDEFCodec.invitation(from: multipleRecords)
        ) { error in
            XCTAssertEqual(error as? NFCInvitationContractError, .recordCount)
        }
    }

    func testDecodeRejectsWrongTypeIdentifierAndURIPrefixCode() {
        let invitation = "envoix://room/R123456-test-room"
        let bytes = Data(invitation.utf8)
        let wrongType = NFCNDEFPayload(
            format: .nfcWellKnown,
            type: Data([0x54]),
            identifier: Data(),
            payload: Data([0x00]) + bytes
        )
        let nonEmptyIdentifier = NFCNDEFPayload(
            format: .nfcWellKnown,
            type: Data([0x55]),
            identifier: Data([0x01]),
            payload: Data([0x00]) + bytes
        )
        let compressedURI = NFCNDEFPayload(
            format: .nfcWellKnown,
            type: Data([0x55]),
            identifier: Data(),
            payload: Data([0x01]) + bytes
        )

        for record in [wrongType, nonEmptyIdentifier, compressedURI] {
            XCTAssertThrowsError(
                try NFCInvitationNDEFCodec.invitation(
                    from: NFCNDEFMessage(records: [record])
                )
            ) { error in
                XCTAssertEqual(error as? NFCInvitationContractError, .recordType)
            }
        }
    }

    func testContractRejectsWhitespaceControlsNonASCIIAndWrongCase() {
        for invitation in [
            "envoix://room/R123 456-test-room",
            "envoix://room/R123456-\ttest-room",
            "envoix://room/R123456-test-\nroom",
            "envoix://room/R123456-café",
            "Envoix://room/R123456-test-room"
        ] {
            XCTAssertThrowsError(
                try NFCInvitationNDEFCodec.message(for: invitation),
                "accepted \(invitation.debugDescription)"
            )
        }
    }

    func testDecodeRejectsEmbeddedSpaceTabAndNewline() {
        for invitation in [
            "envoix://room/R123 456-test-room",
            "envoix://room/R123456-\ttest-room",
            "envoix://room/R123456-test-\nroom"
        ] {
            let record = NFCNDEFPayload(
                format: .nfcWellKnown,
                type: Data([0x55]),
                identifier: Data(),
                payload: Data([0x00]) + Data(invitation.utf8)
            )
            XCTAssertThrowsError(
                try NFCInvitationNDEFCodec.invitation(
                    from: NFCNDEFMessage(records: [record])
                ),
                "decoded \(invitation.debugDescription)"
            )
        }
    }

    func testCarrierRejectsWrongOriginPathAndNonInvitationPayload() {
        let invitation = "envoix://room/R123456-test-room"
        let encoded = base64URLString(for: Data(invitation.utf8))
        let invalidURLs = [
            "http://ece4410j-nuub.github.io/nfc/v1/#\(encoded)",
            "https://ECE4410J-NUUB.github.io/nfc/v1/#\(encoded)",
            "https://ece4410j-nuub.github.io:443/nfc/v1/#\(encoded)",
            "https://ece4410j-nuub.github.io/nfc/v2/#\(encoded)",
            "https://ece4410j-nuub.github.io/nfc/v1/?source=test#\(encoded)"
        ]

        for value in invalidURLs {
            XCTAssertThrowsError(
                try NFCInvitationNDEFCodec.invitation(
                    fromCarrierURL: XCTUnwrap(URL(string: value))
                ),
                "accepted \(value)"
            ) { error in
                XCTAssertEqual(
                    error as? NFCInvitationContractError,
                    .unsupportedInvitation
                )
            }
        }

        let notInvitation = NFCInvitationNDEFCodec.carrierPrefix
            + base64URLString(for: Data("https://example.test".utf8))
        XCTAssertThrowsError(
            try NFCInvitationNDEFCodec.invitation(
                fromCarrierURL: XCTUnwrap(URL(string: notInvitation))
            )
        ) { error in
            XCTAssertEqual(
                error as? NFCInvitationContractError,
                .unsupportedInvitation
            )
        }
    }

    func testCarrierRejectsMalformedAndNonCanonicalBase64URL() {
        let malformedSuffixes = [
            "",
            "A",
            "YWJj=",
            "YWJj+",
            "YWJj/",
            "YWJj%2F",
            // "Yh" has non-zero discarded bits and decodes to the same byte
            // as canonical "Yg"; strict re-encoding must reject it.
            "ZW52b2l4Oi8vcm9vbS9hYh"
        ]

        for suffix in malformedSuffixes {
            let value = NFCInvitationNDEFCodec.carrierPrefix + suffix
            XCTAssertThrowsError(
                try NFCInvitationNDEFCodec.invitation(
                    fromCarrierURL: XCTUnwrap(URL(string: value))
                ),
                "accepted \(value)"
            ) { error in
                XCTAssertEqual(
                    error as? NFCInvitationContractError,
                    .malformedCarrier
                )
            }
        }
    }

    func testContractAcceptsExactMaximumAndRejectsOneByteMore() throws {
        let prefix = "envoix://room/"
        let maximum = prefix + String(
            repeating: "a",
            count: NFCInvitationNDEFCodec.maximumInvitationBytes - prefix.utf8.count
        )
        let oversized = maximum + "a"

        XCTAssertEqual(maximum.utf8.count, 8_211)
        let maximumMessage = try NFCInvitationNDEFCodec.message(for: maximum)
        XCTAssertEqual(
            try NFCInvitationNDEFCodec.invitation(from: maximumMessage),
            maximum
        )
        XCTAssertThrowsError(
            try NFCInvitationNDEFCodec.message(for: oversized)
        ) { error in
            XCTAssertEqual(
                error as? NFCInvitationContractError,
                .oversizedInvitation
            )
        }
        let oversizedRecord = NFCNDEFPayload(
            format: .nfcWellKnown,
            type: Data([0x55]),
            identifier: Data(),
            payload: Data([0x00]) + Data(oversized.utf8)
        )
        XCTAssertThrowsError(
            try NFCInvitationNDEFCodec.invitation(
                from: NFCNDEFMessage(records: [oversizedRecord])
            )
        ) { error in
            XCTAssertEqual(
                error as? NFCInvitationContractError,
                .oversizedInvitation
            )
        }
        let oversizedCarrier = NFCInvitationNDEFCodec.carrierPrefix
            + base64URLString(for: Data(oversized.utf8))
        XCTAssertThrowsError(
            try NFCInvitationNDEFCodec.invitation(
                from: message(containingURI: oversizedCarrier)
            )
        ) { error in
            XCTAssertEqual(
                error as? NFCInvitationContractError,
                .oversizedInvitation
            )
        }
    }

    func testPrivateAIDType4CommandsAreExactShortAPDUs() throws {
        XCTAssertEqual(
            NFCInvitationPrivateAIDProtocol.applicationIdentifier,
            "F0454E564F495801"
        )
        XCTAssertTrue(
            NFCInvitationPrivateAIDProtocol.matchesApplicationIdentifier(
                "f0454e564f495801"
            )
        )
        XCTAssertFalse(
            NFCInvitationPrivateAIDProtocol.matchesApplicationIdentifier(
                "F0454E564F495802"
            )
        )
        XCTAssertEqual(
            NFCInvitationPrivateAIDProtocol.selectNDEFFile.bytes,
            Data([0x00, 0xa4, 0x00, 0x0c, 0x02, 0xe1, 0x04])
        )
        XCTAssertEqual(
            NFCInvitationPrivateAIDProtocol.selectNDEFFile
                .expectedResponseLength,
            0
        )
        XCTAssertNotNil(
            NFCInvitationPrivateAIDProtocol.selectNDEFFile.apdu
        )
        XCTAssertEqual(
            NFCInvitationPrivateAIDProtocol.readNDEFLength.bytes,
            Data([0x00, 0xb0, 0x00, 0x00, 0x02])
        )

        let firstChunk = try NFCInvitationPrivateAIDProtocol.readBinary(
            offset: 2,
            length: 0xff
        )
        XCTAssertEqual(
            firstChunk.bytes,
            Data([0x00, 0xb0, 0x00, 0x02, 0xff])
        )
        XCTAssertEqual(firstChunk.expectedResponseLength, 0xff)

        let laterChunk = try NFCInvitationPrivateAIDProtocol.readBinary(
            offset: 0x1234,
            length: 7
        )
        XCTAssertEqual(
            laterChunk.bytes,
            Data([0x00, 0xb0, 0x12, 0x34, 0x07])
        )
    }

    func testPrivateAIDReadRejectsInvalidOffsetsAndChunkLengths() {
        for (offset, length) in [
            (-1, 1),
            (0x1_0000, 1),
            (2, 0),
            (2, 0x100)
        ] {
            XCTAssertThrowsError(
                try NFCInvitationPrivateAIDProtocol.readBinary(
                    offset: offset,
                    length: length
                )
            ) { error in
                XCTAssertEqual(
                    error as? NFCInvitationPrivateAIDError,
                    .invalidCommand
                )
            }
        }
    }

    func testPrivateAIDResponsesRequire9000AndExactDataLength() throws {
        let command = NFCInvitationPrivateAIDProtocol.readNDEFLength
        XCTAssertNoThrow(
            try NFCInvitationPrivateAIDProtocol.validateResponse(
                Data([0x00, 0x20]),
                sw1: 0x90,
                sw2: 0x00,
                for: command
            )
        )
        XCTAssertThrowsError(
            try NFCInvitationPrivateAIDProtocol.validateResponse(
                Data([0x00, 0x20]),
                sw1: 0x6a,
                sw2: 0x82,
                for: command
            )
        ) { error in
            XCTAssertEqual(
                error as? NFCInvitationPrivateAIDError,
                .commandFailed(status: 0x6a82)
            )
        }
        XCTAssertThrowsError(
            try NFCInvitationPrivateAIDProtocol.validateResponse(
                Data([0x20]),
                sw1: 0x90,
                sw2: 0x00,
                for: command
            )
        ) { error in
            XCTAssertEqual(
                error as? NFCInvitationPrivateAIDError,
                .unexpectedResponseLength(expected: 2, actual: 1)
            )
        }
        XCTAssertThrowsError(
            try NFCInvitationPrivateAIDProtocol.validateResponse(
                Data([0x00]),
                sw1: 0x90,
                sw2: 0x00,
                for: NFCInvitationPrivateAIDProtocol.selectNDEFFile
            )
        ) { error in
            XCTAssertEqual(
                error as? NFCInvitationPrivateAIDError,
                .unexpectedResponseLength(expected: 0, actual: 1)
            )
        }
    }

    func testPrivateAIDNLENIsBoundedBeforeMessageAllocation() throws {
        let maximum =
            NFCInvitationNDEFCodec.maximumSerializedMessageBytes
        XCTAssertLessThan(maximum, 0xffff)
        XCTAssertEqual(
            try NFCInvitationPrivateAIDProtocol.ndefLength(
                from: Data([
                    UInt8((maximum >> 8) & 0xff),
                    UInt8(maximum & 0xff)
                ])
            ),
            maximum
        )

        for invalid in [
            Data(),
            Data([0x01]),
            Data([0x00, 0x00]),
            Data([
                UInt8(((maximum + 1) >> 8) & 0xff),
                UInt8((maximum + 1) & 0xff)
            ])
        ] {
            XCTAssertThrowsError(
                try NFCInvitationPrivateAIDProtocol.ndefLength(from: invalid)
            )
        }
    }

    func testPrivateAIDRawMessageUsesExistingStrictInvitationCodec() throws {
        let invitation = "envoix://room/R123456-private-aid"
        let uriPayload = Data([0x00]) + Data(invitation.utf8)
        let rawMessage = Data([
            0xd1,
            0x01,
            UInt8(uriPayload.count),
            0x55
        ]) + uriPayload

        let message = try NFCInvitationPrivateAIDProtocol.message(
            from: rawMessage
        )
        XCTAssertEqual(
            try NFCInvitationNDEFCodec.invitation(from: message),
            invitation
        )
    }

    func testPrivateAIDRawEnvelopeRejectsMalformedFraming() {
        let invitation = "envoix://room/R123456-private-aid"
        let uriPayload = Data([0x00]) + Data(invitation.utf8)
        let rawMessage = Data([
            0xd1,
            0x01,
            UInt8(uriPayload.count),
            0x55
        ]) + uriPayload

        var missingMessageBegin = rawMessage
        missingMessageBegin[0] &= 0x7f
        var missingMessageEnd = rawMessage
        missingMessageEnd[0] &= 0xbf
        var chunked = rawMessage
        chunked[0] |= 0x20
        var wrongTypeNameFormat = rawMessage
        wrongTypeNameFormat[0] = (wrongTypeNameFormat[0] & 0xf8) | 0x02
        var declaredPayloadTooShort = rawMessage
        declaredPayloadTooShort[2] -= 1
        var declaredPayloadTooLong = rawMessage
        declaredPayloadTooLong[2] += 1
        var truncatedPayload = rawMessage
        truncatedPayload.removeLast()
        var trailingByte = rawMessage
        trailingByte.append(0x00)
        let secondRecord = rawMessage + rawMessage

        for malformed in [
            Data(),
            Data([0xff]),
            missingMessageBegin,
            missingMessageEnd,
            chunked,
            wrongTypeNameFormat,
            declaredPayloadTooShort,
            declaredPayloadTooLong,
            truncatedPayload,
            trailingByte,
            secondRecord,
            // Long-form record without all four payload-length bytes.
            Data([0xc1, 0x01, 0x00, 0x00, 0x01]),
            // IL is set, but its identifier-length byte is absent.
            Data([0xd9, 0x01, 0x01])
        ] {
            XCTAssertThrowsError(
                try NFCInvitationPrivateAIDProtocol.message(from: malformed)
            ) { error in
                XCTAssertEqual(
                    error as? NFCInvitationPrivateAIDError,
                    .malformedNDEFMessage
                )
            }
        }
    }

    func testPrivateAIDRawEnvelopeAcceptsValidLongRecord() throws {
        let invitation =
            "envoix://room/" + String(repeating: "a", count: 300)
        let uriPayload = Data([0x00]) + Data(invitation.utf8)
        let payloadLength = UInt32(uriPayload.count)
        let rawMessage = Data([
            0xc1,
            0x01,
            UInt8((payloadLength >> 24) & 0xff),
            UInt8((payloadLength >> 16) & 0xff),
            UInt8((payloadLength >> 8) & 0xff),
            UInt8(payloadLength & 0xff),
            0x55
        ]) + uriPayload

        XCTAssertGreaterThan(uriPayload.count, 0xff)
        let message = try NFCInvitationPrivateAIDProtocol.message(
            from: rawMessage
        )
        XCTAssertEqual(
            try NFCInvitationNDEFCodec.invitation(from: message),
            invitation
        )
    }

    func testTerminalGateStagesAndDeliversOnlyOnce() {
        var gate = NFCInvitationTerminalGate<Int>()

        XCTAssertTrue(gate.stage(1))
        XCTAssertFalse(gate.stage(2))
        XCTAssertEqual(gate.take(), 1)
        XCTAssertNil(gate.take())

        XCTAssertTrue(gate.stage(3))
        XCTAssertEqual(gate.take(), 3)
    }

    private func message(containingURI uri: String) -> NFCNDEFMessage {
        let record = NFCNDEFPayload(
            format: .nfcWellKnown,
            type: Data([0x55]),
            identifier: Data(),
            payload: Data([0x00]) + Data(uri.utf8)
        )
        return NFCNDEFMessage(records: [record])
    }

    private func base64URLString(for data: Data) -> String {
        data.base64EncodedString()
            .replacingOccurrences(of: "+", with: "-")
            .replacingOccurrences(of: "/", with: "_")
            .replacingOccurrences(of: "=", with: "")
    }
}
