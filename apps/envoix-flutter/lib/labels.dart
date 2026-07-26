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
import 'bindings/envoix_command.dart';
import 'bindings/envoix_read.dart';
import 'commands.dart';

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
      CapabilityActionView.selectSource => 'open the file you chose',
    };

/// Why the authority would not create a card. Every one of these is ITS answer:
/// this app never looks at an invite, so it has no opinion to render instead.
String createRefusalLabel(CreateRefusalView refusal) => switch (refusal) {
      CreateRefusalView.inviteNotRecognized =>
        'That is not an Envoix invite.',
      CreateRefusalView.inviteBareRoomCode =>
        'That is only the room code. Paste the whole invite.',
      CreateRefusalView.inviteMalformed =>
        'That invite is damaged — some of it did not survive the copy.',
      CreateRefusalView.inviteTooLong => 'That invite is too long to be one.',
      CreateRefusalView.inviteUnsupported =>
        'This version of Envoix cannot use that invite.',
      CreateRefusalView.inviteRoleUnsupported =>
        'That invite asks you to send a file. Start a send instead.',
      CreateRefusalView.storageFault =>
        'The transfer could not be written to storage, so nothing was created.',
      CreateRefusalView.internal =>
        'Something inside Envoix failed before the transfer was created.',
    };

/// The authority's answer to one create request, or the honest absence of one.
String createAnswerLabel(CreateIntent request) {
  final IntentFault? fault = request.fault;
  if (fault != null) {
    return switch (fault.origin) {
      // Provably nothing to look for: the encoder refused this before the
      // frame left, so no card can exist and sending the user hunting for one
      // would be a lie the lane went out of its way to avoid telling.
      FaultOrigin.unsent =>
        'Not sent — Envoix could not put this request on the wire ($fault). '
            'Nothing was created.',
      // NOT a verdict: the request may or may not have made a card, and only
      // the card list can say. Offering a guess here would be inventing one.
      FaultOrigin.unanswered => 'No answer arrived ($fault). '
          'If a transfer appears in the list, it was created.',
    };
  }
  return switch (request.outcome) {
    null => 'Asked — no answer yet.',
    CreateOutcomeViewCreated(:final CardCreatedView value) =>
      'Created — transfer ${value.card} exists.',
    CreateOutcomeViewRefused(:final CreateRefusalView value) =>
      'Refused — ${createRefusalLabel(value)}',
  };
}

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

/// What one command asks for. The button says this, and so does the account of
/// what happened to it.
String commandLabel(CommandView command) => switch (command) {
      CommandView.pause => 'Pause',
      CommandView.cancel => 'Cancel',
      CommandView.resume => 'Resume',
      CommandView.remove => 'Remove',
      CommandView.rePickSource => 'Pick the source again',
    };

/// The state the authority recorded against a command. It is the card's
/// disposition at that moment, not a claim about the command's success.
String dispositionLabel(DispositionView state) => switch (state) {
      DispositionViewPreparing() => 'preparing',
      DispositionViewWaiting() => 'waiting for a peer',
      DispositionViewConnecting() => 'connecting',
      DispositionViewVerifying() => 'verifying',
      DispositionViewTransferring() => 'transferring',
      DispositionViewConfirming() => 'confirming',
      DispositionViewPaused(:final PausedStateView value) =>
        'paused ${pauseCauseLabel(value.origin)}',
      DispositionViewUnconfirmed() => 'delivery unconfirmed',
      DispositionViewCompleted() => 'completed',
      DispositionViewFailed() => 'failed',
      DispositionViewCancelled() => 'cancelled',
    };

String pauseCauseLabel(PauseCauseView cause) => switch (cause) {
      PauseCauseView.local => 'by you',
      PauseCauseView.peer => 'by the peer',
      PauseCauseView.lost => 'after losing the connection',
    };

/// Why the authority refused a command at intake. Every reason is typed, and
/// none of them means the command quietly did nothing.
String rejectionReasonLabel(RejectionView reason) => switch (reason) {
      RejectionView.unknownCard => 'the host holds no such card',
      RejectionView.staleEpoch =>
        'a newer view of this app is in charge — re-attach and try again',
      RejectionView.superseded =>
        'a newer view of this app took over while it was queued',
      RejectionView.atCapacity => 'the host has no room to run this card',
      RejectionView.runtimeStopped => 'the transfer runtime has stopped',
      RejectionView.interrupted =>
        'the host died before it could say whether it applied',
      RejectionView.internal => 'an internal fault',
    };

/// The intake verdict. Acceptance is NOT completion: an accepted command has
/// not crossed the durability barrier, and this must never read as if it had.
String acceptanceLabel(AcceptanceView acceptance) => switch (acceptance) {
      AcceptanceViewAccepted() => 'Accepted — not committed yet',
      AcceptanceViewDuplicate(:final DispositionView value) =>
        'Already applied — the card was ${dispositionLabel(value)}',
      // The authority knows WHICH command owns the reused identity, so this
      // names it. "Conflict" alone tells a user nothing they can act on.
      AcceptanceViewConflict(:final CommandView value) =>
        'Refused — that request id already belongs to ${commandLabel(value)}',
      // `interrupted` is the one intake answer that is NOT a refusal: the
      // authority does not know whether the command applied, so calling it
      // refused would be the app deciding. Same words as the completion arm,
      // because it is the same question.
      AcceptanceViewRejected(value: RejectionView.interrupted) =>
        'Unknown — the host died before it could say. Ask again with the same '
            'request to find out.',
      AcceptanceViewRejected(:final RejectionView value) =>
        'Refused — ${rejectionReasonLabel(value)}',
    };

/// The committed completion: what actually became durable, or honestly why not.
String completionLabel(CompletionView completion) => switch (completion) {
      CompletionViewCommitted(:final DispositionView value) =>
        'Committed — the card is ${dispositionLabel(value)}',
      CompletionViewCommitFailed(:final DispositionView value) =>
        'Not durable — it was rolled back and the card stays '
            '${dispositionLabel(value)}',
      CompletionViewInterrupted() =>
        'Unknown — the host died before it could say. Ask again with the same '
            'request to find out.',
      CompletionViewInternal() => 'An internal fault ended it',
    };

/// One intent, as one line. The phase is derived from the answers, so this
/// cannot say "committed" about something nothing committed.
String intentLabel(CommandIntent intent) {
  final CompletionView? completion = intent.completion;
  final AcceptanceView? acceptance = intent.acceptance;
  final String asked = intent.attempts > 1
      ? '${commandLabel(intent.command)} (asked again)'
      : commandLabel(intent.command);
  return switch (intent.phase) {
    CommandPhase.submitted => '$asked — sent, no answer yet',
    CommandPhase.accepted => '$asked — ${acceptanceLabel(acceptance!)}',
    CommandPhase.settled => completion == null
        ? '$asked — ${acceptanceLabel(acceptance!)}'
        : '$asked — ${completionLabel(completion)}',
    CommandPhase.refused =>
      '$asked — not sent; Envoix could not put it on the wire (${intent.fault})',
    CommandPhase.undelivered =>
      '$asked — it never reached the host (${intent.fault})',
  };
}

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
