//! Canonical identities and values carried across Envoix boundaries.

#![forbid(unsafe_code)]

mod identity;
mod name;
mod secret;
mod value;

pub use identity::{ArtifactId, AttemptGen, RecordId, RequestId, TransferId};
pub use name::{LandedName, OfferedName};
pub use secret::Secret;
pub use value::{ByteCount, Direction};
