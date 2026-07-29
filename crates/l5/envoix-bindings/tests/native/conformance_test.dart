// BN3b cross-language conformance suite (Dart side), run by
// `native_harnesses_replay_the_conformance_vectors` in
// tests/cross_language_conformance.rs.
//
// Two directions, one contract:
//   frontend -> backend  Dart hand-builds every body it may originate and must
//                        emit exactly the bytes the Rust reference codec emits
//                        for the same value. The Rust encoder is injective, so
//                        byte equality is proof the host decodes the value the
//                        frontend intended.
//   backend -> frontend  Rust-encoded command and read frames must decode to
//                        the intended Dart values. Acceptance and completion
//                        are host observations: this artifact has no encoder
//                        for them at all.
//
// The probe artifact covers the encoder surface the shipped contracts do not
// use (strings, lists, ascii, variable hex, u16/u32, optionals, the frame cap),
// so every emitted helper is compiled and exercised.

import 'dart:convert';
import 'dart:io';

import 'generated/envoix_command.dart';
import 'generated/envoix_probe.dart';
import 'generated/envoix_read.dart';

int _checks = 0;
final List<String> _failures = <String>[];
late Map<String, String> commandVectors;
late Set<String> originableVectors;
late Map<String, String> readVectors;

void expect(String label, bool ok) {
  _checks++;
  if (!ok) {
    _failures.add(label);
  }
}

void expectEq(String label, Object? actual, Object? expected) {
  _checks++;
  if (actual != expected) {
    _failures.add('$label: got <$actual>, want <$expected>');
  }
}

void expectThrows(
  String label,
  void Function() body,
  String kind,
  String context,
) {
  _checks++;
  try {
    body();
    _failures.add('$label: expected ($kind, $context), nothing was thrown');
  } catch (error) {
    if (!'$error'.contains('($kind, $context)')) {
      _failures.add('$label: expected ($kind, $context), got $error');
    }
  }
}

void expectEncodes(String label, void Function() body) {
  _checks++;
  try {
    body();
  } catch (error) {
    _failures.add('$label: unexpected $error');
  }
}

FrontendIntentView submit(String card, int epoch, String id, CommandView command) =>
    FrontendIntentViewCommand(
        SubmitView(card: card, epoch: epoch, commandId: id, command: command));

FrontendIntentView create(String requestId, CreateIntentView intent) =>
    FrontendIntentViewCreate(
        CreateView(intent: intent, requestId: requestId));

/// The Dart-side twin of the exported originable vectors. Building each body by
/// hand is the point: this is the code a frontend writes.
/// The command vocabulary by vector name: submitted, and named back by a
/// conflict.
const commandViews = <String, CommandView>{
  'pause': CommandView.pause,
  'cancel': CommandView.cancel,
  'resume': CommandView.resume,
  'remove': CommandView.remove,
  're_pick_source': CommandView.rePickSource,
};

Map<String, FrontendIntentView> handBuiltSubmits() {
  const card = '00000000000000ab';
  const id = '000102030405060708090a0b0c0d0e0f';
  final bodies = <String, FrontendIntentView>{};

  expectEq('every CommandView variant is swept', commandViews.length,
      CommandView.values.length);
  commandViews.forEach((name, command) {
    bodies['submit_$name'] = submit(card, 7, id, command);
  });

  const epochs = <String, int>{
    'zero': 0,
    'one': 1,
    'two_pow_53': 9007199254740992,
    'u63_max': 9223372036854775807,
  };
  epochs.forEach((name, epoch) {
    bodies['submit_epoch_$name'] = submit(card, epoch, id, CommandView.pause);
  });
  bodies['submit_ids_min'] =
      submit('0000000000000000', 1, '0' * 32, CommandView.cancel);
  bodies['submit_ids_max'] =
      submit('ffffffffffffffff', 1, 'f' * 32, CommandView.cancel);

  // The create half of the same originable body. A frontend states which side
  // of a room it will be on, or hands over opaque invite text it has not looked
  // at. Neither carries a document.
  bodies['create_mint_send'] = create(
      id, CreateIntentViewMintRoom(MintRoomView(localDirection: LocalDirectionView.send)));
  bodies['create_mint_receive'] = create(
      'f' * 32,
      CreateIntentViewMintRoom(
          MintRoomView(localDirection: LocalDirectionView.receive)));
  const invites = <String, String>{
    'empty': '',
    'canonical': 'envoix://invite/v3/eyJ2ZXJzaW9uIjozfQ',
    'bidirectional': '\u202eenvoix://invite',
  };
  invites.forEach((name, invite) {
    bodies['create_join_$name'] =
        create(id, CreateIntentViewJoinRoom(JoinInviteView(invite: CommandSecretString(invite))));
  });
  bodies['create_join_at_bound'] =
      create(id, CreateIntentViewJoinRoom(JoinInviteView(invite: CommandSecretString('e' * 16384))));
  return bodies;
}

/// frontend -> backend: Dart's bytes must be the reference bytes, for every
/// body a frontend may originate.
void frontendToBackend() {
  final bodies = handBuiltSubmits();
  final mine = bodies.keys.toList()..sort();
  final theirs = originableVectors.toList()..sort();
  expectEq('the Dart suite covers every originable command vector',
      mine.join(','), theirs.join(','));

  bodies.forEach((name, body) {
    final vector = commandVectors[name];
    if (vector == null) {
      _failures.add('$name: no exported vector');
      return;
    }
    expectEq(
        '$name encodes to the reference bytes', encodeCommandFrame(body), vector);
    // Stability: a body that made the round trip re-encodes identically, so a
    // frontend can forward what it decoded without drift.
    expectEq('$name re-encodes identically after a round trip',
        encodeCommandFrame(intentOf(name)), vector);
  });
}

FrontendIntentView intentOf(String name) {
  final body = decodeCommandFrame(commandVectors[name]!).body;
  if (body is! CommandBodyIntent) {
    throw StateError('$name is not a frontend intent');
  }
  return body.value;
}

SubmitView submitOf(String name) {
  final intent = intentOf(name);
  if (intent is! FrontendIntentViewCommand) {
    throw StateError('$name is not a submit');
  }
  return intent.value;
}

CreateView createOf(String name) {
  final intent = intentOf(name);
  if (intent is! FrontendIntentViewCreate) {
    throw StateError('$name is not a create');
  }
  return intent.value;
}

AcceptanceView acceptanceOf(String name) {
  final body = decodeCommandFrame(commandVectors[name]!).body;
  if (body is! CommandBodyAcceptance) {
    throw StateError('$name is not an acceptance');
  }
  return body.value.acceptance;
}

CompletionView completionOf(String name) {
  final body = decodeCommandFrame(commandVectors[name]!).body;
  if (body is! CommandBodyCompletion) {
    throw StateError('$name is not a completion');
  }
  return body.value.completion;
}

/// The vector-name suffix a decoded disposition must carry. Generated Dart
/// classes have no structural equality, so this is how the decode direction is
/// swept exhaustively rather than sampled.
String dispositionLabel(DispositionView value) => switch (value) {
      DispositionViewPreparing() => 'preparing',
      DispositionViewWaiting() => 'waiting',
      DispositionViewConnecting() => 'connecting',
      DispositionViewVerifying() => 'verifying',
      DispositionViewTransferring() => 'transferring',
      DispositionViewConfirming() => 'confirming',
      DispositionViewPaused(value: final state) =>
        'paused_${state.origin.name}',
      DispositionViewUnconfirmed() => 'unconfirmed',
      DispositionViewCompleted() => 'completed',
      DispositionViewFailed() => 'failed',
      DispositionViewCancelled() => 'cancelled',
    };

const rejectionLabels = <RejectionView, String>{
  RejectionView.unknownCard: 'unknown_card',
  RejectionView.staleEpoch: 'stale_epoch',
  RejectionView.superseded: 'superseded',
  RejectionView.atCapacity: 'at_capacity',
  RejectionView.runtimeStopped: 'runtime_stopped',
  RejectionView.interrupted: 'interrupted',
  RejectionView.internal: 'internal',
};

ReadFrame readFrame(String name) => decodeReadFrame(readVectors[name]!);

/// backend -> frontend: the typed values a frontend actually reads.
void backendToFrontend() {
  final pause = submitOf('submit_pause');
  expectEq('submit card', pause.card, '00000000000000ab');
  expectEq('submit epoch', pause.epoch, 7);
  expectEq('submit command_id', pause.commandId,
      '000102030405060708090a0b0c0d0e0f');
  expectEq('submit command', pause.command, CommandView.pause);

  // Integers above 2^53 survive intact on the Dart VM: this is the case that
  // silently corrupts in every JavaScript-family runtime.
  expectEq('epoch 2^53 survives', submitOf('submit_epoch_two_pow_53').epoch,
      9007199254740992);
  expectEq('epoch 2^63-1 survives', submitOf('submit_epoch_u63_max').epoch,
      9223372036854775807);
  expectEq('epoch zero survives', submitOf('submit_epoch_zero').epoch, 0);

  expect('accepted decodes to its unit variant',
      acceptanceOf('acceptance_accepted') is AcceptanceViewAccepted);
  expectEq('every RejectionView variant is swept', rejectionLabels.length,
      RejectionView.values.length);
  rejectionLabels.forEach((rejection, label) {
    final decoded = acceptanceOf('acceptance_rejected_$label');
    expect(
      'acceptance_rejected_$label decodes to its rejection',
      decoded is AcceptanceViewRejected && decoded.value == rejection,
    );
  });

  // A conflict names the command that owns the reused identity — the whole
  // point of the arm, so every command has to survive the crossing.
  commandViews.forEach((name, command) {
    final decoded = acceptanceOf('acceptance_conflict_$name');
    expect(
      'acceptance_conflict_$name names its applied command',
      decoded is AcceptanceViewConflict && decoded.value == command,
    );
  });

  // Every disposition shape, on both arms that carry one.
  var dispositions = 0;
  for (final name in commandVectors.keys) {
    if (name.startsWith('acceptance_duplicate_')) {
      final decoded = acceptanceOf(name);
      expect(
        '$name carries its disposition',
        decoded is AcceptanceViewDuplicate &&
            dispositionLabel(decoded.value) ==
                name.substring('acceptance_duplicate_'.length),
      );
      dispositions++;
    } else if (name.startsWith('completion_committed_')) {
      final decoded = completionOf(name);
      expect(
        '$name carries its disposition',
        decoded is CompletionViewCommitted &&
            dispositionLabel(decoded.value) ==
                name.substring('completion_committed_'.length),
      );
    }
  }
  expectEq('every disposition shape is swept', dispositions, 13);
  final failed = completionOf('completion_commit_failed_paused_lost');
  expect(
    'commit_failed carries its disposition',
    failed is CompletionViewCommitFailed &&
        dispositionLabel(failed.value) == 'paused_lost',
  );
  expect('interrupted decodes to its unit variant',
      completionOf('completion_interrupted') is CompletionViewInterrupted);
  expect('internal decodes to its unit variant',
      completionOf('completion_internal') is CompletionViewInternal);

  // Every exported command vector decodes, not just the sampled ones.
  for (final name in commandVectors.keys) {
    expectEncodes('$name decodes', () {
      decodeCommandFrame(commandVectors[name]!);
    });
  }

  // ---- read frames: decode-only, and the edges a decoder gets wrong ----
  final widest = readFrame('read_card_update_widest').body;
  if (widest is! ReadBodyCardUpdate) {
    _failures.add('read_card_update_widest is not a card update');
    return;
  }
  expectEq('read epoch at u63 max', widest.value.epoch, 9223372036854775807);
  final kind = widest.value.kind;
  if (kind is! CardUpdateKindViewSnapshot) {
    _failures.add('read_card_update_widest is not a snapshot');
    return;
  }
  final card = kind.value;
  expectEq('u32 max generation', card.generation, 4294967295);
  expectEq('total at u63 max', card.total, 9223372036854775807);
  expectEq('bytes at 2^53', card.bytes, 9007199254740992);
  expectEq('bytes_resumed above 2^53', card.bytesResumed, 9007199254740993);
  // Emoji with a skin-tone modifier, RTL text, a decomposed combining
  // mark, and a flag sequence, written escape by escape so the comparison
  // cannot pass through accidental normalization.
  expectEq('multi-byte name survives', card.offeredName,
      '\u{1F44D}\u{1F3FD} \u0645\u0631\u062D\u0628\u0627 e\u0301 \u{1F1FA}\u{1F1F3}.pdf');
  expect('the state union decodes', card.state is ProductStateViewPaused);
  expectEq(
    'the nested pause origin decodes',
    (card.state as ProductStateViewPaused).value.origin,
    PauseOriginView.lost,
  );
  expect('the quiescence union decodes',
      card.quiescence is QuiescenceViewRetiring);
  final outcome = card.outcome;
  if (outcome == null) {
    _failures.add('read_card_update_widest lost its outcome');
    return;
  }
  expectEq('text at exactly the 160-byte bound',
      utf8.encode(outcome.display).length, 160);
  expectEq('optional recovery present', outcome.recovery,
      RecoveryView.reconnectPeer);

  final narrowest = readFrame('read_card_update_narrowest').body;
  if (narrowest is! ReadBodyCardUpdate) {
    _failures.add('read_card_update_narrowest is not a card update');
    return;
  }
  final progress = narrowest.value.kind;
  if (progress is! CardUpdateKindViewProgress) {
    _failures.add('read_card_update_narrowest is not a progress update');
    return;
  }
  expectEq('an empty string decodes as empty', progress.value.offeredName, '');
  expectEq('epoch zero decodes', narrowest.value.epoch, 0);
  expectEq('an explicit null optional decodes as absent',
      progress.value.outcome, null);

  final empty = readFrame('read_evidence_empty').body;
  if (empty is! ReadBodyEvidence) {
    _failures.add('read_evidence_empty is not an evidence frame');
    return;
  }
  expectEq('an empty list decodes as empty', empty.value.entries.length, 0);
  expect('a complete diagnostics status decodes',
      empty.value.status is DiagnosticsStatusViewComplete);
  expectEq('u32 max session generation', empty.value.session.generation,
      4294967295);

  final entries = readFrame('read_evidence_entries').body;
  if (entries is! ReadBodyEvidence) {
    _failures.add('read_evidence_entries is not an evidence frame');
    return;
  }
  expectEq('list elements decode', entries.value.entries.length, 3);
  expectEq('a sequence above 2^53 survives in a list element',
      entries.value.entries[2].sequence, 9223372036854775807);
  expect('a degraded diagnostics status carries its payload',
      entries.value.status is DiagnosticsStatusViewDegraded);

  final manifest = readFrame('read_build_manifest').body;
  if (manifest is! ReadBodyBuildManifest) {
    _failures.add('read_build_manifest is not a manifest frame');
    return;
  }
  expectEq('u16 max wire version',
      manifest.value.protocol.dataWireVersion, 65535);
  expectEq('variable-length hex decodes', manifest.value.protocol.dataMagic,
      'cafebabe');

  for (final name in readVectors.keys) {
    expectEncodes('$name decodes', () {
      decodeReadFrame(readVectors[name]!);
    });
  }
}

/// The encoder enforces every bound its decoder checks, so an over-bound value
/// never becomes bytes.
void encoderHonesty() {
  const id = '000102030405060708090a0b0c0d0e0f';
  const card = '00000000000000ab';

  expectThrows('a negative epoch is rejected',
      () => encodeCommandFrame(submit(card, -1, id, CommandView.pause)),
      'range', 'SubmitView.epoch');
  // 2^63 is not a Dart int at all: the bound above u63 is unrepresentable
  // rather than merely checked.
  _checks++;
  try {
    int.parse('9223372036854775808');
    _failures.add('2^63 parsed as a Dart int');
  } on FormatException {
    // expected
  }

  const badCards = <String, String>{
    'uppercase': '00000000000000AB',
    'too short': '00000000000000a',
    'too long': '00000000000000abc',
    'empty': '',
    'non-hex': '00000000000000ag',
    'unpaired surrogate': '00000000000000\uD800',
  };
  badCards.forEach((label, value) {
    expectThrows('a card that is $label is rejected',
        () => encodeCommandFrame(submit(value, 1, id, CommandView.pause)),
        'bound', 'SubmitView.card');
  });
  const badIds = <String, String>{
    'too short': '000102030405060708090a0b0c0d0e0',
    'too long': '000102030405060708090a0b0c0d0e0ff',
    'uppercase': '000102030405060708090A0B0C0D0E0F',
    'empty': '',
  };
  badIds.forEach((label, value) {
    expectThrows('a command id that is $label is rejected',
        () => encodeCommandFrame(submit(card, 1, value, CommandView.pause)),
        'bound', 'SubmitView.command_id');
  });

  // A recorded platform divergence, not a contract claim: `jsonDecode`
  // resolves duplicate object keys last-wins where the Rust reference codec
  // rejects the frame outright. It is not a smuggling path in this direction
  // (frames reaching a frontend come from the trusted host, and no encoder can
  // emit a duplicate key); the hostile direction, a frontend submitting to
  // Rust, rejects duplicates.
  final duplicated = commandVectors['submit_pause']!
      .replaceFirst('"command":"pause"', '"command":"pause","command":"cancel"');
  final smuggled = decodeCommandFrame(duplicated).body;
  expect(
    'duplicate keys resolve last-wins on the Dart decode side',
    smuggled is CommandBodyIntent &&
        smuggled.value is FrontendIntentViewCommand &&
        (smuggled.value as FrontendIntentViewCommand).value.command ==
            CommandView.cancel,
  );

  // The envelope and the body arm are stamped by the codec: a frame cannot
  // claim another contract or another arm, because the encoder takes the arm's
  // payload and nothing else.
  final stamped =
      jsonDecode(encodeCommandFrame(submit(card, 1, id, CommandView.pause)))
          as Map<String, Object?>;
  expectEq('the encoder stamps the schema envelope', stamped['schema'],
      commandSchemaId);
  expectEq('the encoder stamps the intent arm',
      (stamped['body']! as Map<String, Object?>)['kind'], 'intent');
}

/// 2-, 3-, and 4-byte characters: 9 bytes, 4 UTF-16 units per group.
const wideGroup = 'é中\u{1F600}';

ProbeScalars probeScalars({
  int small = 0,
  int medium = 0,
  int large = 0,
  String shortId = '0000000000000000',
  String longId = '00000000000000000000000000000000',
  String digest =
      '0000000000000000000000000000000000000000000000000000000000000000',
  String blobby = 'ab',
  String text = '',
  String label = '',
  ProbeTone? maybe,
  String? maybeText,
  List<ProbeLeaf> leaves = const <ProbeLeaf>[],
  ProbeChoice choice = const ProbeChoiceNothing(),
}) =>
    ProbeScalars(
      small: small,
      medium: medium,
      large: large,
      shortId: shortId,
      longId: longId,
      digest: digest,
      blobby: blobby,
      text: text,
      label: label,
      maybe: maybe,
      maybeText: maybeText,
      leaves: leaves,
      choice: choice,
    );

ProbeScalars decodedScalars(String text) {
  final body = decodeProbeFrame(text).body;
  if (body is! ProbeBodyScalars) {
    throw StateError('the probe frame is not a scalars body');
  }
  return body.value;
}

/// The encoder surface the two shipped contracts do not exercise: strings,
/// ascii, variable hex, u16/u32, lists, optionals, and the frame cap.
void probeSurface() {
  expectEncodes(
      'a minimal probe frame encodes', () => encodeProbeFrame(probeScalars()));

  // Every field is in bounds and the frame is still over the cap: it fails
  // typed instead of leaving the process oversized. The text fields are
  // multi-byte, so the frame clears the cap in UTF-16 units (528) and breaks it
  // in UTF-8 bytes (578) — counting the wrong unit accepts this frame.
  final over = probeScalars(
    small: 65535,
    medium: 4294967295,
    large: 9223372036854775807,
    shortId: 'f' * 16,
    longId: 'f' * 32,
    digest: 'f' * 64,
    blobby: 'aabbccdd',
    text: wideGroup * 5,
    label: 'y' * 8,
    maybe: ProbeTone.loud,
    maybeText: wideGroup * 5,
    leaves: const <ProbeLeaf>[
      ProbeLeaf(tone: ProbeTone.calm),
      ProbeLeaf(tone: ProbeTone.loud),
      ProbeLeaf(tone: ProbeTone.calm),
      ProbeLeaf(tone: ProbeTone.loud),
    ],
    choice: const ProbeChoiceLeaf(ProbeLeaf(tone: ProbeTone.loud)),
  );
  expectThrows('an over-cap multi-byte frame is rejected',
      () => encodeProbeFrame(over), 'frameTooLarge', 'ProbeFrame');

  // Round trip and stability on a frame that uses every construct.
  final rich = probeScalars(
    small: 65535,
    medium: 4294967295,
    large: 9223372036854775807,
    blobby: 'aabbccdd',
    text: '\u{1F600}\u{1F600}\u{1F600}\u{1F600}',
    label: 'y' * 8,
    maybe: ProbeTone.loud,
    leaves: const <ProbeLeaf>[ProbeLeaf(tone: ProbeTone.calm)],
    choice: const ProbeChoiceLeaf(ProbeLeaf(tone: ProbeTone.loud)),
  );
  final encoded = encodeProbeFrame(rich);
  expectEq('a rich probe frame round-trips',
      encodeProbeFrame(decodedScalars(encoded)), encoded);
  final decoded = decodedScalars(encoded);
  expectEq('16 bytes of emoji survive', decoded.text,
      '\u{1F600}\u{1F600}\u{1F600}\u{1F600}');
  expectEq('an optional enum survives', decoded.maybe, ProbeTone.loud);
  expectEq('an absent optional stays absent', decoded.maybeText, null);
  expectEq('a list survives', decoded.leaves.length, 1);
  expect('a payload union arm survives', decoded.choice is ProbeChoiceLeaf);
  expect(
      'a unit union arm survives',
      decodedScalars(encodeProbeFrame(probeScalars())).choice
          is ProbeChoiceNothing);

  // Numeric ranges.
  expectEncodes(
      'u16 max encodes', () => encodeProbeFrame(probeScalars(small: 65535)));
  expectThrows('u16 overflow is rejected',
      () => encodeProbeFrame(probeScalars(small: 65536)), 'range',
      'ProbeScalars.small');
  expectEncodes('u32 max encodes',
      () => encodeProbeFrame(probeScalars(medium: 4294967295)));
  expectThrows('u32 overflow is rejected',
      () => encodeProbeFrame(probeScalars(medium: 4294967296)), 'range',
      'ProbeScalars.medium');
  expectThrows('a negative u16 is rejected',
      () => encodeProbeFrame(probeScalars(small: -1)), 'range',
      'ProbeScalars.small');
  expectThrows('a negative u63 is rejected',
      () => encodeProbeFrame(probeScalars(large: -1)), 'range',
      'ProbeScalars.large');

  // Fixed-length hex.
  expectThrows('an uppercase digest is rejected',
      () => encodeProbeFrame(probeScalars(digest: 'A' * 64)), 'bound',
      'ProbeScalars.digest');
  expectThrows('a short digest is rejected',
      () => encodeProbeFrame(probeScalars(digest: 'a' * 63)), 'bound',
      'ProbeScalars.digest');

  // Variable-length hex.
  expectEncodes('minimal variable hex encodes',
      () => encodeProbeFrame(probeScalars(blobby: 'ab')));
  expectThrows('empty variable hex is rejected',
      () => encodeProbeFrame(probeScalars(blobby: '')), 'bound',
      'ProbeScalars.blobby');
  expectThrows('odd-length variable hex is rejected',
      () => encodeProbeFrame(probeScalars(blobby: 'abc')), 'bound',
      'ProbeScalars.blobby');
  expectThrows('over-long variable hex is rejected',
      () => encodeProbeFrame(probeScalars(blobby: 'aabbccddee')), 'bound',
      'ProbeScalars.blobby');
  expectThrows('uppercase variable hex is rejected',
      () => encodeProbeFrame(probeScalars(blobby: 'AABB')), 'bound',
      'ProbeScalars.blobby');
  expectThrows('non-hex variable hex is rejected',
      () => encodeProbeFrame(probeScalars(blobby: 'zzzz')), 'bound',
      'ProbeScalars.blobby');

  // UTF-8 text: the bound is bytes, not code units.
  expectEncodes(
      'an empty string encodes', () => encodeProbeFrame(probeScalars(text: '')));
  expectEncodes('45 ascii bytes at the bound encode',
      () => encodeProbeFrame(probeScalars(text: 'x' * 45)));
  expectEncodes('45 multi-byte bytes at the bound encode',
      () => encodeProbeFrame(probeScalars(text: wideGroup * 5)));
  expectThrows('one byte over the bound is rejected',
      () => encodeProbeFrame(probeScalars(text: 'x' * 46)), 'bound',
      'ProbeScalars.text');
  expectThrows('one multi-byte character over the bound is rejected',
      () => encodeProbeFrame(probeScalars(text: '${wideGroup * 5}x')), 'bound',
      'ProbeScalars.text');
  expectEncodes('right-to-left text encodes',
      () => encodeProbeFrame(probeScalars(text: 'مرحبا')));
  expectEncodes('a combining mark encodes',
      () => encodeProbeFrame(probeScalars(text: 'é')));
  expectThrows('a lone high surrogate is rejected',
      () => encodeProbeFrame(probeScalars(text: 'a\uD800')), 'shape',
      'ProbeScalars.text');
  expectThrows('a lone low surrogate is rejected',
      () => encodeProbeFrame(probeScalars(text: 'a\uDC00')), 'shape',
      'ProbeScalars.text');
  expectThrows('a reversed surrogate pair is rejected',
      () => encodeProbeFrame(probeScalars(text: '\uDC00\uD800')), 'shape',
      'ProbeScalars.text');
  expectThrows('a lone surrogate in an optional string is rejected',
      () => encodeProbeFrame(probeScalars(maybeText: '\uD800')), 'shape',
      'ProbeScalars.maybe_text');

  // Printable ASCII.
  expectEncodes(
      'empty ascii encodes', () => encodeProbeFrame(probeScalars(label: '')));
  expectEncodes('ascii at the bound encodes',
      () => encodeProbeFrame(probeScalars(label: 'y' * 8)));
  expectThrows('over-long ascii is rejected',
      () => encodeProbeFrame(probeScalars(label: 'y' * 9)), 'bound',
      'ProbeScalars.label');
  expectThrows('a control character is rejected',
      () => encodeProbeFrame(probeScalars(label: 'a\u001f')), 'bound',
      'ProbeScalars.label');
  expectThrows('delete is rejected',
      () => encodeProbeFrame(probeScalars(label: 'a\u007f')), 'bound',
      'ProbeScalars.label');
  expectThrows('non-ascii text in an ascii field is rejected',
      () => encodeProbeFrame(probeScalars(label: 'é')), 'bound',
      'ProbeScalars.label');

  // List caps.
  expectEncodes(
    'a list at its cap encodes',
    () => encodeProbeFrame(probeScalars(leaves: const <ProbeLeaf>[
      ProbeLeaf(tone: ProbeTone.calm),
      ProbeLeaf(tone: ProbeTone.calm),
      ProbeLeaf(tone: ProbeTone.calm),
      ProbeLeaf(tone: ProbeTone.calm),
    ])),
  );
  expectThrows(
    'one element over the cap is rejected',
    () => encodeProbeFrame(probeScalars(leaves: const <ProbeLeaf>[
      ProbeLeaf(tone: ProbeTone.calm),
      ProbeLeaf(tone: ProbeTone.calm),
      ProbeLeaf(tone: ProbeTone.calm),
      ProbeLeaf(tone: ProbeTone.calm),
      ProbeLeaf(tone: ProbeTone.calm),
    ])),
    'bound',
    'ProbeScalars.leaves',
  );

  // The cap is enforced on the way in on the same unit: the bytes the encoder
  // refused above are refused again when they arrive, though they are under the
  // cap in UTF-16 units.
  expectThrows('an over-cap multi-byte frame is rejected on decode', () {
    final map =
        jsonDecode(encodeProbeFrame(probeScalars())) as Map<String, Object?>;
    final scalars =
        (map['body']! as Map<String, Object?>)['value']! as Map<String, Object?>;
    scalars['small'] = 65535;
    scalars['medium'] = 4294967295;
    scalars['large'] = 9223372036854775807;
    scalars['short_id'] = 'f' * 16;
    scalars['long_id'] = 'f' * 32;
    scalars['digest'] = 'f' * 64;
    scalars['blobby'] = 'aabbccdd';
    scalars['text'] = wideGroup * 5;
    scalars['label'] = 'y' * 8;
    scalars['maybe'] = 'loud';
    scalars['maybe_text'] = wideGroup * 5;
    scalars['leaves'] = <Object?>[
      <String, Object?>{'tone': 'calm'},
      <String, Object?>{'tone': 'loud'},
      <String, Object?>{'tone': 'calm'},
      <String, Object?>{'tone': 'loud'},
    ];
    scalars['choice'] = <String, Object?>{
      'kind': 'leaf',
      'value': <String, Object?>{'tone': 'loud'},
    };
    final text = jsonEncode(map);
    expect('the over-cap frame is under the cap in UTF-16 units',
        text.length <= probeMaxFrameBytes &&
            utf8.encode(text).length > probeMaxFrameBytes);
    decodeProbeFrame(text);
  }, 'frameTooLarge', 'ProbeFrame');

  // The decoder still rejects what it always rejected, on both directions of
  // the same contract.
  expectThrows('an unknown field is rejected on decode', () {
    final map =
        jsonDecode(encodeProbeFrame(probeScalars())) as Map<String, Object?>;
    ((map['body']! as Map<String, Object?>)['value']! as Map<String, Object?>)
        ['smuggled'] = 1;
    decodeProbeFrame(jsonEncode(map));
  }, 'unknownField', 'ProbeBody.scalars');
  expectThrows('an unknown union arm is rejected on decode', () {
    final map =
        jsonDecode(encodeProbeFrame(probeScalars())) as Map<String, Object?>;
    (map['body']! as Map<String, Object?>)['kind'] = 'shell';
    decodeProbeFrame(jsonEncode(map));
  }, 'unknownVariant', 'ProbeFrame.body');
  expectThrows('a wrong schema envelope is rejected on decode', () {
    final map =
        jsonDecode(encodeProbeFrame(probeScalars())) as Map<String, Object?>;
    map['schema'] = 'envoix/binding/probe/2';
    decodeProbeFrame(jsonEncode(map));
  }, 'unknownSchema', 'ProbeFrame');
  // Absent is not the same as explicitly null: an optional key must be present
  // and may be null, so a frame that simply omits it is malformed rather than
  // silently defaulted.
  expectThrows('an absent optional key is rejected on decode', () {
    final map =
        jsonDecode(encodeProbeFrame(probeScalars())) as Map<String, Object?>;
    ((map['body']! as Map<String, Object?>)['value']! as Map<String, Object?>)
        .remove('maybe');
    decodeProbeFrame(jsonEncode(map));
  }, 'shape', 'ProbeScalars.maybe');
  // The encoder cannot produce 2^63 (it is not a Dart int), but a decoder can
  // be handed one: the JSON parser widens it to a double, which is not an int.
  expectThrows('a raw integer above u63 is rejected on decode', () {
    decodeProbeFrame(encodeProbeFrame(probeScalars())
        .replaceFirst('"large":0', '"large":9223372036854775808'));
  }, 'shape', 'ProbeScalars.large');
}

/// The direction policy as a consumer sees it: the read artifact has no encoder
/// to call at all, and the command artifact encodes only the arm a frontend may
/// originate.
void directionPolicy(String artifacts) {
  final read = File('$artifacts/envoix_read.dart').readAsStringSync();
  final command = File('$artifacts/envoix_command.dart').readAsStringSync();
  expect('the read artifact exposes no encoder of any kind',
      !read.contains('_encode') && !read.contains('encodeReadFrame('));
  expect('the read artifact still exposes its decoder',
      read.contains('ReadFrame decodeReadFrame(String text)'));
  expect('the command artifact encodes only the originable body',
      command.contains('String encodeCommandFrame(FrontendIntentView body)'));
  expect('the command artifact still decodes every body',
      command.contains('CommandFrame decodeCommandFrame(String text)'));
  for (final observation in <String>[
    'CommandAcceptanceView',
    'CommandCompletionView',
    'CreateResultView',
    'CreateOutcomeView',
    'CreateRefusalView',
    'CardCreatedView',
    'AcceptanceView',
    'CompletionView',
    'DispositionView',
    'RejectionView',
    'PausedStateView',
    'PauseCauseView',
    'CommandBody',
    'CommandFrame',
  ]) {
    expect('a frontend cannot encode a $observation',
        !command.contains('_encode$observation('));
  }
}

void main(List<String> args) {
  final bundle =
      jsonDecode(File(args[0]).readAsStringSync()) as Map<String, Object?>;
  final command = bundle['command']! as List<Object?>;
  commandVectors = <String, String>{
    for (final entry in command)
      (entry! as Map<String, Object?>)['name']! as String:
          (entry as Map<String, Object?>)['frame']! as String,
  };
  originableVectors = <String>{
    for (final entry in command)
      if ((entry! as Map<String, Object?>)['originable']! as bool)
        (entry as Map<String, Object?>)['name']! as String,
  };
  readVectors = <String, String>{
    for (final entry in bundle['read']! as List<Object?>)
      (entry! as Map<String, Object?>)['name']! as String:
          (entry as Map<String, Object?>)['frame']! as String,
  };

  frontendToBackend();
  backendToFrontend();
  encoderHonesty();
  probeSurface();
  directionPolicy(args[1]);

  print('command vectors: ${commandVectors.length}');
  print('originable vectors: ${originableVectors.length}');
  print('read vectors: ${readVectors.length}');
  print('checks: $_checks');
  if (_failures.isEmpty) {
    print('RESULT: all checks passed');
    return;
  }
  for (final failure in _failures) {
    print('FAIL: $failure');
  }
  print('RESULT: ${_failures.length} failed');
  exit(1);
}
