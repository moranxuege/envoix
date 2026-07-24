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
    pub decls: Vec<Decl>,
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
