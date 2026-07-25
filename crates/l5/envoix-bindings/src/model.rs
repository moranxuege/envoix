//! The parsed schema model every generated artifact derives from.
//!
//! The scalar vocabulary is deliberately closed: there is no bytes/blob type
//! and no handle/path/URI type, so bulk payload bytes and OS handles cannot be
//! represented in a read frame at all. Every string, hex, and list type carries
//! an explicit bound.

/// One parsed schema document, declarations in file order.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SchemaDoc {
    pub id: String,
    pub max_frame_bytes: u32,
    pub root: String,
    /// Who may originate frames on this contract; the emitters read it to
    /// decide which entry points each artifact gets.
    pub direction: Direction,
    /// Contract rules stated by the schema and emitted as generated consts in
    /// every artifact, sorted by key (TOML table order).
    pub rules: Vec<(String, RuleValue)>,
    pub decls: Vec<Decl>,
}

/// Which peers originate frames on a contract. Stated by the schema, so the
/// per-schema direction policy is a property of the contract rather than a
/// coincidence of an emitter: the Rust reference codec always encodes and
/// decodes, and every native artifact always decodes, but only a
/// [`Bidirectional`](Direction::Bidirectional) contract puts an encoder in the
/// native artifacts — and hostile bytes at the Rust decoder.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Direction {
    /// Only the Rust host originates frames; frontends observe them.
    HostToFrontend,
    /// Frontends originate frames too, so they must be able to encode.
    Bidirectional,
}

impl Direction {
    /// Whether the Dart/Kotlin/Swift artifacts carry an encoder.
    pub const fn natives_encode(self) -> bool {
        matches!(self, Self::Bidirectional)
    }
}

/// A contract-rule value; rules freeze semantic guarantees (booleans) and
/// numeric horizons into every generated artifact.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RuleValue {
    Bool(bool),
    Int(u32),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Decl {
    Enum(EnumDecl),
    Struct(StructDecl),
    Union(UnionDecl),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DeclKind {
    Enum,
    Struct,
    Union,
}

impl SchemaDoc {
    pub fn find(&self, name: &str) -> Option<&Decl> {
        self.decls.iter().find(|decl| decl.name() == name)
    }

    /// The body a frontend may originate, or `None` on an observe-only
    /// contract. The parser guarantees a bidirectional contract marks exactly
    /// one union variant and roots it in the frame's single body field.
    pub fn frontend_body(&self) -> Option<FrontendBody<'_>> {
        let variant = self.decls.iter().find_map(|decl| {
            let Decl::Union(decl) = decl else { return None };
            decl.variants
                .iter()
                .find(|variant| variant.frontend_originated)
        })?;
        let Some(Decl::Struct(root)) = self.find(&self.root) else {
            return None;
        };
        Some(FrontendBody {
            field: &root.fields.get(1)?.name,
            variant: &variant.name,
            payload: variant.payload.as_deref()?,
        })
    }

    pub fn kind_of(&self, name: &str) -> Option<DeclKind> {
        self.find(name).map(|decl| match decl {
            Decl::Enum(_) => DeclKind::Enum,
            Decl::Struct(_) => DeclKind::Struct,
            Decl::Union(_) => DeclKind::Union,
        })
    }
}

impl Decl {
    pub fn name(&self) -> &str {
        match self {
            Self::Enum(decl) => &decl.name,
            Self::Struct(decl) => &decl.name,
            Self::Union(decl) => &decl.name,
        }
    }
}

/// A closed set of unit variants; crosses as a snake_case string.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EnumDecl {
    pub name: String,
    pub variants: Vec<String>,
}

/// A record with a fixed field set; unknown keys are rejected by decoders.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StructDecl {
    pub name: String,
    pub fields: Vec<FieldDecl>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FieldDecl {
    pub name: String,
    pub ty: FieldTy,
}

/// A tagged choice; crosses as `{"kind": ..., "value": ...}` with the value
/// key absent for unit variants.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UnionDecl {
    pub name: String,
    pub variants: Vec<UnionVariant>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UnionVariant {
    pub name: String,
    /// Name of an earlier enum/struct/union declaration, or none for a unit
    /// variant.
    pub payload: Option<String>,
    /// Set by `originator = "frontend"`. Direction is per arm, not per
    /// contract: only this variant's payload gets native encoders, so a
    /// frontend has no function with which to fabricate an observation.
    pub frontend_originated: bool,
}

/// The one frame body a frontend may originate, resolved from the schema.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FrontendBody<'a> {
    /// The root struct's single non-envelope field: the frame's body key.
    pub field: &'a str,
    /// The originable variant's wire name, stamped by the native encoder.
    pub variant: &'a str,
    /// The variant's payload declaration — the only type a native may encode.
    pub payload: &'a str,
}

/// Field types. Bounds are part of the type, never a decoder policy.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FieldTy {
    /// Integer 0..=65_535.
    U16,
    /// Integer 0..=4_294_967_295.
    U32,
    /// Integer 0..=2^63-1: every consumer reads it as a non-negative signed 64.
    U63,
    /// Exactly 16 lowercase hex characters (a 64-bit identifier).
    Hex16,
    /// Exactly 32 lowercase hex characters (a 128-bit identifier).
    Hex32,
    /// Exactly 64 lowercase hex characters (a SHA-256 fingerprint).
    Hex64,
    /// Even-length lowercase hex, 2..=max characters.
    HexVar { max_chars: u32 },
    /// UTF-8 text of at most `max_bytes` bytes.
    Str { max_bytes: u32 },
    /// Printable ASCII (0x20..=0x7e) of at most `max_bytes` bytes.
    Ascii { max_bytes: u32 },
    /// A reference to an earlier declaration.
    Named(String),
    /// Present-with-null encodes the absent case; the key is always present.
    Option(Box<FieldTy>),
    /// A homogeneous list of at most `max_len` elements.
    List { element: Box<FieldTy>, max_len: u32 },
}
