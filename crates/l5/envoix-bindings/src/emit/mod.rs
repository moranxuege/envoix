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
    use super::{dart_member, kotlin_member, rust_field, swift_member};

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
}
