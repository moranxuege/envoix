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

struct TransferFailurePresentationCopy: Equatable {
    let title: String
    let detail: String
}

enum TransferStatusText {
    static func title(
        state: TransferActivityState?,
        direction: FfiTransferDirection?,
        fileName: String,
        failureTitle: String? = nil,
        language: String
    ) -> String {
        switch state {
        case nil:
            return AppText.localized("transfer.status.title.selection", language: language)
        case .waitingForPeer?:
            return AppText.localized("transfer.status.title.waiting_for_peer", language: language)
        case .pairing?:
            return AppText.localized("transfer.status.title.pairing", language: language)
        case .awaitingDecision?:
            return AppText.localized("transfer.status.title.review_incoming", language: language)
        case .transferring?:
            if !fileName.isEmpty { return fileName }
            return TransferActivityText.state(
                .transferring,
                direction: direction ?? .receive,
                language: language
            )
        case .saving?:
            return AppText.localized("transfer.status.title.saving", language: language)
        case .finalizingDelivery?:
            return AppText.localized("transfer.status.title.finalizing", language: language)
        case .paused?:
            return AppText.localized("transfer.status.title.paused", language: language)
        case .delivered?:
            return TransferActivityText.state(
                .delivered,
                direction: direction ?? .send,
                language: language
            )
        case .canceled?:
            return AppText.localized("transfer.status.title.canceled", language: language)
        case .failed?:
            return failureTitle
                ?? AppText.localized("transfer.failure.title.generic", language: language)
        case let state?:
            return TransferActivityText.state(
                state,
                direction: direction ?? .receive,
                language: language
            )
        }
    }

    static func detail(
        state: TransferActivityState?,
        direction: FfiTransferDirection?,
        statusText: String,
        failureDetail: String? = nil,
        language: String
    ) -> String? {
        switch state {
        case nil:
            return statusText.isEmpty ? nil : statusText
        case .preparing?:
            return AppText.localized("transfer.status.detail.preparing", language: language)
        case .waitingForPeer?:
            return AppText.localized("transfer.status.detail.waiting_for_peer", language: language)
        case .pairing?, .connecting?:
            return AppText.localized("transfer.status.detail.connecting", language: language)
        case .awaitingDecision?:
            return statusText.isEmpty
                ? AppText.localized("transfer.status.detail.review_incoming", language: language)
                : statusText
        case .transferring?:
            return AppText.localized("transfer.status.detail.transferring", language: language)
        case .verifying?:
            return AppText.localized("transfer.status.detail.verifying", language: language)
        case .saving?, .waitingForReceiverSave?, .finalizingDelivery?:
            return AppText.localized("transfer.status.detail.finalizing", language: language)
        case .paused?:
            return AppText.localized("transfer.status.detail.paused", language: language)
        case .delivered?:
            return AppText.localized(
                direction == .receive
                    ? "transfer.status.detail.received"
                    : "transfer.status.detail.delivered",
                language: language
            )
        case .canceled?:
            return AppText.localized("transfer.status.detail.canceled", language: language)
        case .failed?:
            return failureDetail ?? (statusText.isEmpty ? nil : statusText)
        }
    }

    static func lastStep(
        state: TransferActivityState?,
        statusText: String,
        language: String
    ) -> String? {
        let text = statusText.trimmed
        guard state == .failed, !text.isEmpty else { return nil }
        return AppText.localized(
            "transfer.status.last_step",
            defaultValue: "Last step: \(text)",
            language: language
        )
    }

    static func failureTitle(_ code: FfiFailureCode, language: String) -> String {
        let key: String
        switch code {
        case .userCanceled, .senderCanceled:
            key = "transfer.status.title.canceled"
        case .networkLost:
            key = "transfer.failure.title.connection"
        case .authenticationFailed:
            key = "transfer.failure.title.pairing"
        case .roomNotFound:
            key = "transfer.failure.title.room_unavailable"
        case .roomExpired:
            key = "transfer.failure.title.room_expired"
        case .roomFull:
            key = "transfer.failure.title.room_in_use"
        case .roomRateLimited, .endpointRateLimited, .ipRateLimited:
            key = "transfer.failure.title.try_later"
        case .roomUnderAttack:
            key = "transfer.failure.title.new_room"
        case .serverBusy:
            key = "transfer.failure.title.service_busy"
        case .malformedJoin, .unsupportedRendezvousVersion, .unsupportedFeature:
            key = "transfer.failure.title.update_required"
        case .internalError:
            key = "transfer.failure.title.generic"
        case .senderSourceUnavailable, .senderPermissionLost, .senderSourceChanged,
             .senderItemRemoved:
            key = "transfer.failure.title.source_unavailable"
        case .protocolOrIntegrityFailure:
            key = "transfer.failure.title.verification"
        case .receiverSpaceInsufficient:
            key = "transfer.failure.title.space"
        case .receiverDestinationDecisionRequired, .receiverDestinationUnavailable,
             .receiverSaveFailed, .receiverReusedObjectLost,
             .receiverFinalizationOutcomeUnknown:
            key = "transfer.failure.title.save"
        }
        return AppText.localized(key, language: language)
    }

    static func fallbackFailure(reason: String, language: String) -> TransferFailurePresentationCopy {
        let cleanReason = reason.trimmed
        let lower = cleanReason.lowercased()
        if lower.contains("mdns") && lower.contains("peers discovered") {
            return TransferFailurePresentationCopy(
                title: AppText.localized(
                    "transfer.failure.title.local_network",
                    language: language
                ),
                detail: AppText.localized(
                    "transfer.failure.local_network.detail",
                    language: language
                )
            )
        }
        let title = AppText.localized("transfer.failure.title.generic", language: language)
        if cleanReason.isEmpty {
            return TransferFailurePresentationCopy(
                title: title,
                detail: AppText.localized("transfer.failure.retry.detail", language: language)
            )
        }
        return TransferFailurePresentationCopy(title: title, detail: cleanReason)
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
