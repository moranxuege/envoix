/// Human text for every variant the read contract can express.
///
/// Every function here is one exhaustive `switch` with no default arm, so a
/// variant added to the generated contract stops this file compiling. That is
/// the whole point: `.name` would have kept compiling and quietly shown the
/// wire spelling of a state nobody had thought about, and an unhandled arm
/// would have shown nothing at all.
///
/// Nothing here decides anything. Each function maps one authoritative value to
/// the words for it; no status is inferred, no action legality is computed, and
/// no fact the contract does not carry is invented.

library;

import 'attachment.dart';
import 'bindings/envoix_read.dart';

/// The card's lifecycle state, including which side rested it.
String stateLabel(ProductStateView state) => switch (state) {
      ProductStateViewPreparing() => 'Preparing',
      ProductStateViewWaiting() => 'Waiting for a peer',
      ProductStateViewConnecting() => 'Connecting',
      ProductStateViewVerifying() => 'Verifying',
      ProductStateViewTransferring() => 'Transferring',
      ProductStateViewConfirming() => 'Confirming',
      ProductStateViewPaused(:final PausedView value) =>
        'Paused ${pauseOriginLabel(value.origin)}',
      ProductStateViewUnconfirmed() => 'Delivery unconfirmed',
      ProductStateViewCompleted() => 'Completed',
      ProductStateViewFailed() => 'Failed',
      ProductStateViewCancelled() => 'Cancelled',
    };

String pauseOriginLabel(PauseOriginView origin) => switch (origin) {
      PauseOriginView.local => 'by you',
      PauseOriginView.peer => 'by the peer',
      PauseOriginView.lost => 'after losing the connection',
    };

String directionLabel(DirectionView direction) => switch (direction) {
      DirectionView.send => 'Sending',
      DirectionView.receive => 'Receiving',
    };

/// Where the work has reached inside the current state.
String phaseLabel(PhaseView phase) => switch (phase) {
      PhaseView.preparing => 'preparing the source',
      PhaseView.pairing => 'pairing with the peer',
      PhaseView.authenticating => 'authenticating',
      PhaseView.transferring => 'moving bytes',
      PhaseView.confirming => 'confirming delivery',
      PhaseView.publishing => 'publishing the file',
      PhaseView.restoring => 'restoring',
    };

/// Whether a worker is still running, retiring, or gone. A resting state means
/// nothing until this says the work behind it has actually stopped.
String quiescenceLabel(QuiescenceView quiescence) => switch (quiescence) {
      QuiescenceViewRunning(:final RunningView value) =>
        '${workerLabel(value.worker)} running',
      QuiescenceViewRetiring(:final RetiringView value) =>
        '${workerLabel(value.worker)} stopping to ${retirementLabel(value.intent)}',
      QuiescenceViewQuiescent() => 'no work in flight',
    };

String workerLabel(WorkerKindView worker) => switch (worker) {
      WorkerKindView.attempt => 'transfer',
      WorkerKindView.staging => 'staging',
    };

String retirementLabel(RetirementIntentView intent) => switch (intent) {
      RetirementIntentView.pause => 'pause',
      RetirementIntentView.cancel => 'cancel',
      RetirementIntentView.finalize => 'finish',
    };

String outcomeCodeLabel(OutcomeCodeView code) => switch (code) {
      OutcomeCodeView.completed => 'Completed',
      OutcomeCodeView.cancelled => 'Cancelled',
      OutcomeCodeView.paused => 'Paused',
      OutcomeCodeView.peerLost => 'The peer went away',
      OutcomeCodeView.timeout => 'Timed out',
      OutcomeCodeView.unauthenticated => 'The peer could not be authenticated',
      OutcomeCodeView.versionMismatch => 'The two sides speak different versions',
      OutcomeCodeView.storageFault => 'Storage fault',
      OutcomeCodeView.publishFailed => 'The file could not be published',
      OutcomeCodeView.sourceUnreadable => 'The source could not be read',
      OutcomeCodeView.networkUnreachable => 'The network was unreachable',
      OutcomeCodeView.internal => 'Internal fault',
    };

String retryabilityLabel(RetryabilityView retry) => switch (retry) {
      RetryabilityView.retryable => 'can be retried',
      RetryabilityView.terminal => 'final',
      RetryabilityView.needsUser => 'needs you',
    };

/// What the authority says would help. It is a hint, not an offer: the command
/// that acts on it arrives with F2.
String recoveryLabel(RecoveryView recovery) => switch (recovery) {
      RecoveryView.rePickSource => 'Pick the source file again',
      RecoveryView.retryLater => 'Try again later',
      RecoveryView.reconnectPeer => 'Reconnect to the peer',
    };

String refusalLabel(SubscribeRejectionView reason) => switch (reason) {
      SubscribeRejectionView.unknownCard => 'the host holds no such card',
      SubscribeRejectionView.runtimeStopped => 'the transfer runtime has stopped',
      SubscribeRejectionView.epochExhausted => 'the card ran out of stream epochs',
    };

String losslessLabel(LosslessKindView missed) => switch (missed) {
      LosslessKindView.terminal => 'a final update',
      LosslessKindView.capabilityDuty => 'a system duty',
    };

String dutyKindLabel(DutyKindView kind) => switch (kind) {
      DutyKindView.sourceHandle => 'open the source',
      DutyKindView.grant => 'hold a permission grant',
      DutyKindView.staging => 'stage the file',
      DutyKindView.publication => 'publish the file',
      DutyKindView.courier => 'carry a receipt',
      DutyKindView.foreground => 'keep the service in the foreground',
      DutyKindView.notification => 'show a notification',
      DutyKindView.lock => 'hold a network lock',
      DutyKindView.openShare => 'open or share the file',
    };

String capabilityActionLabel(CapabilityActionView action) => switch (action) {
      CapabilityActionView.postReceipt => 'post the receipt',
    };

/// One evidence entry. The identifier case is deliberately thin: evidence
/// carries the KIND of identifier that was involved and never its value.
String evidenceLabel(EvidenceValueView value) => switch (value) {
      EvidenceValueViewPhase(:final PhaseView value) => 'Reached ${phaseLabel(value)}',
      EvidenceValueViewProgress(:final EvidenceProgressView value) =>
        'Transferred ${value.transferred} of ${value.total} bytes',
      EvidenceValueViewOutcome(:final OutcomeView value) =>
        '${outcomeCodeLabel(value.code)} while ${phaseLabel(value.phase)} — ${value.display}',
      EvidenceValueViewIdentifier(:final RedactedIdView value) =>
        'Recorded a ${redactedIdLabel(value.kind)} identifier',
    };

String redactedIdLabel(RedactedIdKindView kind) => switch (kind) {
      RedactedIdKindView.record => 'card',
      RedactedIdKindView.transfer => 'transfer',
      RedactedIdKindView.artifact => 'artifact',
      RedactedIdKindView.request => 'request',
    };

/// Whether a timeline is the whole story. A degraded one must never be shown
/// as complete, so this says what was lost and how much.
String diagnosticsLabel(DiagnosticsStatusView status) => switch (status) {
      DiagnosticsStatusViewComplete() => 'Complete',
      DiagnosticsStatusViewDegraded(:final DegradedView value) =>
        'Incomplete — ${value.droppedEvents} '
            '${value.droppedEvents == 1 ? 'entry was' : 'entries were'} dropped',
    };

String trustRootLabel(TrustRootView root) => switch (root) {
      TrustRootViewUnprovisioned() => 'not provisioned',
      TrustRootViewSha256(:final TrustRootSha256View value) =>
        'sha256 ${value.fingerprint}',
    };

/// What the lane last said about one card's stream.
String streamLabel(CardRow row) => switch (row.status) {
      StreamStatus.live => 'Live',
      StreamStatus.lagged => 'Stopped after dropping '
          '${row.missed == null ? 'an update' : losslessLabel(row.missed!)}',
      StreamStatus.closed => 'Closed by the host',
    };

/// Why a frame the lane delivered changed nothing on screen.
String rejectionLabel(FrameRejection kind) => switch (kind) {
      FrameRejection.staleEpoch => 'superseded',
      FrameRejection.contractBreach => 'out of contract',
      FrameRejection.undecodable => 'undecodable',
    };

/// How far the transfer has got, or null when the total makes that unanswerable.
/// Two authoritative numbers presented together; nothing here is a status.
double? progressOf(CardView view) {
  if (view.total == 0) {
    return null;
  }
  return view.bytes.clamp(0, view.total) / view.total;
}

/// The same fraction as whole percent, for screen readers.
String percentLabel(CardView view) {
  final double? progress = progressOf(view);
  return progress == null ? 'unknown' : '${(progress * 100).round()} percent';
}
