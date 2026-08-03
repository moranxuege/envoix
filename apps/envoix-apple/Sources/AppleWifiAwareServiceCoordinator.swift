import Foundation
import OSLog

/// Serializes process-wide access to Apple's single Wi-Fi Aware publisher and
/// subscriber roles. Fairness is FIFO per role, so unrelated transfer roles
/// can run concurrently without allowing a later request to overtake an older
/// request for the same role.
actor AppleWifiAwareServiceCoordinator {
    static let shared = AppleWifiAwareServiceCoordinator()

    private let logger = Logger(
        subsystem: Bundle.main.bundleIdentifier ?? "com.envoix.app",
        category: "wifi-aware-service"
    )

    enum Role: Hashable, Sendable {
        case publisher
        case subscriber
    }

    enum Purpose: String, Equatable, Sendable {
        case control
        case diagnostic
        case systemPairing
        case transferReceiver
        case transferSender

        var roles: Set<Role> {
            switch self {
            case .control, .diagnostic, .systemPairing:
                return [.publisher, .subscriber]
            case .transferReceiver:
                return [.publisher]
            case .transferSender:
                return [.subscriber]
            }
        }
    }

    struct Lease: Sendable {
        let purpose: Purpose
        fileprivate let token: UUID
    }

    struct Status: Equatable, Sendable {
        let activePurposes: [Purpose]
        let waitingPurposes: [Purpose]
    }

    private struct Waiter {
        let token: UUID
        let purpose: Purpose
        let continuation: CheckedContinuation<Lease, Error>
    }

    private var activePurposes: [UUID: Purpose] = [:]
    private var waiters: [Waiter] = []

    func acquire(_ purpose: Purpose) async throws -> Lease {
        try Task.checkCancellation()
        let token = UUID()

        return try await withTaskCancellationHandler {
            do {
                let lease = try await withCheckedThrowingContinuation {
                    (continuation: CheckedContinuation<Lease, Error>) in
                    guard !Task.isCancelled else {
                        continuation.resume(throwing: CancellationError())
                        return
                    }
                    waiters.append(Waiter(
                        token: token,
                        purpose: purpose,
                        continuation: continuation
                    ))
                    logger.info(
                        "WFA_LEASE event=request purpose=\(purpose.rawValue, privacy: .public) waiting=\(self.waiters.count, privacy: .public)"
                    )
                    grantEligibleWaiters()
                }
                try Task.checkCancellation()
                return lease
            } catch {
                release(token: token)
                throw error
            }
        } onCancel: {
            Task {
                await self.cancelPending(token: token)
            }
        }
    }

    func release(_ lease: Lease) {
        release(token: lease.token)
    }

    func status() -> Status {
        Status(
            activePurposes: activePurposes.values.sorted { $0.rawValue < $1.rawValue },
            waitingPurposes: waiters.map(\.purpose)
        )
    }

    private func cancelPending(token: UUID) {
        guard let index = waiters.firstIndex(where: { $0.token == token }) else {
            return
        }
        let waiter = waiters.remove(at: index)
        logger.info(
            "WFA_LEASE event=cancel purpose=\(waiter.purpose.rawValue, privacy: .public)"
        )
        waiter.continuation.resume(throwing: CancellationError())
        grantEligibleWaiters()
    }

    private func release(token: UUID) {
        guard let purpose = activePurposes.removeValue(forKey: token) else {
            return
        }
        logger.info(
            "WFA_LEASE event=release purpose=\(purpose.rawValue, privacy: .public)"
        )
        grantEligibleWaiters()
    }

    private func grantEligibleWaiters() {
        var rolesReservedByEarlierWaiters: Set<Role> = []
        var index = 0

        while index < waiters.count {
            let waiter = waiters[index]
            let roles = waiter.purpose.roles
            let rolesInUse = activePurposes.values.reduce(into: Set<Role>()) {
                $0.formUnion($1.roles)
            }
            let canRun = roles.isDisjoint(with: rolesInUse)
                && roles.isDisjoint(with: rolesReservedByEarlierWaiters)

            guard canRun else {
                rolesReservedByEarlierWaiters.formUnion(roles)
                index += 1
                continue
            }

            activePurposes[waiter.token] = waiter.purpose
            waiters.remove(at: index)
            logger.info(
                "WFA_LEASE event=grant purpose=\(waiter.purpose.rawValue, privacy: .public) active=\(self.activePurposes.count, privacy: .public)"
            )
            waiter.continuation.resume(returning: Lease(
                purpose: waiter.purpose,
                token: waiter.token
            ))
        }
    }
}
