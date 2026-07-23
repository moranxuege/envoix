use std::collections::BTreeMap;

use envoix_types::{ArtifactId, OfferedName, RecordId};
use serde::{Deserialize, Serialize};

use crate::Durability;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ArtifactManifestEntry {
    name: OfferedName,
    durability: Durability,
}

impl ArtifactManifestEntry {
    pub const fn new(name: OfferedName, durability: Durability) -> Self {
        Self { name, durability }
    }

    pub const fn name(&self) -> &OfferedName {
        &self.name
    }

    pub const fn durability(&self) -> Durability {
        self.durability
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CardManifest {
    record_id: RecordId,
    committed_at: Durability,
    /// Artifact identity is the key. The entry's name is never used for lookup.
    artifacts: BTreeMap<ArtifactId, ArtifactManifestEntry>,
}

impl CardManifest {
    pub const fn new(record_id: RecordId, committed_at: Durability) -> Self {
        Self {
            record_id,
            committed_at,
            artifacts: BTreeMap::new(),
        }
    }

    pub const fn record_id(&self) -> RecordId {
        self.record_id
    }

    pub const fn committed_at(&self) -> Durability {
        self.committed_at
    }

    pub fn artifacts(&self) -> &BTreeMap<ArtifactId, ArtifactManifestEntry> {
        &self.artifacts
    }

    pub fn set_committed_at(&mut self, durability: Durability) {
        self.committed_at = durability;
    }

    pub fn insert(&mut self, artifact_id: ArtifactId, entry: ArtifactManifestEntry) {
        self.artifacts.insert(artifact_id, entry);
    }
}
