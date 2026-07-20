//! Durable transfer records (roadmap #5): one JSON file per transfer session,
//! written by the driver on every state change. A record is exactly the
//! immutable launch context plus the serializable machine state. What it buys:
//! - resume across app restarts (the relaunch parameters survive the process),
//! - receipt confirmation across restarts (a restored Unconfirmed session
//!   resumes its mailbox poll),
//! - transfer history for free (Completed records are kept).
//!
//! Lifecycle (per the decided semantics): Cancel KEEPS the record; Remove is
//! the one true abandon — record, partials, and receipt all deleted.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Deserializer, Serialize, de};

use super::driver::{SessionContext, SessionParams};
use super::machine::Session;

/// Current record schema version, written on every save. Old records
/// deserialize as 0 (fields were only ever added with serde defaults); v2 adds
/// `session.facts.source_ready`, which needs a state-derived migration rather
/// than a bare default (see [`migrate_source_ready`]).
pub const RECORD_VERSION: u32 = 2;
/// Platform-extra key carrying a frontend's original string card identifier.
pub const EXTERNAL_RECORD_ID_KEY: &str = "external_record_id";

/// Pre-v2 records lack `source_ready`; a bare serde default (`false`) would
/// wrongly re-stage every past-staging record. Derive it from the persisted
/// state instead: only a staged send rests in `Preparing` (source not yet
/// complete); anything past staging — and every receive — has its source in
/// hand. A `Cancelled` send is ambiguous (mid-staging vs after): classify by the
/// staging marker (a `source_uri` in the platform extras) — a one-time migration
/// read only — so a mid-staging cancel re-stages while a direct send stays ready.
fn migrate_source_ready(session: &mut Session, extras: &Option<serde_json::Value>) {
    use super::machine::State;
    session.facts.source_ready = match session.state {
        State::Preparing => false,
        State::Cancelled => !has_staging_source(extras),
        _ => true,
    };
}

fn has_staging_source(extras: &Option<serde_json::Value>) -> bool {
    extras
        .as_ref()
        .and_then(|v| v.get("source_uri"))
        .and_then(serde_json::Value::as_str)
        .is_some_and(|s| !s.is_empty())
}

/// One persisted transfer session.
#[derive(Clone, Debug, Serialize)]
pub struct TransferRecord {
    /// Schema version stamp (see [`RECORD_VERSION`]).
    #[serde(default)]
    pub version: u32,
    /// The frontend's card id — stable across restarts.
    pub id: u64,
    /// First creation time, ms since the Unix epoch.
    pub created_ms: u64,
    /// Last-write time, ms since the Unix epoch (display/GC only).
    pub updated_ms: u64,
    /// Complete immutable context needed to recreate the same Rust session.
    pub context: SessionContext,
    pub session: Session,
    /// Opaque frontend-owned card context (e.g. Android's QR payload and
    /// saved URI). Persisted verbatim, returned on restore, never read by the
    /// core: the record is the ONE durable home for a card, but lifecycle
    /// authority stays with `session`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub platform_extras: Option<serde_json::Value>,
}

impl<'de> Deserialize<'de> for TransferRecord {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct Wire {
            #[serde(default)]
            version: u32,
            id: RecordId,
            #[serde(default)]
            created_ms: Option<u64>,
            updated_ms: u64,
            context: Option<SessionContext>,
            params: Option<SessionParams>,
            session: Session,
            #[serde(default)]
            platform_extras: Option<serde_json::Value>,
        }

        let wire = Wire::deserialize(deserializer)?;
        let context = match (wire.context, wire.params) {
            (Some(context), _) => context,
            (None, Some(params)) => SessionContext {
                client: Default::default(),
                params,
            },
            (None, None) => {
                return Err(de::Error::missing_field("context"));
            }
        };
        let (id, external_id) = wire.id.into_parts();
        let mut platform_extras = wire.platform_extras;
        if let Some(external_id) = external_id {
            let extras = platform_extras.get_or_insert_with(|| serde_json::json!({}));
            if let Some(object) = extras.as_object_mut() {
                object
                    .entry(EXTERNAL_RECORD_ID_KEY)
                    .or_insert_with(|| external_id.into());
            }
        }
        let mut session = wire.session;
        if wire.version < RECORD_VERSION {
            migrate_source_ready(&mut session, &platform_extras);
        }
        Ok(Self {
            version: wire.version,
            id,
            created_ms: wire.created_ms.unwrap_or(wire.updated_ms),
            updated_ms: wire.updated_ms,
            context,
            session,
            platform_extras,
        })
    }
}

#[derive(Deserialize)]
#[serde(untagged)]
enum RecordId {
    Number(u64),
    String(String),
}

impl RecordId {
    fn into_parts(self) -> (u64, Option<String>) {
        match self {
            Self::Number(value) => (value, None),
            Self::String(value) => (stable_record_id(&value), Some(value)),
        }
    }
}

/// The narrow, frontend-facing summary of a persisted record: everything a
/// frontend needs to rehydrate a card and its platform effects, and nothing
/// else. Built from the TYPED record, so the frontend never hand-parses record
/// JSON (nor re-implements its schema migrations) - it reads flat fields.
/// Platform-specific extras (e.g. Android's QR / saved URI) are added by the
/// platform glue, which knows those keys; the core does not.
#[derive(Clone, Debug, Serialize)]
pub struct RestoreContext {
    pub id: u64,
    /// "send" | "receive".
    pub direction: &'static str,
    /// The room code, or the mDNS token; the frontend's card label.
    pub code: String,
    /// The transfer's output/staging path.
    pub path: String,
    pub use_room: bool,
    pub use_mdns: bool,
}

impl TransferRecord {
    /// The typed restore summary (see [`RestoreContext`]). Reads the already-
    /// migrated typed record, so there is no legacy `context`/`params` fallback
    /// to duplicate on the frontend.
    pub fn restore_context(&self) -> RestoreContext {
        use envoix_session::TransferDirection;
        let params = &self.context.params;
        let mut code = String::new();
        let mut use_room = false;
        let mut use_mdns = false;
        for source in &params.sources {
            match source {
                super::PeerSource::Room { code: c, .. } => {
                    use_room = true;
                    code = c.clone();
                }
                super::PeerSource::Mdns { token } => {
                    use_mdns = true;
                    if let Some(token) = token
                        && code.is_empty()
                    {
                        code = token.clone();
                    }
                }
                _ => {}
            }
        }
        RestoreContext {
            id: self.id,
            direction: match params.direction {
                TransferDirection::Send => "send",
                TransferDirection::Receive => "receive",
            },
            code,
            path: params.path.to_string_lossy().into_owned(),
            use_room,
            use_mdns,
        }
    }
}

/// Stable adapter for frontends whose public card identifiers are strings.
/// Decimal identifiers retain their historic numeric value; other strings use
/// deterministic FNV-1a so the mapping survives process and app restarts.
pub fn stable_record_id(value: &str) -> u64 {
    if let Ok(id) = value.parse::<u64>() {
        return id;
    }
    const OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
    const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;
    value.as_bytes().iter().fold(OFFSET_BASIS, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(FNV_PRIME)
    })
}

/// Filesystem store: `<dir>/record-<id>.json`, atomic writes.
#[derive(Clone, Debug)]
pub struct RecordStore {
    dir: PathBuf,
}

impl RecordStore {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    fn path(&self, id: u64) -> PathBuf {
        self.dir.join(format!("record-{id}.json"))
    }

    /// Private endpoint identity for one durable transfer. A separate key per
    /// card avoids relay identity collisions between concurrent receivers.
    pub fn identity_path(&self, id: u64) -> PathBuf {
        self.dir
            .join("identities")
            .join(format!("identity-{id}.json"))
    }

    /// Write (or replace) a record atomically.
    pub async fn save(&self, record: &TransferRecord) -> std::io::Result<()> {
        tokio::fs::create_dir_all(&self.dir).await?;
        let bytes = serde_json::to_vec_pretty(record)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        let tmp = self.dir.join(format!(".record-{}.json.tmp", record.id));
        tokio::fs::write(&tmp, &bytes).await?;
        tokio::fs::rename(&tmp, self.path(record.id)).await?;
        if let Some(external_id) = external_record_id(record) {
            let legacy_path = self.dir.join(format!(
                "record-{}.json",
                legacy_record_file_key(external_id)
            ));
            if legacy_path != self.path(record.id) {
                let _ = tokio::fs::remove_file(legacy_path).await;
            }
        }
        Ok(())
    }

    /// Load every parseable record (unparseable files are skipped, not fatal —
    /// a corrupt record must never block app start).
    pub async fn load_all(&self) -> Vec<TransferRecord> {
        let mut records = BTreeMap::new();
        let Ok(mut entries) = tokio::fs::read_dir(&self.dir).await else {
            return Vec::new();
        };
        while let Ok(Some(entry)) = entries.next_entry().await {
            let name = entry.file_name();
            let Some(name) = name.to_str() else { continue };
            if !name.starts_with("record-") || !name.ends_with(".json") {
                continue;
            }
            match tokio::fs::read(entry.path()).await {
                Ok(bytes) => match serde_json::from_slice::<TransferRecord>(&bytes) {
                    Ok(record) => {
                        let replace =
                            records
                                .get(&record.id)
                                .is_none_or(|current: &TransferRecord| {
                                    record.updated_ms >= current.updated_ms
                                });
                        if replace {
                            records.insert(record.id, record);
                        }
                    }
                    Err(error) => tracing::warn!(%error, name, "skipping unparseable record"),
                },
                Err(error) => tracing::warn!(%error, name, "skipping unreadable record"),
            }
        }
        records.into_values().collect()
    }

    /// Load one record by id, if present and parseable.
    pub async fn load(&self, id: u64) -> Option<TransferRecord> {
        let bytes = tokio::fs::read(self.path(id)).await.ok()?;
        match serde_json::from_slice::<TransferRecord>(&bytes) {
            Ok(record) => Some(record),
            Err(error) => {
                tracing::warn!(%error, id, "unparseable record");
                None
            }
        }
    }

    /// Delete a record (Remove — the one true abandon). Missing is fine.
    pub async fn delete(&self, id: u64) {
        let _ = tokio::fs::remove_file(self.path(id)).await;
        let _ = tokio::fs::remove_file(self.identity_path(id)).await;
    }
}

fn external_record_id(record: &TransferRecord) -> Option<&str> {
    record
        .platform_extras
        .as_ref()?
        .as_object()?
        .get(EXTERNAL_RECORD_ID_KEY)?
        .as_str()
}

fn legacy_record_file_key(id: &str) -> String {
    if !id.is_empty()
        && id.len() <= 128
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        id.to_string()
    } else {
        blake3::hash(id.as_bytes()).to_hex().to_string()
    }
}

/// Delete a transfer's resumable partial and its state sidecar.
pub(crate) async fn discard_partial_files(
    dir: &std::path::Path,
    file_name: Option<&str>,
    transfer_id: Option<&str>,
) {
    use envoix_storage::LocalFileStorage;
    let (Some(name), Some(tid)) = (file_name, transfer_id) else {
        return;
    };
    let tid = envoix_types::TransferId::new(tid.to_owned());
    if let Err(error) = LocalFileStorage::delete_resume_temp(dir, name, &tid).await {
        tracing::debug!(%error, "discard: temp");
    }
    if let Err(error) = LocalFileStorage::delete_resume_state(dir, name, &tid).await {
        tracing::debug!(%error, "discard: state");
    }
}

/// Delete every on-disk artifact of a transfer: partial, resume state, and
/// completion receipt. The one D2 implementation, shared by the live actor
/// (`Cmd::Discard`) and the dead-session path below.
pub(crate) async fn discard_artifacts(
    dir: &std::path::Path,
    file_name: Option<&str>,
    transfer_id: Option<&str>,
) {
    use envoix_storage::LocalFileStorage;
    discard_partial_files(dir, file_name, transfer_id).await;
    if let Some(name) = file_name
        && let Err(error) = LocalFileStorage::delete_receipt(dir, name).await
    {
        tracing::debug!(%error, "discard: receipt");
    }
}

/// Record-authoritative Remove (D2) without a live session: the record is the
/// authority for a transfer's existence — a live driver is an optimization,
/// and its absence must never make Remove skip cleanup (the record would
/// resurrect the card on the next restore).
pub async fn discard_record(store: &RecordStore, id: u64) {
    if let Some(record) = store.load(id).await {
        discard_artifacts(
            &record.context.params.path,
            record.session.file_name.as_deref(),
            record.session.transfer_id.as_deref(),
        )
        .await;
    }
    store.delete(id).await;
}

/// Current Unix time in whole milliseconds.
pub(crate) fn unix_now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[cfg(test)]
#[path = "record_tests.rs"]
mod tests;
