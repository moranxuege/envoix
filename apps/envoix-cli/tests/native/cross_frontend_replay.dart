// Flutter's witness for `cross_frontend_scenario_conformance`.
//
// This uses the app's Attachment and command builders, which in turn consume
// the generated Dart contracts by reference. The Rust test keeps the real Host
// alive between these short-lived Dart processes.

import 'dart:convert';
import 'dart:io';

import '../../../envoix-flutter/lib/attachment.dart';
import '../../../envoix-flutter/lib/bindings/envoix_command.dart';
import '../../../envoix-flutter/lib/bindings/envoix_read.dart';
import '../../../envoix-flutter/lib/commands.dart';


void main(List<String> arguments) {
  final String mode = arguments[0];
  final Directory work = Directory(arguments[1]);

  String text(String file) => File('${work.path}/$file').readAsStringSync();
  List<int> bytes(String file) => File('${work.path}/$file').readAsBytesSync();
  List<List<int>> frames(String file) =>
      File('${work.path}/$file').readAsLinesSync().map(utf8.encode).toList();
  void write(String file, List<int> value) =>
      File('${work.path}/$file').writeAsBytesSync(value);

  Attachment attach(String file) {
    final Attachment attachment = Attachment();
    for (final List<int> frame in frames(file)) {
      attachment.ingest(frame);
    }
    return attachment;
  }

  if (mode == 'create') {
    final String request = mintCommandId();
    File('${work.path}/create.id').writeAsStringSync(request);
    write(
      'create.frame',
      createFrame(
        id: request,
        // The Dart frontend states which side of the room it will be on, and
        // nothing about a document — the same frame the CLI emits.
        intent: const CreateIntentViewMintRoom(
          MintRoomView(localDirection: LocalDirectionView.send),
        ),
      ),
    );
    return;
  }

  if (mode == 'command') {
    final Attachment attachment = attach('opening.frames');
    final CardRow row = attachment.cards.single;
    // Matched, not `contains`: the generated unions are sealed classes without
    // value equality, so identity comparison would silently never match.
    if (!row.view!.allowedActions.any((CardActionView action) =>
        action is CardActionViewCommand &&
        action.value == CommandKindView.cancel)) {
      throw StateError('the authority did not offer cancel');
    }
    final CommandIntent intent =
        attachment.commands.open(row.card, CommandView.cancel);
    File('${work.path}/command.id').writeAsStringSync(intent.id);
    write(
      'submit.frame',
      submitFrame(
        card: row.card,
        epoch: row.epoch,
        id: intent.id,
        command: CommandView.cancel,
      ),
    );
    return;
  }

  if (mode != 'witness') {
    throw ArgumentError.value(mode, 'mode');
  }

  final String request = text('create.id');
  final CommandFrame create = decodeCommandFrame(utf8.decode(bytes('create.result')));
  final bool created = switch (create.body) {
    CommandBodyCreateResult(value: final CreateResultView result)
        when result.requestId == request =>
      result.outcome is CreateOutcomeViewCreated,
    _ => false,
  };

  final Attachment initial = attach('opening.frames');
  final CardRow before = initial.cards.single;
  final CardView beforeView = before.view!;
  final int beforeEpoch = before.epoch;
  final String command = text('command.id');
  final CommandIntent intent =
      initial.commands.open(before.card, CommandView.cancel, id: command);
  initial.ingest(bytes('accepted.frame'));
  final String acceptance = acceptanceToken(intent.acceptance);
  for (final List<int> frame in frames('settled.frames')) {
    initial.ingest(frame);
  }
  final String completion = completionToken(intent.completion);

  // A fresh app attachment contains nothing from the old one. It learns the
  // surviving card solely from the new generated read frames.
  final Attachment fresh = attach('reattached.frames');
  final CardRow after = fresh.cards.single;

  final List<String> witness = <String>[
    'created=$created',
    'direction=${directionToken(beforeView.direction)}',
    'source=${sourceToken(beforeView.source)}',
    'before_state=${stateToken(beforeView.state)}',
    'before_allowed=${beforeView.allowedActions.map(actionToken).join(',')}',
    'invite=${beforeView.invite != null}',
    'acceptance=$acceptance',
    'completion=$completion',
    'after_state=${stateToken(after.view!.state)}',
    'after_quiescence=${quiescenceToken(after.view!.quiescence)}',
    'after_allowed=${after.view!.allowedActions.map(actionToken).join(',')}',
    'card_count=${fresh.cards.length}',
    'epoch_advanced=${after.epoch > beforeEpoch}',
    'fresh_commands=${fresh.commands.forCard(after.card).length}',
  ];
  File('${work.path}/witness.txt').writeAsStringSync('${witness.join('\n')}\n');
}

/// One published action as a stable token, matching the Rust anchor exactly.
///
/// `pick_source` keeps its acquisition in SHAPE rather than by value: the two
/// witnesses drive two different cards, so comparing the key itself would only
/// prove they are different runs. Dropping it would witness nothing about an
/// action whose whole point is naming one.
String actionToken(CardActionView action) => switch (action) {
      CardActionViewCommand(:final value) => commandToken(value),
      CardActionViewPickSource(:final value) =>
        RegExp(r'^[0-9a-f]{32}$').hasMatch(value.acquisition.request)
            ? 'pick_source@<32hex>'
            : 'pick_source@malformed(${value.acquisition.request})',
    };

/// Where a card's source is, as one stable token.
String sourceToken(SourceLifecycleView source) => switch (source) {
      SourceLifecycleViewNotRequired(:final value) => value.peerContent == null
          ? 'not_required:none'
          : 'not_required:${value.peerContent!.offeredName}:${value.peerContent!.total}',
      SourceLifecycleViewAwaitingSelection(:final value) =>
        switch (value.selection) {
          SourceSelectionGateViewSelectable(value: final gate) =>
            'selectable:${gate.reason.name.toLowerCase()}',
          SourceSelectionGateViewRePickRequired(value: final gate) =>
            're_pick_required:${gate.reason.name.toLowerCase()}',
        },
      SourceLifecycleViewAcquiring(:final value) =>
        'acquiring:${value.displayName}',
      SourceLifecycleViewStaging(:final value) => 'staging:${value.displayName}',
      SourceLifecycleViewReady(:final value) =>
        'ready:${value.content.offeredName}:${value.content.total}',
    };

String directionToken(DirectionView direction) => switch (direction) {
      DirectionView.send => 'send',
      DirectionView.receive => 'receive',
    };

String commandToken(CommandKindView command) => switch (command) {
      CommandKindView.pause => 'pause',
      CommandKindView.cancel => 'cancel',
      CommandKindView.resume => 'resume',
      CommandKindView.remove => 'remove',
      CommandKindView.rePickSource => 're_pick_source',
    };

String stateToken(ProductStateView state) => switch (state) {
      ProductStateViewPreparing() => 'preparing',
      ProductStateViewWaiting() => 'waiting',
      ProductStateViewConnecting() => 'connecting',
      ProductStateViewVerifying() => 'verifying',
      ProductStateViewTransferring() => 'transferring',
      ProductStateViewConfirming() => 'confirming',
      ProductStateViewPaused(value: final PausedView paused) =>
        'paused:${pauseOriginToken(paused.origin)}',
      ProductStateViewUnconfirmed() => 'unconfirmed',
      ProductStateViewCompleted() => 'completed',
      ProductStateViewFailed() => 'failed',
      ProductStateViewCancelled() => 'cancelled',
    };

String pauseOriginToken(PauseOriginView origin) => switch (origin) {
      PauseOriginView.local => 'local',
      PauseOriginView.peer => 'peer',
      PauseOriginView.lost => 'lost',
    };

String quiescenceToken(QuiescenceView quiescence) => switch (quiescence) {
      QuiescenceViewRunning() => 'running',
      QuiescenceViewRetiring() => 'retiring',
      QuiescenceViewQuiescent() => 'quiescent',
    };

String acceptanceToken(AcceptanceView? acceptance) => switch (acceptance) {
      AcceptanceViewAccepted() => 'accepted',
      AcceptanceViewDuplicate(value: final DispositionView disposition) =>
        'duplicate:${dispositionToken(disposition)}',
      AcceptanceViewConflict(value: final CommandView command) =>
        'conflict:${command.name}',
      AcceptanceViewRejected(value: final RejectionView reason) =>
        'rejected:${reason.name}',
      null => 'missing',
    };

String completionToken(CompletionView? completion) => switch (completion) {
      CompletionViewCommitted(value: final DispositionView disposition) =>
        'committed:${dispositionToken(disposition)}',
      CompletionViewCommitFailed(value: final DispositionView disposition) =>
        'commit_failed:${dispositionToken(disposition)}',
      CompletionViewInterrupted() => 'interrupted',
      CompletionViewInternal() => 'internal',
      null => 'missing',
    };

String dispositionToken(DispositionView disposition) => switch (disposition) {
      DispositionViewPreparing() => 'preparing',
      DispositionViewWaiting() => 'waiting',
      DispositionViewConnecting() => 'connecting',
      DispositionViewVerifying() => 'verifying',
      DispositionViewTransferring() => 'transferring',
      DispositionViewConfirming() => 'confirming',
      DispositionViewPaused(value: final PausedStateView paused) =>
        'paused:${pauseCauseToken(paused.origin)}',
      DispositionViewUnconfirmed() => 'unconfirmed',
      DispositionViewCompleted() => 'completed',
      DispositionViewFailed() => 'failed',
      DispositionViewCancelled() => 'cancelled',
    };

String pauseCauseToken(PauseCauseView origin) => switch (origin) {
      PauseCauseView.local => 'local',
      PauseCauseView.peer => 'peer',
      PauseCauseView.lost => 'lost',
    };
