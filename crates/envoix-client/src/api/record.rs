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
mod tests {
    use super::super::machine::State;
    use super::*;
    use envoix_session::TransferDirection;

    fn record(id: u64) -> TransferRecord {
        TransferRecord {
            version: RECORD_VERSION,
            id,
            created_ms: 1,
            updated_ms: 1,
            platform_extras: None,
            context: SessionContext {
                client: Default::default(),
                params: SessionParams {
                    direction: TransferDirection::Receive,
                    path: "/tmp/x".into(),
                    sources: vec![super::super::PeerSource::Room {
                        code: "123456-kelp-coral".into(),
                        broker: "id@1.2.3.4:5".into(),
                    }],
                    options: super::super::TransferOptions::default(),
                    publication_required: false,
                },
            },
            session: Session::new(TransferDirection::Receive),
        }
    }

    #[tokio::test]
    async fn save_load_delete_round_trip() {
        let dir = std::env::temp_dir().join(format!("envoix-records-{}", std::process::id()));
        let _ = tokio::fs::remove_dir_all(&dir).await;
        let store = RecordStore::new(&dir);

        let mut r = record(7);
        r.session.state = State::Unconfirmed;
        r.session.transfer_id = Some("transfer-x".into());
        store.save(&r).await.unwrap();
        store.save(&record(3)).await.unwrap();

        let loaded = store.load_all().await;
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded[0].id, 3, "sorted by id");
        assert_eq!(loaded[1].session.state, State::Unconfirmed);
        assert_eq!(loaded[1].session.transfer_id.as_deref(), Some("transfer-x"),);
        assert_eq!(
            loaded[1].context.params.sources,
            record(7).context.params.sources,
            "relaunch context survives"
        );

        store.delete(3).await;
        assert_eq!(store.load_all().await.len(), 1);

        // A corrupt file is skipped, never fatal.
        tokio::fs::write(dir.join("record-9.json"), b"{nope")
            .await
            .unwrap();
        assert_eq!(store.load_all().await.len(), 1);
    }

    #[test]
    fn restore_context_summarizes_the_typed_record() {
        let mut r = record(5); // a Room receive by default
        r.context.params.path = "/out/dir".into();
        let ctx = r.restore_context();
        assert_eq!(ctx.id, 5);
        assert_eq!(ctx.direction, "receive");
        assert_eq!(ctx.code, "123456-kelp-coral");
        assert_eq!(ctx.path, "/out/dir");
        assert!(ctx.use_room);
        assert!(!ctx.use_mdns);
    }

    #[test]
    fn source_ready_migrates_from_state_for_legacy_records() {
        // A pre-v2 record lacks source_ready; the migration derives it from
        // state (a bare serde default of false would wrongly re-stage every
        // past-staging record). Serialize a legacy record with a deliberately
        // WRONG source_ready and confirm the migration overrides it.
        let migrated = |state: State, extras: Option<serde_json::Value>| -> bool {
            let mut r = record(1);
            r.version = 0; // legacy
            r.session = Session::new(TransferDirection::Send);
            r.session.state = state;
            r.session.facts.source_ready = true; // wrong on purpose
            r.platform_extras = extras;
            let json = serde_json::to_string(&r).unwrap();
            serde_json::from_str::<TransferRecord>(&json)
                .unwrap()
                .session
                .facts
                .source_ready
        };
        let staged = || Some(serde_json::json!({ "source_uri": "content://x" }));
        assert!(!migrated(State::Preparing, None), "Preparing -> not ready");
        assert!(migrated(State::Connecting, None), "past staging -> ready");
        assert!(migrated(State::Completed, None), "completed -> ready");
        assert!(
            !migrated(State::Cancelled, staged()),
            "cancelled staged -> re-stage",
        );
        assert!(
            migrated(State::Cancelled, None),
            "cancelled direct -> ready"
        );
    }

    #[test]
    fn restore_context_needs_no_frontend_migration_for_legacy_records() {
        // A pre-context record (params at the top level) deserializes via the
        // typed migration, so restore_context reads it with no fallback - the
        // whole reason the frontend can drop its `context ?: params` dance.
        let mut value = serde_json::to_value(record(9)).unwrap();
        let object = value.as_object_mut().unwrap();
        let context = object.remove("context").unwrap();
        object.insert("params".into(), context["params"].clone());
        let loaded: TransferRecord = serde_json::from_value(value).unwrap();

        let ctx = loaded.restore_context();
        assert_eq!(ctx.id, 9);
        assert_eq!(ctx.code, "123456-kelp-coral");
        assert!(ctx.use_room);
    }

    #[tokio::test]
    async fn discard_record_cleans_artifacts_without_a_live_session() {
        use envoix_storage::{LocalFileStorage, TransferReceipt, TransferResumeState};
        let dir = std::env::temp_dir().join(format!("envoix-discard-{}", std::process::id()));
        let _ = tokio::fs::remove_dir_all(&dir).await;
        tokio::fs::create_dir_all(&dir).await.unwrap();
        let store = RecordStore::new(dir.join("records"));

        // A paused receive: record + partial + state + receipt on disk.
        let mut r = record(4);
        r.context.params.path = dir.clone();
        r.session.file_name = Some("f.bin".into());
        r.session.transfer_id = Some("t-1".into());
        store.save(&r).await.unwrap();
        let tid = envoix_types::TransferId::new("t-1");
        let temp = LocalFileStorage::resumable_temp_path(&dir, "f.bin", &tid).unwrap();
        tokio::fs::write(&temp, b"partial").await.unwrap();
        LocalFileStorage::write_resume_state(
            &dir,
            &TransferResumeState {
                transfer_id: tid.clone(),
                file_name: "f.bin".into(),
                file_size: 100,
                chunk_size: 10,
                bytes_received: 7,
                next_chunk_index: 1,
                hash_bytes: 7,
                hash_checkpoint: None,
                target_file_name: None,
            },
        )
        .await
        .unwrap();
        LocalFileStorage::write_receipt(
            &dir,
            &TransferReceipt {
                transfer_id: tid.clone(),
                file_name: "f.bin".into(),
                file_size: 100,
                file_hash: "h".into(),
            },
        )
        .await
        .unwrap();

        // No live session anywhere: Remove still cleans everything.
        discard_record(&store, 4).await;

        assert!(store.load(4).await.is_none(), "record deleted");
        assert!(
            !tokio::fs::try_exists(&temp).await.unwrap(),
            "partial deleted"
        );
        assert!(
            LocalFileStorage::read_receipt(&dir, "f.bin")
                .await
                .unwrap()
                .is_none(),
            "receipt deleted"
        );
        assert!(
            LocalFileStorage::find_resume_state(&dir, "f.bin", 100, 10)
                .await
                .unwrap()
                .is_none(),
            "state deleted"
        );
        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    #[tokio::test]
    async fn platform_extras_survive_the_round_trip() {
        let dir = std::env::temp_dir().join(format!("envoix-extras-{}", std::process::id()));
        let _ = tokio::fs::remove_dir_all(&dir).await;
        let store = RecordStore::new(&dir);
        let mut r = record(11);
        r.platform_extras =
            Some(serde_json::json!({"qr": "envoix:abc", "saved_uri": "content://x"}));
        store.save(&r).await.unwrap();

        let loaded = store.load(11).await.unwrap();
        assert_eq!(loaded.version, RECORD_VERSION);
        assert_eq!(
            loaded.platform_extras.unwrap()["qr"],
            serde_json::json!("envoix:abc"),
            "the core persists the frontend's context verbatim"
        );
        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    #[tokio::test]
    async fn legacy_string_id_migrates_without_duplicate_cards() {
        let dir = std::env::temp_dir().join(format!("envoix-string-id-{}", std::process::id()));
        let _ = tokio::fs::remove_dir_all(&dir).await;
        tokio::fs::create_dir_all(&dir).await.unwrap();
        let store = RecordStore::new(&dir);
        let external_id = "activity-550e8400-e29b-41d4-a716-446655440000";
        let mut value = serde_json::to_value(record(12)).unwrap();
        value["id"] = external_id.into();
        tokio::fs::write(
            dir.join(format!("record-{external_id}.json")),
            serde_json::to_vec_pretty(&value).unwrap(),
        )
        .await
        .unwrap();

        let loaded = store.load_all().await;
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].id, stable_record_id(external_id));
        assert_eq!(external_record_id(&loaded[0]), Some(external_id));

        store.save(&loaded[0]).await.unwrap();
        assert!(
            !dir.join(format!("record-{external_id}.json")).exists(),
            "saving the adapted record removes the legacy filename"
        );
        assert_eq!(store.load_all().await.len(), 1);
        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    #[test]
    fn deserialize_legacy_params_record_as_default_context() {
        let mut value = serde_json::to_value(record(9)).unwrap();
        let object = value.as_object_mut().unwrap();
        let context = object.remove("context").unwrap();
        object.insert("params".into(), context["params"].clone());

        let loaded: TransferRecord = serde_json::from_value(value).unwrap();

        assert_eq!(loaded.id, 9);
        assert_eq!(loaded.context.client.chunk_size, None);
        assert_eq!(loaded.context.params.path, PathBuf::from("/tmp/x"));
    }
}
