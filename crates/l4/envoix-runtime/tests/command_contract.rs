//! BN2 durable-command-contract proof over the real operation store.
//!
//! Provenance is the live subscription; acceptance and committed completion
//! are separate channels; the `CommandId` ledger rides the L3 record, so the
//! dedup fact and the effect are atomic — exactly once across a hot restart.

use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use envoix_attempt_api::AttemptPlan;
use envoix_operation_store::OperationStore;
use envoix_product::{
    CommitError, CommittedSession, IdentityError, IdentitySource, LedgerHit, NewTransfer,
    ProductCommand, ProductState, RecordDecode, RecordStore, SourceDecision, TransferRecord,
    decode_record,
};
use envoix_runtime::{
    AttemptExecution, AttemptExecutor, CommandCompletion, CommandRejected, CommandVerdict, Runtime,
    RuntimeConfig, SessionProvider, stop_channel,
};
use envoix_storage_api::Durability;
use envoix_storage_local::LocalStorage;
use envoix_types::{ByteCount, CommandId, Direction, OfferedName, RecordId};
use tempfile::TempDir;
use tokio::sync::mpsc;

// ---- scriptable durable store over the real operation store ----

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CommitScript {
    Healthy,
    /// Every commit attempt (including the escalation's best-effort write)
    /// fails — the durable record cannot change.
    FailAll,
    /// Commits park until the script leaves this mode, exposing the window
    /// between acceptance and completion.
    Block,
    /// The next commit panics, unwinding the actor between acceptance and
    /// completion — the deterministic "death mid-command" for the
    /// `Interrupted` disambiguation proof.
    Panic,
}

struct ScriptState {
    mode: Mutex<CommitScript>,
    changed: Condvar,
    parked: Mutex<usize>,
    parked_changed: Condvar,
}

#[derive(Clone)]
struct Script(Arc<ScriptState>);

impl Script {
    fn healthy() -> Self {
        Self(Arc::new(ScriptState {
            mode: Mutex::new(CommitScript::Healthy),
            changed: Condvar::new(),
            parked: Mutex::new(0),
            parked_changed: Condvar::new(),
        }))
    }

    fn set(&self, mode: CommitScript) {
        *self.0.mode.lock().unwrap() = mode;
        self.0.changed.notify_all();
    }

    /// Blocks the TEST thread until one commit is parked in `Block` mode.
    fn wait_parked(&self) {
        let mut parked = self.0.parked.lock().unwrap();
        while *parked == 0 {
            parked = self.0.parked_changed.wait(parked).unwrap();
        }
    }

    /// Called by the store from the actor thread; returns the mode to obey,
    /// waiting out any `Block` window first.
    fn gate(&self) -> CommitScript {
        let mut mode = self.0.mode.lock().unwrap();
        if *mode == CommitScript::Block {
            *self.0.parked.lock().unwrap() += 1;
            self.0.parked_changed.notify_all();
            while *mode == CommitScript::Block {
                mode = self.0.changed.wait(mode).unwrap();
            }
            *self.0.parked.lock().unwrap() -= 1;
        }
        *mode
    }
}

struct ScriptedStore {
    root: PathBuf,
    script: Script,
    operation: Option<OperationStore<LocalStorage>>,
}

impl ScriptedStore {
    fn deferred(root: &Path, script: Script) -> Self {
        Self {
            root: root.to_path_buf(),
            script,
            operation: None,
        }
    }

    fn opened(root: &Path, card: RecordId, script: Script) -> Option<Self> {
        let storage = LocalStorage::open(root).ok()?;
        let operation = OperationStore::open(storage, card).ok()?;
        Some(Self {
            root: root.to_path_buf(),
            script,
            operation: Some(operation),
        })
    }

    fn latest(&self) -> Option<Vec<u8>> {
        self.operation.as_ref()?.latest_record().map(<[u8]>::to_vec)
    }
}

impl RecordStore for ScriptedStore {
    fn commit(&mut self, encoded: &[u8]) -> Result<(), CommitError> {
        match self.script.gate() {
            CommitScript::FailAll => return Err(CommitError),
            CommitScript::Panic => {
                self.script.set(CommitScript::Healthy);
                panic!("scripted mid-commit death");
            }
            CommitScript::Healthy | CommitScript::Block => {}
        }
        let card = match decode_record(encoded).map_err(|_| CommitError)? {
            RecordDecode::Loaded(record) => record.identity.card,
            RecordDecode::UnsupportedFuture { .. } => return Err(CommitError),
        };
        if self.operation.is_none() {
            let storage = LocalStorage::open(&self.root).map_err(|_| CommitError)?;
            self.operation = Some(OperationStore::open(storage, card).map_err(|_| CommitError)?);
        }
        let operation = self.operation.as_mut().ok_or(CommitError)?;
        if operation.record_id() != card {
            return Err(CommitError);
        }
        operation
            .commit_record(encoded, Durability::Durable)
            .map_err(|_| CommitError)?;
        Ok(())
    }
}

struct ScriptedProvider {
    root: PathBuf,
    script: Script,
}

impl SessionProvider for ScriptedProvider {
    type Store = ScriptedStore;

    fn restore(&self, card: RecordId) -> Option<CommittedSession<ScriptedStore>> {
        let store = ScriptedStore::opened(&self.root, card, self.script.clone())?;
        let encoded = store.latest()?;
        let record = match decode_record(&encoded).ok()? {
            RecordDecode::Loaded(record) => record,
            RecordDecode::UnsupportedFuture { .. } => return None,
        };
        Some(CommittedSession::from_record(
            record,
            store,
            NonZeroUsize::new(3).unwrap(),
        ))
    }
}

/// An attempt executor that does nothing until stopped — the card stays live
/// with a `Running` worker so commands target a live actor.
struct InertExecutor;

impl AttemptExecutor for InertExecutor {
    fn start(&self, _plan: AttemptPlan) -> AttemptExecution {
        let (signal_tx, signals) = mpsc::channel(4);
        let (stop, token) = stop_channel();
        tokio::spawn(async move {
            token.stopped().await;
            let _ = signal_tx
                .send(envoix_runtime::ExecutorSignal::Stopped)
                .await;
        });
        AttemptExecution { signals, stop }
    }
}

// ---- helpers ----

struct FixedIdentity {
    next: u8,
}

impl IdentitySource for FixedIdentity {
    fn fill(&mut self, destination: &mut [u8]) -> Result<(), IdentityError> {
        for byte in destination {
            *byte = self.next;
            self.next = self.next.wrapping_add(1);
        }
        Ok(())
    }
}

fn config() -> RuntimeConfig {
    RuntimeConfig::new(
        NonZeroUsize::new(8).unwrap(),
        Duration::from_secs(2),
        NonZeroUsize::new(3).unwrap(),
    )
}

fn create_card(
    root: &Path,
    script: &Script,
    seed: u8,
) -> (
    CommittedSession<ScriptedStore>,
    envoix_product::ApplyOutcome,
) {
    CommittedSession::create(
        NewTransfer {
            direction: Direction::Send,
            offered_name: OfferedName::from_untrusted("payload.bin"),
            total: ByteCount::new(1024),
            source: SourceDecision::Ready,
        },
        &mut FixedIdentity { next: seed },
        ScriptedStore::deferred(root, script.clone()),
        NonZeroUsize::new(3).unwrap(),
    )
    .unwrap()
}

fn durable_state(root: &Path, card: RecordId, script: &Script) -> TransferRecord {
    let store = ScriptedStore::opened(root, card, script.clone()).expect("the card store opens");
    let encoded = store.latest().expect("a committed product record");
    match decode_record(&encoded).unwrap() {
        RecordDecode::Loaded(record) => record,
        RecordDecode::UnsupportedFuture { .. } => panic!("the record decodes"),
    }
}

fn revision_count(root: &Path, card: RecordId, script: &Script) -> usize {
    let store = ScriptedStore::opened(root, card, script.clone()).expect("the card store opens");
    store
        .operation
        .as_ref()
        .expect("an opened store")
        .record_revision_count()
}

const CAPACITY: NonZeroUsize = NonZeroUsize::new(8).unwrap();

/// Waits for a card to hibernate (its retirement dance fully committed).
async fn settle<P: SessionProvider, E: AttemptExecutor>(runtime: &Runtime<P, E>, card: RecordId) {
    tokio::time::timeout(Duration::from_secs(5), async {
        while runtime.is_live(card) {
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    })
    .await
    .expect("card should settle");
}

// ---- the BN2 invariant tests ----

/// Both halves of exactly-once across a hot restart: a committed command's
/// identity answers `Duplicate` from durable truth after death + restore, and
/// a command whose commit was lost re-issues cleanly and applies once.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mutating_hot_restart_exactly_once() {
    let directory = TempDir::new().unwrap();
    let root = directory.path();
    let script = Script::healthy();

    // --- half A: committed before death → duplicate after restore ---
    let runtime = Runtime::start(
        config(),
        ScriptedProvider {
            root: root.to_path_buf(),
            script: script.clone(),
        },
        InertExecutor,
    );
    let (session, outcome) = create_card(root, &script, 0x10);
    let card = session.record().identity.card;
    runtime.admit(session, outcome).unwrap();
    let commander = runtime.subscribe(card, CAPACITY).unwrap();

    let id = CommandId::from_bytes([0xAA; 16]);
    let verdict = runtime
        .submit_command(&commander, id, ProductCommand::Cancel)
        .await
        .unwrap();
    let CommandVerdict::Accepted(ticket) = verdict else {
        panic!("a fresh command id is accepted");
    };
    assert!(matches!(
        ticket.completed().await,
        CommandCompletion::Committed { .. }
    ));
    // Let the retirement dance finish so the pre-death durable truth is stable.
    settle(&runtime, card).await;
    let revisions_after_commit = revision_count(root, card, &script);
    runtime.shutdown().await;

    // "Process death": a fresh runtime over the same durable root.
    let runtime = Runtime::start(
        config(),
        ScriptedProvider {
            root: root.to_path_buf(),
            script: script.clone(),
        },
        InertExecutor,
    );
    runtime.restore(card).unwrap();
    let commander = runtime.subscribe(card, CAPACITY).unwrap();

    // The SAME identity re-issued: identified as a duplicate from committed
    // truth (the card is hibernated — no actor is needed), nothing re-applies.
    let verdict = runtime
        .submit_command(&commander, id, ProductCommand::Cancel)
        .await
        .unwrap();
    assert!(matches!(verdict, CommandVerdict::Duplicate { .. }));
    assert_eq!(revision_count(root, card, &script), revisions_after_commit);
    let durable = durable_state(root, card, &script);
    assert_eq!(durable.state, ProductState::Cancelled);
    assert_eq!(
        durable
            .command_ledger
            .disposition(id, ProductCommand::Cancel),
        Some(LedgerHit::Duplicate {
            state: ProductState::Cancelled
        })
    );

    // --- half B: commit lost before death → clean re-issue applies once ---
    let (session, outcome) = create_card(root, &script, 0x60);
    let second = session.record().identity.card;
    runtime.admit(session, outcome).unwrap();
    let commander = runtime.subscribe(second, CAPACITY).unwrap();

    script.set(CommitScript::FailAll);
    let id_lost = CommandId::from_bytes([0xBB; 16]);
    let verdict = runtime
        .submit_command(&commander, id_lost, ProductCommand::Cancel)
        .await
        .unwrap();
    let CommandVerdict::Accepted(ticket) = verdict else {
        panic!("acceptance precedes the commit outcome");
    };
    // Honest typed failure: accepted, but the effect did NOT become durable.
    assert!(matches!(
        ticket.completed().await,
        CommandCompletion::CommitFailed { .. }
    ));
    assert_eq!(
        durable_state(root, second, &script)
            .command_ledger
            .disposition(id_lost, ProductCommand::Cancel),
        None,
    );
    runtime.shutdown().await;

    // Death; the store heals; the same identity re-issues CLEANLY.
    script.set(CommitScript::Healthy);
    let runtime = Runtime::start(
        config(),
        ScriptedProvider {
            root: root.to_path_buf(),
            script: script.clone(),
        },
        InertExecutor,
    );
    runtime.restore(second).unwrap();
    let commander = runtime.subscribe(second, CAPACITY).unwrap();
    let verdict = runtime
        .submit_command(&commander, id_lost, ProductCommand::Cancel)
        .await
        .unwrap();
    let CommandVerdict::Accepted(ticket) = verdict else {
        panic!("a lost command re-issues as fresh, not duplicate");
    };
    assert!(matches!(
        ticket.completed().await,
        CommandCompletion::Committed { .. }
    ));
    let durable = durable_state(root, second, &script);
    assert_eq!(durable.state, ProductState::Cancelled);
    assert_eq!(
        durable
            .command_ledger
            .disposition(id_lost, ProductCommand::Cancel),
        Some(LedgerHit::Duplicate {
            state: ProductState::Cancelled
        })
    );
    runtime.shutdown().await;
}

/// A superseded attachment's commands are rejected typed — at the intake gate
/// when the reattach already happened, and at the actor's linearization point
/// when the reattach lands between the gate and application.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn stale_epoch_commands_are_inert() {
    let directory = TempDir::new().unwrap();
    let root = directory.path();
    let script = Script::healthy();
    let runtime = Runtime::start(
        config(),
        ScriptedProvider {
            root: root.to_path_buf(),
            script: script.clone(),
        },
        InertExecutor,
    );
    let (session, outcome) = create_card(root, &script, 0x10);
    let card = session.record().identity.card;
    runtime.admit(session, outcome).unwrap();

    // Gate rejection: the first attachment is superseded by the second.
    let stale = runtime.subscribe(card, CAPACITY).unwrap();
    let commander = runtime.subscribe(card, CAPACITY).unwrap();
    assert_eq!(
        runtime
            .submit_command(
                &stale,
                CommandId::from_bytes([0x01; 16]),
                ProductCommand::Pause,
            )
            .await
            .unwrap_err(),
        CommandRejected::StaleEpoch
    );

    // Linearization rejection: park the actor inside a commit, queue a second
    // command that PASSED the gate, then reattach before the actor reaches it.
    script.set(CommitScript::Block);
    let verdict = runtime
        .submit_command(
            &commander,
            CommandId::from_bytes([0x02; 16]),
            ProductCommand::Pause,
        )
        .await
        .unwrap();
    let CommandVerdict::Accepted(parked_ticket) = verdict else {
        panic!("the parked command was accepted before its commit");
    };
    script.wait_parked();
    let queued = {
        let runtime = runtime.clone();
        let commander_epoch_holder = commander;
        tokio::spawn(async move {
            runtime
                .submit_command(
                    &commander_epoch_holder,
                    CommandId::from_bytes([0x03; 16]),
                    ProductCommand::Cancel,
                )
                .await
        })
    };
    // Give the queued submission time to clear the gate and enqueue while the
    // actor is provably parked inside the blocked commit.
    tokio::time::sleep(Duration::from_millis(300)).await;
    let _newest = runtime.subscribe(card, CAPACITY).unwrap();
    script.set(CommitScript::Healthy);

    assert!(matches!(
        parked_ticket.completed().await,
        CommandCompletion::Committed { .. }
    ));
    assert_eq!(
        queued.await.unwrap().unwrap_err(),
        CommandRejected::Superseded
    );
    // The superseded cancel never applied.
    assert_ne!(
        durable_state(root, card, &script).state,
        ProductState::Cancelled
    );
    runtime.shutdown().await;
}

/// Acceptance answers before the commit barrier resolves, and completion —
/// success or typed failure — arrives only on its own channel.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn acceptance_is_not_completion() {
    let directory = TempDir::new().unwrap();
    let root = directory.path();
    let script = Script::healthy();
    let runtime = Runtime::start(
        config(),
        ScriptedProvider {
            root: root.to_path_buf(),
            script: script.clone(),
        },
        InertExecutor,
    );
    let (session, outcome) = create_card(root, &script, 0x10);
    let card = session.record().identity.card;
    runtime.admit(session, outcome).unwrap();
    let commander = runtime.subscribe(card, CAPACITY).unwrap();

    // Acceptance resolves while the commit is parked; completion stays open.
    script.set(CommitScript::Block);
    let verdict = runtime
        .submit_command(
            &commander,
            CommandId::from_bytes([0x0A; 16]),
            ProductCommand::Pause,
        )
        .await
        .unwrap();
    let CommandVerdict::Accepted(ticket) = verdict else {
        panic!("accepted before any commit outcome exists");
    };
    script.wait_parked();
    let completion = tokio::spawn(ticket.completed());
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(
        !completion.is_finished(),
        "completion must await the commit"
    );
    script.set(CommitScript::Healthy);
    assert!(matches!(
        completion.await.unwrap(),
        CommandCompletion::Committed { .. }
    ));

    // A failing barrier reports acceptance, then a typed CommitFailed — never
    // a success and never a hang.
    script.set(CommitScript::FailAll);
    let verdict = runtime
        .submit_command(
            &commander,
            CommandId::from_bytes([0x0B; 16]),
            ProductCommand::Cancel,
        )
        .await
        .unwrap();
    let CommandVerdict::Accepted(ticket) = verdict else {
        panic!("acceptance is independent of the commit outcome");
    };
    assert!(matches!(
        ticket.completed().await,
        CommandCompletion::CommitFailed { .. }
    ));
    script.set(CommitScript::Healthy);
    runtime.shutdown().await;
}

/// A reused identity with a DIFFERENT command is a typed conflict at both
/// checkpoints — the projection gate (after its effect committed) and the
/// actor's linearization point (while the first commit is still parked) —
/// never a plausible-looking `Duplicate`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn reused_identity_with_different_command_conflicts() {
    let directory = TempDir::new().unwrap();
    let root = directory.path();
    let script = Script::healthy();
    let runtime = Runtime::start(
        config(),
        ScriptedProvider {
            root: root.to_path_buf(),
            script: script.clone(),
        },
        InertExecutor,
    );
    let (session, outcome) = create_card(root, &script, 0x70);
    let card = session.record().identity.card;
    runtime.admit(session, outcome).unwrap();
    let commander = runtime.subscribe(card, CAPACITY).unwrap();

    // Actor path: the first command's commit is parked, so the projection has
    // no ledger entry yet and the gate passes the second submit through; the
    // actor's own ledger check must catch it after the first commits.
    script.set(CommitScript::Block);
    let reused = CommandId::from_bytes([0xCC; 16]);
    let verdict = runtime
        .submit_command(&commander, reused, ProductCommand::Pause)
        .await
        .unwrap();
    let CommandVerdict::Accepted(ticket) = verdict else {
        panic!("the first use of the identity is fresh");
    };
    script.wait_parked();
    // The racing reuse passes the gate (no projection entry until the parked
    // commit lands), queues behind it, and must be caught by the actor.
    let racing = runtime.submit_command(&commander, reused, ProductCommand::Cancel);
    let release = async {
        tokio::time::sleep(Duration::from_millis(100)).await;
        script.set(CommitScript::Healthy);
    };
    let (racing, ()) = tokio::join!(racing, release);
    // The verdict names the command that owns the identity, so a frontend can
    // say WHICH command it collided with rather than only that it collided.
    assert!(matches!(
        racing,
        Ok(CommandVerdict::Conflict {
            applied: ProductCommand::Pause
        })
    ));
    assert!(matches!(
        ticket.completed().await,
        CommandCompletion::Committed { .. }
    ));

    // Gate path: the effect is committed and projected, so the reuse is
    // rejected straight from the projection without touching the actor.
    let gate = runtime
        .submit_command(&commander, reused, ProductCommand::Cancel)
        .await;
    assert!(matches!(
        gate,
        Ok(CommandVerdict::Conflict {
            applied: ProductCommand::Pause
        })
    ));
    runtime.shutdown().await;
}

/// The `Interrupted` disambiguation recipe, exercised end to end: an actor
/// killed BETWEEN acceptance and completion resolves the ticket as
/// `Interrupted`, nothing became durable, and re-issuing the SAME identity
/// after the lazy restore proves it — a FRESH acceptance (not a duplicate)
/// shows the command had not committed, and it then applies exactly once.
/// (The committed arm of the recipe — death after commit answers `Duplicate`
/// — is half A of `mutating_hot_restart_exactly_once`.)
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn interrupted_command_disambiguates_after_restore() {
    let directory = TempDir::new().unwrap();
    let root = directory.path();
    let script = Script::healthy();
    let runtime = Runtime::start(
        config(),
        ScriptedProvider {
            root: root.to_path_buf(),
            script: script.clone(),
        },
        InertExecutor,
    );
    let (session, outcome) = create_card(root, &script, 0x80);
    let card = session.record().identity.card;
    runtime.admit(session, outcome).unwrap();
    let commander = runtime.subscribe(card, CAPACITY).unwrap();
    let revisions_before = revision_count(root, card, &script);

    // Death between acceptance and completion: the commit panics the actor.
    script.set(CommitScript::Panic);
    let id = CommandId::from_bytes([0xDD; 16]);
    let verdict = runtime
        .submit_command(&commander, id, ProductCommand::Cancel)
        .await
        .unwrap();
    let CommandVerdict::Accepted(ticket) = verdict else {
        panic!("acceptance precedes the commit barrier");
    };
    assert_eq!(ticket.completed().await, CommandCompletion::Interrupted);
    assert_eq!(revision_count(root, card, &script), revisions_before);

    // Re-issue the same identity: the FRESH acceptance is the proof that the
    // interrupted command never committed; it then applies exactly once
    // (through the lazy pre-queued restore — the actor is dead).
    let verdict = runtime
        .submit_command(&commander, id, ProductCommand::Cancel)
        .await
        .unwrap();
    let CommandVerdict::Accepted(ticket) = verdict else {
        panic!("an uncommitted identity re-issues as fresh, not duplicate");
    };
    assert!(matches!(
        ticket.completed().await,
        CommandCompletion::Committed { .. }
    ));
    let durable = durable_state(root, card, &script);
    assert_eq!(durable.state, ProductState::Cancelled);
    assert_eq!(
        durable
            .command_ledger
            .disposition(id, ProductCommand::Cancel),
        Some(LedgerHit::Duplicate {
            state: ProductState::Cancelled
        })
    );
    runtime.shutdown().await;
}
