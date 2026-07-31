use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, de::Error as _};

const FALLBACK_NAME: &str = "unnamed";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OfferedNameError {
    TooLong { actual: usize, maximum: usize },
}

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct OfferedName(String);

impl OfferedName {
    /// The longest an offered name may be, in UTF-8 bytes.
    ///
    /// This type's whole claim is that it is ONE leaf every filesystem the
    /// transfer may land on can name, so its maximum is the narrowest
    /// per-component limit among them: 255 bytes on ext4/F2FS (Android's own),
    /// 255 UTF-16 units on exFAT and NTFS, 255 UTF-8 bytes on APFS. A longer
    /// name is not a leaf anywhere, so no layer downstream has to decide what
    /// to do with one — and none of them has to invent a number to say so.
    pub const MAX_BYTES: usize = 255;

    /// Reduces an untrusted provider name to one filesystem-independent leaf.
    pub fn from_untrusted(provider_name: impl AsRef<str>) -> Result<Self, OfferedNameError> {
        let leaf = sanitize_leaf(provider_name.as_ref());
        if leaf.len() > Self::MAX_BYTES {
            return Err(OfferedNameError::TooLong {
                actual: leaf.len(),
                maximum: Self::MAX_BYTES,
            });
        }
        Ok(Self(leaf))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<'de> Deserialize<'de> for OfferedName {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let leaf = deserialize_canonical_leaf(deserializer)?;
        Self::from_untrusted(&leaf).map_err(|error| D::Error::custom(format!("{error:?}")))
    }
}

impl fmt::Display for OfferedName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct LandedName(String);

impl LandedName {
    /// Records the leaf name selected by a successful platform publication.
    pub fn new(landed_name: impl AsRef<str>) -> Self {
        Self(sanitize_leaf(landed_name.as_ref()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<'de> Deserialize<'de> for LandedName {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserialize_canonical_leaf(deserializer).map(Self)
    }
}

impl fmt::Display for LandedName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

fn sanitize_leaf(untrusted: &str) -> String {
    let leaf = untrusted.rsplit(['/', '\\']).next().unwrap_or_default();
    if leaf.is_empty() || matches!(leaf, "." | "..") || leaf.contains('\0') {
        FALLBACK_NAME.to_owned()
    } else {
        leaf.to_owned()
    }
}

fn deserialize_canonical_leaf<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: Deserializer<'de>,
{
    let encoded = String::deserialize(deserializer)?;
    if sanitize_leaf(&encoded) != encoded {
        return Err(D::Error::custom("expected a canonical leaf name"));
    }
    Ok(encoded)
}

/// Where one selected document sits inside a produced archive.
///
/// A non-empty sequence of [`OfferedName`]s, which is what makes it safe: every
/// component is already one sanitized filesystem leaf, so this type adds
/// structure and cannot re-admit anything a leaf rejects. `.`, `..`, separators,
/// NUL and absolute roots cannot survive `OfferedName::from_untrusted`, so they
/// cannot appear here either.
///
/// Deliberately NOT an `OfferedName`. That type's whole claim is that it is ONE
/// leaf every filesystem can name, and an archive entry is a relative path with
/// several. Collapsing them would either let a path into a position that
/// promises a leaf, or flatten a directory selection into name collisions.
///
/// Traversal is REFUSED, not neutered. `OfferedName` would sanitize `..` into a
/// harmless leaf, so nothing could escape either way — but dropping it turns
/// `a/../b` into `a/b`, which is a different path, and silently reinterpreting
/// what someone asked for is worse than declining it. Empty components are a
/// different matter: `a//b` and a trailing slash are separator artefacts, not
/// components, and are dropped.
///
/// Duplicate paths within one selection are an INVENTORY question, not a path
/// question, and are settled where the whole list is visible.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct ArchivePath(Vec<OfferedName>);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ArchivePathError {
    /// No component at all, or only separator artefacts.
    Empty,
    /// A `.` or `..` component. See the type docs: refused, not resolved.
    Traversal,
    TooDeep {
        actual: usize,
        maximum: usize,
    },
    TooLong {
        actual: usize,
        maximum: usize,
    },
}

impl ArchivePath {
    /// The deepest an entry may nest.
    ///
    /// A bound rather than a limit anyone will meet: it exists so a hostile or
    /// pathological selection cannot make the inventory unbounded, and the
    /// inventory has to fit in a record.
    pub const MAX_COMPONENTS: usize = 64;

    /// The longest a whole path may be, in UTF-8 bytes including separators.
    /// The narrowest common full-path limit is far larger; this is again a
    /// bound on the record, not a claim about filesystems.
    pub const MAX_BYTES: usize = 4096;

    /// Builds a path from untrusted components: empty ones dropped, traversal
    /// refused, each survivor sanitized as a leaf.
    ///
    /// The artefact/traversal decision is made on the RAW component, before
    /// sanitization, because sanitization cannot be asked afterwards what it
    /// replaced — `OfferedName` substitutes a fallback rather than emptying, so
    /// `..` and a genuinely odd filename become indistinguishable once it has
    /// run.
    pub fn from_untrusted<I, S>(components: I) -> Result<Self, ArchivePathError>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let mut leaves = Vec::new();
        for component in components {
            let raw = component.as_ref();
            if raw.is_empty() {
                continue;
            }
            if raw == "." || raw == ".." {
                return Err(ArchivePathError::Traversal);
            }
            let Ok(leaf) = OfferedName::from_untrusted(raw) else {
                continue;
            };
            leaves.push(leaf);
        }
        Self::from_leaves(leaves)
    }

    /// The path for a document chosen on its own: one leaf, no directory.
    pub fn leaf(name: OfferedName) -> Self {
        Self(vec![name])
    }

    fn from_leaves(leaves: Vec<OfferedName>) -> Result<Self, ArchivePathError> {
        if leaves.is_empty() {
            return Err(ArchivePathError::Empty);
        }
        if leaves.len() > Self::MAX_COMPONENTS {
            return Err(ArchivePathError::TooDeep {
                actual: leaves.len(),
                maximum: Self::MAX_COMPONENTS,
            });
        }
        let bytes = leaves.iter().map(|leaf| leaf.as_str().len()).sum::<usize>()
            + leaves.len().saturating_sub(1);
        if bytes > Self::MAX_BYTES {
            return Err(ArchivePathError::TooLong {
                actual: bytes,
                maximum: Self::MAX_BYTES,
            });
        }
        Ok(Self(leaves))
    }

    /// The final component — what this document is called.
    pub fn name(&self) -> &OfferedName {
        self.0.last().expect("an archive path is never empty")
    }

    pub fn components(&self) -> &[OfferedName] {
        &self.0
    }
}

impl fmt::Display for ArchivePath {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut components = self.0.iter();
        if let Some(first) = components.next() {
            formatter.write_str(first.as_str())?;
        }
        for component in components {
            write!(formatter, "/{}", component.as_str())?;
        }
        Ok(())
    }
}

impl<'de> Deserialize<'de> for ArchivePath {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        // Through `from_leaves`, so hostile stored bytes face the same bounds a
        // fresh selection does. Each component is an `OfferedName` and has
        // already refused anything that is not a leaf.
        let leaves = Vec::<OfferedName>::deserialize(deserializer)?;
        Self::from_leaves(leaves).map_err(|error| D::Error::custom(format!("{error:?}")))
    }
}

#[cfg(test)]
mod archive_path_tests {
    use super::*;

    /// Traversal is refused rather than flattened. Neutering `..` into a leaf
    /// would let `a/../b` through as `a/b` — a path nobody asked for.
    #[test]
    fn traversal_is_refused_not_reinterpreted() {
        assert_eq!(
            ArchivePath::from_untrusted(["..", "etc", "passwd"]),
            Err(ArchivePathError::Traversal)
        );
        assert_eq!(
            ArchivePath::from_untrusted(["a", "..", "b"]),
            Err(ArchivePathError::Traversal)
        );
        assert_eq!(
            ArchivePath::from_untrusted(["a", ".", "b"]),
            Err(ArchivePathError::Traversal)
        );
    }

    /// A separator INSIDE a component cannot smuggle depth past the bounds: each
    /// component is one leaf by construction.
    #[test]
    fn a_separator_inside_a_component_does_not_create_one() {
        let nested = ArchivePath::from_untrusted(["a/b", "c"]).expect("a path");
        assert_eq!(nested.components().len(), 2);
        assert!(
            !nested.components()[0].as_str().contains('/'),
            "a separator survived inside one component: {nested}"
        );
    }

    /// Empty components are separator artefacts and are dropped; a path that is
    /// ONLY artefacts is not a path and is refused.
    #[test]
    fn empty_components_are_dropped_but_not_all_of_them() {
        let path = ArchivePath::from_untrusted(["photos", "", "trip.jpg"]).expect("a path");
        assert_eq!(path.to_string(), "photos/trip.jpg");

        assert_eq!(
            ArchivePath::from_untrusted([""]),
            Err(ArchivePathError::Empty)
        );
        assert_eq!(
            ArchivePath::from_untrusted(Vec::<String>::new()),
            Err(ArchivePathError::Empty)
        );
    }

    /// Bounds exist so an inventory stays something a record can hold.
    #[test]
    fn a_path_is_bounded_in_depth_and_length() {
        let deep: Vec<String> = (0..ArchivePath::MAX_COMPONENTS + 1)
            .map(|index| format!("d{index}"))
            .collect();
        assert!(matches!(
            ArchivePath::from_untrusted(deep),
            Err(ArchivePathError::TooDeep { .. })
        ));

        let long: Vec<String> = (0..32).map(|_| "x".repeat(200)).collect();
        assert!(matches!(
            ArchivePath::from_untrusted(long),
            Err(ArchivePathError::TooLong { .. })
        ));
    }

    /// Stored bytes face the same bounds a fresh selection does.
    #[test]
    fn hostile_stored_bytes_cannot_widen_a_path() {
        let deep = serde_json::to_string(
            &(0..ArchivePath::MAX_COMPONENTS + 1)
                .map(|index| format!("d{index}"))
                .collect::<Vec<_>>(),
        )
        .expect("the fixture encodes");
        assert!(serde_json::from_str::<ArchivePath>(&deep).is_err());
        assert!(serde_json::from_str::<ArchivePath>("[]").is_err());

        let round_tripped: ArchivePath =
            serde_json::from_str(r#"["photos","trip.jpg"]"#).expect("a stored path decodes");
        assert_eq!(round_tripped.to_string(), "photos/trip.jpg");
    }

    /// A document chosen on its own is one leaf, and its name is that leaf.
    #[test]
    fn a_lone_document_is_a_one_component_path() {
        let name = OfferedName::from_untrusted("report.pdf").expect("a leaf");
        let path = ArchivePath::leaf(name.clone());
        assert_eq!(path.components(), [name]);
        assert_eq!(path.to_string(), "report.pdf");
    }
}
