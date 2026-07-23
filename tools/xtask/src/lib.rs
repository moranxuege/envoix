use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::path::{Component, Path, PathBuf};

use cargo_metadata::{DependencyKind, MetadataCommand, Package};
use serde::Deserialize;

pub type CheckResult<T> = Result<T, String>;

#[derive(Debug)]
pub struct IdentifierCheckReport {
    pub checked: usize,
    pub pending: Vec<String>,
    pub violations: Vec<String>,
}

impl IdentifierCheckReport {
    pub fn ensure_success(&self) -> CheckResult<()> {
        if self.violations.is_empty() {
            Ok(())
        } else {
            Err(format_violations("identifier-check", &self.violations))
        }
    }
}

#[derive(Debug)]
pub struct ArchCheckReport {
    pub packages_checked: usize,
    pub manifests_checked: usize,
    pub violations: Vec<String>,
}

impl ArchCheckReport {
    pub fn ensure_success(&self) -> CheckResult<()> {
        if self.violations.is_empty() {
            Ok(())
        } else {
            Err(format_violations("arch-check", &self.violations))
        }
    }
}

pub fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("xtask workspace root exists")
}

#[derive(Debug, Deserialize)]
struct Catalog {
    identifier: Vec<CatalogIdentifier>,
}

#[derive(Clone, Debug, Deserialize)]
struct CatalogIdentifier {
    key: String,
    owner: String,
    owner_path: String,
    collision_scope: String,
    extract: String,
    #[serde(default)]
    comparison: Option<String>,
}

#[derive(Debug, Deserialize)]
struct LegacyRegistry {
    #[serde(default)]
    network_dialect: Vec<LegacyIdentifier>,
    #[serde(default)]
    discovery_invite: Vec<LegacyIdentifier>,
    #[serde(default)]
    deployment: Vec<LegacyIdentifier>,
    #[serde(default)]
    service_api: Vec<LegacyIdentifier>,
    #[serde(default)]
    platform_storage: Vec<LegacyIdentifier>,
    #[serde(default)]
    crypto_label: Vec<LegacyIdentifier>,
}

impl LegacyRegistry {
    fn identifiers(self) -> Vec<LegacyIdentifier> {
        [
            self.network_dialect,
            self.discovery_invite,
            self.deployment,
            self.service_api,
            self.platform_storage,
            self.crypto_label,
        ]
        .into_iter()
        .flatten()
        .collect()
    }
}

#[derive(Clone, Debug, Deserialize)]
struct LegacyIdentifier {
    key: String,
    collision_scope: String,
    policy: String,
    #[serde(default)]
    value: Option<String>,
    #[serde(default)]
    values: Vec<String>,
}

impl LegacyIdentifier {
    fn all_values(&self) -> impl Iterator<Item = &str> {
        self.value
            .iter()
            .map(String::as_str)
            .chain(self.values.iter().map(String::as_str))
    }
}

#[derive(Debug)]
struct LiveIdentifier {
    key: String,
    collision_scope: String,
    comparison: Option<String>,
    values: Vec<String>,
}

enum Extraction {
    Values(Vec<String>),
    Pending(String),
}

pub fn identifier_check(root: &Path) -> CheckResult<IdentifierCheckReport> {
    let catalog: Catalog = load_toml(&root.join("registry/identifier-catalog.toml"))?;
    let legacy: LegacyRegistry = load_toml(&root.join("registry/legacy-identifiers.toml"))?;
    let legacy = legacy.identifiers();

    let mut checked = 0;
    let mut pending = Vec::new();
    let mut violations = Vec::new();
    let mut live = Vec::new();

    for entry in &catalog.identifier {
        match extract_identifier(root, entry) {
            Ok(Extraction::Values(values)) if values.is_empty() => violations.push(format!(
                "{}: existing owner produced no values via {}",
                entry.key, entry.extract
            )),
            Ok(Extraction::Values(values)) => {
                checked += 1;
                live.push(LiveIdentifier {
                    key: entry.key.clone(),
                    collision_scope: entry.collision_scope.clone(),
                    comparison: entry.comparison.clone(),
                    values,
                });
            }
            Ok(Extraction::Pending(reason)) => pending.push(format!("{} ({reason})", entry.key)),
            Err(error) => violations.push(format!("{}: {error}", entry.key)),
        }
    }

    validate_live_identifiers(&live, &legacy, &mut violations);
    pending.sort();
    violations.sort();

    Ok(IdentifierCheckReport {
        checked,
        pending,
        violations,
    })
}

fn validate_live_identifiers(
    live: &[LiveIdentifier],
    legacy: &[LegacyIdentifier],
    violations: &mut Vec<String>,
) {
    let mut fresh_by_scope: HashMap<(&str, &str), &str> = HashMap::new();
    let mut crypto_values: HashMap<&str, &str> = HashMap::new();

    for identifier in live {
        let mut own_values = HashSet::new();
        for value in &identifier.values {
            if !own_values.insert(value.as_str()) {
                continue;
            }
            let scoped = (identifier.collision_scope.as_str(), value.as_str());
            if let Some(first) = fresh_by_scope.insert(scoped, &identifier.key)
                && first != identifier.key
            {
                violations.push(format!(
                    "fresh collision in scope {}: {} and {} both resolve to {:?}",
                    identifier.collision_scope, first, identifier.key, value
                ));
            }
            if identifier.collision_scope == "crypto-label"
                && let Some(first) = crypto_values.insert(value, &identifier.key)
                && first != identifier.key
            {
                violations.push(format!(
                    "crypto labels must be globally unique: {first} and {} both resolve to {:?}",
                    identifier.key, value
                ));
            }

            for old in legacy {
                for old_value in old.all_values() {
                    if value != old_value {
                        continue;
                    }
                    let same_scope = identifier.collision_scope == old.collision_scope;
                    if approved_scoped_reuse(identifier, old, same_scope) {
                        continue;
                    }
                    violations.push(format!(
                        "{} resolves to legacy value {:?} from {} (fresh scope {}, legacy scope {})",
                        identifier.key,
                        value,
                        old.key,
                        identifier.collision_scope,
                        old.collision_scope
                    ));
                }
            }
        }
    }
}

fn approved_scoped_reuse(
    fresh: &LiveIdentifier,
    legacy: &LegacyIdentifier,
    same_scope: bool,
) -> bool {
    let is_tuple_component = fresh
        .comparison
        .as_deref()
        .is_some_and(|comparison| comparison.starts_with("compare-only-as-"));
    if is_tuple_component && !same_scope {
        // Primitive components such as the number 2 are not identifiers outside
        // their dialect. Their canonical tuple is checked as a separate entry.
        return true;
    }
    if legacy.policy != "scoped-component" {
        return false;
    }
    if !same_scope {
        return true;
    }
    fresh.comparison.as_deref().is_some_and(|comparison| {
        comparison.starts_with("compare-only-as-") || comparison == "same-product-name-approved"
    })
}

fn extract_identifier(root: &Path, entry: &CatalogIdentifier) -> CheckResult<Extraction> {
    let owner_path = root.join(&entry.owner_path);
    if !owner_path.exists() {
        return Ok(Extraction::Pending(format!(
            "owner absent: {}",
            entry.owner_path
        )));
    }

    if entry.owner == "manifest:environments" {
        return extract_environment_values(&owner_path, &entry.extract);
    }
    if entry.extract.starts_with("toml-key:") {
        return extract_toml_key(&owner_path, &entry.extract);
    }
    if entry.extract == "binding-schema-header:id" {
        let document: toml::Value = load_toml(&owner_path)?;
        return scalar_value(
            document
                .get("id")
                .ok_or_else(|| format!("{} has no id field", owner_path.display()))?,
        )
        .map(|value| Extraction::Values(vec![value]));
    }

    extract_compiled_owner(entry)
}

fn extract_compiled_owner(entry: &CatalogIdentifier) -> CheckResult<Extraction> {
    use envoix_auth::identifiers as auth;
    use envoix_evidence::identifiers as evidence;
    use envoix_invite::identifiers as invite;
    use envoix_pairing::identifiers as pairing;
    use envoix_platform_apple::identifiers as apple;
    use envoix_platform_local::identifiers as local;
    use envoix_product::record::identifiers as product;
    use envoix_protocol::identifiers as protocol;
    use envoix_protocol::mailbox::identifiers as mailbox;
    use envoix_rendezvous::identifiers as rendezvous;
    use envoix_session_iroh::identifiers as session;
    use envoix_storage_api::identifiers as storage;

    let values = match (entry.owner.as_str(), entry.extract.as_str()) {
        ("crate:envoix-protocol", "rust-const:PROTOCOL_SET_ID") => one(protocol::PROTOCOL_SET_ID),
        ("crate:envoix-protocol", "rust-bytes-const:DATA_ALPN") => one_bytes(protocol::DATA_ALPN)?,
        ("crate:envoix-protocol", "rust-bytes-const:DATA_MAGIC") => {
            one_bytes(protocol::DATA_MAGIC)?
        }
        ("crate:envoix-protocol", "rust-const:DATA_WIRE_VERSION") => {
            one(protocol::DATA_WIRE_VERSION)
        }
        ("crate:envoix-protocol", "rust-function:DataDialect::canonical_identifier") => {
            vec![protocol::DataDialect::canonical_identifier()]
        }
        ("crate:envoix-rendezvous", "rust-bytes-const:RENDEZVOUS_ALPN") => {
            one_bytes(rendezvous::RENDEZVOUS_ALPN)?
        }
        ("crate:envoix-rendezvous", "rust-bytes-const:RENDEZVOUS_MAGIC") => {
            one_bytes(rendezvous::RENDEZVOUS_MAGIC)?
        }
        ("crate:envoix-rendezvous", "rust-const:RENDEZVOUS_WIRE_VERSION") => {
            one(rendezvous::RENDEZVOUS_WIRE_VERSION)
        }
        ("crate:envoix-rendezvous", "rust-function:RendezvousDialect::canonical_identifier") => {
            vec![rendezvous::RendezvousDialect::canonical_identifier()]
        }
        ("crate:envoix-session-iroh", "rust-const:MDNS_SERVICE_LABEL") => {
            one(session::MDNS_SERVICE_LABEL)
        }
        ("crate:envoix-session-iroh", "derive-dns-sd-fqdn:discovery.mdns.service_label") => {
            vec![session::mdns_service_fqdn()]
        }
        ("crate:envoix-session-iroh", "rust-const:CLIENT_PEER_KEY_ALIAS") => {
            one(session::CLIENT_PEER_KEY_ALIAS)
        }
        ("crate:envoix-invite", "rust-const:URI_SCHEME") => one(invite::URI_SCHEME),
        ("crate:envoix-invite", "rust-const:QR_OUTER_PREFIX") => one(invite::QR_OUTER_PREFIX),
        ("crate:envoix-invite", "rust-const:DEEP_LINK_OUTER_PREFIX") => {
            one(invite::DEEP_LINK_OUTER_PREFIX)
        }
        ("crate:envoix-invite", "rust-const:INVITE_PAYLOAD_VERSION") => {
            one(invite::INVITE_PAYLOAD_VERSION)
        }
        ("crate:envoix-invite", "rust-function:InviteDialect::canonical_identifier") => {
            vec![invite::InviteDialect::canonical_identifier()]
        }
        ("crate:envoix-invite", "rust-const:ROOM_CODE_NAMESPACE_PREFIX") => {
            one(invite::ROOM_CODE_NAMESPACE_PREFIX)
        }
        ("crate:envoix-invite", "rust-function:InviteDialect::legacy_rejection_identifiers") => {
            invite::InviteDialect::legacy_rejection_identifiers()
                .iter()
                .map(ToString::to_string)
                .collect()
        }
        ("crate:envoix-protocol", "rust-const:RECEIPT_HTTP_ROUTE") => {
            one(mailbox::RECEIPT_HTTP_ROUTE)
        }
        ("crate:envoix-protocol", "rust-const:RECEIPT_PAYLOAD_SCHEMA_ID") => {
            one(mailbox::RECEIPT_PAYLOAD_SCHEMA_ID)
        }
        ("crate:envoix-protocol", "rust-const:RECEIPT_KIND") => one(mailbox::RECEIPT_KIND),
        ("crate:envoix-protocol", "rust-const:RECEIPT_SLOT_KDF_CONTEXT") => {
            one(mailbox::RECEIPT_SLOT_KDF_CONTEXT)
        }
        ("crate:envoix-protocol", "rust-const:RECEIPT_SEAL_KDF_CONTEXT") => {
            one(mailbox::RECEIPT_SEAL_KDF_CONTEXT)
        }
        ("crate:envoix-protocol", "rust-const:RECEIPT_AAD_PREFIX") => {
            one(mailbox::RECEIPT_AAD_PREFIX)
        }
        ("crate:envoix-protocol", "rust-function:ReceiptSlotDerivation::canonical_identifier") => {
            one(mailbox::ReceiptSlotDerivation::canonical_identifier())
        }
        ("crate:envoix-protocol", "rust-function:ReceiptSealDerivation::canonical_identifier") => {
            one(mailbox::ReceiptSealDerivation::canonical_identifier())
        }
        ("crate:envoix-evidence", "rust-const:EVIDENCE_HTTP_ROUTE") => {
            one(evidence::EVIDENCE_HTTP_ROUTE)
        }
        ("crate:envoix-product", "rust-const:PRODUCT_RECORD_SCHEMA_ID") => {
            one(product::PRODUCT_RECORD_SCHEMA_ID)
        }
        ("crate:envoix-storage-api", "rust-const:OPERATION_ENVELOPE_SCHEMA_ID") => {
            one(storage::OPERATION_ENVELOPE_SCHEMA_ID)
        }
        ("crate:envoix-platform-android", "rust-function:internal_action_identifiers") => {
            return Ok(Extraction::Pending(
                "depends on pending Android applicationId owner".into(),
            ));
        }
        ("crate:envoix-platform-android", "rust-const:PRIVATE_STORAGE_ROOT") => {
            one(envoix_platform_android::identifiers::PRIVATE_STORAGE_ROOT)
        }
        ("crate:envoix-platform-apple", "rust-const:APPLICATION_SUPPORT_ROOT") => {
            one(apple::APPLICATION_SUPPORT_ROOT)
        }
        ("crate:envoix-platform-local", "rust-function:config_root_identifier") => {
            one(local::config_root_identifier())
        }
        ("crate:envoix-platform-local", "rust-function:state_root_identifier") => {
            one(local::state_root_identifier())
        }
        ("crate:envoix-auth", "rust-bytes-const:SPAKE2_DOMAIN") => one_bytes(auth::SPAKE2_DOMAIN)?,
        ("crate:envoix-auth", "rust-bytes-const:SENDER_IDENTITY") => {
            one_bytes(auth::SENDER_IDENTITY)?
        }
        ("crate:envoix-auth", "rust-bytes-const:RECEIVER_IDENTITY") => {
            one_bytes(auth::RECEIVER_IDENTITY)?
        }
        ("crate:envoix-auth", "rust-bytes-const:EXPORTER_LABEL") => {
            one_bytes(auth::EXPORTER_LABEL)?
        }
        ("crate:envoix-auth", "rust-bytes-const:EXPORTER_CONTEXT") => {
            one_bytes(auth::EXPORTER_CONTEXT)?
        }
        ("crate:envoix-auth", "rust-bytes-const:SENDER_CONFIRM_LABEL") => {
            one_bytes(auth::SENDER_CONFIRM_LABEL)?
        }
        ("crate:envoix-auth", "rust-bytes-const:RECEIVER_CONFIRM_LABEL") => {
            one_bytes(auth::RECEIVER_CONFIRM_LABEL)?
        }
        ("crate:envoix-pairing", "rust-bytes-const:SPAKE2_DOMAIN") => {
            one_bytes(pairing::SPAKE2_DOMAIN)?
        }
        ("crate:envoix-pairing", "rust-bytes-const:INITIATOR_IDENTITY") => {
            one_bytes(pairing::INITIATOR_IDENTITY)?
        }
        ("crate:envoix-pairing", "rust-bytes-const:RESPONDER_IDENTITY") => {
            one_bytes(pairing::RESPONDER_IDENTITY)?
        }
        ("crate:envoix-pairing", "rust-const:CONFIRM_KEY_CONTEXT") => {
            one(pairing::CONFIRM_KEY_CONTEXT)
        }
        ("crate:envoix-pairing", "rust-bytes-const:INITIATOR_CONFIRM_LABEL") => {
            one_bytes(pairing::INITIATOR_CONFIRM_LABEL)?
        }
        ("crate:envoix-pairing", "rust-bytes-const:RESPONDER_CONFIRM_LABEL") => {
            one_bytes(pairing::RESPONDER_CONFIRM_LABEL)?
        }
        ("crate:envoix-pairing", "rust-const:BUNDLE_KEY_CONTEXT") => {
            one(pairing::BUNDLE_KEY_CONTEXT)
        }
        ("crate:envoix-pairing", "rust-bytes-const:INITIATOR_SEAL_AAD") => {
            one_bytes(pairing::INITIATOR_SEAL_AAD)?
        }
        ("crate:envoix-pairing", "rust-bytes-const:RESPONDER_SEAL_AAD") => {
            one_bytes(pairing::RESPONDER_SEAL_AAD)?
        }
        ("crate:envoix-pairing", "rust-const:DATA_TOKEN_CONTEXT") => {
            one(pairing::DATA_TOKEN_CONTEXT)
        }
        _ => {
            return Err(format!(
                "existing owner {} has unsupported extractor {}",
                entry.owner, entry.extract
            ));
        }
    };
    Ok(Extraction::Values(values))
}

fn one(value: impl ToString) -> Vec<String> {
    vec![value.to_string()]
}

fn one_bytes(value: &[u8]) -> CheckResult<Vec<String>> {
    Ok(one(std::str::from_utf8(value).map_err(|error| {
        format!("identifier bytes are not UTF-8: {error}")
    })?))
}

fn extract_toml_key(path: &Path, method: &str) -> CheckResult<Extraction> {
    let document: toml::Value = load_toml(path)?;
    let key = method
        .strip_prefix("toml-key:")
        .ok_or_else(|| format!("invalid TOML extractor {method}"))?;
    let value = lookup_toml_path(&document, key)
        .ok_or_else(|| format!("{} has no TOML key {key}", path.display()))?;
    scalar_value(value).map(|value| Extraction::Values(vec![value]))
}

fn lookup_toml_path<'a>(document: &'a toml::Value, path: &str) -> Option<&'a toml::Value> {
    let mut current = document;
    for segment in path.split('.') {
        if let Some((name, index)) = segment.split_once('[') {
            current = current.get(name)?;
            let index = index.strip_suffix(']')?.parse::<usize>().ok()?;
            current = current.as_array()?.get(index)?;
        } else {
            current = current.get(segment)?;
        }
    }
    Some(current)
}

fn extract_environment_values(path: &Path, method: &str) -> CheckResult<Extraction> {
    let document: toml::Value = load_toml(path)?;
    let expression = method
        .strip_prefix("toml-map:")
        .ok_or_else(|| format!("invalid environment extractor {method}"))?;
    let (path_expression, condition) = expression
        .split_once(" where ")
        .map_or((expression, None), |(path, condition)| {
            (path, Some(condition))
        });
    let suffix = path_expression
        .strip_prefix("environment.*.")
        .ok_or_else(|| format!("unsupported environment map {path_expression}"))?;
    let environments = document
        .get("environment")
        .and_then(toml::Value::as_table)
        .ok_or_else(|| format!("{} has no environment table", path.display()))?;

    let mut values = Vec::new();
    for config in environments.values() {
        let Some(value) = lookup_toml_path(config, suffix) else {
            return Err(format!("environment entry has no {suffix}"));
        };
        if condition.is_some_and(|condition| condition == "provisioning_status=provisioned") {
            let parent = suffix.rsplit_once('.').map_or("", |(parent, _)| parent);
            let provisioned = lookup_toml_path(config, &format!("{parent}.provisioning_status"))
                .and_then(toml::Value::as_str)
                == Some("provisioned");
            if !provisioned {
                continue;
            }
        }
        values.push(scalar_value(value)?);
    }
    values.sort();
    if values.is_empty() {
        Ok(Extraction::Pending(
            "deployment value is not provisioned".into(),
        ))
    } else {
        Ok(Extraction::Values(values))
    }
}

fn scalar_value(value: &toml::Value) -> CheckResult<String> {
    match value {
        toml::Value::String(value) => Ok(value.clone()),
        toml::Value::Integer(value) => Ok(value.to_string()),
        toml::Value::Boolean(value) => Ok(value.to_string()),
        _ => Err(format!("expected scalar identifier, found {value}")),
    }
}

fn load_toml<T: for<'de> Deserialize<'de>>(path: &Path) -> CheckResult<T> {
    let text =
        fs::read_to_string(path).map_err(|error| format!("reading {}: {error}", path.display()))?;
    toml::from_str(&text).map_err(|error| format!("parsing {}: {error}", path.display()))
}

// Architecture checking lives below so both gates share root/path utilities.

pub fn arch_check(root: &Path) -> CheckResult<ArchCheckReport> {
    let metadata = MetadataCommand::new()
        .manifest_path(root.join("Cargo.toml"))
        .no_deps()
        .exec()
        .map_err(|error| format!("cargo metadata failed: {error}"))?;
    let workspace_ids: HashSet<_> = metadata.workspace_members.iter().collect();
    let packages: Vec<_> = metadata
        .packages
        .iter()
        .filter(|package| workspace_ids.contains(&package.id))
        .collect();
    let internal_names: HashSet<_> = packages
        .iter()
        .map(|package| package.name.as_str())
        .collect();
    let package_layers: HashMap<_, _> = packages
        .iter()
        .filter_map(|package| package_layer(package).map(|layer| (package.name.as_str(), layer)))
        .collect();
    let mut violations = Vec::new();

    for package in &packages {
        validate_package(
            root,
            package,
            &internal_names,
            &package_layers,
            &mut violations,
        );
    }

    let manifests = root_manifests(root)?;
    for manifest in &manifests {
        validate_no_legacy_path_dependency(root, manifest, &mut violations)?;
    }
    violations.sort();

    Ok(ArchCheckReport {
        packages_checked: packages.len(),
        manifests_checked: manifests.len(),
        violations,
    })
}

fn package_layer(package: &Package) -> Option<&str> {
    package.metadata.get("envoix")?.get("layer")?.as_str()
}

fn package_role(package: &Package) -> Option<&str> {
    package.metadata.get("envoix")?.get("role")?.as_str()
}

fn validate_package(
    root: &Path,
    package: &Package,
    internal_names: &HashSet<&str>,
    package_layers: &HashMap<&str, &str>,
    violations: &mut Vec<String>,
) {
    let Some(layer) = package_layer(package) else {
        violations.push(format!(
            "{}: missing package.metadata.envoix.layer",
            package.name
        ));
        return;
    };
    let Some(role) = package_role(package) else {
        violations.push(format!(
            "{}: missing package.metadata.envoix.role",
            package.name
        ));
        return;
    };
    if role == "tool" {
        return;
    }

    validate_physical_layer(root, package, layer, role, violations);
    if role == "composition-root" && !is_approved_composition_root(root, package) {
        violations.push(format!(
            "{}: composition-root role is only allowed under hosts/ or for the CLI/server apps",
            package.name
        ));
    }

    for dependency in &package.dependencies {
        if !is_architectural_dependency(dependency.kind) {
            continue;
        }
        if internal_names.contains(dependency.name.as_str()) {
            let Some(dependency_layer) = package_layers.get(dependency.name.as_str()) else {
                violations.push(format!(
                    "{} -> {}: internal dependency has no layer metadata",
                    package.name, dependency.name
                ));
                continue;
            };
            if violates_internal_edge(
                role,
                &package.name,
                layer,
                &dependency.name,
                dependency_layer,
                dependency.kind,
            ) {
                violations.push(format!(
                    "{} ({layer}) may not depend on {} ({dependency_layer})",
                    package.name, dependency.name
                ));
            }
        }
    }

    validate_forbidden_dependencies(package, layer, violations);
    if package.name == "envoix-product" {
        validate_product_source(root, package, violations);
    }
}

fn allowed_internal_edge(
    package: &str,
    layer: &str,
    dependency: &str,
    dependency_layer: &str,
) -> bool {
    match layer {
        "L0" => false,
        "L1" => dependency_layer == "L0",
        "L2" => {
            matches!(dependency_layer, "L0" | "L1")
                || (package == "envoix-attempt-iroh"
                    && matches!(
                        dependency,
                        "envoix-auth" | "envoix-transfer" | "envoix-session-iroh"
                    )
                    && dependency_layer == "L2")
        }
        "L3" => matches!(dependency_layer, "L0" | "L1"),
        "L4" => {
            matches!(dependency_layer, "L0" | "L1" | "L3")
                || (package == "envoix-runtime"
                    && dependency == "envoix-evidence"
                    && dependency_layer == "L4")
        }
        "L5" => matches!(dependency_layer, "L0" | "L4"),
        "L6" => matches!(dependency_layer, "L0" | "L1" | "L5"),
        "L7" => true,
        _ => false,
    }
}

fn is_architectural_dependency(kind: DependencyKind) -> bool {
    !matches!(kind, DependencyKind::Development | DependencyKind::Build)
}

fn violates_internal_edge(
    role: &str,
    package: &str,
    layer: &str,
    dependency: &str,
    dependency_layer: &str,
    kind: DependencyKind,
) -> bool {
    is_architectural_dependency(kind)
        && role != "composition-root"
        && !allowed_internal_edge(package, layer, dependency, dependency_layer)
}

fn validate_physical_layer(
    root: &Path,
    package: &Package,
    layer: &str,
    role: &str,
    violations: &mut Vec<String>,
) {
    let manifest = Path::new(package.manifest_path.as_str());
    let relative = manifest.strip_prefix(root).unwrap_or(manifest);
    if role == "composition-root" {
        return;
    }
    let expected = format!("crates/{}/", layer.to_ascii_lowercase());
    if !relative.to_string_lossy().starts_with(&expected) {
        violations.push(format!(
            "{}: layer {layer} package must live under {expected}, found {}",
            package.name,
            relative.display()
        ));
    }
}

fn is_approved_composition_root(root: &Path, package: &Package) -> bool {
    let manifest = Path::new(package.manifest_path.as_str());
    let relative = manifest.strip_prefix(root).unwrap_or(manifest);
    let path = relative.to_string_lossy();
    path.starts_with("hosts/")
        || path == "apps/envoix-cli/Cargo.toml"
        || path == "apps/envoix-server/Cargo.toml"
}

fn validate_forbidden_dependencies(package: &Package, layer: &str, violations: &mut Vec<String>) {
    const TRANSPORT_PLATFORM: &[&str] = &[
        "iroh",
        "quinn",
        "jni",
        "ndk",
        "flutter",
        "dart",
        "kotlin",
        "swift",
        "objc",
        "android",
        "core-foundation",
    ];
    const PRODUCT_FORBIDDEN: &[&str] = &[
        "iroh",
        "quinn",
        "jni",
        "ndk",
        "flutter",
        "dart",
        "kotlin",
        "swift",
        "objc",
        "android",
        "core-foundation",
        "tempfile",
        "cap-std",
    ];
    let forbidden = if package.name == "envoix-product" {
        PRODUCT_FORBIDDEN
    } else if matches!(layer, "L0" | "L1") {
        TRANSPORT_PLATFORM
    } else {
        &[]
    };
    for dependency in &package.dependencies {
        if !is_architectural_dependency(dependency.kind) {
            continue;
        }
        let normalized = dependency.name.to_ascii_lowercase();
        if forbidden.iter().any(|token| normalized.contains(token)) {
            violations.push(format!(
                "{} ({layer}) has forbidden dependency {}",
                package.name, dependency.name
            ));
        }
    }
}

fn validate_product_source(root: &Path, package: &Package, violations: &mut Vec<String>) {
    const FORBIDDEN: &[&str] = &[
        "std::fs",
        "tokio::fs",
        "iroh",
        "jni",
        "flutter",
        "dart",
        "kotlin",
        "swift",
        "android::",
        "objc",
        "core_foundation",
    ];
    let manifest = Path::new(package.manifest_path.as_str());
    let package_root = manifest.parent().unwrap_or(root);
    let mut files = Vec::new();
    collect_files(package_root, "rs", &mut files);
    for file in files {
        let Ok(source) = fs::read_to_string(&file) else {
            continue;
        };
        for token in FORBIDDEN {
            if source.contains(token) {
                violations.push(format!(
                    "envoix-product source {} contains forbidden boundary token {token:?}",
                    file.strip_prefix(root).unwrap_or(&file).display()
                ));
            }
        }
    }
}

fn root_manifests(root: &Path) -> CheckResult<Vec<PathBuf>> {
    let mut manifests = Vec::new();
    collect_manifests(root, root, &mut manifests)?;
    manifests.sort();
    Ok(manifests)
}

fn collect_manifests(
    root: &Path,
    directory: &Path,
    manifests: &mut Vec<PathBuf>,
) -> CheckResult<()> {
    for entry in fs::read_dir(directory)
        .map_err(|error| format!("reading {}: {error}", directory.display()))?
    {
        let entry = entry.map_err(|error| format!("reading directory entry: {error}"))?;
        let path = entry.path();
        if path.is_dir() {
            let relative = path.strip_prefix(root).unwrap_or(&path);
            let first = relative
                .components()
                .next()
                .and_then(|component| match component {
                    Component::Normal(name) => Some(name.to_string_lossy()),
                    _ => None,
                });
            if first.as_deref().is_some_and(|name| {
                name == "legacy" || name == ".git" || name.starts_with("target")
            }) {
                continue;
            }
            collect_manifests(root, &path, manifests)?;
        } else if path.file_name().is_some_and(|name| name == "Cargo.toml") {
            manifests.push(path);
        }
    }
    Ok(())
}

fn validate_no_legacy_path_dependency(
    root: &Path,
    manifest: &Path,
    violations: &mut Vec<String>,
) -> CheckResult<()> {
    let document: toml::Value = load_toml(manifest)?;
    let mut paths = Vec::new();
    collect_path_keys(&document, &mut paths);
    for dependency_path in paths {
        let resolved = normalize_path(&manifest.parent().unwrap_or(root).join(&dependency_path));
        if resolved.starts_with(root.join("legacy")) {
            violations.push(format!(
                "{} has a path dependency into legacy/: {}",
                manifest.strip_prefix(root).unwrap_or(manifest).display(),
                dependency_path
            ));
        }
    }
    Ok(())
}

fn collect_path_keys(value: &toml::Value, paths: &mut Vec<String>) {
    match value {
        toml::Value::Table(table) => {
            for (key, value) in table {
                if key == "path" {
                    if let Some(path) = value.as_str() {
                        paths.push(path.to_owned());
                    }
                } else {
                    collect_path_keys(value, paths);
                }
            }
        }
        toml::Value::Array(values) => {
            for value in values {
                collect_path_keys(value, paths);
            }
        }
        _ => {}
    }
}

fn normalize_path(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            component => normalized.push(component.as_os_str()),
        }
    }
    normalized
}

fn collect_files(directory: &Path, extension: &str, files: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_files(&path, extension, files);
        } else if path.extension().is_some_and(|found| found == extension) {
            files.push(path);
        }
    }
}

fn format_violations(gate: &str, violations: &[String]) -> String {
    let mut grouped = BTreeMap::<&str, Vec<&str>>::new();
    for violation in violations {
        let category = violation.split(':').next().unwrap_or("violation");
        grouped.entry(category).or_default().push(violation);
    }
    let mut message = format!("{gate} failed with {} violation(s):", violations.len());
    for violations in grouped.values() {
        for violation in violations {
            message.push_str("\n- ");
            message.push_str(violation);
        }
    }
    message
}

#[cfg(test)]
mod tests {
    use super::*;

    fn legacy_id(scope: &str, value: &str, policy: &str) -> LegacyIdentifier {
        LegacyIdentifier {
            key: "legacy.probe".into(),
            collision_scope: scope.into(),
            policy: policy.into(),
            value: Some(value.into()),
            values: Vec::new(),
        }
    }

    fn live_id(scope: &str, value: &str, comparison: Option<&str>) -> LiveIdentifier {
        LiveIdentifier {
            key: "fresh.probe".into(),
            collision_scope: scope.into(),
            comparison: comparison.map(str::to_string),
            values: vec![value.into()],
        }
    }

    #[test]
    fn attempt_iroh_l2_composition_edges_are_targeted() {
        for dependency in ["envoix-auth", "envoix-transfer", "envoix-session-iroh"] {
            assert!(allowed_internal_edge(
                "envoix-attempt-iroh",
                "L2",
                dependency,
                "L2"
            ));
        }
        assert!(!allowed_internal_edge(
            "envoix-attempt-iroh",
            "L2",
            "envoix-storage-local",
            "L2"
        ));
        assert!(!allowed_internal_edge(
            "envoix-session-iroh",
            "L2",
            "envoix-auth",
            "L2"
        ));
    }

    /// The gate has teeth: a fresh value equal to a legacy value in the same
    /// scope, and not a tuple component, must be flagged as a violation.
    #[test]
    fn planted_legacy_collision_is_rejected() {
        let mut violations = Vec::new();
        validate_live_identifiers(
            &[live_id("alpn", "envoix/1", None)],
            &[legacy_id("alpn", "envoix/1", "exact")],
            &mut violations,
        );
        assert!(
            violations.iter().any(|v| v.contains("legacy value")),
            "a fresh value equal to a legacy value must be a violation, got {violations:?}"
        );
    }

    /// A genuinely fresh value in the same scope passes (no false positive).
    #[test]
    fn distinct_fresh_value_passes() {
        let mut violations = Vec::new();
        validate_live_identifiers(
            &[live_id("alpn", "envoix/2", None)],
            &[legacy_id("alpn", "envoix/1", "exact")],
            &mut violations,
        );
        assert!(
            violations.is_empty(),
            "a distinct fresh value must pass, got {violations:?}"
        );
    }

    /// A shared primitive reused as a tuple component (e.g. the magic `ENVX`) is
    /// allowed, but a full canonical tuple equal to a legacy tuple is not.
    #[test]
    fn tuple_component_reuse_allowed_but_full_tuple_collision_rejected() {
        let mut allowed = Vec::new();
        validate_live_identifiers(
            &[live_id(
                "data-wire-dialect-component",
                "ENVX",
                Some("compare-only-as-data-dialect-tuple"),
            )],
            &[legacy_id(
                "data-wire-dialect-component",
                "ENVX",
                "scoped-component",
            )],
            &mut allowed,
        );
        assert!(
            allowed.is_empty(),
            "tuple-component reuse must be allowed, got {allowed:?}"
        );

        let mut rejected = Vec::new();
        let tuple = "alpn=envoix/1;magic=ENVX;wire-version=1";
        validate_live_identifiers(
            &[live_id("data-wire-dialect", tuple, None)],
            &[legacy_id("data-wire-dialect", tuple, "exact")],
            &mut rejected,
        );
        assert!(
            !rejected.is_empty(),
            "a fresh tuple equal to a legacy tuple must be rejected"
        );
    }

    #[test]
    fn dependency_kind_refines_layer_enforcement() {
        let wrong_edge = |kind| {
            violates_internal_edge(
                "library",
                "envoix-types",
                "L0",
                "envoix-product",
                "L3",
                kind,
            )
        };

        assert!(!wrong_edge(DependencyKind::Development));
        assert!(!wrong_edge(DependencyKind::Build));
        assert!(wrong_edge(DependencyKind::Normal));
        assert!(wrong_edge(DependencyKind::Unknown));
    }
}
