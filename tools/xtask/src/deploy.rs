use std::path::Path;

use envoix_deployment::{DeploymentCatalogue, LegacyValues};

use crate::{CheckResult, format_violations, legacy_values_by_scope, load_toml_text};

pub const CATALOGUE_PATH: &str = "deploy/environments.toml";

#[derive(Debug)]
pub struct DeployCheckReport {
    pub environments: usize,
    pub deployable: Vec<String>,
    pub blocked: Vec<String>,
    pub violations: Vec<String>,
}

impl DeployCheckReport {
    pub fn ensure_success(&self) -> CheckResult<()> {
        if self.violations.is_empty() {
            Ok(())
        } else {
            Err(format_violations("deploy-check", &self.violations))
        }
    }

    /// The deployer's question, and the only place that answers it with an
    /// exit code: may THIS environment be deployed right now?
    pub fn ensure_deployable(&self, environment: &str) -> CheckResult<()> {
        self.ensure_success()?;
        if self.deployable.iter().any(|name| name == environment) {
            return Ok(());
        }
        let reason = self
            .blocked
            .iter()
            .find(|line| line.starts_with(&format!("{environment}:")))
            .cloned()
            .unwrap_or_else(|| format!("{environment}: no such environment in the catalogue"));
        Err(format!("{environment} may not be deployed\n- {reason}"))
    }
}

/// Judges `deploy/environments.toml` as it stands on disk, so the gate sees
/// the working tree rather than what the last build compiled in.
pub fn deploy_check(root: &Path) -> CheckResult<DeployCheckReport> {
    let path = root.join(CATALOGUE_PATH);
    let catalogue = DeploymentCatalogue::parse(&load_toml_text(&path)?)
        .map_err(|error| format!("{}: {error}", path.display()))?;
    let violations = catalogue_violations(root, &catalogue)?;

    let mut deployable = Vec::new();
    let mut blocked = Vec::new();
    for name in catalogue.environment.keys() {
        let blockers = catalogue.blockers(name);
        if blockers.is_empty() {
            deployable.push(name.clone());
        } else {
            let reasons: Vec<String> = blockers.iter().map(ToString::to_string).collect();
            blocked.push(format!("{name}: {}", reasons.join("; ")));
        }
    }

    Ok(DeployCheckReport {
        environments: catalogue.environment.len(),
        deployable,
        blocked,
        violations,
    })
}

/// The same structural verdict `identifier-check` folds into its own, so a bad
/// catalogue fails the gate that already runs rather than a new one somebody
/// has to remember.
pub(crate) fn catalogue_violations(
    root: &Path,
    catalogue: &DeploymentCatalogue,
) -> CheckResult<Vec<String>> {
    let hosts = legacy_values_by_scope(root, "deployed-service-host")?;
    let node_ids = legacy_values_by_scope(root, "deployed-rendezvous-node-id")?;
    let hosts: Vec<&str> = hosts.iter().map(String::as_str).collect();
    let node_ids: Vec<&str> = node_ids.iter().map(String::as_str).collect();
    let mut violations: Vec<String> = catalogue
        .violations(LegacyValues {
            hosts: &hosts,
            rendezvous_node_ids: &node_ids,
        })
        .iter()
        .map(ToString::to_string)
        .collect();
    violations.sort();
    Ok(violations)
}
