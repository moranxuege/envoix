//! Deterministic emitters from one [`SchemaDoc`](crate::model::SchemaDoc) to
//! the four generated read-binding artifacts.
//!
//! Determinism: every emitter walks declarations in schema file order and
//! builds plain strings — no timestamps, no randomness, no map iteration.

pub mod dart;
pub mod kotlin;
pub mod rust;
pub mod swift;

use crate::model::{Decl, FieldTy, SchemaDoc};

/// One scalar predicate, described once for both halves of a native codec.
///
/// The decode call is `{stem}(json, bound, context)` and the encode call is
/// `encode{Stem}(value, bound, context)`; both are rendered from this single
/// value, so a bound cannot be tightened or loosened on one side only.
pub(crate) struct ScalarPredicate {
    /// The decode helper's name.
    pub stem: &'static str,
    /// The bound argument, as the target language spells it.
    pub bound: String,
}

impl ScalarPredicate {
    /// `integer` → `encodeInteger`.
    pub fn encode_stem(&self) -> String {
        format!("encode{}", upper_camel(self.stem))
    }
}

/// The predicate for a scalar field type. `u63_max` is how the target language
/// names 2^63-1 (the one bound no artifact writes as a literal); every other
/// bound comes straight from the schema.
pub(crate) fn scalar_predicate(ty: &FieldTy, u63_max: &str) -> ScalarPredicate {
    let (stem, bound) = match ty {
        FieldTy::U16 => ("integer", "65535".to_owned()),
        FieldTy::U32 => ("integer", "4294967295".to_owned()),
        FieldTy::U63 => ("integer", u63_max.to_owned()),
        FieldTy::Hex16 => ("hexFixed", "16".to_owned()),
        FieldTy::Hex32 => ("hexFixed", "32".to_owned()),
        FieldTy::Hex64 => ("hexFixed", "64".to_owned()),
        FieldTy::HexVar { max_chars } => ("hexVariable", max_chars.to_string()),
        FieldTy::Str { max_bytes } => ("utf8Bounded", max_bytes.to_string()),
        FieldTy::Ascii { max_bytes } => ("asciiBounded", max_bytes.to_string()),
        FieldTy::Named(_) | FieldTy::Option(_) | FieldTy::List { .. } => {
            unreachable!("not a scalar predicate")
        }
    };
    ScalarPredicate { stem, bound }
}

/// Rust keywords that field names must escape with `r#`. Member names that
/// cannot be raw identifiers at all (`crate`, `self`, `super`) are rejected by
/// the schema parser instead.
const RUST_KEYWORDS: &[&str] = &[
    "abstract", "as", "async", "await", "become", "box", "break", "const", "continue", "do", "dyn",
    "else", "enum", "extern", "false", "final", "fn", "for", "if", "impl", "in", "let", "loop",
    "macro", "match", "mod", "move", "mut", "override", "priv", "pub", "ref", "return", "static",
    "struct", "trait", "true", "try", "type", "typeof", "unsafe", "unsized", "use", "virtual",
    "where", "while", "yield",
];

/// Dart reserved words, which can never be identifiers; the finisher renames
/// with a trailing underscore (the parser rejects schema names ending in `_`,
/// so the rename cannot collide).
const DART_RESERVED: &[&str] = &[
    "assert", "break", "case", "catch", "class", "const", "continue", "default", "do", "else",
    "enum", "extends", "false", "final", "finally", "for", "if", "in", "is", "new", "null",
    "rethrow", "return", "super", "switch", "this", "throw", "true", "try", "var", "void", "while",
    "with",
];

/// Kotlin hard keywords, escaped with backticks.
const KOTLIN_HARD_KEYWORDS: &[&str] = &[
    "as",
    "break",
    "class",
    "continue",
    "do",
    "else",
    "false",
    "for",
    "fun",
    "if",
    "in",
    "interface",
    "is",
    "null",
    "object",
    "package",
    "return",
    "super",
    "this",
    "throw",
    "true",
    "try",
    "typealias",
    "typeof",
    "val",
    "var",
    "when",
    "while",
];

/// Swift reserved keywords a lower-camel member could collide with, escaped
/// with backticks.
const SWIFT_RESERVED: &[&str] = &[
    "as",
    "associatedtype",
    "break",
    "case",
    "catch",
    "class",
    "continue",
    "default",
    "defer",
    "deinit",
    "do",
    "else",
    "enum",
    "extension",
    "fallthrough",
    "false",
    "fileprivate",
    "for",
    "func",
    "guard",
    "if",
    "import",
    "in",
    "init",
    "inout",
    "internal",
    "is",
    "let",
    "nil",
    "open",
    "operator",
    "precedencegroup",
    "private",
    "protocol",
    "public",
    "repeat",
    "rethrows",
    "return",
    "self",
    "static",
    "struct",
    "subscript",
    "super",
    "switch",
    "throw",
    "throws",
    "true",
    "try",
    "typealias",
    "var",
    "where",
    "while",
];

/// A schema member as a Rust field identifier (`type` → `r#type`).
pub(crate) fn rust_field(member: &str) -> String {
    if RUST_KEYWORDS.contains(&member) {
        format!("r#{member}")
    } else {
        member.to_owned()
    }
}

/// A schema member as a Dart identifier (`in` → `in_`).
pub(crate) fn dart_member(member: &str) -> String {
    let camel = lower_camel(member);
    if DART_RESERVED.contains(&camel.as_str()) {
        format!("{camel}_")
    } else {
        camel
    }
}

/// A schema member as a Kotlin identifier (`in` → `` `in` ``).
pub(crate) fn kotlin_member(member: &str) -> String {
    let camel = lower_camel(member);
    if KOTLIN_HARD_KEYWORDS.contains(&camel.as_str()) {
        format!("`{camel}`")
    } else {
        camel
    }
}

/// A schema member as a Swift identifier (`internal` → `` `internal` ``).
pub(crate) fn swift_member(member: &str) -> String {
    let camel = lower_camel(member);
    if SWIFT_RESERVED.contains(&camel.as_str()) {
        format!("`{camel}`")
    } else {
        camel
    }
}

/// `peer_lost` → `PeerLost`.
pub(crate) fn upper_camel(member: &str) -> String {
    let mut output = String::new();
    let mut upper_next = true;
    for c in member.chars() {
        if c == '_' {
            upper_next = true;
        } else if upper_next {
            output.push(c.to_ascii_uppercase());
            upper_next = false;
        } else {
            output.push(c);
        }
    }
    output
}

/// `peer_lost` → `peerLost`.
pub(crate) fn lower_camel(member: &str) -> String {
    let camel = upper_camel(member);
    let mut chars = camel.chars();
    match chars.next() {
        Some(first) => first.to_ascii_lowercase().to_string() + chars.as_str(),
        None => camel,
    }
}

/// `peer_lost` → `PEER_LOST`.
pub(crate) fn upper_snake(member: &str) -> String {
    member.to_ascii_uppercase()
}

/// `OutcomeView` → `outcome_view`.
pub(crate) fn snake(type_name: &str) -> String {
    let mut output = String::new();
    for c in type_name.chars() {
        if c.is_ascii_uppercase() {
            if !output.is_empty() {
                output.push('_');
            }
            output.push(c.to_ascii_lowercase());
        } else {
            output.push(c);
        }
    }
    output
}

/// The root frame's leading `schema` field is the codec-owned envelope: every
/// emitter stamps/verifies it in generated code and omits it from the
/// in-memory model, so a wrong-schema frame is unrepresentable.
pub(crate) fn is_envelope_field(doc: &SchemaDoc, decl: &str, field: &str) -> bool {
    decl == doc.root && field == "schema"
}

/// The artifact stem from the schema id: `envoix/binding/command/1` → `command`.
pub(crate) fn schema_stem(doc: &SchemaDoc) -> &str {
    doc.id
        .split('/')
        .nth(2)
        .expect("schema id shape is validated by the parser")
}

/// The scaffold identifiers [`apply_naming`] rewrites for non-read schemas,
/// longest first so `ReadErrorKind` never half-renames via `ReadError`. The
/// schema parser rejects any declared name whose emitted forms match one of
/// these, so the whole-text rename can never touch a schema-declared name.
pub(crate) const SCAFFOLD_TOKENS: &[&str] = &[
    "ReadContractException",
    "schema/read.schema",
    "READ_MAX_FRAME_BYTES",
    "readMaxFrameBytes",
    "ReadContractError",
    "EnvoixReadCodec",
    "READ_SCHEMA_ID",
    "ReadErrorKind",
    "readSchemaId",
    "ReadError",
];

/// Every emitter writes the codec scaffolding with its read-contract names;
/// for any other schema this pass renames exactly the [`SCAFFOLD_TOKENS`].
/// Type names like `ReadFrame` come from the schema itself and are never
/// touched. The read schema is the identity case, which the read drift test
/// pins byte-exactly.
pub(crate) fn apply_naming(out: String, doc: &SchemaDoc) -> String {
    let stem = schema_stem(doc);
    if stem == "read" {
        return out;
    }
    let mut renamed = out;
    for token in SCAFFOLD_TOKENS {
        let to = if *token == "schema/read.schema" {
            format!("schema/{stem}.schema")
        } else if token.contains("READ") {
            token.replace("READ", &upper_snake(stem))
        } else if token.contains("Read") {
            token.replace("Read", &upper_camel(stem))
        } else {
            token.replace("read", &lower_camel(stem))
        };
        renamed = renamed.replace(token, &to);
    }
    renamed
}

/// Which optional helper groups a schema actually exercises.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct HelperUse {
    pub integer: bool,
    pub u16: bool,
    pub u32: bool,
    pub hex_fixed: bool,
    pub hex_variable: bool,
    pub utf8: bool,
    pub ascii: bool,
    pub list: bool,
    pub unit_variant: bool,
    pub payload_variant: bool,
}

pub(crate) fn helper_use(doc: &SchemaDoc) -> HelperUse {
    let mut used = HelperUse::default();
    for decl in &doc.decls {
        match decl {
            Decl::Enum(_) => {}
            Decl::Struct(decl) => {
                for field in &decl.fields {
                    // The envelope field is stamped/verified directly by the
                    // generated codec, never through the scalar helpers.
                    if is_envelope_field(doc, &decl.name, &field.name) {
                        continue;
                    }
                    visit_ty(&field.ty, &mut used);
                }
            }
            Decl::Union(decl) => {
                for variant in &decl.variants {
                    if variant.payload.is_some() {
                        used.payload_variant = true;
                    } else {
                        used.unit_variant = true;
                    }
                }
            }
        }
    }
    used
}

/// The declarations reachable from the frontend-originated body — exactly the
/// set a native artifact gets encoders for. Every other declaration on the
/// contract is an observation a frontend may only decode, so it has no encoder
/// to call rather than a runtime check saying it must not.
pub(crate) fn encodable_decls(doc: &SchemaDoc) -> Vec<&str> {
    let Some(body) = doc.frontend_body() else {
        return Vec::new();
    };
    let mut reached = vec![body.payload];
    let mut index = 0;
    while index < reached.len() {
        let found = doc.find(reached[index]);
        index += 1;
        match found {
            Some(Decl::Struct(decl)) => {
                for field in &decl.fields {
                    reach_named(&field.ty, &mut reached);
                }
            }
            Some(Decl::Union(decl)) => {
                for variant in &decl.variants {
                    if let Some(payload) = &variant.payload {
                        reach(payload, &mut reached);
                    }
                }
            }
            _ => {}
        }
    }
    reached
}

fn reach<'a>(name: &'a str, reached: &mut Vec<&'a str>) {
    if !reached.contains(&name) {
        reached.push(name);
    }
}

fn reach_named<'a>(ty: &'a FieldTy, reached: &mut Vec<&'a str>) {
    match ty {
        FieldTy::Named(name) => reach(name, reached),
        FieldTy::Option(inner) => reach_named(inner, reached),
        FieldTy::List { element, .. } => reach_named(element, reached),
        _ => {}
    }
}

/// Which helper groups the encode half needs: the same walk as [`helper_use`],
/// restricted to [`encodable_decls`]. An artifact never carries an encode
/// helper no encoder calls.
pub(crate) fn encode_helper_use(doc: &SchemaDoc) -> HelperUse {
    let encodable = encodable_decls(doc);
    let mut used = HelperUse::default();
    for decl in &doc.decls {
        let Decl::Struct(decl) = decl else { continue };
        if !encodable.contains(&decl.name.as_str()) {
            continue;
        }
        for field in &decl.fields {
            visit_ty(&field.ty, &mut used);
        }
    }
    used
}

fn visit_ty(ty: &FieldTy, used: &mut HelperUse) {
    match ty {
        FieldTy::U16 => {
            used.integer = true;
            used.u16 = true;
        }
        FieldTy::U32 => {
            used.integer = true;
            used.u32 = true;
        }
        FieldTy::U63 => used.integer = true,
        FieldTy::Hex16 | FieldTy::Hex32 | FieldTy::Hex64 => used.hex_fixed = true,
        FieldTy::HexVar { .. } => used.hex_variable = true,
        FieldTy::Str { .. } => used.utf8 = true,
        FieldTy::Ascii { .. } => used.ascii = true,
        FieldTy::Named(_) => {}
        FieldTy::Option(inner) => visit_ty(inner, used),
        FieldTy::List { element, .. } => {
            used.list = true;
            visit_ty(element, used);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{apply_naming, dart_member, kotlin_member, rust_field, schema_stem, swift_member};
    use crate::model::SchemaDoc;

    #[test]
    fn finishers_escape_reserved_words_per_language() {
        assert_eq!(rust_field("type"), "r#type");
        assert_eq!(rust_field("state"), "state");
        assert_eq!(dart_member("in"), "in_");
        assert_eq!(dart_member("offered_name"), "offeredName");
        assert_eq!(kotlin_member("in"), "`in`");
        assert_eq!(kotlin_member("is_active"), "isActive");
        assert_eq!(swift_member("internal"), "`internal`");
        assert_eq!(swift_member("peer_lost"), "peerLost");
    }

    fn doc(id: &str) -> SchemaDoc {
        SchemaDoc {
            id: id.to_owned(),
            max_frame_bytes: 1,
            root: "Frame".to_owned(),
            direction: crate::model::Direction::HostToFrontend,
            rules: Vec::new(),
            decls: Vec::new(),
        }
    }

    #[test]
    fn naming_pass_renames_scaffold_tokens_longest_first() {
        let command = doc("envoix/binding/command/1");
        assert_eq!(schema_stem(&command), "command");
        let renamed = apply_naming(
            "ReadErrorKind ReadError readSchemaId READ_SCHEMA_ID schema/read.schema EnvoixReadCodec ReadFrame".to_owned(),
            &command,
        );
        assert_eq!(
            renamed,
            "CommandErrorKind CommandError commandSchemaId COMMAND_SCHEMA_ID schema/command.schema EnvoixCommandCodec ReadFrame"
        );
        let read = doc("envoix/binding/read/1");
        assert_eq!(apply_naming("ReadError".to_owned(), &read), "ReadError");
    }
}
