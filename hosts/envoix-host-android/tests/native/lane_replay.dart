// The Dart half of `flutter_attaches_and_decodes_live_frames`.
//
// It runs the APP's own view model — not a copy of it — over frames a real
// running host produced, and reports what the frontend surfaced. A plain
// script rather than a package test, so `cargo test` can execute it with no
// pub resolution and no Flutter engine: `attachment.dart` deliberately imports
// nothing but `dart:convert` and the generated binding.
//
// Usage: dart run lane_replay.dart <directory> <card> <offeredName> <total>

import 'dart:convert';
import 'dart:io';

import '../../../../apps/envoix-flutter/lib/attachment.dart';
import '../../../../apps/envoix-flutter/lib/bindings/envoix_read.dart';

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
  final Directory work = Directory(arguments[0]);
  final String card = arguments[1];
  final String offeredName = arguments[2];
  final int total = int.parse(arguments[3]);

  List<int> frame(String name) =>
      utf8.encode(File('${work.path}/$name').readAsStringSync());
  final List<List<int>> live = File('${work.path}/live.frames')
      .readAsLinesSync()
      .map(utf8.encode)
      .toList();

  // 1. A live attachment surfaces the card the host actually holds.
  final Attachment attached = Attachment();
  for (final List<int> bytes in live) {
    attached.ingest(bytes);
  }
  equal('cards surfaced', attached.cards.length, 1);
  final CardRow row = attached.cards.single;
  equal('card id', row.card, card);
  equal('offered name', row.view?.offeredName, offeredName);
  equal('total bytes', row.view?.total, total);
  equal('stream status', row.status, StreamStatus.live);
  check('epoch is the host\'s, not zero', row.epoch > 0);
  for (final FrameRejection kind in FrameRejection.values) {
    equal('live frames rejected as ${kind.name}', attached.rejected(kind), 0);
  }

  // 2. A refused attach is a first-class outcome, carrying the typed reason.
  equal('refusals surfaced', attached.refusals.length, 1);
  equal(
    'refusal reason',
    attached.refusals.values.single,
    SubscribeRejectionView.unknownCard,
  );

  // 2b. The diagnostics the host recorded before this attachment existed reach
  //     it as a session timeline — the content of the logs screen, decoded from
  //     what a real host actually emitted.
  equal('timelines surfaced', attached.timelines.length, 1);
  final EvidenceTimelineView evidence = attached.timelines.single;
  equal('timeline session', evidence.session.card, card);
  check('the timeline has entries', evidence.entries.isNotEmpty);
  check(
    'nothing was dropped, so the timeline is complete',
    evidence.status is DiagnosticsStatusViewComplete,
  );

  // 3. An update at this attachment's epoch is admitted; one from another
  //    epoch is dropped as stale and changes nothing.
  attached.ingest(frame('progress.frame'));
  equal('a live update is admitted', attached.rejected(FrameRejection.staleEpoch), 0);
  attached.ingest(frame('stale.frame'));
  equal('a superseded epoch is dropped', attached.rejected(FrameRejection.staleEpoch), 1);
  equal('the stale frame changed no epoch', attached.cards.single.epoch, row.epoch);

  // 4. A lag ends the epoch. It is a state the UI shows, not an error, and
  //    everything after it is stale until a fresh attachment.
  attached.ingest(frame('lag.frame'));
  equal('lag surfaced', attached.cards.single.status, StreamStatus.lagged);
  equal('lag names what it dropped', attached.cards.single.missed,
      LosslessKindView.terminal);
  attached.ingest(frame('progress.frame'));
  equal('the dead epoch admits nothing more',
      attached.rejected(FrameRejection.staleEpoch), 2);

  // 5. A close ends the epoch the same way, on its own attachment.
  final Attachment closing = Attachment();
  for (final List<int> bytes in live) {
    closing.ingest(bytes);
  }
  closing.ingest(frame('closed.frame'));
  equal('close surfaced', closing.cards.single.status, StreamStatus.closed);
  equal('close names nothing dropped', closing.cards.single.missed, null);

  // 6. A card whose epoch this attachment never opened: the stream contract
  //    says an epoch starts with its snapshot, so a bare update is not ours.
  final Attachment cold = Attachment();
  cold.ingest(frame('progress.frame'));
  equal('an unopened epoch delivers nothing', cold.cards.length, 0);
  equal('and is counted', cold.rejected(FrameRejection.staleEpoch), 1);

  // 7. A second snapshot inside one epoch breaks the stream's own contract.
  final Attachment doubled = Attachment();
  final List<int> snapshot = live.firstWhere(
    (List<int> bytes) => decodeReadFrame(utf8.decode(bytes)).body
        is ReadBodyCardUpdate,
  );
  doubled.ingest(snapshot);
  doubled.ingest(snapshot);
  equal('a repeated snapshot is a contract breach',
      doubled.rejected(FrameRejection.contractBreach), 1);

  // 8. Hostile bytes are a counted outcome, never an exception into the UI.
  final Attachment hostile = Attachment();
  for (final String bad in <String>[
    '',
    'not json',
    '{"schema":"envoix/binding/read/1","body":{"kind":"lag"}}',
    '{"schema":"envoix/binding/read/2","body":{"kind":"nope"}}',
  ]) {
    hostile.ingest(utf8.encode(bad));
  }
  hostile.ingest(<int>[0xff, 0xfe, 0xfd]);
  equal('every undecodable frame is counted',
      hostile.rejected(FrameRejection.undecodable), 5);
  equal('and none of them reached the view', hostile.cards.length, 0);

  if (_failures.isEmpty) {
    print('lane replay: $_checks checks pass');
    return;
  }
  for (final String failure in _failures) {
    stderr.writeln('FAIL $failure');
  }
  stderr.writeln('${_failures.length} of $_checks checks failed');
  exit(1);
}
