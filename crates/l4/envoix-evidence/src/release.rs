//! Does a release-shaped artifact agree with the build that produced it?
//!
//! The gate judges the ARTIFACT, never the builder's claims about it. Every
//! verdict here is anchored to a file the caller re-read and re-hashed for
//! itself, and every "this must not be present" rule is expressed as an
//! allow-list, so an entry point or a packaged file nobody reviewed is a
//! failure by construction rather than a missing deny entry.
//!
//! The answer is a pure function over data, so it lives beside the build
//! manifest it judges rather than inside one gate's binary: D1's deployment
//! packaging and any host can ask the same question of their own artifacts.
//! Every fact it needs — the packaged identities, the shipped payload, the
//! signer — arrives as an INPUT, so this layer reads no file, runs no tool, and
//! gains no dependency on the layers above it (the L5 binding ids reach it
//! through the identity maps).
//!
//! [`render_policy`] is the other half of the same idea: the packaging side
//! must not re-parse this policy out of a hand-rolled TOML reader, so this
//! module projects the ledger into one flat, single-valued document that a
//! stock `java.util.Properties` parser reads. A mis-parse can then only produce
//! a different VALUE, never a different table.
//!
//! `tools/xtask`'s `release-gate` is the thin CLI over both: it loads the
//! registry files, composes the compiled identity out of the L5 projection,
//! re-hashes every artifact gradle reported, and calls [`check_release`].

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use serde::Deserialize;

/// How far a signed artifact may travel.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum Distribution {
    /// Signed with the real key, distributed to nobody.
    Internal,
    /// Distributed, so every deployment trust root must be provisioned.
    Public,
}

impl Distribution {
    /// Only a distributed artifact needs a provisioned deployment trust root.
    /// Both enforcers read this decision, never the spelling of the variant.
    pub fn requires_trust_root(self) -> bool {
        matches!(self, Self::Public)
    }

    /// The wire spelling, for the report line and the flat policy projection.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Internal => "internal",
            Self::Public => "public",
        }
    }
}

/// One (applicationId, versionCode) pair that has actually been released.
/// Append-only: the release action adds a row, nothing ever edits one.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ReleasedVersion {
    pub application_id: String,
    pub version_code: u64,
}

/// The release policy a repository has committed to, mirroring
/// `registry/release-ledger.toml`.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReleaseLedger {
    pub policy: ReleasePolicy,
    /// Every release that has actually happened. Empty until D2 publishes one.
    #[serde(default)]
    pub released: Vec<ReleasedVersion>,
}

/// The rules themselves. Every list is an ALLOW-list except
/// [`ReleasePolicy::forbidden_native_symbols`] and
/// [`ReleasePolicy::forbidden_manifest_markers`], which exist only to turn a
/// rejection into a message that names what was recognised.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReleasePolicy {
    pub signer_sha256: String,
    pub required_abis: Vec<String>,
    /// THE payload: the one library this repository builds and accounts for.
    pub native_library: String,
    /// Libraries the release packages but does not build, by soname. They are
    /// held to a different rule — see [`check_payload`] — and naming one here
    /// exempts nothing else: [`ReleasePolicy::native_library`] is still
    /// required at its own path, still hash-checked against the payload
    /// record, and still held to exactly [`ReleasePolicy::allowed_native_symbols`].
    #[serde(default)]
    pub bundled_libraries: Vec<String>,
    /// The complete exported surface of the release payload. A packaged
    /// library that exports anything else — or that is missing one of these —
    /// is rejected, so a `RegisterNatives`-style entry point with no exported
    /// name cannot hide behind a prefix rule.
    pub allowed_native_symbols: Vec<String>,
    /// Recognised debug/instrumentation prefixes. Not the rule, just the
    /// friendlier half of the message when the allow-list rejects one.
    pub forbidden_native_symbols: Vec<String>,
    /// The complete packaged surface of a release artifact, as
    /// `*`-in-one-segment patterns. Anything else in the archive is rejected by
    /// name. A bundle entry is mapped onto its APK spelling before it is judged
    /// here — see [`ArtifactKind::surface_entry`] — so one reviewed list covers
    /// both shapes.
    pub allowed_package_entries: Vec<String>,
    /// The complete set of container entries an app bundle may carry: the ones
    /// bundletool and AGP define, which have no APK counterpart to be judged
    /// as. Everything else in a bundle is app content.
    pub allowed_bundle_entries: Vec<String>,
    /// What the payload is built from, as Ant-style globs. Both enforcers read
    /// this one list, so neither can answer the question differently.
    pub payload_sources: Vec<String>,
    /// Recognised debug markers in the packaged app manifest.
    pub forbidden_manifest_markers: Vec<String>,
    /// Class names a release dex may never define.
    pub forbidden_release_classes: Vec<String>,
    pub distribution: Distribution,
}

/// What `scripts/build-jni-libs.sh` recorded about the payload it produced.
///
/// This is what turns "the sources declare X" into "the shipped binary is the
/// one that was built from X": gradle never invokes cargo, so without this
/// record the packaged `.so` is an unaccountable prebuilt.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PayloadRecord {
    /// The composed build manifest the payload was cross-compiled against.
    pub build_manifest_sha256: String,
    /// Digest over every contract source the payload was compiled from.
    pub sources_sha256: String,
    pub library: Vec<PayloadLibrary>,
    /// The bundled libraries as they were REVIEWED, per ABI. A soname is a
    /// filename, and a filename is not a trust decision: a library that
    /// registers its natives at load time exports nothing to judge, so the
    /// bytes are the only thing that can be judged at all.
    #[serde(default)]
    pub bundled: Vec<BundledLibrary>,
}

/// One cross-compiled library in the payload record.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PayloadLibrary {
    pub build_type: String,
    pub abi: String,
    pub sha256: String,
}

/// One bundled library's accepted bytes.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct BundledLibrary {
    pub soname: String,
    pub abi: String,
    pub sha256: String,
}

impl PayloadRecord {
    fn release_library(&self, abi: &str) -> Option<&PayloadLibrary> {
        self.library
            .iter()
            .find(|library| library.build_type == "release" && library.abi == abi)
    }

    fn bundled_library(&self, soname: &str, abi: &str) -> Option<&BundledLibrary> {
        self.bundled
            .iter()
            .find(|library| library.soname == soname && library.abi == abi)
    }
}

/// The `(abi, soname)` a packaged library's path names.
fn library_identity(artifact: &str) -> (&str, &str) {
    let mut segments = artifact.rsplit('/');
    let soname = segments.next().unwrap_or_default();
    let abi = segments.next().unwrap_or_default();
    (abi, soname)
}

/// Everything the build says about itself, gathered outside this layer.
#[derive(Clone, Debug)]
pub struct BuildIdentity {
    /// The checked-in declaration (`registry/release-identity.toml`).
    pub declared: BTreeMap<String, String>,
    /// What this build actually compiled, composed from the L5 projection.
    pub compiled: BTreeMap<String, String>,
    /// SHA-256 of the composed manifest frame this build encodes right now.
    pub manifest_sha256: String,
    /// Digest over the contract sources as they stand right now.
    pub sources_sha256: String,
    /// The payload record as checked in.
    pub payload: PayloadRecord,
    /// The checked-in flat policy projection the packaging side consumes.
    pub policy_projection: String,
}

/// Which kind of release artifact a facts file describes. An app bundle is a
/// first-class release artifact and carries the same assertions as the APK.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactKind {
    Apk,
    Bundle,
}

impl fmt::Display for ArtifactKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Apk => "apk",
            Self::Bundle => "bundle",
        })
    }
}

impl ArtifactKind {
    /// Where this kind of archive keeps a packaged native library.
    fn library_path(self, abi: &str, soname: &str) -> String {
        match self {
            Self::Apk => format!("lib/{abi}/{soname}"),
            Self::Bundle => format!("base/lib/{abi}/{soname}"),
        }
    }

    /// The reviewed-surface name this archive entry carries, or `None` when the
    /// entry belongs to the container rather than to the app.
    ///
    /// An APK IS the surface. A bundle keeps the same app content under
    /// module-scoped prefixes — `base/dex/`, `base/manifest/` and `base/root/`
    /// hold what an APK keeps at its root, and `base/assets/`, `base/lib/`,
    /// `base/res/` keep their directory — so stripping the prefix is what lets
    /// ONE allow-list judge both shapes. Anything else in a bundle is the
    /// container's own metadata, and is held to
    /// [`ReleasePolicy::allowed_bundle_entries`] instead: every entry is judged
    /// by exactly one of the two lists, and nothing is judged by neither.
    pub fn surface_entry(self, entry: &str) -> Option<&str> {
        match self {
            Self::Apk => Some(entry),
            Self::Bundle => {
                for prefix in ["base/dex/", "base/manifest/", "base/root/"] {
                    if let Some(rest) = entry.strip_prefix(prefix) {
                        return Some(rest);
                    }
                }
                let rest = entry.strip_prefix("base/")?;
                matches!(rest.split('/').next(), Some("assets" | "lib" | "res")).then_some(rest)
            }
        }
    }
}

/// What packaging observed about one release-shaped artifact it produced.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackagedFacts {
    pub variant: String,
    pub kind: ArtifactKind,
    pub application_id: String,
    /// Repository-relative path of the artifact these facts describe. The gate
    /// re-reads it, so facts about a file that is not there are worthless.
    pub artifact: String,
    pub artifact_sha256: String,
    pub version_code: u64,
    pub version_name: String,
    /// Every signer of the artifact. A release has exactly one.
    pub signers: Vec<String>,
    pub abis: Vec<String>,
    /// Every entry name the archive contains.
    pub entries: Vec<String>,
    /// Recognised debug markers found in the packaged app manifest.
    pub manifest_markers: Vec<String>,
    /// Packaged entries whose bytes carry PEM trust material.
    pub trust_material: Vec<String>,
    /// SHA-256 of the build-manifest asset the artifact actually carries.
    pub build_manifest_sha256: Option<String>,
    /// Forbidden class names the packaged dex still defines.
    pub release_classes: Vec<String>,
    pub payload: Vec<PackagedPayload>,
}

/// One shared object found inside the packaged artifact, wherever it sat.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackagedPayload {
    pub artifact: String,
    pub sha256: String,
    /// Every symbol the library's dynamic table DEFINES.
    pub symbols: Vec<String>,
}

/// One artifact the gate measured for itself: the facts packaging wrote, plus
/// the digest the gate computed by re-reading the named file.
#[derive(Clone, Debug)]
pub struct MeasuredArtifact {
    pub facts: PackagedFacts,
    /// `None` when the named artifact is missing or unreadable.
    pub observed_sha256: Option<String>,
}

/// One rule the release verdict is made of.
///
/// Naming them is what makes a clean verdict falsifiable. `disagreements=0` is
/// produced identically by a run that evaluated everything and by one that
/// evaluated nothing, which is how the packaged-surface allow-list stayed off
/// for app bundles from the commit that wrote it: the summary looked the same.
/// A verdict that lists what it ran cannot be produced by a run in which
/// nothing did.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Invariant {
    IdentityDrift,
    PolicyProjection,
    PayloadRecord,
    ArtifactAnchor,
    ReleasedVersions,
    AppVersion,
    Signers,
    Abis,
    PackagedPayload,
    PackagedSurface,
    ShippedManifest,
    ManifestMarkers,
    TrustMaterial,
    ReleaseClasses,
    TrustRoot,
}

impl Invariant {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::IdentityDrift => "identity_drift",
            Self::PolicyProjection => "policy_projection",
            Self::PayloadRecord => "payload_record",
            Self::ArtifactAnchor => "artifact_anchor",
            Self::ReleasedVersions => "released_versions",
            Self::AppVersion => "app_version",
            Self::Signers => "signers",
            Self::Abis => "abis",
            Self::PackagedPayload => "packaged_payload",
            Self::PackagedSurface => "packaged_surface",
            Self::ShippedManifest => "shipped_manifest",
            Self::ManifestMarkers => "manifest_markers",
            Self::TrustMaterial => "trust_material",
            Self::ReleaseClasses => "release_classes",
            Self::TrustRoot => "trust_root",
        }
    }
}

/// Whether an invariant ran, and over how much.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Evaluated {
    /// It ran, over this many facts. Zero facts is not the same as clean: a
    /// rule with nothing to look at reports exactly what a satisfied one does,
    /// which is why the number is printed rather than a tick.
    Judged(usize),
    /// It deliberately did not run, and why. The reason is DATA, stated once
    /// here, so a dormant rule cannot be spelled as a condition in two
    /// enforcers that then drift apart.
    Skipped(&'static str),
}

impl fmt::Display for Evaluated {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Judged(facts) => write!(formatter, "{facts}"),
            Self::Skipped(reason) => write!(formatter, "skipped({reason})"),
        }
    }
}

/// One invariant's record for one artifact, or for the build when it judges no
/// single artifact.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Evaluation {
    pub artifact: Option<String>,
    pub invariant: Invariant,
    pub evaluated: Evaluated,
}

/// The whole verdict: every invariant that was evaluated, and everything that
/// disagreed. The two halves are one value because a caller must not be able to
/// read the second without the first.
#[derive(Clone, Debug, Default)]
pub struct ReleaseVerdict {
    pub evaluations: Vec<Evaluation>,
    pub disagreements: Vec<Disagreement>,
}

impl ReleaseVerdict {
    fn judged(&mut self, artifact: Option<&str>, invariant: Invariant, facts: usize) {
        self.record(artifact, invariant, Evaluated::Judged(facts));
    }

    fn skipped(&mut self, artifact: Option<&str>, invariant: Invariant, reason: &'static str) {
        self.record(artifact, invariant, Evaluated::Skipped(reason));
    }

    fn record(&mut self, artifact: Option<&str>, invariant: Invariant, evaluated: Evaluated) {
        self.evaluations.push(Evaluation {
            artifact: artifact.map(ToOwned::to_owned),
            invariant,
            evaluated,
        });
    }

    fn disagree(&mut self, disagreement: Disagreement) {
        self.disagreements.push(disagreement);
    }
}

/// One specific way a release-shaped build failed to agree with itself. Every
/// variant names the pair that disagrees; none of them is a bare boolean.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Disagreement {
    /// The gate could not re-read the artifact these facts describe.
    ArtifactMissing { variant: String, artifact: String },
    /// The artifact on disk is not the one packaging reported.
    ArtifactDigestMismatch {
        variant: String,
        artifact: String,
        recorded: String,
        observed: String,
    },
    /// This (applicationId, versionCode) pair has already been released.
    VersionAlreadyReleased {
        variant: String,
        application_id: String,
        version_code: u64,
    },
    /// A release may never regress its applicationId's released versionCode.
    VersionRegression {
        variant: String,
        application_id: String,
        version_code: u64,
        last_released: u64,
    },
    /// The packaged app version and the compiled package version disagree.
    AppVersionMismatch {
        variant: String,
        version_name: String,
        package_version: String,
    },
    /// A protocol identifier the build compiled is not the one it declares.
    ProtocolIdDrift {
        field: String,
        declared: Option<String>,
        compiled: Option<String>,
    },
    /// An ABI or schema identifier the build compiled is not the one it
    /// declares.
    SchemaIdDrift {
        field: String,
        declared: Option<String>,
        compiled: Option<String>,
    },
    /// Any other declared manifest identity (package version, trust root).
    ManifestDrift {
        field: String,
        declared: Option<String>,
        compiled: Option<String>,
    },
    /// The flat policy the packaging side consumes is not the projection of
    /// the ledger — a divergent copy is a violation, never invisible.
    PolicyProjectionDrift { detail: String },
    /// A policy value cannot survive the flat projection unambiguously.
    PolicyValueAmbiguous { key: String, value: String },
    /// The artifact was signed by a key that is not the release identity.
    SignerMismatch {
        variant: String,
        expected: String,
        observed: String,
    },
    /// A release artifact has exactly one signer: no lineage, no co-signer.
    SignerCount { variant: String, signers: usize },
    /// A required ABI is not in the artifact.
    MissingAbi { variant: String, abi: String },
    /// The artifact ships an ABI the release does not claim.
    UnexpectedAbi { variant: String, abi: String },
    /// The artifact ships a shared object at a path the release does not claim.
    UnexpectedNativeLibrary { variant: String, artifact: String },
    /// A required payload library is not in the artifact.
    MissingNativeLibrary { variant: String, artifact: String },
    /// A packaged library is not the one the payload record accounts for.
    ShippedPayloadMismatch {
        variant: String,
        artifact: String,
        recorded: String,
        observed: String,
    },
    /// A debug/instrumentation entry point survived into the payload.
    DebugTrustMaterial {
        variant: String,
        artifact: String,
        symbol: String,
    },
    /// A packaged library exports something the release never allowed.
    UnexpectedNativeSymbol {
        variant: String,
        artifact: String,
        symbol: String,
    },
    /// A library the release packages but does not build exports one of the
    /// payload's own entry points. Two libraries answering to the same name is
    /// a lane nobody can reason about, so it fails whichever one loads first.
    ImpersonatedNativeSymbol {
        variant: String,
        artifact: String,
        symbol: String,
    },
    /// A packaged library is missing an entry point the release requires.
    MissingNativeSymbol {
        variant: String,
        artifact: String,
        symbol: String,
    },
    /// A bundled library whose bytes the release never accounted for. A soname
    /// is a filename; `RegisterNatives` needs no exported name at all, so the
    /// digest is the only thing that can decide whether this is the library
    /// that was reviewed.
    UnaccountedBundledLibrary { variant: String, artifact: String },
    /// A bundled library is not the one whose bytes were accepted.
    BundledLibraryMismatch {
        variant: String,
        artifact: String,
        recorded: String,
        observed: String,
    },
    /// The payload record was written against a different build manifest.
    PayloadManifestDrift { recorded: String, compiled: String },
    /// The payload record was written against different contract sources.
    PayloadSourcesDrift { recorded: String, observed: String },
    /// The artifact carries no build-manifest asset to identify itself by.
    ShippedManifestMissing { variant: String },
    /// The artifact identifies itself as a different build.
    ShippedManifestMismatch {
        variant: String,
        expected: String,
        observed: String,
    },
    /// The artifact packages something the release surface does not allow.
    UnexpectedPackageEntry { variant: String, entry: String },
    /// The packaged app manifest carries a debug-shaped marker.
    DebugManifestMarker { variant: String, marker: String },
    /// The artifact carries trust material.
    TestTrustMaterial { variant: String, entry: String },
    /// The packaged dex defines a class only a debug build may define.
    DebugClassInRelease { variant: String, class: String },
    /// A distributed release whose deployment trust root is still a blank slot.
    TrustRootUnprovisioned { variant: String },
}

impl fmt::Display for Disagreement {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ArtifactMissing { variant, artifact } => write!(
                formatter,
                "artifact: {variant} reports {artifact}, which the gate cannot read"
            ),
            Self::ArtifactDigestMismatch {
                variant,
                artifact,
                recorded,
                observed,
            } => write!(
                formatter,
                "artifact: {variant} reports {artifact} as {recorded}, \
                 but the file on disk hashes to {observed}"
            ),
            Self::VersionAlreadyReleased {
                variant,
                application_id,
                version_code,
            } => write!(
                formatter,
                "version: {variant} packages {application_id} versionCode {version_code}, \
                 which is already a released pair"
            ),
            Self::VersionRegression {
                variant,
                application_id,
                version_code,
                last_released,
            } => write!(
                formatter,
                "version: {variant} packages {application_id} versionCode {version_code}, \
                 but {last_released} is already released"
            ),
            Self::AppVersionMismatch {
                variant,
                version_name,
                package_version,
            } => write!(
                formatter,
                "version: {variant} packages versionName {version_name:?}, \
                 but the build compiled package version {package_version:?}"
            ),
            Self::ProtocolIdDrift {
                field,
                declared,
                compiled,
            } => write!(formatter, "protocol: {}", drift(field, declared, compiled)),
            Self::SchemaIdDrift {
                field,
                declared,
                compiled,
            } => write!(formatter, "schema: {}", drift(field, declared, compiled)),
            Self::ManifestDrift {
                field,
                declared,
                compiled,
            } => write!(formatter, "manifest: {}", drift(field, declared, compiled)),
            Self::PolicyProjectionDrift { detail } => write!(
                formatter,
                "policy: the flat projection the packaging side reads is not the ledger: {detail}"
            ),
            Self::PolicyValueAmbiguous { key, value } => write!(
                formatter,
                "policy: {key} carries {value:?}, which cannot survive the flat projection"
            ),
            Self::SignerMismatch {
                variant,
                expected,
                observed,
            } => write!(
                formatter,
                "signer: {variant} is signed by {observed}, \
                 but the release identity is {expected}"
            ),
            Self::SignerCount { variant, signers } => write!(
                formatter,
                "signer: {variant} carries {signers} signers, but a release has exactly one"
            ),
            Self::MissingAbi { variant, abi } => write!(
                formatter,
                "abi: {variant} ships no {abi} payload, but the release requires it"
            ),
            Self::UnexpectedAbi { variant, abi } => write!(
                formatter,
                "abi: {variant} ships an {abi} payload the release does not claim"
            ),
            Self::UnexpectedNativeLibrary { variant, artifact } => write!(
                formatter,
                "payload: {variant} packages {artifact}, which the release does not claim"
            ),
            Self::MissingNativeLibrary { variant, artifact } => write!(
                formatter,
                "payload: {variant} packages no {artifact}, but the release requires it"
            ),
            Self::ShippedPayloadMismatch {
                variant,
                artifact,
                recorded,
                observed,
            } => write!(
                formatter,
                "payload: {variant} packages {artifact} hashing to {observed}, \
                 but the payload record accounts for {recorded}"
            ),
            Self::DebugTrustMaterial {
                variant,
                artifact,
                symbol,
            } => write!(
                formatter,
                "trust: {variant} packages {artifact}, which exports the \
                 debug-only symbol {symbol}"
            ),
            Self::UnexpectedNativeSymbol {
                variant,
                artifact,
                symbol,
            } => write!(
                formatter,
                "payload: {variant} packages {artifact}, which exports {symbol}, \
                 an entry point the release does not allow"
            ),
            Self::ImpersonatedNativeSymbol {
                variant,
                artifact,
                symbol,
            } => write!(
                formatter,
                "payload: {variant} packages {artifact}, a library the release does \
                 not build, which exports the payload's own entry point {symbol}"
            ),
            Self::MissingNativeSymbol {
                variant,
                artifact,
                symbol,
            } => write!(
                formatter,
                "payload: {variant} packages {artifact}, which does not export \
                 the required entry point {symbol}"
            ),
            Self::UnaccountedBundledLibrary { variant, artifact } => write!(
                formatter,
                "payload: {variant} packages {artifact}, a bundled library whose \
                 bytes the release has not recorded: \
                 re-run `cargo run -p xtask -- record-bundled`"
            ),
            Self::BundledLibraryMismatch {
                variant,
                artifact,
                recorded,
                observed,
            } => write!(
                formatter,
                "payload: {variant} packages {artifact} hashing to {observed}, \
                 but the bundled record accepts {recorded}"
            ),
            Self::PayloadManifestDrift { recorded, compiled } => write!(
                formatter,
                "payload: the packaged libraries were built against build manifest \
                 {recorded}, but this build compiles {compiled}: \
                 re-run scripts/build-jni-libs.sh"
            ),
            Self::PayloadSourcesDrift { recorded, observed } => write!(
                formatter,
                "payload: the packaged libraries were built from sources {recorded}, \
                 but the tree is now {observed}: re-run scripts/build-jni-libs.sh"
            ),
            Self::ShippedManifestMissing { variant } => write!(
                formatter,
                "manifest: {variant} carries no build-manifest asset, so the shipped \
                 binary identifies itself as nothing"
            ),
            Self::ShippedManifestMismatch {
                variant,
                expected,
                observed,
            } => write!(
                formatter,
                "manifest: {variant} ships build manifest {observed}, \
                 but this build compiles {expected}"
            ),
            Self::UnexpectedPackageEntry { variant, entry } => write!(
                formatter,
                "surface: {variant} packages {entry}, which the release surface \
                 does not allow"
            ),
            Self::DebugManifestMarker { variant, marker } => write!(
                formatter,
                "trust: {variant} declares {marker}, which only a debug build may carry"
            ),
            Self::TestTrustMaterial { variant, entry } => write!(
                formatter,
                "trust: {variant} packages {entry}, which carries PEM trust material"
            ),
            Self::DebugClassInRelease { variant, class } => write!(
                formatter,
                "trust: {variant} defines {class}, which only a debug build may define"
            ),
            Self::TrustRootUnprovisioned { variant } => write!(
                formatter,
                "trust: {variant} is a public release, but the deployment trust \
                 root slot is unprovisioned"
            ),
        }
    }
}

fn drift(field: &str, declared: &Option<String>, compiled: &Option<String>) -> String {
    match (declared, compiled) {
        (Some(declared), Some(compiled)) => {
            format!("{field} is declared {declared:?} but compiled {compiled:?}")
        }
        (Some(declared), None) => {
            format!("{field} is declared {declared:?} but this build compiles no such identity")
        }
        (None, Some(compiled)) => {
            format!("{field} compiles as {compiled:?} but the declaration does not name it")
        }
        (None, None) => format!("{field} is named by neither side"),
    }
}

/// The flat, single-valued projection of the ledger that the packaging side
/// consumes. One `key=value` per line, lists joined by `,`, nothing nested —
/// so the enforcer that holds the artifact reads it with a stock properties
/// parser instead of a hand-rolled TOML reader.
pub fn render_policy(
    ledger: &ReleaseLedger,
    declared: &BTreeMap<String, String>,
    payload: &PayloadRecord,
) -> String {
    let policy = &ledger.policy;
    let mut text = String::from(
        "# @generated projection of registry/release-ledger.toml, the checked-in\n\
         # payload record and the generated identity declaration. Do not edit;\n\
         # regenerate with `cargo run -p xtask -- record-payload`, which\n\
         # scripts/build-jni-libs.sh runs after every cross-compile.\n\
         #\n\
         # The packaging enforcer reads THIS FILE AND NOTHING ELSE out of the\n\
         # registry, through a stock java.util.Properties parser. Flat and\n\
         # single-valued by construction, so a mis-parse can only produce a\n\
         # different value, never a different table; the release gate re-derives\n\
         # this text and rejects any divergence.\n\n",
    );
    let mut line = |key: &str, value: String| {
        text.push_str(key);
        text.push('=');
        text.push_str(&value);
        text.push('\n');
    };
    line("signer_sha256", policy.signer_sha256.clone());
    line("required_abis", policy.required_abis.join(","));
    line("native_library", policy.native_library.clone());
    line("bundled_libraries", policy.bundled_libraries.join(","));
    line(
        "allowed_native_symbols",
        policy.allowed_native_symbols.join(","),
    );
    line(
        "forbidden_native_symbols",
        policy.forbidden_native_symbols.join(","),
    );
    line(
        "allowed_package_entries",
        policy.allowed_package_entries.join(","),
    );
    line(
        "allowed_bundle_entries",
        policy.allowed_bundle_entries.join(","),
    );
    line("payload_sources", policy.payload_sources.join(","));
    line(
        "forbidden_manifest_markers",
        policy.forbidden_manifest_markers.join(","),
    );
    line(
        "forbidden_release_classes",
        policy.forbidden_release_classes.join(","),
    );
    line("distribution", policy.distribution.as_str().to_owned());
    // The typed decision, not the spelling of the variant: the packaging side
    // never compares `distribution` against a string literal.
    line(
        "trust_root_required",
        policy.distribution.requires_trust_root().to_string(),
    );
    line(
        "released",
        ledger
            .released
            .iter()
            .map(|released| format!("{}:{}", released.application_id, released.version_code))
            .collect::<Vec<_>>()
            .join(","),
    );
    line(
        "build_manifest_sha256",
        payload.build_manifest_sha256.clone(),
    );
    for library in &payload.library {
        line(
            &format!("payload_{}_{}_sha256", library.build_type, library.abi),
            library.sha256.clone(),
        );
    }
    for library in &payload.bundled {
        line(
            &format!("bundled_{}_{}_sha256", library.soname, library.abi),
            library.sha256.clone(),
        );
    }
    for (key, value) in declared {
        line(&format!("identity_{key}"), value.clone());
    }
    text
}

/// The whole release agreement, as a pure function over the policy, what the
/// build says about itself, and what packaging observed about every artifact
/// it produced — each one already re-hashed by the caller.
pub fn check_release(
    ledger: &ReleaseLedger,
    build: &BuildIdentity,
    artifacts: &[MeasuredArtifact],
) -> ReleaseVerdict {
    let mut verdict = ReleaseVerdict::default();
    check_identity_drift(build, &mut verdict);
    check_policy_projection(ledger, build, &mut verdict);
    check_payload_record(build, &mut verdict);

    let package_version = build.compiled.get("package_version");
    let provisioned = build
        .compiled
        .get("trust_root")
        .is_some_and(|slot| slot != "unprovisioned");
    for artifact in artifacts {
        check_artifact(
            ledger,
            build,
            artifact,
            package_version,
            provisioned,
            &mut verdict,
        );
    }
    verdict
}

fn check_identity_drift(build: &BuildIdentity, verdict: &mut ReleaseVerdict) {
    let fields: BTreeSet<&String> = build.declared.keys().chain(build.compiled.keys()).collect();
    verdict.judged(None, Invariant::IdentityDrift, fields.len());
    for field in fields {
        let declared = build.declared.get(field).cloned();
        let compiled = build.compiled.get(field).cloned();
        if declared == compiled {
            continue;
        }
        let field = field.clone();
        verdict.disagree(if field.starts_with("protocol_") {
            Disagreement::ProtocolIdDrift {
                field,
                declared,
                compiled,
            }
        } else if field.starts_with("abi_schema_") {
            Disagreement::SchemaIdDrift {
                field,
                declared,
                compiled,
            }
        } else {
            Disagreement::ManifestDrift {
                field,
                declared,
                compiled,
            }
        });
    }
}

fn check_policy_projection(
    ledger: &ReleaseLedger,
    build: &BuildIdentity,
    verdict: &mut ReleaseVerdict,
) {
    let policy = &ledger.policy;
    for (key, values) in [
        ("required_abis", &policy.required_abis),
        ("bundled_libraries", &policy.bundled_libraries),
        ("allowed_native_symbols", &policy.allowed_native_symbols),
        ("forbidden_native_symbols", &policy.forbidden_native_symbols),
        ("allowed_package_entries", &policy.allowed_package_entries),
        ("allowed_bundle_entries", &policy.allowed_bundle_entries),
        ("payload_sources", &policy.payload_sources),
        (
            "forbidden_manifest_markers",
            &policy.forbidden_manifest_markers,
        ),
        (
            "forbidden_release_classes",
            &policy.forbidden_release_classes,
        ),
    ] {
        for value in values {
            if value.contains([',', '\\', '\n', '\r']) {
                verdict.disagree(Disagreement::PolicyValueAmbiguous {
                    key: key.to_owned(),
                    value: value.clone(),
                });
            }
        }
    }

    let expected = render_policy(ledger, &build.declared, &build.payload);
    verdict.judged(None, Invariant::PolicyProjection, expected.lines().count());
    if expected == build.policy_projection {
        return;
    }
    let detail = expected
        .lines()
        .zip(build.policy_projection.lines())
        .find(|(expected, found)| expected != found)
        .map_or_else(
            || {
                format!(
                    "the projection has {} lines, the ledger renders {}",
                    build.policy_projection.lines().count(),
                    expected.lines().count()
                )
            },
            |(expected, found)| format!("expected {expected:?}, found {found:?}"),
        );
    verdict.disagree(Disagreement::PolicyProjectionDrift { detail });
}

fn check_payload_record(build: &BuildIdentity, verdict: &mut ReleaseVerdict) {
    // Two pins: the manifest the payload was compiled against, and the sources
    // it was compiled from.
    verdict.judged(None, Invariant::PayloadRecord, 2);
    if build.payload.build_manifest_sha256 != build.manifest_sha256 {
        verdict.disagree(Disagreement::PayloadManifestDrift {
            recorded: build.payload.build_manifest_sha256.clone(),
            compiled: build.manifest_sha256.clone(),
        });
    }
    if build.payload.sources_sha256 != build.sources_sha256 {
        verdict.disagree(Disagreement::PayloadSourcesDrift {
            recorded: build.payload.sources_sha256.clone(),
            observed: build.sources_sha256.clone(),
        });
    }
}

fn check_artifact(
    ledger: &ReleaseLedger,
    build: &BuildIdentity,
    artifact: &MeasuredArtifact,
    package_version: Option<&String>,
    provisioned: bool,
    verdict: &mut ReleaseVerdict,
) {
    let policy = &ledger.policy;
    let facts = &artifact.facts;
    let variant = facts.variant.clone();
    // The variant alone does not identify an artifact: one release build emits
    // an APK and a bundle under the same variant, and it is the bundle whose
    // rules went missing.
    let label = format!("{variant} {}", facts.kind);
    let label = Some(label.as_str());

    // Anchor first: a verdict about a file the gate could not read is not a
    // verdict, and neither is one about a file that has since changed.
    verdict.judged(label, Invariant::ArtifactAnchor, 1);
    match &artifact.observed_sha256 {
        None => verdict.disagree(Disagreement::ArtifactMissing {
            variant: variant.clone(),
            artifact: facts.artifact.clone(),
        }),
        Some(observed) if observed != &facts.artifact_sha256 => {
            verdict.disagree(Disagreement::ArtifactDigestMismatch {
                variant: variant.clone(),
                artifact: facts.artifact.clone(),
                recorded: facts.artifact_sha256.clone(),
                observed: observed.clone(),
            });
        }
        Some(_) => {}
    }

    verdict.judged(label, Invariant::ReleasedVersions, ledger.released.len());
    if ledger.released.iter().any(|released| {
        released.application_id == facts.application_id
            && released.version_code == facts.version_code
    }) {
        verdict.disagree(Disagreement::VersionAlreadyReleased {
            variant: variant.clone(),
            application_id: facts.application_id.clone(),
            version_code: facts.version_code,
        });
    }
    if let Some(last_released) = ledger
        .released
        .iter()
        .filter(|released| released.application_id == facts.application_id)
        .map(|released| released.version_code)
        .max()
        && facts.version_code < last_released
    {
        verdict.disagree(Disagreement::VersionRegression {
            variant: variant.clone(),
            application_id: facts.application_id.clone(),
            version_code: facts.version_code,
            last_released,
        });
    }

    verdict.judged(
        label,
        Invariant::AppVersion,
        usize::from(package_version.is_some()),
    );
    if package_version.is_some_and(|version| version != &facts.version_name) {
        verdict.disagree(Disagreement::AppVersionMismatch {
            variant: variant.clone(),
            version_name: facts.version_name.clone(),
            package_version: package_version.cloned().unwrap_or_default(),
        });
    }

    verdict.judged(label, Invariant::Signers, facts.signers.len());
    if facts.signers.len() != 1 {
        verdict.disagree(Disagreement::SignerCount {
            variant: variant.clone(),
            signers: facts.signers.len(),
        });
    }
    for signer in &facts.signers {
        if signer != &policy.signer_sha256 {
            verdict.disagree(Disagreement::SignerMismatch {
                variant: variant.clone(),
                expected: policy.signer_sha256.clone(),
                observed: signer.clone(),
            });
        }
    }

    verdict.judged(
        label,
        Invariant::Abis,
        policy.required_abis.len() + facts.abis.len(),
    );
    for abi in &policy.required_abis {
        if !facts.abis.contains(abi) {
            verdict.disagree(Disagreement::MissingAbi {
                variant: variant.clone(),
                abi: abi.clone(),
            });
        }
    }
    for abi in &facts.abis {
        if !policy.required_abis.contains(abi) {
            verdict.disagree(Disagreement::UnexpectedAbi {
                variant: variant.clone(),
                abi: abi.clone(),
            });
        }
    }

    check_payload(ledger, build, facts, &variant, label, verdict);

    // Every entry, in every shape of archive. An entry is app content, judged
    // against the reviewed surface under the name an APK would give it, or it
    // is this container's own metadata, judged against the bundle list — and
    // there is no third answer for one to fall into.
    verdict.judged(label, Invariant::PackagedSurface, facts.entries.len());
    for entry in &facts.entries {
        let (claimed, patterns) = match facts.kind.surface_entry(entry) {
            Some(surface) => (surface, &policy.allowed_package_entries),
            None => (entry.as_str(), &policy.allowed_bundle_entries),
        };
        if !patterns
            .iter()
            .any(|pattern| matches_pattern(claimed, pattern))
        {
            verdict.disagree(Disagreement::UnexpectedPackageEntry {
                variant: variant.clone(),
                entry: entry.clone(),
            });
        }
    }

    verdict.judged(label, Invariant::ShippedManifest, 1);
    match &facts.build_manifest_sha256 {
        None => verdict.disagree(Disagreement::ShippedManifestMissing {
            variant: variant.clone(),
        }),
        Some(observed) if observed != &build.manifest_sha256 => {
            verdict.disagree(Disagreement::ShippedManifestMismatch {
                variant: variant.clone(),
                expected: build.manifest_sha256.clone(),
                observed: observed.clone(),
            });
        }
        Some(_) => {}
    }

    // These two count the markers and classes the rule LOOKS for, not the ones
    // it found: a deny-list emptied to nothing reports the same clean facts as
    // an artifact that carries none.
    verdict.judged(
        label,
        Invariant::ManifestMarkers,
        policy.forbidden_manifest_markers.len(),
    );
    for marker in &facts.manifest_markers {
        verdict.disagree(Disagreement::DebugManifestMarker {
            variant: variant.clone(),
            marker: marker.clone(),
        });
    }
    verdict.judged(label, Invariant::TrustMaterial, facts.entries.len());
    for entry in &facts.trust_material {
        verdict.disagree(Disagreement::TestTrustMaterial {
            variant: variant.clone(),
            entry: entry.clone(),
        });
    }
    verdict.judged(
        label,
        Invariant::ReleaseClasses,
        policy.forbidden_release_classes.len(),
    );
    for class in &facts.release_classes {
        verdict.disagree(Disagreement::DebugClassInRelease {
            variant: variant.clone(),
            class: class.clone(),
        });
    }

    if policy.distribution.requires_trust_root() {
        verdict.judged(label, Invariant::TrustRoot, 1);
        if !provisioned {
            verdict.disagree(Disagreement::TrustRootUnprovisioned { variant });
        }
    } else {
        verdict.skipped(
            label,
            Invariant::TrustRoot,
            "only a distributed release needs a provisioned trust root",
        );
    }
}

fn check_payload(
    ledger: &ReleaseLedger,
    build: &BuildIdentity,
    facts: &PackagedFacts,
    variant: &str,
    label: Option<&str>,
    verdict: &mut ReleaseVerdict,
) {
    let policy = &ledger.policy;
    verdict.judged(label, Invariant::PackagedPayload, facts.payload.len());
    let allowed: BTreeSet<&String> = policy.allowed_native_symbols.iter().collect();
    let expected_paths: Vec<String> = policy
        .required_abis
        .iter()
        .map(|abi| facts.kind.library_path(abi, &policy.native_library))
        .collect();
    // Libraries the release packages but does not build. They are allowed to
    // EXIST, at exactly these paths and under exactly these names; they are
    // never allowed to answer for the payload, so this set is kept apart from
    // `expected_paths` rather than merged into it.
    let bundled_paths: BTreeSet<String> = policy
        .required_abis
        .iter()
        .flat_map(|abi| {
            policy
                .bundled_libraries
                .iter()
                .map(move |soname| facts.kind.library_path(abi, soname))
        })
        .collect();

    for path in &expected_paths {
        if !facts
            .payload
            .iter()
            .any(|library| &library.artifact == path)
        {
            verdict.disagree(Disagreement::MissingNativeLibrary {
                variant: variant.to_owned(),
                artifact: path.clone(),
            });
        }
    }

    for library in &facts.payload {
        // A library the release packages but does not build gets its own
        // toolchain's exported surface — and none of the payload's. It cannot
        // stand in for the payload either: the assertions below are keyed on
        // `expected_paths`, which no bundled name can enter.
        if bundled_paths.contains(&library.artifact) {
            // Its BYTES, not its name. A bundled library that binds the lane
            // from `JNI_OnLoad` exports nothing a symbol rule could see, so
            // being called `libflutter.so` has to stop being the trust
            // decision.
            let (abi, soname) = library_identity(&library.artifact);
            match build.payload.bundled_library(soname, abi) {
                None => verdict.disagree(Disagreement::UnaccountedBundledLibrary {
                    variant: variant.to_owned(),
                    artifact: library.artifact.clone(),
                }),
                Some(recorded) if recorded.sha256 != library.sha256 => {
                    verdict.disagree(Disagreement::BundledLibraryMismatch {
                        variant: variant.to_owned(),
                        artifact: library.artifact.clone(),
                        recorded: recorded.sha256.clone(),
                        observed: library.sha256.clone(),
                    });
                }
                Some(_) => {}
            }
            for symbol in &library.symbols {
                if allowed.contains(symbol) {
                    verdict.disagree(Disagreement::ImpersonatedNativeSymbol {
                        variant: variant.to_owned(),
                        artifact: library.artifact.clone(),
                        symbol: symbol.clone(),
                    });
                } else if policy
                    .forbidden_native_symbols
                    .iter()
                    .any(|prefix| symbol.starts_with(prefix))
                {
                    verdict.disagree(Disagreement::DebugTrustMaterial {
                        variant: variant.to_owned(),
                        artifact: library.artifact.clone(),
                        symbol: symbol.clone(),
                    });
                }
            }
            continue;
        }
        // The full path, not a suffix: `assets/lib/x/libenvoix_host_android.so`
        // is not the payload, it is a smuggled library.
        if !expected_paths.contains(&library.artifact) {
            verdict.disagree(Disagreement::UnexpectedNativeLibrary {
                variant: variant.to_owned(),
                artifact: library.artifact.clone(),
            });
        } else if let Some(abi) = library
            .artifact
            .rsplit('/')
            .nth(1)
            .and_then(|abi| build.payload.release_library(abi))
            && abi.sha256 != library.sha256
        {
            verdict.disagree(Disagreement::ShippedPayloadMismatch {
                variant: variant.to_owned(),
                artifact: library.artifact.clone(),
                recorded: abi.sha256.clone(),
                observed: library.sha256.clone(),
            });
        }

        for symbol in &library.symbols {
            if allowed.contains(symbol) {
                continue;
            }
            verdict.disagree(
                if policy
                    .forbidden_native_symbols
                    .iter()
                    .any(|prefix| symbol.starts_with(prefix))
                {
                    Disagreement::DebugTrustMaterial {
                        variant: variant.to_owned(),
                        artifact: library.artifact.clone(),
                        symbol: symbol.clone(),
                    }
                } else {
                    Disagreement::UnexpectedNativeSymbol {
                        variant: variant.to_owned(),
                        artifact: library.artifact.clone(),
                        symbol: symbol.clone(),
                    }
                },
            );
        }
        for symbol in &policy.allowed_native_symbols {
            if !library.symbols.contains(symbol) {
                verdict.disagree(Disagreement::MissingNativeSymbol {
                    variant: variant.to_owned(),
                    artifact: library.artifact.clone(),
                    symbol: symbol.clone(),
                });
            }
        }
    }
}

/// Does `entry` match one allow-list pattern? `*` stands for any run of
/// characters inside a single path segment; everything else is literal.
pub fn matches_pattern(entry: &str, pattern: &str) -> bool {
    let mut entry_segments = entry.split('/');
    let mut pattern_segments = pattern.split('/');
    loop {
        match (entry_segments.next(), pattern_segments.next()) {
            (Some(entry), Some(pattern)) if segment_matches(entry, pattern) => {}
            (None, None) => return true,
            _ => return false,
        }
    }
}

/// Does the repository-relative `path` match one `payload_sources` glob?
///
/// The Ant dialect gradle's `fileTree` already speaks, so the one enumeration
/// in the ledger can be handed to both enforcers verbatim: `**` stands for any
/// run of directories INCLUDING none, and `*` keeps the meaning it has in
/// [`matches_pattern`] — any run of characters inside a single segment.
pub fn matches_source_glob(path: &str, pattern: &str) -> bool {
    fn walk(path: &[&str], pattern: &[&str]) -> bool {
        match pattern.split_first() {
            None => path.is_empty(),
            Some((&"**", rest)) => (0..=path.len()).any(|skip| walk(&path[skip..], rest)),
            Some((first, rest)) => match path.split_first() {
                Some((segment, tail)) if segment_matches(segment, first) => walk(tail, rest),
                _ => false,
            },
        }
    }
    walk(
        &path.split('/').collect::<Vec<_>>(),
        &pattern.split('/').collect::<Vec<_>>(),
    )
}

fn segment_matches(segment: &str, pattern: &str) -> bool {
    let parts: Vec<&str> = pattern.split('*').collect();
    let [literal] = parts[..] else {
        let Some(mut rest) = segment.strip_prefix(parts[0]) else {
            return false;
        };
        for part in &parts[1..parts.len() - 1] {
            let Some(index) = rest.find(part) else {
                return false;
            };
            rest = &rest[index + part.len()..];
        }
        let suffix = parts[parts.len() - 1];
        return rest.len() >= suffix.len() && rest.ends_with(suffix);
    };
    segment == literal
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ledger() -> ReleaseLedger {
        ReleaseLedger {
            policy: ReleasePolicy {
                signer_sha256: "ab".repeat(32),
                required_abis: vec!["arm64-v8a".to_owned()],
                native_library: "libhost.so".to_owned(),
                bundled_libraries: vec!["libthird.so".to_owned()],
                allowed_native_symbols: vec!["Java_probe_boot".to_owned()],
                forbidden_native_symbols: vec!["Java_probe_E2e".to_owned()],
                allowed_package_entries: vec![
                    "AndroidManifest.xml".to_owned(),
                    "classes.dex".to_owned(),
                    "lib/*/libhost.so".to_owned(),
                    "lib/*/libthird.so".to_owned(),
                    "res/*.xml".to_owned(),
                    "res/*/*.xml".to_owned(),
                ],
                allowed_bundle_entries: vec!["BundleConfig.pb".to_owned()],
                payload_sources: vec!["Cargo.lock".to_owned()],
                forbidden_manifest_markers: vec![":debuggable(".to_owned()],
                forbidden_release_classes: vec!["E2eBridge".to_owned()],
                distribution: Distribution::Internal,
            },
            released: Vec::new(),
        }
    }

    fn payload_record() -> PayloadRecord {
        PayloadRecord {
            build_manifest_sha256: "11".repeat(32),
            sources_sha256: "22".repeat(32),
            library: vec![PayloadLibrary {
                build_type: "release".to_owned(),
                abi: "arm64-v8a".to_owned(),
                sha256: "33".repeat(32),
            }],
            bundled: vec![BundledLibrary {
                soname: "libthird.so".to_owned(),
                abi: "arm64-v8a".to_owned(),
                sha256: "66".repeat(32),
            }],
        }
    }

    fn identity(ledger: &ReleaseLedger) -> BuildIdentity {
        let map = BTreeMap::from([("package_version".to_owned(), "0.2.0".to_owned())]);
        let payload = payload_record();
        BuildIdentity {
            policy_projection: render_policy(ledger, &map, &payload),
            declared: map.clone(),
            compiled: map,
            manifest_sha256: payload.build_manifest_sha256.clone(),
            sources_sha256: payload.sources_sha256.clone(),
            payload,
        }
    }

    fn artifact() -> MeasuredArtifact {
        let facts = PackagedFacts {
            variant: "prodRelease".to_owned(),
            kind: ArtifactKind::Apk,
            application_id: "app.probe".to_owned(),
            artifact: "build/app.apk".to_owned(),
            artifact_sha256: "44".repeat(32),
            version_code: 1,
            version_name: "0.2.0".to_owned(),
            signers: vec!["ab".repeat(32)],
            abis: vec!["arm64-v8a".to_owned()],
            entries: vec![
                "classes.dex".to_owned(),
                "lib/arm64-v8a/libhost.so".to_owned(),
            ],
            manifest_markers: Vec::new(),
            trust_material: Vec::new(),
            build_manifest_sha256: Some("11".repeat(32)),
            release_classes: Vec::new(),
            payload: vec![PackagedPayload {
                artifact: "lib/arm64-v8a/libhost.so".to_owned(),
                sha256: "33".repeat(32),
                symbols: vec!["Java_probe_boot".to_owned()],
            }],
        };
        MeasuredArtifact {
            observed_sha256: Some(facts.artifact_sha256.clone()),
            facts,
        }
    }

    fn judge(mutate: impl FnOnce(&mut ReleaseLedger, &mut MeasuredArtifact)) -> ReleaseVerdict {
        let mut ledger = ledger();
        let mut artifact = artifact();
        mutate(&mut ledger, &mut artifact);
        let mut build = identity(&ledger);
        build.policy_projection = render_policy(&ledger, &build.declared, &build.payload);
        check_release(&ledger, &build, &[artifact])
    }

    fn verdict(
        mutate: impl FnOnce(&mut ReleaseLedger, &mut MeasuredArtifact),
    ) -> Vec<Disagreement> {
        judge(mutate).disagreements
    }

    #[test]
    fn an_agreeing_release_passes() {
        assert_eq!(verdict(|_, _| {}), Vec::new());
    }

    #[test]
    fn an_artifact_the_gate_cannot_re_read_is_not_a_verdict() {
        assert_eq!(
            verdict(|_, artifact| artifact.observed_sha256 = None),
            vec![Disagreement::ArtifactMissing {
                variant: "prodRelease".to_owned(),
                artifact: "build/app.apk".to_owned(),
            }]
        );
        assert_eq!(
            verdict(|_, artifact| artifact.observed_sha256 = Some("55".repeat(32))),
            vec![Disagreement::ArtifactDigestMismatch {
                variant: "prodRelease".to_owned(),
                artifact: "build/app.apk".to_owned(),
                recorded: "44".repeat(32),
                observed: "55".repeat(32),
            }]
        );
    }

    /// The released record is per applicationId: the same versionCode under a
    /// different id is a different release, and never a regression.
    #[test]
    fn released_versions_are_per_application_id() {
        assert_eq!(
            verdict(|ledger, _| ledger.released.push(ReleasedVersion {
                application_id: "app.probe".to_owned(),
                version_code: 1,
            })),
            vec![Disagreement::VersionAlreadyReleased {
                variant: "prodRelease".to_owned(),
                application_id: "app.probe".to_owned(),
                version_code: 1,
            }]
        );
        assert_eq!(
            verdict(|ledger, _| ledger.released.push(ReleasedVersion {
                application_id: "app.other".to_owned(),
                version_code: 9,
            })),
            Vec::new()
        );
        assert_eq!(
            verdict(|ledger, artifact| {
                ledger.released.push(ReleasedVersion {
                    application_id: "app.probe".to_owned(),
                    version_code: 4,
                });
                artifact.facts.version_code = 2;
            }),
            vec![Disagreement::VersionRegression {
                variant: "prodRelease".to_owned(),
                application_id: "app.probe".to_owned(),
                version_code: 2,
                last_released: 4,
            }]
        );
    }

    /// A signer set is exactly one key: a lineage rider is a second signer.
    #[test]
    fn exactly_one_signer_is_required() {
        let extra = "cd".repeat(32);
        assert_eq!(
            verdict(|_, artifact| artifact.facts.signers.push(extra.clone())),
            vec![
                Disagreement::SignerCount {
                    variant: "prodRelease".to_owned(),
                    signers: 2,
                },
                Disagreement::SignerMismatch {
                    variant: "prodRelease".to_owned(),
                    expected: "ab".repeat(32),
                    observed: extra,
                },
            ]
        );
        assert_eq!(
            verdict(|_, artifact| artifact.facts.signers.clear()),
            vec![Disagreement::SignerCount {
                variant: "prodRelease".to_owned(),
                signers: 0,
            }]
        );
    }

    /// The exported surface is an allow-list, so an entry point with a name
    /// nobody recognises fails exactly like a known debug one.
    #[test]
    fn the_native_surface_is_allow_listed_in_both_directions() {
        assert_eq!(
            verdict(|_, artifact| artifact.facts.payload[0]
                .symbols
                .push("Java_probe_E2ecreate".to_owned())),
            vec![Disagreement::DebugTrustMaterial {
                variant: "prodRelease".to_owned(),
                artifact: "lib/arm64-v8a/libhost.so".to_owned(),
                symbol: "Java_probe_E2ecreate".to_owned(),
            }]
        );
        assert_eq!(
            verdict(|_, artifact| artifact.facts.payload[0]
                .symbols
                .push("JNI_OnLoad".to_owned())),
            vec![Disagreement::UnexpectedNativeSymbol {
                variant: "prodRelease".to_owned(),
                artifact: "lib/arm64-v8a/libhost.so".to_owned(),
                symbol: "JNI_OnLoad".to_owned(),
            }]
        );
        assert_eq!(
            verdict(|_, artifact| artifact.facts.payload[0].symbols.clear()),
            vec![Disagreement::MissingNativeSymbol {
                variant: "prodRelease".to_owned(),
                artifact: "lib/arm64-v8a/libhost.so".to_owned(),
                symbol: "Java_probe_boot".to_owned(),
            }]
        );
    }

    /// A library the release packages but does not build is judged by a
    /// different rule, and naming it exempts nothing: it may export whatever
    /// its own toolchain exports, it may never export the payload's entry
    /// points or the debug lane, and the payload's own assertions are untouched
    /// by its presence.
    #[test]
    fn a_bundled_library_is_not_the_payload() {
        let third = PackagedPayload {
            artifact: "lib/arm64-v8a/libthird.so".to_owned(),
            sha256: "66".repeat(32),
            symbols: vec!["JNI_OnLoad".to_owned(), "_kDartVmSnapshotData".to_owned()],
        };
        // Its own surface is its own business, and the payload still has to be
        // there, hashed and exporting exactly the allow-list.
        assert_eq!(
            verdict(|_, artifact| {
                artifact.facts.payload.push(third.clone());
                artifact
                    .facts
                    .entries
                    .push("lib/arm64-v8a/libthird.so".to_owned());
            }),
            Vec::new()
        );
        // It cannot answer for the payload.
        assert_eq!(
            verdict(|_, artifact| {
                let mut impostor = third.clone();
                impostor.symbols.push("Java_probe_boot".to_owned());
                artifact.facts.payload.push(impostor);
                artifact
                    .facts
                    .entries
                    .push("lib/arm64-v8a/libthird.so".to_owned());
            }),
            vec![Disagreement::ImpersonatedNativeSymbol {
                variant: "prodRelease".to_owned(),
                artifact: "lib/arm64-v8a/libthird.so".to_owned(),
                symbol: "Java_probe_boot".to_owned(),
            }]
        );
        // Nor smuggle the debug lane in.
        assert_eq!(
            verdict(|_, artifact| {
                let mut impostor = third.clone();
                impostor.symbols.push("Java_probe_E2ecreate".to_owned());
                artifact.facts.payload.push(impostor);
                artifact
                    .facts
                    .entries
                    .push("lib/arm64-v8a/libthird.so".to_owned());
            }),
            vec![Disagreement::DebugTrustMaterial {
                variant: "prodRelease".to_owned(),
                artifact: "lib/arm64-v8a/libthird.so".to_owned(),
                symbol: "Java_probe_E2ecreate".to_owned(),
            }]
        );
        // And its presence never stands in for the payload's absence.
        assert_eq!(
            verdict(|_, artifact| {
                artifact.facts.payload = vec![third.clone()];
                artifact.facts.entries = vec!["lib/arm64-v8a/libthird.so".to_owned()];
            }),
            vec![Disagreement::MissingNativeLibrary {
                variant: "prodRelease".to_owned(),
                artifact: "lib/arm64-v8a/libhost.so".to_owned(),
            }]
        );
        // Its BYTES are what the release accepted. An unrecorded bundled
        // library, or one whose bytes are not the recorded ones, is rejected
        // even though its name and its path are both allowed — which is the
        // only rule that reaches a library binding the lane from `JNI_OnLoad`
        // with no exported name to judge.
        assert_eq!(
            verdict(|_, artifact| {
                let mut swapped = third.clone();
                swapped.sha256 = "77".repeat(32);
                artifact.facts.payload.push(swapped);
                artifact
                    .facts
                    .entries
                    .push("lib/arm64-v8a/libthird.so".to_owned());
            }),
            vec![Disagreement::BundledLibraryMismatch {
                variant: "prodRelease".to_owned(),
                artifact: "lib/arm64-v8a/libthird.so".to_owned(),
                recorded: "66".repeat(32),
                observed: "77".repeat(32),
            }]
        );
        assert_eq!(
            verdict(|ledger, artifact| {
                ledger
                    .policy
                    .bundled_libraries
                    .push("libfourth.so".to_owned());
                ledger
                    .policy
                    .allowed_package_entries
                    .push("lib/*/libfourth.so".to_owned());
                let mut unrecorded = third.clone();
                unrecorded.artifact = "lib/arm64-v8a/libfourth.so".to_owned();
                artifact.facts.payload.push(unrecorded);
                artifact
                    .facts
                    .entries
                    .push("lib/arm64-v8a/libfourth.so".to_owned());
            }),
            vec![Disagreement::UnaccountedBundledLibrary {
                variant: "prodRelease".to_owned(),
                artifact: "lib/arm64-v8a/libfourth.so".to_owned(),
            }]
        );
        // A bundled NAME at a path the release does not claim is still a
        // smuggled library, and it is judged as one — the bundled rule is keyed
        // on the full path, so being called `libthird.so` buys nothing.
        let smuggled = verdict(|_, artifact| {
            let mut elsewhere = third.clone();
            elsewhere.artifact = "assets/lib/arm64-v8a/libthird.so".to_owned();
            artifact.facts.payload.push(elsewhere);
            artifact
                .facts
                .entries
                .push("assets/lib/arm64-v8a/libthird.so".to_owned());
        });
        assert!(
            smuggled.contains(&Disagreement::UnexpectedNativeLibrary {
                variant: "prodRelease".to_owned(),
                artifact: "assets/lib/arm64-v8a/libthird.so".to_owned(),
            }) && smuggled.contains(&Disagreement::UnexpectedPackageEntry {
                variant: "prodRelease".to_owned(),
                entry: "assets/lib/arm64-v8a/libthird.so".to_owned(),
            }),
            "a bundled name at an unclaimed path must fail as a smuggled library, \
             got {smuggled:#?}"
        );
    }

    /// A library smuggled outside `lib/` is judged by its FULL path, so a
    /// suffix match cannot wave it through.
    #[test]
    fn a_library_outside_the_payload_path_is_rejected() {
        assert_eq!(
            verdict(|_, artifact| {
                artifact.facts.payload.push(PackagedPayload {
                    artifact: "assets/lib/arm64-v8a/libhost.so".to_owned(),
                    sha256: "33".repeat(32),
                    symbols: vec!["Java_probe_boot".to_owned()],
                });
                artifact
                    .facts
                    .entries
                    .push("assets/lib/arm64-v8a/libhost.so".to_owned());
            }),
            vec![
                Disagreement::UnexpectedNativeLibrary {
                    variant: "prodRelease".to_owned(),
                    artifact: "assets/lib/arm64-v8a/libhost.so".to_owned(),
                },
                Disagreement::UnexpectedPackageEntry {
                    variant: "prodRelease".to_owned(),
                    entry: "assets/lib/arm64-v8a/libhost.so".to_owned(),
                },
            ]
        );
    }

    /// The shipped bytes must be the ones the payload record accounts for.
    #[test]
    fn the_shipped_library_is_the_recorded_one() {
        assert_eq!(
            verdict(|_, artifact| artifact.facts.payload[0].sha256 = "99".repeat(32)),
            vec![Disagreement::ShippedPayloadMismatch {
                variant: "prodRelease".to_owned(),
                artifact: "lib/arm64-v8a/libhost.so".to_owned(),
                recorded: "33".repeat(32),
                observed: "99".repeat(32),
            }]
        );
    }

    /// The artifact identifies itself by the manifest asset it carries.
    #[test]
    fn the_shipped_manifest_identifies_the_build() {
        assert_eq!(
            verdict(|_, artifact| artifact.facts.build_manifest_sha256 = None),
            vec![Disagreement::ShippedManifestMissing {
                variant: "prodRelease".to_owned(),
            }]
        );
        assert_eq!(
            verdict(|_, artifact| artifact.facts.build_manifest_sha256 = Some("99".repeat(32))),
            vec![Disagreement::ShippedManifestMismatch {
                variant: "prodRelease".to_owned(),
                expected: "11".repeat(32),
                observed: "99".repeat(32),
            }]
        );
    }

    /// A file nobody reviewed is rejected by NAME, whatever it is called —
    /// which is what a deny-list of extensions can never do.
    #[test]
    fn the_packaged_surface_is_allow_listed() {
        for entry in ["assets/root.der", "assets/ca", "res/evil.txt"] {
            assert_eq!(
                verdict(|_, artifact| artifact.facts.entries.push(entry.to_owned())),
                vec![Disagreement::UnexpectedPackageEntry {
                    variant: "prodRelease".to_owned(),
                    entry: entry.to_owned(),
                }],
                "{entry} must not be allowed onto the release surface"
            );
        }
    }

    /// The bundle is the artifact that actually gets uploaded, and it is held
    /// to the SAME reviewed surface: app content is judged under the name an
    /// APK would give it, the container's own entries are named, and there is
    /// no third answer an entry can fall into.
    #[test]
    fn a_bundle_is_held_to_the_same_reviewed_surface() {
        fn bundle(artifact: &mut MeasuredArtifact) {
            artifact.facts.kind = ArtifactKind::Bundle;
            artifact.facts.payload[0].artifact = "base/lib/arm64-v8a/libhost.so".to_owned();
            artifact.facts.entries = vec![
                "BundleConfig.pb".to_owned(),
                "base/dex/classes.dex".to_owned(),
                "base/lib/arm64-v8a/libhost.so".to_owned(),
                "base/manifest/AndroidManifest.xml".to_owned(),
                "base/res/drawable-hdpi-v4/icon.xml".to_owned(),
            ];
        }
        assert_eq!(verdict(|_, artifact| bundle(artifact)), Vec::new());

        // App content under every module prefix, and container metadata under
        // none: an unreviewed entry fails wherever a bundle can keep one.
        for entry in [
            "base/root/root.der",
            "base/assets/ca",
            "base/lib/arm64-v8a/libsmuggled.so",
            "base/res/raw/evil.txt",
            "base/evil.pb",
            "BUNDLE-METADATA/whatever",
        ] {
            assert_eq!(
                verdict(|_, artifact| {
                    bundle(artifact);
                    artifact.facts.entries.push(entry.to_owned());
                }),
                vec![Disagreement::UnexpectedPackageEntry {
                    variant: "prodRelease".to_owned(),
                    entry: entry.to_owned(),
                }],
                "{entry} must not be allowed onto the release surface"
            );
        }
    }

    /// A clean verdict has to say what it looked at. Both artifact kinds report
    /// the surface rule over every entry they carry, and the one rule that
    /// deliberately does not run says so with its reason.
    #[test]
    fn the_verdict_names_the_invariants_it_evaluated() {
        for (kind, entries) in [
            (ArtifactKind::Apk, 2),
            // Same fixture, bundle spelling: five entries, all judged.
            (ArtifactKind::Bundle, 5),
        ] {
            let judged = judge(|_, artifact| {
                if kind == ArtifactKind::Bundle {
                    artifact.facts.kind = ArtifactKind::Bundle;
                    artifact.facts.payload[0].artifact = "base/lib/arm64-v8a/libhost.so".to_owned();
                    artifact.facts.entries = vec![
                        "BundleConfig.pb".to_owned(),
                        "base/dex/classes.dex".to_owned(),
                        "base/lib/arm64-v8a/libhost.so".to_owned(),
                        "base/manifest/AndroidManifest.xml".to_owned(),
                        "base/res/drawable-hdpi-v4/icon.xml".to_owned(),
                    ];
                }
            });
            let label = format!("prodRelease {kind}");
            let evaluated = |invariant: Invariant| {
                judged
                    .evaluations
                    .iter()
                    .find(|record| {
                        record.artifact.as_deref() == Some(label.as_str())
                            && record.invariant == invariant
                    })
                    .map(|record| record.evaluated)
            };
            assert_eq!(
                evaluated(Invariant::PackagedSurface),
                Some(Evaluated::Judged(entries)),
                "{label} must report the surface rule over every entry"
            );
            assert_eq!(
                evaluated(Invariant::TrustRoot),
                Some(Evaluated::Skipped(
                    "only a distributed release needs a provisioned trust root"
                )),
                "{label} must say WHY the trust-root rule did not run"
            );
        }
    }

    #[test]
    fn a_stale_payload_record_is_a_failure() {
        let mut ledger = ledger();
        let mut build = identity(&ledger);
        build.sources_sha256 = "77".repeat(32);
        build.manifest_sha256 = "88".repeat(32);
        ledger.released.clear();
        let disagreements = check_release(&ledger, &build, &[artifact()]).disagreements;
        assert!(
            disagreements.contains(&Disagreement::PayloadSourcesDrift {
                recorded: "22".repeat(32),
                observed: "77".repeat(32),
            }) && disagreements.contains(&Disagreement::PayloadManifestDrift {
                recorded: "11".repeat(32),
                compiled: "88".repeat(32),
            }),
            "a payload built from other sources must fail, got {disagreements:#?}"
        );
    }

    /// A divergent copy of the policy inside the file the packaging side reads
    /// is a violation, not an invisible second opinion.
    #[test]
    fn a_hand_edited_policy_projection_is_a_violation() {
        let ledger = ledger();
        let mut build = identity(&ledger);
        build.policy_projection = build
            .policy_projection
            .replace(&"ab".repeat(32), &"ff".repeat(32));
        let disagreements = check_release(&ledger, &build, &[artifact()]).disagreements;
        assert!(
            disagreements
                .iter()
                .any(|found| matches!(found, Disagreement::PolicyProjectionDrift { .. })),
            "a divergent projection must be a violation, got {disagreements:#?}"
        );
    }

    /// A public release with a blank trust-root slot fails; a provisioned one
    /// does not. The decision is the typed enum, never the string.
    #[test]
    fn a_public_release_requires_a_provisioned_trust_root() {
        assert!(!Distribution::Internal.requires_trust_root());
        assert!(Distribution::Public.requires_trust_root());

        let mut ledger = ledger();
        ledger.policy.distribution = Distribution::Public;
        let mut build = identity(&ledger);
        build
            .compiled
            .insert("trust_root".to_owned(), "unprovisioned".to_owned());
        build
            .declared
            .insert("trust_root".to_owned(), "unprovisioned".to_owned());
        build.policy_projection = render_policy(&ledger, &build.declared, &build.payload);
        assert_eq!(
            check_release(&ledger, &build, &[artifact()]).disagreements,
            vec![Disagreement::TrustRootUnprovisioned {
                variant: "prodRelease".to_owned(),
            }]
        );

        build
            .compiled
            .insert("trust_root".to_owned(), "sha256".to_owned());
        build
            .declared
            .insert("trust_root".to_owned(), "sha256".to_owned());
        build.policy_projection = render_policy(&ledger, &build.declared, &build.payload);
        assert_eq!(
            check_release(&ledger, &build, &[artifact()]).disagreements,
            Vec::new()
        );
    }

    #[test]
    fn allow_list_patterns_match_one_segment_at_a_time() {
        assert!(matches_pattern("lib/x86_64/libhost.so", "lib/*/libhost.so"));
        assert!(!matches_pattern("lib/a/b/libhost.so", "lib/*/libhost.so"));
        assert!(matches_pattern("res/aB.9.png", "res/*.png"));
        assert!(!matches_pattern("res/aB.png.der", "res/*.png"));
        assert!(matches_pattern("classes.dex", "classes.dex"));
        assert!(!matches_pattern("classes2.dex", "classes.dex"));
        assert!(matches_pattern("META-INF/x.version", "META-INF/*.version"));
    }

    /// The flat projection has to stay unambiguous, or the packaging side and
    /// the gate would read different lists out of the same line.
    #[test]
    fn an_unsplittable_policy_value_is_reported() {
        let mut ledger = ledger();
        ledger.policy.allowed_native_symbols.push("a,b".to_owned());
        let mut build = identity(&ledger);
        build.policy_projection = render_policy(&ledger, &build.declared, &build.payload);
        assert!(
            check_release(&ledger, &build, &[artifact()])
                .disagreements
                .contains(&Disagreement::PolicyValueAmbiguous {
                    key: "allowed_native_symbols".to_owned(),
                    value: "a,b".to_owned(),
                })
        );
    }
}
