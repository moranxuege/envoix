import XCTest
#if os(macOS)
@testable import Envoix
#else
@testable import Envoix_iOS
#endif

final class AppleWifiAwareServiceCoordinatorTests: XCTestCase {
    func testPurposeRoleMappingMatchesAppleRuntimeOwnership() {
        XCTAssertEqual(
            AppleWifiAwareServiceCoordinator.Purpose.control.roles,
            [.publisher, .subscriber]
        )
        XCTAssertEqual(
            AppleWifiAwareServiceCoordinator.Purpose.diagnostic.roles,
            [.publisher, .subscriber]
        )
        XCTAssertEqual(
            AppleWifiAwareServiceCoordinator.Purpose.systemPairing.roles,
            [.publisher, .subscriber]
        )
        XCTAssertEqual(
            AppleWifiAwareServiceCoordinator.Purpose.transferReceiver.roles,
            [.publisher]
        )
        XCTAssertEqual(
            AppleWifiAwareServiceCoordinator.Purpose.transferSender.roles,
            [.subscriber]
        )
    }

    func testComplementaryTransferRolesRunConcurrently() async throws {
        let coordinator = AppleWifiAwareServiceCoordinator()

        let receiver = try await coordinator.acquire(.transferReceiver)
        let sender = try await coordinator.acquire(.transferSender)

        let status = await coordinator.status()
        XCTAssertEqual(
            status,
            .init(
                activePurposes: [.transferReceiver, .transferSender],
                waitingPurposes: []
            )
        )
        await coordinator.release(receiver)
        await coordinator.release(sender)
    }

    func testControlQueuesBothTransfersThenReleasesThemTogether() async throws {
        let coordinator = AppleWifiAwareServiceCoordinator()
        let control = try await coordinator.acquire(.control)
        let receiverTask = Task {
            try await coordinator.acquire(.transferReceiver)
        }
        await waitForStatus(
            .init(activePurposes: [.control], waitingPurposes: [.transferReceiver]),
            from: coordinator
        )
        let senderTask = Task {
            try await coordinator.acquire(.transferSender)
        }
        await waitForStatus(
            .init(
                activePurposes: [.control],
                waitingPurposes: [.transferReceiver, .transferSender]
            ),
            from: coordinator
        )

        await coordinator.release(control)
        let receiver = try await receiverTask.value
        let sender = try await senderTask.value

        let status = await coordinator.status()
        XCTAssertEqual(
            status,
            .init(
                activePurposes: [.transferReceiver, .transferSender],
                waitingPurposes: []
            )
        )
        await coordinator.release(receiver)
        await coordinator.release(sender)
    }

    func testBlockedRoleDoesNotPreventIndependentRoleFromRunning() async throws {
        let coordinator = AppleWifiAwareServiceCoordinator()
        let firstReceiver = try await coordinator.acquire(.transferReceiver)
        let secondReceiverTask = Task {
            try await coordinator.acquire(.transferReceiver)
        }
        await waitForStatus(
            .init(
                activePurposes: [.transferReceiver],
                waitingPurposes: [.transferReceiver]
            ),
            from: coordinator
        )

        let sender = try await coordinator.acquire(.transferSender)
        let status = await coordinator.status()
        XCTAssertEqual(
            status,
            .init(
                activePurposes: [.transferReceiver, .transferSender],
                waitingPurposes: [.transferReceiver]
            )
        )

        await coordinator.release(firstReceiver)
        let secondReceiver = try await secondReceiverTask.value
        await coordinator.release(secondReceiver)
        await coordinator.release(sender)
    }

    func testCancellationRemovesPendingRequestWithoutBlockingQueue() async throws {
        let coordinator = AppleWifiAwareServiceCoordinator()
        let pairing = try await coordinator.acquire(.systemPairing)
        let cancelledTask = Task {
            try await coordinator.acquire(.transferReceiver)
        }
        await waitForStatus(
            .init(
                activePurposes: [.systemPairing],
                waitingPurposes: [.transferReceiver]
            ),
            from: coordinator
        )

        cancelledTask.cancel()
        do {
            _ = try await cancelledTask.value
            XCTFail("Cancelled acquisition unexpectedly received a lease")
        } catch is CancellationError {
            // Expected.
        } catch {
            XCTFail("Unexpected cancellation error: \(error)")
        }
        await waitForStatus(
            .init(activePurposes: [.systemPairing], waitingPurposes: []),
            from: coordinator
        )

        await coordinator.release(pairing)
        let sender = try await coordinator.acquire(.transferSender)
        await coordinator.release(sender)
    }

    func testDiagnosticExclusivelyOwnsBothRuntimeRoles() async throws {
        let coordinator = AppleWifiAwareServiceCoordinator()
        let diagnostic = try await coordinator.acquire(.diagnostic)
        let receiverTask = Task {
            try await coordinator.acquire(.transferReceiver)
        }
        await waitForStatus(
            .init(
                activePurposes: [.diagnostic],
                waitingPurposes: [.transferReceiver]
            ),
            from: coordinator
        )
        let senderTask = Task {
            try await coordinator.acquire(.transferSender)
        }
        await waitForStatus(
            .init(
                activePurposes: [.diagnostic],
                waitingPurposes: [.transferReceiver, .transferSender]
            ),
            from: coordinator
        )

        await coordinator.release(diagnostic)
        let receiver = try await receiverTask.value
        let sender = try await senderTask.value
        let status = await coordinator.status()
        XCTAssertEqual(
            status,
            .init(
                activePurposes: [.transferReceiver, .transferSender],
                waitingPurposes: []
            )
        )
        await coordinator.release(receiver)
        await coordinator.release(sender)
    }

    func testCancelledDiagnosticRequestDoesNotConsumeRuntimeRoles() async throws {
        let coordinator = AppleWifiAwareServiceCoordinator()
        let control = try await coordinator.acquire(.control)
        let diagnosticTask = Task {
            try await coordinator.acquire(.diagnostic)
        }
        await waitForStatus(
            .init(activePurposes: [.control], waitingPurposes: [.diagnostic]),
            from: coordinator
        )

        diagnosticTask.cancel()
        do {
            _ = try await diagnosticTask.value
            XCTFail("Cancelled diagnostic unexpectedly received a lease")
        } catch is CancellationError {
            // Expected.
        } catch {
            XCTFail("Unexpected cancellation error: \(error)")
        }
        await waitForStatus(
            .init(activePurposes: [.control], waitingPurposes: []),
            from: coordinator
        )

        await coordinator.release(control)
        let receiver = try await coordinator.acquire(.transferReceiver)
        let sender = try await coordinator.acquire(.transferSender)
        await coordinator.release(receiver)
        await coordinator.release(sender)
    }

    func testControlTransferControlTransitionReleasesEveryRole() async throws {
        let coordinator = AppleWifiAwareServiceCoordinator()
        let firstControl = try await coordinator.acquire(.control)
        await coordinator.release(firstControl)

        let receiver = try await coordinator.acquire(.transferReceiver)
        let sender = try await coordinator.acquire(.transferSender)
        let transferStatus = await coordinator.status()
        XCTAssertEqual(
            transferStatus,
            .init(
                activePurposes: [.transferReceiver, .transferSender],
                waitingPurposes: []
            )
        )
        await coordinator.release(receiver)
        await coordinator.release(sender)
        let releasedStatus = await coordinator.status()
        XCTAssertEqual(
            releasedStatus,
            .init(activePurposes: [], waitingPurposes: [])
        )

        let secondControl = try await coordinator.acquire(.control)
        let controlStatus = await coordinator.status()
        XCTAssertEqual(
            controlStatus,
            .init(activePurposes: [.control], waitingPurposes: [])
        )
        await coordinator.release(secondControl)
    }

    func testStaleReleaseCannotUnlockNewLease() async throws {
        let coordinator = AppleWifiAwareServiceCoordinator()
        let first = try await coordinator.acquire(.transferReceiver)
        let secondTask = Task {
            try await coordinator.acquire(.transferReceiver)
        }
        await waitForStatus(
            .init(
                activePurposes: [.transferReceiver],
                waitingPurposes: [.transferReceiver]
            ),
            from: coordinator
        )
        let thirdTask = Task {
            try await coordinator.acquire(.transferReceiver)
        }
        await waitForStatus(
            .init(
                activePurposes: [.transferReceiver],
                waitingPurposes: [.transferReceiver, .transferReceiver]
            ),
            from: coordinator
        )

        await coordinator.release(first)
        let second = try await secondTask.value
        await waitForStatus(
            .init(
                activePurposes: [.transferReceiver],
                waitingPurposes: [.transferReceiver]
            ),
            from: coordinator
        )

        await coordinator.release(first)
        let status = await coordinator.status()
        XCTAssertEqual(
            status,
            .init(
                activePurposes: [.transferReceiver],
                waitingPurposes: [.transferReceiver]
            )
        )

        await coordinator.release(second)
        let third = try await thirdTask.value
        await coordinator.release(third)
    }

    private func waitForStatus(
        _ expected: AppleWifiAwareServiceCoordinator.Status,
        from coordinator: AppleWifiAwareServiceCoordinator,
        file: StaticString = #filePath,
        line: UInt = #line
    ) async {
        for _ in 0..<1_000 {
            if await coordinator.status() == expected {
                return
            }
            await Task.yield()
        }
        let actual = await coordinator.status()
        XCTFail(
            "Timed out waiting for \(expected); got \(actual)",
            file: file,
            line: line
        )
    }
}
