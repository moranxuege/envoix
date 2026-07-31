//! Which blob, and which incarnation of it.

use envoix_types::{ArtifactId, AttemptGen, RecordId};
use serde::{Deserialize, Serialize};

/// One run of one derivation.
///
/// A card's artifact identity is minted once and never moves, so a re-pick
/// produces a NEW derivation of the SAME artifact id. Without something to tell
/// those apart, a worker still finishing the old one would append to — or delete
/// — the new one's bytes, and the logical id could not say which was meant.
///
/// DERIVED rather than minted: the ACQUISITION's generation and the artifact. A
/// re-pick mints a new acquisition, which is exactly when a new incarnation
/// begins — so this needs no entropy and no durable field of its own. Two runs
/// that should share a blob do; two that should not, cannot.
///
/// The acquisition's generation, never the ATTEMPT's. They look alike and their
/// lifetimes are not the same: a resume advances the attempt generation while
/// deliberately keeping the source, so deriving from it would give one
/// unchanged source two blob keys — and a seal published under the first would
/// be invisible to a run that computed the second. This is the third place that
/// distinction has decided something; the first was a resume closing the
/// descriptor it was about to send from.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
pub struct DerivationWorkId {
    generation: AttemptGen,
    artifact: ArtifactId,
}

impl DerivationWorkId {
    pub const fn of(generation: AttemptGen, artifact: ArtifactId) -> Self {
        Self {
            generation,
            artifact,
        }
    }

    pub const fn generation(self) -> AttemptGen {
        self.generation
    }

    pub const fn artifact(self) -> ArtifactId {
        self.artifact
    }
}

/// Where one blob lives: the card that owns it, and which run produced it.
///
/// Card-scoped because ownership is: a removed card's blobs go with it, and no
/// card can name another's. The work makes the key an INCARNATION rather than a
/// name, which is what stops a stale writer from touching a live artifact.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
pub struct BlobKey {
    card: RecordId,
    work: DerivationWorkId,
}

impl BlobKey {
    pub const fn new(card: RecordId, work: DerivationWorkId) -> Self {
        Self { card, work }
    }

    pub const fn card(self) -> RecordId {
        self.card
    }

    pub const fn work(self) -> DerivationWorkId {
        self.work
    }

    pub const fn artifact(self) -> ArtifactId {
        self.work.artifact()
    }
}
