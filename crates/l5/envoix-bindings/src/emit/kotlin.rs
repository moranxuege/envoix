//! Emits the generated Kotlin read bindings (`generated/kotlin/EnvoixRead.kt`).
//!
//! Targets Android/JVM with the platform-bundled `org.json`. The JSON syntax
//! layer inherits `org.json` leniency; every shape, range, bound, and
//! unknown-variant/field/schema rule is enforced strictly on top, matching the
//! Rust reference codec. A host-to-frontend contract is decode-only — an
//! observer cannot fabricate an observation. A bidirectional contract also gets
//! an encoder for the one body its frontends may originate, and every encode
//! helper is the decode predicate itself, so the two halves cannot check
//! different things.

use crate::model::{Decl, FieldTy, SchemaDoc, StructDecl, UnionDecl};

use crate::model::RuleValue;

use super::{
    encodable_decls, encode_helper_use, helper_use, is_envelope_field, kotlin_member,
    scalar_predicate, upper_camel, upper_snake,
};

/// How Kotlin names 2^63-1 in the generated artifact.
const U63_MAX: &str = "Long.MAX_VALUE";

pub fn module(doc: &SchemaDoc) -> String {
    let mut out = String::new();
    out.push_str("// @generated from schema/read.schema by envoix-bindings. Do not edit;\n");
    out.push_str(
        "// regenerate with `ENVOIX_BINDINGS_REGEN=1 cargo test -p envoix-bindings generated_artifacts`.\n",
    );
    out.push_str(
        "// Known platform caveats: `org.json` duplicate-key handling is runtime-dependent\n\
         // (Android keeps the last key; the reference json.org jar throws, so JVM unit\n\
         // tests may see MALFORMED_JSON where a device sees last-wins). JSON `-0`\n\
         // decodes as integer 0 here while the Rust reference codec rejects it (benign:\n\
         // every field with a positive minimum still fails its range check).\n",
    );
    if doc.direction.natives_encode() {
        out.push_str(
            "// Encoded frames are semantically identical to the Rust reference codec's but\n\
             // not byte-identical: `org.json` decides key order and escaping, both of which\n\
             // are runtime-dependent. The wire contract is the decoded value, and every\n\
             // decoder here is order-insensitive. The frame cap is defined over the\n\
             // canonical (serde_json) serialization, and `org.json` escapes U+0080..U+009F\n\
             // and U+2000..U+20FF as `\\uXXXX` — up to 3x the canonical bytes — so this\n\
             // encoder can refuse a frame the contract permits. It is never the other way\n\
             // round: the cap is measured on the bytes this artifact actually emits.\n",
        );
    }
    out.push('\n');
    out.push_str("package com.envoix.bindings\n\n");
    out.push_str("import org.json.JSONArray\nimport org.json.JSONException\nimport org.json.JSONObject\nimport org.json.JSONTokener\n\n");
    out.push_str(&format!(
        "const val READ_SCHEMA_ID: String = \"{}\"\n",
        doc.id
    ));
    out.push_str(&format!(
        "const val READ_MAX_FRAME_BYTES: Int = {}\n\n",
        doc.max_frame_bytes
    ));
    rules_consts(&mut out, doc);
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
                "const val {}: Boolean = {flag}\n",
                upper_snake(key)
            )),
            RuleValue::Int(bound) => {
                out.push_str(&format!("const val {}: Int = {bound}\n", upper_snake(key)))
            }
        }
    }
    out.push('\n');
}

fn error_type(out: &mut String) {
    out.push_str(
        "enum class ReadErrorKind {\n\
         \x20   FRAME_TOO_LARGE,\n\
         \x20   MALFORMED_JSON,\n\
         \x20   UNKNOWN_SCHEMA,\n\
         \x20   SHAPE,\n\
         \x20   UNKNOWN_FIELD,\n\
         \x20   UNKNOWN_VARIANT,\n\
         \x20   RANGE,\n\
         \x20   BOUND,\n\
         }\n\n\
         /** Typed codec failure carrying only static schema context. */\n\
         class ReadContractException(val kind: ReadErrorKind, val context: String) :\n\
         \x20   Exception(\"read contract: $kind at $context\")\n\n",
    );
}

fn kotlin_ty(ty: &FieldTy) -> String {
    match ty {
        FieldTy::U16 | FieldTy::U32 | FieldTy::U63 => "Long".to_owned(),
        FieldTy::Hex16
        | FieldTy::Hex32
        | FieldTy::Hex64
        | FieldTy::HexVar { .. }
        | FieldTy::Str { .. }
        | FieldTy::Ascii { .. } => "String".to_owned(),
        FieldTy::Named(name) => name.clone(),
        FieldTy::Option(inner) => format!("{}?", kotlin_ty(inner)),
        FieldTy::List { element, .. } => format!("List<{}>", kotlin_ty(element)),
    }
}

fn type_decl(out: &mut String, doc: &SchemaDoc, decl: &Decl) {
    match decl {
        Decl::Enum(decl) => {
            out.push_str(&format!("enum class {} {{\n", decl.name));
            for variant in &decl.variants {
                out.push_str(&format!("    {},\n", upper_snake(variant)));
            }
            out.push_str("}\n\n");
        }
        Decl::Struct(decl) => {
            out.push_str(&format!("data class {}(\n", decl.name));
            for field in &decl.fields {
                if is_envelope_field(doc, &decl.name, &field.name) {
                    continue;
                }
                out.push_str(&format!(
                    "    val {}: {},\n",
                    kotlin_member(&field.name),
                    kotlin_ty(&field.ty)
                ));
            }
            out.push_str(")\n\n");
        }
        Decl::Union(decl) => {
            out.push_str(&format!("sealed interface {} {{\n", decl.name));
            for variant in &decl.variants {
                let class = upper_camel(&variant.name);
                match &variant.payload {
                    Some(payload) => out.push_str(&format!(
                        "    data class {class}(val value: {payload}) : {}\n",
                        decl.name
                    )),
                    None => out.push_str(&format!("    object {class} : {}\n", decl.name)),
                }
            }
            out.push_str("}\n\n");
        }
    }
}

fn epoch_gate(out: &mut String) {
    out.push_str(
        "enum class GateDecision {\n\
         \x20   DELIVER,\n\
         \x20   DROP_STALE,\n\
         \x20   CONTRACT_BREACH,\n\
         }\n\n\
         /**\n\
         \x20* Client-side admission for the per-epoch card stream: one gate per\n\
         \x20* attachment. Frames from another epoch are stale; every epoch starts\n\
         \x20* with a snapshot; a lag or close ends the epoch permanently.\n\
         \x20*/\n\
         class EpochGate(private val epoch: Long) {\n\
         \x20   private var sawSnapshot = false\n\
         \x20   private var dead = false\n\n\
         \x20   fun admit(frame: ReadFrame): GateDecision = when (val body = frame.body) {\n\
         \x20       is ReadBody.CardUpdate -> {\n\
         \x20           val update = body.value\n\
         \x20           if (update.epoch != epoch || dead) {\n\
         \x20               GateDecision.DROP_STALE\n\
         \x20           } else if (update.kind is CardUpdateKindView.Snapshot) {\n\
         \x20               if (sawSnapshot) {\n\
         \x20                   GateDecision.CONTRACT_BREACH\n\
         \x20               } else {\n\
         \x20                   sawSnapshot = true\n\
         \x20                   GateDecision.DELIVER\n\
         \x20               }\n\
         \x20           } else if (sawSnapshot) {\n\
         \x20               GateDecision.DELIVER\n\
         \x20           } else {\n\
         \x20               GateDecision.CONTRACT_BREACH\n\
         \x20           }\n\
         \x20       }\n\
         \x20       is ReadBody.Lag -> terminate(body.value.epoch)\n\
         \x20       is ReadBody.Closed -> terminate(body.value.epoch)\n\
         \x20       else -> GateDecision.DELIVER\n\
         \x20   }\n\n\
         \x20   private fun terminate(frameEpoch: Long): GateDecision =\n\
         \x20       if (frameEpoch == epoch && !dead) {\n\
         \x20           dead = true\n\
         \x20           GateDecision.DELIVER\n\
         \x20       } else {\n\
         \x20           GateDecision.DROP_STALE\n\
         \x20       }\n\
         }\n\n",
    );
}

fn codec(out: &mut String, doc: &SchemaDoc) {
    out.push_str("object EnvoixReadCodec {\n");
    entry_point(out, doc);
    helpers(out, doc);
    let encodable = encodable_decls(doc);
    for decl in &doc.decls {
        decode_fn(out, doc, decl);
        if encodable.contains(&decl.name()) {
            encode_fn(out, decl);
        }
    }
    if out.ends_with("\n\n") {
        out.pop();
    }
    out.push_str("}\n");
}

fn entry_point(out: &mut String, doc: &SchemaDoc) {
    out.push_str(&format!(
        "    /**\n\
         \x20    * Decodes and validates one frame. Every failure is a typed\n\
         \x20    * [ReadContractException]; no input, however hostile, misparses.\n\
         \x20    */\n\
         \x20   fun decode(text: String): {root} {{\n\
         \x20       if (text.toByteArray(Charsets.UTF_8).size > READ_MAX_FRAME_BYTES) {{\n\
         \x20           throw ReadContractException(ReadErrorKind.FRAME_TOO_LARGE, \"{root}\")\n\
         \x20       }}\n\
         \x20       val tokener = JSONTokener(text)\n\
         \x20       val value = try {{\n\
         \x20           tokener.nextValue()\n\
         \x20       }} catch (exception: JSONException) {{\n\
         \x20           throw ReadContractException(ReadErrorKind.MALFORMED_JSON, \"{root}\")\n\
         \x20       }}\n\
         \x20       while (tokener.more()) {{\n\
         \x20           val trailing = tokener.next()\n\
         \x20           if (trailing != ' ' && trailing != '\\t' && trailing != '\\r' && trailing != '\\n') {{\n\
         \x20               throw ReadContractException(ReadErrorKind.MALFORMED_JSON, \"{root}\")\n\
         \x20           }}\n\
         \x20       }}\n\
         \x20       val map = obj(value, \"{root}\")\n\
         \x20       val schema = map.opt(\"schema\")\n\
         \x20       if (schema !is String) {{\n\
         \x20           throw ReadContractException(ReadErrorKind.SHAPE, \"{root}.schema\")\n\
         \x20       }}\n\
         \x20       if (schema != READ_SCHEMA_ID) {{\n\
         \x20           throw ReadContractException(ReadErrorKind.UNKNOWN_SCHEMA, \"{root}\")\n\
         \x20       }}\n\
         \x20       return decode{root}(value, \"{root}\")\n\
         \x20   }}\n\n",
        root = doc.root,
    ));
    let Some(body) = doc.frontend_body() else {
        return;
    };
    out.push_str(&format!(
        "    /**\n\
         \x20    * Encodes the one frame a frontend may originate, stamping the schema\n\
         \x20    * envelope and the `{variant}` body around it and enforcing every bound\n\
         \x20    * [decode] checks. Every failure is a typed [ReadContractException]; an\n\
         \x20    * over-bound frame never leaves the process.\n\
         \x20    */\n\
         \x20   fun encode(body: {payload}): String {{\n\
         \x20       val map = JSONObject()\n\
         \x20       map.put(\"schema\", READ_SCHEMA_ID)\n\
         \x20       map.put(\n\
         \x20           \"{field}\",\n\
         \x20           JSONObject().put(\"kind\", \"{variant}\").put(\"value\", encode{payload}(body)),\n\
         \x20       )\n\
         \x20       val text = map.toString()\n\
         \x20       if (text.toByteArray(Charsets.UTF_8).size > READ_MAX_FRAME_BYTES) {{\n\
         \x20           throw ReadContractException(ReadErrorKind.FRAME_TOO_LARGE, \"{root}\")\n\
         \x20       }}\n\
         \x20       return text\n\
         \x20   }}\n\n",
        root = doc.root,
        field = body.field,
        variant = body.variant,
        payload = body.payload,
    ));
}

fn helpers(out: &mut String, doc: &SchemaDoc) {
    let used = helper_use(doc);
    out.push_str(
        "    private fun obj(value: Any?, context: String): JSONObject =\n\
         \x20       value as? JSONObject ?: throw ReadContractException(ReadErrorKind.SHAPE, context)\n\n\
         \x20   private fun knownKeys(map: JSONObject, allowed: Set<String>, context: String) {\n\
         \x20       for (key in map.keys()) {\n\
         \x20           if (key !in allowed) {\n\
         \x20               throw ReadContractException(ReadErrorKind.UNKNOWN_FIELD, context)\n\
         \x20           }\n\
         \x20       }\n\
         \x20   }\n\n\
         \x20   private fun field(map: JSONObject, key: String, context: String): Any? {\n\
         \x20       if (!map.has(key)) {\n\
         \x20           throw ReadContractException(ReadErrorKind.SHAPE, context)\n\
         \x20       }\n\
         \x20       val value = map.get(key)\n\
         \x20       return if (value == JSONObject.NULL) null else value\n\
         \x20   }\n\n",
    );
    if used.integer {
        out.push_str(
            "    private fun integer(value: Any?, max: Long, context: String): Long {\n\
             \x20       val number = when (value) {\n\
             \x20           is Int -> value.toLong()\n\
             \x20           is Long -> value\n\
             \x20           else -> throw ReadContractException(ReadErrorKind.SHAPE, context)\n\
             \x20       }\n\
             \x20       if (number < 0 || number > max) {\n\
             \x20           throw ReadContractException(ReadErrorKind.RANGE, context)\n\
             \x20       }\n\
             \x20       return number\n\
             \x20   }\n\n",
        );
    }
    if used.hex_fixed || used.hex_variable {
        out.push_str(
            "    private fun hexChars(text: String): Boolean =\n\
             \x20       text.all { it in '0'..'9' || it in 'a'..'f' }\n\n",
        );
    }
    if used.hex_fixed {
        out.push_str(
            "    private fun hexFixed(value: Any?, chars: Int, context: String): String {\n\
             \x20       if (value !is String) {\n\
             \x20           throw ReadContractException(ReadErrorKind.SHAPE, context)\n\
             \x20       }\n\
             \x20       if (value.length != chars || !hexChars(value)) {\n\
             \x20           throw ReadContractException(ReadErrorKind.BOUND, context)\n\
             \x20       }\n\
             \x20       return value\n\
             \x20   }\n\n",
        );
    }
    if used.hex_variable {
        out.push_str(
            "    private fun hexVariable(value: Any?, maxChars: Int, context: String): String {\n\
             \x20       if (value !is String) {\n\
             \x20           throw ReadContractException(ReadErrorKind.SHAPE, context)\n\
             \x20       }\n\
             \x20       val valid = value.isNotEmpty() &&\n\
             \x20           value.length % 2 == 0 &&\n\
             \x20           value.length <= maxChars &&\n\
             \x20           hexChars(value)\n\
             \x20       if (!valid) {\n\
             \x20           throw ReadContractException(ReadErrorKind.BOUND, context)\n\
             \x20       }\n\
             \x20       return value\n\
             \x20   }\n\n",
        );
    }
    if used.utf8 {
        out.push_str(
            "    private fun utf8Bounded(value: Any?, maxBytes: Int, context: String): String {\n\
             \x20       if (value !is String) {\n\
             \x20           throw ReadContractException(ReadErrorKind.SHAPE, context)\n\
             \x20       }\n\
             \x20       // Unpaired surrogates parse here but not in the Rust reference codec;\n\
             \x20       // reject them so every language accepts the same strings.\n\
             \x20       var index = 0\n\
             \x20       while (index < value.length) {\n\
             \x20           val unit = value[index]\n\
             \x20           if (unit.isHighSurrogate()) {\n\
             \x20               if (index + 1 == value.length || !value[index + 1].isLowSurrogate()) {\n\
             \x20                   throw ReadContractException(ReadErrorKind.SHAPE, context)\n\
             \x20               }\n\
             \x20               index += 2\n\
             \x20           } else if (unit.isLowSurrogate()) {\n\
             \x20               throw ReadContractException(ReadErrorKind.SHAPE, context)\n\
             \x20           } else {\n\
             \x20               index += 1\n\
             \x20           }\n\
             \x20       }\n\
             \x20       if (value.toByteArray(Charsets.UTF_8).size > maxBytes) {\n\
             \x20           throw ReadContractException(ReadErrorKind.BOUND, context)\n\
             \x20       }\n\
             \x20       return value\n\
             \x20   }\n\n",
        );
    }
    if used.ascii {
        out.push_str(
            "    private fun asciiBounded(value: Any?, maxBytes: Int, context: String): String {\n\
             \x20       if (value !is String) {\n\
             \x20           throw ReadContractException(ReadErrorKind.SHAPE, context)\n\
             \x20       }\n\
             \x20       val valid = value.length <= maxBytes && value.all { it in ' '..'~' }\n\
             \x20       if (!valid) {\n\
             \x20           throw ReadContractException(ReadErrorKind.BOUND, context)\n\
             \x20       }\n\
             \x20       return value\n\
             \x20   }\n\n",
        );
    }
    if used.list {
        out.push_str(
            "    private fun <T> decodeList(\n\
             \x20       value: Any?,\n\
             \x20       maxLen: Int,\n\
             \x20       context: String,\n\
             \x20       decodeElement: (Any?, String) -> T,\n\
             \x20   ): List<T> {\n\
             \x20       val items = value as? JSONArray\n\
             \x20           ?: throw ReadContractException(ReadErrorKind.SHAPE, context)\n\
             \x20       if (items.length() > maxLen) {\n\
             \x20           throw ReadContractException(ReadErrorKind.BOUND, context)\n\
             \x20       }\n\
             \x20       return (0 until items.length()).map { index ->\n\
             \x20           val item = items.get(index)\n\
             \x20           decodeElement(if (item == JSONObject.NULL) null else item, context)\n\
             \x20       }\n\
             \x20   }\n\n",
        );
    }
    if used.payload_variant {
        out.push_str(
            "    private fun payload(map: JSONObject, context: String): Any {\n\
             \x20       val value = field(map, \"value\", context)\n\
             \x20           ?: throw ReadContractException(ReadErrorKind.SHAPE, context)\n\
             \x20       return value\n\
             \x20   }\n\n",
        );
    }
    if used.unit_variant {
        out.push_str(
            "    private fun unitPayload(map: JSONObject, context: String) {\n\
             \x20       if (map.has(\"value\") && map.get(\"value\") != JSONObject.NULL) {\n\
             \x20           throw ReadContractException(ReadErrorKind.SHAPE, context)\n\
             \x20       }\n\
             \x20   }\n\n",
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
            "    private fun encodeInteger(value: Long, max: Long, context: String): Long =\n\
             \x20       integer(value, max, context)\n\n",
        );
    }
    if used.hex_fixed {
        out.push_str(
            "    private fun encodeHexFixed(value: String, chars: Int, context: String): String =\n\
             \x20       hexFixed(value, chars, context)\n\n",
        );
    }
    if used.hex_variable {
        out.push_str(
            "    private fun encodeHexVariable(value: String, maxChars: Int, context: String): String =\n\
             \x20       hexVariable(value, maxChars, context)\n\n",
        );
    }
    if used.utf8 {
        out.push_str(
            "    private fun encodeUtf8Bounded(value: String, maxBytes: Int, context: String): String =\n\
             \x20       utf8Bounded(value, maxBytes, context)\n\n",
        );
    }
    if used.ascii {
        out.push_str(
            "    private fun encodeAsciiBounded(value: String, maxBytes: Int, context: String): String =\n\
             \x20       asciiBounded(value, maxBytes, context)\n\n",
        );
    }
    if used.list {
        out.push_str(
            "    private fun <T> encodeList(\n\
             \x20       value: List<T>,\n\
             \x20       maxLen: Int,\n\
             \x20       context: String,\n\
             \x20       encodeElement: (T) -> Any,\n\
             \x20   ): JSONArray {\n\
             \x20       if (value.size > maxLen) {\n\
             \x20           throw ReadContractException(ReadErrorKind.BOUND, context)\n\
             \x20       }\n\
             \x20       val items = JSONArray()\n\
             \x20       for (item in value) {\n\
             \x20           items.put(encodeElement(item))\n\
             \x20       }\n\
             \x20       return items\n\
             \x20   }\n\n",
        );
    }
}

fn decode_expr(ty: &FieldTy, json: &str, context: &str) -> String {
    if let FieldTy::Named(name) = ty {
        return format!("decode{name}({json}, {context})");
    }
    let predicate = scalar_predicate(ty, U63_MAX);
    format!("{}({json}, {}, {context})", predicate.stem, predicate.bound)
}

fn decode_fn(out: &mut String, doc: &SchemaDoc, decl: &Decl) {
    match decl {
        Decl::Enum(decl) => {
            out.push_str(&format!(
                "    private fun decode{name}(value: Any?, context: String): {name} = when (value) {{\n",
                name = decl.name
            ));
            for variant in &decl.variants {
                out.push_str(&format!(
                    "        \"{variant}\" -> {}.{}\n",
                    decl.name,
                    upper_snake(variant)
                ));
            }
            out.push_str(
                "        is String -> throw ReadContractException(ReadErrorKind.UNKNOWN_VARIANT, context)\n\
                 \x20       else -> throw ReadContractException(ReadErrorKind.SHAPE, context)\n\
                 \x20   }\n\n",
            );
        }
        Decl::Struct(decl) => decode_struct_fn(out, doc, decl),
        Decl::Union(decl) => decode_union_fn(out, decl),
    }
}

fn decode_struct_fn(out: &mut String, doc: &SchemaDoc, decl: &StructDecl) {
    out.push_str(&format!(
        "    private fun decode{name}(value: Any?, context: String): {name} {{\n\
         \x20       val map = obj(value, context)\n",
        name = decl.name
    ));
    let keys = decl
        .fields
        .iter()
        .map(|field| format!("\"{}\"", field.name))
        .collect::<Vec<_>>()
        .join(", ");
    out.push_str(&format!("        knownKeys(map, setOf({keys}), context)\n"));
    out.push_str(&format!("        return {}(\n", decl.name));
    for field in &decl.fields {
        if is_envelope_field(doc, &decl.name, &field.name) {
            continue;
        }
        let context = format!("\"{}.{}\"", decl.name, field.name);
        let json = format!("field(map, \"{}\", {context})", field.name);
        let kotlin_name = kotlin_member(&field.name);
        match &field.ty {
            FieldTy::Option(inner) => {
                let inner_expr = decode_expr(inner, "it", &context);
                out.push_str(&format!(
                    "            {kotlin_name} = {json}?.let {{ {inner_expr} }},\n"
                ));
            }
            FieldTy::List { element, max_len } => {
                let FieldTy::Named(element) = element.as_ref() else {
                    unreachable!("list elements are named types");
                };
                out.push_str(&format!(
                    "            {kotlin_name} = decodeList({json}, {max_len}, {context}, ::decode{element}),\n"
                ));
            }
            ty => {
                let expr = decode_expr(ty, &json, &context);
                out.push_str(&format!("            {kotlin_name} = {expr},\n"));
            }
        }
    }
    out.push_str("        )\n    }\n\n");
}

fn decode_union_fn(out: &mut String, decl: &UnionDecl) {
    out.push_str(&format!(
        "    private fun decode{name}(value: Any?, context: String): {name} {{\n\
         \x20       val map = obj(value, context)\n\
         \x20       knownKeys(map, setOf(\"kind\", \"value\"), context)\n\
         \x20       val kind = field(map, \"kind\", context) as? String\n\
         \x20           ?: throw ReadContractException(ReadErrorKind.SHAPE, context)\n\
         \x20       return when (kind) {{\n",
        name = decl.name
    ));
    for variant in &decl.variants {
        let context = format!("\"{}.{}\"", decl.name, variant.name);
        let class = upper_camel(&variant.name);
        match &variant.payload {
            Some(payload) => {
                out.push_str(&format!(
                    "            \"{name}\" -> {union}.{class}(\n\
                     \x20               decode{payload}(payload(map, {context}), {context}),\n\
                     \x20           )\n",
                    name = variant.name,
                    union = decl.name,
                ));
            }
            None => {
                out.push_str(&format!(
                    "            \"{name}\" -> {{\n\
                     \x20               unitPayload(map, {context})\n\
                     \x20               {union}.{class}\n\
                     \x20           }}\n",
                    name = variant.name,
                    union = decl.name,
                ));
            }
        }
    }
    out.push_str(
        "            else -> throw ReadContractException(ReadErrorKind.UNKNOWN_VARIANT, context)\n\
         \x20       }\n\
         \x20   }\n\n",
    );
}

fn encode_expr(ty: &FieldTy, value: &str, context: &str) -> String {
    if let FieldTy::Named(name) = ty {
        return format!("encode{name}({value})");
    }
    let predicate = scalar_predicate(ty, U63_MAX);
    format!(
        "{}({value}, {}, {context})",
        predicate.encode_stem(),
        predicate.bound
    )
}

fn encode_fn(out: &mut String, decl: &Decl) {
    match decl {
        Decl::Enum(decl) => {
            out.push_str(&format!(
                "    private fun encode{name}(value: {name}): String = when (value) {{\n",
                name = decl.name
            ));
            for variant in &decl.variants {
                out.push_str(&format!(
                    "        {}.{} -> \"{variant}\"\n",
                    decl.name,
                    upper_snake(variant)
                ));
            }
            out.push_str("    }\n\n");
        }
        Decl::Struct(decl) => encode_struct_fn(out, decl),
        Decl::Union(decl) => encode_union_fn(out, decl),
    }
}

/// The root frame is never encodable (nothing a frontend originates contains
/// it), so the envelope is stamped by the entry point alone.
fn encode_struct_fn(out: &mut String, decl: &StructDecl) {
    out.push_str(&format!(
        "    private fun encode{name}(value: {name}): JSONObject {{\n\
         \x20       val map = JSONObject()\n",
        name = decl.name
    ));
    for field in &decl.fields {
        let context = format!("\"{}.{}\"", decl.name, field.name);
        let member = format!("value.{}", kotlin_member(&field.name));
        let expr = match &field.ty {
            FieldTy::Option(inner) => {
                let inner_expr = encode_expr(inner, "it", &context);
                format!("{member}?.let {{ {inner_expr} }} ?: JSONObject.NULL")
            }
            FieldTy::List { element, max_len } => {
                let FieldTy::Named(element) = element.as_ref() else {
                    unreachable!("list elements are named types");
                };
                format!("encodeList({member}, {max_len}, {context}, ::encode{element})")
            }
            ty => encode_expr(ty, &member, &context),
        };
        out.push_str(&format!(
            "        map.put(\"{name}\", {expr})\n",
            name = field.name
        ));
    }
    out.push_str("        return map\n    }\n\n");
}

fn encode_union_fn(out: &mut String, decl: &UnionDecl) {
    out.push_str(&format!(
        "    private fun encode{name}(value: {name}): JSONObject = when (value) {{\n",
        name = decl.name
    ));
    for variant in &decl.variants {
        let class = upper_camel(&variant.name);
        match &variant.payload {
            Some(payload) => {
                let call = format!("encode{payload}(value.value)");
                out.push_str(&format!(
                    "        is {union}.{class} ->\n\
                     \x20           JSONObject().put(\"kind\", \"{name}\").put(\"value\", {call})\n",
                    name = variant.name,
                    union = decl.name,
                ));
            }
            None => out.push_str(&format!(
                "        is {union}.{class} -> JSONObject().put(\"kind\", \"{name}\")\n",
                name = variant.name,
                union = decl.name,
            )),
        }
    }
    out.push_str("    }\n\n");
}
