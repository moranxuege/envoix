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

use std::path::PathBuf;

use serde::{Deserialize, Deserializer, Serialize, de};

use super::driver::{SessionContext, SessionParams};
use super::machine::Session;

/// One persisted transfer session.
#[derive(Clone, Debug, Serialize)]
pub struct TransferRecord {
    /// The frontend's card id — stable across restarts.
    pub id: u64,
    /// Last-write time, ms since the Unix epoch (display/GC only).
    pub updated_ms: u64,
    /// Complete immutable context needed to recreate the same Rust session.
    pub context: SessionContext,
    pub session: Session,
}

impl<'de> Deserialize<'de> for TransferRecord {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct Wire {
            id: u64,
            updated_ms: u64,
            context: Option<SessionContext>,
            params: Option<SessionParams>,
            session: Session,
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
        Ok(Self {
            id: wire.id,
            updated_ms: wire.updated_ms,
            context,
            session: wire.session,
        })
    }
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

    /// Write (or replace) a record atomically.
    pub async fn save(&self, record: &TransferRecord) -> std::io::Result<()> {
        tokio::fs::create_dir_all(&self.dir).await?;
        let bytes = serde_json::to_vec_pretty(record)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        let tmp = self.dir.join(format!(".record-{}.json.tmp", record.id));
        tokio::fs::write(&tmp, &bytes).await?;
        tokio::fs::rename(&tmp, self.path(record.id)).await?;
        Ok(())
    }

    /// Load every parseable record (unparseable files are skipped, not fatal —
    /// a corrupt record must never block app start).
    pub async fn load_all(&self) -> Vec<TransferRecord> {
        let mut records = Vec::new();
        let Ok(mut entries) = tokio::fs::read_dir(&self.dir).await else {
            return records;
        };
        while let Ok(Some(entry)) = entries.next_entry().await {
            let name = entry.file_name();
            let Some(name) = name.to_str() else { continue };
            if !name.starts_with("record-") || !name.ends_with(".json") {
                continue;
            }
            match tokio::fs::read(entry.path()).await {
                Ok(bytes) => match serde_json::from_slice::<TransferRecord>(&bytes) {
                    Ok(record) => records.push(record),
                    Err(error) => tracing::warn!(%error, name, "skipping unparseable record"),
                },
                Err(error) => tracing::warn!(%error, name, "skipping unreadable record"),
            }
        }
        records.sort_by_key(|r| r.id);
        records
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
            id,
            updated_ms: 1,
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
