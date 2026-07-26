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
  WidgetsBinding.instance.addPostFrameCallback((_) {
    final RenderObject? box = context.findRenderObject();
    if (box is! RenderBox || !box.hasSize) {
      return;
    }
    final double ratio = View.of(context).devicePixelRatio;
    final Offset centre = box.localToGlobal(box.size.center(Offset.zero));
    _report('envoix-f2a affordance card=$card command=${command.name} '
        'x=${(centre.dx * ratio).round()} y=${(centre.dy * ratio).round()}');
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

/// The authority's answer as one machine token. Derived, never stored.
String _answer(CommandIntent intent) {
  if (intent.fault != null) {
    return 'undelivered';
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
