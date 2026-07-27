//! The deployment catalogue: one schema for `deploy/environments.toml`, read
//! by the gate that judges it and by the server that obeys it.
//!
//! Two properties are worth stating because the file used to have neither.
//! Every key is required and unknown keys are rejected, so the document cannot
//! carry a rule that no code reads — `require_distinct_hosts` was a comment for
//! its whole life. And an environment's *port block* is part of its identity:
//! hostnames do not tell a validator whether two environments share a machine,
//! but distinct blocks make a collision unrepresentable without asking DNS
//! anything.

#![forbid(unsafe_code)]

mod rules;

use std::collections::BTreeMap;
use std::fmt;

use serde::Deserialize;

pub use rules::{Blocker, LegacyValues, Slot, Violation};

/// The parsed `deploy/environments.toml`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct DeploymentCatalogue {
    pub meta: Meta,
    #[serde(rename = "reserved_port")]
    pub reserved_ports: Vec<ReservedPort>,
    pub validation: Validation,
    pub environment: BTreeMap<String, Environment>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Meta {
    pub schema_version: u32,
    pub purpose: String,
    pub node_endpoint_derivation: String,
    pub https_url_derivation: String,
    /// The wire spelling of [`ProvisioningStatus::Provisioned`]. Checked, not
    /// decorative: `identifier-check` compares against this literal.
    pub provisioned_status: String,
}

/// A port owned by a service this project does not run.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ReservedPort {
    pub port: u16,
    pub owner: String,
    pub note: String,
}

/// What the catalogue asserts about itself. The booleans are not switches: a
/// `false` is a violation, because a rule that can be turned off in the file it
/// governs is not a rule.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Validation {
    pub allowed_environments: Vec<String>,
    pub require_distinct_hosts: bool,
    pub require_distinct_node_ids: bool,
    pub require_distinct_trust_roots: bool,
    pub reject_legacy_node_ids: bool,
    pub reject_legacy_hosts: bool,
    pub prod_must_not_trust_non_prod_roots: bool,
    pub node_id_format_when_provisioned: ScalarFormat,
    pub trust_root_sha256_format_when_provisioned: ScalarFormat,
    pub require_distinct_port_blocks: bool,
    pub require_consistent_service_port_suffixes: bool,
    pub require_loopback_diagnostics_bind: bool,
    pub unallocated_port_blocks: Vec<u16>,
    pub service_port_suffix: ServicePortSuffix,
}

/// A format a provisioned value must have. An enum rather than a regex: the
/// spelling in the file is checked by the parse, so a typo is a parse error
/// instead of a pattern nothing applies.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum ScalarFormat {
    Hex64,
    Sha256Hex64,
}

impl ScalarFormat {
    pub fn accepts(self, value: &str) -> bool {
        let hex64 = |value: &str| {
            value.len() == 64
                && value
                    .bytes()
                    .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
        };
        match self {
            Self::Hex64 => hex64(value),
            Self::Sha256Hex64 => value.strip_prefix("sha256:").is_some_and(hex64),
        }
    }
}

/// The last two digits of a port, one per service, identical in every block.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ServicePortSuffix {
    pub relay: u16,
    pub rendezvous: u16,
    pub mailbox: u16,
    pub evidence: u16,
    pub diagnostics: u16,
}

impl ServicePortSuffix {
    pub const fn of(&self, service: Service) -> u16 {
        match service {
            Service::Relay => self.relay,
            Service::Rendezvous => self.rendezvous,
            Service::Mailbox => self.mailbox,
            Service::Evidence => self.evidence,
            Service::Diagnostics => self.diagnostics,
        }
    }
}

/// Every service an environment declares a port for.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum Service {
    Relay,
    Rendezvous,
    Mailbox,
    Evidence,
    Diagnostics,
}

impl Service {
    pub const ALL: [Self; 5] = [
        Self::Relay,
        Self::Rendezvous,
        Self::Mailbox,
        Self::Evidence,
        Self::Diagnostics,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Relay => "relay",
            Self::Rendezvous => "rendezvous",
            Self::Mailbox => "mailbox",
            Self::Evidence => "evidence",
            Self::Diagnostics => "diagnostics",
        }
    }
}

impl fmt::Display for Service {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Environment {
    pub name: String,
    pub port_block: u16,
    pub rendezvous: RendezvousEndpoint,
    pub relay: PublicEndpoint,
    pub mailbox: PublicEndpoint,
    pub evidence: PublicEndpoint,
    pub diagnostics: DiagnosticsEndpoint,
    pub trust: TrustRoot,
}

impl Environment {
    pub const fn port(&self, service: Service) -> u16 {
        match service {
            Service::Relay => self.relay.port,
            Service::Rendezvous => self.rendezvous.port,
            Service::Mailbox => self.mailbox.port,
            Service::Evidence => self.evidence.port,
            Service::Diagnostics => self.diagnostics.port,
        }
    }

    /// The published hostnames. Diagnostics is absent on purpose: it binds
    /// loopback and is not a published endpoint.
    pub fn hosts(&self) -> [(Service, &str); 4] {
        [
            (Service::Relay, self.relay.host.as_str()),
            (Service::Rendezvous, self.rendezvous.host.as_str()),
            (Service::Mailbox, self.mailbox.host.as_str()),
            (Service::Evidence, self.evidence.host.as_str()),
        ]
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct RendezvousEndpoint {
    pub host: String,
    pub port: u16,
    pub node_id: String,
    pub provisioning_status: ProvisioningStatus,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PublicEndpoint {
    pub scheme: String,
    pub host: String,
    pub port: u16,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct DiagnosticsEndpoint {
    pub bind: String,
    pub port: u16,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct TrustRoot {
    pub root_sha256: String,
    pub provisioning_status: ProvisioningStatus,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ProvisioningStatus {
    Tbd,
    Provisioned,
}

/// The catalogue this build was compiled against. A binary that claims an
/// environment and the gate that judges that environment read the same bytes.
pub const CATALOGUE_TOML: &str = include_str!("../../../../deploy/environments.toml");

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CatalogueError(String);

impl fmt::Display for CatalogueError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "invalid deployment catalogue: {}", self.0)
    }
}

impl std::error::Error for CatalogueError {}

impl DeploymentCatalogue {
    pub fn parse(text: &str) -> Result<Self, CatalogueError> {
        toml::from_str(text).map_err(|error| CatalogueError(error.to_string()))
    }

    /// The catalogue compiled into this build.
    pub fn compiled() -> Result<Self, CatalogueError> {
        Self::parse(CATALOGUE_TOML)
    }

    pub fn environment(&self, name: &str) -> Option<&Environment> {
        self.environment.get(name)
    }

    pub fn reserved_port(&self, port: u16) -> Option<&ReservedPort> {
        self.reserved_ports
            .iter()
            .find(|reserved| reserved.port == port)
    }

    /// What stops `name` being deployed. Empty means nothing does.
    ///
    /// Structural violations are deliberately not folded in: they condemn the
    /// whole file, not one environment, and the caller reports them separately.
    pub fn blockers(&self, name: &str) -> Vec<Blocker> {
        let Some(environment) = self.environment(name) else {
            return vec![Blocker::Undeclared];
        };
        let mut blockers = Vec::new();
        for (slot, status, value, format) in [
            (
                rules::Slot::RendezvousNodeId,
                environment.rendezvous.provisioning_status,
                environment.rendezvous.node_id.as_str(),
                self.validation.node_id_format_when_provisioned,
            ),
            (
                rules::Slot::TrustRoot,
                environment.trust.provisioning_status,
                environment.trust.root_sha256.as_str(),
                self.validation.trust_root_sha256_format_when_provisioned,
            ),
        ] {
            match status {
                ProvisioningStatus::Tbd => {
                    blockers.push(Blocker::Unprovisioned { slot });
                }
                ProvisioningStatus::Provisioned if !format.accepts(value) => {
                    blockers.push(Blocker::Malformed { slot });
                }
                ProvisioningStatus::Provisioned => {}
            }
        }
        blockers
    }
}

#[cfg(test)]
mod tests;
