use std::collections::BTreeMap;
use std::fmt;

use crate::{DeploymentCatalogue, ProvisioningStatus, Service};

/// A provisioned value an environment cannot be deployed without.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Slot {
    RendezvousNodeId,
    TrustRoot,
}

impl Slot {
    const fn as_str(self) -> &'static str {
        match self {
            Self::RendezvousNodeId => "rendezvous.node_id",
            Self::TrustRoot => "trust.root_sha256",
        }
    }
}

impl fmt::Display for Slot {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Why one environment may not be deployed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Blocker {
    Undeclared,
    Unprovisioned { slot: Slot },
    Malformed { slot: Slot },
}

impl fmt::Display for Blocker {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Undeclared => formatter.write_str("no such environment in the catalogue"),
            Self::Unprovisioned { slot } => write!(formatter, "{slot} is not provisioned"),
            Self::Malformed { slot } => {
                write!(
                    formatter,
                    "{slot} claims to be provisioned but is malformed"
                )
            }
        }
    }
}

/// A defect in the catalogue as a whole.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Violation {
    RuleDisabled(&'static str),
    ProvisionedStatusSpelling(String),
    UndeclaredEnvironment(String),
    MissingEnvironment(String),
    NameMismatch {
        key: String,
        name: String,
    },
    DuplicateHost {
        host: String,
        owners: [String; 2],
    },
    LegacyHost {
        owner: String,
        host: String,
    },
    DuplicatePortBlock {
        block: u16,
        owners: [String; 2],
    },
    UnallocatedPortBlock {
        environment: String,
        block: u16,
    },
    PortBlockOutOfRange {
        environment: String,
        block: u16,
    },
    PortOutsideBlock {
        owner: String,
        port: u16,
        expected: u16,
    },
    ReservedPort {
        owner: String,
        port: u16,
        reserved_for: String,
    },
    DuplicateNodeId {
        owners: [String; 2],
    },
    LegacyNodeId {
        environment: String,
    },
    DuplicateTrustRoot {
        owners: [String; 2],
    },
    ProdTrustsNonProdRoot {
        environment: String,
    },
    MalformedProvisionedValue {
        owner: String,
    },
    DiagnosticsBindNotLoopback {
        environment: String,
        bind: String,
    },
}

impl fmt::Display for Violation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::RuleDisabled(rule) => write!(
                formatter,
                "deployment: {rule} is false, but a rule the catalogue governs cannot be switched off in it"
            ),
            Self::ProvisionedStatusSpelling(spelling) => write!(
                formatter,
                "deployment: meta.provisioned_status is {spelling:?}, but the provisioned status is spelled \"provisioned\""
            ),
            Self::UndeclaredEnvironment(name) => write!(
                formatter,
                "deployment: environment {name:?} is declared but not in allowed_environments"
            ),
            Self::MissingEnvironment(name) => write!(
                formatter,
                "deployment: environment {name:?} is allowed but not declared"
            ),
            Self::NameMismatch { key, name } => write!(
                formatter,
                "deployment: environment {key:?} calls itself {name:?}"
            ),
            Self::DuplicateHost { host, owners } => write!(
                formatter,
                "deployment: {} and {} both declare host {host:?}",
                owners[0], owners[1]
            ),
            Self::LegacyHost { owner, host } => write!(
                formatter,
                "deployment: {owner} declares legacy host {host:?}"
            ),
            Self::DuplicatePortBlock { block, owners } => write!(
                formatter,
                "deployment: {} and {} both claim port block {block}xx",
                owners[0], owners[1]
            ),
            Self::UnallocatedPortBlock { environment, block } => write!(
                formatter,
                "deployment: {environment} claims port block {block}xx, which is deliberately unallocated"
            ),
            Self::PortBlockOutOfRange { environment, block } => write!(
                formatter,
                "deployment: {environment} claims port block {block}xx, which is not a usable port range"
            ),
            Self::PortOutsideBlock {
                owner,
                port,
                expected,
            } => write!(
                formatter,
                "deployment: {owner} declares port {port}, but its block and service suffix derive {expected}"
            ),
            Self::ReservedPort {
                owner,
                port,
                reserved_for,
            } => write!(
                formatter,
                "deployment: {owner} declares port {port}, reserved for {reserved_for}"
            ),
            Self::DuplicateNodeId { owners } => write!(
                formatter,
                "deployment: {} and {} share a provisioned rendezvous node id",
                owners[0], owners[1]
            ),
            Self::LegacyNodeId { environment } => write!(
                formatter,
                "deployment: {environment} reuses the legacy rendezvous node id"
            ),
            Self::DuplicateTrustRoot { owners } => write!(
                formatter,
                "deployment: {} and {} share a provisioned trust root",
                owners[0], owners[1]
            ),
            Self::ProdTrustsNonProdRoot { environment } => write!(
                formatter,
                "deployment: prod trusts the same root as {environment}"
            ),
            Self::MalformedProvisionedValue { owner } => write!(
                formatter,
                "deployment: {owner} claims to be provisioned but does not match its declared format"
            ),
            Self::DiagnosticsBindNotLoopback { environment, bind } => write!(
                formatter,
                "deployment: {environment} binds diagnostics to {bind:?}; the operator surface is loopback only"
            ),
        }
    }
}

/// Values the catalogue may never reuse, owned by the legacy registry.
#[derive(Clone, Copy, Debug, Default)]
pub struct LegacyValues<'a> {
    pub hosts: &'a [&'a str],
    pub rendezvous_node_ids: &'a [&'a str],
}

const LOOPBACK_BINDS: [&str; 2] = ["127.0.0.1", "::1"];

impl DeploymentCatalogue {
    /// Every defect in the catalogue as a whole. Empty means the file is sound;
    /// whether a given environment may be deployed is [`Self::blockers`].
    pub fn violations(&self, legacy: LegacyValues<'_>) -> Vec<Violation> {
        let mut violations = Vec::new();
        self.check_assertions(&mut violations);
        self.check_environment_set(&mut violations);
        self.check_hosts(legacy.hosts, &mut violations);
        self.check_ports(&mut violations);
        self.check_provisioned_values(legacy.rendezvous_node_ids, &mut violations);
        self.check_diagnostics_bind(&mut violations);
        violations
    }

    fn check_assertions(&self, violations: &mut Vec<Violation>) {
        let validation = &self.validation;
        for (rule, enabled) in [
            ("require_distinct_hosts", validation.require_distinct_hosts),
            (
                "require_distinct_node_ids",
                validation.require_distinct_node_ids,
            ),
            (
                "require_distinct_trust_roots",
                validation.require_distinct_trust_roots,
            ),
            ("reject_legacy_node_ids", validation.reject_legacy_node_ids),
            ("reject_legacy_hosts", validation.reject_legacy_hosts),
            (
                "prod_must_not_trust_non_prod_roots",
                validation.prod_must_not_trust_non_prod_roots,
            ),
            (
                "require_distinct_port_blocks",
                validation.require_distinct_port_blocks,
            ),
            (
                "require_consistent_service_port_suffixes",
                validation.require_consistent_service_port_suffixes,
            ),
            (
                "require_loopback_diagnostics_bind",
                validation.require_loopback_diagnostics_bind,
            ),
        ] {
            if !enabled {
                violations.push(Violation::RuleDisabled(rule));
            }
        }
        if self.meta.provisioned_status != "provisioned" {
            violations.push(Violation::ProvisionedStatusSpelling(
                self.meta.provisioned_status.clone(),
            ));
        }
    }

    fn check_environment_set(&self, violations: &mut Vec<Violation>) {
        for name in self.environment.keys() {
            if !self.validation.allowed_environments.contains(name) {
                violations.push(Violation::UndeclaredEnvironment(name.clone()));
            }
        }
        for name in &self.validation.allowed_environments {
            if !self.environment.contains_key(name) {
                violations.push(Violation::MissingEnvironment(name.clone()));
            }
        }
        for (key, environment) in &self.environment {
            if &environment.name != key {
                violations.push(Violation::NameMismatch {
                    key: key.clone(),
                    name: environment.name.clone(),
                });
            }
        }
    }

    fn check_hosts(&self, legacy_hosts: &[&str], violations: &mut Vec<Violation>) {
        let mut seen: BTreeMap<&str, String> = BTreeMap::new();
        for (name, environment) in &self.environment {
            for (service, host) in environment.hosts() {
                let owner = owner_of(name, service);
                if let Some(first) = seen.insert(host, owner.clone()) {
                    violations.push(Violation::DuplicateHost {
                        host: host.to_owned(),
                        owners: [first, owner.clone()],
                    });
                }
                if legacy_hosts.contains(&host) {
                    violations.push(Violation::LegacyHost {
                        owner,
                        host: host.to_owned(),
                    });
                }
            }
        }
    }

    /// The whole port scheme is one equation: `port == block * 100 + suffix`.
    /// Distinct blocks then make a cross-environment port collision
    /// unrepresentable, and the suffix keeps the service readable from the
    /// number in every block.
    fn check_ports(&self, violations: &mut Vec<Violation>) {
        let mut blocks: BTreeMap<u16, String> = BTreeMap::new();
        for (name, environment) in &self.environment {
            let block = environment.port_block;
            if let Some(first) = blocks.insert(block, name.clone()) {
                violations.push(Violation::DuplicatePortBlock {
                    block,
                    owners: [first, name.clone()],
                });
            }
            if self.validation.unallocated_port_blocks.contains(&block) {
                violations.push(Violation::UnallocatedPortBlock {
                    environment: name.clone(),
                    block,
                });
            }
            for service in Service::ALL {
                let suffix = self.validation.service_port_suffix.of(service);
                let owner = owner_of(name, service);
                let Some(expected) = block
                    .checked_mul(100)
                    .and_then(|base| base.checked_add(suffix))
                else {
                    violations.push(Violation::PortBlockOutOfRange {
                        environment: name.clone(),
                        block,
                    });
                    break;
                };
                let port = environment.port(service);
                if port != expected {
                    violations.push(Violation::PortOutsideBlock {
                        owner: owner.clone(),
                        port,
                        expected,
                    });
                }
                // Both the declared port and the one the block derives: a block
                // that lands on someone else's service is wrong even when the
                // declared ports disagree with it.
                for candidate in if port == expected {
                    vec![port]
                } else {
                    vec![port, expected]
                } {
                    if let Some(reserved) = self.reserved_port(candidate) {
                        violations.push(Violation::ReservedPort {
                            owner: owner.clone(),
                            port: candidate,
                            reserved_for: reserved.owner.clone(),
                        });
                    }
                }
            }
        }
    }

    fn check_provisioned_values(&self, legacy_node_ids: &[&str], violations: &mut Vec<Violation>) {
        let mut node_ids: BTreeMap<&str, String> = BTreeMap::new();
        let mut trust_roots: BTreeMap<&str, String> = BTreeMap::new();
        for (name, environment) in &self.environment {
            if environment.rendezvous.provisioning_status == ProvisioningStatus::Provisioned {
                let node_id = environment.rendezvous.node_id.as_str();
                let owner = owner_of(name, Service::Rendezvous);
                if !self
                    .validation
                    .node_id_format_when_provisioned
                    .accepts(node_id)
                {
                    violations.push(Violation::MalformedProvisionedValue {
                        owner: owner.clone(),
                    });
                }
                if let Some(first) = node_ids.insert(node_id, owner.clone()) {
                    violations.push(Violation::DuplicateNodeId {
                        owners: [first, owner],
                    });
                }
                if legacy_node_ids.contains(&node_id) {
                    violations.push(Violation::LegacyNodeId {
                        environment: name.clone(),
                    });
                }
            }
            if environment.trust.provisioning_status == ProvisioningStatus::Provisioned {
                let root = environment.trust.root_sha256.as_str();
                let owner = format!("{name}.trust");
                if !self
                    .validation
                    .trust_root_sha256_format_when_provisioned
                    .accepts(root)
                {
                    violations.push(Violation::MalformedProvisionedValue {
                        owner: owner.clone(),
                    });
                }
                if let Some(first) = trust_roots.insert(root, owner.clone()) {
                    violations.push(Violation::DuplicateTrustRoot {
                        owners: [first, owner],
                    });
                }
            }
        }
        let Some(prod) = self.environment.get("prod") else {
            return;
        };
        if prod.trust.provisioning_status != ProvisioningStatus::Provisioned {
            return;
        }
        for (name, environment) in &self.environment {
            if name != "prod"
                && environment.trust.provisioning_status == ProvisioningStatus::Provisioned
                && environment.trust.root_sha256 == prod.trust.root_sha256
            {
                violations.push(Violation::ProdTrustsNonProdRoot {
                    environment: name.clone(),
                });
            }
        }
    }

    fn check_diagnostics_bind(&self, violations: &mut Vec<Violation>) {
        for (name, environment) in &self.environment {
            if !LOOPBACK_BINDS.contains(&environment.diagnostics.bind.as_str()) {
                violations.push(Violation::DiagnosticsBindNotLoopback {
                    environment: name.clone(),
                    bind: environment.diagnostics.bind.clone(),
                });
            }
        }
    }
}

fn owner_of(environment: &str, service: Service) -> String {
    format!("{environment}.{service}")
}
