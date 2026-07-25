use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};

use envoix_attempt_api::{AttemptEventKind, AttemptPlan, RetirementIntent};
use envoix_attempt_iroh::{AttemptError, AttemptHandle};
use envoix_outcomes::OutcomeCode;
use envoix_runtime::{
    AttemptExecution, AttemptExecutor, ExecutorSignal, StopSignal, StopToken, stop_channel,
};
use envoix_types::TransferId;
use tokio::sync::mpsc;

type PreparedLaunch =
    Box<dyn FnOnce(AttemptPlan) -> Result<AttemptHandle, AttemptError> + Send + 'static>;

/// The Android composition's real attempt executor.
///
/// The concrete per-attempt engine is `envoix-attempt-iroh` (which now emits
/// `Phase(Confirming)` between Complete and its ack). A frontend flow (F1/F3
/// invite / rendezvous / pairing) resolves a transfer's source, staging sink,
/// token, and session link, then registers its real launch via [`prepare`].
/// L4 receives this concrete executor at composition time and stays port-only.
///
/// No such flow exists yet, so no launch is ever prepared: `start` for an
/// unprepared transfer PARKS — it holds the attempt open until the reducer
/// retires it, then resolves `Stopped`. Restored cards therefore keep their
/// honest durable truth (`on_restore` maps a lost attempt to `Paused(Lost)`)
/// and nothing fabricates transfer progress. F1/F3 exercise the prepared path.
///
/// [`prepare`]: PreparedIrohExecutor::prepare
#[derive(Clone, Default)]
pub struct PreparedIrohExecutor {
    launches: Arc<Mutex<HashMap<TransferId, PreparedLaunch>>>,
}

impl PreparedIrohExecutor {
    /// Registers the real launch for `transfer`, invoked once the frontend
    /// pick/pairing flow (F1/F3) has resolved its handles.
    pub fn prepare(
        &self,
        transfer: TransferId,
        launch: impl FnOnce(AttemptPlan) -> Result<AttemptHandle, AttemptError> + Send + 'static,
    ) {
        self.launches
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(transfer, Box::new(launch));
    }
}

impl AttemptExecutor for PreparedIrohExecutor {
    fn start(&self, plan: AttemptPlan) -> AttemptExecution {
        let launch = self
            .launches
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&plan.transfer);
        let (signals_tx, signals) = mpsc::channel(32);
        let (stop, token) = stop_channel();
        tokio::spawn(run_attempt(plan, launch, signals_tx, token));
        AttemptExecution { signals, stop }
    }
}

async fn run_attempt(
    plan: AttemptPlan,
    launch: Option<PreparedLaunch>,
    signals: mpsc::Sender<ExecutorSignal>,
    stop: StopToken,
) {
    let Some(launch) = launch else {
        // Not-yet-reachable path: no frontend flow prepares a launch yet, so
        // park honestly — hold open until retired, then resolve Stopped — rather
        // than fabricate a terminal. Restored cards keep their Paused(Lost) truth.
        let _ = stop.stopped().await;
        let _ = signals.send(ExecutorSignal::Stopped).await;
        return;
    };
    let Ok(mut handle) = launch(plan) else {
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
    let mut stop = Box::pin(stop.stopped());
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
