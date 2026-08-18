//! Platform capability reports exposed to the application engine.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PlatformCapability {
    SecureVault,
    FileSource,
    FileDestination,
    NearbyDiscovery,
    ClipboardRead,
    ClipboardWrite,
    BackgroundExecution,
    Notifications,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityAvailability {
    Available,
    Limited,
    Unavailable,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PlatformCapabilities {
    values: BTreeMap<PlatformCapability, CapabilityAvailability>,
}

impl PlatformCapabilities {
    pub fn new(
        values: impl IntoIterator<Item = (PlatformCapability, CapabilityAvailability)>,
    ) -> Self {
        Self {
            values: values.into_iter().collect(),
        }
    }

    pub fn availability(&self, capability: PlatformCapability) -> CapabilityAvailability {
        self.values
            .get(&capability)
            .copied()
            .unwrap_or(CapabilityAvailability::Unavailable)
    }
}
