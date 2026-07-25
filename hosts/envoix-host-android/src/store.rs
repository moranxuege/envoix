use std::sync::PoisonError;

use envoix_operation_store::RecordCommit;
use envoix_product::{CommitError, RecordDecode, RecordStore, decode_record};
use envoix_storage_api::Durability;
use envoix_types::RecordId;

use crate::stores::{CardStores, LiveStore};

/// The durable record store for one card, backed by the card's ONE live
/// operation store (see [`CardStores`]). Opens lazily so a backend that is
/// momentarily unavailable surfaces a typed commit failure instead of failing
/// boot.
pub struct HostStore {
    stores: CardStores,
    card: Option<RecordId>,
    operation: Option<LiveStore>,
}

impl HostStore {
    /// A store whose card identity is learned from the FIRST committed
    /// record (creation mints the identity inside the reducer, so the card
    /// id does not exist before the initial commit).
    pub fn deferred(stores: CardStores) -> Self {
        Self {
            stores,
            card: None,
            operation: None,
        }
    }

    /// The store with its backend already open, or `None` when the durable
    /// image is absent/unreadable (restore treats that as card-absent).
    pub fn opened(stores: CardStores, card: RecordId) -> Option<Self> {
        let operation = stores.open(card)?;
        Some(Self {
            stores,
            card: Some(card),
            operation: Some(operation),
        })
    }

    pub fn latest(&self) -> Option<Vec<u8>> {
        let operation = self.operation.as_ref()?;
        let operation = operation.lock().unwrap_or_else(PoisonError::into_inner);
        operation.latest_record().map(<[u8]>::to_vec)
    }

    fn operation(&mut self, card: RecordId) -> Result<&LiveStore, CommitError> {
        if self.operation.is_none() {
            self.operation = Some(self.stores.open(card).ok_or(CommitError)?);
        }
        Ok(self.operation.as_ref().expect("just opened"))
    }
}

impl RecordStore for HostStore {
    fn commit(&mut self, encoded: &[u8]) -> Result<(), CommitError> {
        let card = match self.card {
            Some(card) => card,
            None => {
                let RecordDecode::Loaded(record) =
                    decode_record(encoded).map_err(|_| CommitError)?
                else {
                    return Err(CommitError);
                };
                let card = record.identity.card;
                self.card = Some(card);
                card
            }
        };
        let operation = self.operation(card)?;
        let committed = operation
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .commit_record(encoded, Durability::Durable);
        match committed {
            Ok(RecordCommit::Committed { .. } | RecordCommit::AlreadyCommitted { .. }) => Ok(()),
            Err(_) => {
                // Drop this holder of the backend so the next attempt takes a
                // fresh handle from the registry (which reopens from disk once
                // the failing store's last holder is gone).
                self.operation = None;
                Err(CommitError)
            }
        }
    }
}
