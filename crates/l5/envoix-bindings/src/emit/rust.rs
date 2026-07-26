//! Emits the generated Rust reference codec (`generated/rust/read.rs`).
//!
//! The emitted module walks dynamic `serde_json::Value` trees with the same
//! shape/range/bound checks the native decoders perform, so all four artifacts
//! implement one algorithm. It is included via `include!` and therefore not
//! touched by rustfmt; the emitter owns its formatting.

use crate::model::{
    Decl, DeclKind, FieldDecl, FieldTy, RuleValue, SchemaDoc, StructDecl, UnionDecl,
};

use super::{
    apply_naming, has_secret, helper_use, is_envelope_field, rust_field, snake, upper_camel,
    upper_snake,
};

pub fn module(doc: &SchemaDoc) -> String {
    let mut out = String::new();
    header(&mut out, doc);
    error_type(&mut out);
    for decl in &doc.decls {
        type_decl(&mut out, doc, decl);
    }
    if supports_epoch_gate(doc) {
        epoch_gate(&mut out);
    }
    root_api(&mut out, doc);
    helpers(&mut out, doc);
    for decl in &doc.decls {
        decode_fn(&mut out, doc, decl);
        encode_fn(&mut out, doc, decl);
    }
    apply_naming(out, doc)
}

fn header(out: &mut String, doc: &SchemaDoc) {
    out.push_str("// @generated from schema/read.schema by envoix-bindings. Do not edit;\n");
    out.push_str(
        "// regenerate with `ENVOIX_BINDINGS_REGEN=1 cargo test -p envoix-bindings generated_artifacts`.\n\n",
    );
    out.push_str("use serde_json::{Map, Value};\n\n");
    if has_secret(doc) {
        out.push_str("use envoix_types::Secret;\n\n");
    }
    out.push_str(&format!(
        "pub const READ_SCHEMA_ID: &str = \"{}\";\n",
        doc.id
    ));
    out.push_str(&format!(
        "pub const READ_MAX_FRAME_BYTES: usize = {};\n\n",
        doc.max_frame_bytes
    ));
    rules_consts(out, doc);
    out.push_str("const U63_MAX: u64 = 9_223_372_036_854_775_807;\n\n");
}

fn rules_consts(out: &mut String, doc: &SchemaDoc) {
    if doc.rules.is_empty() {
        return;
    }
    out.push_str("// Contract rules frozen by schema/read.schema.\n");
    for (key, value) in &doc.rules {
        match value {
            RuleValue::Bool(flag) => {
                out.push_str(&format!("pub const {}: bool = {flag};\n", upper_snake(key)))
            }
            RuleValue::Int(bound) => {
                out.push_str(&format!("pub const {}: u32 = {bound};\n", upper_snake(key)))
            }
        }
    }
    out.push('\n');
}

fn error_type(out: &mut String) {
    out.push_str(
        "/// Typed codec failure. It carries only static schema context, never a\n\
         /// fragment of the (possibly hostile) input.\n\
         #[derive(Clone, Copy, Debug, Eq, PartialEq)]\n\
         pub enum ReadError {\n\
         \x20   FrameTooLarge,\n\
         \x20   MalformedJson,\n\
         \x20   UnknownSchema,\n\
         \x20   Shape { context: &'static str },\n\
         \x20   UnknownField { context: &'static str },\n\
         \x20   UnknownVariant { context: &'static str },\n\
         \x20   Range { context: &'static str },\n\
         \x20   Bound { context: &'static str },\n\
         }\n\n",
    );
}

fn rust_ty(ty: &FieldTy) -> String {
    match ty {
        FieldTy::U16 => "u16".to_owned(),
        FieldTy::U32 => "u32".to_owned(),
        FieldTy::U63 => "u64".to_owned(),
        FieldTy::Hex16
        | FieldTy::Hex32
        | FieldTy::Hex64
        | FieldTy::HexVar { .. }
        | FieldTy::Str { .. }
        | FieldTy::Ascii { .. } => "String".to_owned(),
        FieldTy::Named(name) => name.clone(),
        FieldTy::Option(inner) => format!("Option<{}>", rust_ty(inner)),
        FieldTy::List { element, .. } => format!("Vec<{}>", rust_ty(element)),
    }
}

fn rust_field_ty(field: &FieldDecl) -> String {
    if !field.secret {
        return rust_ty(&field.ty);
    }
    match &field.ty {
        // `Option<Secret<T>>`, never `Secret<Option<T>>`: WHETHER a card has a
        // link is not secret — a card without one shows "no shareable link".
        // What is secret is the link when there is one.
        FieldTy::Option(inner) => format!("Option<Secret<{}>>", rust_ty(inner)),
        ty => format!("Secret<{}>", rust_ty(ty)),
    }
}

fn type_decl(out: &mut String, doc: &SchemaDoc, decl: &Decl) {
    match decl {
        Decl::Enum(decl) => {
            out.push_str("#[derive(Clone, Copy, Debug, Eq, PartialEq)]\n");
            out.push_str(&format!("pub enum {} {{\n", decl.name));
            for variant in &decl.variants {
                out.push_str(&format!("    {},\n", upper_camel(variant)));
            }
            out.push_str("}\n\n");
            out.push_str(&format!("impl {} {{\n", decl.name));
            out.push_str(&format!(
                "    pub const ALL: [Self; {}] = [\n",
                decl.variants.len()
            ));
            for variant in &decl.variants {
                out.push_str(&format!("        Self::{},\n", upper_camel(variant)));
            }
            out.push_str("    ];\n}\n\n");
        }
        Decl::Struct(decl) => {
            out.push_str("#[derive(Clone, Debug, Eq, PartialEq)]\n");
            out.push_str(&format!("pub struct {} {{\n", decl.name));
            for field in &decl.fields {
                if is_envelope_field(doc, &decl.name, &field.name) {
                    continue;
                }
                out.push_str(&format!(
                    "    pub {}: {},\n",
                    rust_field(&field.name),
                    rust_field_ty(field)
                ));
            }
            out.push_str("}\n\n");
        }
        Decl::Union(decl) => {
            let _ = doc;
            out.push_str("#[derive(Clone, Debug, Eq, PartialEq)]\n");
            out.push_str(&format!("pub enum {} {{\n", decl.name));
            for variant in &decl.variants {
                match &variant.payload {
                    Some(payload) => {
                        out.push_str(&format!("    {}({payload}),\n", upper_camel(&variant.name)))
                    }
                    None => out.push_str(&format!("    {},\n", upper_camel(&variant.name))),
                }
            }
            out.push_str("}\n\n");
        }
    }
}

pub(crate) fn supports_epoch_gate(doc: &SchemaDoc) -> bool {
    let Some(Decl::Union(body)) = doc.find("ReadBody") else {
        return false;
    };
    let has = |name: &str| body.variants.iter().any(|variant| variant.name == name);
    has("card_update")
        && has("lag")
        && has("closed")
        && matches!(doc.find("CardUpdateKindView"), Some(Decl::Union(kind)) if kind.variants.iter().any(|variant| variant.name == "snapshot"))
}

fn epoch_gate(out: &mut String) {
    out.push_str(
        "/// What a frontend should do with one decoded frame.\n\
         #[derive(Clone, Copy, Debug, Eq, PartialEq)]\n\
         pub enum GateDecision {\n\
         \x20   Deliver,\n\
         \x20   DropStale,\n\
         \x20   ContractBreach,\n\
         }\n\n\
         /// Client-side admission for the per-epoch card stream: one gate per\n\
         /// attachment. Frames from another epoch are stale; every epoch starts\n\
         /// with a snapshot; a lag or close ends the epoch permanently.\n\
         /// Deliberately neither `Clone` nor `Copy`: a gate is identity-bearing\n\
         /// admission state, and a silent copy would fork it.\n\
         #[derive(Debug, Eq, PartialEq)]\n\
         pub struct EpochGate {\n\
         \x20   epoch: u64,\n\
         \x20   saw_snapshot: bool,\n\
         \x20   dead: bool,\n\
         }\n\n\
         impl EpochGate {\n\
         \x20   pub const fn attach(epoch: u64) -> Self {\n\
         \x20       Self {\n\
         \x20           epoch,\n\
         \x20           saw_snapshot: false,\n\
         \x20           dead: false,\n\
         \x20       }\n\
         \x20   }\n\n\
         \x20   pub fn admit(&mut self, frame: &ReadFrame) -> GateDecision {\n\
         \x20       match &frame.body {\n\
         \x20           ReadBody::CardUpdate(update) => {\n\
         \x20               if update.epoch != self.epoch || self.dead {\n\
         \x20                   return GateDecision::DropStale;\n\
         \x20               }\n\
         \x20               match &update.kind {\n\
         \x20                   CardUpdateKindView::Snapshot(_) => {\n\
         \x20                       if self.saw_snapshot {\n\
         \x20                           GateDecision::ContractBreach\n\
         \x20                       } else {\n\
         \x20                           self.saw_snapshot = true;\n\
         \x20                           GateDecision::Deliver\n\
         \x20                       }\n\
         \x20                   }\n\
         \x20                   _ => {\n\
         \x20                       if self.saw_snapshot {\n\
         \x20                           GateDecision::Deliver\n\
         \x20                       } else {\n\
         \x20                           GateDecision::ContractBreach\n\
         \x20                       }\n\
         \x20                   }\n\
         \x20               }\n\
         \x20           }\n\
         \x20           ReadBody::Lag(lag) => self.terminate(lag.epoch),\n\
         \x20           ReadBody::Closed(closed) => self.terminate(closed.epoch),\n\
         \x20           _ => GateDecision::Deliver,\n\
         \x20       }\n\
         \x20   }\n\n\
         \x20   fn terminate(&mut self, epoch: u64) -> GateDecision {\n\
         \x20       if epoch == self.epoch && !self.dead {\n\
         \x20           self.dead = true;\n\
         \x20           GateDecision::Deliver\n\
         \x20       } else {\n\
         \x20           GateDecision::DropStale\n\
         \x20       }\n\
         \x20   }\n\
         }\n\n",
    );
}

fn root_api(out: &mut String, doc: &SchemaDoc) {
    let root_snake = snake(&doc.root);
    out.push_str(&format!(
        "/// Decodes and validates one frame. Every failure is a typed\n\
         /// [`ReadError`]; no input, however hostile, panics or misparses.\n\
         pub fn decode_{root_snake}(bytes: &[u8]) -> Result<{}, ReadError> {{\n\
         \x20   if bytes.len() > READ_MAX_FRAME_BYTES {{\n\
         \x20       return Err(ReadError::FrameTooLarge);\n\
         \x20   }}\n\
         \x20   let value = strict_json(bytes)?;\n\
         \x20   decode_{root_snake}_value(&value, \"{root}\")\n\
         }}\n\n\
         /// Encodes one frame, stamping the schema envelope and enforcing the\n\
         /// same bounds the decoder checks.\n\
         pub fn encode_{root_snake}(frame: &{root}) -> Result<Vec<u8>, ReadError> {{\n\
         \x20   let value = encode_{root_snake}_value(frame)?;\n\
         \x20   let bytes = serde_json::to_vec(&value).map_err(|_| ReadError::MalformedJson)?;\n\
         \x20   if bytes.len() > READ_MAX_FRAME_BYTES {{\n\
         \x20       return Err(ReadError::FrameTooLarge);\n\
         \x20   }}\n\
         \x20   Ok(bytes)\n\
         }}\n\n",
        doc.root,
        root = doc.root,
    ));
}

fn helpers(out: &mut String, doc: &SchemaDoc) {
    let used = helper_use(doc);
    out.push_str(
        "/// Parses JSON while rejecting duplicate object keys at any depth and\n\
         /// trailing input. A duplicated key is the smuggling shape: a first-wins\n\
         /// upstream parser would see a different value than a last-wins one applies.\n\
         fn strict_json(bytes: &[u8]) -> Result<Value, ReadError> {\n\
         \x20   struct StrictValue;\n\
         \n\
         \x20   impl<'de> serde::de::DeserializeSeed<'de> for StrictValue {\n\
         \x20       type Value = Value;\n\
         \n\
         \x20       fn deserialize<D: serde::Deserializer<'de>>(self, deserializer: D) -> Result<Value, D::Error> {\n\
         \x20           deserializer.deserialize_any(self)\n\
         \x20       }\n\
         \x20   }\n\
         \n\
         \x20   impl<'de> serde::de::Visitor<'de> for StrictValue {\n\
         \x20       type Value = Value;\n\
         \n\
         \x20       fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {\n\
         \x20           formatter.write_str(\"a json value without duplicate object keys\")\n\
         \x20       }\n\
         \n\
         \x20       fn visit_bool<E>(self, value: bool) -> Result<Value, E> {\n\
         \x20           Ok(Value::Bool(value))\n\
         \x20       }\n\
         \n\
         \x20       fn visit_i64<E>(self, value: i64) -> Result<Value, E> {\n\
         \x20           Ok(Value::from(value))\n\
         \x20       }\n\
         \n\
         \x20       fn visit_u64<E>(self, value: u64) -> Result<Value, E> {\n\
         \x20           Ok(Value::from(value))\n\
         \x20       }\n\
         \n\
         \x20       fn visit_f64<E>(self, value: f64) -> Result<Value, E> {\n\
         \x20           Ok(Value::from(value))\n\
         \x20       }\n\
         \n\
         \x20       fn visit_str<E: serde::de::Error>(self, value: &str) -> Result<Value, E> {\n\
         \x20           Ok(Value::from(value))\n\
         \x20       }\n\
         \n\
         \x20       fn visit_unit<E>(self) -> Result<Value, E> {\n\
         \x20           Ok(Value::Null)\n\
         \x20       }\n\
         \n\
         \x20       fn visit_seq<A: serde::de::SeqAccess<'de>>(self, mut access: A) -> Result<Value, A::Error> {\n\
         \x20           let mut items = Vec::new();\n\
         \x20           while let Some(item) = access.next_element_seed(StrictValue)? {\n\
         \x20               items.push(item);\n\
         \x20           }\n\
         \x20           Ok(Value::Array(items))\n\
         \x20       }\n\
         \n\
         \x20       fn visit_map<A: serde::de::MapAccess<'de>>(self, mut access: A) -> Result<Value, A::Error> {\n\
         \x20           let mut map = Map::new();\n\
         \x20           while let Some(key) = access.next_key::<String>()? {\n\
         \x20               let value = access.next_value_seed(StrictValue)?;\n\
         \x20               if map.insert(key, value).is_some() {\n\
         \x20                   return Err(serde::de::Error::custom(\"duplicate object key\"));\n\
         \x20               }\n\
         \x20           }\n\
         \x20           Ok(Value::Object(map))\n\
         \x20       }\n\
         \x20   }\n\
         \n\
         \x20   let mut deserializer = serde_json::Deserializer::from_slice(bytes);\n\
         \x20   let value = serde::de::DeserializeSeed::deserialize(StrictValue, &mut deserializer)\n\
         \x20       .map_err(|_| ReadError::MalformedJson)?;\n\
         \x20   deserializer.end().map_err(|_| ReadError::MalformedJson)?;\n\
         \x20   Ok(value)\n\
         }\n\n\
         fn frame_object<'a>(value: &'a Value, context: &'static str) -> Result<&'a Map<String, Value>, ReadError> {\n\
         \x20   value.as_object().ok_or(ReadError::Shape { context })\n\
         }\n\n\
         fn known_keys(map: &Map<String, Value>, allowed: &[&str], context: &'static str) -> Result<(), ReadError> {\n\
         \x20   for key in map.keys() {\n\
         \x20       if !allowed.contains(&key.as_str()) {\n\
         \x20           return Err(ReadError::UnknownField { context });\n\
         \x20       }\n\
         \x20   }\n\
         \x20   Ok(())\n\
         }\n\n\
         fn field<'a>(map: &'a Map<String, Value>, key: &str, context: &'static str) -> Result<&'a Value, ReadError> {\n\
         \x20   map.get(key).ok_or(ReadError::Shape { context })\n\
         }\n\n",
    );
    if used.integer {
        out.push_str(
            "fn integer(value: &Value, max: u64, context: &'static str) -> Result<u64, ReadError> {\n\
             \x20   let number = value.as_u64().ok_or(ReadError::Shape { context })?;\n\
             \x20   if number > max {\n\
             \x20       return Err(ReadError::Range { context });\n\
             \x20   }\n\
             \x20   Ok(number)\n\
             }\n\n\
             fn encode_u63(number: u64, context: &'static str) -> Result<Value, ReadError> {\n\
             \x20   if number > U63_MAX {\n\
             \x20       return Err(ReadError::Range { context });\n\
             \x20   }\n\
             \x20   Ok(Value::from(number))\n\
             }\n\n",
        );
    }
    if used.u16 {
        out.push_str(
            "fn integer_u16(value: &Value, context: &'static str) -> Result<u16, ReadError> {\n\
             \x20   let number = integer(value, 65_535, context)?;\n\
             \x20   u16::try_from(number).map_err(|_| ReadError::Range { context })\n\
             }\n\n",
        );
    }
    if used.u32 {
        out.push_str(
            "fn integer_u32(value: &Value, context: &'static str) -> Result<u32, ReadError> {\n\
             \x20   let number = integer(value, 4_294_967_295, context)?;\n\
             \x20   u32::try_from(number).map_err(|_| ReadError::Range { context })\n\
             }\n\n",
        );
    }
    if used.hex_fixed || used.hex_variable {
        out.push_str(
            "fn hex_chars(text: &str) -> bool {\n\
             \x20   text.bytes().all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))\n\
             }\n\n",
        );
    }
    if used.hex_fixed {
        out.push_str(
            "fn hex_fixed(value: &Value, chars: usize, context: &'static str) -> Result<String, ReadError> {\n\
             \x20   let text = value.as_str().ok_or(ReadError::Shape { context })?;\n\
             \x20   encode_hex_fixed(text, chars, context)?;\n\
             \x20   Ok(text.to_owned())\n\
             }\n\n\
             fn encode_hex_fixed(text: &str, chars: usize, context: &'static str) -> Result<Value, ReadError> {\n\
             \x20   if text.len() != chars || !hex_chars(text) {\n\
             \x20       return Err(ReadError::Bound { context });\n\
             \x20   }\n\
             \x20   Ok(Value::from(text))\n\
             }\n\n",
        );
    }
    if used.hex_variable {
        out.push_str(
            "fn hex_variable(value: &Value, max_chars: usize, context: &'static str) -> Result<String, ReadError> {\n\
             \x20   let text = value.as_str().ok_or(ReadError::Shape { context })?;\n\
             \x20   encode_hex_variable(text, max_chars, context)?;\n\
             \x20   Ok(text.to_owned())\n\
             }\n\n\
             fn encode_hex_variable(text: &str, max_chars: usize, context: &'static str) -> Result<Value, ReadError> {\n\
             \x20   let valid = !text.is_empty() && text.len().is_multiple_of(2) && text.len() <= max_chars && hex_chars(text);\n\
             \x20   if !valid {\n\
             \x20       return Err(ReadError::Bound { context });\n\
             \x20   }\n\
             \x20   Ok(Value::from(text))\n\
             }\n\n",
        );
    }
    if used.utf8 {
        out.push_str(
            "fn utf8_bounded(value: &Value, max_bytes: usize, context: &'static str) -> Result<String, ReadError> {\n\
             \x20   let text = value.as_str().ok_or(ReadError::Shape { context })?;\n\
             \x20   encode_utf8_bounded(text, max_bytes, context)?;\n\
             \x20   Ok(text.to_owned())\n\
             }\n\n\
             fn encode_utf8_bounded(text: &str, max_bytes: usize, context: &'static str) -> Result<Value, ReadError> {\n\
             \x20   if text.len() > max_bytes {\n\
             \x20       return Err(ReadError::Bound { context });\n\
             \x20   }\n\
             \x20   Ok(Value::from(text))\n\
             }\n\n",
        );
    }
    if used.ascii {
        out.push_str(
            "fn ascii_bounded(value: &Value, max_bytes: usize, context: &'static str) -> Result<String, ReadError> {\n\
             \x20   let text = value.as_str().ok_or(ReadError::Shape { context })?;\n\
             \x20   encode_ascii_bounded(text, max_bytes, context)?;\n\
             \x20   Ok(text.to_owned())\n\
             }\n\n\
             fn encode_ascii_bounded(text: &str, max_bytes: usize, context: &'static str) -> Result<Value, ReadError> {\n\
             \x20   let valid = text.len() <= max_bytes && text.bytes().all(|byte| (0x20..=0x7e).contains(&byte));\n\
             \x20   if !valid {\n\
             \x20       return Err(ReadError::Bound { context });\n\
             \x20   }\n\
             \x20   Ok(Value::from(text))\n\
             }\n\n",
        );
    }
    if used.payload_variant {
        out.push_str(
            "fn payload<'a>(map: &'a Map<String, Value>, context: &'static str) -> Result<&'a Value, ReadError> {\n\
             \x20   match map.get(\"value\") {\n\
             \x20       Some(value) if !value.is_null() => Ok(value),\n\
             \x20       _ => Err(ReadError::Shape { context }),\n\
             \x20   }\n\
             }\n\n",
        );
    }
    if used.unit_variant {
        out.push_str(
            "fn unit_payload(map: &Map<String, Value>, context: &'static str) -> Result<(), ReadError> {\n\
             \x20   match map.get(\"value\") {\n\
             \x20       None | Some(Value::Null) => Ok(()),\n\
             \x20       Some(_) => Err(ReadError::Shape { context }),\n\
             \x20   }\n\
             }\n\n",
        );
    }
}

fn decode_expr(doc: &SchemaDoc, ty: &FieldTy, json: &str, context: &str) -> String {
    match ty {
        FieldTy::U16 => format!("integer_u16({json}, {context})?"),
        FieldTy::U32 => format!("integer_u32({json}, {context})?"),
        FieldTy::U63 => format!("integer({json}, U63_MAX, {context})?"),
        FieldTy::Hex16 => format!("hex_fixed({json}, 16, {context})?"),
        FieldTy::Hex32 => format!("hex_fixed({json}, 32, {context})?"),
        FieldTy::Hex64 => format!("hex_fixed({json}, 64, {context})?"),
        FieldTy::HexVar { max_chars } => format!("hex_variable({json}, {max_chars}, {context})?"),
        FieldTy::Str { max_bytes } => format!("utf8_bounded({json}, {max_bytes}, {context})?"),
        FieldTy::Ascii { max_bytes } => format!("ascii_bounded({json}, {max_bytes}, {context})?"),
        FieldTy::Named(name) => {
            format!("decode_{}_value({json}, {context})?", snake(name))
        }
        FieldTy::Option(_) | FieldTy::List { .. } => {
            let _ = doc;
            unreachable!("wrapper types are expanded at the field level")
        }
    }
}

fn encode_expr(doc: &SchemaDoc, ty: &FieldTy, value: &str, context: &str) -> String {
    match ty {
        FieldTy::U16 | FieldTy::U32 => format!("Value::from({value})"),
        FieldTy::U63 => format!("encode_u63({value}, {context})?"),
        FieldTy::Hex16 => format!("encode_hex_fixed(&{value}, 16, {context})?"),
        FieldTy::Hex32 => format!("encode_hex_fixed(&{value}, 32, {context})?"),
        FieldTy::Hex64 => format!("encode_hex_fixed(&{value}, 64, {context})?"),
        FieldTy::HexVar { max_chars } => {
            format!("encode_hex_variable(&{value}, {max_chars}, {context})?")
        }
        FieldTy::Str { max_bytes } => {
            format!("encode_utf8_bounded(&{value}, {max_bytes}, {context})?")
        }
        FieldTy::Ascii { max_bytes } => {
            format!("encode_ascii_bounded(&{value}, {max_bytes}, {context})?")
        }
        FieldTy::Named(name) => {
            let call = format!("encode_{}_value(&{value})", snake(name));
            match doc.kind_of(name) {
                Some(DeclKind::Enum) => call,
                _ => format!("{call}?"),
            }
        }
        FieldTy::Option(_) | FieldTy::List { .. } => {
            unreachable!("wrapper types are expanded at the field level")
        }
    }
}

fn decode_fn(out: &mut String, doc: &SchemaDoc, decl: &Decl) {
    match decl {
        Decl::Enum(decl) => {
            let fn_name = snake(&decl.name);
            out.push_str(&format!(
                "fn decode_{fn_name}_value(value: &Value, context: &'static str) -> Result<{}, ReadError> {{\n",
                decl.name
            ));
            out.push_str(
                "    let text = value.as_str().ok_or(ReadError::Shape { context })?;\n    match text {\n",
            );
            for variant in &decl.variants {
                out.push_str(&format!(
                    "        \"{variant}\" => Ok({}::{}),\n",
                    decl.name,
                    upper_camel(variant)
                ));
            }
            out.push_str("        _ => Err(ReadError::UnknownVariant { context }),\n    }\n}\n\n");
        }
        Decl::Struct(decl) => decode_struct_fn(out, doc, decl),
        Decl::Union(decl) => decode_union_fn(out, doc, decl),
    }
}

fn decode_struct_fn(out: &mut String, doc: &SchemaDoc, decl: &StructDecl) {
    let fn_name = snake(&decl.name);
    out.push_str(&format!(
        "fn decode_{fn_name}_value(value: &Value, context: &'static str) -> Result<{}, ReadError> {{\n",
        decl.name
    ));
    out.push_str("    let map = frame_object(value, context)?;\n");
    if decl.name == doc.root {
        out.push_str(&format!(
            "    match map.get(\"schema\").and_then(Value::as_str) {{\n\
             \x20       Some(schema) if schema == READ_SCHEMA_ID => {{}}\n\
             \x20       Some(_) => return Err(ReadError::UnknownSchema),\n\
             \x20       None => return Err(ReadError::Shape {{ context: \"{}.schema\" }}),\n\
             \x20   }}\n",
            decl.name
        ));
    }
    let keys = decl
        .fields
        .iter()
        .map(|field| format!("\"{}\"", field.name))
        .collect::<Vec<_>>()
        .join(", ");
    out.push_str(&format!("    known_keys(map, &[{keys}], context)?;\n"));
    for field in &decl.fields {
        if is_envelope_field(doc, &decl.name, &field.name) {
            continue;
        }
        decode_struct_field(out, doc, decl, field);
    }
    let names = decl
        .fields
        .iter()
        .filter(|field| !is_envelope_field(doc, &decl.name, &field.name))
        .map(|field| rust_field(&field.name))
        .collect::<Vec<_>>()
        .join(",\n        ");
    out.push_str(&format!(
        "    Ok({} {{\n        {names},\n    }})\n}}\n\n",
        decl.name
    ));
}

fn decode_struct_field(out: &mut String, doc: &SchemaDoc, decl: &StructDecl, field: &FieldDecl) {
    let context = format!("\"{}.{}\"", decl.name, field.name);
    let json = format!("field(map, \"{}\", {context})?", field.name);
    match &field.ty {
        FieldTy::Option(inner) => {
            let inner_expr = decode_expr(doc, inner, "present", &context);
            let inner_expr = if field.secret {
                format!("Secret::new({inner_expr})")
            } else {
                inner_expr
            };
            out.push_str(&format!(
                "    let {name} = match {json} {{\n\
                 \x20       Value::Null => None,\n\
                 \x20       present => Some({inner_expr}),\n\
                 \x20   }};\n",
                name = rust_field(&field.name),
            ));
        }
        FieldTy::List { element, max_len } => {
            let element_expr = decode_expr(doc, element, "item", &context);
            out.push_str(&format!(
                "    let {name} = {{\n\
                 \x20       let items = {json}.as_array().ok_or(ReadError::Shape {{ context: {context} }})?;\n\
                 \x20       if items.len() > {max_len} {{\n\
                 \x20           return Err(ReadError::Bound {{ context: {context} }});\n\
                 \x20       }}\n\
                 \x20       let mut collected = Vec::with_capacity(items.len());\n\
                 \x20       for item in items {{\n\
                 \x20           collected.push({element_expr});\n\
                 \x20       }}\n\
                 \x20       collected\n\
                 \x20   }};\n",
                name = rust_field(&field.name),
            ));
        }
        ty => {
            let expr = decode_expr(doc, ty, &json, &context);
            let expr = if field.secret {
                format!("Secret::new({expr})")
            } else {
                expr
            };
            out.push_str(&format!("    let {} = {expr};\n", rust_field(&field.name)));
        }
    }
}

fn decode_union_fn(out: &mut String, doc: &SchemaDoc, decl: &UnionDecl) {
    let fn_name = snake(&decl.name);
    out.push_str(&format!(
        "fn decode_{fn_name}_value(value: &Value, context: &'static str) -> Result<{}, ReadError> {{\n",
        decl.name
    ));
    out.push_str("    let map = frame_object(value, context)?;\n");
    out.push_str("    known_keys(map, &[\"kind\", \"value\"], context)?;\n");
    out.push_str(
        "    let kind = field(map, \"kind\", context)?\n        .as_str()\n        .ok_or(ReadError::Shape { context })?;\n",
    );
    out.push_str("    match kind {\n");
    for variant in &decl.variants {
        let variant_context = format!("\"{}.{}\"", decl.name, variant.name);
        match &variant.payload {
            Some(payload) => {
                let payload_expr = format!("payload(map, {variant_context})?");
                let decode = decode_expr(
                    doc,
                    &FieldTy::Named(payload.clone()),
                    &payload_expr,
                    &variant_context,
                );
                out.push_str(&format!(
                    "        \"{name}\" => Ok({}::{}({decode})),\n",
                    decl.name,
                    upper_camel(&variant.name),
                    name = variant.name,
                ));
            }
            None => {
                out.push_str(&format!(
                    "        \"{name}\" => {{\n\
                     \x20           unit_payload(map, {variant_context})?;\n\
                     \x20           Ok({}::{})\n\
                     \x20       }}\n",
                    decl.name,
                    upper_camel(&variant.name),
                    name = variant.name,
                ));
            }
        }
    }
    out.push_str("        _ => Err(ReadError::UnknownVariant { context }),\n    }\n}\n\n");
}

fn encode_fn(out: &mut String, doc: &SchemaDoc, decl: &Decl) {
    match decl {
        Decl::Enum(decl) => {
            let fn_name = snake(&decl.name);
            out.push_str(&format!(
                "fn encode_{fn_name}_value(value: &{}) -> Value {{\n    Value::from(match value {{\n",
                decl.name
            ));
            for variant in &decl.variants {
                out.push_str(&format!(
                    "        {}::{} => \"{variant}\",\n",
                    decl.name,
                    upper_camel(variant)
                ));
            }
            out.push_str("    })\n}\n\n");
        }
        Decl::Struct(decl) => encode_struct_fn(out, doc, decl),
        Decl::Union(decl) => encode_union_fn(out, doc, decl),
    }
}

fn encode_struct_fn(out: &mut String, doc: &SchemaDoc, decl: &StructDecl) {
    let fn_name = snake(&decl.name);
    out.push_str(&format!(
        "fn encode_{fn_name}_value(value: &{}) -> Result<Value, ReadError> {{\n",
        decl.name
    ));
    out.push_str("    let mut map = Map::new();\n");
    for field in &decl.fields {
        if is_envelope_field(doc, &decl.name, &field.name) {
            out.push_str("    map.insert(\"schema\".to_owned(), Value::from(READ_SCHEMA_ID));\n");
            continue;
        }
        encode_struct_field(out, doc, decl, field);
    }
    out.push_str("    Ok(Value::Object(map))\n}\n\n");
}

fn encode_struct_field(out: &mut String, doc: &SchemaDoc, decl: &StructDecl, field: &FieldDecl) {
    let context = format!("\"{}.{}\"", decl.name, field.name);
    let access = format!("value.{}", rust_field(&field.name));
    match &field.ty {
        FieldTy::Option(inner) => {
            let inner_expr = match inner.as_ref() {
                FieldTy::Named(name) => {
                    let call = format!("encode_{}_value(inner)", snake(name));
                    match doc.kind_of(name) {
                        Some(DeclKind::Enum) => call,
                        _ => format!("{call}?"),
                    }
                }
                scalar => encode_scalar_ref(scalar, "inner", &context),
            };
            // The one place a secret is meant to leave: onto the wire.
            let inner_expr = if field.secret {
                inner_expr.replace("inner", "inner.expose()")
            } else {
                inner_expr
            };
            out.push_str(&format!(
                "    map.insert(\n\
                 \x20       \"{name}\".to_owned(),\n\
                 \x20       match &{access} {{\n\
                 \x20           None => Value::Null,\n\
                 \x20           Some(inner) => {inner_expr},\n\
                 \x20       }},\n\
                 \x20   );\n",
                name = field.name,
            ));
        }
        FieldTy::List { element, max_len } => {
            let element_call = match element.as_ref() {
                FieldTy::Named(name) => {
                    let call = format!("encode_{}_value(item)", snake(name));
                    match doc.kind_of(name) {
                        Some(DeclKind::Enum) => call,
                        _ => format!("{call}?"),
                    }
                }
                _ => unreachable!("list elements are named types"),
            };
            out.push_str(&format!(
                "    map.insert(\"{name}\".to_owned(), {{\n\
                 \x20       if {access}.len() > {max_len} {{\n\
                 \x20           return Err(ReadError::Bound {{ context: {context} }});\n\
                 \x20       }}\n\
                 \x20       let mut items = Vec::with_capacity({access}.len());\n\
                 \x20       for item in &{access} {{\n\
                 \x20           items.push({element_call});\n\
                 \x20       }}\n\
                 \x20       Value::Array(items)\n\
                 \x20   }});\n",
                name = field.name,
            ));
        }
        ty => {
            let expr = if field.secret {
                let FieldTy::Str { max_bytes } = ty else {
                    unreachable!("the parser permits secret bounded strings only")
                };
                format!("encode_utf8_bounded({access}.expose(), {max_bytes}, {context})?")
            } else {
                encode_expr(doc, ty, &access, &context)
            };
            out.push_str(&format!(
                "    map.insert(\"{name}\".to_owned(), {expr});\n",
                name = field.name,
            ));
        }
    }
}

fn encode_scalar_ref(ty: &FieldTy, value: &str, context: &str) -> String {
    match ty {
        FieldTy::U16 | FieldTy::U32 => format!("Value::from(*{value})"),
        FieldTy::U63 => format!("encode_u63(*{value}, {context})?"),
        FieldTy::Hex16 => format!("encode_hex_fixed({value}, 16, {context})?"),
        FieldTy::Hex32 => format!("encode_hex_fixed({value}, 32, {context})?"),
        FieldTy::Hex64 => format!("encode_hex_fixed({value}, 64, {context})?"),
        FieldTy::HexVar { max_chars } => {
            format!("encode_hex_variable({value}, {max_chars}, {context})?")
        }
        FieldTy::Str { max_bytes } => {
            format!("encode_utf8_bounded({value}, {max_bytes}, {context})?")
        }
        FieldTy::Ascii { max_bytes } => {
            format!("encode_ascii_bounded({value}, {max_bytes}, {context})?")
        }
        FieldTy::Named(_) | FieldTy::Option(_) | FieldTy::List { .. } => {
            unreachable!("handled by the caller")
        }
    }
}

fn encode_union_fn(out: &mut String, doc: &SchemaDoc, decl: &UnionDecl) {
    let fn_name = snake(&decl.name);
    out.push_str(&format!(
        "fn encode_{fn_name}_value(value: &{}) -> Result<Value, ReadError> {{\n",
        decl.name
    ));
    out.push_str("    let mut map = Map::new();\n    match value {\n");
    for variant in &decl.variants {
        match &variant.payload {
            Some(payload) => {
                let call = format!("encode_{}_value(payload)", snake(payload));
                let call = match doc.kind_of(payload) {
                    Some(DeclKind::Enum) => call,
                    _ => format!("{call}?"),
                };
                out.push_str(&format!(
                    "        {}::{}(payload) => {{\n\
                     \x20           map.insert(\"kind\".to_owned(), Value::from(\"{name}\"));\n\
                     \x20           map.insert(\"value\".to_owned(), {call});\n\
                     \x20       }}\n",
                    decl.name,
                    upper_camel(&variant.name),
                    name = variant.name,
                ));
            }
            None => {
                out.push_str(&format!(
                    "        {}::{} => {{\n\
                     \x20           map.insert(\"kind\".to_owned(), Value::from(\"{name}\"));\n\
                     \x20       }}\n",
                    decl.name,
                    upper_camel(&variant.name),
                    name = variant.name,
                ));
            }
        }
    }
    out.push_str("    }\n    Ok(Value::Object(map))\n}\n\n");
}
