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

const String capabilitySchemaId = 'envoix/binding/capability/1';
const int capabilityMaxFrameBytes = 65536;
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

enum CapabilityRequestView {
  scanInvite,
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

sealed class CapabilityStepView {
  const CapabilityStepView();
}

final class CapabilityStepViewRequested extends CapabilityStepView {
  const CapabilityStepViewRequested();
}

final class CapabilityStepViewProvided extends CapabilityStepView {
  const CapabilityStepViewProvided(this.value);

  final ScannedTextView value;
}

final class CapabilityStepViewDeclined extends CapabilityStepView {
  const CapabilityStepViewDeclined(this.value);

  final DeclinedReasonView value;
}

final class CapabilityExchangeView {
  const CapabilityExchangeView({
    required this.capability,
    required this.step,
  });

  final CapabilityRequestView capability;
  final CapabilityStepView step;
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

String _encodeUtf8Bounded(String value, int maxBytes, String context) =>
    _utf8Bounded(value, maxBytes, context);

CapabilityRequestView _decodeCapabilityRequestView(Object? value, String context) {
  return switch (value) {
    'scan_invite' => CapabilityRequestView.scanInvite,
    String() =>
      throw CapabilityContractException(CapabilityErrorKind.unknownVariant, context),
    _ => throw CapabilityContractException(CapabilityErrorKind.shape, context),
  };
}

String _encodeCapabilityRequestView(CapabilityRequestView value) {
  return switch (value) {
    CapabilityRequestView.scanInvite => 'scan_invite',
  };
}

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

CapabilityStepView _decodeCapabilityStepView(Object? value, String context) {
  final map = _object(value, context);
  _knownKeys(map, const {'kind', 'value'}, context);
  final kind = _field(map, 'kind', context);
  if (kind is! String) {
    throw CapabilityContractException(CapabilityErrorKind.shape, context);
  }
  switch (kind) {
    case 'requested':
      _unitPayload(map, 'CapabilityStepView.requested');
      return const CapabilityStepViewRequested();
    case 'provided':
      return CapabilityStepViewProvided(
        _decodeScannedTextView(_payload(map, 'CapabilityStepView.provided'), 'CapabilityStepView.provided'),
      );
    case 'declined':
      return CapabilityStepViewDeclined(
        _decodeDeclinedReasonView(_payload(map, 'CapabilityStepView.declined'), 'CapabilityStepView.declined'),
      );
    default:
      throw CapabilityContractException(CapabilityErrorKind.unknownVariant, context);
  }
}

Map<String, Object?> _encodeCapabilityStepView(CapabilityStepView value) {
  return switch (value) {
    CapabilityStepViewRequested() => <String, Object?>{'kind': 'requested'},
    CapabilityStepViewProvided(value: final payload) => <String, Object?>{
        'kind': 'provided',
        'value': _encodeScannedTextView(payload),
      },
    CapabilityStepViewDeclined(value: final payload) => <String, Object?>{
        'kind': 'declined',
        'value': _encodeDeclinedReasonView(payload),
      },
  };
}

CapabilityExchangeView _decodeCapabilityExchangeView(Object? value, String context) {
  final map = _object(value, context);
  _knownKeys(map, const {'capability', 'step'}, context);
  return CapabilityExchangeView(
    capability: _decodeCapabilityRequestView(_field(map, 'capability', 'CapabilityExchangeView.capability'), 'CapabilityExchangeView.capability'),
    step: _decodeCapabilityStepView(_field(map, 'step', 'CapabilityExchangeView.step'), 'CapabilityExchangeView.step'),
  );
}

Map<String, Object?> _encodeCapabilityExchangeView(CapabilityExchangeView value) {
  return <String, Object?>{
    'capability': _encodeCapabilityRequestView(value.capability),
    'step': _encodeCapabilityStepView(value.step),
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
