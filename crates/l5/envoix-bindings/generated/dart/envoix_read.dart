// @generated from schema/read.schema by envoix-bindings. Do not edit;
// regenerate with `ENVOIX_BINDINGS_REGEN=1 cargo test -p envoix-bindings generated_artifacts`.
// Known platform caveat: JSON `-0` decodes as integer 0 here while the Rust
// reference codec rejects it (benign: every field with a positive minimum
// still fails its range check).

import 'dart:convert';

const String readSchemaId = 'envoix/binding/read/5';
const int readMaxFrameBytes = 1048576;
const int _u63Max = 9223372036854775807;

enum ReadErrorKind {
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
final class ReadContractException implements Exception {
  const ReadContractException(this.kind, this.context);

  final ReadErrorKind kind;
  final String context;

  @override
  String toString() => 'ReadContractException(${kind.name}, $context)';
}

/// Bounded contract text whose ordinary string representation is always
/// redacted. Rendering the user-visible value requires an explicit
/// [expose] call at the UI boundary.
final class ReadSecretString {
  const ReadSecretString(this._value);

  final String _value;

  String expose() => _value;

  @override
  String toString() => 'ReadSecretString([redacted])';
}

enum DirectionView {
  send,
  receive,
}

enum PhaseView {
  preparing,
  pairing,
  authenticating,
  transferring,
  confirming,
  publishing,
  restoring,
}

enum OutcomeCodeView {
  completed,
  cancelled,
  paused,
  peerLost,
  timeout,
  unauthenticated,
  versionMismatch,
  storageFault,
  publishFailed,
  sourceUnreadable,
  networkUnreachable,
  internal,
}

enum RetryabilityView {
  retryable,
  terminal,
  needsUser,
}

enum RecoveryView {
  rePickSource,
  retryLater,
  reconnectPeer,
}

enum PauseOriginView {
  local,
  peer,
  lost,
}

enum WorkerKindView {
  attempt,
  staging,
}

enum RetirementIntentView {
  pause,
  cancel,
  finalize,
}

enum DutyKindView {
  sourceHandle,
  grant,
  staging,
  publication,
  courier,
  foreground,
  notification,
  lock,
  openShare,
}

enum CapabilityActionView {
  postReceipt,
  selectSource,
}

enum CommandKindView {
  pause,
  cancel,
  resume,
  remove,
  rePickSource,
}

enum RedactedIdKindView {
  record,
  transfer,
  artifact,
  request,
}

enum LosslessKindView {
  terminal,
  capabilityDuty,
}

enum SubscribeRejectionView {
  unknownCard,
  runtimeStopped,
  epochExhausted,
}

final class OutcomeView {
  const OutcomeView({
    required this.code,
    required this.phase,
    required this.retry,
    required this.recovery,
    required this.display,
  });

  final OutcomeCodeView code;
  final PhaseView phase;
  final RetryabilityView retry;
  final RecoveryView? recovery;
  final String display;
}

final class PausedView {
  const PausedView({
    required this.origin,
  });

  final PauseOriginView origin;
}

sealed class ProductStateView {
  const ProductStateView();
}

final class ProductStateViewPreparing extends ProductStateView {
  const ProductStateViewPreparing();
}

final class ProductStateViewWaiting extends ProductStateView {
  const ProductStateViewWaiting();
}

final class ProductStateViewConnecting extends ProductStateView {
  const ProductStateViewConnecting();
}

final class ProductStateViewVerifying extends ProductStateView {
  const ProductStateViewVerifying();
}

final class ProductStateViewTransferring extends ProductStateView {
  const ProductStateViewTransferring();
}

final class ProductStateViewConfirming extends ProductStateView {
  const ProductStateViewConfirming();
}

final class ProductStateViewPaused extends ProductStateView {
  const ProductStateViewPaused(this.value);

  final PausedView value;
}

final class ProductStateViewUnconfirmed extends ProductStateView {
  const ProductStateViewUnconfirmed();
}

final class ProductStateViewCompleted extends ProductStateView {
  const ProductStateViewCompleted();
}

final class ProductStateViewFailed extends ProductStateView {
  const ProductStateViewFailed();
}

final class ProductStateViewCancelled extends ProductStateView {
  const ProductStateViewCancelled();
}

final class RunningView {
  const RunningView({
    required this.worker,
  });

  final WorkerKindView worker;
}

final class RetiringView {
  const RetiringView({
    required this.worker,
    required this.intent,
  });

  final WorkerKindView worker;
  final RetirementIntentView intent;
}

sealed class QuiescenceView {
  const QuiescenceView();
}

final class QuiescenceViewRunning extends QuiescenceView {
  const QuiescenceViewRunning(this.value);

  final RunningView value;
}

final class QuiescenceViewRetiring extends QuiescenceView {
  const QuiescenceViewRetiring(this.value);

  final RetiringView value;
}

final class QuiescenceViewQuiescent extends QuiescenceView {
  const QuiescenceViewQuiescent();
}

final class IdentityView {
  const IdentityView({
    required this.card,
    required this.transfer,
    required this.artifact,
  });

  final String card;
  final String transfer;
  final String artifact;
}

final class InviteView {
  const InviteView({
    required this.code,
    required this.codeFingerprint,
    required this.link,
  });

  final ReadSecretString code;
  final String codeFingerprint;
  final ReadSecretString? link;
}

final class CardView {
  const CardView({
    required this.identity,
    required this.direction,
    required this.offeredName,
    required this.total,
    required this.state,
    required this.quiescence,
    required this.generation,
    required this.phase,
    required this.bytes,
    required this.bytesResumed,
    required this.outcome,
    required this.allowedActions,
    required this.invite,
  });

  final IdentityView identity;
  final DirectionView direction;
  final String offeredName;
  final int total;
  final ProductStateView state;
  final QuiescenceView quiescence;
  final int generation;
  final PhaseView phase;
  final int bytes;
  final int bytesResumed;
  final OutcomeView? outcome;
  final List<CommandKindView> allowedActions;
  final InviteView? invite;
}

final class DutyProvenanceView {
  const DutyProvenanceView({
    required this.card,
    required this.generation,
    required this.request,
  });

  final String card;
  final int generation;
  final String request;
}

final class DutyView {
  const DutyView({
    required this.provenance,
    required this.kind,
  });

  final DutyProvenanceView provenance;
  final DutyKindView kind;
}

final class DutyFrameView {
  const DutyFrameView({
    required this.duty,
    required this.action,
  });

  final DutyView duty;
  final CapabilityActionView action;
}

sealed class CardUpdateKindView {
  const CardUpdateKindView();
}

final class CardUpdateKindViewSnapshot extends CardUpdateKindView {
  const CardUpdateKindViewSnapshot(this.value);

  final CardView value;
}

final class CardUpdateKindViewProgress extends CardUpdateKindView {
  const CardUpdateKindViewProgress(this.value);

  final CardView value;
}

final class CardUpdateKindViewState extends CardUpdateKindView {
  const CardUpdateKindViewState(this.value);

  final CardView value;
}

final class CardUpdateKindViewTerminal extends CardUpdateKindView {
  const CardUpdateKindViewTerminal(this.value);

  final CardView value;
}

final class CardUpdateKindViewCapabilityDuty extends CardUpdateKindView {
  const CardUpdateKindViewCapabilityDuty(this.value);

  final DutyFrameView value;
}

final class CardUpdateView {
  const CardUpdateView({
    required this.epoch,
    required this.card,
    required this.kind,
  });

  final int epoch;
  final String card;
  final CardUpdateKindView kind;
}

final class LagView {
  const LagView({
    required this.epoch,
    required this.card,
    required this.missed,
  });

  final int epoch;
  final String card;
  final LosslessKindView missed;
}

final class ClosedView {
  const ClosedView({
    required this.epoch,
    required this.card,
  });

  final int epoch;
  final String card;
}

final class SubscribeRejectedView {
  const SubscribeRejectedView({
    required this.card,
    required this.reason,
  });

  final String card;
  final SubscribeRejectionView reason;
}

final class SessionKeyView {
  const SessionKeyView({
    required this.card,
    required this.generation,
  });

  final String card;
  final int generation;
}

final class EvidenceProgressView {
  const EvidenceProgressView({
    required this.transferred,
    required this.total,
  });

  final int transferred;
  final int total;
}

final class RedactedIdView {
  const RedactedIdView({
    required this.kind,
  });

  final RedactedIdKindView kind;
}

sealed class EvidenceValueView {
  const EvidenceValueView();
}

final class EvidenceValueViewPhase extends EvidenceValueView {
  const EvidenceValueViewPhase(this.value);

  final PhaseView value;
}

final class EvidenceValueViewProgress extends EvidenceValueView {
  const EvidenceValueViewProgress(this.value);

  final EvidenceProgressView value;
}

final class EvidenceValueViewOutcome extends EvidenceValueView {
  const EvidenceValueViewOutcome(this.value);

  final OutcomeView value;
}

final class EvidenceValueViewIdentifier extends EvidenceValueView {
  const EvidenceValueViewIdentifier(this.value);

  final RedactedIdView value;
}

final class DegradedView {
  const DegradedView({
    required this.droppedEvents,
  });

  final int droppedEvents;
}

sealed class DiagnosticsStatusView {
  const DiagnosticsStatusView();
}

final class DiagnosticsStatusViewComplete extends DiagnosticsStatusView {
  const DiagnosticsStatusViewComplete();
}

final class DiagnosticsStatusViewDegraded extends DiagnosticsStatusView {
  const DiagnosticsStatusViewDegraded(this.value);

  final DegradedView value;
}

final class TimelineEntryView {
  const TimelineEntryView({
    required this.sequence,
    required this.value,
  });

  final int sequence;
  final EvidenceValueView value;
}

final class EvidenceTimelineView {
  const EvidenceTimelineView({
    required this.session,
    required this.status,
    required this.entries,
  });

  final SessionKeyView session;
  final DiagnosticsStatusView status;
  final List<TimelineEntryView> entries;
}

final class ProtocolManifestView {
  const ProtocolManifestView({
    required this.setId,
    required this.dataAlpn,
    required this.dataMagic,
    required this.dataWireVersion,
  });

  final String setId;
  final String dataAlpn;
  final String dataMagic;
  final int dataWireVersion;
}

final class AbiSchemaManifestView {
  const AbiSchemaManifestView({
    required this.readBindingSchemaId,
    required this.commandBindingSchemaId,
    required this.evidenceRustAbiId,
    required this.evidenceTimelineSchemaId,
    required this.mailboxReceiptSchemaId,
    required this.operationEnvelopeSchemaId,
  });

  final String readBindingSchemaId;
  final String commandBindingSchemaId;
  final String evidenceRustAbiId;
  final String evidenceTimelineSchemaId;
  final String mailboxReceiptSchemaId;
  final String operationEnvelopeSchemaId;
}

final class TrustRootSha256View {
  const TrustRootSha256View({
    required this.fingerprint,
  });

  final String fingerprint;
}

sealed class TrustRootView {
  const TrustRootView();
}

final class TrustRootViewUnprovisioned extends TrustRootView {
  const TrustRootViewUnprovisioned();
}

final class TrustRootViewSha256 extends TrustRootView {
  const TrustRootViewSha256(this.value);

  final TrustRootSha256View value;
}

final class BuildManifestView {
  const BuildManifestView({
    required this.packageVersion,
    required this.protocol,
    required this.abiSchema,
    required this.trustRoot,
  });

  final String packageVersion;
  final ProtocolManifestView protocol;
  final AbiSchemaManifestView abiSchema;
  final TrustRootView trustRoot;
}

sealed class ReadBody {
  const ReadBody();
}

final class ReadBodyCardUpdate extends ReadBody {
  const ReadBodyCardUpdate(this.value);

  final CardUpdateView value;
}

final class ReadBodyLag extends ReadBody {
  const ReadBodyLag(this.value);

  final LagView value;
}

final class ReadBodyClosed extends ReadBody {
  const ReadBodyClosed(this.value);

  final ClosedView value;
}

final class ReadBodySubscribeRejected extends ReadBody {
  const ReadBodySubscribeRejected(this.value);

  final SubscribeRejectedView value;
}

final class ReadBodyEvidence extends ReadBody {
  const ReadBodyEvidence(this.value);

  final EvidenceTimelineView value;
}

final class ReadBodyBuildManifest extends ReadBody {
  const ReadBodyBuildManifest(this.value);

  final BuildManifestView value;
}

final class ReadFrame {
  const ReadFrame({
    required this.body,
  });

  final ReadBody body;
}

enum GateDecision { deliver, dropStale, contractBreach }

/// Client-side admission for the per-epoch card stream: one gate per
/// attachment. Frames from another epoch are stale; every epoch starts
/// with a snapshot; a lag or close ends the epoch permanently.
final class EpochGate {
  EpochGate.attach(this._epoch);

  final int _epoch;
  bool _sawSnapshot = false;
  bool _dead = false;

  GateDecision admit(ReadFrame frame) {
    switch (frame.body) {
      case final ReadBodyCardUpdate body:
        final update = body.value;
        if (update.epoch != _epoch || _dead) {
          return GateDecision.dropStale;
        }
        if (update.kind is CardUpdateKindViewSnapshot) {
          if (_sawSnapshot) {
            return GateDecision.contractBreach;
          }
          _sawSnapshot = true;
          return GateDecision.deliver;
        }
        return _sawSnapshot
            ? GateDecision.deliver
            : GateDecision.contractBreach;
      case final ReadBodyLag body:
        return _terminate(body.value.epoch);
      case final ReadBodyClosed body:
        return _terminate(body.value.epoch);
      default:
        return GateDecision.deliver;
    }
  }

  GateDecision _terminate(int epoch) {
    if (epoch == _epoch && !_dead) {
      _dead = true;
      return GateDecision.deliver;
    }
    return GateDecision.dropStale;
  }
}

/// Decodes and validates one frame. Every failure is a typed
/// [ReadContractException]; no input, however hostile, misparses.
ReadFrame decodeReadFrame(String text) {
  if (utf8.encode(text).length > readMaxFrameBytes) {
    throw const ReadContractException(ReadErrorKind.frameTooLarge, 'ReadFrame');
  }
  final Object? value;
  try {
    value = jsonDecode(text);
  } on FormatException {
    throw const ReadContractException(ReadErrorKind.malformedJson, 'ReadFrame');
  }
  final map = _object(value, 'ReadFrame');
  final schema = map['schema'];
  if (schema is! String) {
    throw const ReadContractException(ReadErrorKind.shape, 'ReadFrame.schema');
  }
  if (schema != readSchemaId) {
    throw const ReadContractException(ReadErrorKind.unknownSchema, 'ReadFrame');
  }
  return _decodeReadFrame(value, 'ReadFrame');
}

Map<String, Object?> _object(Object? value, String context) {
  if (value is! Map<String, Object?>) {
    throw ReadContractException(ReadErrorKind.shape, context);
  }
  return value;
}

void _knownKeys(Map<String, Object?> map, Set<String> allowed, String context) {
  for (final key in map.keys) {
    if (!allowed.contains(key)) {
      throw ReadContractException(ReadErrorKind.unknownField, context);
    }
  }
}

Object? _field(Map<String, Object?> map, String key, String context) {
  if (!map.containsKey(key)) {
    throw ReadContractException(ReadErrorKind.shape, context);
  }
  return map[key];
}

int _integer(Object? value, int max, String context) {
  if (value is! int) {
    throw ReadContractException(ReadErrorKind.shape, context);
  }
  if (value < 0 || value > max) {
    throw ReadContractException(ReadErrorKind.range, context);
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
    throw ReadContractException(ReadErrorKind.shape, context);
  }
  if (value.length != chars || !_hexChars(value)) {
    throw ReadContractException(ReadErrorKind.bound, context);
  }
  return value;
}

String _hexVariable(Object? value, int maxChars, String context) {
  if (value is! String) {
    throw ReadContractException(ReadErrorKind.shape, context);
  }
  final valid = value.isNotEmpty &&
      value.length.isEven &&
      value.length <= maxChars &&
      _hexChars(value);
  if (!valid) {
    throw ReadContractException(ReadErrorKind.bound, context);
  }
  return value;
}

String _utf8Bounded(Object? value, int maxBytes, String context) {
  if (value is! String) {
    throw ReadContractException(ReadErrorKind.shape, context);
  }
  // Unpaired surrogates parse here but not in the Rust reference codec;
  // reject them so every language accepts the same strings.
  var index = 0;
  while (index < value.length) {
    final unit = value.codeUnitAt(index);
    if (unit >= 0xd800 && unit <= 0xdbff) {
      if (index + 1 == value.length) {
        throw ReadContractException(ReadErrorKind.shape, context);
      }
      final next = value.codeUnitAt(index + 1);
      if (next < 0xdc00 || next > 0xdfff) {
        throw ReadContractException(ReadErrorKind.shape, context);
      }
      index += 2;
    } else if (unit >= 0xdc00 && unit <= 0xdfff) {
      throw ReadContractException(ReadErrorKind.shape, context);
    } else {
      index += 1;
    }
  }
  if (utf8.encode(value).length > maxBytes) {
    throw ReadContractException(ReadErrorKind.bound, context);
  }
  return value;
}

String _asciiBounded(Object? value, int maxBytes, String context) {
  if (value is! String) {
    throw ReadContractException(ReadErrorKind.shape, context);
  }
  if (value.length > maxBytes) {
    throw ReadContractException(ReadErrorKind.bound, context);
  }
  for (final unit in value.codeUnits) {
    if (unit < 0x20 || unit > 0x7e) {
      throw ReadContractException(ReadErrorKind.bound, context);
    }
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
    throw ReadContractException(ReadErrorKind.shape, context);
  }
  if (value.length > maxLen) {
    throw ReadContractException(ReadErrorKind.bound, context);
  }
  return List<T>.unmodifiable(
    value.map((item) => decodeElement(item, context)),
  );
}

Object? _payload(Map<String, Object?> map, String context) {
  final value = map['value'];
  if (value == null) {
    throw ReadContractException(ReadErrorKind.shape, context);
  }
  return value;
}

void _unitPayload(Map<String, Object?> map, String context) {
  if (map['value'] != null) {
    throw ReadContractException(ReadErrorKind.shape, context);
  }
}

DirectionView _decodeDirectionView(Object? value, String context) {
  return switch (value) {
    'send' => DirectionView.send,
    'receive' => DirectionView.receive,
    String() =>
      throw ReadContractException(ReadErrorKind.unknownVariant, context),
    _ => throw ReadContractException(ReadErrorKind.shape, context),
  };
}

PhaseView _decodePhaseView(Object? value, String context) {
  return switch (value) {
    'preparing' => PhaseView.preparing,
    'pairing' => PhaseView.pairing,
    'authenticating' => PhaseView.authenticating,
    'transferring' => PhaseView.transferring,
    'confirming' => PhaseView.confirming,
    'publishing' => PhaseView.publishing,
    'restoring' => PhaseView.restoring,
    String() =>
      throw ReadContractException(ReadErrorKind.unknownVariant, context),
    _ => throw ReadContractException(ReadErrorKind.shape, context),
  };
}

OutcomeCodeView _decodeOutcomeCodeView(Object? value, String context) {
  return switch (value) {
    'completed' => OutcomeCodeView.completed,
    'cancelled' => OutcomeCodeView.cancelled,
    'paused' => OutcomeCodeView.paused,
    'peer_lost' => OutcomeCodeView.peerLost,
    'timeout' => OutcomeCodeView.timeout,
    'unauthenticated' => OutcomeCodeView.unauthenticated,
    'version_mismatch' => OutcomeCodeView.versionMismatch,
    'storage_fault' => OutcomeCodeView.storageFault,
    'publish_failed' => OutcomeCodeView.publishFailed,
    'source_unreadable' => OutcomeCodeView.sourceUnreadable,
    'network_unreachable' => OutcomeCodeView.networkUnreachable,
    'internal' => OutcomeCodeView.internal,
    String() =>
      throw ReadContractException(ReadErrorKind.unknownVariant, context),
    _ => throw ReadContractException(ReadErrorKind.shape, context),
  };
}

RetryabilityView _decodeRetryabilityView(Object? value, String context) {
  return switch (value) {
    'retryable' => RetryabilityView.retryable,
    'terminal' => RetryabilityView.terminal,
    'needs_user' => RetryabilityView.needsUser,
    String() =>
      throw ReadContractException(ReadErrorKind.unknownVariant, context),
    _ => throw ReadContractException(ReadErrorKind.shape, context),
  };
}

RecoveryView _decodeRecoveryView(Object? value, String context) {
  return switch (value) {
    're_pick_source' => RecoveryView.rePickSource,
    'retry_later' => RecoveryView.retryLater,
    'reconnect_peer' => RecoveryView.reconnectPeer,
    String() =>
      throw ReadContractException(ReadErrorKind.unknownVariant, context),
    _ => throw ReadContractException(ReadErrorKind.shape, context),
  };
}

PauseOriginView _decodePauseOriginView(Object? value, String context) {
  return switch (value) {
    'local' => PauseOriginView.local,
    'peer' => PauseOriginView.peer,
    'lost' => PauseOriginView.lost,
    String() =>
      throw ReadContractException(ReadErrorKind.unknownVariant, context),
    _ => throw ReadContractException(ReadErrorKind.shape, context),
  };
}

WorkerKindView _decodeWorkerKindView(Object? value, String context) {
  return switch (value) {
    'attempt' => WorkerKindView.attempt,
    'staging' => WorkerKindView.staging,
    String() =>
      throw ReadContractException(ReadErrorKind.unknownVariant, context),
    _ => throw ReadContractException(ReadErrorKind.shape, context),
  };
}

RetirementIntentView _decodeRetirementIntentView(Object? value, String context) {
  return switch (value) {
    'pause' => RetirementIntentView.pause,
    'cancel' => RetirementIntentView.cancel,
    'finalize' => RetirementIntentView.finalize,
    String() =>
      throw ReadContractException(ReadErrorKind.unknownVariant, context),
    _ => throw ReadContractException(ReadErrorKind.shape, context),
  };
}

DutyKindView _decodeDutyKindView(Object? value, String context) {
  return switch (value) {
    'source_handle' => DutyKindView.sourceHandle,
    'grant' => DutyKindView.grant,
    'staging' => DutyKindView.staging,
    'publication' => DutyKindView.publication,
    'courier' => DutyKindView.courier,
    'foreground' => DutyKindView.foreground,
    'notification' => DutyKindView.notification,
    'lock' => DutyKindView.lock,
    'open_share' => DutyKindView.openShare,
    String() =>
      throw ReadContractException(ReadErrorKind.unknownVariant, context),
    _ => throw ReadContractException(ReadErrorKind.shape, context),
  };
}

CapabilityActionView _decodeCapabilityActionView(Object? value, String context) {
  return switch (value) {
    'post_receipt' => CapabilityActionView.postReceipt,
    'select_source' => CapabilityActionView.selectSource,
    String() =>
      throw ReadContractException(ReadErrorKind.unknownVariant, context),
    _ => throw ReadContractException(ReadErrorKind.shape, context),
  };
}

CommandKindView _decodeCommandKindView(Object? value, String context) {
  return switch (value) {
    'pause' => CommandKindView.pause,
    'cancel' => CommandKindView.cancel,
    'resume' => CommandKindView.resume,
    'remove' => CommandKindView.remove,
    're_pick_source' => CommandKindView.rePickSource,
    String() =>
      throw ReadContractException(ReadErrorKind.unknownVariant, context),
    _ => throw ReadContractException(ReadErrorKind.shape, context),
  };
}

RedactedIdKindView _decodeRedactedIdKindView(Object? value, String context) {
  return switch (value) {
    'record' => RedactedIdKindView.record,
    'transfer' => RedactedIdKindView.transfer,
    'artifact' => RedactedIdKindView.artifact,
    'request' => RedactedIdKindView.request,
    String() =>
      throw ReadContractException(ReadErrorKind.unknownVariant, context),
    _ => throw ReadContractException(ReadErrorKind.shape, context),
  };
}

LosslessKindView _decodeLosslessKindView(Object? value, String context) {
  return switch (value) {
    'terminal' => LosslessKindView.terminal,
    'capability_duty' => LosslessKindView.capabilityDuty,
    String() =>
      throw ReadContractException(ReadErrorKind.unknownVariant, context),
    _ => throw ReadContractException(ReadErrorKind.shape, context),
  };
}

SubscribeRejectionView _decodeSubscribeRejectionView(Object? value, String context) {
  return switch (value) {
    'unknown_card' => SubscribeRejectionView.unknownCard,
    'runtime_stopped' => SubscribeRejectionView.runtimeStopped,
    'epoch_exhausted' => SubscribeRejectionView.epochExhausted,
    String() =>
      throw ReadContractException(ReadErrorKind.unknownVariant, context),
    _ => throw ReadContractException(ReadErrorKind.shape, context),
  };
}

OutcomeView _decodeOutcomeView(Object? value, String context) {
  final map = _object(value, context);
  _knownKeys(map, const {'code', 'phase', 'retry', 'recovery', 'display'}, context);
  return OutcomeView(
    code: _decodeOutcomeCodeView(_field(map, 'code', 'OutcomeView.code'), 'OutcomeView.code'),
    phase: _decodePhaseView(_field(map, 'phase', 'OutcomeView.phase'), 'OutcomeView.phase'),
    retry: _decodeRetryabilityView(_field(map, 'retry', 'OutcomeView.retry'), 'OutcomeView.retry'),
    recovery: switch (_field(map, 'recovery', 'OutcomeView.recovery')) {
      null => null,
      final present => _decodeRecoveryView(present, 'OutcomeView.recovery'),
    },
    display: _utf8Bounded(_field(map, 'display', 'OutcomeView.display'), 160, 'OutcomeView.display'),
  );
}

PausedView _decodePausedView(Object? value, String context) {
  final map = _object(value, context);
  _knownKeys(map, const {'origin'}, context);
  return PausedView(
    origin: _decodePauseOriginView(_field(map, 'origin', 'PausedView.origin'), 'PausedView.origin'),
  );
}

ProductStateView _decodeProductStateView(Object? value, String context) {
  final map = _object(value, context);
  _knownKeys(map, const {'kind', 'value'}, context);
  final kind = _field(map, 'kind', context);
  if (kind is! String) {
    throw ReadContractException(ReadErrorKind.shape, context);
  }
  switch (kind) {
    case 'preparing':
      _unitPayload(map, 'ProductStateView.preparing');
      return const ProductStateViewPreparing();
    case 'waiting':
      _unitPayload(map, 'ProductStateView.waiting');
      return const ProductStateViewWaiting();
    case 'connecting':
      _unitPayload(map, 'ProductStateView.connecting');
      return const ProductStateViewConnecting();
    case 'verifying':
      _unitPayload(map, 'ProductStateView.verifying');
      return const ProductStateViewVerifying();
    case 'transferring':
      _unitPayload(map, 'ProductStateView.transferring');
      return const ProductStateViewTransferring();
    case 'confirming':
      _unitPayload(map, 'ProductStateView.confirming');
      return const ProductStateViewConfirming();
    case 'paused':
      return ProductStateViewPaused(
        _decodePausedView(_payload(map, 'ProductStateView.paused'), 'ProductStateView.paused'),
      );
    case 'unconfirmed':
      _unitPayload(map, 'ProductStateView.unconfirmed');
      return const ProductStateViewUnconfirmed();
    case 'completed':
      _unitPayload(map, 'ProductStateView.completed');
      return const ProductStateViewCompleted();
    case 'failed':
      _unitPayload(map, 'ProductStateView.failed');
      return const ProductStateViewFailed();
    case 'cancelled':
      _unitPayload(map, 'ProductStateView.cancelled');
      return const ProductStateViewCancelled();
    default:
      throw ReadContractException(ReadErrorKind.unknownVariant, context);
  }
}

RunningView _decodeRunningView(Object? value, String context) {
  final map = _object(value, context);
  _knownKeys(map, const {'worker'}, context);
  return RunningView(
    worker: _decodeWorkerKindView(_field(map, 'worker', 'RunningView.worker'), 'RunningView.worker'),
  );
}

RetiringView _decodeRetiringView(Object? value, String context) {
  final map = _object(value, context);
  _knownKeys(map, const {'worker', 'intent'}, context);
  return RetiringView(
    worker: _decodeWorkerKindView(_field(map, 'worker', 'RetiringView.worker'), 'RetiringView.worker'),
    intent: _decodeRetirementIntentView(_field(map, 'intent', 'RetiringView.intent'), 'RetiringView.intent'),
  );
}

QuiescenceView _decodeQuiescenceView(Object? value, String context) {
  final map = _object(value, context);
  _knownKeys(map, const {'kind', 'value'}, context);
  final kind = _field(map, 'kind', context);
  if (kind is! String) {
    throw ReadContractException(ReadErrorKind.shape, context);
  }
  switch (kind) {
    case 'running':
      return QuiescenceViewRunning(
        _decodeRunningView(_payload(map, 'QuiescenceView.running'), 'QuiescenceView.running'),
      );
    case 'retiring':
      return QuiescenceViewRetiring(
        _decodeRetiringView(_payload(map, 'QuiescenceView.retiring'), 'QuiescenceView.retiring'),
      );
    case 'quiescent':
      _unitPayload(map, 'QuiescenceView.quiescent');
      return const QuiescenceViewQuiescent();
    default:
      throw ReadContractException(ReadErrorKind.unknownVariant, context);
  }
}

IdentityView _decodeIdentityView(Object? value, String context) {
  final map = _object(value, context);
  _knownKeys(map, const {'card', 'transfer', 'artifact'}, context);
  return IdentityView(
    card: _hexFixed(_field(map, 'card', 'IdentityView.card'), 16, 'IdentityView.card'),
    transfer: _hexFixed(_field(map, 'transfer', 'IdentityView.transfer'), 32, 'IdentityView.transfer'),
    artifact: _hexFixed(_field(map, 'artifact', 'IdentityView.artifact'), 32, 'IdentityView.artifact'),
  );
}

InviteView _decodeInviteView(Object? value, String context) {
  final map = _object(value, context);
  _knownKeys(map, const {'code', 'code_fingerprint', 'link'}, context);
  return InviteView(
    code: ReadSecretString(_utf8Bounded(_field(map, 'code', 'InviteView.code'), 64, 'InviteView.code')),
    codeFingerprint: _hexFixed(_field(map, 'code_fingerprint', 'InviteView.code_fingerprint'), 16, 'InviteView.code_fingerprint'),
    link: switch (_field(map, 'link', 'InviteView.link')) {
      null => null,
      final present => ReadSecretString(_utf8Bounded(present, 5481, 'InviteView.link')),
    },
  );
}

CardView _decodeCardView(Object? value, String context) {
  final map = _object(value, context);
  _knownKeys(map, const {'identity', 'direction', 'offered_name', 'total', 'state', 'quiescence', 'generation', 'phase', 'bytes', 'bytes_resumed', 'outcome', 'allowed_actions', 'invite'}, context);
  return CardView(
    identity: _decodeIdentityView(_field(map, 'identity', 'CardView.identity'), 'CardView.identity'),
    direction: _decodeDirectionView(_field(map, 'direction', 'CardView.direction'), 'CardView.direction'),
    offeredName: _utf8Bounded(_field(map, 'offered_name', 'CardView.offered_name'), 255, 'CardView.offered_name'),
    total: _integer(_field(map, 'total', 'CardView.total'), _u63Max, 'CardView.total'),
    state: _decodeProductStateView(_field(map, 'state', 'CardView.state'), 'CardView.state'),
    quiescence: _decodeQuiescenceView(_field(map, 'quiescence', 'CardView.quiescence'), 'CardView.quiescence'),
    generation: _integer(_field(map, 'generation', 'CardView.generation'), 4294967295, 'CardView.generation'),
    phase: _decodePhaseView(_field(map, 'phase', 'CardView.phase'), 'CardView.phase'),
    bytes: _integer(_field(map, 'bytes', 'CardView.bytes'), _u63Max, 'CardView.bytes'),
    bytesResumed: _integer(_field(map, 'bytes_resumed', 'CardView.bytes_resumed'), _u63Max, 'CardView.bytes_resumed'),
    outcome: switch (_field(map, 'outcome', 'CardView.outcome')) {
      null => null,
      final present => _decodeOutcomeView(present, 'CardView.outcome'),
    },
    allowedActions: _list(_field(map, 'allowed_actions', 'CardView.allowed_actions'), 5, 'CardView.allowed_actions', _decodeCommandKindView),
    invite: switch (_field(map, 'invite', 'CardView.invite')) {
      null => null,
      final present => _decodeInviteView(present, 'CardView.invite'),
    },
  );
}

DutyProvenanceView _decodeDutyProvenanceView(Object? value, String context) {
  final map = _object(value, context);
  _knownKeys(map, const {'card', 'generation', 'request'}, context);
  return DutyProvenanceView(
    card: _hexFixed(_field(map, 'card', 'DutyProvenanceView.card'), 16, 'DutyProvenanceView.card'),
    generation: _integer(_field(map, 'generation', 'DutyProvenanceView.generation'), 4294967295, 'DutyProvenanceView.generation'),
    request: _hexFixed(_field(map, 'request', 'DutyProvenanceView.request'), 32, 'DutyProvenanceView.request'),
  );
}

DutyView _decodeDutyView(Object? value, String context) {
  final map = _object(value, context);
  _knownKeys(map, const {'provenance', 'kind'}, context);
  return DutyView(
    provenance: _decodeDutyProvenanceView(_field(map, 'provenance', 'DutyView.provenance'), 'DutyView.provenance'),
    kind: _decodeDutyKindView(_field(map, 'kind', 'DutyView.kind'), 'DutyView.kind'),
  );
}

DutyFrameView _decodeDutyFrameView(Object? value, String context) {
  final map = _object(value, context);
  _knownKeys(map, const {'duty', 'action'}, context);
  return DutyFrameView(
    duty: _decodeDutyView(_field(map, 'duty', 'DutyFrameView.duty'), 'DutyFrameView.duty'),
    action: _decodeCapabilityActionView(_field(map, 'action', 'DutyFrameView.action'), 'DutyFrameView.action'),
  );
}

CardUpdateKindView _decodeCardUpdateKindView(Object? value, String context) {
  final map = _object(value, context);
  _knownKeys(map, const {'kind', 'value'}, context);
  final kind = _field(map, 'kind', context);
  if (kind is! String) {
    throw ReadContractException(ReadErrorKind.shape, context);
  }
  switch (kind) {
    case 'snapshot':
      return CardUpdateKindViewSnapshot(
        _decodeCardView(_payload(map, 'CardUpdateKindView.snapshot'), 'CardUpdateKindView.snapshot'),
      );
    case 'progress':
      return CardUpdateKindViewProgress(
        _decodeCardView(_payload(map, 'CardUpdateKindView.progress'), 'CardUpdateKindView.progress'),
      );
    case 'state':
      return CardUpdateKindViewState(
        _decodeCardView(_payload(map, 'CardUpdateKindView.state'), 'CardUpdateKindView.state'),
      );
    case 'terminal':
      return CardUpdateKindViewTerminal(
        _decodeCardView(_payload(map, 'CardUpdateKindView.terminal'), 'CardUpdateKindView.terminal'),
      );
    case 'capability_duty':
      return CardUpdateKindViewCapabilityDuty(
        _decodeDutyFrameView(_payload(map, 'CardUpdateKindView.capability_duty'), 'CardUpdateKindView.capability_duty'),
      );
    default:
      throw ReadContractException(ReadErrorKind.unknownVariant, context);
  }
}

CardUpdateView _decodeCardUpdateView(Object? value, String context) {
  final map = _object(value, context);
  _knownKeys(map, const {'epoch', 'card', 'kind'}, context);
  return CardUpdateView(
    epoch: _integer(_field(map, 'epoch', 'CardUpdateView.epoch'), _u63Max, 'CardUpdateView.epoch'),
    card: _hexFixed(_field(map, 'card', 'CardUpdateView.card'), 16, 'CardUpdateView.card'),
    kind: _decodeCardUpdateKindView(_field(map, 'kind', 'CardUpdateView.kind'), 'CardUpdateView.kind'),
  );
}

LagView _decodeLagView(Object? value, String context) {
  final map = _object(value, context);
  _knownKeys(map, const {'epoch', 'card', 'missed'}, context);
  return LagView(
    epoch: _integer(_field(map, 'epoch', 'LagView.epoch'), _u63Max, 'LagView.epoch'),
    card: _hexFixed(_field(map, 'card', 'LagView.card'), 16, 'LagView.card'),
    missed: _decodeLosslessKindView(_field(map, 'missed', 'LagView.missed'), 'LagView.missed'),
  );
}

ClosedView _decodeClosedView(Object? value, String context) {
  final map = _object(value, context);
  _knownKeys(map, const {'epoch', 'card'}, context);
  return ClosedView(
    epoch: _integer(_field(map, 'epoch', 'ClosedView.epoch'), _u63Max, 'ClosedView.epoch'),
    card: _hexFixed(_field(map, 'card', 'ClosedView.card'), 16, 'ClosedView.card'),
  );
}

SubscribeRejectedView _decodeSubscribeRejectedView(Object? value, String context) {
  final map = _object(value, context);
  _knownKeys(map, const {'card', 'reason'}, context);
  return SubscribeRejectedView(
    card: _hexFixed(_field(map, 'card', 'SubscribeRejectedView.card'), 16, 'SubscribeRejectedView.card'),
    reason: _decodeSubscribeRejectionView(_field(map, 'reason', 'SubscribeRejectedView.reason'), 'SubscribeRejectedView.reason'),
  );
}

SessionKeyView _decodeSessionKeyView(Object? value, String context) {
  final map = _object(value, context);
  _knownKeys(map, const {'card', 'generation'}, context);
  return SessionKeyView(
    card: _hexFixed(_field(map, 'card', 'SessionKeyView.card'), 16, 'SessionKeyView.card'),
    generation: _integer(_field(map, 'generation', 'SessionKeyView.generation'), 4294967295, 'SessionKeyView.generation'),
  );
}

EvidenceProgressView _decodeEvidenceProgressView(Object? value, String context) {
  final map = _object(value, context);
  _knownKeys(map, const {'transferred', 'total'}, context);
  return EvidenceProgressView(
    transferred: _integer(_field(map, 'transferred', 'EvidenceProgressView.transferred'), _u63Max, 'EvidenceProgressView.transferred'),
    total: _integer(_field(map, 'total', 'EvidenceProgressView.total'), _u63Max, 'EvidenceProgressView.total'),
  );
}

RedactedIdView _decodeRedactedIdView(Object? value, String context) {
  final map = _object(value, context);
  _knownKeys(map, const {'kind'}, context);
  return RedactedIdView(
    kind: _decodeRedactedIdKindView(_field(map, 'kind', 'RedactedIdView.kind'), 'RedactedIdView.kind'),
  );
}

EvidenceValueView _decodeEvidenceValueView(Object? value, String context) {
  final map = _object(value, context);
  _knownKeys(map, const {'kind', 'value'}, context);
  final kind = _field(map, 'kind', context);
  if (kind is! String) {
    throw ReadContractException(ReadErrorKind.shape, context);
  }
  switch (kind) {
    case 'phase':
      return EvidenceValueViewPhase(
        _decodePhaseView(_payload(map, 'EvidenceValueView.phase'), 'EvidenceValueView.phase'),
      );
    case 'progress':
      return EvidenceValueViewProgress(
        _decodeEvidenceProgressView(_payload(map, 'EvidenceValueView.progress'), 'EvidenceValueView.progress'),
      );
    case 'outcome':
      return EvidenceValueViewOutcome(
        _decodeOutcomeView(_payload(map, 'EvidenceValueView.outcome'), 'EvidenceValueView.outcome'),
      );
    case 'identifier':
      return EvidenceValueViewIdentifier(
        _decodeRedactedIdView(_payload(map, 'EvidenceValueView.identifier'), 'EvidenceValueView.identifier'),
      );
    default:
      throw ReadContractException(ReadErrorKind.unknownVariant, context);
  }
}

DegradedView _decodeDegradedView(Object? value, String context) {
  final map = _object(value, context);
  _knownKeys(map, const {'dropped_events'}, context);
  return DegradedView(
    droppedEvents: _integer(_field(map, 'dropped_events', 'DegradedView.dropped_events'), _u63Max, 'DegradedView.dropped_events'),
  );
}

DiagnosticsStatusView _decodeDiagnosticsStatusView(Object? value, String context) {
  final map = _object(value, context);
  _knownKeys(map, const {'kind', 'value'}, context);
  final kind = _field(map, 'kind', context);
  if (kind is! String) {
    throw ReadContractException(ReadErrorKind.shape, context);
  }
  switch (kind) {
    case 'complete':
      _unitPayload(map, 'DiagnosticsStatusView.complete');
      return const DiagnosticsStatusViewComplete();
    case 'degraded':
      return DiagnosticsStatusViewDegraded(
        _decodeDegradedView(_payload(map, 'DiagnosticsStatusView.degraded'), 'DiagnosticsStatusView.degraded'),
      );
    default:
      throw ReadContractException(ReadErrorKind.unknownVariant, context);
  }
}

TimelineEntryView _decodeTimelineEntryView(Object? value, String context) {
  final map = _object(value, context);
  _knownKeys(map, const {'sequence', 'value'}, context);
  return TimelineEntryView(
    sequence: _integer(_field(map, 'sequence', 'TimelineEntryView.sequence'), _u63Max, 'TimelineEntryView.sequence'),
    value: _decodeEvidenceValueView(_field(map, 'value', 'TimelineEntryView.value'), 'TimelineEntryView.value'),
  );
}

EvidenceTimelineView _decodeEvidenceTimelineView(Object? value, String context) {
  final map = _object(value, context);
  _knownKeys(map, const {'session', 'status', 'entries'}, context);
  return EvidenceTimelineView(
    session: _decodeSessionKeyView(_field(map, 'session', 'EvidenceTimelineView.session'), 'EvidenceTimelineView.session'),
    status: _decodeDiagnosticsStatusView(_field(map, 'status', 'EvidenceTimelineView.status'), 'EvidenceTimelineView.status'),
    entries: _list(_field(map, 'entries', 'EvidenceTimelineView.entries'), 1024, 'EvidenceTimelineView.entries', _decodeTimelineEntryView),
  );
}

ProtocolManifestView _decodeProtocolManifestView(Object? value, String context) {
  final map = _object(value, context);
  _knownKeys(map, const {'set_id', 'data_alpn', 'data_magic', 'data_wire_version'}, context);
  return ProtocolManifestView(
    setId: _asciiBounded(_field(map, 'set_id', 'ProtocolManifestView.set_id'), 64, 'ProtocolManifestView.set_id'),
    dataAlpn: _hexVariable(_field(map, 'data_alpn', 'ProtocolManifestView.data_alpn'), 64, 'ProtocolManifestView.data_alpn'),
    dataMagic: _hexVariable(_field(map, 'data_magic', 'ProtocolManifestView.data_magic'), 32, 'ProtocolManifestView.data_magic'),
    dataWireVersion: _integer(_field(map, 'data_wire_version', 'ProtocolManifestView.data_wire_version'), 65535, 'ProtocolManifestView.data_wire_version'),
  );
}

AbiSchemaManifestView _decodeAbiSchemaManifestView(Object? value, String context) {
  final map = _object(value, context);
  _knownKeys(map, const {'read_binding_schema_id', 'command_binding_schema_id', 'evidence_rust_abi_id', 'evidence_timeline_schema_id', 'mailbox_receipt_schema_id', 'operation_envelope_schema_id'}, context);
  return AbiSchemaManifestView(
    readBindingSchemaId: _asciiBounded(_field(map, 'read_binding_schema_id', 'AbiSchemaManifestView.read_binding_schema_id'), 64, 'AbiSchemaManifestView.read_binding_schema_id'),
    commandBindingSchemaId: _asciiBounded(_field(map, 'command_binding_schema_id', 'AbiSchemaManifestView.command_binding_schema_id'), 64, 'AbiSchemaManifestView.command_binding_schema_id'),
    evidenceRustAbiId: _asciiBounded(_field(map, 'evidence_rust_abi_id', 'AbiSchemaManifestView.evidence_rust_abi_id'), 64, 'AbiSchemaManifestView.evidence_rust_abi_id'),
    evidenceTimelineSchemaId: _asciiBounded(_field(map, 'evidence_timeline_schema_id', 'AbiSchemaManifestView.evidence_timeline_schema_id'), 64, 'AbiSchemaManifestView.evidence_timeline_schema_id'),
    mailboxReceiptSchemaId: _asciiBounded(_field(map, 'mailbox_receipt_schema_id', 'AbiSchemaManifestView.mailbox_receipt_schema_id'), 64, 'AbiSchemaManifestView.mailbox_receipt_schema_id'),
    operationEnvelopeSchemaId: _asciiBounded(_field(map, 'operation_envelope_schema_id', 'AbiSchemaManifestView.operation_envelope_schema_id'), 64, 'AbiSchemaManifestView.operation_envelope_schema_id'),
  );
}

TrustRootSha256View _decodeTrustRootSha256View(Object? value, String context) {
  final map = _object(value, context);
  _knownKeys(map, const {'fingerprint'}, context);
  return TrustRootSha256View(
    fingerprint: _hexFixed(_field(map, 'fingerprint', 'TrustRootSha256View.fingerprint'), 64, 'TrustRootSha256View.fingerprint'),
  );
}

TrustRootView _decodeTrustRootView(Object? value, String context) {
  final map = _object(value, context);
  _knownKeys(map, const {'kind', 'value'}, context);
  final kind = _field(map, 'kind', context);
  if (kind is! String) {
    throw ReadContractException(ReadErrorKind.shape, context);
  }
  switch (kind) {
    case 'unprovisioned':
      _unitPayload(map, 'TrustRootView.unprovisioned');
      return const TrustRootViewUnprovisioned();
    case 'sha256':
      return TrustRootViewSha256(
        _decodeTrustRootSha256View(_payload(map, 'TrustRootView.sha256'), 'TrustRootView.sha256'),
      );
    default:
      throw ReadContractException(ReadErrorKind.unknownVariant, context);
  }
}

BuildManifestView _decodeBuildManifestView(Object? value, String context) {
  final map = _object(value, context);
  _knownKeys(map, const {'package_version', 'protocol', 'abi_schema', 'trust_root'}, context);
  return BuildManifestView(
    packageVersion: _asciiBounded(_field(map, 'package_version', 'BuildManifestView.package_version'), 32, 'BuildManifestView.package_version'),
    protocol: _decodeProtocolManifestView(_field(map, 'protocol', 'BuildManifestView.protocol'), 'BuildManifestView.protocol'),
    abiSchema: _decodeAbiSchemaManifestView(_field(map, 'abi_schema', 'BuildManifestView.abi_schema'), 'BuildManifestView.abi_schema'),
    trustRoot: _decodeTrustRootView(_field(map, 'trust_root', 'BuildManifestView.trust_root'), 'BuildManifestView.trust_root'),
  );
}

ReadBody _decodeReadBody(Object? value, String context) {
  final map = _object(value, context);
  _knownKeys(map, const {'kind', 'value'}, context);
  final kind = _field(map, 'kind', context);
  if (kind is! String) {
    throw ReadContractException(ReadErrorKind.shape, context);
  }
  switch (kind) {
    case 'card_update':
      return ReadBodyCardUpdate(
        _decodeCardUpdateView(_payload(map, 'ReadBody.card_update'), 'ReadBody.card_update'),
      );
    case 'lag':
      return ReadBodyLag(
        _decodeLagView(_payload(map, 'ReadBody.lag'), 'ReadBody.lag'),
      );
    case 'closed':
      return ReadBodyClosed(
        _decodeClosedView(_payload(map, 'ReadBody.closed'), 'ReadBody.closed'),
      );
    case 'subscribe_rejected':
      return ReadBodySubscribeRejected(
        _decodeSubscribeRejectedView(_payload(map, 'ReadBody.subscribe_rejected'), 'ReadBody.subscribe_rejected'),
      );
    case 'evidence':
      return ReadBodyEvidence(
        _decodeEvidenceTimelineView(_payload(map, 'ReadBody.evidence'), 'ReadBody.evidence'),
      );
    case 'build_manifest':
      return ReadBodyBuildManifest(
        _decodeBuildManifestView(_payload(map, 'ReadBody.build_manifest'), 'ReadBody.build_manifest'),
      );
    default:
      throw ReadContractException(ReadErrorKind.unknownVariant, context);
  }
}

ReadFrame _decodeReadFrame(Object? value, String context) {
  final map = _object(value, context);
  _knownKeys(map, const {'schema', 'body'}, context);
  return ReadFrame(
    body: _decodeReadBody(_field(map, 'body', 'ReadFrame.body'), 'ReadFrame.body'),
  );
}
