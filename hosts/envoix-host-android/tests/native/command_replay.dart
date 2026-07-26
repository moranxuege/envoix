// The Dart half of `flutter_mutating_hot_restart_preserves_cards`.
//
// It runs the APP's own view model and the APP's own encoder — not copies of
// them — against a real running host: it reads what the authority says is
// admissible for a real card, encodes the submit frames a tap would produce,
// and afterwards reports what the frontend surfaced from the real acceptance
// and completion frames that host produced.
//
// A plain script rather than a package test, so `cargo test` can execute it
// with no pub resolution and no Flutter engine. `Commander` lives in
// `lane.dart`, which imports Flutter, so the driver plays the commander's part
// by hand — the tap-to-bytes half of that path is `flutter test`'s.
//
// Usage: dart run command_replay.dart issue  <directory> <card>
//        dart run command_replay.dart render <directory> <card>

import 'dart:convert';
import 'dart:io';

import '../../../../apps/envoix-flutter/lib/attachment.dart';
import '../../../../apps/envoix-flutter/lib/bindings/envoix_command.dart';
import '../../../../apps/envoix-flutter/lib/bindings/envoix_read.dart';
import '../../../../apps/envoix-flutter/lib/commands.dart';
import '../../../../apps/envoix-flutter/lib/labels.dart';

int _checks = 0;
final List<String> _failures = <String>[];

void check(String what, bool holds) {
  _checks += 1;
  if (!holds) {
    _failures.add(what);
  }
}

void equal(String what, Object? found, Object? expected) {
  check('$what: expected $expected, found $found', found == expected);
}

void main(List<String> arguments) {
  final String mode = arguments[0];
  final Directory work = Directory(arguments[1]);
  final String card = arguments[2];

  String text(String name) => File('${work.path}/$name').readAsStringSync();
  List<int> frame(String name) => utf8.encode(text(name));
  List<List<int>> frames(String name) =>
      File('${work.path}/$name').readAsLinesSync().map(utf8.encode).toList();

  Attachment attachedTo(String name) {
    final Attachment attachment = Attachment();
    for (final List<int> bytes in frames(name)) {
      attachment.ingest(bytes);
    }
    return attachment;
  }

  void write(String name, List<int> bytes) =>
      File('${work.path}/$name').writeAsBytesSync(bytes);

  if (mode == 'issue') {
    // 1. The affordances come from the authority, not from the state beside
    //    them: the card publishes what it will admit, and this is all the app
    //    knows about legality.
    final Attachment attachment = attachedTo('live.frames');
    final CardRow row = attachment.cards.single;
    equal('card id', row.card, card);
    final CardView view = row.view!;
    equal(
      'the authority offers what a live send offers',
      view.allowedActions.map(commandOf).map(commandLabel).join(','),
      'Pause,Cancel,Remove',
    );

    // 2. One identity for this intent, and the app's own encoder to spell it.
    final CommandIntent intent =
        attachment.commands.open(row.card, CommandView.pause);
    check('the identity is a hex32', RegExp(r'^[0-9a-f]{32}$').hasMatch(intent.id));
    write('submit.frame', submitFrame(
      card: row.card,
      epoch: row.epoch,
      id: intent.id,
      command: CommandView.pause,
    ));
    // The same identity carrying a DIFFERENT command: a conflict, not a
    // plausible-looking duplicate.
    write('conflict.frame', submitFrame(
      card: row.card,
      epoch: row.epoch,
      id: intent.id,
      command: CommandView.cancel,
    ));
    File('${work.path}/command.id').writeAsStringSync(intent.id);
    report();
    return;
  }

  final String id = text('command.id');

  // 3. The attachment that issued the command, fed the REAL acceptance and the
  //    REAL completion the host produced for it.
  final Attachment attachment = attachedTo('live.frames');
  attachment.commands.open(card, CommandView.pause, id: id);
  final CommandIntent intent = attachment.commands.forCard(card).single;

  attachment.ingest(frame('accepted.frame'));
  equal('acceptance is not completion', intent.phase, CommandPhase.accepted);
  equal(
    'and it says so',
    intentLabel(intent),
    'Pause — Accepted — not committed yet',
  );

  attachment.ingest(frame('completed.frame'));
  equal('the committed completion settles it', intent.phase, CommandPhase.settled);
  equal(
    'and names the durable disposition',
    intentLabel(intent),
    'Pause — Committed — the card is paused by you',
  );

  // 4. The same identity again: the host answers from its durable ledger. This
  //    is the `Interrupted` disambiguation's committed arm, run for real.
  attachment.commands.open(card, CommandView.pause, id: id);
  attachment.ingest(frame('duplicate.frame'));
  equal(
    'a re-issued identity is answered from committed truth',
    intentLabel(attachment.commands.forCard(card).single),
    'Pause (asked again) — Already applied — the card was paused by you',
  );

  // 5. The same identity with a different command is refused typed, and the
  //    refusal NAMES the command that owns the identity — the host knew it, so
  //    the contract carries it and the user is told what they collided with.
  final Attachment conflicting = attachedTo('live.frames');
  conflicting.commands.open(card, CommandView.cancel, id: id);
  conflicting.ingest(frame('conflict.frame'));
  equal(
    'a reused identity with a different command names the applied one',
    intentLabel(conflicting.commands.forCard(card).single),
    'Cancel — Refused — that request id already belongs to Pause',
  );

  // 6. A command issued from an attachment the host has replaced is refused,
  //    typed, and surfaced — never silently dropped.
  final Attachment superseded = attachedTo('live.frames');
  superseded.commands.open(card, CommandView.pause, id: id);
  superseded.ingest(frame('stale.frame'));
  equal(
    'a superseded attachment cannot command',
    intentLabel(superseded.commands.forCard(card).single),
    'Pause — Refused — a newer view of this app is in charge — re-attach and '
        'try again',
  );

  // 7. The restart: a new attachment, re-seeded by the host. The card carries
  //    the committed effect, the offer has moved with it, and the frontend
  //    remembers nothing at all about the command that caused either.
  final Attachment restarted = attachedTo('restart.frames');
  final CardRow row = restarted.cards.single;
  equal('the card survived', row.card, card);
  equal('with the effect the command committed', stateLabel(row.view!.state),
      'Paused by you');
  equal(
    'and the offer the authority now makes',
    row.view!.allowedActions.map(commandOf).map(commandLabel).join(','),
    'Resume,Cancel,Remove',
  );
  check('a fresh epoch', row.epoch > attachment.cards.single.epoch);
  equal('and no command state at all', restarted.commands.forCard(card).length, 0);
  equal('nor an unaddressed answer', restarted.commands.unaddressed, 0);
  for (final FrameRejection kind in FrameRejection.values) {
    equal('restart frames rejected as ${kind.name}', restarted.rejected(kind), 0);
  }
  report();
}

void report() {
  if (_failures.isEmpty) {
    print('command replay: $_checks checks pass');
    return;
  }
  for (final String failure in _failures) {
    stderr.writeln('FAIL $failure');
  }
  stderr.writeln('${_failures.length} of $_checks checks failed');
  exit(1);
}
