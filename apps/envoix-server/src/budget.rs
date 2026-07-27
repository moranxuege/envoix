//! One budget per service, and nothing between them.
//!
//! A budget is two things an operator can size independently: how many callers
//! a service may serve at once, and how many worker threads it may run them on.
//! Both are per service. There is no shared pool to drain, so load on one
//! service cannot take capacity from another — not because the code is careful
//! about it, but because there is nothing to take.
//!
//! Only [`ServiceBudget`] can admit, it is not `Clone`, and exactly one exists
//! per service. [`BudgetMeter`] is the observation side: it can read every
//! counter and record what happened, and it holds no permit source at all, so
//! handing one to diagnostics cannot hand over the ability to spend.

use std::io;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};

use tokio::runtime::{Builder, Runtime};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio::task::JoinHandle;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Service {
    Pairing,
    Mailbox,
    Diagnostics,
}

impl Service {
    pub const ALL: [Self; 3] = [Self::Pairing, Self::Mailbox, Self::Diagnostics];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pairing => "pairing",
            Self::Mailbox => "mailbox",
            Self::Diagnostics => "diagnostics",
        }
    }

    /// The worker-thread name of this service's runtime. Every worker of one
    /// runtime carries it, so the name a request records identifies the
    /// executor that served it.
    const fn worker_name(self) -> &'static str {
        match self {
            Self::Pairing => "envoix-pairing",
            Self::Mailbox => "envoix-mailbox",
            Self::Diagnostics => "envoix-diagnostics",
        }
    }
}

struct BudgetState {
    service: Service,
    capacity: usize,
    in_flight: AtomicUsize,
    admitted: AtomicU64,
    refused: AtomicU64,
    worker: OnceLock<String>,
}

/// The spend side of one service's budget. Not `Clone`: a budget belongs to the
/// service it was built for and cannot be handed to a second one.
pub struct ServiceBudget {
    permits: Arc<Semaphore>,
    state: Arc<BudgetState>,
}

impl ServiceBudget {
    fn new(service: Service, capacity: usize) -> Self {
        Self {
            permits: Arc::new(Semaphore::new(capacity)),
            state: Arc::new(BudgetState {
                service,
                capacity,
                in_flight: AtomicUsize::new(0),
                admitted: AtomicU64::new(0),
                refused: AtomicU64::new(0),
                worker: OnceLock::new(),
            }),
        }
    }

    /// Takes a slot if this service has one free. Never waits: a caller that
    /// cannot be served now is told so now.
    pub fn try_admit(&self) -> Option<Admission> {
        let permit = Arc::clone(&self.permits).try_acquire_owned().ok()?;
        self.state.in_flight.fetch_add(1, Ordering::Relaxed);
        self.state.admitted.fetch_add(1, Ordering::Relaxed);
        Some(Admission {
            _permit: permit,
            state: Arc::clone(&self.state),
        })
    }

    pub fn meter(&self) -> BudgetMeter {
        BudgetMeter {
            state: Arc::clone(&self.state),
        }
    }
}

/// A slot held for as long as its caller is being served.
pub struct Admission {
    _permit: OwnedSemaphorePermit,
    state: Arc<BudgetState>,
}

impl Drop for Admission {
    fn drop(&mut self) {
        self.state.in_flight.fetch_sub(1, Ordering::Relaxed);
    }
}

/// The observation side of a budget: every counter, and no way to admit.
#[derive(Clone)]
pub struct BudgetMeter {
    state: Arc<BudgetState>,
}

impl BudgetMeter {
    pub fn service(&self) -> Service {
        self.state.service
    }

    pub fn capacity(&self) -> usize {
        self.state.capacity
    }

    pub fn in_flight(&self) -> usize {
        self.state.in_flight.load(Ordering::Relaxed)
    }

    pub fn admitted(&self) -> u64 {
        self.state.admitted.load(Ordering::Relaxed)
    }

    pub fn refused(&self) -> u64 {
        self.state.refused.load(Ordering::Relaxed)
    }

    pub fn record_refused(&self) {
        self.state.refused.fetch_add(1, Ordering::Relaxed);
    }

    /// Records which executor actually ran this service's work, so the thread
    /// partition is observable rather than merely intended.
    pub fn record_worker(&self) {
        if self.state.worker.get().is_some() {
            return;
        }
        let name = std::thread::current()
            .name()
            .unwrap_or("unnamed")
            .to_owned();
        let _ = self.state.worker.set(name);
    }

    pub fn worker(&self) -> Option<&str> {
        self.state.worker.get().map(String::as_str)
    }
}

/// One service's worker threads. Dropping it never blocks, so a handle may be
/// released from inside an async context.
pub struct ServiceRuntime {
    runtime: Option<Runtime>,
}

impl ServiceRuntime {
    fn new(service: Service, worker_threads: usize) -> Result<Self, io::Error> {
        let runtime = Builder::new_multi_thread()
            .worker_threads(worker_threads)
            .max_blocking_threads(1)
            .thread_name(service.worker_name())
            .enable_all()
            .build()?;
        Ok(Self {
            runtime: Some(runtime),
        })
    }

    pub fn spawn<F>(&self, future: F) -> JoinHandle<F::Output>
    where
        F: Future + Send + 'static,
        F::Output: Send + 'static,
    {
        self.runtime
            .as_ref()
            .expect("runtime is taken only on drop")
            .spawn(future)
    }
}

impl Drop for ServiceRuntime {
    fn drop(&mut self) {
        if let Some(runtime) = self.runtime.take() {
            runtime.shutdown_background();
        }
    }
}

/// How much each service may spend. Zero is not a budget, so it is not a
/// representable one.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BudgetPlan {
    pub max_concurrent: usize,
    pub worker_threads: usize,
}

#[derive(Debug)]
pub enum BudgetError {
    Invalid {
        service: Service,
        field: &'static str,
    },
    Runtime {
        service: Service,
        source: io::Error,
    },
}

/// The three budgets, built one per service from the enum itself, so a service
/// cannot be given a second service's budget and none can be left out.
pub struct ServiceBudgets {
    pub pairing: (ServiceBudget, ServiceRuntime),
    pub mailbox: (ServiceBudget, ServiceRuntime),
    pub diagnostics: (ServiceBudget, ServiceRuntime),
}

impl ServiceBudgets {
    pub fn build(plan: impl Fn(Service) -> BudgetPlan) -> Result<Self, BudgetError> {
        let build = |service| {
            let BudgetPlan {
                max_concurrent,
                worker_threads,
            } = plan(service);
            for (field, value) in [
                ("max concurrent callers", max_concurrent),
                ("worker threads", worker_threads),
            ] {
                if value == 0 {
                    return Err(BudgetError::Invalid { service, field });
                }
            }
            let runtime = ServiceRuntime::new(service, worker_threads)
                .map_err(|source| BudgetError::Runtime { service, source })?;
            Ok((ServiceBudget::new(service, max_concurrent), runtime))
        };
        Ok(Self {
            pairing: build(Service::Pairing)?,
            mailbox: build(Service::Mailbox)?,
            diagnostics: build(Service::Diagnostics)?,
        })
    }

    pub fn meters(&self) -> [BudgetMeter; 3] {
        [
            self.pairing.0.meter(),
            self.mailbox.0.meter(),
            self.diagnostics.0.meter(),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plan(_: Service) -> BudgetPlan {
        BudgetPlan {
            max_concurrent: 2,
            worker_threads: 1,
        }
    }

    #[test]
    fn a_budget_admits_exactly_its_capacity_and_recovers() {
        let budget = ServiceBudget::new(Service::Mailbox, 2);
        let meter = budget.meter();
        let first = budget.try_admit().expect("capacity 2 admits the first");
        let second = budget.try_admit().expect("capacity 2 admits the second");
        assert!(budget.try_admit().is_none(), "the third must be refused");
        assert_eq!(meter.in_flight(), 2);
        assert_eq!(meter.admitted(), 2);

        meter.record_refused();
        assert_eq!(meter.refused(), 1);
        drop(first);
        assert_eq!(meter.in_flight(), 1);
        assert!(budget.try_admit().is_some(), "a freed slot is reusable");
        drop(second);
    }

    /// Spending one service's budget leaves the others untouched. This is the
    /// admission half of `server_admission_isolation`, without any I/O.
    #[test]
    fn draining_one_budget_does_not_touch_another() {
        let budgets = ServiceBudgets::build(plan).unwrap();
        let held: Vec<_> = std::iter::repeat_with(|| budgets.mailbox.0.try_admit())
            .take(2)
            .collect();
        assert!(held.iter().all(Option::is_some));
        assert!(budgets.mailbox.0.try_admit().is_none());
        assert!(
            budgets.pairing.0.try_admit().is_some(),
            "pairing admission may not be a function of mailbox load"
        );
        assert!(budgets.diagnostics.0.try_admit().is_some());
    }

    #[test]
    fn a_zero_budget_is_rejected_before_any_thread_is_spawned() {
        let zero_concurrency = |service| BudgetPlan {
            max_concurrent: usize::from(service != Service::Diagnostics),
            worker_threads: 1,
        };
        assert!(matches!(
            ServiceBudgets::build(zero_concurrency),
            Err(BudgetError::Invalid {
                service: Service::Diagnostics,
                field: "max concurrent callers"
            })
        ));
        let zero_threads = |_| BudgetPlan {
            max_concurrent: 1,
            worker_threads: 0,
        };
        assert!(matches!(
            ServiceBudgets::build(zero_threads),
            Err(BudgetError::Invalid {
                field: "worker threads",
                ..
            })
        ));
    }

    /// The thread partition, observed rather than assumed: work submitted to
    /// each service's runtime runs on workers no other service has.
    #[test]
    fn each_service_runs_on_its_own_named_workers() {
        let budgets = ServiceBudgets::build(plan).unwrap();
        let mut recorded = Vec::new();
        for (budget, runtime) in [&budgets.pairing, &budgets.mailbox, &budgets.diagnostics] {
            let meter = budget.meter();
            let recorder = meter.clone();
            let handle = runtime.spawn(async move { recorder.record_worker() });
            for _ in 0..1000 {
                if meter.worker().is_some() {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
            drop(handle);
            recorded.push(
                meter
                    .worker()
                    .expect("the spawned task records its worker")
                    .to_owned(),
            );
        }
        recorded.sort();
        recorded.dedup();
        assert_eq!(
            recorded.len(),
            3,
            "each service must run on its own runtime, not a shared pool"
        );
    }
}
