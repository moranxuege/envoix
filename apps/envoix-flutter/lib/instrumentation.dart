import 'package:flutter/widgets.dart';

import 'attachment.dart';
import 'bindings/envoix_command.dart';
import 'bindings/envoix_read.dart';
import 'commands.dart';
import 'labels.dart';

/// Every line already reported, so a rebuild does not repeat one.
final Set<String> rendered = <String>{};

/// Forgets what has been reported, because leaving a destination and coming
/// back puts its content on screen again — a fresh claim, not a repeat. The
/// ledger exists to stop a rebuild repeating a line within one screen; scoped
/// any wider it would silence the redraw a reader actually navigated to see.
void forgetRendered() => rendered.clear();

/// What the on-device instrumentation reads: one line per distinct thing this
/// app has actually PUT ON SCREEN, emitted from the widget that drew it — a
/// claim about the screen rather than about the model behind it.
void reportRendered(CardRow row) {
  _report('envoix-f1b rendered card=${row.card} '
      'epoch=${row.epoch} status=${row.status.name}');
}

/// The same claim for the logs screen: this timeline was drawn, with this many
/// entries, and said this about its own completeness.
void reportTimeline(EvidenceTimelineView timeline) {
  final String status = switch (timeline.status) {
    DiagnosticsStatusViewComplete() => 'complete',
    DiagnosticsStatusViewDegraded(:final DegradedView value) =>
      'degraded:${value.droppedEvents}',
  };
  _report('envoix-f1c timeline card=${timeline.session.card} '
      'generation=${timeline.session.generation} '
      'entries=${timeline.entries.length} diagnostics=$status');
}

/// The card as it was DRAWN, with the offer the authority currently makes for
/// it. This is how on-device instrumentation sees that a command changed
/// durable truth — and that the legality published beside it moved too.
void reportCard(CardRow row) {
  final CardView? view = row.view;
  if (view == null) {
    return;
  }
  final String actions = view.allowedActions
      .map((CommandKindView kind) => commandOf(kind).name)
      .join(',');
  _report('envoix-f2a card=${row.card} actions=$actions '
      'state=${stateLabel(view.state)}');
}

/// Where an affordance was drawn, in device pixels.
///
/// The harness taps a coordinate, and next to `Remove` a blind tap is not a
/// benign miss. So the app says where it put the button rather than the script
/// guessing: post-frame, because a widget does not know where it is until the
/// frame it is in has been laid out.
void reportAffordance(BuildContext context, String card, CommandView command) {
  _reportPoint(
    context,
    'envoix-f2a affordance card=$card command=${command.name}',
  );
}

/// Where the new-transfer sheet drew one of its controls.
///
/// The same reasoning as [reportAffordance], for the same reason: `Join` sits
/// one row below `Start sending`, so a guessed coordinate is not a benign miss
/// here either. The name is the CONTROL's, not its label, so re-wording a
/// button cannot silently change what a harness taps.
void reportSheetControl(BuildContext context, String control) {
  _reportPoint(context, 'envoix-f2b sheet control=$control');
}

/// One widget's centre in device pixels, reported post-frame — a widget does
/// not know where it is until the frame it is in has been laid out.
///
/// Laid out is not the same as landed. A control that animates in — the
/// floating action button scaling up, a bottom sheet sliding into place — is
/// mid-transform on the frame after its build, so `localToGlobal` answers with
/// a point it will never occupy. On a screen with no cards nothing rebuilds
/// afterwards to correct it, so that first answer would be the only one, and a
/// harness would tap it and miss.
///
/// "On the screen" is not the test either: a sheet is on the screen for every
/// frame of its slide. The test is that the centre STOPPED MOVING — the same
/// point on two consecutive frames is a point the control has settled at.
void _reportPoint(BuildContext context, String what) =>
    _reportPointWhenSettled(context, what, 0, null);

/// The frame budget a control gets to settle in. Material's entrance
/// transitions are ~200-300 ms, so 40 frames is generous at 60 Hz while still
/// answering for a control that never stops moving.
const int _settlingFrames = 40;

void _reportPointWhenSettled(
  BuildContext context,
  String what,
  int frame,
  Offset? previous,
) {
  WidgetsBinding.instance.addPostFrameCallback((_) {
    if (!context.mounted) {
      return;
    }
    final RenderObject? box = context.findRenderObject();
    if (box is! RenderBox || !box.hasSize) {
      return;
    }
    final double ratio = View.of(context).devicePixelRatio;
    final Offset centre = box.localToGlobal(box.size.center(Offset.zero));
    if (centre != previous && frame < _settlingFrames) {
      _reportPointWhenSettled(context, what, frame + 1, centre);
      return;
    }
    _report('$what x=${(centre.dx * ratio).round()} '
        'y=${(centre.dy * ratio).round()}');
  });
}

/// The same claim for a command: this intent was drawn, in this phase, with
/// the authority's own answer, against this card.
///
/// The phase alone is not enough for instrumentation: `settled` is equally the
/// phase of a committed pause and of a refusal, so a harness that asserted only
/// the phase would pass on a command that did nothing. The answer says which.
void reportIntent(CommandIntent intent) {
  _report('envoix-f2a command card=${intent.card} id=${intent.id} '
      'command=${intent.command.name} attempts=${intent.attempts} '
      'phase=${intent.phase.name} answer=${_answer(intent)}');
}

/// The same claim for a create request: this is what the authority answered
/// when the app asked for a card. A harness needs the answer, not the asking —
/// `refused:invite_bare_room_code` and `created:<card>` are different outcomes
/// of the same tap.
void reportCreate(CreateIntent request) {
  _report('envoix-f2b create id=${request.id} kind=${request.kind.name} '
      'answer=${_createAnswer(request)}');
}

/// The create answer as one machine token. Derived, never stored.
String _createAnswer(CreateIntent request) {
  final IntentFault? fault = request.fault;
  if (fault != null) {
    return switch (fault.origin) {
      FaultOrigin.unsent => 'unsent',
      FaultOrigin.unanswered => 'undelivered',
    };
  }
  return switch (request.outcome) {
    null => 'none',
    CreateOutcomeViewCreated(:final CardCreatedView value) =>
      'created:${value.card}',
    CreateOutcomeViewRefused(:final CreateRefusalView value) =>
      'refused:${value.name}',
  };
}

/// The card's published invite, as it was DRAWN. The room code is what the user
/// reads out, so instrumentation asserting a send flow needs to see that one
/// actually reached the screen.
void reportInvite(String card, InviteView invite) {
  _report('envoix-f2b invite card=$card code=${invite.code} '
      'link=${invite.link == null ? 'absent' : 'present'}');
}

/// The authority's answer as one machine token. Derived, never stored.
String _answer(CommandIntent intent) {
  final IntentFault? fault = intent.fault;
  if (fault != null) {
    return switch (fault.origin) {
      FaultOrigin.unsent => 'unsent',
      FaultOrigin.unanswered => 'undelivered',
    };
  }
  final CompletionView? completion = intent.completion;
  if (completion != null) {
    return switch (completion) {
      CompletionViewCommitted(:final DispositionView value) =>
        'committed:${_disposition(value)}',
      CompletionViewCommitFailed(:final DispositionView value) =>
        'commit_failed:${_disposition(value)}',
      CompletionViewInterrupted() => 'interrupted',
      CompletionViewInternal() => 'internal',
    };
  }
  return switch (intent.acceptance) {
    null => 'none',
    AcceptanceViewAccepted() => 'accepted',
    AcceptanceViewDuplicate(:final DispositionView value) =>
      'duplicate:${_disposition(value)}',
    AcceptanceViewConflict(:final CommandView value) => 'conflict:${value.name}',
    AcceptanceViewRejected(:final RejectionView value) =>
      'rejected:${value.name}',
  };
}

String _disposition(DispositionView state) => switch (state) {
      DispositionViewPreparing() => 'preparing',
      DispositionViewWaiting() => 'waiting',
      DispositionViewConnecting() => 'connecting',
      DispositionViewVerifying() => 'verifying',
      DispositionViewTransferring() => 'transferring',
      DispositionViewConfirming() => 'confirming',
      DispositionViewPaused(:final PausedStateView value) =>
        'paused:${value.origin.name}',
      DispositionViewUnconfirmed() => 'unconfirmed',
      DispositionViewCompleted() => 'completed',
      DispositionViewFailed() => 'failed',
      DispositionViewCancelled() => 'cancelled',
    };

void _report(String line) {
  if (rendered.add(line)) {
    debugPrint(line);
  }
}
