//! Strict parser from the TOML schema document to [`SchemaDoc`].
//!
//! The schema file is trusted repository content, so parse errors may carry a
//! locating context string; they never carry input fragments from anywhere
//! else. Strictness rules that keep every generated decoder simple:
//! declarations may reference earlier declarations only, `option` wraps a
//! scalar or named type, and `list` elements are named types.

use crate::model::{
    Decl, EnumDecl, FieldDecl, FieldTy, SchemaDoc, StructDecl, UnionDecl, UnionVariant,
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
        if !matches!(key.as_str(), "id" | "root" | "limits" | "decl") {
            return Err(SchemaParseError::shape(format!("unknown key {key}")));
        }
    }

    let id = require_str(table.get("id"), "id")?;
    let root = require_str(table.get("root"), "root")?;
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
    Ok(doc)
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
                    if !matches!(key.as_str(), "name" | "payload") {
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
                parsed.push(UnionVariant {
                    name: variant_name,
                    payload,
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

fn require_type_name(name: &str, context: &str) -> Result<(), SchemaParseError> {
    let mut chars = name.chars();
    let valid = chars.next().is_some_and(|first| first.is_ascii_uppercase())
        && chars.all(|c| c.is_ascii_alphanumeric());
    if !valid {
        return Err(SchemaParseError::grammar(format!(
            "{context}: bad type name {name}"
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
