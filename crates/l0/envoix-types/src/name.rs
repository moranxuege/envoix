use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, de::Error as _};

const FALLBACK_NAME: &str = "unnamed";

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
    pub fn from_untrusted(provider_name: impl AsRef<str>) -> Self {
        Self(sanitize_leaf(provider_name.as_ref()))
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
        deserialize_canonical_leaf(deserializer).map(Self)
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
