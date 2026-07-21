//! uniffi bridge exposing the envoix client core to native UIs (Swift/Kotlin).
//!
//! The bridge is intentionally thin: it wires the unified envoix client API to a
//! small, foreign-implementable observer.
//! All networking, pairing, and transfer logic stays in the Rust core.
//!
//! Operations are non-blocking. Each call spawns work on a session-owned tokio
//! runtime and returns immediately; results arrive through [`TransferObserver`]
//! callbacks, which fire on runtime threads — the UI must hop to its own main
//! thread before touching UI state.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use envoix_client::{
    BindAddrs, PeerDescriptor, TransferDirection, TransferSummary,
    api::{
        Client, DataPath, ErrorKind, FailureCategory, FailureCode, FailureOrigin, FailurePhase,
        Invite, PairingStep, PathPolicy, PeerSource, Phase, RecoveryAction, Role,
        SessionFailureCode, StampedEvent, Transfer, TransferError, TransferEvent, TransferMode,
        TransferOptions,
        driver::{
            ClientContext, SessionContext, SessionNotice, SessionParams, SessionSnapshot,
            TransferSession as CanonicalTransferSession,
        },
        machine::{PauseOrigin, State as CanonicalState},
        record::{EXTERNAL_RECORD_ID_KEY, RecordStore, TransferRecord, stable_record_id},
    },
};
use envoix_qr::QrInvitePayload;
use envoix_rendezvous_iroh::generate_code;
use envoix_storage::LocalFileStorage;
use envoix_types::TransferId;
use tokio::runtime::{Handle, Runtime};
use tokio::sync::{mpsc, oneshot};

uniffi::setup_scaffolding!();

include!("ffi_bridge_api.rs");
include!("ffi_bridge_runtime.rs");

mod session;
pub(crate) use session::*;
