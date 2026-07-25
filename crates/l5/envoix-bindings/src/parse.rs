//! Strict parser from the TOML schema document to [`SchemaDoc`].
//!
//! The schema file is trusted repository content, so parse errors may carry a
//! locating context string; they never carry input fragments from anywhere
//! else. Strictness rules that keep every generated decoder simple:
//! declarations may reference earlier declarations only, `option` wraps a
//! scalar or named type, and `list` elements are named types.

use crate::model::{
    Decl, Direction, EnumDecl, FieldDecl, FieldTy, RuleValue, SchemaDoc, StructDecl, UnionDecl,
    UnionVariant,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SchemaParseError {
    /// The document is not valid TOML.
    Toml,
    /// A key is missing, unknown, or has the wrong TOML type.
    Shape { context: String },
    /// A name or type expression violates the schema grammar.
    Grammar { context: String },
    /// A duplicate declaration or an unresolved/forward reference.
    Reference { context: String },
}

impl SchemaParseError {
    fn shape(context: impl Into<String>) -> Self {
        Self::Shape {
            context: context.into(),
        }
    }

    fn grammar(context: impl Into<String>) -> Self {
        Self::Grammar {
            context: context.into(),
        }
    }

    fn reference(context: impl Into<String>) -> Self {
        Self::Reference {
            context: context.into(),
        }
    }
}

pub fn parse_schema(text: &str) -> Result<SchemaDoc, SchemaParseError> {
    let document: toml::Value = toml::from_str(text).map_err(|_| SchemaParseError::Toml)?;
    let table = document
        .as_table()
        .ok_or_else(|| SchemaParseError::shape("document root"))?;

    for key in table.keys() {
        if !matches!(
            key.as_str(),
            "id" | "root" | "direction" | "limits" | "rules" | "decl"
        ) {
            return Err(SchemaParseError::shape(format!("unknown key {key}")));
        }
    }

    let id = require_str(table.get("id"), "id")?;
    require_schema_id(&id)?;
    let root = require_str(table.get("root"), "root")?;
    let direction = require_direction(&require_str(table.get("direction"), "direction")?)?;
    let limits = table
        .get("limits")
        .and_then(toml::Value::as_table)
        .ok_or_else(|| SchemaParseError::shape("limits"))?;
    for key in limits.keys() {
        if key != "max_frame_bytes" {
            return Err(SchemaParseError::shape(format!("unknown key limits.{key}")));
        }
    }
    let max_frame_bytes = require_bound(limits.get("max_frame_bytes"), "limits.max_frame_bytes")?;

    let mut rules = Vec::new();
    if let Some(raw_rules) = table.get("rules") {
        let raw_rules = raw_rules
            .as_table()
            .ok_or_else(|| SchemaParseError::shape("rules"))?;
        for (key, value) in raw_rules {
            let context = format!("rules.{key}");
            require_member_name(key, &context)?;
            let value = match value {
                toml::Value::Boolean(flag) => RuleValue::Bool(*flag),
                toml::Value::Integer(_) => RuleValue::Int(require_bound(Some(value), &context)?),
                _ => return Err(SchemaParseError::shape(context)),
            };
            rules.push((key.clone(), value));
        }
    }

    let raw_decls = table
        .get("decl")
        .and_then(toml::Value::as_array)
        .ok_or_else(|| SchemaParseError::shape("decl"))?;

    let mut decls = Vec::new();
    let mut declared: Vec<String> = Vec::new();
    for (index, raw) in raw_decls.iter().enumerate() {
        let context = format!("decl[{index}]");
        let decl = parse_decl(raw, &context, &declared)?;
        if declared.iter().any(|name| name == decl.name()) {
            return Err(SchemaParseError::reference(format!(
                "{context}: duplicate declaration {}",
                decl.name()
            )));
        }
        declared.push(decl.name().to_owned());
        decls.push(decl);
    }

    let doc = SchemaDoc {
        id,
        max_frame_bytes,
        root,
        direction,
        rules,
        decls,
    };
    let Some(Decl::Struct(root_decl)) = doc.find(&doc.root) else {
        return Err(SchemaParseError::reference(
            "root must name a declared struct",
        ));
    };
    // The emitters treat this field as the codec-owned envelope: it is
    // stamped/verified by generated code and absent from the in-memory models.
    if !root_decl
        .fields
        .first()
        .is_some_and(|field| field.name == "schema" && matches!(field.ty, FieldTy::Ascii { .. }))
    {
        return Err(SchemaParseError::grammar(
            "root struct must lead with an ascii schema field",
        ));
    }
    require_origination(&doc)?;
    require_collation_stable_keys(&doc)?;
    Ok(doc)
}

/// Origination is a property of a union arm, not of a whole contract: a
/// bidirectional schema names exactly one variant its frontends may originate,
/// and the native artifacts get encoders for that payload alone. An
/// observe-only schema names none, so no native can encode anything.
fn require_origination(doc: &SchemaDoc) -> Result<(), SchemaParseError> {
    let marked = doc
        .decls
        .iter()
        .filter_map(|decl| match decl {
            Decl::Union(decl) => Some(decl),
            _ => None,
        })
        .flat_map(|decl| decl.variants.iter())
        .filter(|variant| variant.frontend_originated)
        .count();
    if doc.direction == Direction::HostToFrontend {
        if marked != 0 {
            return Err(SchemaParseError::grammar(
                "an observe-only contract has no frontend-originated body",
            ));
        }
        return Ok(());
    }
    if marked != 1 {
        return Err(SchemaParseError::grammar(
            "a bidirectional contract names exactly one frontend-originated body",
        ));
    }
    // The originated payload has to be the frame's whole body, so the native
    // encoder's argument type IS the arm it may originate.
    let Some(Decl::Struct(root)) = doc.find(&doc.root) else {
        unreachable!("the root struct is validated above");
    };
    let body = match root.fields.as_slice() {
        [_envelope, body] => body,
        _ => {
            return Err(SchemaParseError::grammar(
                "root of a bidirectional contract is the envelope plus one body field",
            ));
        }
    };
    let FieldTy::Named(union) = &body.ty else {
        return Err(SchemaParseError::grammar(
            "the body field of a bidirectional contract must name a union",
        ));
    };
    let Some(Decl::Union(union)) = doc.find(union) else {
        return Err(SchemaParseError::grammar(
            "the body field of a bidirectional contract must name a union",
        ));
    };
    if !union
        .variants
        .iter()
        .any(|variant| variant.frontend_originated)
    {
        return Err(SchemaParseError::grammar(
            "the frontend-originated variant must be an arm of the body union",
        ));
    }
    Ok(())
}

/// Foundation's `.sortedKeys` orders keys with `NSString.compare` under the
/// system locale, not by byte value, so the Swift encoder is byte-identical to
/// the reference codec only while every key pair in a struct orders the same
/// way under both. A pair decided by two letters — or by one key being a prefix
/// of the other — always does; digits (numeric collation) and `_`
/// (punctuation) can reorder, so `a0b`/`a_b` and `a2`/`a10` are rejected here
/// rather than shipped under a header that claims more than holds.
fn require_collation_stable_keys(doc: &SchemaDoc) -> Result<(), SchemaParseError> {
    if doc.direction != Direction::Bidirectional {
        return Ok(());
    }
    for decl in &doc.decls {
        let Decl::Struct(decl) = decl else { continue };
        for (index, field) in decl.fields.iter().enumerate() {
            for other in &decl.fields[index + 1..] {
                let differing = field
                    .name
                    .bytes()
                    .zip(other.name.bytes())
                    .find(|(left, right)| left != right);
                let Some((left, right)) = differing else {
                    continue;
                };
                if !left.is_ascii_lowercase() || !right.is_ascii_lowercase() {
                    return Err(SchemaParseError::grammar(format!(
                        "{}: {} and {} may order differently under Foundation collation",
                        decl.name, field.name, other.name
                    )));
                }
            }
        }
    }
    Ok(())
}

fn parse_decl(
    raw: &toml::Value,
    context: &str,
    declared: &[String],
) -> Result<Decl, SchemaParseError> {
    let table = raw
        .as_table()
        .ok_or_else(|| SchemaParseError::shape(context))?;
    let kind = require_str(table.get("kind"), &format!("{context}.kind"))?;
    let name = require_str(table.get("name"), &format!("{context}.name"))?;
    require_type_name(&name, context)?;

    let allowed: &[&str] = match kind.as_str() {
        "enum" | "union" => &["kind", "name", "variants"],
        "struct" => &["kind", "name", "fields"],
        _ => {
            return Err(SchemaParseError::grammar(format!(
                "{context}: unknown decl kind {kind}"
            )));
        }
    };
    for key in table.keys() {
        if !allowed.contains(&key.as_str()) {
            return Err(SchemaParseError::shape(format!(
                "{context}: unknown key {key}"
            )));
        }
    }

    match kind.as_str() {
        "enum" => {
            let variants = require_array(table.get("variants"), &format!("{context}.variants"))?;
            let mut names = Vec::new();
            for (index, variant) in variants.iter().enumerate() {
                let member_context = format!("{context}.variants[{index}]");
                let variant = variant
                    .as_str()
                    .ok_or_else(|| SchemaParseError::shape(&member_context))?;
                require_member_name(variant, &member_context)?;
                push_unique(&mut names, variant.to_owned(), &member_context)?;
            }
            require_non_empty(&names, context)?;
            Ok(Decl::Enum(EnumDecl {
                name,
                variants: names,
            }))
        }
        "struct" => {
            let fields = require_array(table.get("fields"), &format!("{context}.fields"))?;
            let mut parsed = Vec::new();
            let mut names: Vec<String> = Vec::new();
            for (index, field) in fields.iter().enumerate() {
                let member_context = format!("{context}.fields[{index}]");
                let field = field
                    .as_table()
                    .ok_or_else(|| SchemaParseError::shape(&member_context))?;
                for key in field.keys() {
                    if !matches!(key.as_str(), "name" | "type") {
                        return Err(SchemaParseError::shape(format!(
                            "{member_context}: unknown key {key}"
                        )));
                    }
                }
                let field_name = require_str(field.get("name"), &format!("{member_context}.name"))?;
                require_member_name(&field_name, &member_context)?;
                push_unique(&mut names, field_name.clone(), &member_context)?;
                let type_text = require_str(field.get("type"), &format!("{member_context}.type"))?;
                let ty = parse_type(&type_text, &member_context, declared, true)?;
                parsed.push(FieldDecl {
                    name: field_name,
                    ty,
                });
            }
            require_non_empty(&parsed, context)?;
            Ok(Decl::Struct(StructDecl {
                name,
                fields: parsed,
            }))
        }
        _ => {
            let variants = require_array(table.get("variants"), &format!("{context}.variants"))?;
            let mut parsed = Vec::new();
            let mut names: Vec<String> = Vec::new();
            for (index, variant) in variants.iter().enumerate() {
                let member_context = format!("{context}.variants[{index}]");
                let variant = variant
                    .as_table()
                    .ok_or_else(|| SchemaParseError::shape(&member_context))?;
                for key in variant.keys() {
                    if !matches!(key.as_str(), "name" | "payload" | "originator") {
                        return Err(SchemaParseError::shape(format!(
                            "{member_context}: unknown key {key}"
                        )));
                    }
                }
                let variant_name =
                    require_str(variant.get("name"), &format!("{member_context}.name"))?;
                require_member_name(&variant_name, &member_context)?;
                push_unique(&mut names, variant_name.clone(), &member_context)?;
                let payload = match variant.get("payload") {
                    None => None,
                    Some(value) => {
                        let payload = value
                            .as_str()
                            .ok_or_else(|| SchemaParseError::shape(&member_context))?;
                        require_declared(payload, &member_context, declared)?;
                        Some(payload.to_owned())
                    }
                };
                let frontend_originated = match variant.get("originator") {
                    None => false,
                    Some(value) if value.as_str() == Some("frontend") => {
                        if payload.is_none() {
                            return Err(SchemaParseError::grammar(format!(
                                "{member_context}: a frontend-originated variant needs a payload"
                            )));
                        }
                        true
                    }
                    Some(_) => {
                        return Err(SchemaParseError::grammar(format!(
                            "{member_context}: bad originator"
                        )));
                    }
                };
                parsed.push(UnionVariant {
                    name: variant_name,
                    payload,
                    frontend_originated,
                });
            }
            require_non_empty(&parsed, context)?;
            Ok(Decl::Union(UnionDecl {
                name,
                variants: parsed,
            }))
        }
    }
}

fn parse_type(
    text: &str,
    context: &str,
    declared: &[String],
    allow_wrappers: bool,
) -> Result<FieldTy, SchemaParseError> {
    let text = text.trim();
    match text {
        "u16" => return Ok(FieldTy::U16),
        "u32" => return Ok(FieldTy::U32),
        "u63" => return Ok(FieldTy::U63),
        "hex16" => return Ok(FieldTy::Hex16),
        "hex32" => return Ok(FieldTy::Hex32),
        "hex64" => return Ok(FieldTy::Hex64),
        _ => {}
    }

    if let Some(inner) = text.strip_suffix(')') {
        let (head, argument) = inner
            .split_once('(')
            .ok_or_else(|| SchemaParseError::grammar(format!("{context}: bad type {text}")))?;
        return match head {
            "hexv" => Ok(FieldTy::HexVar {
                max_chars: parse_even_bound(argument, context)?,
            }),
            "str" => Ok(FieldTy::Str {
                max_bytes: parse_bound(argument, context)?,
            }),
            "ascii" => Ok(FieldTy::Ascii {
                max_bytes: parse_bound(argument, context)?,
            }),
            "option" if allow_wrappers => {
                let element = parse_type(argument, context, declared, false)?;
                Ok(FieldTy::Option(Box::new(element)))
            }
            "list" if allow_wrappers => {
                let (element, bound) = argument.rsplit_once(',').ok_or_else(|| {
                    SchemaParseError::grammar(format!("{context}: bad list {text}"))
                })?;
                let element = element.trim();
                require_declared(element, context, declared)?;
                Ok(FieldTy::List {
                    element: Box::new(FieldTy::Named(element.to_owned())),
                    max_len: parse_bound(bound, context)?,
                })
            }
            _ => Err(SchemaParseError::grammar(format!(
                "{context}: bad type {text}"
            ))),
        };
    }

    require_declared(text, context, declared)?;
    Ok(FieldTy::Named(text.to_owned()))
}

fn parse_bound(text: &str, context: &str) -> Result<u32, SchemaParseError> {
    let bound: u32 = text
        .trim()
        .parse()
        .map_err(|_| SchemaParseError::grammar(format!("{context}: bad bound {text}")))?;
    if bound == 0 {
        return Err(SchemaParseError::grammar(format!("{context}: zero bound")));
    }
    Ok(bound)
}

fn parse_even_bound(text: &str, context: &str) -> Result<u32, SchemaParseError> {
    let bound = parse_bound(text, context)?;
    if bound % 2 != 0 {
        return Err(SchemaParseError::grammar(format!(
            "{context}: hexv bound must be even"
        )));
    }
    Ok(bound)
}

fn require_declared(
    name: &str,
    context: &str,
    declared: &[String],
) -> Result<(), SchemaParseError> {
    require_type_name(name, context)?;
    if !declared.iter().any(|declared| declared == name) {
        return Err(SchemaParseError::reference(format!(
            "{context}: {name} is not declared earlier"
        )));
    }
    Ok(())
}

/// Schema ids are `envoix/binding/<stem>/<version>` with a lowercase stem and
/// a numeric version. The stem names the emitted artifacts; a malformed id is
/// a parse error, never a silent fallback.
fn require_schema_id(id: &str) -> Result<(), SchemaParseError> {
    let mut segments = id.split('/');
    let shape = segments.next() == Some("envoix")
        && segments.next() == Some("binding")
        && segments.next().is_some_and(|stem| {
            let mut chars = stem.chars();
            chars.next().is_some_and(|first| first.is_ascii_lowercase())
                && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
        })
        && segments.next().is_some_and(|version| {
            !version.is_empty() && version.chars().all(|c| c.is_ascii_digit())
        })
        && segments.next().is_none();
    if !shape {
        return Err(SchemaParseError::grammar(format!("bad schema id {id}")));
    }
    Ok(())
}

/// Every schema states who originates its frames, so no emitter has to guess
/// which entry points an artifact needs. There is no default: a contract with
/// an unstated direction is a parse error, never a silently observe-only one.
fn require_direction(direction: &str) -> Result<Direction, SchemaParseError> {
    match direction {
        "host_to_frontend" => Ok(Direction::HostToFrontend),
        "bidirectional" => Ok(Direction::Bidirectional),
        _ => Err(SchemaParseError::grammar(format!(
            "bad direction {direction}"
        ))),
    }
}

fn require_type_name(name: &str, context: &str) -> Result<(), SchemaParseError> {
    let mut chars = name.chars();
    let valid = chars.next().is_some_and(|first| first.is_ascii_uppercase())
        && chars.all(|c| c.is_ascii_alphanumeric());
    if !valid {
        return Err(SchemaParseError::grammar(format!(
            "{context}: bad type name {name}"
        )));
    }
    // The naming pass whole-text-renames the codec scaffold identifiers, so a
    // declared name matching one would silently diverge across artifacts.
    if crate::emit::SCAFFOLD_TOKENS.contains(&name) {
        return Err(SchemaParseError::grammar(format!(
            "{context}: {name} collides with a codec scaffold identifier"
        )));
    }
    Ok(())
}

fn require_member_name(name: &str, context: &str) -> Result<(), SchemaParseError> {
    let mut chars = name.chars();
    let valid = chars.next().is_some_and(|first| first.is_ascii_lowercase())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_');
    // Trailing underscores would collide with the Dart reserved-word rename,
    // consecutive underscores make camel-casing ambiguous across artifacts,
    // and `crate`/`self`/`super` cannot be raw identifiers in Rust.
    let representable =
        !name.ends_with('_') && !name.contains("__") && !matches!(name, "crate" | "self" | "super");
    if !valid || !representable {
        return Err(SchemaParseError::grammar(format!(
            "{context}: bad member name {name}"
        )));
    }
    // No emitted form of a member may match a naming-scaffold token (the pass
    // would rename it in some languages but not others).
    let emitted = [
        name.to_owned(),
        crate::emit::lower_camel(name),
        crate::emit::upper_camel(name),
        crate::emit::upper_snake(name),
    ];
    if emitted
        .iter()
        .any(|form| crate::emit::SCAFFOLD_TOKENS.contains(&form.as_str()))
    {
        return Err(SchemaParseError::grammar(format!(
            "{context}: {name} collides with a codec scaffold identifier"
        )));
    }
    Ok(())
}

fn require_str(value: Option<&toml::Value>, context: &str) -> Result<String, SchemaParseError> {
    value
        .and_then(toml::Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| SchemaParseError::shape(context))
}

fn require_array<'a>(
    value: Option<&'a toml::Value>,
    context: &str,
) -> Result<&'a [toml::Value], SchemaParseError> {
    value
        .and_then(toml::Value::as_array)
        .map(Vec::as_slice)
        .ok_or_else(|| SchemaParseError::shape(context))
}

fn require_bound(value: Option<&toml::Value>, context: &str) -> Result<u32, SchemaParseError> {
    let value = value
        .and_then(toml::Value::as_integer)
        .ok_or_else(|| SchemaParseError::shape(context))?;
    u32::try_from(value)
        .ok()
        .filter(|bound| *bound > 0)
        .ok_or_else(|| SchemaParseError::grammar(format!("{context}: bad bound")))
}

fn require_non_empty<T>(members: &[T], context: &str) -> Result<(), SchemaParseError> {
    if members.is_empty() {
        return Err(SchemaParseError::grammar(format!("{context}: empty decl")));
    }
    Ok(())
}

fn push_unique(
    names: &mut Vec<String>,
    name: String,
    context: &str,
) -> Result<(), SchemaParseError> {
    if names.contains(&name) {
        return Err(SchemaParseError::reference(format!(
            "{context}: duplicate member {name}"
        )));
    }
    names.push(name);
    Ok(())
}
