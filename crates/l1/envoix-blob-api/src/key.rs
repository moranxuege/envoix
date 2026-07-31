//! Which blob, and which incarnation of it.

use envoix_types::{ArtifactId, AttemptGen, RecordId, TransferId};
use serde::{Deserialize, Serialize};

/// One incarnation of one card's bulk bytes, and what makes it one.
///
/// Two arms because two things produce bulk bytes and they are stable under
/// different facts. Getting that wrong loses a partial exactly when it is worth
/// most, so what each arm carries is chosen by what SURVIVES the retries that
/// arm expects — never by what happens to be in scope.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BlobWorkId {
    /// Bytes produced from a card's own selection.
    ///
    /// Keyed by the ACQUISITION's generation. A card's artifact id is minted
    /// once and never moves, so a re-pick produces a new derivation of the same
    /// id — and the acquisition generation is what advances exactly then. The
    /// ATTEMPT's generation advances on resume while the source stays, so
    /// deriving from it would give one unchanged source two keys and hide a seal
    /// from the run that should adopt it.
    Derivation {
        acquisition: AttemptGen,
        artifact: ArtifactId,
    },
    /// Bytes received from a peer.
    ///
    /// Keyed by the TRANSFER, which is stable product identity, and never by the
    /// attempt generation — a receive resume advances that on purpose and must
    /// find the same partial. The same distinction the derivation arm makes,
    /// resolved to a different stable fact because a receive has no acquisition.
    Reception {
        transfer: TransferId,
        artifact: ArtifactId,
    },
}

impl BlobWorkId {
    pub const fn of_derivation(acquisition: AttemptGen, artifact: ArtifactId) -> Self {
        Self::Derivation {
            acquisition,
            artifact,
        }
    }

    pub const fn of_reception(transfer: TransferId, artifact: ArtifactId) -> Self {
        Self::Reception { transfer, artifact }
    }

    pub const fn artifact(self) -> ArtifactId {
        match self {
            Self::Derivation { artifact, .. } | Self::Reception { artifact, .. } => artifact,
        }
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
    work: BlobWorkId,
}

impl BlobKey {
    pub const fn new(card: RecordId, work: BlobWorkId) -> Self {
        Self { card, work }
    }

    pub const fn card(self) -> RecordId {
        self.card
    }

    pub const fn work(self) -> BlobWorkId {
        self.work
    }

    pub const fn artifact(self) -> ArtifactId {
        self.work.artifact()
    }
}
