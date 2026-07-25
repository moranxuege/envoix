use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};

use envoix_operation_store::OperationStore;
use envoix_storage_local::LocalStorage;
use envoix_types::RecordId;

/// One card's durable operation store, shared by every writer in the process.
pub type LiveStore = Arc<Mutex<OperationStore<LocalStorage>>>;

/// The registry's non-owning reference to a live store.
type StoreSlot = Weak<Mutex<OperationStore<LocalStorage>>>;

/// The process's live card stores — at most ONE per card, by construction.
///
/// The operation store's single-writer precondition is only real if one store
/// per card exists over one backend handle: two `LocalStorage::open` of the
/// same root hold independent in-memory writer leases, so they do not exclude
/// each other and their cached images fork. Every opener in this composition
/// (the session provider behind a card actor, and the boot outbox drainer)
/// takes its handle from here, so those two paths write through the same
/// mutex. Entries are weak: a card's store is dropped with its last holder,
/// and the next opener re-reads it from disk.
#[derive(Clone)]
pub struct CardStores {
    root: PathBuf,
    live: Arc<Mutex<HashMap<RecordId, StoreSlot>>>,
}

impl CardStores {
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            live: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The card's live store, opened if this is its first holder. `None` when
    /// the durable image is absent or unreadable.
    pub fn open(&self, card: RecordId) -> Option<LiveStore> {
        let mut live = self.lock();
        if let Some(store) = live.get(&card).and_then(Weak::upgrade) {
            return Some(store);
        }
        let storage = LocalStorage::open(&self.root).ok()?;
        let store = Arc::new(Mutex::new(OperationStore::open(storage, card).ok()?));
        live.insert(card, Arc::downgrade(&store));
        Some(store)
    }

    fn lock(&self) -> MutexGuard<'_, HashMap<RecordId, StoreSlot>> {
        self.live.lock().unwrap_or_else(PoisonError::into_inner)
    }
}
