#[test]
fn identifier_registry_unique_and_no_legacy_collision() {
    let report = xtask::identifier_check(&xtask::workspace_root())
        .expect("identifier registry and owners must be readable");
    assert!(report.checked > 0, "the gate must extract live identifiers");
    report.ensure_success().unwrap_or_else(|error| {
        panic!(
            "{error}\npending entries are allowed until their owners exist: {:#?}",
            report.pending
        )
    });
}

#[test]
fn dependency_direction_enforced() {
    let report =
        xtask::arch_check(&xtask::workspace_root()).expect("workspace metadata must be readable");
    assert!(
        report.packages_checked > 0,
        "the gate must inspect packages"
    );
    assert!(
        report.manifests_checked > 0,
        "the gate must inspect manifests"
    );
    report.ensure_success().unwrap_or_else(|error| {
        panic!("{error}");
    });
}
