//! Emits the generated Dart read bindings (`generated/dart/envoix_read.dart`).
//!
//! The decoder walks `jsonDecode` output with the same shape/range/bound
//! checks as the Rust reference codec and throws `ReadContractException` with a
//! typed reason and static context. On a host-to-frontend contract that is all
//! Dart gets — an observer cannot fabricate an observation. A bidirectional
//! contract additionally gets an encoder for the one body its frontends may
//! originate, and every encode helper is the decode predicate itself, so the
//! two halves cannot check different things.

use crate::model::{Decl, FieldDecl, FieldTy, RuleValue, SchemaDoc, StructDecl, UnionDecl};

use super::{
    dart_member, encodable_decls, encode_helper_use, helper_use, is_envelope_field,
    scalar_predicate, upper_camel,
};

/// How Dart names 2^63-1 in the generated artifact.
const U63_MAX: &str = "_u63Max";

pub fn module(doc: &SchemaDoc) -> String {
    let mut out = String::new();
    out.push_str("// @generated from schema/read.schema by envoix-bindings. Do not edit;\n");
    out.push_str(
        "// regenerate with `ENVOIX_BINDINGS_REGEN=1 cargo test -p envoix-bindings generated_artifacts`.\n",
    );
    out.push_str(
        "// Known platform caveat: JSON `-0` decodes as integer 0 here while the Rust\n\
         // reference codec rejects it (benign: every field with a positive minimum\n\
         // still fails its range check).\n",
    );
    if doc.direction.natives_encode() {
        out.push_str(
            "// Encoded frames are byte-identical to the Rust reference codec's: object\n\
             // keys are emitted in the sorted order serde_json serializes and `jsonEncode`\n\
             // keeps insertion order. On the JavaScript backend `int` is a double, so\n\
             // values above 2^53 lose precision before the encoder ever sees them; this\n\
             // contract is for the Dart VM (Flutter mobile/desktop).\n",
        );
    }
    out.push('\n');
    out.push_str("import 'dart:convert';\n\n");
    out.push_str(&format!("const String readSchemaId = '{}';\n", doc.id));
    out.push_str(&format!(
        "const int readMaxFrameBytes = {};\n",
        doc.max_frame_bytes
    ));
    rules_consts(&mut out, doc);
    out.push_str("const int _u63Max = 9223372036854775807;\n\n");
    error_type(&mut out);
    for decl in &doc.decls {
        type_decl(&mut out, doc, decl);
    }
    if super::rust::supports_epoch_gate(doc) {
        epoch_gate(&mut out);
    }
    entry_point(&mut out, doc);
    helpers(&mut out, doc);
    let encodable = encodable_decls(doc);
    for decl in &doc.decls {
        decode_fn(&mut out, doc, decl);
        if encodable.contains(&decl.name()) {
            encode_fn(&mut out, decl);
        }
    }
    if out.ends_with("\n\n") {
        out.pop();
    }
    super::apply_naming(out, doc)
}

fn rules_consts(out: &mut String, doc: &SchemaDoc) {
    if doc.rules.is_empty() {
        return;
    }
    out.push_str("// Contract rules frozen by schema/read.schema.\n");
    for (key, value) in &doc.rules {
        match value {
            RuleValue::Bool(flag) => out.push_str(&format!(
                "const bool {} = {flag};\n",
                super::lower_camel(key)
            )),
            RuleValue::Int(bound) => out.push_str(&format!(
                "const int {} = {bound};\n",
                super::lower_camel(key)
            )),
        }
    }
}

fn error_type(out: &mut String) {
    out.push_str(
        "enum ReadErrorKind {\n\
         \x20 frameTooLarge,\n\
         \x20 malformedJson,\n\
         \x20 unknownSchema,\n\
         \x20 shape,\n\
         \x20 unknownField,\n\
         \x20 unknownVariant,\n\
         \x20 range,\n\
         \x20 bound,\n\
         }\n\n\
         /// Typed codec failure carrying only static schema context.\n\
         final class ReadContractException implements Exception {\n\
         \x20 const ReadContractException(this.kind, this.context);\n\n\
         \x20 final ReadErrorKind kind;\n\
         \x20 final String context;\n\n\
         \x20 @override\n\
         \x20 String toString() => 'ReadContractException(${kind.name}, $context)';\n\
         }\n\n",
    );
}

fn dart_ty(ty: &FieldTy) -> String {
    match ty {
        FieldTy::U16 | FieldTy::U32 | FieldTy::U63 => "int".to_owned(),
        FieldTy::Hex16
        | FieldTy::Hex32
        | FieldTy::Hex64
        | FieldTy::HexVar { .. }
        | FieldTy::Str { .. }
        | FieldTy::Ascii { .. } => "String".to_owned(),
        FieldTy::Named(name) => name.clone(),
        FieldTy::Option(inner) => format!("{}?", dart_ty(inner)),
        FieldTy::List { element, .. } => format!("List<{}>", dart_ty(element)),
    }
}

fn type_decl(out: &mut String, doc: &SchemaDoc, decl: &Decl) {
    match decl {
        Decl::Enum(decl) => {
            out.push_str(&format!("enum {} {{\n", decl.name));
            for variant in &decl.variants {
                out.push_str(&format!("  {},\n", dart_member(variant)));
            }
            out.push_str("}\n\n");
        }
        Decl::Struct(decl) => {
            out.push_str(&format!("final class {} {{\n", decl.name));
            out.push_str(&format!("  const {}({{\n", decl.name));
            for field in &decl.fields {
                if is_envelope_field(doc, &decl.name, &field.name) {
                    continue;
                }
                out.push_str(&format!(
                    "    required this.{},\n",
                    dart_member(&field.name)
                ));
            }
            out.push_str("  });\n\n");
            for field in &decl.fields {
                if is_envelope_field(doc, &decl.name, &field.name) {
                    continue;
                }
                out.push_str(&format!(
                    "  final {} {};\n",
                    dart_ty(&field.ty),
                    dart_member(&field.name)
                ));
            }
            out.push_str("}\n\n");
        }
        Decl::Union(decl) => {
            out.push_str(&format!(
                "sealed class {} {{\n  const {}();\n}}\n\n",
                decl.name, decl.name
            ));
            for variant in &decl.variants {
                let class = format!("{}{}", decl.name, upper_camel(&variant.name));
                match &variant.payload {
                    Some(payload) => {
                        out.push_str(&format!(
                            "final class {class} extends {} {{\n\
                             \x20 const {class}(this.value);\n\n\
                             \x20 final {payload} value;\n\
                             }}\n\n",
                            decl.name
                        ));
                    }
                    None => {
                        out.push_str(&format!(
                            "final class {class} extends {} {{\n\
                             \x20 const {class}();\n\
                             }}\n\n",
                            decl.name
                        ));
                    }
                }
            }
        }
    }
}

fn epoch_gate(out: &mut String) {
    out.push_str(
        "enum GateDecision { deliver, dropStale, contractBreach }\n\n\
         /// Client-side admission for the per-epoch card stream: one gate per\n\
         /// attachment. Frames from another epoch are stale; every epoch starts\n\
         /// with a snapshot; a lag or close ends the epoch permanently.\n\
         final class EpochGate {\n\
         \x20 EpochGate.attach(this._epoch);\n\n\
         \x20 final int _epoch;\n\
         \x20 bool _sawSnapshot = false;\n\
         \x20 bool _dead = false;\n\n\
         \x20 GateDecision admit(ReadFrame frame) {\n\
         \x20   switch (frame.body) {\n\
         \x20     case final ReadBodyCardUpdate body:\n\
         \x20       final update = body.value;\n\
         \x20       if (update.epoch != _epoch || _dead) {\n\
         \x20         return GateDecision.dropStale;\n\
         \x20       }\n\
         \x20       if (update.kind is CardUpdateKindViewSnapshot) {\n\
         \x20         if (_sawSnapshot) {\n\
         \x20           return GateDecision.contractBreach;\n\
         \x20         }\n\
         \x20         _sawSnapshot = true;\n\
         \x20         return GateDecision.deliver;\n\
         \x20       }\n\
         \x20       return _sawSnapshot\n\
         \x20           ? GateDecision.deliver\n\
         \x20           : GateDecision.contractBreach;\n\
         \x20     case final ReadBodyLag body:\n\
         \x20       return _terminate(body.value.epoch);\n\
         \x20     case final ReadBodyClosed body:\n\
         \x20       return _terminate(body.value.epoch);\n\
         \x20     default:\n\
         \x20       return GateDecision.deliver;\n\
         \x20   }\n\
         \x20 }\n\n\
         \x20 GateDecision _terminate(int epoch) {\n\
         \x20   if (epoch == _epoch && !_dead) {\n\
         \x20     _dead = true;\n\
         \x20     return GateDecision.deliver;\n\
         \x20   }\n\
         \x20   return GateDecision.dropStale;\n\
         \x20 }\n\
         }\n\n",
    );
}

fn entry_point(out: &mut String, doc: &SchemaDoc) {
    out.push_str(&format!(
        "/// Decodes and validates one frame. Every failure is a typed\n\
         /// [ReadContractException]; no input, however hostile, misparses.\n\
         {root} decode{root}(String text) {{\n\
         \x20 if (utf8.encode(text).length > readMaxFrameBytes) {{\n\
         \x20   throw const ReadContractException(ReadErrorKind.frameTooLarge, '{root}');\n\
         \x20 }}\n\
         \x20 final Object? value;\n\
         \x20 try {{\n\
         \x20   value = jsonDecode(text);\n\
         \x20 }} on FormatException {{\n\
         \x20   throw const ReadContractException(ReadErrorKind.malformedJson, '{root}');\n\
         \x20 }}\n\
         \x20 final map = _object(value, '{root}');\n\
         \x20 final schema = map['schema'];\n\
         \x20 if (schema is! String) {{\n\
         \x20   throw const ReadContractException(ReadErrorKind.shape, '{root}.schema');\n\
         \x20 }}\n\
         \x20 if (schema != readSchemaId) {{\n\
         \x20   throw const ReadContractException(ReadErrorKind.unknownSchema, '{root}');\n\
         \x20 }}\n\
         \x20 return _decode{root}(value, '{root}');\n\
         }}\n\n",
        root = doc.root,
    ));
    let Some(body) = doc.frontend_body() else {
        return;
    };
    let mut keys = [
        ("schema".to_owned(), "readSchemaId".to_owned()),
        (
            body.field.to_owned(),
            format!(
                "<String, Object?>{{\n      'kind': '{}',\n      'value': _encode{}(body),\n    }}",
                body.variant, body.payload
            ),
        ),
    ];
    keys.sort_by(|left, right| left.0.cmp(&right.0));
    let literal = keys
        .iter()
        .map(|(key, value)| format!("    '{key}': {value},\n"))
        .collect::<String>();
    out.push_str(&format!(
        "/// Encodes the one frame a frontend may originate, stamping the schema\n\
         /// envelope and the `{variant}` body around it and enforcing every bound\n\
         /// [decode{root}] checks. Every failure is a typed\n\
         /// [ReadContractException]; an over-bound frame never leaves the process.\n\
         String encode{root}({payload} body) {{\n\
         \x20 final text = jsonEncode(<String, Object?>{{\n\
         {literal}\
         \x20 }});\n\
         \x20 if (utf8.encode(text).length > readMaxFrameBytes) {{\n\
         \x20   throw const ReadContractException(ReadErrorKind.frameTooLarge, '{root}');\n\
         \x20 }}\n\
         \x20 return text;\n\
         }}\n\n",
        root = doc.root,
        variant = body.variant,
        payload = body.payload,
    ));
}

fn helpers(out: &mut String, doc: &SchemaDoc) {
    let used = helper_use(doc);
    out.push_str(
        "Map<String, Object?> _object(Object? value, String context) {\n\
         \x20 if (value is! Map<String, Object?>) {\n\
         \x20   throw ReadContractException(ReadErrorKind.shape, context);\n\
         \x20 }\n\
         \x20 return value;\n\
         }\n\n\
         void _knownKeys(Map<String, Object?> map, Set<String> allowed, String context) {\n\
         \x20 for (final key in map.keys) {\n\
         \x20   if (!allowed.contains(key)) {\n\
         \x20     throw ReadContractException(ReadErrorKind.unknownField, context);\n\
         \x20   }\n\
         \x20 }\n\
         }\n\n\
         Object? _field(Map<String, Object?> map, String key, String context) {\n\
         \x20 if (!map.containsKey(key)) {\n\
         \x20   throw ReadContractException(ReadErrorKind.shape, context);\n\
         \x20 }\n\
         \x20 return map[key];\n\
         }\n\n",
    );
    if used.integer {
        out.push_str(
            "int _integer(Object? value, int max, String context) {\n\
             \x20 if (value is! int) {\n\
             \x20   throw ReadContractException(ReadErrorKind.shape, context);\n\
             \x20 }\n\
             \x20 if (value < 0 || value > max) {\n\
             \x20   throw ReadContractException(ReadErrorKind.range, context);\n\
             \x20 }\n\
             \x20 return value;\n\
             }\n\n",
        );
    }
    if used.hex_fixed || used.hex_variable {
        out.push_str(
            "bool _hexChars(String text) {\n\
             \x20 for (final unit in text.codeUnits) {\n\
             \x20   final digit =\n\
             \x20       (unit >= 0x30 && unit <= 0x39) || (unit >= 0x61 && unit <= 0x66);\n\
             \x20   if (!digit) {\n\
             \x20     return false;\n\
             \x20   }\n\
             \x20 }\n\
             \x20 return true;\n\
             }\n\n",
        );
    }
    if used.hex_fixed {
        out.push_str(
            "String _hexFixed(Object? value, int chars, String context) {\n\
             \x20 if (value is! String) {\n\
             \x20   throw ReadContractException(ReadErrorKind.shape, context);\n\
             \x20 }\n\
             \x20 if (value.length != chars || !_hexChars(value)) {\n\
             \x20   throw ReadContractException(ReadErrorKind.bound, context);\n\
             \x20 }\n\
             \x20 return value;\n\
             }\n\n",
        );
    }
    if used.hex_variable {
        out.push_str(
            "String _hexVariable(Object? value, int maxChars, String context) {\n\
             \x20 if (value is! String) {\n\
             \x20   throw ReadContractException(ReadErrorKind.shape, context);\n\
             \x20 }\n\
             \x20 final valid = value.isNotEmpty &&\n\
             \x20     value.length.isEven &&\n\
             \x20     value.length <= maxChars &&\n\
             \x20     _hexChars(value);\n\
             \x20 if (!valid) {\n\
             \x20   throw ReadContractException(ReadErrorKind.bound, context);\n\
             \x20 }\n\
             \x20 return value;\n\
             }\n\n",
        );
    }
    if used.utf8 {
        out.push_str(
            "String _utf8Bounded(Object? value, int maxBytes, String context) {\n\
             \x20 if (value is! String) {\n\
             \x20   throw ReadContractException(ReadErrorKind.shape, context);\n\
             \x20 }\n\
             \x20 // Unpaired surrogates parse here but not in the Rust reference codec;\n\
             \x20 // reject them so every language accepts the same strings.\n\
             \x20 var index = 0;\n\
             \x20 while (index < value.length) {\n\
             \x20   final unit = value.codeUnitAt(index);\n\
             \x20   if (unit >= 0xd800 && unit <= 0xdbff) {\n\
             \x20     if (index + 1 == value.length) {\n\
             \x20       throw ReadContractException(ReadErrorKind.shape, context);\n\
             \x20     }\n\
             \x20     final next = value.codeUnitAt(index + 1);\n\
             \x20     if (next < 0xdc00 || next > 0xdfff) {\n\
             \x20       throw ReadContractException(ReadErrorKind.shape, context);\n\
             \x20     }\n\
             \x20     index += 2;\n\
             \x20   } else if (unit >= 0xdc00 && unit <= 0xdfff) {\n\
             \x20     throw ReadContractException(ReadErrorKind.shape, context);\n\
             \x20   } else {\n\
             \x20     index += 1;\n\
             \x20   }\n\
             \x20 }\n\
             \x20 if (utf8.encode(value).length > maxBytes) {\n\
             \x20   throw ReadContractException(ReadErrorKind.bound, context);\n\
             \x20 }\n\
             \x20 return value;\n\
             }\n\n",
        );
    }
    if used.ascii {
        out.push_str(
            "String _asciiBounded(Object? value, int maxBytes, String context) {\n\
             \x20 if (value is! String) {\n\
             \x20   throw ReadContractException(ReadErrorKind.shape, context);\n\
             \x20 }\n\
             \x20 if (value.length > maxBytes) {\n\
             \x20   throw ReadContractException(ReadErrorKind.bound, context);\n\
             \x20 }\n\
             \x20 for (final unit in value.codeUnits) {\n\
             \x20   if (unit < 0x20 || unit > 0x7e) {\n\
             \x20     throw ReadContractException(ReadErrorKind.bound, context);\n\
             \x20   }\n\
             \x20 }\n\
             \x20 return value;\n\
             }\n\n",
        );
    }
    if used.list {
        out.push_str(
            "List<T> _list<T>(\n\
             \x20 Object? value,\n\
             \x20 int maxLen,\n\
             \x20 String context,\n\
             \x20 T Function(Object?, String) decodeElement,\n\
             ) {\n\
             \x20 if (value is! List<Object?>) {\n\
             \x20   throw ReadContractException(ReadErrorKind.shape, context);\n\
             \x20 }\n\
             \x20 if (value.length > maxLen) {\n\
             \x20   throw ReadContractException(ReadErrorKind.bound, context);\n\
             \x20 }\n\
             \x20 return List<T>.unmodifiable(\n\
             \x20   value.map((item) => decodeElement(item, context)),\n\
             \x20 );\n\
             }\n\n",
        );
    }
    if used.payload_variant {
        out.push_str(
            "Object? _payload(Map<String, Object?> map, String context) {\n\
             \x20 final value = map['value'];\n\
             \x20 if (value == null) {\n\
             \x20   throw ReadContractException(ReadErrorKind.shape, context);\n\
             \x20 }\n\
             \x20 return value;\n\
             }\n\n",
        );
    }
    if used.unit_variant {
        out.push_str(
            "void _unitPayload(Map<String, Object?> map, String context) {\n\
             \x20 if (map['value'] != null) {\n\
             \x20   throw ReadContractException(ReadErrorKind.shape, context);\n\
             \x20 }\n\
             }\n\n",
        );
    }
    encode_helpers(out, encode_helper_use(doc));
}

/// The encode half of every helper group the encodable declarations exercise.
/// Each one *is* the decode predicate — the decode helper's type test is a
/// tautology for an already-typed value — so the two halves cannot check
/// different bounds, and no bound is written twice.
fn encode_helpers(out: &mut String, used: super::HelperUse) {
    if used.integer {
        out.push_str(
            "int _encodeInteger(int value, int max, String context) =>\n\
             \x20   _integer(value, max, context);\n\n",
        );
    }
    if used.hex_fixed {
        out.push_str(
            "String _encodeHexFixed(String value, int chars, String context) =>\n\
             \x20   _hexFixed(value, chars, context);\n\n",
        );
    }
    if used.hex_variable {
        out.push_str(
            "String _encodeHexVariable(String value, int maxChars, String context) =>\n\
             \x20   _hexVariable(value, maxChars, context);\n\n",
        );
    }
    if used.utf8 {
        out.push_str(
            "String _encodeUtf8Bounded(String value, int maxBytes, String context) =>\n\
             \x20   _utf8Bounded(value, maxBytes, context);\n\n",
        );
    }
    if used.ascii {
        out.push_str(
            "String _encodeAsciiBounded(String value, int maxBytes, String context) =>\n\
             \x20   _asciiBounded(value, maxBytes, context);\n\n",
        );
    }
    if used.list {
        out.push_str(
            "List<Object?> _encodeList<T>(\n\
             \x20 List<T> value,\n\
             \x20 int maxLen,\n\
             \x20 String context,\n\
             \x20 Object? Function(T) encodeElement,\n\
             ) {\n\
             \x20 if (value.length > maxLen) {\n\
             \x20   throw ReadContractException(ReadErrorKind.bound, context);\n\
             \x20 }\n\
             \x20 return value.map(encodeElement).toList();\n\
             }\n\n",
        );
    }
}

fn decode_expr(ty: &FieldTy, json: &str, context: &str) -> String {
    if let FieldTy::Named(name) = ty {
        return format!("_decode{name}({json}, {context})");
    }
    let predicate = scalar_predicate(ty, U63_MAX);
    format!(
        "_{}({json}, {}, {context})",
        predicate.stem, predicate.bound
    )
}

fn decode_fn(out: &mut String, doc: &SchemaDoc, decl: &Decl) {
    match decl {
        Decl::Enum(decl) => {
            out.push_str(&format!(
                "{name} _decode{name}(Object? value, String context) {{\n\
                 \x20 return switch (value) {{\n",
                name = decl.name
            ));
            for variant in &decl.variants {
                out.push_str(&format!(
                    "    '{variant}' => {}.{},\n",
                    decl.name,
                    dart_member(variant)
                ));
            }
            out.push_str(
                "    String() =>\n\
                 \x20     throw ReadContractException(ReadErrorKind.unknownVariant, context),\n\
                 \x20   _ => throw ReadContractException(ReadErrorKind.shape, context),\n\
                 \x20 };\n\
                 }\n\n",
            );
        }
        Decl::Struct(decl) => decode_struct_fn(out, doc, decl),
        Decl::Union(decl) => decode_union_fn(out, decl),
    }
}

fn decode_struct_fn(out: &mut String, doc: &SchemaDoc, decl: &StructDecl) {
    out.push_str(&format!(
        "{name} _decode{name}(Object? value, String context) {{\n\
         \x20 final map = _object(value, context);\n",
        name = decl.name
    ));
    let keys = decl
        .fields
        .iter()
        .map(|field| format!("'{}'", field.name))
        .collect::<Vec<_>>()
        .join(", ");
    out.push_str(&format!("  _knownKeys(map, const {{{keys}}}, context);\n"));
    out.push_str(&format!("  return {}(\n", decl.name));
    for field in &decl.fields {
        if is_envelope_field(doc, &decl.name, &field.name) {
            continue;
        }
        let context = format!("'{}.{}'", decl.name, field.name);
        let json = format!("_field(map, '{}', {context})", field.name);
        let dart_name = dart_member(&field.name);
        match &field.ty {
            FieldTy::Option(inner) => {
                let inner_expr = decode_expr(inner, "present", &context);
                out.push_str(&format!(
                    "    {dart_name}: switch ({json}) {{\n\
                     \x20     null => null,\n\
                     \x20     final present => {inner_expr},\n\
                     \x20   }},\n"
                ));
            }
            FieldTy::List { element, max_len } => {
                let FieldTy::Named(element) = element.as_ref() else {
                    unreachable!("list elements are named types");
                };
                out.push_str(&format!(
                    "    {dart_name}: _list({json}, {max_len}, {context}, _decode{element}),\n"
                ));
            }
            ty => {
                let expr = decode_expr(ty, &json, &context);
                out.push_str(&format!("    {dart_name}: {expr},\n"));
            }
        }
    }
    out.push_str("  );\n}\n\n");
}

fn decode_union_fn(out: &mut String, decl: &UnionDecl) {
    out.push_str(&format!(
        "{name} _decode{name}(Object? value, String context) {{\n\
         \x20 final map = _object(value, context);\n\
         \x20 _knownKeys(map, const {{'kind', 'value'}}, context);\n\
         \x20 final kind = _field(map, 'kind', context);\n\
         \x20 if (kind is! String) {{\n\
         \x20   throw ReadContractException(ReadErrorKind.shape, context);\n\
         \x20 }}\n\
         \x20 switch (kind) {{\n",
        name = decl.name
    ));
    for variant in &decl.variants {
        let context = format!("'{}.{}'", decl.name, variant.name);
        let class = format!("{}{}", decl.name, upper_camel(&variant.name));
        match &variant.payload {
            Some(payload) => {
                out.push_str(&format!(
                    "    case '{name}':\n\
                     \x20     return {class}(\n\
                     \x20       _decode{payload}(_payload(map, {context}), {context}),\n\
                     \x20     );\n",
                    name = variant.name,
                ));
            }
            None => {
                out.push_str(&format!(
                    "    case '{name}':\n\
                     \x20     _unitPayload(map, {context});\n\
                     \x20     return const {class}();\n",
                    name = variant.name,
                ));
            }
        }
    }
    out.push_str(
        "    default:\n\
         \x20     throw ReadContractException(ReadErrorKind.unknownVariant, context);\n\
         \x20 }\n\
         }\n\n",
    );
}

fn encode_expr(ty: &FieldTy, value: &str, context: &str) -> String {
    if let FieldTy::Named(name) = ty {
        return format!("_encode{name}({value})");
    }
    let predicate = scalar_predicate(ty, U63_MAX);
    format!(
        "_{}({value}, {}, {context})",
        predicate.encode_stem(),
        predicate.bound
    )
}

fn encode_fn(out: &mut String, decl: &Decl) {
    match decl {
        Decl::Enum(decl) => {
            out.push_str(&format!(
                "String _encode{name}({name} value) {{\n  return switch (value) {{\n",
                name = decl.name
            ));
            for variant in &decl.variants {
                out.push_str(&format!(
                    "    {}.{} => '{variant}',\n",
                    decl.name,
                    dart_member(variant)
                ));
            }
            out.push_str("  };\n}\n\n");
        }
        Decl::Struct(decl) => encode_struct_fn(out, decl),
        Decl::Union(decl) => encode_union_fn(out, decl),
    }
}

/// Keys are emitted in sorted order because `jsonEncode` keeps insertion order
/// and the Rust reference codec serializes a sorted map: same value, same bytes.
/// The root frame is never encodable (nothing a frontend originates contains
/// it), so the envelope is stamped by the entry point alone.
fn encode_struct_fn(out: &mut String, decl: &StructDecl) {
    out.push_str(&format!(
        "Map<String, Object?> _encode{name}({name} value) {{\n\
         \x20 return <String, Object?>{{\n",
        name = decl.name
    ));
    let mut fields: Vec<&FieldDecl> = decl.fields.iter().collect();
    fields.sort_by(|left, right| left.name.cmp(&right.name));
    for field in fields {
        let context = format!("'{}.{}'", decl.name, field.name);
        let member = format!("value.{}", dart_member(&field.name));
        match &field.ty {
            FieldTy::Option(inner) => {
                let inner_expr = encode_expr(inner, &format!("{member}!"), &context);
                out.push_str(&format!(
                    "    '{name}': {member} == null ? null : {inner_expr},\n",
                    name = field.name,
                ));
            }
            FieldTy::List { element, max_len } => {
                let FieldTy::Named(element) = element.as_ref() else {
                    unreachable!("list elements are named types");
                };
                out.push_str(&format!(
                    "    '{name}': _encodeList({member}, {max_len}, {context}, _encode{element}),\n",
                    name = field.name,
                ));
            }
            ty => {
                let expr = encode_expr(ty, &member, &context);
                out.push_str(&format!("    '{name}': {expr},\n", name = field.name));
            }
        }
    }
    out.push_str("  };\n}\n\n");
}

fn encode_union_fn(out: &mut String, decl: &UnionDecl) {
    out.push_str(&format!(
        "Map<String, Object?> _encode{name}({name} value) {{\n  return switch (value) {{\n",
        name = decl.name
    ));
    for variant in &decl.variants {
        let class = format!("{}{}", decl.name, upper_camel(&variant.name));
        match &variant.payload {
            Some(payload) => out.push_str(&format!(
                "    {class}(value: final payload) => <String, Object?>{{\n\
                 \x20       'kind': '{name}',\n\
                 \x20       'value': _encode{payload}(payload),\n\
                 \x20     }},\n",
                name = variant.name,
            )),
            None => out.push_str(&format!(
                "    {class}() => <String, Object?>{{'kind': '{name}'}},\n",
                name = variant.name,
            )),
        }
    }
    out.push_str("  };\n}\n\n");
}
