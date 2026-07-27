//! The deployment catalogue: one schema for `deploy/environments.toml`, read
//! by the gate that judges it, by the server that obeys it, and by every build
//! that has to know which deployment it is for.
//!
//! Three properties are worth stating because the file used to have none of
//! them. Every key is required and unknown keys are rejected, so the document
//! cannot carry a rule that no code reads — `require_distinct_hosts` was a
//! comment for its whole life. An environment's *port block* is part of its
//! identity: hostnames do not tell a validator whether two environments share a
//! machine, but distinct blocks make a collision unrepresentable without asking
//! DNS anything. And [`BUILD_TARGET`] is this build's deployment identity,
//! resolved by `build.rs` from these same bytes: an app cannot hold an
//! endpoint the catalogue does not declare, and cannot be COMPILED at all for
//! an environment the catalogue will not deploy.

#![forbid(unsafe_code)]

mod catalogue;
mod rules;

use std::borrow::Cow;

pub use catalogue::{
    Blocker, CatalogueError, DeploymentCatalogue, DeploymentIdentity, DiagnosticsEndpoint,
    Environment, IdentityError, Meta, ProvisioningStatus, PublicEndpoint, RENDEZVOUS_PLACEHOLDERS,
    RendezvousEndpoint, ReservedPort, SERVICE_PLACEHOLDERS, ScalarFormat, Service,
    ServicePortSuffix, Slot, Validation,
};
pub use rules::{LegacyValues, Violation};

/// The catalogue this build was compiled against. A binary that claims an
/// environment and the gate that judges that environment read the same bytes.
pub const CATALOGUE_TOML: &str = include_str!("../../../../deploy/environments.toml");

// `pub static BUILD_TARGET: DeploymentIdentity`, resolved by `build.rs` from
// `CATALOGUE_TOML` for `$ENVOIX_ENVIRONMENT` (or the catalogue's own
// `default_build_environment`). The build script REFUSES to emit one for an
// environment the catalogue will not deploy, so a non-deployable build does not
// fail a check — it does not compile.
include!(concat!(env!("OUT_DIR"), "/build_target.rs"));

impl DeploymentCatalogue {
    /// The catalogue compiled into this build.
    pub fn compiled() -> Result<Self, CatalogueError> {
        Self::parse(CATALOGUE_TOML)
    }
}

#[cfg(test)]
mod tests;
