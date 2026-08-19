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
            return AppText.localized("transfer.path.direct", language: language)
        case .directIpv4:
            return AppText.localized("transfer.path.direct_ipv4", language: language)
        case .directIpv6:
            return AppText.localized("transfer.path.direct_ipv6", language: language)
        case .relay:
            return AppText.localized("transfer.path.relay", language: language)
        case .wifiAware:
            return AppText.localized("transfer.path.wifi_aware", language: language)
        case .other:
            return AppText.localized("transfer.path.other", language: language)
        }
    }

    static func label(for event: FfiConnectionPathEvent, language: String) -> String {
        let path = label(for: event.pathKind, language: language)
        guard event.eventKind == .changed else { return path }
        let changed = AppText.localized("transfer.path.changed", language: language)
        return "\(path) · \(changed)"
    }
}

enum TransferActivityText {
    static func direction(_ direction: FfiTransferDirection, language: String) -> String {
        switch direction {
        case .send:
            return AppText.localized("transfer.direction.send", language: language)
        case .receive:
            return AppText.localized("transfer.direction.receive", language: language)
        }
    }

    static func state(
        _ state: TransferActivityState,
        direction: FfiTransferDirection,
        language: String
    ) -> String {
        let key: String
        switch state {
        case .preparing: key = "transfer.state.preparing"
        case .waitingForPeer: key = "transfer.state.waiting_for_peer"
        case .pairing: key = "transfer.state.pairing"
        case .connecting: key = "transfer.state.connecting"
        case .awaitingDecision: key = "transfer.state.awaiting_decision"
        case .transferring:
            key = direction == .send
                ? "transfer.state.sending"
                : "transfer.state.receiving"
        case .verifying: key = "transfer.state.verifying"
        case .saving: key = "transfer.state.saving"
        case .waitingForReceiverSave: key = "transfer.state.waiting_for_receiver_save"
        case .finalizingDelivery: key = "transfer.state.finalizing_delivery"
        case .paused: key = "transfer.state.paused"
        case .delivered:
            key = direction == .send
                ? "transfer.state.delivered"
                : "transfer.state.received"
        case .failed: key = "transfer.state.failed"
        case .canceled: key = "transfer.state.canceled"
        }
        return AppText.localized(key, language: language)
    }

    static func stage(_ stage: FfiTransferStage, language: String) -> String {
        let key: String
        switch stage {
        case .sessionStarted: key = "transfer.stage.started"
        case .connectionReady: key = "transfer.stage.connected"
        case .authenticationStarted: key = "transfer.stage.authenticating"
        case .authenticationComplete: key = "transfer.stage.authenticated"
        case .manifestOffer: key = "transfer.stage.offer"
        case .manifestAccepted: key = "transfer.stage.accepted"
        case .firstPayload: key = "transfer.stage.first_byte"
        case .payloadComplete: key = "transfer.stage.payload_complete"
        case .deliveryComplete: key = "transfer.stage.delivered"
        case .canceled: key = "transfer.stage.canceled"
        case .failed: key = "transfer.stage.failed"
        }
        return AppText.localized(key, language: language)
    }

    static func itemCount(_ count: UInt64, language: String) -> String {
        let displayCount = Int64(min(count, UInt64(Int64.max)))
        return AppText.localized(
            "activity.item_count",
            defaultValue: "\(displayCount) items",
            language: language
        )
    }

    static func transferCount(_ count: Int, language: String) -> String {
        let displayCount = Int64(max(count, 0))
        return AppText.localized(
            "activity.transfer_count",
            defaultValue: "\(displayCount) transfers",
            language: language
        )
    }

    static func updated(_ relative: String, language: String) -> String {
        AppText.localized(
            "activity.updated",
            defaultValue: "Updated \(relative)",
            language: language
        )
    }

    static func savedIn(_ destination: String, language: String) -> String {
        AppText.localized(
            "activity.saved.in_folder",
            defaultValue: "Saved in \(destination)",
            language: language
        )
    }

    static func savedItems(_ count: Int, language: String) -> String {
        let displayCount = Int64(max(count, 0))
        return AppText.localized(
            "activity.saved.item_count",
            defaultValue: "Saved \(displayCount) items",
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
