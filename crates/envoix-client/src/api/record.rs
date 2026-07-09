//! Durable transfer records (roadmap #5): one JSON file per transfer session,
//! written by the driver on every state change. A record is exactly
//! `(SessionParams, Session)` — the machine state was designed serializable
//! for this. What it buys:
//! - resume across app restarts (the relaunch parameters survive the process),
//! - receipt confirmation across restarts (a restored Unconfirmed session
//!   resumes its mailbox poll),
//! - transfer history for free (Completed records are kept).
//!
//! Lifecycle (per the decided semantics): Cancel KEEPS the record; Remove is
//! the one true abandon — record, partials, and receipt all deleted.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use super::driver::SessionParams;
use super::machine::Session;

/// One persisted transfer session.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct TransferRecord {
    /// The frontend's card id — stable across restarts.
    pub id: u64,
    /// Last-write time, ms since the Unix epoch (display/GC only).
    pub updated_ms: u64,
    pub params: SessionParams,
    pub session: Session,
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

    /// Delete a record (Remove — the one true abandon). Missing is fine.
    pub async fn delete(&self, id: u64) {
        let _ = tokio::fs::remove_file(self.path(id)).await;
    }
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
            params: SessionParams {
                direction: TransferDirection::Receive,
                path: "/tmp/x".into(),
                sources: vec![super::super::PeerSource::Room {
                    code: "123456-kelp-coral".into(),
                    broker: "id@1.2.3.4:5".into(),
                }],
                options: super::super::TransferOptions::default(),
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
            loaded[1].params.sources,
            record(7).params.sources,
            "relaunch params survive"
        );

        store.delete(3).await;
        assert_eq!(store.load_all().await.len(), 1);

        // A corrupt file is skipped, never fatal.
        tokio::fs::write(dir.join("record-9.json"), b"{nope")
            .await
            .unwrap();
        assert_eq!(store.load_all().await.len(), 1);
    }
}
