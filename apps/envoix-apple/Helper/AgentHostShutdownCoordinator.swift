import EnvoixCore
import Foundation

protocol EnvoixEngineHelperHostShuttingDown: Sendable {
    func shutdown() async throws -> FfiAgentHostLifecycleState
}

extension EnvoixEngineHelperHost: EnvoixEngineHelperHostShuttingDown {}

struct EnvoixEngineHelperShutdownOutcome: @unchecked Sendable {
    let state: FfiAgentHostLifecycleState?
    let error: Error?
    let timedOut: Bool
}

enum EnvoixEngineHelperShutdownCoordinator {
    private static let shutdownTimeout: DispatchTimeInterval = .seconds(4)

    @MainActor
    static func begin(
        host: EnvoixEngineHelperHostShuttingDown,
        timeout: DispatchTimeInterval = shutdownTimeout,
        completion: @MainActor @escaping @Sendable (
            EnvoixEngineHelperShutdownOutcome
        ) -> Void
    ) -> Task<Void, Never> {
        let gate = CompletionGate(completion: completion)
        let watchdog = DispatchWorkItem { [weak gate] in
            MainActor.assumeIsolated {
                gate?.finish(EnvoixEngineHelperShutdownOutcome(
                    state: nil,
                    error: nil,
                    timedOut: true
                ))
            }
        }
        gate.watchdog = watchdog
        DispatchQueue.main.asyncAfter(
            deadline: .now() + timeout,
            execute: watchdog
        )

        return Task.detached(priority: .userInitiated) {
            let outcome: EnvoixEngineHelperShutdownOutcome
            do {
                outcome = EnvoixEngineHelperShutdownOutcome(
                    state: try await host.shutdown(),
                    error: nil,
                    timedOut: false
                )
            } catch {
                outcome = EnvoixEngineHelperShutdownOutcome(
                    state: nil,
                    error: error,
                    timedOut: false
                )
            }
            await gate.finish(outcome)
        }
    }

    @MainActor
    private final class CompletionGate {
        var watchdog: DispatchWorkItem?

        private var finished = false
        private let completion: @MainActor @Sendable (
            EnvoixEngineHelperShutdownOutcome
        ) -> Void

        init(
            completion: @MainActor @escaping @Sendable (
                EnvoixEngineHelperShutdownOutcome
            ) -> Void
        ) {
            self.completion = completion
        }

        func finish(_ outcome: EnvoixEngineHelperShutdownOutcome) {
            guard !finished else { return }
            finished = true
            watchdog?.cancel()
            watchdog = nil
            completion(outcome)
        }
    }
}
