#if os(macOS)
import EnvoixCore
import XCTest
@testable import Envoix

final class RoomCodeInputFormatterTests: XCTestCase {
    func testBleVerificationUsesBuiltInEndpointsWhenSettingsAreEmpty() throws {
        let now = Date(timeIntervalSince1970: 1_700_000_000)
        let verification = try BleVerificationInvitation.make(
            broker: "  ",
            relay: "",
            now: now
        )

        for payload in [verification.publicOffer, verification.privateInvitation] {
            let components = try XCTUnwrap(URLComponents(string: payload))
            let query = try XCTUnwrap(components.queryItems)
            XCTAssertEqual(
                query.first(where: { $0.name == "broker" })?.value,
                defaultRendezvousBroker
            )
            XCTAssertEqual(
                query.first(where: { $0.name == "relay" })?.value,
                defaultRelayURL
            )
        }
    }

    func testSeparatorFreeInputIsFormattedWithoutTruncation() {
        XCTAssertEqual(
            formatRoomCodeInput("123456K7M49V2D"),
            "123456-k7m4-9v2d"
        )
        XCTAssertEqual(formatRoomCodeInput("ABC"), "abc")
    }

    func testSequentialTypingAddsBothCanonicalSeparators() {
        var formatted = ""
        for character in "123456K7M49V2D" {
            formatted = formatRoomCodeInput(formatted + String(character))
        }

        XCTAssertEqual(formatted, "123456-k7m4-9v2d")
    }

    func testCanonicalPrefixesAreRebuiltAndLowercased() {
        XCTAssertEqual(formatRoomCodeInput("123456-"), "123456-")
        XCTAssertEqual(
            formatRoomCodeInput("123456-K7M4"),
            "123456-k7m4"
        )
        XCTAssertEqual(
            formatRoomCodeInput("123456K7M4-"),
            "123456-k7m4-"
        )
        XCTAssertEqual(
            formatRoomCodeInput("123456-K7M4-9V2D"),
            "123456-k7m4-9v2d"
        )
    }

    func testInviteURIRemainsUnchanged() {
        let invite = "envoix://invite/v2/Ab-C"
        let uppercaseScheme = "ENVOIX://INVITE/V2/Ab-C"

        XCTAssertEqual(formatRoomCodeInput(invite), invite)
        XCTAssertEqual(formatRoomCodeInput(uppercaseScheme), uppercaseScheme)
    }

    func testSequentialInviteTypingDoesNotInsertRoomCodeSeparators() {
        var formatted = ""
        let invite = "envoix://invite/v2/Ab-C"
        for character in invite {
            formatted = formatRoomCodeInput(formatted + String(character))
        }

        XCTAssertEqual(formatted, invite)
    }

    func testInvalidOrOversizedInputRemainsUnchangedForCoreValidation() {
        let invalidInputs = [
            "12345-6k7m49v2d",
            "123456--k7m49v2d",
            "123456k-7m49v2d",
            "123456k7m49v2dx",
            "123456-k7m49v2d",
            "123456k7m4-9v2d",
            "123456-k7m4-9v2d-extra",
            "123456k7m49v2!",
            "123456k7m49v2é",
            "123456 k7m49v2d",
            " 123456-k7m4-9v2d",
            "123456-k7m4-9v2d ",
        ]

        for input in invalidInputs {
            XCTAssertEqual(
                formatRoomCodeInput(input),
                input,
                "Unexpectedly rewrote invalid input \(input)"
            )
        }
    }

    func testCanonicalBareRoomControlCodeAcceptsCurrentFormsOnly() {
        XCTAssertEqual(
            canonicalBareRoomControlCode("123456-K7M4-9V2D"),
            "123456-k7m4-9v2d"
        )
        XCTAssertEqual(
            canonicalBareRoomControlCode("123456K7M49V2D"),
            "123456-k7m4-9v2d"
        )

        for input in [
            "R123456-k7m4-9v2d",
            "r123456-k7m4-9v2d",
            "abcdef-k7m4-9v2d",
            "123456-k7m4-9v2",
            "123456-k7m4-9v2!",
        ] {
            XCTAssertNil(
                canonicalBareRoomControlCode(input),
                "Accepted invalid Room code \(input)"
            )
        }
    }

    func testConnectionInputClassifierSeparatesInviteV2AndRoomControl() throws {
        let canonicalRoom = try classifyConnectionInput(
            "123456-k7m4-9v2d",
            fallbackBroker: defaultRendezvousBroker,
            fallbackRelay: defaultRelayURL,
            allowBareRoomControl: true
        )
        XCTAssertEqual(canonicalRoom.kind, .roomControl)
        XCTAssertEqual(canonicalRoom.normalizedInput, "123456-k7m4-9v2d")

        let compactRoom = try classifyConnectionInput(
            "123456K7M49V2D",
            fallbackBroker: defaultRendezvousBroker,
            fallbackRelay: defaultRelayURL,
            allowBareRoomControl: true
        )
        XCTAssertEqual(compactRoom.kind, .roomControl)
        XCTAssertEqual(compactRoom.normalizedInput, "123456-k7m4-9v2d")

        let roomURI = try classifyConnectionInput(
            "envoix://room/123456-k7m4-9v2d",
            fallbackBroker: defaultRendezvousBroker,
            fallbackRelay: defaultRelayURL,
            allowBareRoomControl: false
        )
        XCTAssertEqual(roomURI.kind, .roomControl)

        let invitation = try makePairingInvite(
            role: .send,
            broker: defaultRendezvousBroker,
            relay: defaultRelayURL
        )
        let inviteV2 = try classifyConnectionInput(
            invitation.payload,
            fallbackBroker: defaultRendezvousBroker,
            fallbackRelay: defaultRelayURL,
            allowBareRoomControl: true
        )
        XCTAssertEqual(inviteV2.kind, .inviteV2)
        XCTAssertEqual(inviteV2.normalizedInput, invitation.payload)
        XCTAssertFalse(invitation.roomCode.isEmpty)
        XCTAssertEqual(inviteV2.pairingInvite?.roomCode, "")
    }

    func testConnectionInputClassifierRejectsLegacyAndMalformedForms() {
        for input in [
            "R123456-k7m4-9v2d",
            "r123456-k7m4-9v2d",
            "envoix://room/R123456-k7m4-9v2d",
            "envoix://pair/123456-k7m4-9v2d",
            "envoix://invite/v2/opaque",
            "123456-k7m4-9v2!",
        ] {
            XCTAssertThrowsError(
                try classifyConnectionInput(
                    input,
                    fallbackBroker: defaultRendezvousBroker,
                    fallbackRelay: defaultRelayURL,
                    allowBareRoomControl: true
                ),
                "Accepted unsupported connection input \(input)"
            )
        }
    }
}
#endif
