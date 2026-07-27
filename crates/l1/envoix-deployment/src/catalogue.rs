// The catalogue schema and the identity one build is compiled for.
//
// This file is compiled TWICE: once as a module of the library, and once by
// `build.rs`, which `include!`s it so the build script resolves a build's
// deployment identity with the same parser and the same deployability rule the
// library and the gates use. Plain comments rather than `//!`, no `crate::`
// path and no submodule: everything here has to be valid mid-file too.

use std::borrow::Cow;
use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Serialize};

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
    /// The spelling of a rendezvous endpoint, as a template over the
    /// placeholders [`RENDEZVOUS_PLACEHOLDERS`] names. Applied, not described:
    /// [`DeploymentCatalogue::identity`] expands it, so a build's broker is
    /// spelled by this line rather than by a `format!` beside it.
    pub node_endpoint_derivation: String,
    /// The same, for the HTTPS services, over [`SERVICE_PLACEHOLDERS`].
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
    /// Which environment a build targets when nothing selects one, and which
    /// one a PUBLIC release must be built for. Both are read: the first by
    /// `build.rs`, the second by the release gate's destination rule.
    pub default_build_environment: String,
    pub production_environment: String,
    pub require_distinct_hosts: bool,
    pub require_distinct_node_ids: bool,
    pub reject_legacy_node_ids: bool,
    pub reject_legacy_hosts: bool,
    pub node_id_format_when_provisioned: ScalarFormat,
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
}

impl ScalarFormat {
    pub fn accepts(self, value: &str) -> bool {
        match self {
            Self::Hex64 => {
                value.len() == 64
                    && value
                        .bytes()
                        .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
            }
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
    /// The Android product flavour that ships to this environment.
    pub app_flavor: String,
    pub rendezvous: RendezvousEndpoint,
    pub relay: PublicEndpoint,
    pub mailbox: PublicEndpoint,
    pub evidence: PublicEndpoint,
    pub diagnostics: DiagnosticsEndpoint,
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

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ProvisioningStatus {
    Tbd,
    Provisioned,
}

/// A provisioned value an environment cannot be deployed without.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Slot {
    RendezvousNodeId,
}

impl Slot {
    const fn as_str(self) -> &'static str {
        match self {
            Self::RendezvousNodeId => "rendezvous.node_id",
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

/// The placeholders `meta.node_endpoint_derivation` may use, and must use all
/// of: a rendezvous endpoint that named fewer would be missing part of the
/// identity a client authenticates.
pub const RENDEZVOUS_PLACEHOLDERS: [&str; 3] = ["node_id", "rendezvous.host", "rendezvous.port"];
/// The placeholders `meta.https_url_derivation` may use, and must use all of.
pub const SERVICE_PLACEHOLDERS: [&str; 3] = ["service.scheme", "service.host", "service.port"];

/// The deployment ONE build is compiled for.
///
/// Three fields, and every one of them has a consumer: `environment` is what
/// the release gate's destination rule judges, and the other two are the broker
/// and relay every invite this build mints is frozen to. The strings are `Cow`
/// so the compiled answer (`BUILD_TARGET`, a `static` written by `build.rs`)
/// and a parsed one are the SAME type — there is no second shape for a
/// deployment identity to drift into.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct DeploymentIdentity {
    pub environment: Cow<'static, str>,
    pub rendezvous_endpoint: Cow<'static, str>,
    pub relay_url: Cow<'static, str>,
}

impl DeploymentIdentity {
    /// This identity as the `static` item `build.rs` writes into `OUT_DIR`.
    ///
    /// The destructure is exhaustive on purpose: a field added to this type
    /// fails to compile here until it is rendered, so the compiled answer can
    /// never carry less than the parsed one.
    pub fn render_static(&self, name: &str) -> String {
        let Self {
            environment,
            rendezvous_endpoint,
            relay_url,
        } = self;
        format!(
            "pub static {name}: DeploymentIdentity = DeploymentIdentity {{\n    \
             environment: Cow::Borrowed({environment:?}),\n    \
             rendezvous_endpoint: Cow::Borrowed({rendezvous_endpoint:?}),\n    \
             relay_url: Cow::Borrowed({relay_url:?}),\n}};\n"
        )
    }
}

/// Why a build cannot carry an environment's identity.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum IdentityError {
    /// The environment exists but may not be deployed, so no build may target
    /// it. This is the case `build.rs` turns into a compile error.
    Blocked {
        environment: String,
        blockers: Vec<Blocker>,
    },
    /// A derivation template in `[meta]` cannot spell this environment.
    Derivation { key: &'static str, detail: String },
}

impl fmt::Display for IdentityError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Blocked {
                environment,
                blockers,
            } => {
                let reasons: Vec<String> = blockers.iter().map(ToString::to_string).collect();
                write!(
                    formatter,
                    "{environment} may not be deployed, so nothing may be built for it: {}",
                    reasons.join("; ")
                )
            }
            Self::Derivation { key, detail } => write!(formatter, "meta.{key}: {detail}"),
        }
    }
}

impl std::error::Error for IdentityError {}

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
    ///
    /// The rendezvous node id is the only value an environment cannot be
    /// deployed without: it IS the identity clients authenticate, and every
    /// other endpoint is derived from the port block and the hostname.
    pub fn blockers(&self, name: &str) -> Vec<Blocker> {
        let Some(environment) = self.environment(name) else {
            return vec![Blocker::Undeclared];
        };
        let slot = Slot::RendezvousNodeId;
        let format = self.validation.node_id_format_when_provisioned;
        match environment.rendezvous.provisioning_status {
            ProvisioningStatus::Tbd => vec![Blocker::Unprovisioned { slot }],
            ProvisioningStatus::Provisioned if !format.accepts(&environment.rendezvous.node_id) => {
                vec![Blocker::Malformed { slot }]
            }
            ProvisioningStatus::Provisioned => Vec::new(),
        }
    }

    /// The identity a build targeting `name` carries, or why no build may.
    ///
    /// Deployability is the SAME question `deploy-check` answers, asked here so
    /// that "may this be deployed" and "may this be built for" cannot diverge:
    /// an environment nobody may deploy is one nobody may compile an app for.
    pub fn identity(&self, name: &str) -> Result<DeploymentIdentity, IdentityError> {
        let blockers = self.blockers(name);
        if !blockers.is_empty() {
            return Err(IdentityError::Blocked {
                environment: name.to_owned(),
                blockers,
            });
        }
        let environment = self
            .environment(name)
            .expect("an environment with no blockers is declared");
        let rendezvous_endpoint = expand(
            &self.meta.node_endpoint_derivation,
            &[
                ("node_id", environment.rendezvous.node_id.clone()),
                ("rendezvous.host", environment.rendezvous.host.clone()),
                ("rendezvous.port", environment.rendezvous.port.to_string()),
            ],
        )
        .map_err(|detail| IdentityError::Derivation {
            key: "node_endpoint_derivation",
            detail,
        })?;
        let relay_url = expand(
            &self.meta.https_url_derivation,
            &[
                ("service.scheme", environment.relay.scheme.clone()),
                ("service.host", environment.relay.host.clone()),
                ("service.port", environment.relay.port.to_string()),
            ],
        )
        .map_err(|detail| IdentityError::Derivation {
            key: "https_url_derivation",
            detail,
        })?;
        Ok(DeploymentIdentity {
            environment: Cow::Owned(name.to_owned()),
            rendezvous_endpoint: Cow::Owned(rendezvous_endpoint),
            relay_url: Cow::Owned(relay_url),
        })
    }
}

impl Meta {
    /// Every derivation template that cannot spell what it claims to, keyed by
    /// the `[meta]` field that carries it. Checked against the placeholder sets
    /// rather than against one environment, so a template that names an unknown
    /// placeholder — or drops one of the identity's parts — is a defect in the
    /// file whether or not any environment is currently deployable.
    pub(crate) fn derivation_errors(&self) -> Vec<(&'static str, String)> {
        let mut errors = Vec::new();
        for (key, template, placeholders) in [
            (
                "node_endpoint_derivation",
                &self.node_endpoint_derivation,
                RENDEZVOUS_PLACEHOLDERS.as_slice(),
            ),
            (
                "https_url_derivation",
                &self.https_url_derivation,
                SERVICE_PLACEHOLDERS.as_slice(),
            ),
        ] {
            let probes: Vec<(&str, String)> = placeholders
                .iter()
                .map(|placeholder| (*placeholder, String::new()))
                .collect();
            if let Err(detail) = expand(template, &probes) {
                errors.push((key, detail));
            }
        }
        errors
    }
}

/// Substitutes `<name>` placeholders in a derivation template.
///
/// Both directions are errors: a placeholder the template names that this
/// substitution does not know, and a value this substitution offers that the
/// template never uses. A template is a rule about a spelling, and one that
/// silently dropped half an identity would be worse than no template at all.
fn expand(template: &str, values: &[(&str, String)]) -> Result<String, String> {
    let mut expanded = String::with_capacity(template.len());
    let mut used = vec![false; values.len()];
    let mut rest = template;
    while let Some(open) = rest.find('<') {
        expanded.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        let Some(close) = after.find('>') else {
            return Err(format!("{template:?} has an unterminated placeholder"));
        };
        let name = &after[..close];
        let Some(index) = values.iter().position(|(key, _)| *key == name) else {
            return Err(format!("{template:?} names unknown placeholder <{name}>"));
        };
        used[index] = true;
        expanded.push_str(&values[index].1);
        rest = &after[close + 1..];
    }
    expanded.push_str(rest);
    let missing: Vec<&str> = values
        .iter()
        .zip(&used)
        .filter(|(_, used)| !**used)
        .map(|((key, _), _)| *key)
        .collect();
    if missing.is_empty() {
        Ok(expanded)
    } else {
        Err(format!(
            "{template:?} never uses <{}>",
            missing.join(">, <")
        ))
    }
}
