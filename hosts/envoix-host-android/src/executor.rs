use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};

use envoix_attempt_api::{AttemptEventKind, AttemptPlan, AttemptStamp, RetirementIntent};
use envoix_attempt_iroh::{AttemptError, AttemptHandle};
use envoix_outcomes::OutcomeCode;
use envoix_runtime::{
    AttemptExecution, AttemptExecutor, AttemptLaunch, ExecutorSignal, PreparedAttemptIo,
    StopSignal, StopToken, stop_channel,
};
use tokio::sync::{mpsc, oneshot};

/// The transport half: everything an attempt needs that the AUTHORITY does not
/// supply — the link, the token, the timeouts. The I/O half arrives separately,
/// resolved by the card from its own committed record, and the two meet here.
type PreparedLaunch = Box<
    dyn FnOnce(AttemptPlan, PreparedAttemptIo) -> Result<AttemptHandle, AttemptError>
        + Send
        + 'static,
>;

/// One attempt's two halves, whichever arrived first.
///
/// A rendezvous rather than an ordering requirement. The launch is composed by
/// whoever resolved the transport and the source; the attempt is started by the
/// reducer's own effect. Neither can wait for the other without deadlocking the
/// side it is waiting on, and requiring `prepare` to win the race left a started
/// attempt parked forever with a launch sitting unused beside it.
enum Rendezvous {
    /// The launch arrived first and is waiting for its attempt.
    Prepared(PreparedLaunch),
    /// The attempt started first and is waiting for its launch.
    Awaiting(oneshot::Sender<PreparedLaunch>),
}

/// The Android composition's real attempt executor.
///
/// The concrete per-attempt engine is `envoix-attempt-iroh` (which now emits
/// `Phase(Confirming)` between Complete and its ack). A frontend flow (F1/F3
/// invite / rendezvous / pairing) resolves a transfer's source, staging sink,
/// token, and session link, then registers its real launch via [`prepare`].
/// L4 receives this concrete executor at composition time and stays port-only.
///
/// No such flow exists yet, so no launch is ever prepared: `start` for an
/// unprepared attempt PARKS — it holds the attempt open until the reducer
/// retires it, then resolves `Stopped`. Restored cards therefore keep their
/// honest durable truth (`on_restore` maps a lost attempt to `Paused(Lost)`)
/// and nothing fabricates transfer progress.
///
/// [`prepare`]: PreparedIrohExecutor::prepare
#[derive(Clone, Default)]
pub struct PreparedIrohExecutor {
    /// Keyed by the ATTEMPT, never the transfer.
    ///
    /// A transfer outlives its attempts — a pause and resume mints a new
    /// generation against the same `TransferId` — so a launch keyed by transfer
    /// could be composed for one attempt and consumed by a later one, carrying a
    /// link and a source that the resumed attempt never asked for. A prepared
    /// launch is one-shot authority for exactly one attempt.
    ///
    /// This is the opposite conclusion to the SOURCE registry, deliberately.
    /// There, cleanup by attempt generation was the defect: a resume keeps its
    /// ready source, so the source's lifetime spans generations. A launch's does
    /// not. Same-looking key, different lifetime — see
    /// [`BoundSourceRegistry::discard_superseded`].
    ///
    /// [`BoundSourceRegistry::discard_superseded`]: crate::BoundSourceRegistry::discard_superseded
    pending: Arc<Mutex<HashMap<AttemptStamp, Rendezvous>>>,
}

impl PreparedIrohExecutor {
    /// Registers the real launch for one ATTEMPT, once whoever resolves the
    /// transport and the source has done so. Returns whether it was taken.
    ///
    /// If that attempt is already waiting, the launch reaches it immediately.
    /// Otherwise it waits for the attempt to start.
    ///
    /// False means this attempt was already prepared. Replacing would be worse
    /// than refusing: the resident launch may already have been handed to a
    /// running attempt, and a second one composed against different handles has
    /// no attempt left to belong to.
    pub fn prepare(
        &self,
        stamp: AttemptStamp,
        launch: impl FnOnce(AttemptPlan, PreparedAttemptIo) -> Result<AttemptHandle, AttemptError>
        + Send
        + 'static,
    ) -> bool {
        let launch: PreparedLaunch = Box::new(launch);
        let mut pending = self.lock();
        match pending.remove(&stamp) {
            Some(Rendezvous::Awaiting(waiting)) => {
                // The attempt is already running and blocked on this. A closed
                // receiver means it stopped while waiting, which is not an error
                // — the launch simply has nothing to drive and is dropped.
                let _ = waiting.send(launch);
                true
            }
            Some(resident @ Rendezvous::Prepared(_)) => {
                pending.insert(stamp, resident);
                false
            }
            None => {
                pending.insert(stamp, Rendezvous::Prepared(launch));
                true
            }
        }
    }

    /// Drops whatever this attempt had pending. Idempotent.
    fn discard(&self, stamp: AttemptStamp) {
        self.lock().remove(&stamp);
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<AttemptStamp, Rendezvous>> {
        self.pending.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl AttemptExecutor for PreparedIrohExecutor {
    fn start(&self, launch: AttemptLaunch) -> AttemptExecution {
        let (plan, io) = launch.into_parts();
        let (signals_tx, signals) = mpsc::channel(32);
        let (stop, token) = stop_channel();
        let rendezvous = {
            let mut pending = self.lock();
            // Every earlier attempt on this card is superseded by this one, and
            // a launch composed for one of them will never be started. Pruned
            // here because starting is the moment that proves it: the reducer
            // does not mint a new stamp for a card whose previous attempt could
            // still run.
            pending.retain(|resident, _| {
                resident.card != plan.stamp.card || resident.generation >= plan.stamp.generation
            });
            match pending.remove(&plan.stamp) {
                Some(Rendezvous::Prepared(launch)) => Waiting::Ready(launch),
                // A second start for one stamp replaces the waiter; the first
                // one's receiver closes and it parks until it is stopped.
                Some(Rendezvous::Awaiting(_)) | None => {
                    let (sender, receiver) = oneshot::channel();
                    pending.insert(plan.stamp, Rendezvous::Awaiting(sender));
                    Waiting::Pending(receiver)
                }
            }
        };
        tokio::spawn(run_attempt(
            plan,
            io,
            rendezvous,
            self.clone(),
            signals_tx,
            token,
        ));
        AttemptExecution { signals, stop }
    }
}

/// What a starting attempt has in hand.
enum Waiting {
    Ready(PreparedLaunch),
    Pending(oneshot::Receiver<PreparedLaunch>),
}

async fn run_attempt(
    plan: AttemptPlan,
    io: PreparedAttemptIo,
    waiting: Waiting,
    executor: PreparedIrohExecutor,
    signals: mpsc::Sender<ExecutorSignal>,
    stop: StopToken,
) {
    let stamp = plan.stamp;
    let mut stopped = Box::pin(stop.stopped());
    let launch = match waiting {
        Waiting::Ready(launch) => launch,
        Waiting::Pending(arriving) => {
            tokio::select! {
                // BIASED, stop first. When a retirement and a launch are both
                // ready the stop must win: the reducer has ended this attempt,
                // and running the launch anyway would open a transport for an
                // attempt that no longer exists — then immediately tear it down.
                // Left to `select!`'s randomness this was a coin flip.
                biased;
                // Retired before anything was prepared — today's whole story,
                // because no flow composes a launch yet. Holding open until the
                // reducer retires it is what lets a restored card keep its
                // Paused(Lost) truth instead of being told a terminal that never
                // happened.
                _ = &mut stopped => {
                    executor.discard(stamp);
                    let _ = signals.send(ExecutorSignal::Stopped).await;
                    return;
                }
                arrived = arriving => match arrived {
                    Ok(launch) => launch,
                    // Superseded while waiting: a later start took this stamp's
                    // slot. Nothing will arrive, so park honestly rather than
                    // fabricate a terminal.
                    Err(_) => {
                        let _ = stopped.await;
                        let _ = signals.send(ExecutorSignal::Stopped).await;
                        return;
                    }
                },
            }
        }
    };
    let Ok(mut handle) = launch(plan, io) else {
        // A prepared launch that fails to spawn a real link is a genuine error.
        let _ = signals
            .send(ExecutorSignal::Event(AttemptEventKind::Terminal(
                OutcomeCode::Internal,
            )))
            .await;
        let _ = signals.send(ExecutorSignal::Stopped).await;
        return;
    };

    let control = handle.control();
    let mut stop = stopped;
    let mut stop_requested = false;
    loop {
        if !stop_requested {
            tokio::select! {
                signal = &mut stop => {
                    let _ = control.request(teardown_intent(signal));
                    stop_requested = true;
                    continue;
                }
                event = handle.next_event() => {
                    if let Some(event) = event {
                        if signals.send(ExecutorSignal::Event(event.kind)).await.is_err() {
                            let _ = control.request(RetirementIntent::Cancel);
                            return;
                        }
                        continue;
                    }
                }
            }
        } else if let Some(event) = handle.next_event().await {
            if signals
                .send(ExecutorSignal::Event(event.kind))
                .await
                .is_err()
            {
                let _ = control.request(RetirementIntent::Cancel);
                return;
            }
            continue;
        }
        break;
    }
    let _ = handle.wait_ack().await;
    let _ = signals.send(ExecutorSignal::Stopped).await;
}

/// The transport-level intent for one stop. A teardown (process shutdown or a
/// superseded card actor) preserves resumable state, so it retires the attempt
/// as a Pause; only a reducer-authorized Cancel discards.
const fn teardown_intent(signal: StopSignal) -> RetirementIntent {
    match signal {
        StopSignal::Retire(intent) => intent,
        StopSignal::Detached => RetirementIntent::Pause,
    }
}

#[cfg(test)]
mod tests {
    use envoix_attempt_api::ResumeIntent;
    use envoix_runtime::{PreparedSource, StagedIdentity};
    use envoix_types::{ArtifactId, AttemptGen, Direction, RecordId, TransferId};

    use super::*;

    /// One transfer, whichever attempt generation. The transfer id deliberately
    /// does NOT move with the generation — that is the collision the stamp
    /// keying exists to survive.
    fn plan(generation: u32) -> AttemptPlan {
        AttemptPlan {
            stamp: AttemptStamp {
                card: RecordId::new(41),
                generation: AttemptGen::new(generation),
            },
            direction: Direction::Send,
            transfer: TransferId::from_bytes([0x11; 16]),
            artifact: ArtifactId::from_bytes([0x22; 16]),
            resume: ResumeIntent::Fresh,
        }
    }

    /// A launch that refuses to spawn. Its refusal is OBSERVABLE — the executor
    /// answers a terminal — which is what lets a test see that an attempt got
    /// the launch at all, without standing up a transport.
    fn refusing_launch()
    -> impl FnOnce(AttemptPlan, PreparedAttemptIo) -> Result<AttemptHandle, AttemptError> {
        |_, _| Err(AttemptError::WrongDirection)
    }

    /// A send launch over a source that reads nothing. These cases are about the
    /// RENDEZVOUS — which half arrives first, and what happens when a retirement
    /// races it — so the bytes never matter; what matters is that a send arrives
    /// with a source at all, which the launch type now requires.
    fn sending(plan: AttemptPlan) -> AttemptLaunch {
        struct EmptySource;
        impl envoix_capabilities::SourceSession for EmptySource {
            fn read_at(
                &mut self,
                _offset: envoix_types::ByteCount,
                _destination: &mut [u8],
            ) -> Result<usize, envoix_capabilities::SourceReadError> {
                Ok(0)
            }
        }
        AttemptLaunch::sending(
            plan,
            PreparedSource::new(
                Box::new(EmptySource),
                StagedIdentity {
                    total: envoix_types::ByteCount::new(0),
                    digest: envoix_runtime::ContentHash::from_bytes([0; 32]),
                },
            ),
        )
        .expect("the plan sends")
    }

    /// Asserts this attempt received its launch, by the terminal the refusing
    /// launch answers. NOT stopped first: a stop deliberately wins the race
    /// against an arriving launch, so stopping before observing would be racing
    /// the very bias this exists to check.
    async fn assert_launched(mut execution: AttemptExecution) {
        assert!(
            matches!(
                execution.signals.recv().await,
                Some(ExecutorSignal::Event(AttemptEventKind::Terminal(
                    OutcomeCode::Internal
                )))
            ),
            "the attempt did not run the launch meant for it"
        );
        while execution.signals.recv().await.is_some() {}
    }

    /// Asserts this attempt never received a launch: stopping is the only thing
    /// that can finish it, and it answers `Stopped` and nothing else.
    async fn assert_parked(mut execution: AttemptExecution) {
        execution.stop.stop(RetirementIntent::Cancel);
        while let Some(signal) = execution.signals.recv().await {
            assert!(
                matches!(signal, ExecutorSignal::Stopped),
                "a parked attempt emitted {signal:?}"
            );
        }
    }

    /// Either order works. Requiring `prepare` to win left a started attempt
    /// parked forever beside a launch that could not reach it — and the two
    /// halves are produced by different actors with no ordering between them.
    #[tokio::test]
    async fn a_launch_and_its_attempt_meet_in_either_order() {
        let executor = PreparedIrohExecutor::default();
        assert!(executor.prepare(plan(1).stamp, refusing_launch()));
        assert_launched(executor.start(sending(plan(1)))).await;

        let executor = PreparedIrohExecutor::default();
        let execution = executor.start(sending(plan(2)));
        assert!(executor.prepare(plan(2).stamp, refusing_launch()));
        assert_launched(execution).await;
    }

    /// A retirement that lands together with a launch wins.
    ///
    /// Both halves are ready before the attempt's task first polls: the reducer
    /// has ended this attempt and a launch has arrived for it. Running the launch
    /// would open a transport for an attempt that no longer exists and then
    /// immediately tear it down. Left to `select!`'s randomness this was a coin
    /// flip, and the flip was visible as a flaky test rather than as a decision.
    #[tokio::test]
    async fn a_retirement_beats_a_launch_that_arrives_with_it() {
        let executor = PreparedIrohExecutor::default();
        let mut execution = executor.start(sending(plan(1)));
        // No await between the spawn and these, so the attempt's task has not
        // polled yet and both branches are ready when it does.
        execution.stop.stop(RetirementIntent::Cancel);
        assert!(executor.prepare(plan(1).stamp, refusing_launch()));

        assert_parked(execution).await;
    }

    /// A launch belongs to ONE attempt, not to the transfer.
    ///
    /// A pause and resume mints a new generation against the same `TransferId`,
    /// so keying by transfer let a launch composed for the first attempt — with
    /// its link and its source — be consumed by the second, which had asked for
    /// neither.
    #[tokio::test]
    async fn a_later_attempt_cannot_consume_an_earlier_ones_launch() {
        let executor = PreparedIrohExecutor::default();
        assert!(executor.prepare(plan(1).stamp, refusing_launch()));

        assert_parked(executor.start(sending(plan(2)))).await;
    }

    /// And the superseded launch does not sit in the map forever.
    #[tokio::test]
    async fn starting_an_attempt_prunes_the_ones_it_supersedes() {
        let executor = PreparedIrohExecutor::default();
        assert!(executor.prepare(plan(1).stamp, refusing_launch()));

        let execution = executor.start(sending(plan(2)));

        assert!(
            !executor.lock().contains_key(&plan(1).stamp),
            "a superseded launch was retained"
        );
        drop(execution);
    }

    /// A second preparation for one attempt is refused, not swapped in. The
    /// resident launch may already be driving a running attempt.
    #[tokio::test]
    async fn one_attempt_is_prepared_once() {
        let executor = PreparedIrohExecutor::default();
        assert!(executor.prepare(plan(1).stamp, refusing_launch()));
        assert!(
            !executor.prepare(plan(1).stamp, refusing_launch()),
            "a second launch replaced the one already prepared"
        );
    }

    /// An attempt retired before anything prepared it parks and then stops, and
    /// leaves nothing behind. That is today's whole story: no flow composes a
    /// launch yet, and a card restored from one of these keeps its `Paused(Lost)`
    /// truth rather than being told a terminal that never happened.
    #[tokio::test]
    async fn an_attempt_nobody_prepared_parks_and_leaves_nothing_behind() {
        let executor = PreparedIrohExecutor::default();

        assert_parked(executor.start(sending(plan(1)))).await;

        assert!(
            executor.lock().is_empty(),
            "a parked attempt left its waiting slot behind"
        );
    }
}
