import EnvoixCore
import Foundation
import XCTest
@testable import Envoix_iOS

final class WifiAwareReceiverStartupTests: XCTestCase {
    func testRoomSenderAndInviteReceiverUseSamePeerHello() throws {
        guard #available(iOS 26.0, *) else {
            throw XCTSkip("Wi-Fi Aware requires iOS or iPadOS 26")
        }
        let invite = try makePairingInvite(
            role: .send,
            broker: defaultRendezvousBroker,
            relay: defaultRelayURL
        )
        let sender = makeRequest(
            direction: .send,
            mode: .room,
            code: invite.roomCode
        )
        let receiver = makeRequest(
            direction: .receive,
            mode: .invite,
            invite: invite.payload
        )

        let senderAuthenticator = try AppleWifiAwareTransportSession
            .peerHelloAuthenticator(for: sender)
        let receiverAuthenticator = try AppleWifiAwareTransportSession
            .peerHelloAuthenticator(for: receiver)

        XCTAssertEqual(senderAuthenticator, receiverAuthenticator)
        XCTAssertEqual(
            AppleWifiAwareTransportSession.peerHelloDatagram(
                authenticator: senderAuthenticator
            ),
            AppleWifiAwareTransportSession.peerHelloDatagram(
                authenticator: receiverAuthenticator
            )
        )
    }

    func testPeerHelloRejectsRequestWithoutCanonicalRoomCode() throws {
        guard #available(iOS 26.0, *) else {
            throw XCTSkip("Wi-Fi Aware requires iOS or iPadOS 26")
        }
        let request = makeRequest(
            direction: .send,
            mode: .room,
            code: "not-a-room"
        )

        XCTAssertThrowsError(
            try AppleWifiAwareTransportSession.peerHelloAuthenticator(
                for: request
            )
        ) { error in
            guard case AppleWifiAwareTransportError
                .invalidTransferAuthenticator = error else {
                return XCTFail("Unexpected error: \(error)")
            }
        }
        XCTAssertFalse(
            AppleWifiAwareTransportSession.isRecoverableWifiAwareFailure(
                AppleWifiAwareTransportError.invalidTransferAuthenticator
            )
        )
    }

    func testPeerHelloBindsToTheAuthenticatedTransfer() throws {
        guard #available(iOS 26.0, *) else {
            throw XCTSkip("Wi-Fi Aware requires iOS or iPadOS 26")
        }

        let first = AppleWifiAwareTransportSession.peerHelloDatagram(
            authenticator: "invite-a"
        )
        let repeated = AppleWifiAwareTransportSession.peerHelloDatagram(
            authenticator: "invite-a"
        )
        let second = AppleWifiAwareTransportSession.peerHelloDatagram(
            authenticator: "invite-b"
        )

        XCTAssertEqual(first, repeated)
        XCTAssertNotEqual(first, second)
        XCTAssertNotEqual(
            first,
            AppleWifiAwareTransportSession.defaultPeerHelloDatagram
        )
    }

    func testEmptyAuthenticatorRetainsRawProbeCompatibility() throws {
        guard #available(iOS 26.0, *) else {
            throw XCTSkip("Wi-Fi Aware requires iOS or iPadOS 26")
        }

        XCTAssertEqual(
            AppleWifiAwareTransportSession.peerHelloDatagram(authenticator: ""),
            AppleWifiAwareTransportSession.defaultPeerHelloDatagram
        )
    }

    func testReceiverAdmissionAcceptsOnlyOneAuthenticatedConnection() async throws {
        guard #available(iOS 26.0, *) else {
            throw XCTSkip("Wi-Fi Aware requires iOS or iPadOS 26")
        }
        let admission = AppleWifiAwareReceiverAdmission()

        async let first = admission.claim()
        async let second = admission.claim()
        let outcomes = await (first, second)

        XCTAssertNotEqual(outcomes.0, outcomes.1)
    }

    func testDatagramRouterInterceptsEveryRepeatedControlFrame() throws {
        guard #available(iOS 26.0, *) else {
            throw XCTSkip("Wi-Fi Aware requires iOS or iPadOS 26")
        }
        let control = Data("control".utf8)
        let payload = Data("payload".utf8)

        XCTAssertFalse(
            AppleWifiAwareDatagramRouter.shouldForward(
                control,
                intercepting: control
            )
        )
        XCTAssertFalse(
            AppleWifiAwareDatagramRouter.shouldForward(
                control,
                intercepting: control
            )
        )
        XCTAssertTrue(
            AppleWifiAwareDatagramRouter.shouldForward(
                payload,
                intercepting: control
            )
        )
    }

    func testDatagramInboxCancellationReleasesPendingReceive() async throws {
        guard #available(iOS 26.0, *) else {
            throw XCTSkip("Wi-Fi Aware requires iOS or iPadOS 26")
        }
        let inbox = AppleWifiAwareDatagramInbox()
        let pendingReceive = Task {
            try await inbox.receive(maxBytes: 32)
        }
        await Task.yield()
        pendingReceive.cancel()

        do {
            _ = try await pendingReceive.value
            XCTFail("Expected the pending receive to be cancelled")
        } catch is CancellationError {
            // Expected.
        }

        let payload = Data("next-payload".utf8)
        await inbox.deliver(payload)
        let received = try await inbox.receive(maxBytes: 32)
        XCTAssertEqual(received, payload)
    }

    func testPeerReadyHandshakeRetriesUntilAcknowledged() async throws {
        guard #available(iOS 26.0, *) else {
            throw XCTSkip("Wi-Fi Aware requires iOS or iPadOS 26")
        }
        let (events, continuation) = AsyncStream<Void>.makeStream(
            bufferingPolicy: .bufferingNewest(1)
        )
        let recorder = PeerHelloSendRecorder()

        try await AppleWifiAwareTransportSession.awaitPeerReady(
            events,
            timeout: .seconds(1),
            retryInterval: .milliseconds(10),
            sendPeerHello: {
                if await recorder.record() == 3 {
                    continuation.yield()
                }
            }
        )
        continuation.finish()

        let sendCount = await recorder.count
        XCTAssertEqual(sendCount, 3)
    }

    func testPeerReadyTimeoutStopsRetryLoop() async throws {
        guard #available(iOS 26.0, *) else {
            throw XCTSkip("Wi-Fi Aware requires iOS or iPadOS 26")
        }
        let (events, continuation) = AsyncStream<Void>.makeStream(
            bufferingPolicy: .bufferingNewest(1)
        )
        defer { continuation.finish() }
        let recorder = PeerHelloSendRecorder()

        do {
            try await AppleWifiAwareTransportSession.awaitPeerReady(
                events,
                timeout: .milliseconds(40),
                retryInterval: .milliseconds(5),
                sendPeerHello: {
                    _ = await recorder.record()
                }
            )
            XCTFail("Expected the Wi-Fi Aware peer-ready handshake to time out")
        } catch AppleWifiAwareTransportError.peerReadyTimedOut {
            // Expected.
        }

        let countAtTimeout = await recorder.count
        try await Task<Never, Never>.sleep(for: .milliseconds(20))
        let countAfterDelay = await recorder.count
        XCTAssertGreaterThan(countAtTimeout, 0)
        XCTAssertEqual(countAfterDelay, countAtTimeout)
    }

    @MainActor
    func testListenerReadyNotifiesExactlyOnce() async throws {
        guard #available(iOS 26.0, *) else {
            throw XCTSkip("Wi-Fi Aware requires iOS or iPadOS 26")
        }
        let (events, continuation) = AsyncThrowingStream<Void, Error>.makeStream()
        continuation.yield()
        continuation.yield()
        continuation.finish()
        let recorder = ReceiverStartupNotificationRecorder()

        try await AppleWifiAwareTransportSession.awaitReceiverListenerReady(
            events,
            onListenerReady: {
                recorder.record()
            }
        )

        XCTAssertEqual(recorder.count, 1)
    }

    @MainActor
    func testListenerFailureDoesNotNotifyLaunch() async throws {
        guard #available(iOS 26.0, *) else {
            throw XCTSkip("Wi-Fi Aware requires iOS or iPadOS 26")
        }
        let (events, continuation) = AsyncThrowingStream<Void, Error>.makeStream()
        continuation.finish(throwing: ReceiverStartupTestError.listenerFailed)
        let recorder = ReceiverStartupNotificationRecorder()

        do {
            try await AppleWifiAwareTransportSession.awaitReceiverListenerReady(
                events,
                onListenerReady: {
                    recorder.record()
                }
            )
            XCTFail("Expected listener failure")
        } catch ReceiverStartupTestError.listenerFailed {
            // Expected.
        }

        XCTAssertEqual(recorder.count, 0)
    }

    @MainActor
    func testListenerCancellationDoesNotNotifyLaunch() async throws {
        guard #available(iOS 26.0, *) else {
            throw XCTSkip("Wi-Fi Aware requires iOS or iPadOS 26")
        }
        let (events, continuation) = AsyncThrowingStream<Void, Error>.makeStream()
        continuation.finish(throwing: CancellationError())
        let recorder = ReceiverStartupNotificationRecorder()

        do {
            try await AppleWifiAwareTransportSession.awaitReceiverListenerReady(
                events,
                onListenerReady: {
                    recorder.record()
                }
            )
            XCTFail("Expected listener cancellation")
        } catch is CancellationError {
            // Expected.
        }

        XCTAssertEqual(recorder.count, 0)
    }

    @MainActor
    func testReceiveLaunchSignalResumesOnlyOnce() async {
        let activityID: String? = await withCheckedContinuation { continuation in
            let signal = TransferViewModel.ReceiveLaunchSignal(continuation)
            signal.resolve("first")
            signal.resolve("second")
        }

        XCTAssertEqual(activityID, "first")
    }

    private func makeRequest(
        direction: FfiTransferDirection,
        mode: FfiTransferMode,
        code: String = "",
        invite: String = ""
    ) -> FfiTransferRequest {
        FfiTransferRequest(
            direction: direction,
            mode: mode,
            peerDescriptor: "",
            invite: invite,
            code: code,
            token: "",
            rememberConsent: false,
            rememberedCredentialRef: "",
            rememberedGeneration: 0,
            rememberedPreviousGeneration: nil,
            broker: defaultRendezvousBroker,
            relay: defaultRelayURL,
            configPath: "",
            pathPolicy: .auto,
            rendezvous: FfiRendezvousPlan(
                useRoom: true,
                useMdns: false,
                internetAvailable: true
            )
        )
    }
}

private enum ReceiverStartupTestError: Error {
    case listenerFailed
}

@MainActor
private final class ReceiverStartupNotificationRecorder {
    private(set) var count = 0

    func record() {
        count += 1
    }
}

private actor PeerHelloSendRecorder {
    private(set) var count = 0

    func record() -> Int {
        count += 1
        return count
    }
}
