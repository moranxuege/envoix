// @generated from schema/capability.schema by envoix-bindings. Do not edit;
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

const String capabilitySchemaId = 'envoix/binding/capability/2';
const int capabilityMaxFrameBytes = 65536;
const int _u63Max = 9223372036854775807;

enum CapabilityErrorKind {
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
final class CapabilityContractException implements Exception {
  const CapabilityContractException(this.kind, this.context);

  final CapabilityErrorKind kind;
  final String context;

  @override
  String toString() => 'CapabilityContractException(${kind.name}, $context)';
}

/// Bounded contract text whose ordinary string representation is always
/// redacted. Rendering the user-visible value requires an explicit
/// [expose] call at the UI boundary.
final class CapabilitySecretString {
  const CapabilitySecretString(this._value);

  final String _value;

  String expose() => _value;

  @override
  String toString() => 'CapabilitySecretString([redacted])';
}

final class ScannedTextView {
  const ScannedTextView({
    required this.text,
  });

  final CapabilitySecretString text;
}

enum DeclinedView {
  cancelled,
  refused,
  unsupported,
}

final class DeclinedReasonView {
  const DeclinedReasonView({
    required this.reason,
  });

  final DeclinedView reason;
}

sealed class ScanInviteStepView {
  const ScanInviteStepView();
}

final class ScanInviteStepViewRequested extends ScanInviteStepView {
  const ScanInviteStepViewRequested();
}

final class ScanInviteStepViewProvided extends ScanInviteStepView {
  const ScanInviteStepViewProvided(this.value);

  final ScannedTextView value;
}

final class ScanInviteStepViewDeclined extends ScanInviteStepView {
  const ScanInviteStepViewDeclined(this.value);

  final DeclinedReasonView value;
}

final class ScanInviteExchangeView {
  const ScanInviteExchangeView({
    required this.step,
  });

  final ScanInviteStepView step;
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

final class PickedSourceView {
  const PickedSourceView({
    required this.displayName,
    required this.reportedSize,
  });

  final String displayName;
  final int? reportedSize;
}

enum PickSourceFailureView {
  pickerUnavailable,
  metadataUnavailable,
  internal,
}

final class PickSourceFailureReasonView {
  const PickSourceFailureReasonView({
    required this.reason,
  });

  final PickSourceFailureView reason;
}

sealed class PickSourceStepView {
  const PickSourceStepView();
}

final class PickSourceStepViewRequested extends PickSourceStepView {
  const PickSourceStepViewRequested();
}

final class PickSourceStepViewProvided extends PickSourceStepView {
  const PickSourceStepViewProvided(this.value);

  final PickedSourceView value;
}

final class PickSourceStepViewDeclined extends PickSourceStepView {
  const PickSourceStepViewDeclined(this.value);

  final DeclinedReasonView value;
}

final class PickSourceStepViewFailed extends PickSourceStepView {
  const PickSourceStepViewFailed(this.value);

  final PickSourceFailureReasonView value;
}

final class PickSourceExchangeView {
  const PickSourceExchangeView({
    required this.acquisition,
    required this.step,
  });

  final SourceAcquisitionKeyView acquisition;
  final PickSourceStepView step;
}

sealed class CapabilityExchangeView {
  const CapabilityExchangeView();
}

final class CapabilityExchangeViewScanInvite extends CapabilityExchangeView {
  const CapabilityExchangeViewScanInvite(this.value);

  final ScanInviteExchangeView value;
}

final class CapabilityExchangeViewPickSource extends CapabilityExchangeView {
  const CapabilityExchangeViewPickSource(this.value);

  final PickSourceExchangeView value;
}

sealed class CapabilityBody {
  const CapabilityBody();
}

final class CapabilityBodyExchange extends CapabilityBody {
  const CapabilityBodyExchange(this.value);

  final CapabilityExchangeView value;
}

final class CapabilityFrame {
  const CapabilityFrame({
    required this.body,
  });

  final CapabilityBody body;
}

/// Decodes and validates one frame. Every failure is a typed
/// [CapabilityContractException]; no input, however hostile, misparses.
CapabilityFrame decodeCapabilityFrame(String text) {
  if (utf8.encode(text).length > capabilityMaxFrameBytes) {
    throw const CapabilityContractException(CapabilityErrorKind.frameTooLarge, 'CapabilityFrame');
  }
  final Object? value;
  try {
    value = jsonDecode(text);
  } on FormatException {
    throw const CapabilityContractException(CapabilityErrorKind.malformedJson, 'CapabilityFrame');
  }
  final map = _object(value, 'CapabilityFrame');
  final schema = map['schema'];
  if (schema is! String) {
    throw const CapabilityContractException(CapabilityErrorKind.shape, 'CapabilityFrame.schema');
  }
  if (schema != capabilitySchemaId) {
    throw const CapabilityContractException(CapabilityErrorKind.unknownSchema, 'CapabilityFrame');
  }
  return _decodeCapabilityFrame(value, 'CapabilityFrame');
}

/// Encodes the one frame a frontend may originate, stamping the schema
/// envelope and the `exchange` body around it and enforcing every bound
/// [decodeCapabilityFrame] checks. Every failure is a typed
/// [CapabilityContractException]; an over-bound frame never leaves the process.
String encodeCapabilityFrame(CapabilityExchangeView body) {
  final text = jsonEncode(<String, Object?>{
    'body': <String, Object?>{
      'kind': 'exchange',
      'value': _encodeCapabilityExchangeView(body),
    },
    'schema': capabilitySchemaId,
  });
  if (utf8.encode(text).length > capabilityMaxFrameBytes) {
    throw const CapabilityContractException(CapabilityErrorKind.frameTooLarge, 'CapabilityFrame');
  }
  return text;
}

Map<String, Object?> _object(Object? value, String context) {
  if (value is! Map<String, Object?>) {
    throw CapabilityContractException(CapabilityErrorKind.shape, context);
  }
  return value;
}

void _knownKeys(Map<String, Object?> map, Set<String> allowed, String context) {
  for (final key in map.keys) {
    if (!allowed.contains(key)) {
      throw CapabilityContractException(CapabilityErrorKind.unknownField, context);
    }
  }
}

Object? _field(Map<String, Object?> map, String key, String context) {
  if (!map.containsKey(key)) {
    throw CapabilityContractException(CapabilityErrorKind.shape, context);
  }
  return map[key];
}

int _integer(Object? value, int max, String context) {
  if (value is! int) {
    throw CapabilityContractException(CapabilityErrorKind.shape, context);
  }
  if (value < 0 || value > max) {
    throw CapabilityContractException(CapabilityErrorKind.range, context);
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
    throw CapabilityContractException(CapabilityErrorKind.shape, context);
  }
  if (value.length != chars || !_hexChars(value)) {
    throw CapabilityContractException(CapabilityErrorKind.bound, context);
  }
  return value;
}

String _utf8Bounded(Object? value, int maxBytes, String context) {
  if (value is! String) {
    throw CapabilityContractException(CapabilityErrorKind.shape, context);
  }
  // Unpaired surrogates parse here but not in the Rust reference codec;
  // reject them so every language accepts the same strings.
  var index = 0;
  while (index < value.length) {
    final unit = value.codeUnitAt(index);
    if (unit >= 0xd800 && unit <= 0xdbff) {
      if (index + 1 == value.length) {
        throw CapabilityContractException(CapabilityErrorKind.shape, context);
      }
      final next = value.codeUnitAt(index + 1);
      if (next < 0xdc00 || next > 0xdfff) {
        throw CapabilityContractException(CapabilityErrorKind.shape, context);
      }
      index += 2;
    } else if (unit >= 0xdc00 && unit <= 0xdfff) {
      throw CapabilityContractException(CapabilityErrorKind.shape, context);
    } else {
      index += 1;
    }
  }
  if (utf8.encode(value).length > maxBytes) {
    throw CapabilityContractException(CapabilityErrorKind.bound, context);
  }
  return value;
}

Object? _payload(Map<String, Object?> map, String context) {
  final value = map['value'];
  if (value == null) {
    throw CapabilityContractException(CapabilityErrorKind.shape, context);
  }
  return value;
}

void _unitPayload(Map<String, Object?> map, String context) {
  if (map['value'] != null) {
    throw CapabilityContractException(CapabilityErrorKind.shape, context);
  }
}

int _encodeInteger(int value, int max, String context) =>
    _integer(value, max, context);

String _encodeHexFixed(String value, int chars, String context) =>
    _hexFixed(value, chars, context);

String _encodeUtf8Bounded(String value, int maxBytes, String context) =>
    _utf8Bounded(value, maxBytes, context);

ScannedTextView _decodeScannedTextView(Object? value, String context) {
  final map = _object(value, context);
  _knownKeys(map, const {'text'}, context);
  return ScannedTextView(
    text: CapabilitySecretString(_utf8Bounded(_field(map, 'text', 'ScannedTextView.text'), 16384, 'ScannedTextView.text')),
  );
}

Map<String, Object?> _encodeScannedTextView(ScannedTextView value) {
  return <String, Object?>{
    'text': _encodeUtf8Bounded(value.text.expose(), 16384, 'ScannedTextView.text'),
  };
}

DeclinedView _decodeDeclinedView(Object? value, String context) {
  return switch (value) {
    'cancelled' => DeclinedView.cancelled,
    'refused' => DeclinedView.refused,
    'unsupported' => DeclinedView.unsupported,
    String() =>
      throw CapabilityContractException(CapabilityErrorKind.unknownVariant, context),
    _ => throw CapabilityContractException(CapabilityErrorKind.shape, context),
  };
}

String _encodeDeclinedView(DeclinedView value) {
  return switch (value) {
    DeclinedView.cancelled => 'cancelled',
    DeclinedView.refused => 'refused',
    DeclinedView.unsupported => 'unsupported',
  };
}

DeclinedReasonView _decodeDeclinedReasonView(Object? value, String context) {
  final map = _object(value, context);
  _knownKeys(map, const {'reason'}, context);
  return DeclinedReasonView(
    reason: _decodeDeclinedView(_field(map, 'reason', 'DeclinedReasonView.reason'), 'DeclinedReasonView.reason'),
  );
}

Map<String, Object?> _encodeDeclinedReasonView(DeclinedReasonView value) {
  return <String, Object?>{
    'reason': _encodeDeclinedView(value.reason),
  };
}

ScanInviteStepView _decodeScanInviteStepView(Object? value, String context) {
  final map = _object(value, context);
  _knownKeys(map, const {'kind', 'value'}, context);
  final kind = _field(map, 'kind', context);
  if (kind is! String) {
    throw CapabilityContractException(CapabilityErrorKind.shape, context);
  }
  switch (kind) {
    case 'requested':
      _unitPayload(map, 'ScanInviteStepView.requested');
      return const ScanInviteStepViewRequested();
    case 'provided':
      return ScanInviteStepViewProvided(
        _decodeScannedTextView(_payload(map, 'ScanInviteStepView.provided'), 'ScanInviteStepView.provided'),
      );
    case 'declined':
      return ScanInviteStepViewDeclined(
        _decodeDeclinedReasonView(_payload(map, 'ScanInviteStepView.declined'), 'ScanInviteStepView.declined'),
      );
    default:
      throw CapabilityContractException(CapabilityErrorKind.unknownVariant, context);
  }
}

Map<String, Object?> _encodeScanInviteStepView(ScanInviteStepView value) {
  return switch (value) {
    ScanInviteStepViewRequested() => <String, Object?>{'kind': 'requested'},
    ScanInviteStepViewProvided(value: final payload) => <String, Object?>{
        'kind': 'provided',
        'value': _encodeScannedTextView(payload),
      },
    ScanInviteStepViewDeclined(value: final payload) => <String, Object?>{
        'kind': 'declined',
        'value': _encodeDeclinedReasonView(payload),
      },
  };
}

ScanInviteExchangeView _decodeScanInviteExchangeView(Object? value, String context) {
  final map = _object(value, context);
  _knownKeys(map, const {'step'}, context);
  return ScanInviteExchangeView(
    step: _decodeScanInviteStepView(_field(map, 'step', 'ScanInviteExchangeView.step'), 'ScanInviteExchangeView.step'),
  );
}

Map<String, Object?> _encodeScanInviteExchangeView(ScanInviteExchangeView value) {
  return <String, Object?>{
    'step': _encodeScanInviteStepView(value.step),
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

PickedSourceView _decodePickedSourceView(Object? value, String context) {
  final map = _object(value, context);
  _knownKeys(map, const {'display_name', 'reported_size'}, context);
  return PickedSourceView(
    displayName: _utf8Bounded(_field(map, 'display_name', 'PickedSourceView.display_name'), 1020, 'PickedSourceView.display_name'),
    reportedSize: switch (_field(map, 'reported_size', 'PickedSourceView.reported_size')) {
      null => null,
      final present => _integer(present, _u63Max, 'PickedSourceView.reported_size'),
    },
  );
}

Map<String, Object?> _encodePickedSourceView(PickedSourceView value) {
  return <String, Object?>{
    'display_name': _encodeUtf8Bounded(value.displayName, 1020, 'PickedSourceView.display_name'),
    'reported_size': value.reportedSize == null ? null : _encodeInteger(value.reportedSize!, _u63Max, 'PickedSourceView.reported_size'),
  };
}

PickSourceFailureView _decodePickSourceFailureView(Object? value, String context) {
  return switch (value) {
    'picker_unavailable' => PickSourceFailureView.pickerUnavailable,
    'metadata_unavailable' => PickSourceFailureView.metadataUnavailable,
    'internal' => PickSourceFailureView.internal,
    String() =>
      throw CapabilityContractException(CapabilityErrorKind.unknownVariant, context),
    _ => throw CapabilityContractException(CapabilityErrorKind.shape, context),
  };
}

String _encodePickSourceFailureView(PickSourceFailureView value) {
  return switch (value) {
    PickSourceFailureView.pickerUnavailable => 'picker_unavailable',
    PickSourceFailureView.metadataUnavailable => 'metadata_unavailable',
    PickSourceFailureView.internal => 'internal',
  };
}

PickSourceFailureReasonView _decodePickSourceFailureReasonView(Object? value, String context) {
  final map = _object(value, context);
  _knownKeys(map, const {'reason'}, context);
  return PickSourceFailureReasonView(
    reason: _decodePickSourceFailureView(_field(map, 'reason', 'PickSourceFailureReasonView.reason'), 'PickSourceFailureReasonView.reason'),
  );
}

Map<String, Object?> _encodePickSourceFailureReasonView(PickSourceFailureReasonView value) {
  return <String, Object?>{
    'reason': _encodePickSourceFailureView(value.reason),
  };
}

PickSourceStepView _decodePickSourceStepView(Object? value, String context) {
  final map = _object(value, context);
  _knownKeys(map, const {'kind', 'value'}, context);
  final kind = _field(map, 'kind', context);
  if (kind is! String) {
    throw CapabilityContractException(CapabilityErrorKind.shape, context);
  }
  switch (kind) {
    case 'requested':
      _unitPayload(map, 'PickSourceStepView.requested');
      return const PickSourceStepViewRequested();
    case 'provided':
      return PickSourceStepViewProvided(
        _decodePickedSourceView(_payload(map, 'PickSourceStepView.provided'), 'PickSourceStepView.provided'),
      );
    case 'declined':
      return PickSourceStepViewDeclined(
        _decodeDeclinedReasonView(_payload(map, 'PickSourceStepView.declined'), 'PickSourceStepView.declined'),
      );
    case 'failed':
      return PickSourceStepViewFailed(
        _decodePickSourceFailureReasonView(_payload(map, 'PickSourceStepView.failed'), 'PickSourceStepView.failed'),
      );
    default:
      throw CapabilityContractException(CapabilityErrorKind.unknownVariant, context);
  }
}

Map<String, Object?> _encodePickSourceStepView(PickSourceStepView value) {
  return switch (value) {
    PickSourceStepViewRequested() => <String, Object?>{'kind': 'requested'},
    PickSourceStepViewProvided(value: final payload) => <String, Object?>{
        'kind': 'provided',
        'value': _encodePickedSourceView(payload),
      },
    PickSourceStepViewDeclined(value: final payload) => <String, Object?>{
        'kind': 'declined',
        'value': _encodeDeclinedReasonView(payload),
      },
    PickSourceStepViewFailed(value: final payload) => <String, Object?>{
        'kind': 'failed',
        'value': _encodePickSourceFailureReasonView(payload),
      },
  };
}

PickSourceExchangeView _decodePickSourceExchangeView(Object? value, String context) {
  final map = _object(value, context);
  _knownKeys(map, const {'acquisition', 'step'}, context);
  return PickSourceExchangeView(
    acquisition: _decodeSourceAcquisitionKeyView(_field(map, 'acquisition', 'PickSourceExchangeView.acquisition'), 'PickSourceExchangeView.acquisition'),
    step: _decodePickSourceStepView(_field(map, 'step', 'PickSourceExchangeView.step'), 'PickSourceExchangeView.step'),
  );
}

Map<String, Object?> _encodePickSourceExchangeView(PickSourceExchangeView value) {
  return <String, Object?>{
    'acquisition': _encodeSourceAcquisitionKeyView(value.acquisition),
    'step': _encodePickSourceStepView(value.step),
  };
}

CapabilityExchangeView _decodeCapabilityExchangeView(Object? value, String context) {
  final map = _object(value, context);
  _knownKeys(map, const {'kind', 'value'}, context);
  final kind = _field(map, 'kind', context);
  if (kind is! String) {
    throw CapabilityContractException(CapabilityErrorKind.shape, context);
  }
  switch (kind) {
    case 'scan_invite':
      return CapabilityExchangeViewScanInvite(
        _decodeScanInviteExchangeView(_payload(map, 'CapabilityExchangeView.scan_invite'), 'CapabilityExchangeView.scan_invite'),
      );
    case 'pick_source':
      return CapabilityExchangeViewPickSource(
        _decodePickSourceExchangeView(_payload(map, 'CapabilityExchangeView.pick_source'), 'CapabilityExchangeView.pick_source'),
      );
    default:
      throw CapabilityContractException(CapabilityErrorKind.unknownVariant, context);
  }
}

Map<String, Object?> _encodeCapabilityExchangeView(CapabilityExchangeView value) {
  return switch (value) {
    CapabilityExchangeViewScanInvite(value: final payload) => <String, Object?>{
        'kind': 'scan_invite',
        'value': _encodeScanInviteExchangeView(payload),
      },
    CapabilityExchangeViewPickSource(value: final payload) => <String, Object?>{
        'kind': 'pick_source',
        'value': _encodePickSourceExchangeView(payload),
      },
  };
}

CapabilityBody _decodeCapabilityBody(Object? value, String context) {
  final map = _object(value, context);
  _knownKeys(map, const {'kind', 'value'}, context);
  final kind = _field(map, 'kind', context);
  if (kind is! String) {
    throw CapabilityContractException(CapabilityErrorKind.shape, context);
  }
  switch (kind) {
    case 'exchange':
      return CapabilityBodyExchange(
        _decodeCapabilityExchangeView(_payload(map, 'CapabilityBody.exchange'), 'CapabilityBody.exchange'),
      );
    default:
      throw CapabilityContractException(CapabilityErrorKind.unknownVariant, context);
  }
}

CapabilityFrame _decodeCapabilityFrame(Object? value, String context) {
  final map = _object(value, context);
  _knownKeys(map, const {'schema', 'body'}, context);
  return CapabilityFrame(
    body: _decodeCapabilityBody(_field(map, 'body', 'CapabilityFrame.body'), 'CapabilityFrame.body'),
  );
}
