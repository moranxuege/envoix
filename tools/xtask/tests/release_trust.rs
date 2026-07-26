//! BN5 proofs against the REAL repository: a release-shaped build agrees with
//! itself, and the debug instrumentation entry points cannot reach a release
//! payload — proven by compiling the payload and reading its symbol table, not
//! by measuring offsets in the source text.
//!
//! The verdict's own failure modes are unit-tested in `envoix-evidence`, beside
//! the function that produces them. What can only be tested here is the live
//! data: the checked-in declaration, the flat policy projection, the manifest
//! asset and the payload record all against the build that is actually loaded.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use envoix_evidence::release::{
    ArtifactKind, Disagreement, MeasuredArtifact, PackagedFacts, PackagedPayload, ReleaseLedger,
    check_release,
};
use xtask::release::{MANIFEST_ASSET, build_identity, load_ledger, sha256_hex};

/// What packaging reports for a release that agrees with everything.
fn agreeing(
    ledger: &ReleaseLedger,
    build: &envoix_evidence::release::BuildIdentity,
) -> PackagedFacts {
    let policy = &ledger.policy;
    let payload: Vec<PackagedPayload> = policy
        .required_abis
        .iter()
        .map(|abi| PackagedPayload {
            artifact: format!("lib/{abi}/{}", policy.native_library),
            sha256: build
                .payload
                .library
                .iter()
                .find(|library| library.build_type == "release" && &library.abi == abi)
                .expect("the payload record accounts for every required ABI")
                .sha256
                .clone(),
            symbols: policy.allowed_native_symbols.clone(),
        })
        .collect();
    PackagedFacts {
        variant: "prodRelease".to_owned(),
        kind: ArtifactKind::Apk,
        application_id: "app.envoix.host".to_owned(),
        artifact: "Cargo.toml".to_owned(),
        artifact_sha256: String::new(),
        version_code: 1,
        version_name: build
            .compiled
            .get("package_version")
            .expect("the manifest names a package version")
            .clone(),
        signers: vec![policy.signer_sha256.clone()],
        abis: policy.required_abis.clone(),
        entries: [
            "AndroidManifest.xml".to_owned(),
            "classes.dex".to_owned(),
            "assets/envoix-build-manifest.json".to_owned(),
        ]
        .into_iter()
        .chain(payload.iter().map(|library| library.artifact.clone()))
        .collect(),
        manifest_markers: Vec::new(),
        trust_material: Vec::new(),
        build_manifest_sha256: Some(build.manifest_sha256.clone()),
        release_classes: Vec::new(),
        payload,
    }
}

#[test]
fn release_package_trust_and_metadata_agreement() {
    let root = xtask::workspace_root();
    let ledger = load_ledger(&root).expect("the release ledger parses");
    let build = build_identity(&root, &ledger).expect("the build identity loads");

    // The repository as it stands: the checked-in declaration, the flat policy
    // projection the packaging side reads and the payload record all agree with
    // what this build compiled, so a release-shaped artifact passes. This is
    // therefore also the live drift gate — a wire or schema id changed, a
    // hand-edited projection, or a payload built from other contract sources
    // fails right here, telling you to re-run scripts/build-jni-libs.sh.
    let mut facts = agreeing(&ledger, &build);
    let artifact = root.join(&facts.artifact);
    facts.artifact_sha256 = sha256_hex(&fs::read(&artifact).expect("the named artifact is real"));
    let measured = MeasuredArtifact {
        observed_sha256: Some(facts.artifact_sha256.clone()),
        observed_payload: Some(facts.payload.clone()),
        facts,
    };
    let clean = check_release(&ledger, &build, std::slice::from_ref(&measured)).disagreements;
    assert!(
        clean.is_empty(),
        "an agreeing release must pass, got {clean:#?}"
    );

    // The artifact the build embeds is the manifest this build compiled, so
    // "declared -> compiled -> shipped" is one chain and not two claims.
    assert_eq!(
        sha256_hex(&fs::read(root.join(MANIFEST_ASSET)).expect("the manifest asset is checked in")),
        build.manifest_sha256,
        "the packaged build-manifest asset is stale: re-run scripts/build-jni-libs.sh"
    );

    // The manifest names every identity the running system speaks: both
    // generated binding contracts, all four L4 ids, the protocol set, and the
    // trust-root slot.
    for identity in [
        "abi_schema_read_binding_schema_id",
        "abi_schema_command_binding_schema_id",
        "abi_schema_evidence_rust_abi_id",
        "abi_schema_evidence_timeline_schema_id",
        "abi_schema_mailbox_receipt_schema_id",
        "abi_schema_operation_envelope_schema_id",
        "protocol_set_id",
        "protocol_data_alpn",
        "protocol_data_magic",
        "protocol_data_wire_version",
        "package_version",
        "trust_root",
    ] {
        assert!(
            build.compiled.contains_key(identity),
            "the composed manifest must name {identity}"
        );
    }

    // BN5 leaves the deployment trust root to D1; the rule is armed by the
    // ledger's `distribution` switch, which D2 flips.
    assert_eq!(
        build.compiled.get("trust_root").map(String::as_str),
        Some("unprovisioned"),
        "BN5 leaves the deployment trust root to D1"
    );

    // Drift against the LIVE declaration: the BN3 command contract is one of
    // the ids the BN5 manifest carries, so a silent change to it is caught.
    let mut drifted = build.clone();
    let compiled = drifted
        .declared
        .insert(
            "abi_schema_command_binding_schema_id".to_owned(),
            "envoix/binding/command/0".to_owned(),
        )
        .expect("the declaration names the command contract");
    assert!(
        check_release(&ledger, &drifted, std::slice::from_ref(&measured))
            .disagreements
            .contains(&Disagreement::SchemaIdDrift {
                field: "abi_schema_command_binding_schema_id".to_owned(),
                declared: Some("envoix/binding/command/0".to_owned()),
                compiled: Some(compiled),
            })
    );

    // And a declaration naming an identity this build no longer compiles.
    let mut drifted = build.clone();
    drifted.declared.insert(
        "abi_schema_retired_schema_id".to_owned(),
        "envoix/binding/retired/1".to_owned(),
    );
    assert!(
        check_release(&ledger, &drifted, &[measured])
            .disagreements
            .contains(&Disagreement::SchemaIdDrift {
                field: "abi_schema_retired_schema_id".to_owned(),
                declared: Some("envoix/binding/retired/1".to_owned()),
                compiled: None,
            })
    );
}

/// The bundle list is for the CONTAINER, and the hole it closed can be
/// re-opened by putting app content in it: an entry named there is never
/// mapped onto the reviewed surface, so `base/root/anything` listed as
/// "bundletool's" would ship unreviewed exactly as 139 entries used to.
///
/// The two lists are disjoint by construction — `surface_entry` answers `Some`
/// or `None` and never both — so the property to hold the LEDGER to is that
/// every bundle-list pattern really is a container path.
#[test]
fn the_bundle_container_list_never_claims_app_content() {
    let ledger = load_ledger(&xtask::workspace_root()).expect("the release ledger parses");
    for pattern in &ledger.policy.allowed_bundle_entries {
        assert_eq!(
            ArtifactKind::Bundle.surface_entry(pattern),
            None,
            "{pattern} is app content: it belongs in allowed_package_entries, \
             where the reviewed surface can see it"
        );
    }
    assert!(
        !ledger.policy.allowed_bundle_entries.is_empty(),
        "a bundle carries container entries; an empty list would fail every bundle"
    );
}

/// Resources are shipped data. The release policy therefore names every
/// currently reviewed archive entry rather than pre-authorising a directory or
/// extension for whatever a future dependency/source edit puts there.
#[test]
fn the_packaged_resource_inventory_contains_no_patterns() {
    let ledger = load_ledger(&xtask::workspace_root()).expect("the release ledger parses");
    let resources: Vec<&String> = ledger
        .policy
        .allowed_package_entries
        .iter()
        .filter(|entry| entry.starts_with("res/"))
        .collect();
    assert!(!resources.is_empty(), "the packaged app carries resources");
    for entry in resources {
        assert!(
            !entry.contains('*'),
            "{entry} pre-authorises unreviewed resource data"
        );
    }
}

/// The debug instrumentation entry points are cut at the root: they compile
/// only under a non-default cargo feature. This is proved by BUILDING the host
/// cdylib both ways and reading the dynamic symbol table of each — a claim
/// about a compiled artifact, so an export appended anywhere in the source,
/// inside the feature gate's braces or not, is caught.
#[test]
fn release_cdylib_omits_debug_instrumentation() {
    let root = xtask::workspace_root();
    let ledger = load_ledger(&root).expect("the release ledger parses");
    let allowed: BTreeSet<String> = ledger
        .policy
        .allowed_native_symbols
        .iter()
        .cloned()
        .collect();

    // The release configuration IS the default configuration: no default
    // feature can turn the instrumentation on.
    let manifest: toml::Value = toml::from_str(
        &fs::read_to_string(root.join("hosts/envoix-host-android/Cargo.toml"))
            .expect("the host manifest is readable"),
    )
    .expect("the host manifest parses");
    let features = manifest
        .get("features")
        .and_then(toml::Value::as_table)
        .expect("the host crate declares features");
    assert!(
        features.contains_key("e2e-instrumentation"),
        "the instrumentation feature must exist to gate the entry points"
    );
    assert!(
        !features.contains_key("default"),
        "no default feature may enable instrumentation"
    );

    let released = defined_symbols(&build_host_cdylib(&root, &[]));
    assert_eq!(
        released, allowed,
        "a release-configured host cdylib must export exactly the allowed entry points"
    );

    // Not vacuous: the same sources with the feature on DO export the
    // instrumentation lane, and the allow-list rejects it.
    let instrumented = defined_symbols(&build_host_cdylib(&root, &["e2e-instrumentation"]));
    let extra: BTreeSet<&String> = instrumented.difference(&allowed).collect();
    assert!(
        !extra.is_empty()
            && extra.iter().all(|symbol| ledger
                .policy
                .forbidden_native_symbols
                .iter()
                .any(|prefix| symbol.starts_with(prefix))),
        "the instrumented build must export the debug lane and nothing else, got {extra:?}"
    );
}

/// The Kotlin lane and the released surface are ONE vocabulary, spelled in two
/// places that no compiler relates: `NativeHost` declares `external fun`s, the
/// ledger names exported symbols, and the two agree only because someone typed
/// them the same way. A verb renamed on one side and not the other passes every
/// other gate in this repository and fails at the first call, in a release
/// build, as an `UnsatisfiedLinkError` — the class of hole F2b's `submit` ->
/// `intent` rename would otherwise have to be walked across by hand.
///
/// Both directions: a declaration the release does not export is a dead binding,
/// and an exported entry point nothing declares is a door with no lock on the
/// Kotlin side. The cdylib test above pins the ledger to the compiled artifact,
/// so pinning the declaration to the ledger closes the triangle.
#[test]
fn the_kotlin_lane_declares_exactly_the_released_symbols() {
    const NATIVE_HOST: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../apps/envoix-flutter/android/app/src/main/kotlin/app/envoix/host/NativeHost.kt"
    ));
    const PREFIX: &str = "Java_app_envoix_host_NativeHost_";

    let ledger = load_ledger(&xtask::workspace_root()).expect("the release ledger parses");
    let declared: BTreeSet<String> = NATIVE_HOST
        .lines()
        .filter_map(|line| line.trim().strip_prefix("external fun "))
        .map(|declaration| {
            declaration
                .chars()
                .take_while(|character| character.is_alphanumeric() || *character == '_')
                .collect()
        })
        .collect();
    assert!(!declared.is_empty(), "the JNI lane declares no verb at all");
    for verb in &declared {
        // JNI mangles `_` in a Java method name to `_1`, so a verb spelled with
        // one would not be the symbol this concatenation builds.
        assert!(
            !verb.contains('_'),
            "{verb} would be mangled; this mapping only holds for unmangled names"
        );
    }

    let bound: BTreeSet<String> = declared
        .iter()
        .map(|verb| format!("{PREFIX}{verb}"))
        .collect();
    let released: BTreeSet<String> = ledger
        .policy
        .allowed_native_symbols
        .iter()
        .cloned()
        .collect();
    assert_eq!(
        bound, released,
        "the Kotlin lane and the released symbol surface are not the same vocabulary"
    );
}

/// Builds the host cdylib with the given features and returns the shared
/// object cargo produced. A nested build is honest about the target directory:
/// it asks cargo which file it wrote rather than guessing a path.
fn build_host_cdylib(root: &Path, features: &[&str]) -> PathBuf {
    let mut command = Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_owned()));
    command.current_dir(root).args([
        "build",
        "-p",
        "envoix-host-android",
        "--message-format=json",
    ]);
    if !features.is_empty() {
        command.args(["--features", &features.join(",")]);
    }
    let output = command.output().expect("cargo runs");
    assert!(
        output.status.success(),
        "building the host cdylib failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .filter_map(|message| {
            message
                .get("filenames")?
                .as_array()?
                .iter()
                .filter_map(serde_json::Value::as_str)
                .find(|name| name.ends_with(".so"))
                .map(PathBuf::from)
        })
        .next_back()
        .expect("cargo reports the cdylib it wrote")
}

/// Every symbol an ELF shared object DEFINES in its dynamic table — the exact
/// set a caller could bind to. Read here rather than shelled out to `nm` so the
/// proof needs no toolchain beyond the one that built the file.
fn defined_symbols(library: &Path) -> BTreeSet<String> {
    const SHT_DYNSYM: u32 = 11;
    let bytes = fs::read(library).expect("the built cdylib is readable");
    let word = |at: usize| u64::from_le_bytes(bytes[at..at + 8].try_into().expect("8 bytes"));
    let half = |at: usize| u16::from_le_bytes(bytes[at..at + 2].try_into().expect("2 bytes"));
    let full = |at: usize| u32::from_le_bytes(bytes[at..at + 4].try_into().expect("4 bytes"));
    assert_eq!(
        &bytes[..5],
        b"\x7fELF\x02",
        "only ELF64 little-endian objects"
    );

    let section = |index: usize| (word(0x28) as usize) + index * half(0x3a) as usize;
    let mut symbols = BTreeSet::new();
    for index in 0..half(0x3c) as usize {
        let header = section(index);
        if full(header + 4) != SHT_DYNSYM {
            continue;
        }
        let strings = section(full(header + 0x28) as usize);
        let strings = word(strings + 0x18) as usize;
        let table = word(header + 0x18) as usize;
        let entry = word(header + 0x38) as usize;
        for offset in (0..word(header + 0x20) as usize).step_by(entry.max(1)) {
            let symbol = table + offset;
            let defined = half(symbol + 6) != 0;
            let global = matches!(bytes[symbol + 4] >> 4, 1 | 2);
            if !defined || !global {
                continue;
            }
            let name = strings + full(symbol) as usize;
            let end = name
                + bytes[name..]
                    .iter()
                    .position(|byte| *byte == 0)
                    .unwrap_or(0);
            symbols.insert(String::from_utf8_lossy(&bytes[name..end]).into_owned());
        }
    }
    symbols
}
