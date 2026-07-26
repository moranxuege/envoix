//! The release trust gate: is this release-shaped build coherent and clean?
//!
//! The verdict itself is [`envoix_evidence::release::check_release`], a pure
//! function that lives beside the manifest it judges. This module is the CLI
//! half: it gathers the inputs that verdict needs, and it is the ONLY writer of
//! the four generated release records.
//!
//! The IDENTITY input is pure Rust, because Rust is the only side that can see
//! the L4 build manifest composed with the L5 binding contracts. The PACKAGING
//! input — signer fingerprint, shipped ABIs, versionCode, the packaged manifest
//! and the payload's symbol tables — can only be observed by Gradle, which
//! holds the artifact; Gradle asserts it in-build and writes what it saw as
//! typed facts, and this gate RE-READS AND RE-HASHES every artifact those facts
//! name before judging them, so a facts file is a report about a real file or
//! it is nothing.
//!
//! The packaging side never reads this registry's TOML. It reads
//! `registry/release-policy.properties`, the flat projection rendered here from
//! the ledger, and the gate re-derives that text and rejects any divergence —
//! so the two enforcers cannot read different values out of the same policy.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use envoix_bindings::build_manifest_frame;
use envoix_bindings::read::encode_read_frame;
use envoix_evidence::BUILD_TRUST_MANIFEST;
use envoix_evidence::release::{
    BuildIdentity, BundledLibrary, Distribution, Evaluation, MeasuredArtifact, PackagedFacts,
    PayloadLibrary, PayloadRecord, ReleaseLedger, ReleaseVerdict, check_release,
    matches_source_glob, render_policy,
};
use serde::Deserialize;
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::{CheckResult, format_violations, load_toml};

/// The hand-maintained release policy, read by this gate and nothing else.
const LEDGER_FILE: &str = "registry/release-ledger.toml";
/// The generated declaration of the identities this build speaks.
const IDENTITY_FILE: &str = "registry/release-identity.toml";
/// The generated flat projection the packaging side consumes.
const POLICY_FILE: &str = "registry/release-policy.properties";
/// The generated record of the payload `scripts/build-jni-libs.sh` produced.
const PAYLOAD_FILE: &str = "registry/release-payload.toml";
/// The composed build manifest, embedded in every packaged artifact so the
/// shipped binary can be asked which build it is.
pub const MANIFEST_ASSET: &str =
    "apps/envoix-flutter/android/app/src/main/assets/envoix-build-manifest.json";
/// Where the gradle release-trust assertions deposit one facts file per
/// artifact.
const FACTS_DIR: &str = "apps/envoix-flutter/android/app/build/outputs/envoix-release-trust";
/// Where `scripts/build-jni-libs.sh` leaves each build type's payload.
const JNI_LIBS_DIR: &str = "apps/envoix-flutter/android/app/src";
/// The build types that get their own payload.
const BUILD_TYPES: [&str; 2] = ["debug", "release"];

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct FactsFile {
    facts: PackagedFacts,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct PayloadFile {
    payload: PayloadRecord,
}

#[derive(Debug)]
pub struct ReleaseGateReport {
    pub artifacts: usize,
    pub identities: usize,
    pub distribution: Distribution,
    pub verdict: ReleaseVerdict,
}

impl ReleaseGateReport {
    pub fn ensure_success(&self) -> CheckResult<()> {
        if self.verdict.disagreements.is_empty() {
            return Ok(());
        }
        let rendered: Vec<String> = self
            .verdict
            .disagreements
            .iter()
            .map(ToString::to_string)
            .collect();
        Err(format_violations("release-gate", &rendered))
    }

    /// What was actually evaluated, one line per artifact plus one for the
    /// build-wide rules, in the order the verdict ran them. A clean summary
    /// that names nothing is indistinguishable from a gate that did nothing;
    /// this is the half that makes the other half mean something.
    pub fn invariant_summary(&self) -> Vec<String> {
        let mut lines: Vec<(Option<&str>, String)> = Vec::new();
        for Evaluation {
            artifact,
            invariant,
            evaluated,
        } in &self.verdict.evaluations
        {
            let artifact = artifact.as_deref();
            let entry = format!("{}={evaluated}", invariant.as_str());
            match lines.last_mut() {
                Some((subject, rendered)) if *subject == artifact => {
                    rendered.push(' ');
                    rendered.push_str(&entry);
                }
                _ => lines.push((
                    artifact,
                    format!("{}: {entry}", artifact.unwrap_or("build")),
                )),
            }
        }
        lines.into_iter().map(|(_, line)| line).collect()
    }
}

/// Judges every release artifact the packaging side reported.
pub fn release_gate(root: &Path) -> CheckResult<ReleaseGateReport> {
    let ledger = load_ledger(root)?;
    let mut build = build_identity(root, &ledger)?;
    // Swap the projection the ledger REQUIRES for the copy the packaging side
    // actually read, so a hand-edited policy is a disagreement rather than an
    // invisible second opinion.
    build.policy_projection = String::from_utf8(read_file(&root.join(POLICY_FILE))?)
        .map_err(|error| format!("{POLICY_FILE}: {error}"))?;
    let artifacts = load_artifacts(root)?;
    Ok(ReleaseGateReport {
        artifacts: artifacts.len(),
        identities: build.compiled.len(),
        distribution: ledger.policy.distribution,
        verdict: check_release(&ledger, &build, &artifacts),
    })
}

pub fn load_ledger(root: &Path) -> CheckResult<ReleaseLedger> {
    load_toml(&root.join(LEDGER_FILE))
}

/// Everything this build says about itself: the checked-in declaration, what it
/// actually compiled, the digests that pin the shipped payload to those
/// sources, and the flat policy the packaging side is REQUIRED to read.
///
/// The projection is rendered here rather than read: it is a build artifact of
/// `record-payload`, not a source, so a checkout that has not built the payload
/// has none to read — and keeping it out of the tree keeps the release signer
/// fingerprint in exactly one checked-in file. [`release_gate`], which runs only
/// where an artifact was packaged, substitutes the copy that was on disk.
pub fn build_identity(root: &Path, ledger: &ReleaseLedger) -> CheckResult<BuildIdentity> {
    let (compiled, frame) = compiled_identity()?;
    let declared = load_declared_identity(root)?;
    let payload = load_payload(root)?;
    Ok(BuildIdentity {
        policy_projection: render_policy(ledger, &declared, &payload),
        declared,
        compiled,
        manifest_sha256: sha256_hex(&frame),
        sources_sha256: sources_digest(root, ledger)?,
        payload,
    })
}

/// Every identity this build compiled, flattened out of the L5 projection of
/// the L4 manifest, alongside the encoded frame itself — the bytes every
/// artifact embeds so it can be asked which build it is.
///
/// The composed manifest is the complete identity set, so the KEY SET is
/// data-driven: an identity added to the manifest automatically appears here,
/// which drifts the checked-in declaration until it is regenerated and
/// reviewed. Nothing has to remember to list it.
pub fn compiled_identity() -> CheckResult<(BTreeMap<String, String>, Vec<u8>)> {
    let frame = build_manifest_frame(&BUILD_TRUST_MANIFEST);
    let bytes = encode_read_frame(&frame)
        .map_err(|error| format!("the build manifest does not encode: {error:?}"))?;
    let document: Value = serde_json::from_slice(&bytes)
        .map_err(|error| format!("the encoded build manifest is not JSON: {error}"))?;
    let manifest = document
        .get("body")
        .and_then(|body| body.get("value"))
        .ok_or_else(|| "the build manifest frame carries no body".to_owned())?;
    let mut identities = BTreeMap::new();
    flatten("", manifest, &mut identities)?;
    Ok((identities, bytes))
}

fn flatten(prefix: &str, value: &Value, out: &mut BTreeMap<String, String>) -> CheckResult<()> {
    match value {
        Value::Object(map) => {
            // A union encodes as its variant tag plus an optional payload.
            if let Some(kind) = map.get("kind").and_then(Value::as_str) {
                out.insert(prefix.to_owned(), kind.to_owned());
                return match map.get("value") {
                    Some(payload) => flatten(prefix, payload, out),
                    None => Ok(()),
                };
            }
            for (name, child) in map {
                let key = if prefix.is_empty() {
                    name.clone()
                } else {
                    format!("{prefix}_{name}")
                };
                flatten(&key, child, out)?;
            }
            Ok(())
        }
        Value::String(text) => {
            out.insert(prefix.to_owned(), text.clone());
            Ok(())
        }
        Value::Number(number) => {
            out.insert(prefix.to_owned(), number.to_string());
            Ok(())
        }
        Value::Bool(flag) => {
            out.insert(prefix.to_owned(), flag.to_string());
            Ok(())
        }
        _ => Err(format!("manifest identity {prefix} is not a scalar")),
    }
}

pub fn load_declared_identity(root: &Path) -> CheckResult<BTreeMap<String, String>> {
    let path = root.join(IDENTITY_FILE);
    let document: toml::Value = load_toml(&path)?;
    let table = document
        .get("identity")
        .and_then(toml::Value::as_table)
        .ok_or_else(|| format!("{} has no [identity] table", path.display()))?;
    let mut declared = BTreeMap::new();
    for (key, value) in table {
        let value = value
            .as_str()
            .ok_or_else(|| format!("{}: identity {key} is not a string", path.display()))?;
        declared.insert(key.clone(), value.to_owned());
    }
    Ok(declared)
}

/// Re-records the payload `scripts/build-jni-libs.sh` has just cross-compiled,
/// then regenerates everything derived from it. This is the ONE write path for
/// all four generated release records, and it is bound to the moment the
/// binaries are produced: the shipped library, the manifest it was built
/// against and the sources it was compiled from are recorded together or not
/// at all.
pub fn record_payload(root: &Path) -> CheckResult<PayloadRecord> {
    let ledger = load_ledger(root)?;
    let (compiled, frame) = compiled_identity()?;
    let mut library = Vec::new();
    for build_type in BUILD_TYPES {
        for abi in &ledger.policy.required_abis {
            let path = root
                .join(JNI_LIBS_DIR)
                .join(build_type)
                .join("jniLibs")
                .join(abi)
                .join(&ledger.policy.native_library);
            library.push(PayloadLibrary {
                build_type: build_type.to_owned(),
                abi: abi.clone(),
                sha256: sha256_hex(&read_file(&path)?),
            });
        }
    }
    let record = PayloadRecord {
        build_manifest_sha256: sha256_hex(&frame),
        sources_sha256: sources_digest(root, &ledger)?,
        library,
        // Cross-compiling the payload says nothing about the libraries the
        // release only packages, so what was already accepted is carried
        // forward rather than silently dropped.
        bundled: load_payload(root)
            .map(|payload| payload.bundled)
            .unwrap_or_default(),
    };
    write_payload_record(root, &record)?;
    regenerate(root, &ledger, &compiled, &frame, &record)?;
    Ok(record)
}

/// Records the BYTES of the libraries the release packages but does not build.
///
/// `libflutter.so` comes from the Flutter SDK's artifact cache and `libapp.so`
/// from the Dart AOT compile, and AGP strips both on their way into the
/// archive — so the shipped bytes exist nowhere but the packaged artifact
/// itself. The packaging assertions write their facts BEFORE they fail, so the
/// build that first sees an unrecorded library leaves exactly what this needs:
/// run it, and the digests land in `registry/release-payload.toml` as the
/// reviewable diff where those bytes were accepted. A Flutter upgrade or any
/// Dart edit therefore fails the release once and is accepted on purpose.
pub fn record_bundled(root: &Path) -> CheckResult<PayloadRecord> {
    let ledger = load_ledger(root)?;
    let (compiled, frame) = compiled_identity()?;
    let mut record = load_payload(root)?;
    let mut accepted: BTreeMap<(String, String), String> = BTreeMap::new();
    for artifact in load_artifacts(root)? {
        for library in &artifact.facts.payload {
            let mut segments = library.artifact.rsplit('/');
            let (Some(soname), Some(abi)) = (segments.next(), segments.next()) else {
                continue;
            };
            if !ledger
                .policy
                .bundled_libraries
                .iter()
                .any(|name| name == soname)
            {
                continue;
            }
            let key = (soname.to_owned(), abi.to_owned());
            if let Some(seen) = accepted.get(&key)
                && seen != &library.sha256
            {
                return Err(format!(
                    "{soname} differs between this build's artifacts for {abi}: \
                     {seen} and {}",
                    library.sha256
                ));
            }
            accepted.insert(key, library.sha256.clone());
        }
    }
    record.bundled = accepted
        .into_iter()
        .map(|((soname, abi), sha256)| BundledLibrary {
            soname,
            abi,
            sha256,
        })
        .collect();
    write_payload_record(root, &record)?;
    regenerate(root, &ledger, &compiled, &frame, &record)?;
    Ok(record)
}

fn load_payload(root: &Path) -> CheckResult<PayloadRecord> {
    Ok(load_toml::<PayloadFile>(&root.join(PAYLOAD_FILE))?.payload)
}

/// The one serializer for the payload record. Two commands write it — the
/// cross-compile records the payload, the packaged artifact records the
/// bundled bytes — and each carries the other's rows forward untouched.
fn write_payload_record(root: &Path, record: &PayloadRecord) -> CheckResult<()> {
    let mut text = String::from(
        "# @generated by `cargo run -p xtask -- record-payload`, which\n\
         # scripts/build-jni-libs.sh runs the moment it finishes cross-compiling,\n\
         # and by `record-bundled`, which accepts the bytes of the libraries the\n\
         # release packages but does not build. Do not edit: this is the only\n\
         # accounting that ties the .so files an APK packages to the manifest and\n\
         # the contract sources they were built from, and gradle never invokes\n\
         # cargo.\n\n[payload]\n",
    );
    text.push_str(&format!(
        "build_manifest_sha256 = {:?}\nsources_sha256 = {:?}\n",
        record.build_manifest_sha256, record.sources_sha256
    ));
    for entry in &record.library {
        text.push_str(&format!(
            "\n[[payload.library]]\nbuild_type = {:?}\nabi = {:?}\nsha256 = {:?}\n",
            entry.build_type, entry.abi, entry.sha256
        ));
    }
    for entry in &record.bundled {
        text.push_str(&format!(
            "\n[[payload.bundled]]\nsoname = {:?}\nabi = {:?}\nsha256 = {:?}\n",
            entry.soname, entry.abi, entry.sha256
        ));
    }
    write_file(&root.join(PAYLOAD_FILE), text.as_bytes())
}

/// Rewrites the three records derived from the compiled build: the reviewable
/// identity declaration, the manifest asset every artifact embeds, and the flat
/// policy projection the packaging side reads.
pub fn regenerate(
    root: &Path,
    ledger: &ReleaseLedger,
    compiled: &BTreeMap<String, String>,
    frame: &[u8],
    payload: &PayloadRecord,
) -> CheckResult<()> {
    let mut text = String::from(
        "# @generated from the compiled build manifest (the L4 manifest composed\n\
         # with the L5 binding contracts). Do not edit; regenerate with\n\
         # `cargo run -p xtask -- record-payload`, which\n\
         # scripts/build-jni-libs.sh runs after every cross-compile.\n\
         #\n\
         # Every identity a release claims to speak. A change here is the\n\
         # reviewable record that this build no longer speaks what the last one\n\
         # did — the release gate refuses to package a build that drifted from\n\
         # this declaration.\n\n[identity]\n",
    );
    for (key, value) in compiled {
        text.push_str(&format!("{key} = {value:?}\n"));
    }
    write_file(&root.join(IDENTITY_FILE), text.as_bytes())?;
    write_file(&root.join(MANIFEST_ASSET), frame)?;
    write_file(
        &root.join(POLICY_FILE),
        render_policy(ledger, compiled, payload).as_bytes(),
    )
}

/// Every artifact the packaging side reported, each one re-read and re-hashed
/// here. A facts file that names no readable artifact still produces a verdict
/// — [`MeasuredArtifact::observed_sha256`] is `None` and the gate rejects it —
/// so a hand-written facts file cannot inflate the artifact count.
fn load_artifacts(root: &Path) -> CheckResult<Vec<MeasuredArtifact>> {
    let directory = root.join(FACTS_DIR);
    let entries = fs::read_dir(&directory).map_err(|error| {
        format!(
            "no packaging facts in {}: assemble a release first ({error})",
            directory.display()
        )
    })?;
    let mut paths = Vec::new();
    for entry in entries {
        let path = entry
            .map_err(|error| format!("reading {}: {error}", directory.display()))?
            .path();
        if path
            .extension()
            .is_some_and(|extension| extension == "toml")
        {
            paths.push(path);
        }
    }
    paths.sort();
    if paths.is_empty() {
        return Err(format!(
            "no packaging facts in {}: assemble a release first",
            directory.display()
        ));
    }
    paths
        .iter()
        .map(|path| {
            let facts = load_toml::<FactsFile>(path)?.facts;
            let observed_sha256 = read_file(&root.join(&facts.artifact))
                .ok()
                .map(|bytes| sha256_hex(&bytes));
            Ok(MeasuredArtifact {
                facts,
                observed_sha256,
            })
        })
        .collect()
}

/// The digest that pins the payload to the sources it was compiled from: every
/// file the ledger's `payload_sources` globs name — the whole compiled tree,
/// its manifests, the resolved lockfile, the binding contracts, and the cargo
/// configuration that decides the payload's exported symbol surface. Edit any
/// of them without rebuilding and the recorded payload is stale, which is what
/// `PayloadSourcesDrift` has always claimed and now covers.
///
/// The globs are the ONE enumeration: gradle's freshness guard reads the same
/// list out of the flat projection, so the two enforcers cannot disagree about
/// what a payload is built from. A glob that names no file is an error rather
/// than an empty contribution — a source set can go wrong by matching nothing.
fn sources_digest(root: &Path, ledger: &ReleaseLedger) -> CheckResult<String> {
    let mut files = BTreeSet::new();
    for pattern in &ledger.policy.payload_sources {
        let base: PathBuf = pattern
            .split('/')
            .take_while(|segment| !segment.contains('*'))
            .collect();
        let mut candidates = Vec::new();
        collect(&root.join(&base), &mut candidates);
        let mut matched = 0;
        for candidate in candidates {
            let relative = candidate
                .strip_prefix(root)
                .map_err(|error| format!("{}: {error}", candidate.display()))?
                .to_string_lossy()
                .into_owned();
            if matches_source_glob(&relative, pattern) {
                matched += 1;
                files.insert(relative);
            }
        }
        if matched == 0 {
            return Err(format!(
                "{LEDGER_FILE}: payload_sources pattern {pattern:?} names no file, \
                 so it contributes nothing to the payload digest"
            ));
        }
    }

    let mut index = String::new();
    for file in &files {
        index.push_str(&format!(
            "{file} {}\n",
            sha256_hex(&read_file(&root.join(file))?)
        ));
    }
    Ok(sha256_hex(index.as_bytes()))
}

/// Every file under `path`, or `path` itself when it names one.
fn collect(path: &Path, files: &mut Vec<PathBuf>) {
    if path.is_file() {
        files.push(path.to_owned());
        return;
    }
    let Ok(entries) = fs::read_dir(path) else {
        return;
    };
    for entry in entries.flatten() {
        collect(&entry.path(), files);
    }
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn read_file(path: &Path) -> CheckResult<Vec<u8>> {
    fs::read(path).map_err(|error| format!("reading {}: {error}", path.display()))
}

fn write_file(path: &Path, bytes: &[u8]) -> CheckResult<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("creating {}: {error}", parent.display()))?;
    }
    fs::write(path, bytes).map_err(|error| format!("writing {}: {error}", path.display()))
}
