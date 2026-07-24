//! Emits the generated Kotlin read bindings (`generated/kotlin/EnvoixRead.kt`).
//!
//! Decode-only, targeting Android/JVM with the platform-bundled `org.json`.
//! The JSON syntax layer inherits `org.json` leniency; every shape, range,
//! bound, and unknown-variant/field/schema rule is enforced strictly on top,
//! matching the Rust reference codec.

use crate::model::{Decl, FieldTy, SchemaDoc, StructDecl, UnionDecl};

use super::{helper_use, is_envelope_field, kotlin_member, upper_camel, upper_snake};

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
         // every field with a positive minimum still fails its range check).\n\n",
    );
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
    error_type(&mut out);
    for decl in &doc.decls {
        type_decl(&mut out, doc, decl);
    }
    if super::rust::supports_epoch_gate(doc) {
        epoch_gate(&mut out);
    }
    codec(&mut out, doc);
    out
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
}

fn decode_expr(ty: &FieldTy, json: &str, context: &str) -> String {
    match ty {
        FieldTy::U16 => format!("integer({json}, 65535, {context})"),
        FieldTy::U32 => format!("integer({json}, 4294967295, {context})"),
        FieldTy::U63 => format!("integer({json}, Long.MAX_VALUE, {context})"),
        FieldTy::Hex16 => format!("hexFixed({json}, 16, {context})"),
        FieldTy::Hex32 => format!("hexFixed({json}, 32, {context})"),
        FieldTy::Hex64 => format!("hexFixed({json}, 64, {context})"),
        FieldTy::HexVar { max_chars } => format!("hexVariable({json}, {max_chars}, {context})"),
        FieldTy::Str { max_bytes } => format!("utf8Bounded({json}, {max_bytes}, {context})"),
        FieldTy::Ascii { max_bytes } => format!("asciiBounded({json}, {max_bytes}, {context})"),
        FieldTy::Named(name) => format!("decode{name}({json}, {context})"),
        FieldTy::Option(_) | FieldTy::List { .. } => {
            unreachable!("wrapper types are expanded at the field level")
        }
    }
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
