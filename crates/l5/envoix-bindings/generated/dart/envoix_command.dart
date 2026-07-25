// @generated from schema/command.schema by envoix-bindings. Do not edit;
// regenerate with `ENVOIX_BINDINGS_REGEN=1 cargo test -p envoix-bindings generated_artifacts`.
// Known platform caveat: JSON `-0` decodes as integer 0 here while the Rust
// reference codec rejects it (benign: every field with a positive minimum
// still fails its range check).
// Encoded frames are byte-identical to the Rust reference codec's: object
// keys are emitted in the sorted order serde_json serializes and `jsonEncode`
// keeps insertion order. On the JavaScript backend `int` is a double, so
// values above 2^53 lose precision before the encoder ever sees them; this
// contract is for the Dart VM (Flutter mobile/desktop).

import 'dart:convert';

const String commandSchemaId = 'envoix/binding/command/1';
const int commandMaxFrameBytes = 1048576;
// Contract rules frozen by schema/command.schema.
const bool newestAttachmentCommands = true;
const int retryHorizonCompletions = 256;
const bool supersessionInertPreAcceptanceOnly = true;
const int _u63Max = 9223372036854775807;

enum CommandErrorKind {
  frameTooLarge,
  malformedJson,
  unknownSchema,
  shape,
  unknownField,
  unknownVariant,
  range,
  bound,
}

/// Typed codec failure carrying only static schema context.
final class CommandContractException implements Exception {
  const CommandContractException(this.kind, this.context);

  final CommandErrorKind kind;
  final String context;

  @override
  String toString() => 'CommandContractException(${kind.name}, $context)';
}

enum CommandView {
  pause,
  cancel,
  resume,
  remove,
  rePickSource,
}

enum PauseCauseView {
  local,
  peer,
  lost,
}

final class PausedStateView {
  const PausedStateView({
    required this.origin,
  });

  final PauseCauseView origin;
}

sealed class DispositionView {
  const DispositionView();
}

final class DispositionViewPreparing extends DispositionView {
  const DispositionViewPreparing();
}

final class DispositionViewWaiting extends DispositionView {
  const DispositionViewWaiting();
}

final class DispositionViewConnecting extends DispositionView {
  const DispositionViewConnecting();
}

final class DispositionViewVerifying extends DispositionView {
  const DispositionViewVerifying();
}

final class DispositionViewTransferring extends DispositionView {
  const DispositionViewTransferring();
}

final class DispositionViewConfirming extends DispositionView {
  const DispositionViewConfirming();
}

final class DispositionViewPaused extends DispositionView {
  const DispositionViewPaused(this.value);

  final PausedStateView value;
}

final class DispositionViewUnconfirmed extends DispositionView {
  const DispositionViewUnconfirmed();
}

final class DispositionViewCompleted extends DispositionView {
  const DispositionViewCompleted();
}

final class DispositionViewFailed extends DispositionView {
  const DispositionViewFailed();
}

final class DispositionViewCancelled extends DispositionView {
  const DispositionViewCancelled();
}

final class SubmitView {
  const SubmitView({
    required this.card,
    required this.epoch,
    required this.commandId,
    required this.command,
  });

  final String card;
  final int epoch;
  final String commandId;
  final CommandView command;
}

enum RejectionView {
  unknownCard,
  staleEpoch,
  superseded,
  atCapacity,
  runtimeStopped,
  interrupted,
  conflict,
  internal,
}

sealed class AcceptanceView {
  const AcceptanceView();
}

final class AcceptanceViewAccepted extends AcceptanceView {
  const AcceptanceViewAccepted();
}

final class AcceptanceViewDuplicate extends AcceptanceView {
  const AcceptanceViewDuplicate(this.value);

  final DispositionView value;
}

final class AcceptanceViewRejected extends AcceptanceView {
  const AcceptanceViewRejected(this.value);

  final RejectionView value;
}

final class CommandAcceptanceView {
  const CommandAcceptanceView({
    required this.commandId,
    required this.acceptance,
  });

  final String commandId;
  final AcceptanceView acceptance;
}

sealed class CompletionView {
  const CompletionView();
}

final class CompletionViewCommitted extends CompletionView {
  const CompletionViewCommitted(this.value);

  final DispositionView value;
}

final class CompletionViewCommitFailed extends CompletionView {
  const CompletionViewCommitFailed(this.value);

  final DispositionView value;
}

final class CompletionViewInterrupted extends CompletionView {
  const CompletionViewInterrupted();
}

final class CompletionViewInternal extends CompletionView {
  const CompletionViewInternal();
}

final class CommandCompletionView {
  const CommandCompletionView({
    required this.commandId,
    required this.completion,
  });

  final String commandId;
  final CompletionView completion;
}

sealed class CommandBody {
  const CommandBody();
}

final class CommandBodySubmit extends CommandBody {
  const CommandBodySubmit(this.value);

  final SubmitView value;
}

final class CommandBodyAcceptance extends CommandBody {
  const CommandBodyAcceptance(this.value);

  final CommandAcceptanceView value;
}

final class CommandBodyCompletion extends CommandBody {
  const CommandBodyCompletion(this.value);

  final CommandCompletionView value;
}

final class CommandFrame {
  const CommandFrame({
    required this.body,
  });

  final CommandBody body;
}

/// Decodes and validates one frame. Every failure is a typed
/// [CommandContractException]; no input, however hostile, misparses.
CommandFrame decodeCommandFrame(String text) {
  if (utf8.encode(text).length > commandMaxFrameBytes) {
    throw const CommandContractException(CommandErrorKind.frameTooLarge, 'CommandFrame');
  }
  final Object? value;
  try {
    value = jsonDecode(text);
  } on FormatException {
    throw const CommandContractException(CommandErrorKind.malformedJson, 'CommandFrame');
  }
  final map = _object(value, 'CommandFrame');
  final schema = map['schema'];
  if (schema is! String) {
    throw const CommandContractException(CommandErrorKind.shape, 'CommandFrame.schema');
  }
  if (schema != commandSchemaId) {
    throw const CommandContractException(CommandErrorKind.unknownSchema, 'CommandFrame');
  }
  return _decodeCommandFrame(value, 'CommandFrame');
}

/// Encodes the one frame a frontend may originate, stamping the schema
/// envelope and the `submit` body around it and enforcing every bound
/// [decodeCommandFrame] checks. Every failure is a typed
/// [CommandContractException]; an over-bound frame never leaves the process.
String encodeCommandFrame(SubmitView body) {
  final text = jsonEncode(<String, Object?>{
    'body': <String, Object?>{
      'kind': 'submit',
      'value': _encodeSubmitView(body),
    },
    'schema': commandSchemaId,
  });
  if (utf8.encode(text).length > commandMaxFrameBytes) {
    throw const CommandContractException(CommandErrorKind.frameTooLarge, 'CommandFrame');
  }
  return text;
}

Map<String, Object?> _object(Object? value, String context) {
  if (value is! Map<String, Object?>) {
    throw CommandContractException(CommandErrorKind.shape, context);
  }
  return value;
}

void _knownKeys(Map<String, Object?> map, Set<String> allowed, String context) {
  for (final key in map.keys) {
    if (!allowed.contains(key)) {
      throw CommandContractException(CommandErrorKind.unknownField, context);
    }
  }
}

Object? _field(Map<String, Object?> map, String key, String context) {
  if (!map.containsKey(key)) {
    throw CommandContractException(CommandErrorKind.shape, context);
  }
  return map[key];
}

int _integer(Object? value, int max, String context) {
  if (value is! int) {
    throw CommandContractException(CommandErrorKind.shape, context);
  }
  if (value < 0 || value > max) {
    throw CommandContractException(CommandErrorKind.range, context);
  }
  return value;
}

bool _hexChars(String text) {
  for (final unit in text.codeUnits) {
    final digit =
        (unit >= 0x30 && unit <= 0x39) || (unit >= 0x61 && unit <= 0x66);
    if (!digit) {
      return false;
    }
  }
  return true;
}

String _hexFixed(Object? value, int chars, String context) {
  if (value is! String) {
    throw CommandContractException(CommandErrorKind.shape, context);
  }
  if (value.length != chars || !_hexChars(value)) {
    throw CommandContractException(CommandErrorKind.bound, context);
  }
  return value;
}

Object? _payload(Map<String, Object?> map, String context) {
  final value = map['value'];
  if (value == null) {
    throw CommandContractException(CommandErrorKind.shape, context);
  }
  return value;
}

void _unitPayload(Map<String, Object?> map, String context) {
  if (map['value'] != null) {
    throw CommandContractException(CommandErrorKind.shape, context);
  }
}

int _encodeInteger(int value, int max, String context) =>
    _integer(value, max, context);

String _encodeHexFixed(String value, int chars, String context) =>
    _hexFixed(value, chars, context);

CommandView _decodeCommandView(Object? value, String context) {
  return switch (value) {
    'pause' => CommandView.pause,
    'cancel' => CommandView.cancel,
    'resume' => CommandView.resume,
    'remove' => CommandView.remove,
    're_pick_source' => CommandView.rePickSource,
    String() =>
      throw CommandContractException(CommandErrorKind.unknownVariant, context),
    _ => throw CommandContractException(CommandErrorKind.shape, context),
  };
}

String _encodeCommandView(CommandView value) {
  return switch (value) {
    CommandView.pause => 'pause',
    CommandView.cancel => 'cancel',
    CommandView.resume => 'resume',
    CommandView.remove => 'remove',
    CommandView.rePickSource => 're_pick_source',
  };
}

PauseCauseView _decodePauseCauseView(Object? value, String context) {
  return switch (value) {
    'local' => PauseCauseView.local,
    'peer' => PauseCauseView.peer,
    'lost' => PauseCauseView.lost,
    String() =>
      throw CommandContractException(CommandErrorKind.unknownVariant, context),
    _ => throw CommandContractException(CommandErrorKind.shape, context),
  };
}

PausedStateView _decodePausedStateView(Object? value, String context) {
  final map = _object(value, context);
  _knownKeys(map, const {'origin'}, context);
  return PausedStateView(
    origin: _decodePauseCauseView(_field(map, 'origin', 'PausedStateView.origin'), 'PausedStateView.origin'),
  );
}

DispositionView _decodeDispositionView(Object? value, String context) {
  final map = _object(value, context);
  _knownKeys(map, const {'kind', 'value'}, context);
  final kind = _field(map, 'kind', context);
  if (kind is! String) {
    throw CommandContractException(CommandErrorKind.shape, context);
  }
  switch (kind) {
    case 'preparing':
      _unitPayload(map, 'DispositionView.preparing');
      return const DispositionViewPreparing();
    case 'waiting':
      _unitPayload(map, 'DispositionView.waiting');
      return const DispositionViewWaiting();
    case 'connecting':
      _unitPayload(map, 'DispositionView.connecting');
      return const DispositionViewConnecting();
    case 'verifying':
      _unitPayload(map, 'DispositionView.verifying');
      return const DispositionViewVerifying();
    case 'transferring':
      _unitPayload(map, 'DispositionView.transferring');
      return const DispositionViewTransferring();
    case 'confirming':
      _unitPayload(map, 'DispositionView.confirming');
      return const DispositionViewConfirming();
    case 'paused':
      return DispositionViewPaused(
        _decodePausedStateView(_payload(map, 'DispositionView.paused'), 'DispositionView.paused'),
      );
    case 'unconfirmed':
      _unitPayload(map, 'DispositionView.unconfirmed');
      return const DispositionViewUnconfirmed();
    case 'completed':
      _unitPayload(map, 'DispositionView.completed');
      return const DispositionViewCompleted();
    case 'failed':
      _unitPayload(map, 'DispositionView.failed');
      return const DispositionViewFailed();
    case 'cancelled':
      _unitPayload(map, 'DispositionView.cancelled');
      return const DispositionViewCancelled();
    default:
      throw CommandContractException(CommandErrorKind.unknownVariant, context);
  }
}

SubmitView _decodeSubmitView(Object? value, String context) {
  final map = _object(value, context);
  _knownKeys(map, const {'card', 'epoch', 'command_id', 'command'}, context);
  return SubmitView(
    card: _hexFixed(_field(map, 'card', 'SubmitView.card'), 16, 'SubmitView.card'),
    epoch: _integer(_field(map, 'epoch', 'SubmitView.epoch'), _u63Max, 'SubmitView.epoch'),
    commandId: _hexFixed(_field(map, 'command_id', 'SubmitView.command_id'), 32, 'SubmitView.command_id'),
    command: _decodeCommandView(_field(map, 'command', 'SubmitView.command'), 'SubmitView.command'),
  );
}

Map<String, Object?> _encodeSubmitView(SubmitView value) {
  return <String, Object?>{
    'card': _encodeHexFixed(value.card, 16, 'SubmitView.card'),
    'command': _encodeCommandView(value.command),
    'command_id': _encodeHexFixed(value.commandId, 32, 'SubmitView.command_id'),
    'epoch': _encodeInteger(value.epoch, _u63Max, 'SubmitView.epoch'),
  };
}

RejectionView _decodeRejectionView(Object? value, String context) {
  return switch (value) {
    'unknown_card' => RejectionView.unknownCard,
    'stale_epoch' => RejectionView.staleEpoch,
    'superseded' => RejectionView.superseded,
    'at_capacity' => RejectionView.atCapacity,
    'runtime_stopped' => RejectionView.runtimeStopped,
    'interrupted' => RejectionView.interrupted,
    'conflict' => RejectionView.conflict,
    'internal' => RejectionView.internal,
    String() =>
      throw CommandContractException(CommandErrorKind.unknownVariant, context),
    _ => throw CommandContractException(CommandErrorKind.shape, context),
  };
}

AcceptanceView _decodeAcceptanceView(Object? value, String context) {
  final map = _object(value, context);
  _knownKeys(map, const {'kind', 'value'}, context);
  final kind = _field(map, 'kind', context);
  if (kind is! String) {
    throw CommandContractException(CommandErrorKind.shape, context);
  }
  switch (kind) {
    case 'accepted':
      _unitPayload(map, 'AcceptanceView.accepted');
      return const AcceptanceViewAccepted();
    case 'duplicate':
      return AcceptanceViewDuplicate(
        _decodeDispositionView(_payload(map, 'AcceptanceView.duplicate'), 'AcceptanceView.duplicate'),
      );
    case 'rejected':
      return AcceptanceViewRejected(
        _decodeRejectionView(_payload(map, 'AcceptanceView.rejected'), 'AcceptanceView.rejected'),
      );
    default:
      throw CommandContractException(CommandErrorKind.unknownVariant, context);
  }
}

CommandAcceptanceView _decodeCommandAcceptanceView(Object? value, String context) {
  final map = _object(value, context);
  _knownKeys(map, const {'command_id', 'acceptance'}, context);
  return CommandAcceptanceView(
    commandId: _hexFixed(_field(map, 'command_id', 'CommandAcceptanceView.command_id'), 32, 'CommandAcceptanceView.command_id'),
    acceptance: _decodeAcceptanceView(_field(map, 'acceptance', 'CommandAcceptanceView.acceptance'), 'CommandAcceptanceView.acceptance'),
  );
}

CompletionView _decodeCompletionView(Object? value, String context) {
  final map = _object(value, context);
  _knownKeys(map, const {'kind', 'value'}, context);
  final kind = _field(map, 'kind', context);
  if (kind is! String) {
    throw CommandContractException(CommandErrorKind.shape, context);
  }
  switch (kind) {
    case 'committed':
      return CompletionViewCommitted(
        _decodeDispositionView(_payload(map, 'CompletionView.committed'), 'CompletionView.committed'),
      );
    case 'commit_failed':
      return CompletionViewCommitFailed(
        _decodeDispositionView(_payload(map, 'CompletionView.commit_failed'), 'CompletionView.commit_failed'),
      );
    case 'interrupted':
      _unitPayload(map, 'CompletionView.interrupted');
      return const CompletionViewInterrupted();
    case 'internal':
      _unitPayload(map, 'CompletionView.internal');
      return const CompletionViewInternal();
    default:
      throw CommandContractException(CommandErrorKind.unknownVariant, context);
  }
}

CommandCompletionView _decodeCommandCompletionView(Object? value, String context) {
  final map = _object(value, context);
  _knownKeys(map, const {'command_id', 'completion'}, context);
  return CommandCompletionView(
    commandId: _hexFixed(_field(map, 'command_id', 'CommandCompletionView.command_id'), 32, 'CommandCompletionView.command_id'),
    completion: _decodeCompletionView(_field(map, 'completion', 'CommandCompletionView.completion'), 'CommandCompletionView.completion'),
  );
}

CommandBody _decodeCommandBody(Object? value, String context) {
  final map = _object(value, context);
  _knownKeys(map, const {'kind', 'value'}, context);
  final kind = _field(map, 'kind', context);
  if (kind is! String) {
    throw CommandContractException(CommandErrorKind.shape, context);
  }
  switch (kind) {
    case 'submit':
      return CommandBodySubmit(
        _decodeSubmitView(_payload(map, 'CommandBody.submit'), 'CommandBody.submit'),
      );
    case 'acceptance':
      return CommandBodyAcceptance(
        _decodeCommandAcceptanceView(_payload(map, 'CommandBody.acceptance'), 'CommandBody.acceptance'),
      );
    case 'completion':
      return CommandBodyCompletion(
        _decodeCommandCompletionView(_payload(map, 'CommandBody.completion'), 'CommandBody.completion'),
      );
    default:
      throw CommandContractException(CommandErrorKind.unknownVariant, context);
  }
}

CommandFrame _decodeCommandFrame(Object? value, String context) {
  final map = _object(value, context);
  _knownKeys(map, const {'schema', 'body'}, context);
  return CommandFrame(
    body: _decodeCommandBody(_field(map, 'body', 'CommandFrame.body'), 'CommandFrame.body'),
  );
}
