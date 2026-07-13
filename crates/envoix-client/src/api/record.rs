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
    pub id: String,
    /// First creation time, ms since the Unix epoch.
    pub created_ms: u64,
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
            id: RecordId,
            #[serde(default)]
            created_ms: Option<u64>,
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
            id: wire.id.into_string(),
            created_ms: wire.created_ms.unwrap_or(wire.updated_ms),
            updated_ms: wire.updated_ms,
            context,
            session: wire.session,
        })
    }
}

#[derive(Deserialize)]
#[serde(untagged)]
enum RecordId {
    String(String),
    LegacyNumber(u64),
}

impl RecordId {
    fn into_string(self) -> String {
        match self {
            Self::String(value) => value,
            Self::LegacyNumber(value) => value.to_string(),
        }
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

    fn path(&self, id: &str) -> PathBuf {
        self.dir
            .join(format!("record-{}.json", record_file_key(id)))
    }

    /// Private endpoint identity for one durable transfer. A separate key per
    /// card avoids relay identity collisions between concurrent receivers.
    pub fn identity_path(&self, id: &str) -> PathBuf {
        self.dir
            .join("identities")
            .join(format!("identity-{}.json", record_file_key(id)))
    }

    /// Write (or replace) a record atomically.
    pub async fn save(&self, record: &TransferRecord) -> std::io::Result<()> {
        tokio::fs::create_dir_all(&self.dir).await?;
        let bytes = serde_json::to_vec_pretty(record)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        let tmp = self
            .dir
            .join(format!(".record-{}.json.tmp", record_file_key(&record.id)));
        tokio::fs::write(&tmp, &bytes).await?;
        tokio::fs::rename(&tmp, self.path(&record.id)).await?;
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
        records.sort_by(|left, right| left.id.cmp(&right.id));
        records
    }

    /// Delete a record (Remove — the one true abandon). Missing is fine.
    pub async fn delete(&self, id: &str) {
        let _ = tokio::fs::remove_file(self.path(id)).await;
        let _ = tokio::fs::remove_file(self.identity_path(id)).await;
    }
}

fn record_file_key(id: &str) -> String {
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

    fn record(id: impl ToString) -> TransferRecord {
        TransferRecord {
            id: id.to_string(),
            created_ms: 1,
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
        assert_eq!(loaded[0].id, "3", "sorted by id");
        assert_eq!(loaded[1].session.state, State::Unconfirmed);
        assert_eq!(loaded[1].session.transfer_id.as_deref(), Some("transfer-x"),);
        assert_eq!(
            loaded[1].context.params.sources,
            record(7).context.params.sources,
            "relaunch context survives"
        );

        store.delete("3").await;
        assert_eq!(store.load_all().await.len(), 1);

        // A corrupt file is skipped, never fatal.
        tokio::fs::write(dir.join("record-9.json"), b"{nope")
            .await
            .unwrap();
        assert_eq!(store.load_all().await.len(), 1);
    }

    #[tokio::test]
    async fn string_ids_are_stable_and_cannot_escape_the_record_directory() {
        let dir =
            std::env::temp_dir().join(format!("envoix-record-string-ids-{}", std::process::id()));
        let _ = tokio::fs::remove_dir_all(&dir).await;
        let store = RecordStore::new(&dir);

        store.save(&record("activity-uuid_1")).await.unwrap();
        store.save(&record("../outside")).await.unwrap();

        let safe_identity = store.identity_path("../outside");
        assert!(safe_identity.starts_with(dir.join("identities")));
        assert!(!safe_identity.to_string_lossy().contains("../outside"));

        let loaded = store.load_all().await;
        assert_eq!(loaded.len(), 2);
        assert!(loaded.iter().any(|record| record.id == "activity-uuid_1"));
        assert!(loaded.iter().any(|record| record.id == "../outside"));
        assert!(dir.join("record-activity-uuid_1.json").is_file());
        assert_eq!(record_file_key("../outside").len(), 64);
        assert!(!dir.parent().unwrap().join("outside.json").exists());

        store.delete("../outside").await;
        assert_eq!(store.load_all().await.len(), 1);
        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    #[test]
    fn deserialize_legacy_params_record_as_default_context() {
        let mut value = serde_json::to_value(record(9)).unwrap();
        let object = value.as_object_mut().unwrap();
        let context = object.remove("context").unwrap();
        object.remove("created_ms");
        object.insert("id".into(), serde_json::json!(9));
        object.insert("params".into(), context["params"].clone());

        let loaded: TransferRecord = serde_json::from_value(value).unwrap();

        assert_eq!(loaded.id, "9");
        assert_eq!(loaded.created_ms, loaded.updated_ms);
        assert_eq!(loaded.context.client.chunk_size, None);
        assert_eq!(loaded.context.params.path, PathBuf::from("/tmp/x"));
    }
}
