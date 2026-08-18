import Foundation
import EnvoixCore

struct ActivityActionAvailability: Equatable {
    let canPause: Bool
    let canResume: Bool
    let canCancel: Bool
    let canApprove: Bool
    let canDelete: Bool
    let isFinalizing: Bool
}

enum TransferProgressPresentation: Equatable {
    case hidden
    case active
    case complete
    case retained
}

/// Decides when choosing a new source becomes a new draft rather than a
/// continuation of the activity currently owned by the view model.
///
/// Terminal activities remain in `AppModel.activities` as history, but must be
/// detached from the live sender before manifest preparation begins. Otherwise
/// the preparation state can inherit the old activity and keep Send disabled
/// after the new manifest is ready.
enum TransferDraftLifecyclePolicy {
    static func shouldDetachActivityBeforePreparation(
        _ state: TransferActivityState?
    ) -> Bool {
        state.map(TransferPresentationPolicy.isTerminal) ?? false
    }
}

enum ConnectionPathPresentationPolicy {
    static func label(for path: FfiDataPathKind, language: String) -> String {
        switch path {
        case .direct:
            return AppText.value("Data path · Direct", "数据路径 · 直连", language: language)
        case .directIpv4:
            return AppText.value(
                "Data path · Direct · IPv4",
                "数据路径 · 直连 · IPv4",
                language: language
            )
        case .directIpv6:
            return AppText.value(
                "Data path · Direct · IPv6",
                "数据路径 · 直连 · IPv6",
                language: language
            )
        case .relay:
            return AppText.value("Data path · Relay", "数据路径 · 中继", language: language)
        case .wifiAware:
            return AppText.value(
                "Data path · Wi‑Fi Aware",
                "数据路径 · Wi‑Fi Aware",
                language: language
            )
        case .other:
            return AppText.value("Data path · Other", "数据路径 · 其他", language: language)
        }
    }

    static func label(for event: FfiConnectionPathEvent, language: String) -> String {
        let path = label(for: event.pathKind, language: language)
        guard event.eventKind == .changed else { return path }
        return AppText.value(
            "\(path) · changed",
            "\(path) · 已切换",
            language: language
        )
    }
}

enum ActivityStageTimingPresentationPolicy {
    private static let microsecondsPerMillisecond: UInt64 = 1_000
    private static let microsecondsPerSecond: UInt64 = 1_000_000
    private static let secondsPerMinute: UInt64 = 60

    static func latestAttempt(
        from samples: [ActivityStageTimingSample]
    ) -> [ActivityStageTimingSample] {
        guard let latestAttemptID = samples.map(\.attemptID).max() else { return [] }
        return samples.filter { $0.attemptID == latestAttemptID }.sorted {
            if $0.elapsedMicroseconds != $1.elapsedMicroseconds {
                return $0.elapsedMicroseconds < $1.elapsedMicroseconds
            }
            return $0.diagnosticLine < $1.diagnosticLine
        }
    }

    static func elapsedString(microseconds: UInt64) -> String {
        if microseconds < microsecondsPerMillisecond {
            return "<1 ms"
        }
        if microseconds < microsecondsPerSecond {
            let roundedMilliseconds =
                (microseconds + microsecondsPerMillisecond / 2)
                / microsecondsPerMillisecond
            return "\(roundedMilliseconds) ms"
        }

        let wholeSeconds = microseconds / microsecondsPerSecond
        if wholeSeconds >= secondsPerMinute {
            return "\(wholeSeconds / secondsPerMinute)m "
                + "\(wholeSeconds % secondsPerMinute)s"
        }

        let seconds = Double(microseconds) / Double(microsecondsPerSecond)
        return seconds < 10
            ? String(format: "%.2f s", locale: Locale(identifier: "en_US_POSIX"), seconds)
            : String(format: "%.1f s", locale: Locale(identifier: "en_US_POSIX"), seconds)
    }
}

enum TransferMetricFreshnessPolicy {
    static let maximumCurrentMetricAge: TimeInterval = 2.5

    static func isFresh(sampledAt: Date?, now: Date) -> Bool {
        guard let sampledAt else { return false }
        let age = now.timeIntervalSince(sampledAt)
        return age >= 0 && age <= maximumCurrentMetricAge
    }
}

/// Filters low-level manifest phases before they become receiver-facing state.
///
/// Manifest V2 can emit `verifying` for an individual entry and then return to
/// `transferring` for the next entry. That detail is useful diagnostically but
/// reads as a backwards jump in the product UI. Sender phases remain an exact
/// projection of the core events.
enum TransferPhasePresentationPolicy {
    static func shouldSurface(
        _ next: FfiManifestV2Phase,
        direction: FfiTransferDirection,
        currentState: TransferActivityState?,
        observedBytes: UInt64,
        totalBytes: UInt64
    ) -> Bool {
        guard direction == .receive else { return true }

        switch next {
        case .verifying:
            return observedBytes >= totalBytes && currentState != .verifying
        case .transferring:
            return currentState != .transferring && currentState != .verifying
        default:
            return true
        }
    }
}

/// Pure lifecycle-to-presentation policy shared by every Apple transfer
/// surface. SwiftUI renders this result and does not infer actions or progress
/// behavior independently.
enum TransferPresentationPolicy {
    static func actions(
        for state: TransferActivityState,
        failure: FfiTransferFailure? = nil
    ) -> ActivityActionAvailability {
        let canCancel: Bool
        switch state {
        case .waitingForPeer, .pairing, .connecting, .transferring, .verifying:
            canCancel = true
        case .preparing, .awaitingDecision:
            canCancel = true
        case .paused:
            canCancel = true
        case .saving, .waitingForReceiverSave, .finalizingDelivery,
             .delivered, .failed, .canceled:
            canCancel = false
        }
        return ActivityActionAvailability(
            // InviteV2 is one-use after authentication. Until continuation is
            // backed by renewable remembered credentials on both platforms,
            // Pause/Resume would be a false promise; cancel and re-offer.
            canPause: false,
            canResume: false,
            canCancel: canCancel,
            canApprove: state == .awaitingDecision,
            canDelete: isTerminal(state),
            isFinalizing: isFinalizing(state)
        )
    }

    static func allowsInPlaceResume(_ failure: FfiTransferFailure?) -> Bool {
        guard let failure, failure.retryable else { return false }
        return failure.recoveryAction == .retry || failure.recoveryAction == .resume
    }

    static func terminalState(for failure: FfiTransferFailure) -> TransferActivityState {
        switch failure.outcome {
        case .canceled: return .canceled
        case .failed: return .failed
        }
    }

    static func shouldReleaseSession(after failure: FfiTransferFailure) -> Bool {
        failure.sessionDisposition == .release
    }

    static func progress(for state: TransferActivityState) -> TransferProgressPresentation {
        switch state {
        case .preparing, .waitingForPeer, .pairing, .connecting, .awaitingDecision:
            return .hidden
        case .transferring:
            return .active
        case .verifying, .saving, .waitingForReceiverSave, .finalizingDelivery:
            return .complete
        case .delivered:
            return .hidden
        case .paused, .failed, .canceled:
            return .retained
        }
    }

    static func isFinalizing(_ state: TransferActivityState) -> Bool {
        switch state {
        case .saving, .waitingForReceiverSave, .finalizingDelivery:
            return true
        default:
            return false
        }
    }

    static func isTerminal(_ state: TransferActivityState) -> Bool {
        switch state {
        case .delivered, .failed, .canceled:
            return true
        default:
            return false
        }
    }
}

func activityActionAvailability(for record: TransferActivityRecord) -> ActivityActionAvailability {
    TransferPresentationPolicy.actions(for: record.state, failure: record.failure)
}
