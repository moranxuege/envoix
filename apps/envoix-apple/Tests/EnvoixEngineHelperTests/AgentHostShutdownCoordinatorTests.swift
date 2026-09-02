import EnvoixCore
import Foundation
import XCTest

final class AgentHostShutdownCoordinatorTests: XCTestCase {
    func testShutdownStartsWhileMainActorIsBlockedAndCompletesOnMainActor() async {
        let shutdownStarted = DispatchSemaphore(value: 0)
        let completion = expectation(description: "shutdown completion")
        let host = ImmediateShutdownHost(started: shutdownStarted)

        let task = await MainActor.run { () -> Task<Void, Never> in
            let task = EnvoixEngineHelperShutdownCoordinator.begin(
                host: host
            ) { outcome in
                XCTAssertTrue(Thread.isMainThread)
                XCTAssertEqual(outcome.state, .stopped)
                XCTAssertNil(outcome.error)
                XCTAssertFalse(outcome.timedOut)
                completion.fulfill()
            }
            XCTAssertEqual(
                shutdownStarted.wait(timeout: .now() + 1),
                .success,
                "shutdown must not wait for the AppKit termination loop to release MainActor"
            )
            return task
        }

        await fulfillment(of: [completion], timeout: 2)
        await task.value
    }

    func testShutdownWatchdogCompletesWhenHostDoesNotStop() async {
        let shutdownStarted = DispatchSemaphore(value: 0)
        let completion = expectation(description: "shutdown timeout")
        let host = SuspendedShutdownHost(started: shutdownStarted)

        let task = await MainActor.run { () -> Task<Void, Never> in
            let task = EnvoixEngineHelperShutdownCoordinator.begin(
                host: host,
                timeout: .milliseconds(50)
            ) { outcome in
                XCTAssertTrue(Thread.isMainThread)
                XCTAssertNil(outcome.state)
                XCTAssertNil(outcome.error)
                XCTAssertTrue(outcome.timedOut)
                completion.fulfill()
            }
            XCTAssertEqual(shutdownStarted.wait(timeout: .now() + 1), .success)
            return task
        }

        await fulfillment(of: [completion], timeout: 2)
        task.cancel()
        await task.value
    }
}

private final class ImmediateShutdownHost:
    EnvoixEngineHelperHostShuttingDown,
    @unchecked Sendable
{
    private let started: DispatchSemaphore

    init(started: DispatchSemaphore) {
        self.started = started
    }

    func shutdown() async throws -> FfiAgentHostLifecycleState {
        started.signal()
        return .stopped
    }
}

private final class SuspendedShutdownHost:
    EnvoixEngineHelperHostShuttingDown,
    @unchecked Sendable
{
    private let started: DispatchSemaphore

    init(started: DispatchSemaphore) {
        self.started = started
    }

    func shutdown() async throws -> FfiAgentHostLifecycleState {
        started.signal()
        try await Task.sleep(nanoseconds: 60_000_000_000)
        return .stopped
    }
}
