import AppKit
import Darwin
import EnvoixCore
import Foundation
import OSLog

@main
@MainActor
enum EnvoixEngineHelperMain {
    static func main() {
        let application = NSApplication.shared
        let delegate = EnvoixEngineHelperDelegate()
        application.delegate = delegate
        application.setActivationPolicy(.prohibited)
        withExtendedLifetime(delegate) {
            application.run()
        }
    }
}

@MainActor
private final class EnvoixEngineHelperDelegate: NSObject, NSApplicationDelegate {
    private static let lifecyclePollNanoseconds: UInt64 = 1_000_000_000

    private let logger = Logger(
        subsystem: MacOSAgentBoundary.helperBundleIdentifier,
        category: "lifecycle"
    )
    private var host: EnvoixEngineHelperHost?
    private var readinessTask: Task<Void, Never>?
    private var lifecycleTask: Task<Void, Never>?
    private var shutdownTask: Task<Void, Never>?
    private var signalSources: [DispatchSourceSignal] = []

    func applicationDidFinishLaunching(_ notification: Notification) {
        installTerminationSignals()
        do {
            let configuration = try MacOSAgentBoundary.hostConfiguration()
            let host = try EnvoixEngineHelperHost.start(
                configuration: configuration,
                vault: AppleApplicationVault(configuration: .macOSHelper())
            )
            self.host = host
            readinessTask = Task { @MainActor [weak self] in
                await self?.waitUntilReady(host)
            }
        } catch {
            logFailure(event: "host_start_failed", error: error)
            NSApp.terminate(nil)
        }
    }

    func applicationShouldTerminate(
        _ sender: NSApplication
    ) -> NSApplication.TerminateReply {
        readinessTask?.cancel()
        lifecycleTask?.cancel()
        guard let host else {
            return .terminateNow
        }
        guard shutdownTask == nil else {
            return .terminateCancel
        }

        logger.info("event=host_shutdown_requested")
        shutdownTask = EnvoixEngineHelperShutdownCoordinator.begin(
            host: host
        ) { [weak self] outcome in
            guard let self else { return }
            if let state = outcome.state {
                logger.info(
                    "event=host_shutdown_completed state=\(state.logValue, privacy: .public)"
                )
            } else if outcome.timedOut {
                logger.error("event=host_shutdown_timed_out")
            } else if let error = outcome.error {
                logFailure(event: "host_shutdown_failed", error: error)
            }
            self.host = nil
            NSApp.terminate(nil)
        }
        // `terminateLater` enters an AppKit nested event loop that does not
        // reliably service Swift tasks or main-queue watchdogs. Cancel this
        // attempt, keep the ordinary run loop alive during bounded shutdown,
        // then terminate again after `host` has been cleared above.
        return .terminateCancel
    }

    func applicationWillTerminate(_ notification: Notification) {
        readinessTask?.cancel()
        lifecycleTask?.cancel()
        signalSources.forEach { $0.cancel() }
        signalSources.removeAll()
    }

    private func waitUntilReady(_ host: EnvoixEngineHelperHost) async {
        do {
            let readiness = try await host.waitUntilReady()
            guard self.host === host else { return }
            logger.info(
                "event=host_ready agent_protocol=\(readiness.agentProtocolVersion) application_contract=\(readiness.applicationContractVersion)"
            )
            startLifecycleMonitor(host)
        } catch is CancellationError {
            return
        } catch {
            guard self.host === host else { return }
            logFailure(event: "host_readiness_failed", error: error)
            NSApp.terminate(nil)
        }
    }

    private func startLifecycleMonitor(_ host: EnvoixEngineHelperHost) {
        lifecycleTask?.cancel()
        lifecycleTask = Task { @MainActor [weak self] in
            while !Task.isCancelled {
                do {
                    try await Task.sleep(nanoseconds: Self.lifecyclePollNanoseconds)
                } catch {
                    return
                }
                guard let self, self.host === host else { return }
                switch host.lifecycle() {
                case .starting, .ready, .stopping:
                    continue
                case .stopped:
                    logger.info("event=host_stopped")
                    NSApp.terminate(nil)
                    return
                case let .failed(failure):
                    logger.error(
                        "event=host_runtime_failed code=\(failure.code.logValue, privacy: .public)"
                    )
                    NSApp.terminate(nil)
                    return
                }
            }
        }
    }

    private func installTerminationSignals() {
        for signalNumber in [SIGTERM, SIGINT] {
            Darwin.signal(signalNumber, SIG_IGN)
            let source = DispatchSource.makeSignalSource(
                signal: signalNumber,
                queue: .main
            )
            source.setEventHandler {
                NSApp.terminate(nil)
            }
            source.resume()
            signalSources.append(source)
        }
    }

    private func logFailure(event: String, error: Error) {
        logger.error(
            "event=\(event, privacy: .public) code=\(error.agentHostLogCode, privacy: .public)"
        )
    }
}

private extension Error {
    var agentHostLogCode: String {
        if let hostError = self as? FfiAgentHostError,
           case let .Failed(code, _) = hostError {
            return code.logValue
        }
        if let boundaryError = self as? MacOSAgentBoundaryError {
            switch boundaryError {
            case .applicationSupportUnavailable:
                return "application_support_unavailable"
            case .invalidControlEndpoint:
                return "invalid_control_endpoint"
            case .incompatibleCore:
                return "incompatible_core"
            case .incompatibleReadiness:
                return "incompatible_readiness"
            }
        }
        return "internal"
    }
}

private extension FfiAgentHostErrorCode {
    var logValue: String {
        switch self {
        case .unsupportedPlatform: return "unsupported_platform"
        case .invalidConfiguration: return "invalid_configuration"
        case .stateAlreadyOwned: return "state_already_owned"
        case .unsupportedPersistentState: return "unsupported_persistent_state"
        case .stateCorrupt: return "state_corrupt"
        case .vaultUnavailable: return "vault_unavailable"
        case .vaultInteractionRequired: return "vault_interaction_required"
        case .vaultPermissionDenied: return "vault_permission_denied"
        case .vaultCorrupt: return "vault_corrupt"
        case .vaultCanceled: return "vault_canceled"
        case .ioFailure: return "io_failure"
        case .shutdownBeforeReady: return "shutdown_before_ready"
        case .`internal`: return "internal"
        }
    }
}

private extension FfiAgentHostLifecycleState {
    var logValue: String {
        switch self {
        case .starting: return "starting"
        case .ready: return "ready"
        case .stopping: return "stopping"
        case .stopped: return "stopped"
        case .failed: return "failed"
        }
    }
}
