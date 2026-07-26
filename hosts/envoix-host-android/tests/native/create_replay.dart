// The Dart half of `flutter_creates_a_transfer_without_the_debug_bridge`.
//
// It runs the APP's own create encoder and the APP's own view model against a
// real running host. The frames the host is fed here are the frames a tap in
// the new-transfer sheet produces; the answers rendered here are the answers
// that host actually gave.
//
// A plain script rather than a package test, so `cargo test` can execute it
// with no pub resolution and no Flutter engine. `Creator` lives in `lane.dart`,
// which imports Flutter, so this drives its encoder directly — the widget half
// of the same path is `flutter test`'s.
//
// Usage: dart run create_replay.dart ask-send <directory>
//        dart run create_replay.dart ask-join <directory>
//        dart run create_replay.dart render   <directory>

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
  check('$what: expected <$expected>, found <$found>', found == expected);
}

/// The answer the host gave to one request, bound to the intent that made it.
CreateIntent answered(String id, CreateKind kind, List<int> reply) {
  final CreateIntent request = CreateIntent(id: id, kind: kind);
  final CommandFrame frame = decodeCommandFrame(utf8.decode(reply));
  if (frame.body case final CommandBodyCreateResult body
      when body.value.requestId == id) {
    request.outcome = body.value.outcome;
  } else {
    request.fault = const IntentFault(
      FaultOrigin.unanswered,
      'the host answered a request nobody made',
    );
  }
  return request;
}

void main(List<String> arguments) {
  final String mode = arguments[0];
  final Directory work = Directory(arguments[1]);

  String text(String name) => File('${work.path}/$name').readAsStringSync();
  List<int> frame(String name) => utf8.encode(text(name));
  void write(String name, List<int> bytes) =>
      File('${work.path}/$name').writeAsBytesSync(bytes);
  void writeText(String name, String value) =>
      File('${work.path}/$name').writeAsStringSync(value);

  if (mode == 'ask-send') {
    // The frontend knows only what the platform told it about the document:
    // a name and a size. There is no URI in this process to spell.
    final String id = mintCommandId();
    check('the identity is a hex32', RegExp(r'^[0-9a-f]{32}$').hasMatch(id));
    write(
      'send.frame',
      createFrame(
        id: id,
        intent: const CreateIntentViewSend(
          SendSourceView(displayName: 'quarterly report.pdf', total: 4096),
        ),
      ),
    );
    writeText('send.id', id);
    report();
    return;
  }

  if (mode == 'ask-join') {
    // The invite text is whatever it is. This driver reads it from a file the
    // host wrote and passes it through — it does not look at it, and the
    // deliberately-invalid one below is passed through just as untouched.
    final String id = mintCommandId();
    write(
      'join.frame',
      createFrame(
        id: id,
        intent: CreateIntentViewJoin(JoinInviteView(invite: text('invite.txt'))),
      ),
    );
    writeText('join.id', id);

    final String bare = mintCommandId();
    write(
      'bare.frame',
      createFrame(
        id: bare,
        // Six digits and two words: exactly the shape the old app called
        // "ready" with `contains("-")`. This app has no opinion about it.
        intent: const CreateIntentViewJoin(
          JoinInviteView(invite: '000123-amber-brass'),
        ),
      ),
    );
    writeText('bare.id', bare);
    report();
    return;
  }

  // The answers the running host gave, rendered by the app's own words.
  final CreateIntent send =
      answered(text('send.id'), CreateKind.send, frame('send.result'));
  check('the send was created', send.card != null);
  equal(
    'and the app says so in the authority\'s terms',
    createAnswerLabel(send),
    'Created — transfer ${send.card} exists.',
  );

  final CreateIntent join =
      answered(text('join.id'), CreateKind.join, frame('join.result'));
  check('the join was created', join.card != null);
  check('and it is a different card', join.card != send.card);

  final CreateIntent bare =
      answered(text('bare.id'), CreateKind.join, frame('bare.result'));
  equal(
    'a bare room code is refused BY THE AUTHORITY, in its own words',
    createAnswerLabel(bare),
    'Refused — That is only the room code. Paste the whole invite.',
  );
  equal('and no card was made', bare.card, null);

  // The cards themselves, as the lane delivered them.
  final Attachment attachment = Attachment();
  for (final List<int> bytes
      in File('${work.path}/cards.frames').readAsLinesSync().map(utf8.encode)) {
    attachment.ingest(bytes);
  }
  equal('both cards are on the lane', attachment.cards.length, 2);
  for (final FrameRejection kind in FrameRejection.values) {
    equal('frames rejected as ${kind.name}', attachment.rejected(kind), 0);
  }

  final CardRow sender = attachment.cards
      .firstWhere((CardRow row) => row.card == send.card);
  final CardView sending = sender.view!;
  equal('the send is a send', directionLabel(sending.direction), 'Sending');
  equal('preparing its source', stateLabel(sending.state), 'Preparing');
  equal('with the name the platform reported', sending.offeredName,
      'quarterly report.pdf');
  final InviteView? published = sending.invite;
  check('the send publishes an invite to share', published != null);
  check(
    'whose room code the authority generated',
    RegExp(r'^\d{6}-[a-z]+-[a-z]+$').hasMatch(published!.code),
  );
  check('and whose link the frontend can copy', published.link != null);

  final CardRow joined =
      attachment.cards.firstWhere((CardRow row) => row.card == join.card);
  final CardView joining = joined.view!;
  // The invite declared its creator's role, and the AUTHORITY took the other
  // one. Nothing in this app chose a direction.
  equal('the joiner receives', directionLabel(joining.direction), 'Receiving');
  equal(
    'on the same room code the sender published',
    joining.invite?.code,
    published.code,
  );
  report();
}

void report() {
  if (_failures.isEmpty) {
    print('create replay: $_checks checks pass');
    return;
  }
  for (final String failure in _failures) {
    stderr.writeln('FAIL $failure');
  }
  stderr.writeln('${_failures.length} of $_checks checks failed');
  exit(1);
}
