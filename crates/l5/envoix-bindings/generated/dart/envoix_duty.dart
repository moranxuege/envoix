// @generated from schema/duty.schema by envoix-bindings. Do not edit;
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

const String dutySchemaId = 'envoix/binding/duty/1';
const int dutyMaxFrameBytes = 4096;
const int _u63Max = 9223372036854775807;

enum DutyErrorKind {
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
final class DutyContractException implements Exception {
  const DutyContractException(this.kind, this.context);

  final DutyErrorKind kind;
  final String context;

  @override
  String toString() => 'DutyContractException(${kind.name}, $context)';
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

enum NoticeView {
  transferComplete,
  transferFailed,
  actionNeeded,
}

enum LockDirectiveView {
  hold,
  release,
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

final class PublicationWorkView {
  const PublicationWorkView({
    required this.staged,
    required this.displayName,
    required this.totalBytes,
  });

  final String staged;
  final String displayName;
  final int totalBytes;
}

final class ForegroundWorkView {
  const ForegroundWorkView({
    required this.activeTransfers,
  });

  final int activeTransfers;
}

final class NotificationWorkView {
  const NotificationWorkView({
    required this.notice,
  });

  final NoticeView notice;
}

final class LockWorkView {
  const LockWorkView({
    required this.directive,
  });

  final LockDirectiveView directive;
}

sealed class WorkView {
  const WorkView();
}

final class WorkViewSourceHandle extends WorkView {
  const WorkViewSourceHandle();
}

final class WorkViewGrant extends WorkView {
  const WorkViewGrant();
}

final class WorkViewStaging extends WorkView {
  const WorkViewStaging();
}

final class WorkViewPublication extends WorkView {
  const WorkViewPublication(this.value);

  final PublicationWorkView value;
}

final class WorkViewCourier extends WorkView {
  const WorkViewCourier();
}

final class WorkViewForeground extends WorkView {
  const WorkViewForeground(this.value);

  final ForegroundWorkView value;
}

final class WorkViewNotification extends WorkView {
  const WorkViewNotification(this.value);

  final NotificationWorkView value;
}

final class WorkViewLock extends WorkView {
  const WorkViewLock(this.value);

  final LockWorkView value;
}

final class WorkViewOpenShare extends WorkView {
  const WorkViewOpenShare();
}

final class DutyOrderView {
  const DutyOrderView({
    required this.provenance,
    required this.work,
  });

  final DutyProvenanceView provenance;
  final WorkView work;
}

final class DutyReportView {
  const DutyReportView({
    required this.provenance,
    required this.outcome,
  });

  final DutyProvenanceView provenance;
  final OutcomeCodeView outcome;
}

sealed class DutyBody {
  const DutyBody();
}

final class DutyBodyOrder extends DutyBody {
  const DutyBodyOrder(this.value);

  final DutyOrderView value;
}

final class DutyBodyReport extends DutyBody {
  const DutyBodyReport(this.value);

  final DutyReportView value;
}

final class DutyFrame {
  const DutyFrame({
    required this.body,
  });

  final DutyBody body;
}

/// Decodes and validates one frame. Every failure is a typed
/// [DutyContractException]; no input, however hostile, misparses.
DutyFrame decodeDutyFrame(String text) {
  if (utf8.encode(text).length > dutyMaxFrameBytes) {
    throw const DutyContractException(DutyErrorKind.frameTooLarge, 'DutyFrame');
  }
  final Object? value;
  try {
    value = jsonDecode(text);
  } on FormatException {
    throw const DutyContractException(DutyErrorKind.malformedJson, 'DutyFrame');
  }
  final map = _object(value, 'DutyFrame');
  final schema = map['schema'];
  if (schema is! String) {
    throw const DutyContractException(DutyErrorKind.shape, 'DutyFrame.schema');
  }
  if (schema != dutySchemaId) {
    throw const DutyContractException(DutyErrorKind.unknownSchema, 'DutyFrame');
  }
  return _decodeDutyFrame(value, 'DutyFrame');
}

/// Encodes the one frame a frontend may originate, stamping the schema
/// envelope and the `report` body around it and enforcing every bound
/// [decodeDutyFrame] checks. Every failure is a typed
/// [DutyContractException]; an over-bound frame never leaves the process.
String encodeDutyFrame(DutyReportView body) {
  final text = jsonEncode(<String, Object?>{
    'body': <String, Object?>{
      'kind': 'report',
      'value': _encodeDutyReportView(body),
    },
    'schema': dutySchemaId,
  });
  if (utf8.encode(text).length > dutyMaxFrameBytes) {
    throw const DutyContractException(DutyErrorKind.frameTooLarge, 'DutyFrame');
  }
  return text;
}

Map<String, Object?> _object(Object? value, String context) {
  if (value is! Map<String, Object?>) {
    throw DutyContractException(DutyErrorKind.shape, context);
  }
  return value;
}

void _knownKeys(Map<String, Object?> map, Set<String> allowed, String context) {
  for (final key in map.keys) {
    if (!allowed.contains(key)) {
      throw DutyContractException(DutyErrorKind.unknownField, context);
    }
  }
}

Object? _field(Map<String, Object?> map, String key, String context) {
  if (!map.containsKey(key)) {
    throw DutyContractException(DutyErrorKind.shape, context);
  }
  return map[key];
}

int _integer(Object? value, int max, String context) {
  if (value is! int) {
    throw DutyContractException(DutyErrorKind.shape, context);
  }
  if (value < 0 || value > max) {
    throw DutyContractException(DutyErrorKind.range, context);
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
    throw DutyContractException(DutyErrorKind.shape, context);
  }
  if (value.length != chars || !_hexChars(value)) {
    throw DutyContractException(DutyErrorKind.bound, context);
  }
  return value;
}

String _utf8Bounded(Object? value, int maxBytes, String context) {
  if (value is! String) {
    throw DutyContractException(DutyErrorKind.shape, context);
  }
  // Unpaired surrogates parse here but not in the Rust reference codec;
  // reject them so every language accepts the same strings.
  var index = 0;
  while (index < value.length) {
    final unit = value.codeUnitAt(index);
    if (unit >= 0xd800 && unit <= 0xdbff) {
      if (index + 1 == value.length) {
        throw DutyContractException(DutyErrorKind.shape, context);
      }
      final next = value.codeUnitAt(index + 1);
      if (next < 0xdc00 || next > 0xdfff) {
        throw DutyContractException(DutyErrorKind.shape, context);
      }
      index += 2;
    } else if (unit >= 0xdc00 && unit <= 0xdfff) {
      throw DutyContractException(DutyErrorKind.shape, context);
    } else {
      index += 1;
    }
  }
  if (utf8.encode(value).length > maxBytes) {
    throw DutyContractException(DutyErrorKind.bound, context);
  }
  return value;
}

Object? _payload(Map<String, Object?> map, String context) {
  final value = map['value'];
  if (value == null) {
    throw DutyContractException(DutyErrorKind.shape, context);
  }
  return value;
}

void _unitPayload(Map<String, Object?> map, String context) {
  if (map['value'] != null) {
    throw DutyContractException(DutyErrorKind.shape, context);
  }
}

int _encodeInteger(int value, int max, String context) =>
    _integer(value, max, context);

String _encodeHexFixed(String value, int chars, String context) =>
    _hexFixed(value, chars, context);

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
      throw DutyContractException(DutyErrorKind.unknownVariant, context),
    _ => throw DutyContractException(DutyErrorKind.shape, context),
  };
}

String _encodeOutcomeCodeView(OutcomeCodeView value) {
  return switch (value) {
    OutcomeCodeView.completed => 'completed',
    OutcomeCodeView.cancelled => 'cancelled',
    OutcomeCodeView.paused => 'paused',
    OutcomeCodeView.peerLost => 'peer_lost',
    OutcomeCodeView.timeout => 'timeout',
    OutcomeCodeView.unauthenticated => 'unauthenticated',
    OutcomeCodeView.versionMismatch => 'version_mismatch',
    OutcomeCodeView.storageFault => 'storage_fault',
    OutcomeCodeView.publishFailed => 'publish_failed',
    OutcomeCodeView.sourceUnreadable => 'source_unreadable',
    OutcomeCodeView.networkUnreachable => 'network_unreachable',
    OutcomeCodeView.internal => 'internal',
  };
}

NoticeView _decodeNoticeView(Object? value, String context) {
  return switch (value) {
    'transfer_complete' => NoticeView.transferComplete,
    'transfer_failed' => NoticeView.transferFailed,
    'action_needed' => NoticeView.actionNeeded,
    String() =>
      throw DutyContractException(DutyErrorKind.unknownVariant, context),
    _ => throw DutyContractException(DutyErrorKind.shape, context),
  };
}

LockDirectiveView _decodeLockDirectiveView(Object? value, String context) {
  return switch (value) {
    'hold' => LockDirectiveView.hold,
    'release' => LockDirectiveView.release,
    String() =>
      throw DutyContractException(DutyErrorKind.unknownVariant, context),
    _ => throw DutyContractException(DutyErrorKind.shape, context),
  };
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

Map<String, Object?> _encodeDutyProvenanceView(DutyProvenanceView value) {
  return <String, Object?>{
    'card': _encodeHexFixed(value.card, 16, 'DutyProvenanceView.card'),
    'generation': _encodeInteger(value.generation, 4294967295, 'DutyProvenanceView.generation'),
    'request': _encodeHexFixed(value.request, 32, 'DutyProvenanceView.request'),
  };
}

PublicationWorkView _decodePublicationWorkView(Object? value, String context) {
  final map = _object(value, context);
  _knownKeys(map, const {'staged', 'display_name', 'total_bytes'}, context);
  return PublicationWorkView(
    staged: _utf8Bounded(_field(map, 'staged', 'PublicationWorkView.staged'), 512, 'PublicationWorkView.staged'),
    displayName: _utf8Bounded(_field(map, 'display_name', 'PublicationWorkView.display_name'), 255, 'PublicationWorkView.display_name'),
    totalBytes: _integer(_field(map, 'total_bytes', 'PublicationWorkView.total_bytes'), _u63Max, 'PublicationWorkView.total_bytes'),
  );
}

ForegroundWorkView _decodeForegroundWorkView(Object? value, String context) {
  final map = _object(value, context);
  _knownKeys(map, const {'active_transfers'}, context);
  return ForegroundWorkView(
    activeTransfers: _integer(_field(map, 'active_transfers', 'ForegroundWorkView.active_transfers'), 4294967295, 'ForegroundWorkView.active_transfers'),
  );
}

NotificationWorkView _decodeNotificationWorkView(Object? value, String context) {
  final map = _object(value, context);
  _knownKeys(map, const {'notice'}, context);
  return NotificationWorkView(
    notice: _decodeNoticeView(_field(map, 'notice', 'NotificationWorkView.notice'), 'NotificationWorkView.notice'),
  );
}

LockWorkView _decodeLockWorkView(Object? value, String context) {
  final map = _object(value, context);
  _knownKeys(map, const {'directive'}, context);
  return LockWorkView(
    directive: _decodeLockDirectiveView(_field(map, 'directive', 'LockWorkView.directive'), 'LockWorkView.directive'),
  );
}

WorkView _decodeWorkView(Object? value, String context) {
  final map = _object(value, context);
  _knownKeys(map, const {'kind', 'value'}, context);
  final kind = _field(map, 'kind', context);
  if (kind is! String) {
    throw DutyContractException(DutyErrorKind.shape, context);
  }
  switch (kind) {
    case 'source_handle':
      _unitPayload(map, 'WorkView.source_handle');
      return const WorkViewSourceHandle();
    case 'grant':
      _unitPayload(map, 'WorkView.grant');
      return const WorkViewGrant();
    case 'staging':
      _unitPayload(map, 'WorkView.staging');
      return const WorkViewStaging();
    case 'publication':
      return WorkViewPublication(
        _decodePublicationWorkView(_payload(map, 'WorkView.publication'), 'WorkView.publication'),
      );
    case 'courier':
      _unitPayload(map, 'WorkView.courier');
      return const WorkViewCourier();
    case 'foreground':
      return WorkViewForeground(
        _decodeForegroundWorkView(_payload(map, 'WorkView.foreground'), 'WorkView.foreground'),
      );
    case 'notification':
      return WorkViewNotification(
        _decodeNotificationWorkView(_payload(map, 'WorkView.notification'), 'WorkView.notification'),
      );
    case 'lock':
      return WorkViewLock(
        _decodeLockWorkView(_payload(map, 'WorkView.lock'), 'WorkView.lock'),
      );
    case 'open_share':
      _unitPayload(map, 'WorkView.open_share');
      return const WorkViewOpenShare();
    default:
      throw DutyContractException(DutyErrorKind.unknownVariant, context);
  }
}

DutyOrderView _decodeDutyOrderView(Object? value, String context) {
  final map = _object(value, context);
  _knownKeys(map, const {'provenance', 'work'}, context);
  return DutyOrderView(
    provenance: _decodeDutyProvenanceView(_field(map, 'provenance', 'DutyOrderView.provenance'), 'DutyOrderView.provenance'),
    work: _decodeWorkView(_field(map, 'work', 'DutyOrderView.work'), 'DutyOrderView.work'),
  );
}

DutyReportView _decodeDutyReportView(Object? value, String context) {
  final map = _object(value, context);
  _knownKeys(map, const {'provenance', 'outcome'}, context);
  return DutyReportView(
    provenance: _decodeDutyProvenanceView(_field(map, 'provenance', 'DutyReportView.provenance'), 'DutyReportView.provenance'),
    outcome: _decodeOutcomeCodeView(_field(map, 'outcome', 'DutyReportView.outcome'), 'DutyReportView.outcome'),
  );
}

Map<String, Object?> _encodeDutyReportView(DutyReportView value) {
  return <String, Object?>{
    'outcome': _encodeOutcomeCodeView(value.outcome),
    'provenance': _encodeDutyProvenanceView(value.provenance),
  };
}

DutyBody _decodeDutyBody(Object? value, String context) {
  final map = _object(value, context);
  _knownKeys(map, const {'kind', 'value'}, context);
  final kind = _field(map, 'kind', context);
  if (kind is! String) {
    throw DutyContractException(DutyErrorKind.shape, context);
  }
  switch (kind) {
    case 'order':
      return DutyBodyOrder(
        _decodeDutyOrderView(_payload(map, 'DutyBody.order'), 'DutyBody.order'),
      );
    case 'report':
      return DutyBodyReport(
        _decodeDutyReportView(_payload(map, 'DutyBody.report'), 'DutyBody.report'),
      );
    default:
      throw DutyContractException(DutyErrorKind.unknownVariant, context);
  }
}

DutyFrame _decodeDutyFrame(Object? value, String context) {
  final map = _object(value, context);
  _knownKeys(map, const {'schema', 'body'}, context);
  return DutyFrame(
    body: _decodeDutyBody(_field(map, 'body', 'DutyFrame.body'), 'DutyFrame.body'),
  );
}
