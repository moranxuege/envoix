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

enum ConnectionPathPresentationPolicy {
    static func label(for path: FfiDataPathKind, language: String) -> String {
        switch path {
        case .direct: return AppText.value("Direct path", "直连链路", language: language)
        case .relay: return AppText.value("Relay path", "中继链路", language: language)
        case .other: return AppText.value("Other path", "其他链路", language: language)
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

/// Pure lifecycle-to-presentation policy shared by every Apple transfer
/// surface. SwiftUI renders this result and does not infer actions or progress
/// behavior independently.
enum TransferPresentationPolicy {
    static func actions(
        for state: TransferActivityState,
        failure: FfiTransferFailure? = nil
    ) -> ActivityActionAvailability {
        let canPause: Bool
        let canCancel: Bool
        switch state {
        case .waitingForPeer, .pairing, .connecting, .transferring, .verifying:
            canPause = true
            canCancel = true
        case .preparing, .awaitingDecision:
            canPause = false
            canCancel = true
        case .paused:
            canPause = false
            canCancel = true
        case .saving, .waitingForReceiverSave, .finalizingDelivery,
             .delivered, .failed, .canceled:
            canPause = false
            canCancel = false
        }
        return ActivityActionAvailability(
            canPause: canPause,
            canResume: state == .paused || (state == .failed && failure?.retryable == true),
            canCancel: canCancel,
            canApprove: state == .awaitingDecision,
            canDelete: isTerminal(state),
            isFinalizing: isFinalizing(state)
        )
    }

    static func progress(for state: TransferActivityState) -> TransferProgressPresentation {
        switch state {
        case .preparing, .waitingForPeer, .pairing, .connecting, .awaitingDecision:
            return .hidden
        case .transferring:
            return .active
        case .verifying, .saving, .waitingForReceiverSave, .finalizingDelivery, .delivered:
            return .complete
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
