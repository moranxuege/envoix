use std::num::NonZeroUsize;

use envoix_product::{CommittedSession, RecordDecode, decode_record};
use envoix_runtime::SessionProvider;
use envoix_types::RecordId;

use crate::store::HostStore;
use crate::stores::CardStores;

/// Restores a card's durable session from the process's live card stores.
pub struct HostProvider {
    stores: CardStores,
    max_commit_attempts: NonZeroUsize,
}

impl HostProvider {
    pub fn new(stores: CardStores, max_commit_attempts: NonZeroUsize) -> Self {
        Self {
            stores,
            max_commit_attempts,
        }
    }
}

impl SessionProvider for HostProvider {
    type Store = HostStore;

    fn restore(&self, card: RecordId) -> Option<CommittedSession<HostStore>> {
        let store = HostStore::opened(self.stores.clone(), card)?;
        let encoded = store.latest()?;
        let record = match decode_record(&encoded).ok()? {
            RecordDecode::Loaded(record) => record,
            // A future or corrupt record never restores here; the quarantine
            // path owns it (the runtime reports the card absent).
            RecordDecode::UnsupportedFuture { .. } => return None,
        };
        Some(CommittedSession::from_record(
            *record,
            store,
            self.max_commit_attempts,
        ))
    }
}
