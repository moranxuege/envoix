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

const String commandSchemaId = 'envoix/binding/command/7';
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

/// Bounded contract text whose ordinary string representation is always
/// redacted. Rendering the user-visible value requires an explicit
/// [expose] call at the UI boundary.
final class CommandSecretString {
  const CommandSecretString(this._value);

  final String _value;

  String expose() => _value;

  @override
  String toString() => 'CommandSecretString([redacted])';
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

enum LocalDirectionView {
  send,
  receive,
}

final class MintRoomView {
  const MintRoomView({
    required this.localDirection,
  });

  final LocalDirectionView localDirection;
}

final class JoinInviteView {
  const JoinInviteView({
    required this.invite,
  });

  final CommandSecretString invite;
}

sealed class CreateIntentView {
  const CreateIntentView();
}

final class CreateIntentViewMintRoom extends CreateIntentView {
  const CreateIntentViewMintRoom(this.value);

  final MintRoomView value;
}

final class CreateIntentViewJoinRoom extends CreateIntentView {
  const CreateIntentViewJoinRoom(this.value);

  final JoinInviteView value;
}

final class SourceAcquisitionKeyView {
  const SourceAcquisitionKeyView({
    required this.card,
    required this.generation,
    required this.request,
  });

  final String card;
  final int generation;
  final String request;
}

final class OfferedItemView {
  const OfferedItemView({
    required this.displayName,
    required this.reportedSize,
  });

  final String displayName;
  final int? reportedSize;
}

final class SourceOfferView {
  const SourceOfferView({
    required this.key,
    required this.items,
  });

  final SourceAcquisitionKeyView key;
  final List<OfferedItemView> items;
}

final class CreateView {
  const CreateView({
    required this.intent,
    required this.requestId,
  });

  final CreateIntentView intent;
  final String requestId;
}

enum SourceOfferAnswerView {
  accepted,
  alreadyAccepted,
  conflict,
  stale,
  unknownCard,
  notExpected,
}

enum SourceOfferRefusalView {
  staleEpoch,
  nameTooLong,
  outputRequired,
  runtimeStopped,
  interrupted,
  storageFault,
  internal,
}

sealed class SourceOfferOutcomeView {
  const SourceOfferOutcomeView();
}

final class SourceOfferOutcomeViewAnswered extends SourceOfferOutcomeView {
  const SourceOfferOutcomeViewAnswered(this.value);

  final SourceOfferAnswerView value;
}

final class SourceOfferOutcomeViewRefused extends SourceOfferOutcomeView {
  const SourceOfferOutcomeViewRefused(this.value);

  final SourceOfferRefusalView value;
}

final class SourceOfferResultView {
  const SourceOfferResultView({
    required this.key,
    required this.outcome,
  });

  final SourceAcquisitionKeyView key;
  final SourceOfferOutcomeView outcome;
}

sealed class FrontendIntentView {
  const FrontendIntentView();
}

final class FrontendIntentViewCommand extends FrontendIntentView {
  const FrontendIntentViewCommand(this.value);

  final SubmitView value;
}

final class FrontendIntentViewCreate extends FrontendIntentView {
  const FrontendIntentViewCreate(this.value);

  final CreateView value;
}

final class FrontendIntentViewSourceOffer extends FrontendIntentView {
  const FrontendIntentViewSourceOffer(this.value);

  final SourceOfferView value;
}

enum RejectionView {
  unknownCard,
  staleEpoch,
  superseded,
  atCapacity,
  runtimeStopped,
  interrupted,
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

final class AcceptanceViewConflict extends AcceptanceView {
  const AcceptanceViewConflict(this.value);

  final CommandView value;
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

enum CreateRefusalView {
  inviteNotRecognized,
  inviteBareRoomCode,
  inviteMalformed,
  inviteTooLong,
  inviteUnsupported,
  inviteRoleUnsupported,
  nameTooLong,
  storageFault,
  internal,
}

final class CardCreatedView {
  const CardCreatedView({
    required this.card,
  });

  final String card;
}

sealed class CreateOutcomeView {
  const CreateOutcomeView();
}

final class CreateOutcomeViewCreated extends CreateOutcomeView {
  const CreateOutcomeViewCreated(this.value);

  final CardCreatedView value;
}

final class CreateOutcomeViewRefused extends CreateOutcomeView {
  const CreateOutcomeViewRefused(this.value);

  final CreateRefusalView value;
}

final class CreateResultView {
  const CreateResultView({
    required this.outcome,
    required this.requestId,
  });

  final CreateOutcomeView outcome;
  final String requestId;
}

sealed class CommandBody {
  const CommandBody();
}

final class CommandBodyIntent extends CommandBody {
  const CommandBodyIntent(this.value);

  final FrontendIntentView value;
}

final class CommandBodyAcceptance extends CommandBody {
  const CommandBodyAcceptance(this.value);

  final CommandAcceptanceView value;
}

final class CommandBodyCompletion extends CommandBody {
  const CommandBodyCompletion(this.value);

  final CommandCompletionView value;
}

final class CommandBodyCreateResult extends CommandBody {
  const CommandBodyCreateResult(this.value);

  final CreateResultView value;
}

final class CommandBodySourceOfferResult extends CommandBody {
  const CommandBodySourceOfferResult(this.value);

  final SourceOfferResultView value;
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
/// envelope and the `intent` body around it and enforcing every bound
/// [decodeCommandFrame] checks. Every failure is a typed
/// [CommandContractException]; an over-bound frame never leaves the process.
String encodeCommandFrame(FrontendIntentView body) {
  final text = jsonEncode(<String, Object?>{
    'body': <String, Object?>{
      'kind': 'intent',
      'value': _encodeFrontendIntentView(body),
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

String _utf8Bounded(Object? value, int maxBytes, String context) {
  if (value is! String) {
    throw CommandContractException(CommandErrorKind.shape, context);
  }
  // Unpaired surrogates parse here but not in the Rust reference codec;
  // reject them so every language accepts the same strings.
  var index = 0;
  while (index < value.length) {
    final unit = value.codeUnitAt(index);
    if (unit >= 0xd800 && unit <= 0xdbff) {
      if (index + 1 == value.length) {
        throw CommandContractException(CommandErrorKind.shape, context);
      }
      final next = value.codeUnitAt(index + 1);
      if (next < 0xdc00 || next > 0xdfff) {
        throw CommandContractException(CommandErrorKind.shape, context);
      }
      index += 2;
    } else if (unit >= 0xdc00 && unit <= 0xdfff) {
      throw CommandContractException(CommandErrorKind.shape, context);
    } else {
      index += 1;
    }
  }
  if (utf8.encode(value).length > maxBytes) {
    throw CommandContractException(CommandErrorKind.bound, context);
  }
  return value;
}

List<T> _list<T>(
  Object? value,
  int maxLen,
  String context,
  T Function(Object?, String) decodeElement,
) {
  if (value is! List<Object?>) {
    throw CommandContractException(CommandErrorKind.shape, context);
  }
  if (value.length > maxLen) {
    throw CommandContractException(CommandErrorKind.bound, context);
  }
  return List<T>.unmodifiable(
    value.map((item) => decodeElement(item, context)),
  );
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

String _encodeUtf8Bounded(String value, int maxBytes, String context) =>
    _utf8Bounded(value, maxBytes, context);

List<Object?> _encodeList<T>(
  List<T> value,
  int maxLen,
  String context,
  Object? Function(T) encodeElement,
) {
  if (value.length > maxLen) {
    throw CommandContractException(CommandErrorKind.bound, context);
  }
  return value.map(encodeElement).toList();
}

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

LocalDirectionView _decodeLocalDirectionView(Object? value, String context) {
  return switch (value) {
    'send' => LocalDirectionView.send,
    'receive' => LocalDirectionView.receive,
    String() =>
      throw CommandContractException(CommandErrorKind.unknownVariant, context),
    _ => throw CommandContractException(CommandErrorKind.shape, context),
  };
}

String _encodeLocalDirectionView(LocalDirectionView value) {
  return switch (value) {
    LocalDirectionView.send => 'send',
    LocalDirectionView.receive => 'receive',
  };
}

MintRoomView _decodeMintRoomView(Object? value, String context) {
  final map = _object(value, context);
  _knownKeys(map, const {'local_direction'}, context);
  return MintRoomView(
    localDirection: _decodeLocalDirectionView(_field(map, 'local_direction', 'MintRoomView.local_direction'), 'MintRoomView.local_direction'),
  );
}

Map<String, Object?> _encodeMintRoomView(MintRoomView value) {
  return <String, Object?>{
    'local_direction': _encodeLocalDirectionView(value.localDirection),
  };
}

JoinInviteView _decodeJoinInviteView(Object? value, String context) {
  final map = _object(value, context);
  _knownKeys(map, const {'invite'}, context);
  return JoinInviteView(
    invite: CommandSecretString(_utf8Bounded(_field(map, 'invite', 'JoinInviteView.invite'), 16384, 'JoinInviteView.invite')),
  );
}

Map<String, Object?> _encodeJoinInviteView(JoinInviteView value) {
  return <String, Object?>{
    'invite': _encodeUtf8Bounded(value.invite.expose(), 16384, 'JoinInviteView.invite'),
  };
}

CreateIntentView _decodeCreateIntentView(Object? value, String context) {
  final map = _object(value, context);
  _knownKeys(map, const {'kind', 'value'}, context);
  final kind = _field(map, 'kind', context);
  if (kind is! String) {
    throw CommandContractException(CommandErrorKind.shape, context);
  }
  switch (kind) {
    case 'mint_room':
      return CreateIntentViewMintRoom(
        _decodeMintRoomView(_payload(map, 'CreateIntentView.mint_room'), 'CreateIntentView.mint_room'),
      );
    case 'join_room':
      return CreateIntentViewJoinRoom(
        _decodeJoinInviteView(_payload(map, 'CreateIntentView.join_room'), 'CreateIntentView.join_room'),
      );
    default:
      throw CommandContractException(CommandErrorKind.unknownVariant, context);
  }
}

Map<String, Object?> _encodeCreateIntentView(CreateIntentView value) {
  return switch (value) {
    CreateIntentViewMintRoom(value: final payload) => <String, Object?>{
        'kind': 'mint_room',
        'value': _encodeMintRoomView(payload),
      },
    CreateIntentViewJoinRoom(value: final payload) => <String, Object?>{
        'kind': 'join_room',
        'value': _encodeJoinInviteView(payload),
      },
  };
}

SourceAcquisitionKeyView _decodeSourceAcquisitionKeyView(Object? value, String context) {
  final map = _object(value, context);
  _knownKeys(map, const {'card', 'generation', 'request'}, context);
  return SourceAcquisitionKeyView(
    card: _hexFixed(_field(map, 'card', 'SourceAcquisitionKeyView.card'), 16, 'SourceAcquisitionKeyView.card'),
    generation: _integer(_field(map, 'generation', 'SourceAcquisitionKeyView.generation'), 4294967295, 'SourceAcquisitionKeyView.generation'),
    request: _hexFixed(_field(map, 'request', 'SourceAcquisitionKeyView.request'), 32, 'SourceAcquisitionKeyView.request'),
  );
}

Map<String, Object?> _encodeSourceAcquisitionKeyView(SourceAcquisitionKeyView value) {
  return <String, Object?>{
    'card': _encodeHexFixed(value.card, 16, 'SourceAcquisitionKeyView.card'),
    'generation': _encodeInteger(value.generation, 4294967295, 'SourceAcquisitionKeyView.generation'),
    'request': _encodeHexFixed(value.request, 32, 'SourceAcquisitionKeyView.request'),
  };
}

OfferedItemView _decodeOfferedItemView(Object? value, String context) {
  final map = _object(value, context);
  _knownKeys(map, const {'display_name', 'reported_size'}, context);
  return OfferedItemView(
    displayName: _utf8Bounded(_field(map, 'display_name', 'OfferedItemView.display_name'), 1020, 'OfferedItemView.display_name'),
    reportedSize: switch (_field(map, 'reported_size', 'OfferedItemView.reported_size')) {
      null => null,
      final present => _integer(present, _u63Max, 'OfferedItemView.reported_size'),
    },
  );
}

Map<String, Object?> _encodeOfferedItemView(OfferedItemView value) {
  return <String, Object?>{
    'display_name': _encodeUtf8Bounded(value.displayName, 1020, 'OfferedItemView.display_name'),
    'reported_size': value.reportedSize == null ? null : _encodeInteger(value.reportedSize!, _u63Max, 'OfferedItemView.reported_size'),
  };
}

SourceOfferView _decodeSourceOfferView(Object? value, String context) {
  final map = _object(value, context);
  _knownKeys(map, const {'key', 'items'}, context);
  return SourceOfferView(
    key: _decodeSourceAcquisitionKeyView(_field(map, 'key', 'SourceOfferView.key'), 'SourceOfferView.key'),
    items: _list(_field(map, 'items', 'SourceOfferView.items'), 1024, 'SourceOfferView.items', _decodeOfferedItemView),
  );
}

Map<String, Object?> _encodeSourceOfferView(SourceOfferView value) {
  return <String, Object?>{
    'items': _encodeList(value.items, 1024, 'SourceOfferView.items', _encodeOfferedItemView),
    'key': _encodeSourceAcquisitionKeyView(value.key),
  };
}

CreateView _decodeCreateView(Object? value, String context) {
  final map = _object(value, context);
  _knownKeys(map, const {'intent', 'request_id'}, context);
  return CreateView(
    intent: _decodeCreateIntentView(_field(map, 'intent', 'CreateView.intent'), 'CreateView.intent'),
    requestId: _hexFixed(_field(map, 'request_id', 'CreateView.request_id'), 32, 'CreateView.request_id'),
  );
}

Map<String, Object?> _encodeCreateView(CreateView value) {
  return <String, Object?>{
    'intent': _encodeCreateIntentView(value.intent),
    'request_id': _encodeHexFixed(value.requestId, 32, 'CreateView.request_id'),
  };
}

SourceOfferAnswerView _decodeSourceOfferAnswerView(Object? value, String context) {
  return switch (value) {
    'accepted' => SourceOfferAnswerView.accepted,
    'already_accepted' => SourceOfferAnswerView.alreadyAccepted,
    'conflict' => SourceOfferAnswerView.conflict,
    'stale' => SourceOfferAnswerView.stale,
    'unknown_card' => SourceOfferAnswerView.unknownCard,
    'not_expected' => SourceOfferAnswerView.notExpected,
    String() =>
      throw CommandContractException(CommandErrorKind.unknownVariant, context),
    _ => throw CommandContractException(CommandErrorKind.shape, context),
  };
}

SourceOfferRefusalView _decodeSourceOfferRefusalView(Object? value, String context) {
  return switch (value) {
    'stale_epoch' => SourceOfferRefusalView.staleEpoch,
    'name_too_long' => SourceOfferRefusalView.nameTooLong,
    'output_required' => SourceOfferRefusalView.outputRequired,
    'runtime_stopped' => SourceOfferRefusalView.runtimeStopped,
    'interrupted' => SourceOfferRefusalView.interrupted,
    'storage_fault' => SourceOfferRefusalView.storageFault,
    'internal' => SourceOfferRefusalView.internal,
    String() =>
      throw CommandContractException(CommandErrorKind.unknownVariant, context),
    _ => throw CommandContractException(CommandErrorKind.shape, context),
  };
}

SourceOfferOutcomeView _decodeSourceOfferOutcomeView(Object? value, String context) {
  final map = _object(value, context);
  _knownKeys(map, const {'kind', 'value'}, context);
  final kind = _field(map, 'kind', context);
  if (kind is! String) {
    throw CommandContractException(CommandErrorKind.shape, context);
  }
  switch (kind) {
    case 'answered':
      return SourceOfferOutcomeViewAnswered(
        _decodeSourceOfferAnswerView(_payload(map, 'SourceOfferOutcomeView.answered'), 'SourceOfferOutcomeView.answered'),
      );
    case 'refused':
      return SourceOfferOutcomeViewRefused(
        _decodeSourceOfferRefusalView(_payload(map, 'SourceOfferOutcomeView.refused'), 'SourceOfferOutcomeView.refused'),
      );
    default:
      throw CommandContractException(CommandErrorKind.unknownVariant, context);
  }
}

SourceOfferResultView _decodeSourceOfferResultView(Object? value, String context) {
  final map = _object(value, context);
  _knownKeys(map, const {'key', 'outcome'}, context);
  return SourceOfferResultView(
    key: _decodeSourceAcquisitionKeyView(_field(map, 'key', 'SourceOfferResultView.key'), 'SourceOfferResultView.key'),
    outcome: _decodeSourceOfferOutcomeView(_field(map, 'outcome', 'SourceOfferResultView.outcome'), 'SourceOfferResultView.outcome'),
  );
}

FrontendIntentView _decodeFrontendIntentView(Object? value, String context) {
  final map = _object(value, context);
  _knownKeys(map, const {'kind', 'value'}, context);
  final kind = _field(map, 'kind', context);
  if (kind is! String) {
    throw CommandContractException(CommandErrorKind.shape, context);
  }
  switch (kind) {
    case 'command':
      return FrontendIntentViewCommand(
        _decodeSubmitView(_payload(map, 'FrontendIntentView.command'), 'FrontendIntentView.command'),
      );
    case 'create':
      return FrontendIntentViewCreate(
        _decodeCreateView(_payload(map, 'FrontendIntentView.create'), 'FrontendIntentView.create'),
      );
    case 'source_offer':
      return FrontendIntentViewSourceOffer(
        _decodeSourceOfferView(_payload(map, 'FrontendIntentView.source_offer'), 'FrontendIntentView.source_offer'),
      );
    default:
      throw CommandContractException(CommandErrorKind.unknownVariant, context);
  }
}

Map<String, Object?> _encodeFrontendIntentView(FrontendIntentView value) {
  return switch (value) {
    FrontendIntentViewCommand(value: final payload) => <String, Object?>{
        'kind': 'command',
        'value': _encodeSubmitView(payload),
      },
    FrontendIntentViewCreate(value: final payload) => <String, Object?>{
        'kind': 'create',
        'value': _encodeCreateView(payload),
      },
    FrontendIntentViewSourceOffer(value: final payload) => <String, Object?>{
        'kind': 'source_offer',
        'value': _encodeSourceOfferView(payload),
      },
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
    case 'conflict':
      return AcceptanceViewConflict(
        _decodeCommandView(_payload(map, 'AcceptanceView.conflict'), 'AcceptanceView.conflict'),
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

CreateRefusalView _decodeCreateRefusalView(Object? value, String context) {
  return switch (value) {
    'invite_not_recognized' => CreateRefusalView.inviteNotRecognized,
    'invite_bare_room_code' => CreateRefusalView.inviteBareRoomCode,
    'invite_malformed' => CreateRefusalView.inviteMalformed,
    'invite_too_long' => CreateRefusalView.inviteTooLong,
    'invite_unsupported' => CreateRefusalView.inviteUnsupported,
    'invite_role_unsupported' => CreateRefusalView.inviteRoleUnsupported,
    'name_too_long' => CreateRefusalView.nameTooLong,
    'storage_fault' => CreateRefusalView.storageFault,
    'internal' => CreateRefusalView.internal,
    String() =>
      throw CommandContractException(CommandErrorKind.unknownVariant, context),
    _ => throw CommandContractException(CommandErrorKind.shape, context),
  };
}

CardCreatedView _decodeCardCreatedView(Object? value, String context) {
  final map = _object(value, context);
  _knownKeys(map, const {'card'}, context);
  return CardCreatedView(
    card: _hexFixed(_field(map, 'card', 'CardCreatedView.card'), 16, 'CardCreatedView.card'),
  );
}

CreateOutcomeView _decodeCreateOutcomeView(Object? value, String context) {
  final map = _object(value, context);
  _knownKeys(map, const {'kind', 'value'}, context);
  final kind = _field(map, 'kind', context);
  if (kind is! String) {
    throw CommandContractException(CommandErrorKind.shape, context);
  }
  switch (kind) {
    case 'created':
      return CreateOutcomeViewCreated(
        _decodeCardCreatedView(_payload(map, 'CreateOutcomeView.created'), 'CreateOutcomeView.created'),
      );
    case 'refused':
      return CreateOutcomeViewRefused(
        _decodeCreateRefusalView(_payload(map, 'CreateOutcomeView.refused'), 'CreateOutcomeView.refused'),
      );
    default:
      throw CommandContractException(CommandErrorKind.unknownVariant, context);
  }
}

CreateResultView _decodeCreateResultView(Object? value, String context) {
  final map = _object(value, context);
  _knownKeys(map, const {'outcome', 'request_id'}, context);
  return CreateResultView(
    outcome: _decodeCreateOutcomeView(_field(map, 'outcome', 'CreateResultView.outcome'), 'CreateResultView.outcome'),
    requestId: _hexFixed(_field(map, 'request_id', 'CreateResultView.request_id'), 32, 'CreateResultView.request_id'),
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
    case 'intent':
      return CommandBodyIntent(
        _decodeFrontendIntentView(_payload(map, 'CommandBody.intent'), 'CommandBody.intent'),
      );
    case 'acceptance':
      return CommandBodyAcceptance(
        _decodeCommandAcceptanceView(_payload(map, 'CommandBody.acceptance'), 'CommandBody.acceptance'),
      );
    case 'completion':
      return CommandBodyCompletion(
        _decodeCommandCompletionView(_payload(map, 'CommandBody.completion'), 'CommandBody.completion'),
      );
    case 'create_result':
      return CommandBodyCreateResult(
        _decodeCreateResultView(_payload(map, 'CommandBody.create_result'), 'CommandBody.create_result'),
      );
    case 'source_offer_result':
      return CommandBodySourceOfferResult(
        _decodeSourceOfferResultView(_payload(map, 'CommandBody.source_offer_result'), 'CommandBody.source_offer_result'),
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
