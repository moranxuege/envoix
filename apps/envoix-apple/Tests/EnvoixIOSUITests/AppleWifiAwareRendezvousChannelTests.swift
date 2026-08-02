#if os(iOS) && canImport(WiFiAware)
import XCTest
@testable import Envoix_iOS

final class AppleWifiAwareRendezvousChannelTests: XCTestCase {
    private let remoteIdentity = LocalNearbyDiscoveryIdentity(
        peerKey: "8899aabbccddeeff",
        displayName: "Remote"
    )

    func testResponseWaiterRequiresMatchingTypeAndRequestID() async throws {
        let state = AppleWifiAwareRendezvousChannelState()
        let waiter = try await state.registerResponse(
            type: .helloAck,
            requestID: 41,
            expectedPeerKey: nil,
            timeout: .seconds(1)
        )

        let wrongType = await state.route(message(type: .inviteAck, requestID: 41))
        let wrongID = await state.route(message(type: .helloAck, requestID: 42))
        var pendingCount = await state.pendingResponseCount
        XCTAssertEqual(wrongType, .ignored)
        XCTAssertEqual(wrongID, .ignored)
        XCTAssertEqual(pendingCount, 1)

        let matching = message(type: .helloAck, requestID: 41)
        let matchingRoute = await state.route(matching)
        let received = try await waiter.value()
        XCTAssertEqual(matchingRoute, .handledResponse)
        XCTAssertEqual(received, matching)
        pendingCount = await state.pendingResponseCount
        XCTAssertEqual(pendingCount, 0)
    }

    func testResponseWaiterRejectsChangedPeerIdentity() async throws {
        let state = AppleWifiAwareRendezvousChannelState()
        let waiter = try await state.registerResponse(
            type: .inviteAck,
            requestID: 52,
            expectedPeerKey: remoteIdentity.peerKey,
            timeout: .seconds(1)
        )
        let changedIdentity = LocalNearbyDiscoveryIdentity(
            peerKey: "fedcba9876543210",
            displayName: "Changed"
        )

        let changedRoute = await state.route(
            WifiAwareRendezvousProtocol.Message(
                type: .inviteAck,
                requestID: 52,
                senderIdentity: changedIdentity,
                content: ""
            )
        )
        XCTAssertEqual(changedRoute, .handledResponse)
        await assertChannelError(.peerIdentityChanged) {
            _ = try await waiter.value()
        }
    }

    func testDuplicateResponseKeyIsRejectedWithoutReplacingFirstWaiter() async throws {
        let state = AppleWifiAwareRendezvousChannelState()
        let first = try await state.registerResponse(
            type: .helloAck,
            requestID: 63,
            expectedPeerKey: nil,
            timeout: .seconds(1)
        )

        await assertChannelError(.duplicateRequest) {
            _ = try await state.registerResponse(
                type: .helloAck,
                requestID: 63,
                expectedPeerKey: nil,
                timeout: .seconds(1)
            )
        }
        let response = message(type: .helloAck, requestID: 63)
        let responseRoute = await state.route(response)
        let received = try await first.value()
        XCTAssertEqual(responseRoute, .handledResponse)
        XCTAssertEqual(received, response)
    }

    func testWaiterTimesOutAndIsRemoved() async throws {
        let state = AppleWifiAwareRendezvousChannelState()
        let waiter = try await state.registerResponse(
            type: .helloAck,
            requestID: 74,
            expectedPeerKey: nil,
            timeout: .milliseconds(20)
        )

        await assertChannelError(.responseTimedOut) {
            _ = try await waiter.value()
        }
        let pendingCount = await state.pendingResponseCount
        XCTAssertEqual(pendingCount, 0)
    }

    func testCancellingWaiterRemovesIt() async throws {
        let state = AppleWifiAwareRendezvousChannelState()
        let waiter = try await state.registerResponse(
            type: .helloAck,
            requestID: 85,
            expectedPeerKey: nil,
            timeout: .seconds(5)
        )
        let task = Task {
            try await waiter.value()
        }

        task.cancel()
        do {
            _ = try await task.value
            XCTFail("Expected waiter cancellation")
        } catch is CancellationError {
            // Expected.
        } catch {
            XCTFail("Unexpected error: \(error)")
        }
        for _ in 0..<10 {
            guard await state.pendingResponseCount != 0 else { break }
            await Task.yield()
        }
        let pendingCount = await state.pendingResponseCount
        XCTAssertEqual(pendingCount, 0)
    }

    func testCloseFailsEveryWaiterAndRejectsFutureRequests() async throws {
        let state = AppleWifiAwareRendezvousChannelState()
        let hello = try await state.registerResponse(
            type: .helloAck,
            requestID: 96,
            expectedPeerKey: nil,
            timeout: .seconds(5)
        )
        let invite = try await state.registerResponse(
            type: .inviteAck,
            requestID: 97,
            expectedPeerKey: remoteIdentity.peerKey,
            timeout: .seconds(5)
        )

        await state.close()

        await assertChannelError(.closed) { _ = try await hello.value() }
        await assertChannelError(.closed) { _ = try await invite.value() }
        await assertChannelError(.closed) {
            _ = try await state.registerResponse(
                type: .helloAck,
                requestID: 98,
                expectedPeerKey: nil,
                timeout: .seconds(1)
            )
        }
        let pendingCount = await state.pendingResponseCount
        XCTAssertEqual(pendingCount, 0)
    }

    func testInboundAcknowledgementRequiresAcceptedRequest() {
        let hello = message(type: .hello, requestID: 107)
        let invite = message(type: .invite, requestID: 108)
        let acknowledgement = message(type: .helloAck, requestID: 109)

        XCTAssertEqual(
            AppleWifiAwareRendezvousChannelState.acknowledgementKind(
                for: hello,
                accepted: true
            ),
            .hello
        )
        XCTAssertNil(
            AppleWifiAwareRendezvousChannelState.acknowledgementKind(
                for: hello,
                accepted: false
            )
        )
        XCTAssertEqual(
            AppleWifiAwareRendezvousChannelState.acknowledgementKind(
                for: invite,
                accepted: true
            ),
            .invite
        )
        XCTAssertNil(
            AppleWifiAwareRendezvousChannelState.acknowledgementKind(
                for: acknowledgement,
                accepted: true
            )
        )
    }

    func testRunCanStartOnlyOnceAndClosePreventsStarting() async throws {
        let running = AppleWifiAwareRendezvousChannelState()
        try await running.beginRun()
        await assertChannelError(.alreadyRunning) {
            try await running.beginRun()
        }

        let closed = AppleWifiAwareRendezvousChannelState()
        await closed.close()
        await assertChannelError(.closed) {
            try await closed.beginRun()
        }
    }

    private func message(
        type: WifiAwareRendezvousProtocol.MessageType,
        requestID: UInt64
    ) -> WifiAwareRendezvousProtocol.Message {
        WifiAwareRendezvousProtocol.Message(
            type: type,
            requestID: requestID,
            senderIdentity: remoteIdentity,
            content: type == .invite
                ? "envoix://invite/v2/channel-test"
                : ""
        )
    }

    private func assertChannelError(
        _ expected: AppleWifiAwareRendezvousChannelError,
        operation: () async throws -> Void,
        file: StaticString = #filePath,
        line: UInt = #line
    ) async {
        do {
            try await operation()
            XCTFail("Expected \(expected)", file: file, line: line)
        } catch let error as AppleWifiAwareRendezvousChannelError {
            XCTAssertEqual(error, expected, file: file, line: line)
        } catch {
            XCTFail("Unexpected error: \(error)", file: file, line: line)
        }
    }
}
#endif
