//! Emits the generated Swift read bindings (`generated/swift/EnvoixRead.swift`).
//!
//! Decode-only, over Foundation `JSONSerialization`. Booleans and doubles are
//! rejected for integer fields via `NSNumber.objCType`; every shape, range,
//! bound, and unknown-variant/field/schema rule matches the Rust reference
//! codec.

use crate::model::{Decl, FieldTy, SchemaDoc, StructDecl, UnionDecl};

use crate::model::RuleValue;

use super::{helper_use, is_envelope_field, swift_member};

pub fn module(doc: &SchemaDoc) -> String {
    let mut out = String::new();
    out.push_str("// @generated from schema/read.schema by envoix-bindings. Do not edit;\n");
    out.push_str(
        "// regenerate with `ENVOIX_BINDINGS_REGEN=1 cargo test -p envoix-bindings generated_artifacts`.\n",
    );
    out.push_str(
        "// Known platform caveats: JSON `-0` may decode as integer 0 here while the\n\
         // Rust reference codec rejects it (benign: every field with a positive\n\
         // minimum still fails its range check). Unpaired-surrogate escapes need no\n\
         // explicit scan in this artifact: a Swift `String` cannot hold them, so\n\
         // `JSONSerialization` never produces one.\n\n",
    );
    out.push_str("import Foundation\n\n");
    out.push_str(&format!("public let readSchemaId = \"{}\"\n", doc.id));
    out.push_str(&format!(
        "public let readMaxFrameBytes = {}\n",
        doc.max_frame_bytes
    ));
    rules_consts(&mut out, doc);
    out.push_str("private let u63Max: Int64 = 9_223_372_036_854_775_807\n\n");
    error_type(&mut out);
    for decl in &doc.decls {
        type_decl(&mut out, doc, decl);
    }
    if super::rust::supports_epoch_gate(doc) {
        epoch_gate(&mut out);
    }
    codec(&mut out, doc);
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
                "public let {} = {flag}\n",
                super::lower_camel(key)
            )),
            RuleValue::Int(bound) => out.push_str(&format!(
                "public let {} = {bound}\n",
                super::lower_camel(key)
            )),
        }
    }
}

fn error_type(out: &mut String) {
    out.push_str(
        "public enum ReadErrorKind {\n\
         \x20   case frameTooLarge\n\
         \x20   case malformedJson\n\
         \x20   case unknownSchema\n\
         \x20   case shape\n\
         \x20   case unknownField\n\
         \x20   case unknownVariant\n\
         \x20   case range\n\
         \x20   case bound\n\
         }\n\n\
         /// Typed codec failure carrying only static schema context.\n\
         public struct ReadContractError: Error, Equatable {\n\
         \x20   public let kind: ReadErrorKind\n\
         \x20   public let context: String\n\
         }\n\n",
    );
}

fn swift_ty(ty: &FieldTy) -> String {
    match ty {
        FieldTy::U16 | FieldTy::U32 | FieldTy::U63 => "Int64".to_owned(),
        FieldTy::Hex16
        | FieldTy::Hex32
        | FieldTy::Hex64
        | FieldTy::HexVar { .. }
        | FieldTy::Str { .. }
        | FieldTy::Ascii { .. } => "String".to_owned(),
        FieldTy::Named(name) => name.clone(),
        FieldTy::Option(inner) => format!("{}?", swift_ty(inner)),
        FieldTy::List { element, .. } => format!("[{}]", swift_ty(element)),
    }
}

fn type_decl(out: &mut String, doc: &SchemaDoc, decl: &Decl) {
    match decl {
        Decl::Enum(decl) => {
            out.push_str(&format!(
                "public enum {}: String, Equatable {{\n",
                decl.name
            ));
            for variant in &decl.variants {
                out.push_str(&format!(
                    "    case {} = \"{variant}\"\n",
                    swift_member(variant)
                ));
            }
            out.push_str("}\n\n");
        }
        Decl::Struct(decl) => {
            out.push_str(&format!("public struct {}: Equatable {{\n", decl.name));
            for field in &decl.fields {
                if is_envelope_field(doc, &decl.name, &field.name) {
                    continue;
                }
                out.push_str(&format!(
                    "    public let {}: {}\n",
                    swift_member(&field.name),
                    swift_ty(&field.ty)
                ));
            }
            out.push_str("}\n\n");
        }
        Decl::Union(decl) => {
            out.push_str(&format!("public enum {}: Equatable {{\n", decl.name));
            for variant in &decl.variants {
                match &variant.payload {
                    Some(payload) => out.push_str(&format!(
                        "    case {}({payload})\n",
                        swift_member(&variant.name)
                    )),
                    None => out.push_str(&format!("    case {}\n", swift_member(&variant.name))),
                }
            }
            out.push_str("}\n\n");
        }
    }
}

fn epoch_gate(out: &mut String) {
    out.push_str(
        "public enum GateDecision {\n\
         \x20   case deliver\n\
         \x20   case dropStale\n\
         \x20   case contractBreach\n\
         }\n\n\
         /// Client-side admission for the per-epoch card stream: one gate per\n\
         /// attachment. Frames from another epoch are stale; every epoch starts\n\
         /// with a snapshot; a lag or close ends the epoch permanently.\n\
         public struct EpochGate {\n\
         \x20   private let epoch: Int64\n\
         \x20   private var sawSnapshot = false\n\
         \x20   private var dead = false\n\n\
         \x20   public init(attach epoch: Int64) {\n\
         \x20       self.epoch = epoch\n\
         \x20   }\n\n\
         \x20   public mutating func admit(_ frame: ReadFrame) -> GateDecision {\n\
         \x20       switch frame.body {\n\
         \x20       case .cardUpdate(let update):\n\
         \x20           if update.epoch != epoch || dead {\n\
         \x20               return .dropStale\n\
         \x20           }\n\
         \x20           if case .snapshot = update.kind {\n\
         \x20               if sawSnapshot {\n\
         \x20                   return .contractBreach\n\
         \x20               }\n\
         \x20               sawSnapshot = true\n\
         \x20               return .deliver\n\
         \x20           }\n\
         \x20           return sawSnapshot ? .deliver : .contractBreach\n\
         \x20       case .lag(let lag):\n\
         \x20           return terminate(lag.epoch)\n\
         \x20       case .closed(let closed):\n\
         \x20           return terminate(closed.epoch)\n\
         \x20       default:\n\
         \x20           return .deliver\n\
         \x20       }\n\
         \x20   }\n\n\
         \x20   private mutating func terminate(_ frameEpoch: Int64) -> GateDecision {\n\
         \x20       if frameEpoch == epoch && !dead {\n\
         \x20           dead = true\n\
         \x20           return .deliver\n\
         \x20       }\n\
         \x20       return .dropStale\n\
         \x20   }\n\
         }\n\n",
    );
}

fn codec(out: &mut String, doc: &SchemaDoc) {
    out.push_str("public enum EnvoixReadCodec {\n");
    entry_point(out, doc);
    helpers(out, doc);
    for decl in &doc.decls {
        decode_fn(out, doc, decl);
    }
    if out.ends_with("\n\n") {
        out.pop();
    }
    out.push_str("}\n");
}

fn entry_point(out: &mut String, doc: &SchemaDoc) {
    out.push_str(&format!(
        "    /// Decodes and validates one frame. Every failure is a typed\n\
         \x20   /// `ReadContractError`; no input, however hostile, misparses.\n\
         \x20   public static func decode(_ data: Data) throws -> {root} {{\n\
         \x20       if data.count > readMaxFrameBytes {{\n\
         \x20           throw ReadContractError(kind: .frameTooLarge, context: \"{root}\")\n\
         \x20       }}\n\
         \x20       let parsed: Any\n\
         \x20       do {{\n\
         \x20           parsed = try JSONSerialization.jsonObject(with: data)\n\
         \x20       }} catch {{\n\
         \x20           throw ReadContractError(kind: .malformedJson, context: \"{root}\")\n\
         \x20       }}\n\
         \x20       let map = try object(parsed, \"{root}\")\n\
         \x20       guard let schema = map[\"schema\"] as? String else {{\n\
         \x20           throw ReadContractError(kind: .shape, context: \"{root}.schema\")\n\
         \x20       }}\n\
         \x20       guard schema == readSchemaId else {{\n\
         \x20           throw ReadContractError(kind: .unknownSchema, context: \"{root}\")\n\
         \x20       }}\n\
         \x20       return try decode{root}(parsed, \"{root}\")\n\
         \x20   }}\n\n",
        root = doc.root,
    ));
}

fn helpers(out: &mut String, doc: &SchemaDoc) {
    let used = helper_use(doc);
    out.push_str(
        "    private static func object(_ value: Any?, _ context: String) throws -> [String: Any] {\n\
         \x20       guard let map = value as? [String: Any] else {\n\
         \x20           throw ReadContractError(kind: .shape, context: context)\n\
         \x20       }\n\
         \x20       return map\n\
         \x20   }\n\n\
         \x20   private static func knownKeys(_ map: [String: Any], _ allowed: Set<String>, _ context: String) throws {\n\
         \x20       for key in map.keys where !allowed.contains(key) {\n\
         \x20           throw ReadContractError(kind: .unknownField, context: context)\n\
         \x20       }\n\
         \x20   }\n\n\
         \x20   private static func field(_ map: [String: Any], _ key: String, _ context: String) throws -> Any? {\n\
         \x20       guard let value = map[key] else {\n\
         \x20           throw ReadContractError(kind: .shape, context: context)\n\
         \x20       }\n\
         \x20       return value is NSNull ? nil : value\n\
         \x20   }\n\n",
    );
    if used.integer {
        out.push_str(
            "    private static func integer(_ value: Any?, _ max: Int64, _ context: String) throws -> Int64 {\n\
             \x20       guard let number = value as? NSNumber else {\n\
             \x20           throw ReadContractError(kind: .shape, context: context)\n\
             \x20       }\n\
             \x20       let objCType = String(cString: number.objCType)\n\
             \x20       if objCType == \"c\" || objCType == \"B\" || objCType == \"d\" || objCType == \"f\" {\n\
             \x20           throw ReadContractError(kind: .shape, context: context)\n\
             \x20       }\n\
             \x20       let wide = number.int64Value\n\
             \x20       guard wide >= 0, wide <= max else {\n\
             \x20           throw ReadContractError(kind: .range, context: context)\n\
             \x20       }\n\
             \x20       return wide\n\
             \x20   }\n\n",
        );
    }
    if used.hex_fixed || used.hex_variable {
        out.push_str(
            "    private static func hexChars(_ text: String) -> Bool {\n\
             \x20       for scalar in text.unicodeScalars {\n\
             \x20           let digit = (scalar.value >= 0x30 && scalar.value <= 0x39)\n\
             \x20               || (scalar.value >= 0x61 && scalar.value <= 0x66)\n\
             \x20           if !digit {\n\
             \x20               return false\n\
             \x20           }\n\
             \x20       }\n\
             \x20       return true\n\
             \x20   }\n\n",
        );
    }
    if used.hex_fixed {
        out.push_str(
            "    private static func hexFixed(_ value: Any?, _ chars: Int, _ context: String) throws -> String {\n\
             \x20       guard let text = value as? String else {\n\
             \x20           throw ReadContractError(kind: .shape, context: context)\n\
             \x20       }\n\
             \x20       guard text.utf8.count == chars, hexChars(text) else {\n\
             \x20           throw ReadContractError(kind: .bound, context: context)\n\
             \x20       }\n\
             \x20       return text\n\
             \x20   }\n\n",
        );
    }
    if used.hex_variable {
        out.push_str(
            "    private static func hexVariable(_ value: Any?, _ maxChars: Int, _ context: String) throws -> String {\n\
             \x20       guard let text = value as? String else {\n\
             \x20           throw ReadContractError(kind: .shape, context: context)\n\
             \x20       }\n\
             \x20       let length = text.utf8.count\n\
             \x20       let valid = length > 0 && length % 2 == 0 && length <= maxChars && hexChars(text)\n\
             \x20       guard valid else {\n\
             \x20           throw ReadContractError(kind: .bound, context: context)\n\
             \x20       }\n\
             \x20       return text\n\
             \x20   }\n\n",
        );
    }
    if used.utf8 {
        out.push_str(
            "    private static func utf8Bounded(_ value: Any?, _ maxBytes: Int, _ context: String) throws -> String {\n\
             \x20       guard let text = value as? String else {\n\
             \x20           throw ReadContractError(kind: .shape, context: context)\n\
             \x20       }\n\
             \x20       guard text.utf8.count <= maxBytes else {\n\
             \x20           throw ReadContractError(kind: .bound, context: context)\n\
             \x20       }\n\
             \x20       return text\n\
             \x20   }\n\n",
        );
    }
    if used.ascii {
        out.push_str(
            "    private static func asciiBounded(_ value: Any?, _ maxBytes: Int, _ context: String) throws -> String {\n\
             \x20       guard let text = value as? String else {\n\
             \x20           throw ReadContractError(kind: .shape, context: context)\n\
             \x20       }\n\
             \x20       let valid = text.utf8.count <= maxBytes\n\
             \x20           && text.unicodeScalars.allSatisfy { $0.value >= 0x20 && $0.value <= 0x7e }\n\
             \x20       guard valid else {\n\
             \x20           throw ReadContractError(kind: .bound, context: context)\n\
             \x20       }\n\
             \x20       return text\n\
             \x20   }\n\n",
        );
    }
    if used.list {
        out.push_str(
            "    private static func decodeList<T>(\n\
             \x20       _ value: Any?,\n\
             \x20       _ maxLen: Int,\n\
             \x20       _ context: String,\n\
             \x20       _ decodeElement: (Any?, String) throws -> T\n\
             \x20   ) throws -> [T] {\n\
             \x20       guard let items = value as? [Any] else {\n\
             \x20           throw ReadContractError(kind: .shape, context: context)\n\
             \x20       }\n\
             \x20       if items.count > maxLen {\n\
             \x20           throw ReadContractError(kind: .bound, context: context)\n\
             \x20       }\n\
             \x20       return try items.map { try decodeElement($0 is NSNull ? nil : $0, context) }\n\
             \x20   }\n\n",
        );
    }
    if used.payload_variant {
        out.push_str(
            "    private static func payload(_ map: [String: Any], _ context: String) throws -> Any {\n\
             \x20       guard let value = map[\"value\"], !(value is NSNull) else {\n\
             \x20           throw ReadContractError(kind: .shape, context: context)\n\
             \x20       }\n\
             \x20       return value\n\
             \x20   }\n\n",
        );
    }
    if used.unit_variant {
        out.push_str(
            "    private static func unitPayload(_ map: [String: Any], _ context: String) throws {\n\
             \x20       if let value = map[\"value\"], !(value is NSNull) {\n\
             \x20           throw ReadContractError(kind: .shape, context: context)\n\
             \x20       }\n\
             \x20   }\n\n",
        );
    }
}

fn decode_expr(ty: &FieldTy, json: &str, context: &str) -> String {
    match ty {
        FieldTy::U16 => format!("try integer({json}, 65535, {context})"),
        FieldTy::U32 => format!("try integer({json}, 4294967295, {context})"),
        FieldTy::U63 => format!("try integer({json}, u63Max, {context})"),
        FieldTy::Hex16 => format!("try hexFixed({json}, 16, {context})"),
        FieldTy::Hex32 => format!("try hexFixed({json}, 32, {context})"),
        FieldTy::Hex64 => format!("try hexFixed({json}, 64, {context})"),
        FieldTy::HexVar { max_chars } => format!("try hexVariable({json}, {max_chars}, {context})"),
        FieldTy::Str { max_bytes } => format!("try utf8Bounded({json}, {max_bytes}, {context})"),
        FieldTy::Ascii { max_bytes } => format!("try asciiBounded({json}, {max_bytes}, {context})"),
        FieldTy::Named(name) => format!("try decode{name}({json}, {context})"),
        FieldTy::Option(_) | FieldTy::List { .. } => {
            unreachable!("wrapper types are expanded at the field level")
        }
    }
}

fn decode_fn(out: &mut String, doc: &SchemaDoc, decl: &Decl) {
    match decl {
        Decl::Enum(decl) => {
            out.push_str(&format!(
                "    private static func decode{name}(_ value: Any?, _ context: String) throws -> {name} {{\n\
                 \x20       guard let text = value as? String else {{\n\
                 \x20           throw ReadContractError(kind: .shape, context: context)\n\
                 \x20       }}\n\
                 \x20       guard let decoded = {name}(rawValue: text) else {{\n\
                 \x20           throw ReadContractError(kind: .unknownVariant, context: context)\n\
                 \x20       }}\n\
                 \x20       return decoded\n\
                 \x20   }}\n\n",
                name = decl.name
            ));
        }
        Decl::Struct(decl) => decode_struct_fn(out, doc, decl),
        Decl::Union(decl) => decode_union_fn(out, decl),
    }
}

fn decode_struct_fn(out: &mut String, doc: &SchemaDoc, decl: &StructDecl) {
    out.push_str(&format!(
        "    private static func decode{name}(_ value: Any?, _ context: String) throws -> {name} {{\n\
         \x20       let map = try object(value, context)\n",
        name = decl.name
    ));
    let keys = decl
        .fields
        .iter()
        .map(|field| format!("\"{}\"", field.name))
        .collect::<Vec<_>>()
        .join(", ");
    out.push_str(&format!("        try knownKeys(map, [{keys}], context)\n"));
    for field in &decl.fields {
        if is_envelope_field(doc, &decl.name, &field.name) {
            continue;
        }
        let context = format!("\"{}.{}\"", decl.name, field.name);
        let json = format!("try field(map, \"{}\", {context})", field.name);
        let swift_name = swift_member(&field.name);
        match &field.ty {
            FieldTy::Option(inner) => {
                let inner_expr = decode_expr(inner, "present", &context);
                out.push_str(&format!(
                    "        let {swift_name}: {}?\n\
                     \x20       if let present = {json} {{\n\
                     \x20           {swift_name} = {inner_expr}\n\
                     \x20       }} else {{\n\
                     \x20           {swift_name} = nil\n\
                     \x20       }}\n",
                    swift_ty(inner)
                ));
            }
            FieldTy::List { element, max_len } => {
                let FieldTy::Named(element) = element.as_ref() else {
                    unreachable!("list elements are named types");
                };
                out.push_str(&format!(
                    "        let {swift_name} = try decodeList({json}, {max_len}, {context}, decode{element})\n"
                ));
            }
            ty => {
                let expr = decode_expr(ty, &json, &context);
                out.push_str(&format!("        let {swift_name} = {expr}\n"));
            }
        }
    }
    let arguments = decl
        .fields
        .iter()
        .filter(|field| !is_envelope_field(doc, &decl.name, &field.name))
        .map(|field| {
            let name = swift_member(&field.name);
            format!("{name}: {name}")
        })
        .collect::<Vec<_>>()
        .join(",\n            ");
    out.push_str(&format!(
        "        return {}(\n            {arguments}\n        )\n    }}\n\n",
        decl.name
    ));
}

fn decode_union_fn(out: &mut String, decl: &UnionDecl) {
    out.push_str(&format!(
        "    private static func decode{name}(_ value: Any?, _ context: String) throws -> {name} {{\n\
         \x20       let map = try object(value, context)\n\
         \x20       try knownKeys(map, [\"kind\", \"value\"], context)\n\
         \x20       guard let kind = try field(map, \"kind\", context) as? String else {{\n\
         \x20           throw ReadContractError(kind: .shape, context: context)\n\
         \x20       }}\n\
         \x20       switch kind {{\n",
        name = decl.name
    ));
    for variant in &decl.variants {
        let context = format!("\"{}.{}\"", decl.name, variant.name);
        let case = swift_member(&variant.name);
        match &variant.payload {
            Some(payload) => {
                out.push_str(&format!(
                    "        case \"{name}\":\n\
                     \x20           return .{case}(try decode{payload}(payload(map, {context}), {context}))\n",
                    name = variant.name,
                ));
            }
            None => {
                out.push_str(&format!(
                    "        case \"{name}\":\n\
                     \x20           try unitPayload(map, {context})\n\
                     \x20           return .{case}\n",
                    name = variant.name,
                ));
            }
        }
    }
    out.push_str(
        "        default:\n\
         \x20           throw ReadContractError(kind: .unknownVariant, context: context)\n\
         \x20       }\n\
         \x20   }\n\n",
    );
}
