//! Every rule is checked against the catalogue that ships, and against a
//! mutation of it that the rule must reject. A rule with no mutation here is a
//! rule nobody has proved has teeth.

use super::*;

fn shipped() -> DeploymentCatalogue {
    DeploymentCatalogue::compiled().expect("the shipped catalogue parses")
}

/// Applies one targeted edit to the shipped catalogue text.
fn mutate(from: &str, to: &str) -> String {
    let count = CATALOGUE_TOML.matches(from).count();
    assert_eq!(count, 1, "mutation anchor {from:?} matched {count} times");
    CATALOGUE_TOML.replace(from, to)
}

fn violations_of(text: &str) -> Vec<Violation> {
    DeploymentCatalogue::parse(text)
        .expect("mutation must still parse")
        .violations(LegacyValues::default())
}

#[test]
fn the_shipped_catalogue_is_sound() {
    assert!(
        shipped().violations(LegacyValues::default()).is_empty(),
        "{:?}",
        shipped().violations(LegacyValues::default())
    );
}

#[test]
fn an_unknown_key_is_a_parse_error() {
    let text = mutate("[validation]\n", "[validation]\nrequire_luck = true\n");
    assert!(DeploymentCatalogue::parse(&text).is_err());
}

#[test]
fn a_missing_rule_is_a_parse_error() {
    let text = mutate("require_distinct_hosts = true\n", "");
    assert!(DeploymentCatalogue::parse(&text).is_err());
}

#[test]
fn a_rule_cannot_be_switched_off_in_the_file_it_governs() {
    let text = mutate(
        "require_distinct_hosts = true",
        "require_distinct_hosts = false",
    );
    assert!(
        violations_of(&text).contains(&Violation::RuleDisabled("require_distinct_hosts")),
        "disabling a rule must be a violation"
    );
}

#[test]
fn two_environments_may_not_share_a_host() {
    let text = mutate("rdz.dev.envoix.chkxwlyh.us", "rdz.test.envoix.chkxwlyh.us");
    assert!(
        violations_of(&text)
            .iter()
            .any(|violation| matches!(violation, Violation::DuplicateHost { .. })),
        "a shared host must be a violation — it was only a comment before D1"
    );
}

#[test]
fn a_legacy_host_may_not_be_reused() {
    let legacy = ["rdz.prod.envoix.chkxwlyh.us"];
    let violations = shipped().violations(LegacyValues {
        hosts: &legacy,
        rendezvous_node_ids: &[],
    });
    assert!(
        violations
            .iter()
            .any(|violation| matches!(violation, Violation::LegacyHost { .. }))
    );
}

#[test]
fn two_environments_may_not_share_a_port_block() {
    let text = mutate(
        "[environment.dev]\nname = \"dev\"\nport_block = 96",
        "[environment.dev]\nname = \"dev\"\nport_block = 97",
    );
    assert!(
        violations_of(&text)
            .iter()
            .any(|violation| matches!(violation, Violation::DuplicatePortBlock { .. }))
    );
}

#[test]
fn a_port_must_derive_from_its_block_and_service_suffix() {
    let text = mutate("port = 9645", "port = 9646");
    let violations = violations_of(&text);
    assert!(
        violations.contains(&Violation::PortOutsideBlock {
            owner: "dev.rendezvous".into(),
            port: 9646,
            expected: 9645,
        }),
        "{violations:?}"
    );
}

#[test]
fn the_unallocated_block_may_not_be_claimed() {
    let text = mutate("port_block = 96", "port_block = 95");
    assert!(
        violations_of(&text)
            .iter()
            .any(|violation| matches!(violation, Violation::UnallocatedPortBlock { .. }))
    );
}

/// The hard boundary as a rule: the ports of the live services this project
/// does not own cannot be claimed by any environment.
#[test]
fn a_reserved_port_may_not_be_claimed() {
    let text = mutate("port_block = 96", "port_block = 84");
    let violations = violations_of(&text);
    assert!(
        violations
            .iter()
            .any(|violation| matches!(violation, Violation::ReservedPort { port: 8444, .. })),
        "{violations:?}"
    );
    assert!(
        violations
            .iter()
            .any(|violation| matches!(violation, Violation::ReservedPort { port: 8445, .. }))
    );
}

#[test]
fn a_port_block_outside_the_port_range_is_rejected() {
    let text = mutate("port_block = 96", "port_block = 900");
    assert!(
        violations_of(&text)
            .iter()
            .any(|violation| matches!(violation, Violation::PortBlockOutOfRange { .. }))
    );
}

#[test]
fn an_environment_must_be_allowed_and_must_agree_with_its_key() {
    let dropped = mutate(
        "allowed_environments = [\"dev\", \"test\", \"prod\"]",
        "allowed_environments = [\"test\", \"prod\"]",
    );
    assert!(violations_of(&dropped).contains(&Violation::UndeclaredEnvironment("dev".into())));

    let renamed = mutate(
        "[environment.dev]\nname = \"dev\"",
        "[environment.dev]\nname = \"development\"",
    );
    assert!(violations_of(&renamed).contains(&Violation::NameMismatch {
        key: "dev".into(),
        name: "development".into(),
    }));
}

#[test]
fn the_operator_surface_is_loopback_only() {
    let text = mutate(
        "[environment.dev.diagnostics]\nbind = \"127.0.0.1\"",
        "[environment.dev.diagnostics]\nbind = \"0.0.0.0\"",
    );
    assert!(
        violations_of(&text).contains(&Violation::DiagnosticsBindNotLoopback {
            environment: "dev".into(),
            bind: "0.0.0.0".into(),
        })
    );
}

#[test]
fn a_provisioned_value_must_match_its_declared_format() {
    let text = mutate(
        "node_id = \"TBD_PROVISION_TEST_RENDEZVOUS_NODE_ID\"\nprovisioning_status = \"tbd\"",
        "node_id = \"TBD_PROVISION_TEST_RENDEZVOUS_NODE_ID\"\nprovisioning_status = \"provisioned\"",
    );
    assert!(
        violations_of(&text).contains(&Violation::MalformedProvisionedValue {
            owner: "test.rendezvous".into(),
        })
    );
}

#[test]
fn provisioned_node_ids_must_be_distinct() {
    let node_id = "26117638e1bc254b31fa343e55db98313279a5a689f9e66a04a731ad62ad0501";
    // dev HOLDS this id (promoted from test), so the duplicate is planted on
    // the environment that no longer has one.
    let text = mutate(
        "node_id = \"TBD_PROVISION_TEST_RENDEZVOUS_NODE_ID\"\nprovisioning_status = \"tbd\"",
        &format!("node_id = \"{node_id}\"\nprovisioning_status = \"provisioned\""),
    );
    assert!(
        violations_of(&text)
            .iter()
            .any(|violation| matches!(violation, Violation::DuplicateNodeId { .. })),
        "distinctness was claimed but never checked before D1"
    );
}

#[test]
fn a_legacy_node_id_may_not_be_reused() {
    let legacy = ["26117638e1bc254b31fa343e55db98313279a5a689f9e66a04a731ad62ad0501"];
    let violations = shipped().violations(LegacyValues {
        hosts: &[],
        rendezvous_node_ids: &legacy,
    });
    assert!(violations.contains(&Violation::LegacyNodeId {
        environment: "dev".into(),
    }));
}

#[test]
fn the_provisioned_status_spelling_is_checked_not_assumed() {
    let text = mutate(
        "provisioned_status = \"provisioned\"",
        "provisioned_status = \"ready\"",
    );
    assert!(
        violations_of(&text)
            .iter()
            .any(|violation| matches!(violation, Violation::ProvisionedStatusSpelling(_)))
    );
}

/// An environment is deployable exactly when it holds a real rendezvous
/// identity. dev holds the id promoted from test, so it is the one environment
/// that passes — the gate is a gate, not a wall.
#[test]
fn an_environment_is_deployable_once_its_node_id_is_provisioned() {
    let catalogue = shipped();
    assert!(
        catalogue.blockers("dev").is_empty(),
        "dev holds the promoted node id, which is the whole of its identity"
    );
    assert_eq!(
        catalogue.blockers("test"),
        vec![Blocker::Unprovisioned {
            slot: Slot::RendezvousNodeId
        }],
        "test vacated its identity to dev and needs its own key"
    );
    assert_eq!(
        catalogue.blockers("prod"),
        vec![Blocker::Unprovisioned {
            slot: Slot::RendezvousNodeId
        }]
    );
    assert_eq!(catalogue.blockers("staging"), vec![Blocker::Undeclared]);
}

/// Provisioning the vacated environment with a key of its own unblocks it and
/// leaves the file sound.
#[test]
fn a_newly_keyed_environment_becomes_deployable() {
    let text = mutate(
        "node_id = \"TBD_PROVISION_TEST_RENDEZVOUS_NODE_ID\"\nprovisioning_status = \"tbd\"",
        "node_id = \"3333333333333333333333333333333333333333333333333333333333333333\"\nprovisioning_status = \"provisioned\"",
    );
    let catalogue = DeploymentCatalogue::parse(&text).unwrap();
    assert!(catalogue.blockers("test").is_empty());
    assert!(catalogue.violations(LegacyValues::default()).is_empty());
}

#[test]
fn reserved_ports_name_their_owner() {
    let catalogue = shipped();
    for port in [8444, 8445, 8446] {
        assert!(
            catalogue.reserved_port(port).is_some(),
            "port {port} belongs to a service this project does not run"
        );
    }
    assert!(catalogue.reserved_port(9445).is_none());
}
