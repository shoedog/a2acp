use crate::provider::WorktreeProvider;
use crate::provider_path::{
    resolve_worktree, sidecar_path, write_sidecar, WorktreeConfig, WorktreeSidecar,
};
use bridge_core::domain::{Part, SessionSpec};
use bridge_core::error::BridgeError;
use bridge_core::ids::SessionId;
use bridge_core::orch::{AgentSessionCaps, ReconcileOutcome};
use bridge_core::permission::TurnMeta;
use bridge_core::ports::{
    AgentBackend, BackendObservers, BackendStream, DiagnosticObserver, RichEventSink,
};
use bridge_core::SessionCwd;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;
use tokio::sync::{watch, Mutex, Notify};

const FAILED_CONFIGURE_RETRY_INITIAL: Duration = Duration::from_secs(1);
const FAILED_CONFIGURE_RETRY_MAX: Duration = Duration::from_secs(30);
const MAX_WORKTREE_CONFIGURES_IN_FLIGHT: u64 = 64;

#[derive(Clone)]
pub struct WorktreeIdentity {
    pub run_id: String,
    pub host: String,
    pub lease: String,
}

enum WtState {
    Reserving {
        claim: u64,
        configure: u64,
        entry: WtEntry,
    },
    Ready(WtEntry),
}

#[derive(Clone)]
struct WtEntry {
    canonical_source: String,
    worktree_path: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum CleanupStrength {
    Forget,
    Release,
}

impl CleanupStrength {
    fn transition_codes(self) -> (&'static str, &'static str, &'static str) {
        match self {
            Self::Forget => (
                "worktree.teardown.forget",
                "worktree.teardown.forgotten",
                "worktree.teardown.forget_failed",
            ),
            Self::Release => (
                "worktree.teardown.release",
                "worktree.teardown.released",
                "worktree.teardown.release_failed",
            ),
        }
    }
}

#[derive(Default)]
struct CleanupCellState {
    inner_strength: Option<CleanupStrength>,
    provider_removed: bool,
    sidecar_removed: bool,
    entry: Option<WtEntry>,
}

struct CleanupCell {
    state: Mutex<CleanupCellState>,
    flight: StdMutex<Option<CleanupFlightSlot>>,
    lifecycle: StdMutex<CleanupLifecycle>,
    configure_settled: Notify,
}

struct CleanupFlightSlot {
    id: u64,
    strength: CleanupStrength,
    report: CleanupReportReceiver,
    #[cfg(test)]
    joined_waiters: u64,
}

type CleanupReportReceiver = watch::Receiver<Option<Result<(), BridgeError>>>;
type CleanupFlightHandle = (CleanupStrength, CleanupReportReceiver);

#[derive(Default)]
struct CleanupLifecycle {
    configuring: u64,
    active_configures: HashSet<u64>,
    configured: bool,
    cleanup_started: bool,
    failed_configure_cleanup_pending: bool,
}

struct ConfigureAdmission<'a> {
    owner: &'a WorktreeBackend,
    count: &'a AtomicU64,
    notify: &'a Notify,
    id: u64,
    session: String,
    session_id: SessionId,
    cell: Arc<CleanupCell>,
    cells: Arc<StdMutex<HashMap<String, Arc<CleanupCell>>>>,
    cleanup_on_drop: bool,
}

impl Drop for ConfigureAdmission<'_> {
    fn drop(&mut self) {
        let (remove_configure_only_cell, start_cleanup) = {
            let mut lifecycle = self
                .cell
                .lifecycle
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            let start_cleanup = self.cleanup_on_drop && !lifecycle.configured;
            if start_cleanup {
                lifecycle.failed_configure_cleanup_pending = true;
            }
            lifecycle.configuring = lifecycle
                .configuring
                .checked_sub(1)
                .expect("configure admission count is balanced");
            assert!(
                lifecycle.active_configures.remove(&self.id),
                "configure admission identity is balanced"
            );
            self.cell.configure_settled.notify_waiters();
            (
                !lifecycle.configured
                    && lifecycle.configuring == 0
                    && !lifecycle.cleanup_started
                    && !lifecycle.failed_configure_cleanup_pending,
                start_cleanup,
            )
        };
        if remove_configure_only_cell {
            let mut cells = self.cells.lock().unwrap_or_else(|error| error.into_inner());
            if cells
                .get(&self.session)
                .is_some_and(|current| Arc::ptr_eq(current, &self.cell))
            {
                let lifecycle = self
                    .cell
                    .lifecycle
                    .lock()
                    .unwrap_or_else(|error| error.into_inner());
                if !lifecycle.configured
                    && lifecycle.configuring == 0
                    && !lifecycle.cleanup_started
                    && !lifecycle.failed_configure_cleanup_pending
                {
                    cells.remove(&self.session);
                }
            }
        }
        self.count.fetch_sub(1, Ordering::SeqCst);
        self.notify.notify_waiters();
        if start_cleanup {
            // Reservation publication arms this synchronous fallback before
            // provider/sidecar/inner awaits. Dropping the returned receiver
            // detaches from the observer-free flight; its reporter retains
            // exact failed-configure retry ownership.
            let _ =
                self.owner
                    .start_or_join_cleanup(&self.session_id, CleanupStrength::Release, true);
        }
    }
}

impl ConfigureAdmission<'_> {
    fn id(&self) -> u64 {
        self.id
    }

    fn retain_for_session(&mut self) {
        self.cell
            .lifecycle
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .configured = true;
        self.cleanup_on_drop = false;
    }

    fn retain_failed_configure_cleanup(&mut self) {
        self.cell
            .lifecycle
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .failed_configure_cleanup_pending = true;
        self.cleanup_on_drop = true;
    }

    fn arm_cleanup_on_drop(&mut self) {
        self.cleanup_on_drop = true;
    }
}

impl CleanupCell {
    fn new() -> Self {
        Self {
            state: Mutex::new(CleanupCellState::default()),
            flight: StdMutex::new(None),
            lifecycle: StdMutex::new(CleanupLifecycle::default()),
            configure_settled: Notify::new(),
        }
    }
}

pub struct WorktreeBackend {
    inner: Arc<dyn AgentBackend>,
    provider: Arc<dyn WorktreeProvider>,
    cfg: WorktreeConfig,
    allowed_root: Option<SessionCwd>,
    identity: WorktreeIdentity,
    map: Arc<Mutex<HashMap<String, WtState>>>,
    cleanup_cells: Arc<StdMutex<HashMap<String, Arc<CleanupCell>>>>,
    sealed: Arc<AtomicBool>,
    configure_inflight: AtomicU64,
    configure_settled: Notify,
    next_claim: AtomicU64,
    next_configure_admission: AtomicU64,
    next_cleanup_flight: Arc<AtomicU64>,
    notify: Arc<Notify>,
    #[cfg(test)]
    retirement_joined_cell_count: AtomicU64,
    #[cfg(test)]
    retirement_joined_cell: Notify,
    #[cfg(test)]
    cleanup_waiting_reservation_count: Arc<AtomicU64>,
    #[cfg(test)]
    cleanup_waiting_reservation: Arc<Notify>,
    #[cfg(test)]
    cleanup_flight_started_count: AtomicU64,
    #[cfg(test)]
    cleanup_flight_started: Notify,
    #[cfg(test)]
    configure_admitted: Notify,
    #[cfg(test)]
    failed_configure_retry_now: Arc<Notify>,
}

impl WorktreeBackend {
    pub fn new(
        inner: Arc<dyn AgentBackend>,
        provider: Arc<dyn WorktreeProvider>,
        cfg: WorktreeConfig,
        allowed_root: Option<SessionCwd>,
        identity: WorktreeIdentity,
    ) -> Self {
        Self {
            inner,
            provider,
            cfg,
            allowed_root,
            identity,
            map: Arc::new(Mutex::new(HashMap::new())),
            cleanup_cells: Arc::new(StdMutex::new(HashMap::new())),
            sealed: Arc::new(AtomicBool::new(false)),
            configure_inflight: AtomicU64::new(0),
            configure_settled: Notify::new(),
            next_claim: AtomicU64::new(1),
            next_configure_admission: AtomicU64::new(1),
            next_cleanup_flight: Arc::new(AtomicU64::new(1)),
            notify: Arc::new(Notify::new()),
            #[cfg(test)]
            retirement_joined_cell_count: AtomicU64::new(0),
            #[cfg(test)]
            retirement_joined_cell: Notify::new(),
            #[cfg(test)]
            cleanup_waiting_reservation_count: Arc::new(AtomicU64::new(0)),
            #[cfg(test)]
            cleanup_waiting_reservation: Arc::new(Notify::new()),
            #[cfg(test)]
            cleanup_flight_started_count: AtomicU64::new(0),
            #[cfg(test)]
            cleanup_flight_started: Notify::new(),
            #[cfg(test)]
            configure_admitted: Notify::new(),
            #[cfg(test)]
            failed_configure_retry_now: Arc::new(Notify::new()),
        }
    }

    fn admit_configure(&self, session: &SessionId) -> Result<ConfigureAdmission<'_>, BridgeError> {
        let configure_id = self
            .next_configure_admission
            .fetch_add(1, Ordering::Relaxed);
        let key = session.as_str().to_owned();
        let cell = {
            let mut cells = self
                .cleanup_cells
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            // Retirement publishes `sealed` while holding this same lock.
            // Once admission observes an open backend here, retirement must
            // observe its count/cell; once retirement seals here, no new
            // configure can publish a cell behind its snapshot.
            if self.sealed.load(Ordering::SeqCst) {
                return Err(BridgeError::SessionExpired);
            }
            if self.configure_inflight.load(Ordering::SeqCst) >= MAX_WORKTREE_CONFIGURES_IN_FLIGHT {
                return Err(BridgeError::AgentOverloaded);
            }
            if cells.values().any(|cell| {
                cell.lifecycle
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .failed_configure_cleanup_pending
            }) {
                // A failed configuration has live partial cleanup state and an
                // owned retry flight. Fail closed before allocating another
                // worktree; recovery success evicts that cell and reopens
                // admission.
                return Err(BridgeError::AgentOverloaded);
            }
            let cell = cells
                .entry(key.clone())
                .or_insert_with(|| Arc::new(CleanupCell::new()))
                .clone();
            let mut lifecycle = cell
                .lifecycle
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            if lifecycle.cleanup_started {
                return Err(BridgeError::SessionExpired);
            }
            lifecycle.configuring += 1;
            lifecycle.active_configures.insert(configure_id);
            self.configure_inflight.fetch_add(1, Ordering::SeqCst);
            drop(lifecycle);
            cell
        };
        #[cfg(test)]
        self.configure_admitted.notify_waiters();
        Ok(ConfigureAdmission {
            owner: self,
            count: &self.configure_inflight,
            notify: &self.configure_settled,
            id: configure_id,
            session: key,
            session_id: session.clone(),
            cell,
            cells: self.cleanup_cells.clone(),
            cleanup_on_drop: false,
        })
    }

    #[cfg(test)]
    async fn wait_for_retirement_joined_cell(&self) {
        while self.retirement_joined_cell_count.load(Ordering::SeqCst) == 0 {
            self.retirement_joined_cell.notified().await;
        }
    }

    #[cfg(test)]
    async fn wait_for_cleanup_waiting_reservation(&self) {
        while self
            .cleanup_waiting_reservation_count
            .load(Ordering::SeqCst)
            == 0
        {
            self.cleanup_waiting_reservation.notified().await;
        }
    }

    #[cfg(test)]
    async fn wait_for_cleanup_flight_started(&self) {
        while self.cleanup_flight_started_count.load(Ordering::SeqCst) == 0 {
            let started = self.cleanup_flight_started.notified();
            if self.cleanup_flight_started_count.load(Ordering::SeqCst) == 0 {
                started.await;
            }
        }
    }

    #[cfg(test)]
    async fn wait_for_configure_inflight(&self, expected: u64) {
        while self.configure_inflight.load(Ordering::SeqCst) < expected {
            let admitted = self.configure_admitted.notified();
            if self.configure_inflight.load(Ordering::SeqCst) < expected {
                admitted.await;
            }
        }
    }

    #[cfg(test)]
    fn trigger_failed_configure_retry(&self) {
        self.failed_configure_retry_now.notify_one();
    }

    #[cfg(test)]
    fn cleanup_join_count(&self, session: &SessionId) -> u64 {
        let cell = self
            .cleanup_cells
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .get(session.as_str())
            .cloned();
        cell.and_then(|cell| {
            cell.flight
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .as_ref()
                .map(|flight| flight.joined_waiters)
        })
        .unwrap_or(0)
    }

    #[cfg(test)]
    fn cleanup_flight_strength(&self, session: &SessionId) -> Option<CleanupStrength> {
        let cell = self
            .cleanup_cells
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .get(session.as_str())
            .cloned()?;
        let strength = cell
            .flight
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .as_ref()
            .map(|flight| flight.strength);
        strength
    }

    #[cfg(test)]
    fn cleanup_flight_report(&self, session: &SessionId) -> Option<CleanupReportReceiver> {
        let cell = self
            .cleanup_cells
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .get(session.as_str())
            .cloned()?;
        let report = cell
            .flight
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .as_ref()
            .map(|flight| flight.report.clone());
        report
    }

    #[cfg(test)]
    fn cleanup_cell_count(&self) -> usize {
        self.cleanup_cells
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .len()
    }

    fn claim_cleanup_cell(
        &self,
        session: &SessionId,
        allow_new_when_sealed: bool,
    ) -> Option<Arc<CleanupCell>> {
        let mut cells = self
            .cleanup_cells
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let cell = match cells.get(session.as_str()).cloned() {
            Some(cell) => cell,
            None if self.sealed.load(Ordering::SeqCst) && !allow_new_when_sealed => return None,
            None => {
                let cell = Arc::new(CleanupCell::new());
                cells.insert(session.as_str().to_owned(), cell.clone());
                cell
            }
        };
        cell.lifecycle
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .cleanup_started = true;
        Some(cell)
    }

    async fn entry_for_cleanup(
        map: &Mutex<HashMap<String, WtState>>,
        notify: &Notify,
        session: &SessionId,
    ) -> Option<WtEntry> {
        let mut map = map.lock().await;
        match map.get(session.as_str()) {
            Some(WtState::Ready(entry)) => Some(entry.clone()),
            Some(WtState::Reserving { entry, .. }) => {
                let entry = entry.clone();
                map.remove(session.as_str());
                notify.notify_waiters();
                Some(entry)
            }
            None => None,
        }
    }

    async fn cleanup_session(
        &self,
        session: &SessionId,
        strength: CleanupStrength,
    ) -> Result<(), BridgeError> {
        self.cleanup_session_with_sealed_admission(session, strength, false)
            .await
    }

    async fn cleanup_session_with_sealed_admission(
        &self,
        session: &SessionId,
        strength: CleanupStrength,
        allow_new_when_sealed: bool,
    ) -> Result<(), BridgeError> {
        let requested = strength;
        loop {
            let Some((flight_strength, report)) =
                self.start_or_join_cleanup(session, requested, allow_new_when_sealed)
            else {
                return Ok(());
            };
            let result = wait_for_cleanup_report(report).await;
            match result {
                Err(error) => return Err(error),
                Ok(()) if flight_strength >= requested => return Ok(()),
                Ok(()) => {
                    // A stronger request joined a weaker in-flight cleanup.
                    // The completed weaker report is shared first; loop once
                    // to install/join the monotonic upgrade.
                }
            }
        }
    }

    fn start_or_join_cleanup(
        &self,
        session: &SessionId,
        requested: CleanupStrength,
        allow_new_when_sealed: bool,
    ) -> Option<CleanupFlightHandle> {
        // Acquire the cell synchronously and move every cleanup dependency into
        // a task before the caller's first await. Dropping the report waiter
        // therefore detaches, rather than cancels, the cleanup flight.
        let cell = self.claim_cleanup_cell(session, allow_new_when_sealed)?;
        let mut slot = cell
            .flight
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let mut requested = requested;
        if let Some(existing) = slot.as_mut() {
            let completed = existing.report.borrow().clone();
            if matches!(completed, Some(Err(_))) {
                requested = requested.max(existing.strength);
            }
            let reusable = match completed {
                None => existing.strength >= requested,
                Some(Ok(())) => existing.strength >= requested,
                Some(Err(_)) => false,
            };
            if reusable {
                #[cfg(test)]
                {
                    existing.joined_waiters += 1;
                }
                return Some((existing.strength, existing.report.clone()));
            }
        }

        let inner = self.inner.clone();
        let provider = self.provider.clone();
        let map = self.map.clone();
        let notify = self.notify.clone();
        let worker_session = session.clone();
        let session_key = session.as_str().to_owned();
        let flight_id = self.next_cleanup_flight.fetch_add(1, Ordering::Relaxed);
        let next_cleanup_flight = self.next_cleanup_flight.clone();
        #[cfg(test)]
        let cleanup_waiting_reservation_count = self.cleanup_waiting_reservation_count.clone();
        #[cfg(test)]
        let cleanup_waiting_reservation = self.cleanup_waiting_reservation.clone();
        #[cfg(test)]
        let failed_configure_retry_now = self.failed_configure_retry_now.clone();
        let (report_tx, report_rx) = watch::channel(None);
        *slot = Some(CleanupFlightSlot {
            id: flight_id,
            strength: requested,
            report: report_rx.clone(),
            #[cfg(test)]
            joined_waiters: 0,
        });
        #[cfg(test)]
        {
            self.cleanup_flight_started_count
                .fetch_add(1, Ordering::SeqCst);
            self.cleanup_flight_started.notify_waiters();
        }
        drop(slot);

        let worker = tokio::spawn({
            let worker_cell = cell.clone();
            let inner = inner.clone();
            let provider = provider.clone();
            let map = map.clone();
            let notify = notify.clone();
            let worker_session = worker_session.clone();
            #[cfg(test)]
            let cleanup_waiting_reservation_count = cleanup_waiting_reservation_count.clone();
            #[cfg(test)]
            let cleanup_waiting_reservation = cleanup_waiting_reservation.clone();
            async move {
                Self::run_cleanup_flight(
                    worker_cell,
                    inner,
                    provider,
                    map,
                    notify,
                    worker_session,
                    requested,
                    #[cfg(test)]
                    cleanup_waiting_reservation_count,
                    #[cfg(test)]
                    cleanup_waiting_reservation,
                )
                .await
            }
        });
        let cleanup_cells = self.cleanup_cells.clone();
        let sealed = self.sealed.clone();
        let reporter_cell = cell;
        tokio::spawn(async move {
            let mut worker = worker;
            let mut current_flight_id = flight_id;
            let mut current_report_tx = report_tx;
            let mut retry_delay = FAILED_CONFIGURE_RETRY_INITIAL;
            loop {
                let report = match worker.await {
                    Ok(result) => result,
                    Err(_) => Err(BridgeError::agent_crashed("worktree cleanup task failed")),
                };
                let retry_failed_configure = report.is_err()
                    && reporter_cell
                        .lifecycle
                        .lock()
                        .unwrap_or_else(|error| error.into_inner())
                        .failed_configure_cleanup_pending;
                if report.is_ok() {
                    // Successful component state is needed only through this
                    // flight. A stale reporter may notify its own waiters, but
                    // only the exact current flight can finalize shared state.
                    // Failed configuration additionally requires Release
                    // strength before its marker can clear.
                    let mut cells = cleanup_cells
                        .lock()
                        .unwrap_or_else(|error| error.into_inner());
                    let owns_cell = cells
                        .get(&session_key)
                        .is_some_and(|current| Arc::ptr_eq(current, &reporter_cell));
                    let may_finalize = if owns_cell {
                        let slot = reporter_cell
                            .flight
                            .lock()
                            .unwrap_or_else(|error| error.into_inner());
                        match slot.as_ref() {
                            Some(current) if current.id == current_flight_id => {
                                let mut lifecycle = reporter_cell
                                    .lifecycle
                                    .lock()
                                    .unwrap_or_else(|error| error.into_inner());
                                let required_release = lifecycle.failed_configure_cleanup_pending;
                                let satisfied = !required_release
                                    || current.strength >= CleanupStrength::Release;
                                if satisfied {
                                    lifecycle.failed_configure_cleanup_pending = false;
                                }
                                satisfied
                            }
                            _ => false,
                        }
                    } else {
                        false
                    };
                    if may_finalize && !sealed.load(Ordering::SeqCst) {
                        debug_assert!(cells
                            .get(&session_key)
                            .is_some_and(|current| Arc::ptr_eq(current, &reporter_cell)));
                        cells.remove(&session_key);
                        notify.notify_waiters();
                    }
                }
                current_report_tx.send_replace(Some(report));
                if !retry_failed_configure {
                    break;
                }

                // A failed configuration has no caller-owned session after it
                // returns. Keep one process-scoped retry owner in the same
                // cleanup cell. Explicit release/retirement can replace this
                // completed failed slot first; the id check then hands off to
                // that newer owner instead of running a duplicate cleanup.
                #[cfg(test)]
                tokio::select! {
                    _ = tokio::time::sleep(retry_delay) => {}
                    _ = failed_configure_retry_now.notified() => {}
                }
                #[cfg(not(test))]
                tokio::time::sleep(retry_delay).await;

                let (next_report_tx, next_report_rx) = watch::channel(None);
                let next_flight_id = next_cleanup_flight.fetch_add(1, Ordering::Relaxed);
                {
                    let mut slot = reporter_cell
                        .flight
                        .lock()
                        .unwrap_or_else(|error| error.into_inner());
                    if slot.as_ref().map(|flight| flight.id) != Some(current_flight_id) {
                        break;
                    }
                    *slot = Some(CleanupFlightSlot {
                        id: next_flight_id,
                        strength: CleanupStrength::Release,
                        report: next_report_rx,
                        #[cfg(test)]
                        joined_waiters: 0,
                    });
                }
                current_flight_id = next_flight_id;
                current_report_tx = next_report_tx;
                worker = tokio::spawn({
                    let worker_cell = reporter_cell.clone();
                    let inner = inner.clone();
                    let provider = provider.clone();
                    let map = map.clone();
                    let notify = notify.clone();
                    let worker_session = worker_session.clone();
                    #[cfg(test)]
                    let cleanup_waiting_reservation_count =
                        cleanup_waiting_reservation_count.clone();
                    #[cfg(test)]
                    let cleanup_waiting_reservation = cleanup_waiting_reservation.clone();
                    async move {
                        Self::run_cleanup_flight(
                            worker_cell,
                            inner,
                            provider,
                            map,
                            notify,
                            worker_session,
                            CleanupStrength::Release,
                            #[cfg(test)]
                            cleanup_waiting_reservation_count,
                            #[cfg(test)]
                            cleanup_waiting_reservation,
                        )
                        .await
                    }
                });
                retry_delay = retry_delay
                    .saturating_mul(2)
                    .min(FAILED_CONFIGURE_RETRY_MAX);
            }
        });
        Some((requested, report_rx))
    }

    async fn cleanup_session_observed(
        &self,
        session: &SessionId,
        strength: CleanupStrength,
        observer: Arc<dyn DiagnosticObserver>,
    ) -> Result<(), BridgeError> {
        let (started_code, completed_code, failed_code) = strength.transition_codes();
        // Select/start the observer-free cleanup flight synchronously before
        // the first diagnostic await. If the journal write or its caller is
        // canceled, dropping this report receiver only detaches observation;
        // it cannot suppress or restart cleanup.
        let cleanup = self.start_or_join_cleanup(session, strength, false);
        let started_observation = record_cleanup_transition(
            observer.as_ref(),
            bridge_core::diagnostics::PhaseStatus::Started,
            started_code,
        )
        .await;
        let result = match cleanup {
            Some((flight_strength, report)) => {
                let result = wait_for_cleanup_report(report).await;
                if result.is_ok() && flight_strength < strength {
                    self.cleanup_session(session, strength).await
                } else {
                    result
                }
            }
            None => Ok(()),
        };
        started_observation?;
        let (status, terminal_code) = if result.is_ok() {
            (
                bridge_core::diagnostics::PhaseStatus::Completed,
                completed_code,
            )
        } else {
            (bridge_core::diagnostics::PhaseStatus::Failed, failed_code)
        };
        let observation = record_cleanup_transition(observer.as_ref(), status, terminal_code).await;
        match (result, observation) {
            (Err(primary), _) => Err(primary),
            (Ok(()), Err(observation)) => Err(observation),
            (Ok(()), Ok(())) => Ok(()),
        }
    }

    #[allow(clippy::too_many_arguments)]
    async fn run_cleanup_flight(
        cell: Arc<CleanupCell>,
        inner: Arc<dyn AgentBackend>,
        provider: Arc<dyn WorktreeProvider>,
        map: Arc<Mutex<HashMap<String, WtState>>>,
        notify: Arc<Notify>,
        session: SessionId,
        strength: CleanupStrength,
        #[cfg(test)] cleanup_waiting_reservation_count: Arc<AtomicU64>,
        #[cfg(test)] cleanup_waiting_reservation: Arc<Notify>,
    ) -> Result<(), BridgeError> {
        // Configure admission is published synchronously, before its first
        // git/inner await. Cleanup claims the same cell and waits for every
        // already-admitted configure to settle, closing the pre-reservation
        // configure-after-release window for git and pass-through sessions.
        loop {
            let settled = cell.configure_settled.notified();
            let configuring = cell
                .lifecycle
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .configuring;
            if configuring == 0 {
                break;
            }
            #[cfg(test)]
            {
                cleanup_waiting_reservation_count.fetch_add(1, Ordering::SeqCst);
                cleanup_waiting_reservation.notify_waiters();
            }
            settled.await;
        }

        // This mutex is the per-session single-flight boundary. A stronger
        // release waits for an in-flight forget, then performs only the missing
        // stronger inner step; concurrent equal requests join the completed
        // component state.
        let mut state = cell.state.lock().await;
        let mut first_error = None;
        // A reserving configure may not have invoked the inner backend yet. Let
        // it publish Ready (or remove its failed reservation) before teardown,
        // otherwise configure could resurrect inner state after release.
        if state.entry.is_none() {
            state.entry = Self::entry_for_cleanup(map.as_ref(), notify.as_ref(), &session).await;
        }
        let entry = state.entry.clone();

        if state.inner_strength.is_none_or(|done| done < strength) {
            let inner_result = match strength {
                CleanupStrength::Forget => inner.forget_session_checked(&session).await,
                CleanupStrength::Release => inner.release_session_checked(&session).await,
            };
            match inner_result {
                Ok(()) => state.inner_strength = Some(strength),
                Err(error) => first_error = Some(error),
            }
        }

        if let Some(entry) = entry {
            if !state.provider_removed {
                match provider
                    .remove(&entry.canonical_source, &entry.worktree_path)
                    .await
                {
                    Ok(()) => state.provider_removed = true,
                    Err(error) if first_error.is_none() => first_error = Some(error),
                    Err(_) => {}
                }
            }
            if !state.sidecar_removed {
                match std::fs::remove_file(sidecar_path(&entry.worktree_path)) {
                    Ok(()) => state.sidecar_removed = true,
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                        state.sidecar_removed = true;
                    }
                    Err(_) if first_error.is_none() => {
                        first_error = Some(BridgeError::agent_crashed(
                            "worktree sidecar removal failed",
                        ));
                    }
                    Err(_) => {}
                }
            }
            if state.provider_removed && state.sidecar_removed {
                let mut map = map.lock().await;
                let still_same = matches!(
                    map.get(session.as_str()),
                    Some(WtState::Ready(current))
                        if current.canonical_source == entry.canonical_source
                            && current.worktree_path == entry.worktree_path
                );
                if still_same {
                    map.remove(session.as_str());
                    notify.notify_waiters();
                }
                state.entry = None;
            }
        } else {
            state.provider_removed = true;
            state.sidecar_removed = true;
        }

        match first_error {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }
}

async fn wait_for_cleanup_report(mut report: CleanupReportReceiver) -> Result<(), BridgeError> {
    loop {
        if let Some(result) = report.borrow().clone() {
            return result;
        }
        if report.changed().await.is_err() {
            return Err(BridgeError::agent_crashed(
                "worktree cleanup report channel closed",
            ));
        }
    }
}

async fn record_cleanup_transition(
    observer: &dyn DiagnosticObserver,
    status: bridge_core::diagnostics::PhaseStatus,
    code: &'static str,
) -> Result<(), BridgeError> {
    use bridge_core::diagnostics::{
        diagnostic_timestamp_ms, DiagnosticEvent, DiagnosticPhase, DiagnosticRedactor,
        PersistedPhaseTransition, PersistedPhaseTransitionInput,
    };

    let transition = PersistedPhaseTransition::build_static_code(
        PersistedPhaseTransitionInput {
            phase: DiagnosticPhase::Teardown,
            status,
            at_ms: diagnostic_timestamp_ms(),
            operation: None,
            code: None,
            auth: None,
        },
        Some(code),
        &DiagnosticRedactor::default(),
    )
    .map_err(|_| BridgeError::InvalidStateTransition)?;
    observer
        .record(
            DiagnosticEvent::new(transition, None)
                .map_err(|_| BridgeError::InvalidStateTransition)?,
        )
        .await
}

#[async_trait::async_trait]
impl AgentBackend for WorktreeBackend {
    async fn prompt(
        &self,
        session: &SessionId,
        parts: Vec<Part>,
    ) -> Result<BackendStream, BridgeError> {
        self.inner.prompt(session, parts).await
    }

    async fn prompt_observed(
        &self,
        session: &SessionId,
        parts: Vec<Part>,
        sink: Arc<dyn RichEventSink>,
    ) -> Result<BackendStream, BridgeError> {
        self.inner.prompt_observed(session, parts, sink).await
    }

    async fn prompt_with_observers(
        &self,
        session: &SessionId,
        parts: Vec<Part>,
        observers: BackendObservers,
    ) -> Result<BackendStream, BridgeError> {
        self.inner
            .prompt_with_observers(session, parts, observers)
            .await
    }

    async fn cancel(&self, session: &SessionId) -> Result<(), BridgeError> {
        self.inner.cancel(session).await
    }

    async fn cancel_observed(
        &self,
        session: &SessionId,
        observer: Arc<dyn DiagnosticObserver>,
    ) -> Result<(), BridgeError> {
        self.inner.cancel_observed(session, observer).await
    }

    async fn configure_turn(&self, session: &SessionId, meta: TurnMeta) {
        self.inner.configure_turn(session, meta).await;
    }

    async fn configure_session(
        &self,
        session: &SessionId,
        spec: &SessionSpec,
    ) -> Result<(), BridgeError> {
        let mut admission = self.admit_configure(session)?;
        let repo = match &spec.cwd {
            Some(c) => c.clone(),
            None => {
                let result = self.inner.configure_session(session, spec).await;
                if result.is_ok() {
                    admission.retain_for_session();
                }
                return result;
            }
        };

        if !self.provider.is_git_repo(repo.as_str()).await {
            let result = self.inner.configure_session(session, spec).await;
            if result.is_ok() {
                admission.retain_for_session();
            }
            return result;
        }

        let resolved = resolve_worktree(
            &self.cfg,
            &self.allowed_root,
            repo.as_str(),
            session.as_str(),
        )?;
        let key = session.as_str().to_string();
        let reservation_entry = WtEntry {
            canonical_source: resolved.canonical_source.clone(),
            worktree_path: resolved.worktree_path.clone(),
        };
        let admission_cell = admission.cell.clone();
        let claim;

        loop {
            let map_changed = self.notify.notified();
            let configure_changed = admission_cell.configure_settled.notified();
            let mut map = self.map.lock().await;
            match map.get(session.as_str()) {
                Some(WtState::Ready(e)) => {
                    if e.canonical_source != resolved.canonical_source {
                        return Err(BridgeError::ConfigMismatch { field: "cwd" });
                    }
                    let worktree_path = e.worktree_path.clone();
                    drop(map);
                    let sub = SessionSpec {
                        config: spec.config.clone(),
                        cwd: Some(SessionCwd::parse(&worktree_path)?),
                    };
                    let result = self.inner.configure_session(session, &sub).await;
                    if result.is_ok() {
                        admission.retain_for_session();
                    }
                    return result;
                }
                Some(WtState::Reserving { configure, .. }) => {
                    let owner_active = admission_cell
                        .lifecycle
                        .lock()
                        .unwrap_or_else(|error| error.into_inner())
                        .active_configures
                        .contains(configure);
                    drop(map);
                    if !owner_active {
                        // The reservation owner was canceled before publishing
                        // Ready. Give up our own admission so the cleanup flight
                        // can take the orphaned metadata without self-waiting.
                        admission.retain_failed_configure_cleanup();
                        drop(admission);
                        self.cleanup_session_with_sealed_admission(
                            session,
                            CleanupStrength::Release,
                            true,
                        )
                        .await?;
                        return Err(BridgeError::SessionExpired);
                    }
                    tokio::select! {
                        _ = map_changed => {}
                        _ = configure_changed => {}
                    }
                }
                None => {
                    if self.sealed.load(Ordering::SeqCst) {
                        return Err(BridgeError::SessionExpired);
                    }
                    admission.arm_cleanup_on_drop();
                    claim = self.next_claim.fetch_add(1, Ordering::Relaxed);
                    map.insert(
                        key.clone(),
                        WtState::Reserving {
                            claim,
                            configure: admission.id(),
                            entry: reservation_entry.clone(),
                        },
                    );
                    break;
                }
            }
        }

        let common_dir = match self
            .provider
            .add(&resolved.canonical_source, &resolved.worktree_path)
            .await
        {
            Ok(common_dir) => common_dir,
            Err(e) => {
                // Keep the reservation metadata until the shared cleanup cell
                // owns it. Provider add may have partially succeeded, so an
                // explicit retry must retain the exact source/path.
                admission.retain_failed_configure_cleanup();
                drop(admission);
                let _ = self
                    .cleanup_session_with_sealed_admission(session, CleanupStrength::Release, true)
                    .await;
                return Err(e);
            }
        };

        let sidecar = WorktreeSidecar {
            canonical_source: resolved.canonical_source.clone(),
            common_dir,
            worktree_path: resolved.worktree_path.clone(),
            owner: self.cfg.owner.clone(),
            run_id: self.identity.run_id.clone(),
            host: self.identity.host.clone(),
            lease: self.identity.lease.clone(),
        };
        if let Err(e) = write_sidecar(&sidecar) {
            admission.retain_failed_configure_cleanup();
            drop(admission);
            let _ = self
                .cleanup_session_with_sealed_admission(session, CleanupStrength::Release, true)
                .await;
            return Err(e);
        }

        let sub_cwd = match SessionCwd::parse(&resolved.worktree_path) {
            Ok(cwd) => cwd,
            Err(e) => {
                admission.retain_failed_configure_cleanup();
                drop(admission);
                let _ = self
                    .cleanup_session_with_sealed_admission(session, CleanupStrength::Release, true)
                    .await;
                return Err(e);
            }
        };
        let sub = SessionSpec {
            config: spec.config.clone(),
            cwd: Some(sub_cwd),
        };
        if let Err(e) = self.inner.configure_session(session, &sub).await {
            admission.retain_failed_configure_cleanup();
            drop(admission);
            let _ = self
                .cleanup_session_with_sealed_admission(session, CleanupStrength::Release, true)
                .await;
            return Err(e);
        }

        let mut map = self.map.lock().await;
        let owns_claim = matches!(
            map.get(session.as_str()),
            Some(WtState::Reserving { claim: current, .. }) if *current == claim
        );
        if owns_claim {
            let sealed = self.sealed.load(Ordering::SeqCst);
            map.insert(
                key,
                WtState::Ready(WtEntry {
                    canonical_source: resolved.canonical_source,
                    worktree_path: resolved.worktree_path,
                }),
            );
            self.notify.notify_waiters();
            drop(map);
            if sealed {
                // Per-session cleanup waits for admitted configuration. Give
                // up this admission before joining the cleanup flight to avoid
                // waiting on ourselves during retirement.
                admission.retain_failed_configure_cleanup();
                drop(admission);
                let _ = self
                    .cleanup_session_with_sealed_admission(session, CleanupStrength::Release, true)
                    .await;
                return Err(BridgeError::SessionExpired);
            }
            admission.retain_for_session();
            return Ok(());
        }
        drop(map);
        admission.retain_failed_configure_cleanup();
        drop(admission);
        let _ = self
            .cleanup_session_with_sealed_admission(session, CleanupStrength::Release, true)
            .await;
        self.notify.notify_waiters();
        Err(BridgeError::SessionExpired)
    }

    async fn forget_session(&self, session: &SessionId) {
        let _ = self.cleanup_session(session, CleanupStrength::Forget).await;
    }

    async fn forget_session_checked(&self, session: &SessionId) -> Result<(), BridgeError> {
        self.cleanup_session(session, CleanupStrength::Forget).await
    }

    async fn forget_session_observed(
        &self,
        session: &SessionId,
        observer: Arc<dyn DiagnosticObserver>,
    ) -> Result<(), BridgeError> {
        self.cleanup_session_observed(session, CleanupStrength::Forget, observer)
            .await
    }

    async fn release_session(&self, session: &SessionId) {
        let _ = self
            .cleanup_session(session, CleanupStrength::Release)
            .await;
    }

    async fn release_session_checked(&self, session: &SessionId) -> Result<(), BridgeError> {
        self.cleanup_session(session, CleanupStrength::Release)
            .await
    }

    async fn release_session_observed(
        &self,
        session: &SessionId,
        observer: Arc<dyn DiagnosticObserver>,
    ) -> Result<(), BridgeError> {
        self.cleanup_session_observed(session, CleanupStrength::Release, observer)
            .await
    }

    async fn reconcile_config(
        &self,
        session: &SessionId,
        spec: &SessionSpec,
    ) -> Result<ReconcileOutcome, BridgeError> {
        let mapped = match self.map.lock().await.get(session.as_str()) {
            Some(WtState::Ready(e)) => Some(e.worktree_path.clone()),
            _ => None,
        };
        match mapped {
            Some(wt) => {
                let sub = SessionSpec {
                    config: spec.config.clone(),
                    cwd: Some(SessionCwd::parse(&wt)?),
                };
                self.inner.reconcile_config(session, &sub).await
            }
            None => self.inner.reconcile_config(session, spec).await,
        }
    }

    fn capabilities(&self) -> AgentSessionCaps {
        self.inner.capabilities()
    }

    async fn retire(&self) -> Result<(), BridgeError> {
        {
            // Linearize the admission boundary with configure publication and
            // successful-flight eviction. A configured session already owns a
            // retained cell, so no known owner can fall through the sealed
            // no-op path before retirement snapshots it.
            let _cells = self
                .cleanup_cells
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            self.sealed.store(true, Ordering::SeqCst);
        }

        loop {
            let settled = self.configure_settled.notified();
            if self.configure_inflight.load(Ordering::SeqCst) == 0 {
                break;
            }
            settled.await;
        }

        // No configure admission remains. A Ready entry is ordinary retirement
        // work; any remaining Reserving entry is an ownerless canceled
        // configure whose stored cleanup metadata is now safe to take over.
        let mut sessions: Vec<String> = self.map.lock().await.keys().cloned().collect();
        sessions.extend(
            self.cleanup_cells
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .keys()
                .cloned(),
        );
        sessions.sort();
        sessions.dedup();

        let mut first_error = None;
        for raw in sessions {
            #[cfg(test)]
            {
                if self
                    .cleanup_cells
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .contains_key(&raw)
                {
                    self.retirement_joined_cell_count
                        .fetch_add(1, Ordering::SeqCst);
                    self.retirement_joined_cell.notify_waiters();
                }
            }
            let Ok(session) = SessionId::parse(raw) else {
                if first_error.is_none() {
                    first_error = Some(BridgeError::InvalidStateTransition);
                }
                continue;
            };
            if let Err(error) = self
                .cleanup_session_with_sealed_admission(&session, CleanupStrength::Release, true)
                .await
            {
                if first_error.is_none() {
                    first_error = Some(error);
                }
            }
        }
        if let Err(error) = self.inner.retire().await {
            if first_error.is_none() {
                first_error = Some(error);
            }
        }
        match first_error {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bridge_core::domain::{EffectiveConfig, Part, SessionSpec};
    use bridge_core::error::BridgeError;
    use bridge_core::ids::SessionId;
    use bridge_core::ports::{
        AgentBackend, BackendStream, DiagnosticObserver, RichEventSink, Update,
    };
    use bridge_core::SessionCwd;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, SystemTime, UNIX_EPOCH};
    use tokio::sync::{oneshot, Notify};
    use tokio_stream::StreamExt;

    #[derive(Default)]
    struct Rec {
        configured_cwd: Mutex<Vec<Option<String>>>,
        order: Mutex<Vec<String>>,
        configure_count: AtomicUsize,
        fail_configure: AtomicBool,
        configure_gate: Mutex<Option<oneshot::Receiver<()>>>,
        blocked_configure_started_count: AtomicUsize,
        blocked_configure_started: Notify,
        add_count: AtomicUsize,
        remove_count: AtomicUsize,
        remove_started: Notify,
        fail_remove: AtomicBool,
        fail_release: AtomicBool,
        retire_count: AtomicUsize,
        retire_gate: Mutex<Option<oneshot::Receiver<()>>>,
        retire_started_count: AtomicUsize,
        retire_started: Notify,
        composite_count: AtomicUsize,
        diagnostics: Mutex<Vec<Arc<dyn DiagnosticObserver>>>,
        rich_sinks: Mutex<Vec<Arc<dyn RichEventSink>>>,
    }

    impl Rec {
        fn block_next_configure(&self) -> oneshot::Sender<()> {
            let (allow, gate) = oneshot::channel();
            assert!(
                self.configure_gate.lock().unwrap().replace(gate).is_none(),
                "only one inner configure gate may be armed"
            );
            allow
        }

        async fn wait_for_blocked_configure(&self) {
            while self.blocked_configure_started_count.load(Ordering::SeqCst) == 0 {
                let started = self.blocked_configure_started.notified();
                if self.blocked_configure_started_count.load(Ordering::SeqCst) == 0 {
                    started.await;
                }
            }
        }

        async fn wait_for_remove_count(&self, expected: usize) {
            while self.remove_count.load(Ordering::SeqCst) < expected {
                let started = self.remove_started.notified();
                if self.remove_count.load(Ordering::SeqCst) < expected {
                    started.await;
                }
            }
        }

        fn block_next_retire(&self) -> oneshot::Sender<()> {
            let (allow, gate) = oneshot::channel();
            assert!(
                self.retire_gate.lock().unwrap().replace(gate).is_none(),
                "only one inner retire gate may be armed"
            );
            allow
        }

        async fn wait_for_blocked_retire(&self) {
            while self.retire_started_count.load(Ordering::SeqCst) == 0 {
                let started = self.retire_started.notified();
                if self.retire_started_count.load(Ordering::SeqCst) == 0 {
                    started.await;
                }
            }
        }
    }

    struct FakeInner {
        rec: Arc<Rec>,
    }

    #[async_trait::async_trait]
    impl AgentBackend for FakeInner {
        async fn prompt(
            &self,
            _session: &SessionId,
            _parts: Vec<Part>,
        ) -> Result<BackendStream, BridgeError> {
            Ok(Box::pin(tokio_stream::iter(Vec::<
                Result<Update, BridgeError>,
            >::new())))
        }

        async fn prompt_with_observers(
            &self,
            _session: &SessionId,
            _parts: Vec<Part>,
            observers: BackendObservers,
        ) -> Result<BackendStream, BridgeError> {
            self.rec.composite_count.fetch_add(1, Ordering::SeqCst);
            self.rec
                .diagnostics
                .lock()
                .unwrap()
                .push(observers.diagnostic);
            self.rec
                .rich_sinks
                .lock()
                .unwrap()
                .push(observers.rich.expect("test supplies a rich sink"));
            Ok(Box::pin(tokio_stream::iter(Vec::<
                Result<Update, BridgeError>,
            >::new())))
        }

        async fn cancel(&self, _session: &SessionId) -> Result<(), BridgeError> {
            Ok(())
        }

        async fn configure_session(
            &self,
            _session: &SessionId,
            spec: &SessionSpec,
        ) -> Result<(), BridgeError> {
            self.rec.configure_count.fetch_add(1, Ordering::SeqCst);
            self.rec
                .configured_cwd
                .lock()
                .unwrap()
                .push(spec.cwd.as_ref().map(|c| c.as_str().to_string()));
            let gate = self.rec.configure_gate.lock().unwrap().take();
            if let Some(gate) = gate {
                self.rec
                    .blocked_configure_started_count
                    .fetch_add(1, Ordering::SeqCst);
                self.rec.blocked_configure_started.notify_waiters();
                let _ = gate.await;
            }
            if self.rec.fail_configure.load(Ordering::SeqCst) {
                Err(BridgeError::StoreFailure)
            } else {
                Ok(())
            }
        }

        async fn forget_session(&self, _session: &SessionId) {
            self.rec.order.lock().unwrap().push("inner_forget".into());
        }

        async fn release_session(&self, _session: &SessionId) {
            self.rec.order.lock().unwrap().push("inner_release".into());
        }

        async fn release_session_checked(&self, session: &SessionId) -> Result<(), BridgeError> {
            self.release_session(session).await;
            if self.rec.fail_release.load(Ordering::SeqCst) {
                Err(BridgeError::StoreFailure)
            } else {
                Ok(())
            }
        }

        async fn retire(&self) -> Result<(), BridgeError> {
            self.rec.retire_count.fetch_add(1, Ordering::SeqCst);
            self.rec.order.lock().unwrap().push("inner_retire".into());
            let gate = self.rec.retire_gate.lock().unwrap().take();
            self.rec.retire_started_count.fetch_add(1, Ordering::SeqCst);
            self.rec.retire_started.notify_waiters();
            if let Some(gate) = gate {
                let _ = gate.await;
            }
            Ok(())
        }
    }

    struct FakeProv {
        rec: Arc<Rec>,
    }

    struct NonGitProv {
        rec: Arc<Rec>,
    }

    struct SidecarWriteFailProv {
        rec: Arc<Rec>,
    }

    struct PartialAddFailProv {
        rec: Arc<Rec>,
    }

    #[async_trait::async_trait]
    impl crate::provider::WorktreeProvider for FakeProv {
        async fn add(&self, _repo: &str, _worktree_path: &str) -> Result<String, BridgeError> {
            self.rec.add_count.fetch_add(1, Ordering::SeqCst);
            tokio::task::yield_now().await;
            Ok(String::new())
        }

        async fn remove(&self, _repo: &str, _worktree_path: &str) -> Result<(), BridgeError> {
            self.rec.remove_count.fetch_add(1, Ordering::SeqCst);
            self.rec.remove_started.notify_waiters();
            self.rec.order.lock().unwrap().push("wt_remove".into());
            if self.rec.fail_remove.load(Ordering::SeqCst) {
                Err(BridgeError::StoreFailure)
            } else {
                Ok(())
            }
        }

        async fn is_git_repo(&self, _path: &str) -> bool {
            true
        }
    }

    #[async_trait::async_trait]
    impl crate::provider::WorktreeProvider for NonGitProv {
        async fn add(&self, _repo: &str, _worktree_path: &str) -> Result<String, BridgeError> {
            self.rec.add_count.fetch_add(1, Ordering::SeqCst);
            Err(BridgeError::InvalidStateTransition)
        }

        async fn remove(&self, _repo: &str, _worktree_path: &str) -> Result<(), BridgeError> {
            self.rec.remove_count.fetch_add(1, Ordering::SeqCst);
            Err(BridgeError::InvalidStateTransition)
        }

        async fn is_git_repo(&self, _path: &str) -> bool {
            false
        }
    }

    #[async_trait::async_trait]
    impl crate::provider::WorktreeProvider for SidecarWriteFailProv {
        async fn add(&self, _repo: &str, worktree_path: &str) -> Result<String, BridgeError> {
            self.rec.add_count.fetch_add(1, Ordering::SeqCst);
            std::fs::create_dir_all(format!("{}.tmp", sidecar_path(worktree_path))).unwrap();
            Ok(String::new())
        }

        async fn remove(&self, _repo: &str, worktree_path: &str) -> Result<(), BridgeError> {
            self.rec.remove_count.fetch_add(1, Ordering::SeqCst);
            self.rec.order.lock().unwrap().push("wt_remove".into());
            if self.rec.fail_remove.load(Ordering::SeqCst) {
                Err(BridgeError::StoreFailure)
            } else {
                let _ = std::fs::remove_dir_all(format!("{}.tmp", sidecar_path(worktree_path)));
                Ok(())
            }
        }

        async fn is_git_repo(&self, _path: &str) -> bool {
            true
        }
    }

    #[async_trait::async_trait]
    impl crate::provider::WorktreeProvider for PartialAddFailProv {
        async fn add(&self, _repo: &str, _worktree_path: &str) -> Result<String, BridgeError> {
            self.rec.add_count.fetch_add(1, Ordering::SeqCst);
            Err(BridgeError::StoreFailure)
        }

        async fn remove(&self, _repo: &str, _worktree_path: &str) -> Result<(), BridgeError> {
            self.rec.remove_count.fetch_add(1, Ordering::SeqCst);
            self.rec.order.lock().unwrap().push("wt_remove".into());
            if self.rec.fail_remove.load(Ordering::SeqCst) {
                Err(BridgeError::StoreFailure)
            } else {
                Ok(())
            }
        }

        async fn is_git_repo(&self, _path: &str) -> bool {
            true
        }
    }

    fn unique_temp_dir(name: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "a2a-bridge-backend-{name}-{}-{nanos}",
            std::process::id()
        ))
    }

    fn spec(cwd: Option<&str>) -> SessionSpec {
        SessionSpec {
            config: EffectiveConfig::default(),
            cwd: cwd.map(|c| SessionCwd::parse(c).unwrap()),
        }
    }

    fn identity() -> WorktreeIdentity {
        WorktreeIdentity {
            run_id: "run-id".into(),
            host: "host-a".into(),
            lease: "/tmp/a2a-bridge-test.lock".into(),
        }
    }

    fn backend_fixture(
        name: &str,
    ) -> (
        Arc<WorktreeBackend>,
        Arc<Rec>,
        PathBuf,
        PathBuf,
        crate::provider_path::WorktreeConfig,
    ) {
        let tmp = unique_temp_dir(name);
        let allowed_root = tmp.join("allowed");
        let source = allowed_root.join("source");
        let worktree_root = tmp.join("worktrees");
        std::fs::create_dir_all(&source).unwrap();
        std::fs::create_dir_all(&worktree_root).unwrap();
        let canonical_allowed_root = std::fs::canonicalize(&allowed_root).unwrap();
        let canonical_worktree_root = std::fs::canonicalize(&worktree_root).unwrap();

        let rec = Arc::new(Rec::default());
        let cfg = crate::provider_path::WorktreeConfig {
            root: canonical_worktree_root.to_string_lossy().into_owned(),
            owner: "ownr".into(),
            run: "run7".into(),
        };
        let be = Arc::new(WorktreeBackend::new(
            Arc::new(FakeInner { rec: rec.clone() }),
            Arc::new(FakeProv { rec: rec.clone() }),
            cfg.clone(),
            Some(SessionCwd::parse(&canonical_allowed_root.to_string_lossy()).unwrap()),
            identity(),
        ));
        (be, rec, tmp, source, cfg)
    }

    #[derive(Default)]
    struct MarkerDiagnostic;

    #[async_trait::async_trait]
    impl DiagnosticObserver for MarkerDiagnostic {
        async fn record(
            &self,
            _event: bridge_core::diagnostics::DiagnosticEvent,
        ) -> Result<(), BridgeError> {
            Ok(())
        }
    }

    #[derive(Default)]
    struct RecordingDiagnostic {
        events: Mutex<Vec<(bridge_core::diagnostics::PhaseStatus, String)>>,
    }

    struct RejectingDiagnostic;

    #[derive(Default)]
    struct PendingDiagnostic {
        entered_count: AtomicUsize,
        entered: Notify,
    }

    impl PendingDiagnostic {
        async fn wait_until_entered(&self) {
            while self.entered_count.load(Ordering::SeqCst) == 0 {
                let entered = self.entered.notified();
                if self.entered_count.load(Ordering::SeqCst) == 0 {
                    entered.await;
                }
            }
        }
    }

    #[async_trait::async_trait]
    impl DiagnosticObserver for RejectingDiagnostic {
        async fn record(
            &self,
            _event: bridge_core::diagnostics::DiagnosticEvent,
        ) -> Result<(), BridgeError> {
            Err(BridgeError::StoreFailure)
        }
    }

    #[async_trait::async_trait]
    impl DiagnosticObserver for PendingDiagnostic {
        async fn record(
            &self,
            _event: bridge_core::diagnostics::DiagnosticEvent,
        ) -> Result<(), BridgeError> {
            self.entered_count.fetch_add(1, Ordering::SeqCst);
            self.entered.notify_waiters();
            std::future::pending().await
        }
    }

    #[async_trait::async_trait]
    impl DiagnosticObserver for RecordingDiagnostic {
        async fn record(
            &self,
            event: bridge_core::diagnostics::DiagnosticEvent,
        ) -> Result<(), BridgeError> {
            let transition = event.transition();
            self.events.lock().unwrap().push((
                transition.status(),
                transition
                    .code()
                    .map(|code| code.as_str().to_owned())
                    .unwrap_or_default(),
            ));
            Ok(())
        }
    }

    #[derive(Default)]
    struct MarkerRichSink;

    #[async_trait::async_trait]
    impl RichEventSink for MarkerRichSink {
        fn record(&self, _kind: bridge_core::orch::OrchEventKind) {}

        async fn flush(&self) -> Result<(), BridgeError> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn prompt_with_observers_forwards_both_channels_exactly_once() {
        let (backend, rec, tmp, _source, _cfg) = backend_fixture("composite-forwarding");
        let session = SessionId::parse("ctx-composite-g0").unwrap();
        let diagnostic: Arc<dyn DiagnosticObserver> = Arc::new(MarkerDiagnostic);
        let rich: Arc<dyn RichEventSink> = Arc::new(MarkerRichSink);

        let mut stream = backend
            .prompt_with_observers(
                &session,
                vec![],
                BackendObservers::new(diagnostic.clone(), Some(rich.clone())),
            )
            .await
            .unwrap();
        assert!(stream.next().await.is_none());

        assert_eq!(rec.composite_count.load(Ordering::SeqCst), 1);
        let seen_diagnostics = rec.diagnostics.lock().unwrap();
        assert_eq!(seen_diagnostics.len(), 1);
        assert!(Arc::ptr_eq(&seen_diagnostics[0], &diagnostic));
        let seen_rich = rec.rich_sinks.lock().unwrap();
        assert_eq!(seen_rich.len(), 1);
        assert!(Arc::ptr_eq(&seen_rich[0], &rich));

        std::fs::remove_dir_all(tmp).unwrap();
    }

    #[tokio::test]
    async fn configure_substitutes_then_release_delegates_then_removes() {
        let (be, rec, tmp, source, cfg) = backend_fixture("release");
        let sid = SessionId::parse("ctx-c1-g0").unwrap();

        be.configure_session(&sid, &spec(Some(&source.to_string_lossy())))
            .await
            .unwrap();

        let seen = rec.configured_cwd.lock().unwrap()[0].clone().unwrap();
        assert!(
            seen.starts_with(&cfg.root),
            "inner cwd substituted to the worktree root: {seen}"
        );
        assert_ne!(seen, source.to_string_lossy());

        be.release_session(&sid).await;
        assert_eq!(
            rec.order.lock().unwrap().as_slice(),
            ["inner_release", "wt_remove"],
            "delegate-then-remove"
        );

        std::fs::remove_dir_all(tmp).unwrap();
    }

    #[tokio::test]
    async fn successful_passthrough_configure_survives_later_canceled_admission() {
        let (be, rec, tmp, _source, _cfg) = backend_fixture("retained-cell-canceled-configure");
        let sid = SessionId::parse("ctx-retained-cell-canceled-configure-g0").unwrap();
        be.configure_session(&sid, &spec(None)).await.unwrap();
        assert_eq!(be.cleanup_cell_count(), 1);

        let _allow_configure = rec.block_next_configure();
        let configure_be = be.clone();
        let configure_sid = sid.clone();
        let configure = tokio::spawn(async move {
            configure_be
                .configure_session(&configure_sid, &spec(None))
                .await
        });
        rec.wait_for_blocked_configure().await;
        configure.abort();
        assert!(configure.await.unwrap_err().is_cancelled());
        assert_eq!(
            be.cleanup_cell_count(),
            1,
            "a later canceled admission must not erase an earlier configured owner"
        );

        {
            let _cells = be
                .cleanup_cells
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            be.sealed.store(true, Ordering::SeqCst);
        }
        rec.fail_release.store(true, Ordering::SeqCst);
        assert_eq!(
            be.release_session_checked(&sid).await,
            Err(BridgeError::StoreFailure),
            "known post-seal release must still reach the retained inner session"
        );
        rec.fail_release.store(false, Ordering::SeqCst);
        be.retire().await.unwrap();
        assert_eq!(
            rec.order
                .lock()
                .unwrap()
                .iter()
                .filter(|step| step.as_str() == "inner_release")
                .count(),
            2,
            "retirement retries the one failed inner component"
        );
        std::fs::remove_dir_all(tmp).unwrap();
    }

    #[tokio::test]
    async fn successful_passthrough_configure_survives_later_failed_admission() {
        let (be, rec, tmp, _source, _cfg) = backend_fixture("retained-cell-failed-configure");
        let sid = SessionId::parse("ctx-retained-cell-failed-configure-g0").unwrap();
        be.configure_session(&sid, &spec(None)).await.unwrap();
        rec.fail_configure.store(true, Ordering::SeqCst);

        assert_eq!(
            be.configure_session(&sid, &spec(None)).await,
            Err(BridgeError::StoreFailure)
        );
        assert_eq!(
            be.cleanup_cell_count(),
            1,
            "a later failed admission must not erase an earlier configured owner"
        );

        rec.fail_configure.store(false, Ordering::SeqCst);
        be.release_session_checked(&sid).await.unwrap();
        assert_eq!(be.cleanup_cell_count(), 0);
        std::fs::remove_dir_all(tmp).unwrap();
    }

    #[tokio::test]
    async fn successful_cleanup_cells_do_not_accumulate_across_distinct_sessions() {
        let (be, _rec, tmp, source, _cfg) = backend_fixture("cleanup-cell-retirement");

        for index in 0..3 {
            let sid = SessionId::parse(format!("ctx-cleanup-retire-{index}-g0")).unwrap();
            be.configure_session(&sid, &spec(Some(&source.to_string_lossy())))
                .await
                .unwrap();
            be.release_session_checked(&sid).await.unwrap();
            assert_eq!(
                be.cleanup_cell_count(),
                0,
                "a completed flight must retire its map entry before reporting success"
            );
        }

        std::fs::remove_dir_all(tmp).unwrap();
    }

    #[tokio::test]
    async fn forget_then_release_upgrades_inner_cleanup_without_repeating_worktree_removal() {
        let (be, rec, tmp, source, _cfg) = backend_fixture("cleanup-upgrade");
        let sid = SessionId::parse("ctx-upgrade-g0").unwrap();
        be.configure_session(&sid, &spec(Some(&source.to_string_lossy())))
            .await
            .unwrap();

        let observer = Arc::new(RecordingDiagnostic::default());
        be.forget_session_observed(&sid, observer.clone())
            .await
            .unwrap();
        be.release_session_checked(&sid).await.unwrap();

        assert_eq!(
            rec.order.lock().unwrap().as_slice(),
            ["inner_forget", "wt_remove", "inner_release"],
            "release is a monotonic inner upgrade and joins completed metadata cleanup"
        );
        assert_eq!(rec.remove_count.load(Ordering::SeqCst), 1);
        assert_eq!(
            observer.events.lock().unwrap().as_slice(),
            [
                (
                    bridge_core::diagnostics::PhaseStatus::Started,
                    "worktree.teardown.forget".to_owned(),
                ),
                (
                    bridge_core::diagnostics::PhaseStatus::Completed,
                    "worktree.teardown.forgotten".to_owned(),
                ),
            ]
        );
        std::fs::remove_dir_all(tmp).unwrap();
    }

    #[tokio::test]
    async fn observed_release_propagates_provider_failure_and_retries_only_failed_component() {
        let (be, rec, tmp, source, _cfg) = backend_fixture("provider-retry");
        let sid = SessionId::parse("ctx-provider-retry-g0").unwrap();
        be.configure_session(&sid, &spec(Some(&source.to_string_lossy())))
            .await
            .unwrap();
        rec.fail_remove.store(true, Ordering::SeqCst);
        let observer = Arc::new(RecordingDiagnostic::default());

        assert_eq!(
            be.release_session_observed(&sid, observer.clone()).await,
            Err(BridgeError::StoreFailure)
        );
        assert_eq!(
            be.cleanup_cell_count(),
            1,
            "a failed flight retains component state for an explicit retry"
        );
        assert!(be.map.lock().await.contains_key(sid.as_str()));
        rec.fail_remove.store(false, Ordering::SeqCst);
        be.release_session_checked(&sid).await.unwrap();

        let order = rec.order.lock().unwrap().clone();
        assert_eq!(
            order
                .iter()
                .filter(|step| step.as_str() == "inner_release")
                .count(),
            1,
            "successful inner release is not repeated while provider removal retries"
        );
        assert_eq!(rec.remove_count.load(Ordering::SeqCst), 2);
        assert!(be.map.lock().await.is_empty());
        assert_eq!(
            be.cleanup_cell_count(),
            0,
            "a successful retry retires the completed cleanup cell"
        );
        assert_eq!(
            observer.events.lock().unwrap().as_slice(),
            [
                (
                    bridge_core::diagnostics::PhaseStatus::Started,
                    "worktree.teardown.release".to_owned(),
                ),
                (
                    bridge_core::diagnostics::PhaseStatus::Failed,
                    "worktree.teardown.release_failed".to_owned(),
                ),
            ]
        );
        std::fs::remove_dir_all(tmp).unwrap();
    }

    #[tokio::test]
    async fn ownerless_reservation_retry_retains_worktree_metadata_after_provider_failure() {
        let (be, rec, tmp, source, _cfg) = backend_fixture("reservation-provider-retry");
        let sid = SessionId::parse("ctx-reservation-provider-retry-g0").unwrap();
        let _allow_configure = rec.block_next_configure();
        let configure_be = be.clone();
        let configure_sid = sid.clone();
        let session_spec = spec(Some(&source.to_string_lossy()));
        let configure = tokio::spawn(async move {
            configure_be
                .configure_session(&configure_sid, &session_spec)
                .await
        });
        rec.wait_for_blocked_configure().await;
        rec.fail_remove.store(true, Ordering::SeqCst);
        configure.abort();
        assert!(configure.await.unwrap_err().is_cancelled());
        tokio::time::timeout(Duration::from_secs(2), rec.wait_for_remove_count(1))
            .await
            .expect("cancellation-owned cleanup must attempt provider removal");
        assert_eq!(be.cleanup_cell_count(), 1);

        rec.fail_remove.store(false, Ordering::SeqCst);
        be.release_session_checked(&sid).await.unwrap();

        assert_eq!(
            rec.remove_count.load(Ordering::SeqCst),
            2,
            "the retry must retain canonical source/path and retry provider removal"
        );
        assert!(be.map.lock().await.is_empty());
        assert_eq!(be.cleanup_cell_count(), 0);
        std::fs::remove_dir_all(tmp).unwrap();
    }

    #[tokio::test]
    async fn inner_configure_failure_retains_metadata_when_provider_cleanup_needs_retry() {
        let (be, rec, tmp, source, _cfg) = backend_fixture("inner-config-provider-retry");
        let sid = SessionId::parse("ctx-inner-config-provider-retry-g0").unwrap();
        rec.fail_configure.store(true, Ordering::SeqCst);
        rec.fail_remove.store(true, Ordering::SeqCst);

        assert_eq!(
            be.configure_session(&sid, &spec(Some(&source.to_string_lossy())))
                .await,
            Err(BridgeError::StoreFailure)
        );
        assert_eq!(rec.remove_count.load(Ordering::SeqCst), 1);
        assert_eq!(be.cleanup_cell_count(), 1);

        rec.fail_configure.store(false, Ordering::SeqCst);
        rec.fail_remove.store(false, Ordering::SeqCst);
        be.release_session_checked(&sid).await.unwrap();

        assert_eq!(rec.remove_count.load(Ordering::SeqCst), 2);
        assert_eq!(
            rec.order
                .lock()
                .unwrap()
                .iter()
                .filter(|step| step.as_str() == "inner_release")
                .count(),
            1,
            "retry must resume provider cleanup without repeating completed inner release"
        );
        assert_eq!(be.cleanup_cell_count(), 0);
        std::fs::remove_dir_all(tmp).unwrap();
    }

    #[tokio::test]
    async fn failed_configure_cleanup_has_owned_retry_and_blocks_new_allocation() {
        let (be, rec, tmp, source, _cfg) = backend_fixture("failed-config-owned-retry");
        let failed = SessionId::parse("ctx-failed-config-owned-retry-g0").unwrap();
        let distinct = SessionId::parse("ctx-failed-config-owned-retry-other-g0").unwrap();
        rec.fail_configure.store(true, Ordering::SeqCst);
        rec.fail_remove.store(true, Ordering::SeqCst);

        assert_eq!(
            be.configure_session(&failed, &spec(Some(&source.to_string_lossy())))
                .await,
            Err(BridgeError::StoreFailure)
        );
        assert_eq!(rec.add_count.load(Ordering::SeqCst), 1);
        assert_eq!(rec.remove_count.load(Ordering::SeqCst), 1);
        assert_eq!(be.cleanup_cell_count(), 1);

        assert_eq!(
            be.configure_session(&distinct, &spec(Some(&source.to_string_lossy())))
                .await,
            Err(BridgeError::AgentOverloaded),
            "degraded cleanup must reject a distinct allocation before provider add"
        );
        assert_eq!(rec.add_count.load(Ordering::SeqCst), 1);

        rec.fail_configure.store(false, Ordering::SeqCst);
        rec.fail_remove.store(false, Ordering::SeqCst);
        be.trigger_failed_configure_retry();
        tokio::time::timeout(Duration::from_secs(2), rec.wait_for_remove_count(2))
            .await
            .expect("the backend-owned retry must re-enter provider cleanup");
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                let changed = be.notify.notified();
                if be.cleanup_cell_count() == 0 {
                    break;
                }
                changed.await;
            }
        })
        .await
        .expect("successful backend-owned recovery must evict the failed cell");

        assert!(be.map.lock().await.is_empty());
        assert_eq!(be.cleanup_cell_count(), 0);
        assert_eq!(rec.remove_count.load(Ordering::SeqCst), 2);
        assert_eq!(
            rec.order
                .lock()
                .unwrap()
                .iter()
                .filter(|step| step.as_str() == "inner_release")
                .count(),
            1,
            "the backend-owned retry must not repeat completed inner release"
        );
        std::fs::remove_dir_all(tmp).unwrap();
    }

    #[tokio::test]
    async fn canceled_side_effecting_configure_retains_autonomous_cleanup_owner() {
        let (be, rec, tmp, source, _cfg) = backend_fixture("canceled-config-owned-retry");
        let canceled = SessionId::parse("ctx-canceled-config-owned-retry-g0").unwrap();
        let distinct = SessionId::parse("ctx-canceled-config-owned-retry-other-g0").unwrap();
        rec.fail_remove.store(true, Ordering::SeqCst);
        let _allow_configure = rec.block_next_configure();
        let configure_be = be.clone();
        let configure_session = canceled.clone();
        let session_spec = spec(Some(&source.to_string_lossy()));
        let configure = tokio::spawn(async move {
            configure_be
                .configure_session(&configure_session, &session_spec)
                .await
        });
        rec.wait_for_blocked_configure().await;

        configure.abort();
        assert!(configure.await.unwrap_err().is_cancelled());
        tokio::time::timeout(Duration::from_secs(2), rec.wait_for_remove_count(1))
            .await
            .expect("cancellation after allocation must start owned cleanup");
        assert_eq!(rec.add_count.load(Ordering::SeqCst), 1);
        assert_eq!(be.cleanup_cell_count(), 1);

        assert_eq!(
            be.configure_session(&distinct, &spec(Some(&source.to_string_lossy())))
                .await,
            Err(BridgeError::AgentOverloaded),
            "canceled side effects must degrade admission before another provider add"
        );
        assert_eq!(rec.add_count.load(Ordering::SeqCst), 1);

        rec.fail_remove.store(false, Ordering::SeqCst);
        be.trigger_failed_configure_retry();
        tokio::time::timeout(Duration::from_secs(2), rec.wait_for_remove_count(2))
            .await
            .expect("the cancellation-owned retry must resume provider cleanup");
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                let changed = be.notify.notified();
                if be.cleanup_cell_count() == 0 {
                    break;
                }
                changed.await;
            }
        })
        .await
        .expect("cancellation-owned recovery must evict the failed cell");

        assert!(be.map.lock().await.is_empty());
        assert_eq!(rec.remove_count.load(Ordering::SeqCst), 2);
        assert_eq!(
            rec.order
                .lock()
                .unwrap()
                .iter()
                .filter(|step| step.as_str() == "inner_release")
                .count(),
            1,
            "retry must not repeat the completed inner release"
        );
        std::fs::remove_dir_all(tmp).unwrap();
    }

    #[tokio::test]
    async fn forget_takeover_preserves_failed_release_strength() {
        let (be, rec, tmp, source, _cfg) = backend_fixture("forget-preserves-release");
        let sid = SessionId::parse("ctx-forget-preserves-release-g0").unwrap();
        rec.fail_configure.store(true, Ordering::SeqCst);
        rec.fail_release.store(true, Ordering::SeqCst);

        assert_eq!(
            be.configure_session(&sid, &spec(Some(&source.to_string_lossy())))
                .await,
            Err(BridgeError::StoreFailure)
        );
        assert_eq!(
            be.cleanup_flight_strength(&sid),
            Some(CleanupStrength::Release)
        );

        rec.fail_configure.store(false, Ordering::SeqCst);
        rec.fail_release.store(false, Ordering::SeqCst);
        be.forget_session_checked(&sid).await.unwrap();

        let order = rec.order.lock().unwrap();
        assert_eq!(
            order
                .iter()
                .filter(|step| step.as_str() == "inner_release")
                .count(),
            2,
            "failed Release must be retried at Release strength"
        );
        assert_eq!(
            order
                .iter()
                .filter(|step| step.as_str() == "inner_forget")
                .count(),
            0,
            "a weaker Forget takeover must not downgrade failed Release"
        );
        drop(order);
        assert_eq!(be.cleanup_cell_count(), 0);
        std::fs::remove_dir_all(tmp).unwrap();
    }

    #[tokio::test]
    async fn current_forget_cannot_finalize_failed_configure_marker() {
        let (be, rec, tmp, source, _cfg) = backend_fixture("current-forget-release-marker");
        let sid = SessionId::parse("ctx-current-forget-release-marker-g0").unwrap();
        let allow_configure = rec.block_next_configure();
        let configure_be = be.clone();
        let configure_sid = sid.clone();
        let session_spec = spec(Some(&source.to_string_lossy()));
        let configure = tokio::spawn(async move {
            configure_be
                .configure_session(&configure_sid, &session_spec)
                .await
        });
        rec.wait_for_blocked_configure().await;

        let forget_be = be.clone();
        let forget_sid = sid.clone();
        let forget =
            tokio::spawn(async move { forget_be.forget_session_checked(&forget_sid).await });
        be.wait_for_cleanup_waiting_reservation().await;
        assert_eq!(
            be.cleanup_flight_strength(&sid),
            Some(CleanupStrength::Forget),
            "the exact current reporter must still be Forget"
        );

        // Model the production handoff interval in ConfigureAdmission::drop:
        // the failed-configure marker is published under lifecycle before the
        // synchronous Release takeover acquires the flight slot.
        {
            let cells = be.cleanup_cells.lock().unwrap();
            let cell = cells.get(sid.as_str()).unwrap();
            cell.lifecycle
                .lock()
                .unwrap()
                .failed_configure_cleanup_pending = true;
        }
        allow_configure.send(()).unwrap();
        tokio::time::timeout(Duration::from_secs(2), configure)
            .await
            .expect("configure admission must settle after its gate opens")
            .unwrap()
            .unwrap();
        tokio::time::timeout(Duration::from_secs(2), forget)
            .await
            .expect("current Forget flight must report after configure settles")
            .unwrap()
            .unwrap();

        {
            let cells = be.cleanup_cells.lock().unwrap();
            let cell = cells
                .get(sid.as_str())
                .expect("Forget cannot evict a Release-required cleanup cell");
            assert!(
                cell.lifecycle
                    .lock()
                    .unwrap()
                    .failed_configure_cleanup_pending,
                "current Forget success cannot satisfy or clear a Release marker"
            );
        }
        assert_eq!(be.cleanup_cell_count(), 1);

        tokio::time::timeout(Duration::from_secs(2), be.release_session_checked(&sid))
            .await
            .expect("explicit Release must take over the retained marked cell")
            .unwrap();
        assert_eq!(be.cleanup_cell_count(), 0);
        std::fs::remove_dir_all(tmp).unwrap();
    }

    #[tokio::test]
    async fn ordinary_forget_evicts_marker_free_cleanup_cell() {
        let (be, rec, tmp, source, _cfg) = backend_fixture("ordinary-forget-evicts-cell");
        let sid = SessionId::parse("ctx-ordinary-forget-evicts-cell-g0").unwrap();
        be.configure_session(&sid, &spec(Some(&source.to_string_lossy())))
            .await
            .unwrap();
        assert_eq!(be.cleanup_cell_count(), 1);

        be.forget_session_checked(&sid).await.unwrap();

        assert_eq!(
            be.cleanup_cell_count(),
            0,
            "current marker-free Forget success must evict its cleanup cell"
        );
        assert!(be.map.lock().await.is_empty());
        assert_eq!(rec.remove_count.load(Ordering::SeqCst), 1);
        std::fs::remove_dir_all(tmp).unwrap();
    }

    #[tokio::test]
    async fn stale_pending_forget_cannot_clear_newer_failed_release_marker() {
        let (be, rec, tmp, source, _cfg) = backend_fixture("stale-forget-release-marker");
        let sid = SessionId::parse("ctx-stale-forget-release-marker-g0").unwrap();
        let distinct = SessionId::parse("ctx-stale-forget-release-marker-other-g0").unwrap();
        let _allow_configure = rec.block_next_configure();
        let configure_be = be.clone();
        let configure_sid = sid.clone();
        let session_spec = spec(Some(&source.to_string_lossy()));
        let configure = tokio::spawn(async move {
            configure_be
                .configure_session(&configure_sid, &session_spec)
                .await
        });
        rec.wait_for_blocked_configure().await;

        let forget_be = be.clone();
        let forget_sid = sid.clone();
        let forget =
            tokio::spawn(async move { forget_be.forget_session_checked(&forget_sid).await });
        be.wait_for_cleanup_waiting_reservation().await;
        assert_eq!(
            be.cleanup_flight_strength(&sid),
            Some(CleanupStrength::Forget)
        );

        rec.fail_release.store(true, Ordering::SeqCst);
        configure.abort();
        assert!(configure.await.unwrap_err().is_cancelled());
        assert_eq!(
            be.cleanup_flight_strength(&sid),
            Some(CleanupStrength::Release)
        );
        let release_report = be
            .cleanup_flight_report(&sid)
            .expect("destructor-owned Release publishes a report");

        forget.await.unwrap().unwrap();
        assert_eq!(
            wait_for_cleanup_report(release_report).await,
            Err(BridgeError::StoreFailure)
        );
        assert!(
            be.cleanup_cells
                .lock()
                .unwrap()
                .get(sid.as_str())
                .unwrap()
                .lifecycle
                .lock()
                .unwrap()
                .failed_configure_cleanup_pending,
            "a stale successful Forget must not clear failed Release ownership"
        );
        assert_eq!(
            be.configure_session(&distinct, &spec(Some(&source.to_string_lossy())))
                .await,
            Err(BridgeError::AgentOverloaded),
            "distinct allocation stays closed while Release remains failed"
        );
        assert_eq!(rec.add_count.load(Ordering::SeqCst), 1);

        rec.fail_release.store(false, Ordering::SeqCst);
        be.trigger_failed_configure_retry();
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                let changed = be.notify.notified();
                if be.cleanup_cell_count() == 0 {
                    break;
                }
                changed.await;
            }
        })
        .await
        .expect("automatic Release retry must clear the exact failed marker");

        {
            let order = rec.order.lock().unwrap();
            assert_eq!(
                order
                    .iter()
                    .filter(|step| step.as_str() == "inner_release")
                    .count(),
                2
            );
            assert_eq!(
                order
                    .iter()
                    .filter(|step| step.as_str() == "inner_forget")
                    .count(),
                1
            );
        }
        assert_eq!(rec.remove_count.load(Ordering::SeqCst), 1);
        assert!(be.map.lock().await.is_empty());
        std::fs::remove_dir_all(tmp).unwrap();
    }

    #[tokio::test]
    async fn configure_admission_capacity_bounds_the_orphan_producing_wave() {
        let (be, _rec, tmp, _source, _cfg) = backend_fixture("configure-admission-capacity");
        let mut admissions = Vec::new();
        for index in 0..MAX_WORKTREE_CONFIGURES_IN_FLIGHT {
            let session = SessionId::parse(format!("ctx-configure-capacity-{index}-g0")).unwrap();
            admissions.push(
                be.admit_configure(&session)
                    .expect("capacity admits the bounded prefix"),
            );
        }
        let rejected = SessionId::parse("ctx-configure-capacity-rejected-g0").unwrap();

        assert_eq!(
            be.admit_configure(&rejected).err().unwrap(),
            BridgeError::AgentOverloaded
        );
        assert_eq!(
            be.configure_inflight.load(Ordering::SeqCst),
            MAX_WORKTREE_CONFIGURES_IN_FLIGHT
        );

        drop(admissions);
        assert_eq!(be.configure_inflight.load(Ordering::SeqCst), 0);
        assert_eq!(be.cleanup_cell_count(), 0);
        std::fs::remove_dir_all(tmp).unwrap();
    }

    #[tokio::test]
    async fn sidecar_write_failure_retains_metadata_when_provider_cleanup_needs_retry() {
        let tmp = unique_temp_dir("sidecar-provider-retry");
        let allowed_root = tmp.join("allowed");
        let source = allowed_root.join("source");
        let worktree_root = tmp.join("worktrees");
        std::fs::create_dir_all(&source).unwrap();
        std::fs::create_dir_all(&worktree_root).unwrap();
        let canonical_allowed_root = std::fs::canonicalize(&allowed_root).unwrap();
        let canonical_worktree_root = std::fs::canonicalize(&worktree_root).unwrap();
        let rec = Arc::new(Rec::default());
        rec.fail_remove.store(true, Ordering::SeqCst);
        let be = WorktreeBackend::new(
            Arc::new(FakeInner { rec: rec.clone() }),
            Arc::new(SidecarWriteFailProv { rec: rec.clone() }),
            crate::provider_path::WorktreeConfig {
                root: canonical_worktree_root.to_string_lossy().into_owned(),
                owner: "ownr".into(),
                run: "run7".into(),
            },
            Some(SessionCwd::parse(&canonical_allowed_root.to_string_lossy()).unwrap()),
            identity(),
        );
        let sid = SessionId::parse("ctx-sidecar-provider-retry-g0").unwrap();

        assert_eq!(
            be.configure_session(&sid, &spec(Some(&source.to_string_lossy())))
                .await,
            Err(BridgeError::StoreFailure),
            "the provider-created temp directory must force sidecar write failure"
        );
        assert_eq!(rec.remove_count.load(Ordering::SeqCst), 1);
        assert_eq!(be.cleanup_cell_count(), 1);

        rec.fail_remove.store(false, Ordering::SeqCst);
        be.release_session_checked(&sid).await.unwrap();

        assert_eq!(rec.remove_count.load(Ordering::SeqCst), 2);
        assert_eq!(be.cleanup_cell_count(), 0);
        std::fs::remove_dir_all(tmp).unwrap();
    }

    #[tokio::test]
    async fn partial_provider_add_failure_retains_metadata_for_cleanup_retry() {
        let tmp = unique_temp_dir("partial-add-provider-retry");
        let allowed_root = tmp.join("allowed");
        let source = allowed_root.join("source");
        let worktree_root = tmp.join("worktrees");
        std::fs::create_dir_all(&source).unwrap();
        std::fs::create_dir_all(&worktree_root).unwrap();
        let canonical_allowed_root = std::fs::canonicalize(&allowed_root).unwrap();
        let canonical_worktree_root = std::fs::canonicalize(&worktree_root).unwrap();
        let rec = Arc::new(Rec::default());
        rec.fail_remove.store(true, Ordering::SeqCst);
        let be = WorktreeBackend::new(
            Arc::new(FakeInner { rec: rec.clone() }),
            Arc::new(PartialAddFailProv { rec: rec.clone() }),
            crate::provider_path::WorktreeConfig {
                root: canonical_worktree_root.to_string_lossy().into_owned(),
                owner: "ownr".into(),
                run: "run7".into(),
            },
            Some(SessionCwd::parse(&canonical_allowed_root.to_string_lossy()).unwrap()),
            identity(),
        );
        let sid = SessionId::parse("ctx-partial-add-provider-retry-g0").unwrap();

        assert_eq!(
            be.configure_session(&sid, &spec(Some(&source.to_string_lossy())))
                .await,
            Err(BridgeError::StoreFailure)
        );
        assert_eq!(rec.remove_count.load(Ordering::SeqCst), 1);
        assert_eq!(be.cleanup_cell_count(), 1);

        rec.fail_remove.store(false, Ordering::SeqCst);
        be.release_session_checked(&sid).await.unwrap();

        assert_eq!(rec.remove_count.load(Ordering::SeqCst), 2);
        assert_eq!(be.cleanup_cell_count(), 0);
        std::fs::remove_dir_all(tmp).unwrap();
    }

    #[tokio::test]
    async fn observed_start_persistence_failure_is_fatal_but_does_not_cancel_cleanup() {
        let (be, rec, tmp, source, _cfg) = backend_fixture("observer-start-failure");
        let sid = SessionId::parse("ctx-observer-start-failure-g0").unwrap();
        be.configure_session(&sid, &spec(Some(&source.to_string_lossy())))
            .await
            .unwrap();

        assert_eq!(
            be.release_session_observed(&sid, Arc::new(RejectingDiagnostic))
                .await,
            Err(BridgeError::StoreFailure)
        );
        assert!(
            be.map.lock().await.is_empty(),
            "observer persistence failure must not strand worktree metadata"
        );
        assert_eq!(rec.remove_count.load(Ordering::SeqCst), 1);
        assert_eq!(
            rec.order
                .lock()
                .unwrap()
                .iter()
                .filter(|step| step.as_str() == "inner_release")
                .count(),
            1
        );
        std::fs::remove_dir_all(tmp).unwrap();
    }

    #[tokio::test]
    async fn observed_cleanup_claims_flight_before_pending_started_observation() {
        let tmp = unique_temp_dir("observed-start-pending");
        let allowed_root = tmp.join("allowed");
        let source = allowed_root.join("source");
        let worktree_root = tmp.join("worktrees");
        std::fs::create_dir_all(&source).unwrap();
        std::fs::create_dir_all(&worktree_root).unwrap();
        let canonical_allowed_root = std::fs::canonicalize(&allowed_root).unwrap();
        let canonical_worktree_root = std::fs::canonicalize(&worktree_root).unwrap();
        let rec = Arc::new(Rec::default());
        let (allow_remove, remove_gate) = oneshot::channel();
        let provider = Arc::new(BlockingRemoveProv::new(rec.clone(), remove_gate));
        let be = Arc::new(WorktreeBackend::new(
            Arc::new(FakeInner { rec: rec.clone() }),
            provider.clone(),
            crate::provider_path::WorktreeConfig {
                root: canonical_worktree_root.to_string_lossy().into_owned(),
                owner: "ownr".into(),
                run: "run7".into(),
            },
            Some(SessionCwd::parse(&canonical_allowed_root.to_string_lossy()).unwrap()),
            identity(),
        ));
        let sid = SessionId::parse("ctx-observed-start-pending-g0").unwrap();
        be.configure_session(&sid, &spec(Some(&source.to_string_lossy())))
            .await
            .unwrap();

        let observer = Arc::new(PendingDiagnostic::default());
        let weak_observer = Arc::downgrade(&observer);
        let observed_be = be.clone();
        let observed_sid = sid.clone();
        let observed_observer = observer.clone();
        let observed = tokio::spawn(async move {
            observed_be
                .release_session_observed(&observed_sid, observed_observer)
                .await
        });
        observer.wait_until_entered().await;
        tokio::time::timeout(Duration::from_secs(2), provider.wait_for_remove())
            .await
            .expect("cleanup flight must be owned before the started observation awaits");

        observed.abort();
        assert!(observed.await.unwrap_err().is_cancelled());
        drop(observer);
        assert!(
            weak_observer.upgrade().is_none(),
            "observer-free cleanup must not retain the canceled operation observer"
        );
        allow_remove.send(()).unwrap();
        for _ in 0..100 {
            if be.cleanup_cell_count() == 0 && be.map.lock().await.is_empty() {
                break;
            }
            tokio::task::yield_now().await;
        }
        assert_eq!(be.cleanup_cell_count(), 0);
        assert!(be.map.lock().await.is_empty());
        assert_eq!(
            rec.order.lock().unwrap().as_slice(),
            ["inner_release", "wt_remove"]
        );
        std::fs::remove_dir_all(tmp).unwrap();
    }

    #[tokio::test]
    async fn checked_release_propagates_sidecar_failure_without_repeating_prior_components() {
        let (be, rec, tmp, source, _cfg) = backend_fixture("sidecar-retry");
        let sid = SessionId::parse("ctx-sidecar-retry-g0").unwrap();
        be.configure_session(&sid, &spec(Some(&source.to_string_lossy())))
            .await
            .unwrap();
        let worktree_path = match be.map.lock().await.get(sid.as_str()) {
            Some(WtState::Ready(entry)) => entry.worktree_path.clone(),
            _ => panic!("configured worktree is ready"),
        };
        let sidecar = sidecar_path(&worktree_path);
        std::fs::remove_file(&sidecar).unwrap();
        std::fs::create_dir(&sidecar).unwrap();

        let error = be.release_session_checked(&sid).await.unwrap_err();
        assert!(matches!(error, BridgeError::AgentCrashed { .. }));
        std::fs::remove_dir(&sidecar).unwrap();
        be.release_session_checked(&sid).await.unwrap();

        let order = rec.order.lock().unwrap().clone();
        assert_eq!(
            order
                .iter()
                .filter(|step| step.as_str() == "inner_release")
                .count(),
            1
        );
        assert_eq!(rec.remove_count.load(Ordering::SeqCst), 1);
        assert!(be.map.lock().await.is_empty());
        std::fs::remove_dir_all(tmp).unwrap();
    }

    #[tokio::test]
    async fn same_source_idempotent_rededelegates_diff_source_rejected_passthrough() {
        let (be, rec, tmp, source, _cfg) = backend_fixture("idempotent");
        let other = tmp.join("allowed").join("other");
        std::fs::create_dir_all(&other).unwrap();
        let sid = SessionId::parse("ctx-c1-g0").unwrap();

        be.configure_session(&sid, &spec(Some(&source.to_string_lossy())))
            .await
            .unwrap();
        be.configure_session(&sid, &spec(Some(&source.to_string_lossy())))
            .await
            .unwrap();

        assert_eq!(rec.add_count.load(Ordering::SeqCst), 1);
        assert_eq!(rec.configure_count.load(Ordering::SeqCst), 2);

        let err = be
            .configure_session(&sid, &spec(Some(&other.to_string_lossy())))
            .await
            .unwrap_err();
        assert_eq!(err, BridgeError::ConfigMismatch { field: "cwd" });

        let sid2 = SessionId::parse("ctx-c2-g0").unwrap();
        be.configure_session(&sid2, &spec(None)).await.unwrap();
        assert!(rec.configured_cwd.lock().unwrap().last().unwrap().is_none());
        assert_eq!(rec.add_count.load(Ordering::SeqCst), 1);

        std::fs::remove_dir_all(tmp).unwrap();
    }

    #[tokio::test]
    async fn retire_drains_map_removes_all_worktrees_idempotent() {
        let (be, rec, tmp, source1, _cfg) = backend_fixture("retire");
        let source2 = tmp.join("allowed").join("source2");
        std::fs::create_dir_all(&source2).unwrap();
        let sid1 = SessionId::parse("ctx-c1-g0").unwrap();
        let sid2 = SessionId::parse("ctx-c2-g0").unwrap();

        be.configure_session(&sid1, &spec(Some(&source1.to_string_lossy())))
            .await
            .unwrap();
        be.configure_session(&sid2, &spec(Some(&source2.to_string_lossy())))
            .await
            .unwrap();

        be.retire().await.unwrap();
        assert_eq!(rec.remove_count.load(Ordering::SeqCst), 2);
        assert!(be.map.lock().await.is_empty());

        be.retire().await.unwrap();
        assert_eq!(rec.remove_count.load(Ordering::SeqCst), 2);

        std::fs::remove_dir_all(tmp).unwrap();
    }

    #[tokio::test]
    async fn concurrent_configure_same_session_adds_once() {
        let (be, rec, tmp, source, _cfg) = backend_fixture("concurrent");
        let sid = SessionId::parse("ctx-c1-g0").unwrap();
        let spec = spec(Some(&source.to_string_lossy()));

        let (a, b) = tokio::join!(
            be.configure_session(&sid, &spec),
            be.configure_session(&sid, &spec)
        );

        a.unwrap();
        b.unwrap();
        assert_eq!(rec.add_count.load(Ordering::SeqCst), 1);

        std::fs::remove_dir_all(tmp).unwrap();
    }

    async fn assert_retirement_waits_for_passthrough_configure(non_git_cwd: bool) {
        let tmp = unique_temp_dir(if non_git_cwd {
            "retire-non-git-configure"
        } else {
            "retire-no-cwd-configure"
        });
        let allowed_root = tmp.join("allowed");
        let source = allowed_root.join("source");
        let worktree_root = tmp.join("worktrees");
        std::fs::create_dir_all(&source).unwrap();
        std::fs::create_dir_all(&worktree_root).unwrap();
        let canonical_allowed_root = std::fs::canonicalize(&allowed_root).unwrap();
        let canonical_source = std::fs::canonicalize(&source).unwrap();
        let canonical_worktree_root = std::fs::canonicalize(&worktree_root).unwrap();
        let rec = Arc::new(Rec::default());
        let provider: Arc<dyn crate::provider::WorktreeProvider> = if non_git_cwd {
            Arc::new(NonGitProv { rec: rec.clone() })
        } else {
            Arc::new(FakeProv { rec: rec.clone() })
        };
        let be = Arc::new(WorktreeBackend::new(
            Arc::new(FakeInner { rec: rec.clone() }),
            provider,
            crate::provider_path::WorktreeConfig {
                root: canonical_worktree_root.to_string_lossy().into_owned(),
                owner: "ownr".into(),
                run: "run7".into(),
            },
            Some(SessionCwd::parse(&canonical_allowed_root.to_string_lossy()).unwrap()),
            identity(),
        ));
        let session = SessionId::parse(if non_git_cwd {
            "ctx-retire-non-git-g0"
        } else {
            "ctx-retire-no-cwd-g0"
        })
        .unwrap();
        let session_spec = if non_git_cwd {
            spec(Some(&canonical_source.to_string_lossy()))
        } else {
            spec(None)
        };
        let allow_configure = rec.block_next_configure();
        let configure_be = be.clone();
        let configure_session = session.clone();
        let configure = tokio::spawn(async move {
            configure_be
                .configure_session(&configure_session, &session_spec)
                .await
        });
        rec.wait_for_blocked_configure().await;
        assert_eq!(be.configure_inflight.load(Ordering::SeqCst), 1);

        let retire_be = be.clone();
        let retire = tokio::spawn(async move { retire_be.retire().await });
        while !be.sealed.load(Ordering::SeqCst) {
            tokio::task::yield_now().await;
        }
        assert_eq!(
            rec.retire_count.load(Ordering::SeqCst),
            0,
            "retirement must not pass an admitted pass-through configure"
        );

        let rejected = be
            .configure_session(
                &SessionId::parse("ctx-retire-after-seal-g0").unwrap(),
                &spec(None),
            )
            .await;
        assert_eq!(rejected, Err(BridgeError::SessionExpired));
        assert_eq!(
            rec.configure_count.load(Ordering::SeqCst),
            1,
            "post-seal configure must not reach the inner backend"
        );

        allow_configure.send(()).unwrap();
        tokio::time::timeout(Duration::from_secs(2), configure)
            .await
            .expect("admitted configure must settle")
            .unwrap()
            .unwrap();
        tokio::time::timeout(Duration::from_secs(2), retire)
            .await
            .expect("retirement must resume after configure settles")
            .unwrap()
            .unwrap();

        assert_eq!(be.configure_inflight.load(Ordering::SeqCst), 0);
        assert_eq!(rec.retire_count.load(Ordering::SeqCst), 1);
        assert_eq!(rec.add_count.load(Ordering::SeqCst), 0);
        assert_eq!(rec.remove_count.load(Ordering::SeqCst), 0);
        std::fs::remove_dir_all(tmp).unwrap();
    }

    #[tokio::test]
    async fn retirement_waits_for_admitted_configure_without_cwd() {
        assert_retirement_waits_for_passthrough_configure(false).await;
    }

    #[tokio::test]
    async fn retirement_waits_for_admitted_non_git_configure() {
        assert_retirement_waits_for_passthrough_configure(true).await;
    }

    async fn assert_release_waits_for_passthrough_configure(non_git_cwd: bool) {
        let tmp = unique_temp_dir(if non_git_cwd {
            "release-non-git-configure"
        } else {
            "release-no-cwd-configure"
        });
        let allowed_root = tmp.join("allowed");
        let source = allowed_root.join("source");
        let worktree_root = tmp.join("worktrees");
        std::fs::create_dir_all(&source).unwrap();
        std::fs::create_dir_all(&worktree_root).unwrap();
        let canonical_allowed_root = std::fs::canonicalize(&allowed_root).unwrap();
        let canonical_source = std::fs::canonicalize(&source).unwrap();
        let canonical_worktree_root = std::fs::canonicalize(&worktree_root).unwrap();
        let rec = Arc::new(Rec::default());
        let provider: Arc<dyn crate::provider::WorktreeProvider> = if non_git_cwd {
            Arc::new(NonGitProv { rec: rec.clone() })
        } else {
            Arc::new(FakeProv { rec: rec.clone() })
        };
        let be = Arc::new(WorktreeBackend::new(
            Arc::new(FakeInner { rec: rec.clone() }),
            provider,
            crate::provider_path::WorktreeConfig {
                root: canonical_worktree_root.to_string_lossy().into_owned(),
                owner: "ownr".into(),
                run: "run7".into(),
            },
            Some(SessionCwd::parse(&canonical_allowed_root.to_string_lossy()).unwrap()),
            identity(),
        ));
        let session = SessionId::parse(if non_git_cwd {
            "ctx-release-non-git-g0"
        } else {
            "ctx-release-no-cwd-g0"
        })
        .unwrap();
        let session_spec = if non_git_cwd {
            spec(Some(&canonical_source.to_string_lossy()))
        } else {
            spec(None)
        };
        let allow_configure = rec.block_next_configure();
        let configure_be = be.clone();
        let configure_session = session.clone();
        let configure = tokio::spawn(async move {
            configure_be
                .configure_session(&configure_session, &session_spec)
                .await
        });
        rec.wait_for_blocked_configure().await;

        let release_be = be.clone();
        let release_session = session.clone();
        let release =
            tokio::spawn(async move { release_be.release_session_checked(&release_session).await });
        be.wait_for_cleanup_flight_started().await;
        assert!(
            rec.order.lock().unwrap().is_empty(),
            "release must not pass the admitted pass-through configure"
        );

        allow_configure.send(()).unwrap();
        configure.await.unwrap().unwrap();
        release.await.unwrap().unwrap();
        assert_eq!(rec.order.lock().unwrap().as_slice(), ["inner_release"]);
        assert_eq!(rec.add_count.load(Ordering::SeqCst), 0);
        assert_eq!(rec.remove_count.load(Ordering::SeqCst), 0);
        assert_eq!(be.cleanup_cell_count(), 0);
        std::fs::remove_dir_all(tmp).unwrap();
    }

    #[tokio::test]
    async fn release_waits_for_admitted_configure_without_cwd() {
        assert_release_waits_for_passthrough_configure(false).await;
    }

    #[tokio::test]
    async fn release_waits_for_admitted_non_git_configure() {
        assert_release_waits_for_passthrough_configure(true).await;
    }

    struct BlockingProv {
        rec: Arc<Rec>,
        add_entered: Arc<Notify>,
        allow_add: Arc<Notify>,
    }

    struct BlockingProbeProv {
        rec: Arc<Rec>,
        gate: Mutex<Option<oneshot::Receiver<()>>>,
        probe_started_count: AtomicUsize,
        probe_started: Notify,
    }

    impl BlockingProbeProv {
        fn new(rec: Arc<Rec>, gate: oneshot::Receiver<()>) -> Self {
            Self {
                rec,
                gate: Mutex::new(Some(gate)),
                probe_started_count: AtomicUsize::new(0),
                probe_started: Notify::new(),
            }
        }

        async fn wait_for_probe(&self) {
            while self.probe_started_count.load(Ordering::SeqCst) == 0 {
                let started = self.probe_started.notified();
                if self.probe_started_count.load(Ordering::SeqCst) == 0 {
                    started.await;
                }
            }
        }
    }

    struct BlockingRemoveProv {
        rec: Arc<Rec>,
        gate: Mutex<Option<oneshot::Receiver<()>>>,
        remove_started: Notify,
        remove_started_count: AtomicUsize,
        fail_first: AtomicBool,
    }

    impl BlockingRemoveProv {
        fn new(rec: Arc<Rec>, gate: oneshot::Receiver<()>) -> Self {
            Self {
                rec,
                gate: Mutex::new(Some(gate)),
                remove_started: Notify::new(),
                remove_started_count: AtomicUsize::new(0),
                fail_first: AtomicBool::new(false),
            }
        }

        fn new_failing_once(rec: Arc<Rec>, gate: oneshot::Receiver<()>) -> Self {
            let provider = Self::new(rec, gate);
            provider.fail_first.store(true, Ordering::SeqCst);
            provider
        }

        async fn wait_for_remove(&self) {
            while self.remove_started_count.load(Ordering::SeqCst) == 0 {
                self.remove_started.notified().await;
            }
        }
    }

    #[async_trait::async_trait]
    impl crate::provider::WorktreeProvider for BlockingRemoveProv {
        async fn add(&self, _repo: &str, _worktree_path: &str) -> Result<String, BridgeError> {
            self.rec.add_count.fetch_add(1, Ordering::SeqCst);
            Ok(String::new())
        }

        async fn remove(&self, _repo: &str, _worktree_path: &str) -> Result<(), BridgeError> {
            self.rec.remove_count.fetch_add(1, Ordering::SeqCst);
            self.rec.order.lock().unwrap().push("wt_remove".into());
            self.remove_started_count.fetch_add(1, Ordering::SeqCst);
            self.remove_started.notify_waiters();
            let gate = self.gate.lock().unwrap().take();
            if let Some(gate) = gate {
                let _ = gate.await;
            }
            if self.fail_first.swap(false, Ordering::SeqCst) {
                Err(BridgeError::StoreFailure)
            } else {
                Ok(())
            }
        }

        async fn is_git_repo(&self, _path: &str) -> bool {
            true
        }
    }

    #[async_trait::async_trait]
    impl crate::provider::WorktreeProvider for BlockingProv {
        async fn add(&self, _repo: &str, _worktree_path: &str) -> Result<String, BridgeError> {
            self.rec.add_count.fetch_add(1, Ordering::SeqCst);
            self.add_entered.notify_one();
            self.allow_add.notified().await;
            Ok(String::new())
        }

        async fn remove(&self, _repo: &str, _worktree_path: &str) -> Result<(), BridgeError> {
            self.rec.remove_count.fetch_add(1, Ordering::SeqCst);
            self.rec.order.lock().unwrap().push("wt_remove".into());
            Ok(())
        }

        async fn is_git_repo(&self, _path: &str) -> bool {
            true
        }
    }

    #[async_trait::async_trait]
    impl crate::provider::WorktreeProvider for BlockingProbeProv {
        async fn add(&self, _repo: &str, _worktree_path: &str) -> Result<String, BridgeError> {
            self.rec.add_count.fetch_add(1, Ordering::SeqCst);
            Ok(String::new())
        }

        async fn remove(&self, _repo: &str, _worktree_path: &str) -> Result<(), BridgeError> {
            self.rec.remove_count.fetch_add(1, Ordering::SeqCst);
            self.rec.order.lock().unwrap().push("wt_remove".into());
            Ok(())
        }

        async fn is_git_repo(&self, _path: &str) -> bool {
            self.probe_started_count.fetch_add(1, Ordering::SeqCst);
            self.probe_started.notify_waiters();
            let gate = self
                .gate
                .lock()
                .unwrap()
                .take()
                .expect("probe is entered once");
            let _ = gate.await;
            true
        }
    }

    #[tokio::test]
    async fn release_claimed_during_git_probe_cleans_the_admitted_configure() {
        let tmp = unique_temp_dir("release-during-git-probe");
        let allowed_root = tmp.join("allowed");
        let source = allowed_root.join("source");
        let worktree_root = tmp.join("worktrees");
        std::fs::create_dir_all(&source).unwrap();
        std::fs::create_dir_all(&worktree_root).unwrap();
        let canonical_allowed_root = std::fs::canonicalize(&allowed_root).unwrap();
        let canonical_worktree_root = std::fs::canonicalize(&worktree_root).unwrap();
        let rec = Arc::new(Rec::default());
        let (allow_probe, probe_gate) = oneshot::channel();
        let provider = Arc::new(BlockingProbeProv::new(rec.clone(), probe_gate));
        let be = Arc::new(WorktreeBackend::new(
            Arc::new(FakeInner { rec: rec.clone() }),
            provider.clone(),
            crate::provider_path::WorktreeConfig {
                root: canonical_worktree_root.to_string_lossy().into_owned(),
                owner: "ownr".into(),
                run: "run7".into(),
            },
            Some(SessionCwd::parse(&canonical_allowed_root.to_string_lossy()).unwrap()),
            identity(),
        ));
        let sid = SessionId::parse("ctx-release-during-probe-g0").unwrap();
        let session_spec = spec(Some(&source.to_string_lossy()));
        let configure_be = be.clone();
        let configure_sid = sid.clone();
        let configure = tokio::spawn(async move {
            configure_be
                .configure_session(&configure_sid, &session_spec)
                .await
        });
        provider.wait_for_probe().await;

        let release_be = be.clone();
        let release_sid = sid.clone();
        let release =
            tokio::spawn(async move { release_be.release_session_checked(&release_sid).await });
        be.wait_for_cleanup_flight_started().await;
        assert!(
            rec.order.lock().unwrap().is_empty(),
            "cleanup must wait for the admitted configure before releasing inner state"
        );

        allow_probe.send(()).unwrap();
        configure.await.unwrap().unwrap();
        release.await.unwrap().unwrap();
        assert!(be.map.lock().await.is_empty());
        assert_eq!(rec.remove_count.load(Ordering::SeqCst), 1);
        assert_eq!(
            rec.order.lock().unwrap().as_slice(),
            ["inner_release", "wt_remove"]
        );
        std::fs::remove_dir_all(tmp).unwrap();
    }

    #[tokio::test]
    async fn teardown_during_reserving_does_not_leak() {
        let tmp = unique_temp_dir("teardown-reserving");
        let allowed_root = tmp.join("allowed");
        let source = allowed_root.join("source");
        let worktree_root = tmp.join("worktrees");
        std::fs::create_dir_all(&source).unwrap();
        std::fs::create_dir_all(&worktree_root).unwrap();
        let canonical_allowed_root = std::fs::canonicalize(&allowed_root).unwrap();
        let canonical_worktree_root = std::fs::canonicalize(&worktree_root).unwrap();
        let rec = Arc::new(Rec::default());
        let add_entered = Arc::new(Notify::new());
        let allow_add = Arc::new(Notify::new());
        let be = Arc::new(WorktreeBackend::new(
            Arc::new(FakeInner { rec: rec.clone() }),
            Arc::new(BlockingProv {
                rec: rec.clone(),
                add_entered: add_entered.clone(),
                allow_add: allow_add.clone(),
            }),
            crate::provider_path::WorktreeConfig {
                root: canonical_worktree_root.to_string_lossy().into_owned(),
                owner: "ownr".into(),
                run: "run7".into(),
            },
            Some(SessionCwd::parse(&canonical_allowed_root.to_string_lossy()).unwrap()),
            identity(),
        ));
        let sid = SessionId::parse("ctx-c1-g0").unwrap();
        let session_spec = spec(Some(&source.to_string_lossy()));
        let task_be = be.clone();
        let task_sid = sid.clone();
        let configure =
            tokio::spawn(async move { task_be.configure_session(&task_sid, &session_spec).await });

        add_entered.notified().await;
        let release_be = be.clone();
        let release_sid = sid.clone();
        let release = tokio::spawn(async move {
            release_be.release_session(&release_sid).await;
        });
        be.wait_for_cleanup_waiting_reservation().await;
        assert!(
            rec.order.lock().unwrap().is_empty(),
            "cleanup must wait for the configuring reservation before releasing inner state"
        );
        allow_add.notify_one();
        configure.await.unwrap().unwrap();
        release.await.unwrap();

        assert!(be.map.lock().await.is_empty());
        assert_eq!(rec.add_count.load(Ordering::SeqCst), 1);
        assert_eq!(rec.remove_count.load(Ordering::SeqCst), 1);

        std::fs::remove_dir_all(tmp).unwrap();
    }

    #[tokio::test]
    async fn canceled_configure_reservation_is_owned_by_started_cleanup() {
        let tmp = unique_temp_dir("cancel-configure-reserving");
        let allowed_root = tmp.join("allowed");
        let source = allowed_root.join("source");
        let worktree_root = tmp.join("worktrees");
        std::fs::create_dir_all(&source).unwrap();
        std::fs::create_dir_all(&worktree_root).unwrap();
        let canonical_allowed_root = std::fs::canonicalize(&allowed_root).unwrap();
        let canonical_worktree_root = std::fs::canonicalize(&worktree_root).unwrap();
        let rec = Arc::new(Rec::default());
        let add_entered = Arc::new(Notify::new());
        let allow_add = Arc::new(Notify::new());
        let be = Arc::new(WorktreeBackend::new(
            Arc::new(FakeInner { rec: rec.clone() }),
            Arc::new(BlockingProv {
                rec: rec.clone(),
                add_entered: add_entered.clone(),
                allow_add,
            }),
            crate::provider_path::WorktreeConfig {
                root: canonical_worktree_root.to_string_lossy().into_owned(),
                owner: "ownr".into(),
                run: "run7".into(),
            },
            Some(SessionCwd::parse(&canonical_allowed_root.to_string_lossy()).unwrap()),
            identity(),
        ));
        let sid = SessionId::parse("ctx-cancel-configure-reserving-g0").unwrap();
        let session_spec = spec(Some(&source.to_string_lossy()));
        let configure_be = be.clone();
        let configure_sid = sid.clone();
        let configure = tokio::spawn(async move {
            configure_be
                .configure_session(&configure_sid, &session_spec)
                .await
        });
        add_entered.notified().await;

        let release_be = be.clone();
        let release_sid = sid.clone();
        let release =
            tokio::spawn(async move { release_be.release_session_checked(&release_sid).await });
        be.wait_for_cleanup_waiting_reservation().await;
        configure.abort();
        assert!(configure.await.unwrap_err().is_cancelled());

        tokio::time::timeout(Duration::from_secs(2), release)
            .await
            .expect("cleanup must take over an ownerless reservation")
            .unwrap()
            .unwrap();
        assert!(be.map.lock().await.is_empty());
        assert_eq!(rec.remove_count.load(Ordering::SeqCst), 1);
        assert_eq!(
            rec.order.lock().unwrap().as_slice(),
            ["inner_release", "wt_remove"]
        );
        std::fs::remove_dir_all(tmp).unwrap();
    }

    #[tokio::test]
    async fn concurrent_configure_takes_over_when_reservation_owner_is_canceled() {
        let tmp = unique_temp_dir("concurrent-cancel-configure-reserving");
        let allowed_root = tmp.join("allowed");
        let source = allowed_root.join("source");
        let worktree_root = tmp.join("worktrees");
        std::fs::create_dir_all(&source).unwrap();
        std::fs::create_dir_all(&worktree_root).unwrap();
        let canonical_allowed_root = std::fs::canonicalize(&allowed_root).unwrap();
        let canonical_worktree_root = std::fs::canonicalize(&worktree_root).unwrap();
        let rec = Arc::new(Rec::default());
        let add_entered = Arc::new(Notify::new());
        let be = Arc::new(WorktreeBackend::new(
            Arc::new(FakeInner { rec: rec.clone() }),
            Arc::new(BlockingProv {
                rec: rec.clone(),
                add_entered: add_entered.clone(),
                allow_add: Arc::new(Notify::new()),
            }),
            crate::provider_path::WorktreeConfig {
                root: canonical_worktree_root.to_string_lossy().into_owned(),
                owner: "ownr".into(),
                run: "run7".into(),
            },
            Some(SessionCwd::parse(&canonical_allowed_root.to_string_lossy()).unwrap()),
            identity(),
        ));
        let sid = SessionId::parse("ctx-concurrent-cancel-configure-reserving-g0").unwrap();

        let first_be = be.clone();
        let first_sid = sid.clone();
        let first_spec = spec(Some(&source.to_string_lossy()));
        let first =
            tokio::spawn(async move { first_be.configure_session(&first_sid, &first_spec).await });
        add_entered.notified().await;

        let second_be = be.clone();
        let second_sid = sid.clone();
        let second_spec = spec(Some(&source.to_string_lossy()));
        let second =
            tokio::spawn(
                async move { second_be.configure_session(&second_sid, &second_spec).await },
            );
        be.wait_for_configure_inflight(2).await;
        first.abort();
        assert!(first.await.unwrap_err().is_cancelled());

        assert_eq!(
            tokio::time::timeout(Duration::from_secs(2), second)
                .await
                .expect("the peer configure must observe its canceled reservation owner")
                .unwrap(),
            Err(BridgeError::SessionExpired)
        );
        assert!(be.map.lock().await.is_empty());
        assert_eq!(rec.remove_count.load(Ordering::SeqCst), 1);
        assert_eq!(
            rec.order.lock().unwrap().as_slice(),
            ["inner_release", "wt_remove"]
        );
        std::fs::remove_dir_all(tmp).unwrap();
    }

    #[tokio::test]
    async fn retirement_seals_during_reservation_then_joins_published_cleanup_cell() {
        let tmp = unique_temp_dir("retire-during-reserving");
        let allowed_root = tmp.join("allowed");
        let source = allowed_root.join("source");
        let worktree_root = tmp.join("worktrees");
        std::fs::create_dir_all(&source).unwrap();
        std::fs::create_dir_all(&worktree_root).unwrap();
        let canonical_allowed_root = std::fs::canonicalize(&allowed_root).unwrap();
        let canonical_worktree_root = std::fs::canonicalize(&worktree_root).unwrap();
        let rec = Arc::new(Rec::default());
        let add_entered = Arc::new(Notify::new());
        let allow_add = Arc::new(Notify::new());
        let be = Arc::new(WorktreeBackend::new(
            Arc::new(FakeInner { rec: rec.clone() }),
            Arc::new(BlockingProv {
                rec: rec.clone(),
                add_entered: add_entered.clone(),
                allow_add: allow_add.clone(),
            }),
            crate::provider_path::WorktreeConfig {
                root: canonical_worktree_root.to_string_lossy().into_owned(),
                owner: "ownr".into(),
                run: "run7".into(),
            },
            Some(SessionCwd::parse(&canonical_allowed_root.to_string_lossy()).unwrap()),
            identity(),
        ));
        let sid = SessionId::parse("ctx-retire-reserving-g0").unwrap();
        let session_spec = spec(Some(&source.to_string_lossy()));
        let configure_be = be.clone();
        let configure_sid = sid.clone();
        let configure = tokio::spawn(async move {
            configure_be
                .configure_session(&configure_sid, &session_spec)
                .await
        });
        add_entered.notified().await;

        let retire_be = be.clone();
        let retire = tokio::spawn(async move { retire_be.retire().await });
        while !be.sealed.load(Ordering::SeqCst) {
            tokio::task::yield_now().await;
        }
        assert_eq!(
            be.configure_session(
                &SessionId::parse("ctx-after-seal-g0").unwrap(),
                &spec(Some(&source.to_string_lossy())),
            )
            .await,
            Err(BridgeError::SessionExpired)
        );

        allow_add.notify_one();
        assert_eq!(configure.await.unwrap(), Err(BridgeError::SessionExpired));
        retire.await.unwrap().unwrap();
        assert!(be.map.lock().await.is_empty());
        assert_eq!(rec.add_count.load(Ordering::SeqCst), 1);
        assert_eq!(rec.remove_count.load(Ordering::SeqCst), 1);
        assert_eq!(rec.retire_count.load(Ordering::SeqCst), 1);
        std::fs::remove_dir_all(tmp).unwrap();
    }

    #[tokio::test]
    async fn retirement_takes_over_a_canceled_configure_reservation() {
        let tmp = unique_temp_dir("retire-canceled-reserving");
        let allowed_root = tmp.join("allowed");
        let source = allowed_root.join("source");
        let worktree_root = tmp.join("worktrees");
        std::fs::create_dir_all(&source).unwrap();
        std::fs::create_dir_all(&worktree_root).unwrap();
        let canonical_allowed_root = std::fs::canonicalize(&allowed_root).unwrap();
        let canonical_worktree_root = std::fs::canonicalize(&worktree_root).unwrap();
        let rec = Arc::new(Rec::default());
        let add_entered = Arc::new(Notify::new());
        let be = Arc::new(WorktreeBackend::new(
            Arc::new(FakeInner { rec: rec.clone() }),
            Arc::new(BlockingProv {
                rec: rec.clone(),
                add_entered: add_entered.clone(),
                allow_add: Arc::new(Notify::new()),
            }),
            crate::provider_path::WorktreeConfig {
                root: canonical_worktree_root.to_string_lossy().into_owned(),
                owner: "ownr".into(),
                run: "run7".into(),
            },
            Some(SessionCwd::parse(&canonical_allowed_root.to_string_lossy()).unwrap()),
            identity(),
        ));
        let sid = SessionId::parse("ctx-retire-canceled-reserving-g0").unwrap();
        let session_spec = spec(Some(&source.to_string_lossy()));
        let configure_be = be.clone();
        let configure_sid = sid.clone();
        let configure = tokio::spawn(async move {
            configure_be
                .configure_session(&configure_sid, &session_spec)
                .await
        });
        add_entered.notified().await;

        let retire_be = be.clone();
        let retire = tokio::spawn(async move { retire_be.retire().await });
        while !be.sealed.load(Ordering::SeqCst) {
            tokio::task::yield_now().await;
        }
        configure.abort();
        assert!(configure.await.unwrap_err().is_cancelled());

        tokio::time::timeout(Duration::from_secs(2), retire)
            .await
            .expect("retirement must take over the ownerless reservation")
            .unwrap()
            .unwrap();
        assert!(be.map.lock().await.is_empty());
        assert_eq!(rec.remove_count.load(Ordering::SeqCst), 1);
        assert_eq!(
            rec.order.lock().unwrap().as_slice(),
            ["inner_release", "wt_remove", "inner_retire"]
        );
        std::fs::remove_dir_all(tmp).unwrap();
    }

    #[tokio::test]
    async fn forced_retire_joins_inflight_release_before_retiring_inner_backend() {
        let tmp = unique_temp_dir("release-retire-single-flight");
        let allowed_root = tmp.join("allowed");
        let source = allowed_root.join("source");
        let worktree_root = tmp.join("worktrees");
        std::fs::create_dir_all(&source).unwrap();
        std::fs::create_dir_all(&worktree_root).unwrap();
        let canonical_allowed_root = std::fs::canonicalize(&allowed_root).unwrap();
        let canonical_worktree_root = std::fs::canonicalize(&worktree_root).unwrap();
        let rec = Arc::new(Rec::default());
        let (allow_remove, remove_gate) = oneshot::channel();
        let provider = Arc::new(BlockingRemoveProv::new(rec.clone(), remove_gate));
        let be = Arc::new(WorktreeBackend::new(
            Arc::new(FakeInner { rec: rec.clone() }),
            provider.clone(),
            crate::provider_path::WorktreeConfig {
                root: canonical_worktree_root.to_string_lossy().into_owned(),
                owner: "ownr".into(),
                run: "run7".into(),
            },
            Some(SessionCwd::parse(&canonical_allowed_root.to_string_lossy()).unwrap()),
            identity(),
        ));
        let sid = SessionId::parse("ctx-release-retire-g0").unwrap();
        be.configure_session(&sid, &spec(Some(&source.to_string_lossy())))
            .await
            .unwrap();

        let release_be = be.clone();
        let release_sid = sid.clone();
        let release =
            tokio::spawn(async move { release_be.release_session_checked(&release_sid).await });
        provider.wait_for_remove().await;

        let retire_be = be.clone();
        let retire = tokio::spawn(async move { retire_be.retire().await });
        be.wait_for_retirement_joined_cell().await;
        assert_eq!(
            rec.retire_count.load(Ordering::SeqCst),
            0,
            "inner retirement must wait for the per-session cleanup cell"
        );

        allow_remove.send(()).unwrap();
        release.await.unwrap().unwrap();
        retire.await.unwrap().unwrap();
        assert_eq!(rec.remove_count.load(Ordering::SeqCst), 1);
        assert_eq!(
            rec.order.lock().unwrap().as_slice(),
            ["inner_release", "wt_remove", "inner_retire"]
        );
        std::fs::remove_dir_all(tmp).unwrap();
    }

    #[tokio::test]
    async fn configure_rejected_after_cleanup_started_keeps_global_admission_count_balanced() {
        let tmp = unique_temp_dir("rejected-configure-count");
        let allowed_root = tmp.join("allowed");
        let source = allowed_root.join("source");
        let worktree_root = tmp.join("worktrees");
        std::fs::create_dir_all(&source).unwrap();
        std::fs::create_dir_all(&worktree_root).unwrap();
        let canonical_allowed_root = std::fs::canonicalize(&allowed_root).unwrap();
        let canonical_worktree_root = std::fs::canonicalize(&worktree_root).unwrap();
        let rec = Arc::new(Rec::default());
        let (allow_remove, remove_gate) = oneshot::channel();
        let provider = Arc::new(BlockingRemoveProv::new(rec.clone(), remove_gate));
        let be = Arc::new(WorktreeBackend::new(
            Arc::new(FakeInner { rec: rec.clone() }),
            provider.clone(),
            crate::provider_path::WorktreeConfig {
                root: canonical_worktree_root.to_string_lossy().into_owned(),
                owner: "ownr".into(),
                run: "run7".into(),
            },
            Some(SessionCwd::parse(&canonical_allowed_root.to_string_lossy()).unwrap()),
            identity(),
        ));
        let sid = SessionId::parse("ctx-rejected-configure-count-g0").unwrap();
        let session_spec = spec(Some(&source.to_string_lossy()));
        be.configure_session(&sid, &session_spec).await.unwrap();

        let release_be = be.clone();
        let release_sid = sid.clone();
        let release =
            tokio::spawn(async move { release_be.release_session_checked(&release_sid).await });
        provider.wait_for_remove().await;

        assert_eq!(
            be.configure_session(&sid, &session_spec).await,
            Err(BridgeError::SessionExpired)
        );
        assert_eq!(
            be.configure_inflight.load(Ordering::SeqCst),
            0,
            "a rejected admission must not decrement a counter it never incremented"
        );

        let retire_be = be.clone();
        let retire = tokio::spawn(async move { retire_be.retire().await });
        assert_eq!(rec.retire_count.load(Ordering::SeqCst), 0);
        allow_remove.send(()).unwrap();
        release.await.unwrap().unwrap();
        tokio::time::timeout(Duration::from_secs(2), retire)
            .await
            .expect("balanced admission count must let retirement reach cleanup")
            .unwrap()
            .unwrap();
        assert_eq!(rec.retire_count.load(Ordering::SeqCst), 1);
        std::fs::remove_dir_all(tmp).unwrap();
    }

    #[tokio::test]
    async fn release_after_retirement_cleanup_joins_completed_sealed_cell() {
        let (be, rec, tmp, source, _cfg) = backend_fixture("retire-first-late-release");
        let sid = SessionId::parse("ctx-retire-first-late-release-g0").unwrap();
        be.configure_session(&sid, &spec(Some(&source.to_string_lossy())))
            .await
            .unwrap();
        let allow_retire = rec.block_next_retire();

        let retire_be = be.clone();
        let retire = tokio::spawn(async move { retire_be.retire().await });
        rec.wait_for_blocked_retire().await;
        assert_eq!(
            rec.order
                .lock()
                .unwrap()
                .iter()
                .filter(|step| step.as_str() == "inner_release")
                .count(),
            1,
            "retirement's per-session cleanup completes before inner retirement"
        );

        be.release_session_checked(&sid).await.unwrap();
        assert_eq!(
            rec.order
                .lock()
                .unwrap()
                .iter()
                .filter(|step| step.as_str() == "inner_release")
                .count(),
            1,
            "a late warm owner must join retirement's completed sealed cell"
        );

        allow_retire.send(()).unwrap();
        retire.await.unwrap().unwrap();
        assert_eq!(
            rec.order.lock().unwrap().as_slice(),
            ["inner_release", "wt_remove", "inner_retire"]
        );
        std::fs::remove_dir_all(tmp).unwrap();
    }

    #[tokio::test]
    async fn known_release_after_seal_before_retirement_snapshot_joins_retained_cell() {
        let (be, rec, tmp, source, _cfg) = backend_fixture("seal-before-snapshot");
        let sid = SessionId::parse("ctx-seal-before-snapshot-g0").unwrap();
        be.configure_session(&sid, &spec(Some(&source.to_string_lossy())))
            .await
            .unwrap();

        // Reproduce retire's seal publication boundary without letting its
        // subsequent map snapshot run. Admission, sealing, and reporter
        // eviction must all use this same map lock in production.
        {
            let _cells = be
                .cleanup_cells
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            be.sealed.store(true, Ordering::SeqCst);
        }
        rec.fail_remove.store(true, Ordering::SeqCst);

        assert_eq!(
            be.release_session_checked(&sid).await,
            Err(BridgeError::StoreFailure),
            "a known owner after seal must join its retained cell and receive the cleanup report"
        );

        rec.fail_remove.store(false, Ordering::SeqCst);
        be.retire().await.unwrap();
        assert_eq!(rec.remove_count.load(Ordering::SeqCst), 2);
        assert_eq!(
            rec.order
                .lock()
                .unwrap()
                .iter()
                .filter(|step| step.as_str() == "inner_release")
                .count(),
            1,
            "retirement retries only the incomplete provider component"
        );
        std::fs::remove_dir_all(tmp).unwrap();
    }

    #[tokio::test]
    async fn sealed_backend_does_not_cache_unknown_late_release_sessions() {
        let (be, rec, tmp, _source, _cfg) = backend_fixture("sealed-unknown-release");
        be.retire().await.unwrap();

        for index in 0..3 {
            let sid = SessionId::parse(format!("ctx-sealed-unknown-{index}-g0")).unwrap();
            be.release_session_checked(&sid).await.unwrap();
        }

        assert_eq!(be.cleanup_cell_count(), 0);
        assert_eq!(
            rec.order
                .lock()
                .unwrap()
                .iter()
                .filter(|step| step.as_str() == "inner_release")
                .count(),
            0,
            "unknown cleanup after retirement must not create a new per-session generation"
        );
        std::fs::remove_dir_all(tmp).unwrap();
    }

    #[tokio::test]
    async fn checked_release_cleanup_flight_survives_waiter_cancellation() {
        let tmp = unique_temp_dir("release-waiter-canceled");
        let allowed_root = tmp.join("allowed");
        let source = allowed_root.join("source");
        let worktree_root = tmp.join("worktrees");
        std::fs::create_dir_all(&source).unwrap();
        std::fs::create_dir_all(&worktree_root).unwrap();
        let canonical_allowed_root = std::fs::canonicalize(&allowed_root).unwrap();
        let canonical_worktree_root = std::fs::canonicalize(&worktree_root).unwrap();
        let rec = Arc::new(Rec::default());
        let (allow_remove, remove_gate) = oneshot::channel();
        let provider = Arc::new(BlockingRemoveProv::new(rec.clone(), remove_gate));
        let be = Arc::new(WorktreeBackend::new(
            Arc::new(FakeInner { rec: rec.clone() }),
            provider.clone(),
            crate::provider_path::WorktreeConfig {
                root: canonical_worktree_root.to_string_lossy().into_owned(),
                owner: "ownr".into(),
                run: "run7".into(),
            },
            Some(SessionCwd::parse(&canonical_allowed_root.to_string_lossy()).unwrap()),
            identity(),
        ));
        let sid = SessionId::parse("ctx-release-waiter-canceled-g0").unwrap();
        be.configure_session(&sid, &spec(Some(&source.to_string_lossy())))
            .await
            .unwrap();

        let release_be = be.clone();
        let release_sid = sid.clone();
        let release =
            tokio::spawn(async move { release_be.release_session_checked(&release_sid).await });
        provider.wait_for_remove().await;
        release.abort();
        assert!(release.await.unwrap_err().is_cancelled());
        assert!(
            allow_remove.send(()).is_ok(),
            "canceling the report waiter must not cancel the provider-removal flight"
        );

        for _ in 0..100 {
            if be.map.lock().await.is_empty() {
                break;
            }
            tokio::task::yield_now().await;
        }
        assert!(be.map.lock().await.is_empty());
        assert_eq!(rec.remove_count.load(Ordering::SeqCst), 1);
        assert_eq!(
            rec.order
                .lock()
                .unwrap()
                .iter()
                .filter(|step| step.as_str() == "inner_release")
                .count(),
            1
        );
        std::fs::remove_dir_all(tmp).unwrap();
    }

    #[tokio::test]
    async fn concurrent_release_waiters_share_failure_report_then_explicit_retry_resumes_component()
    {
        let tmp = unique_temp_dir("release-shared-report");
        let allowed_root = tmp.join("allowed");
        let source = allowed_root.join("source");
        let worktree_root = tmp.join("worktrees");
        std::fs::create_dir_all(&source).unwrap();
        std::fs::create_dir_all(&worktree_root).unwrap();
        let canonical_allowed_root = std::fs::canonicalize(&allowed_root).unwrap();
        let canonical_worktree_root = std::fs::canonicalize(&worktree_root).unwrap();
        let rec = Arc::new(Rec::default());
        let (allow_first_remove, first_remove_gate) = oneshot::channel();
        let provider = Arc::new(BlockingRemoveProv::new_failing_once(
            rec.clone(),
            first_remove_gate,
        ));
        let be = Arc::new(WorktreeBackend::new(
            Arc::new(FakeInner { rec: rec.clone() }),
            provider.clone(),
            crate::provider_path::WorktreeConfig {
                root: canonical_worktree_root.to_string_lossy().into_owned(),
                owner: "ownr".into(),
                run: "run7".into(),
            },
            Some(SessionCwd::parse(&canonical_allowed_root.to_string_lossy()).unwrap()),
            identity(),
        ));
        let sid = SessionId::parse("ctx-release-shared-report-g0").unwrap();
        be.configure_session(&sid, &spec(Some(&source.to_string_lossy())))
            .await
            .unwrap();

        let first_be = be.clone();
        let first_sid = sid.clone();
        let first = tokio::spawn(async move { first_be.release_session_checked(&first_sid).await });
        provider.wait_for_remove().await;
        let second_be = be.clone();
        let second_sid = sid.clone();
        let second =
            tokio::spawn(async move { second_be.release_session_checked(&second_sid).await });
        for _ in 0..100 {
            if be.cleanup_join_count(&sid) >= 1 {
                break;
            }
            tokio::task::yield_now().await;
        }
        assert_eq!(be.cleanup_join_count(&sid), 1);

        allow_first_remove.send(()).unwrap();
        assert_eq!(first.await.unwrap(), Err(BridgeError::StoreFailure));
        assert_eq!(second.await.unwrap(), Err(BridgeError::StoreFailure));
        assert_eq!(
            rec.remove_count.load(Ordering::SeqCst),
            1,
            "a concurrent waiter joins the failed flight instead of retrying it"
        );
        assert!(be.map.lock().await.contains_key(sid.as_str()));

        be.release_session_checked(&sid).await.unwrap();
        assert_eq!(rec.remove_count.load(Ordering::SeqCst), 2);
        assert!(be.map.lock().await.is_empty());
        assert_eq!(
            rec.order
                .lock()
                .unwrap()
                .iter()
                .filter(|step| step.as_str() == "inner_release")
                .count(),
            1,
            "explicit retry resumes only the failed provider component"
        );
        std::fs::remove_dir_all(tmp).unwrap();
    }

    #[tokio::test]
    async fn stronger_release_upgrade_survives_its_waiter_cancellation() {
        let tmp = unique_temp_dir("release-upgrade-waiter-canceled");
        let allowed_root = tmp.join("allowed");
        let source = allowed_root.join("source");
        let worktree_root = tmp.join("worktrees");
        std::fs::create_dir_all(&source).unwrap();
        std::fs::create_dir_all(&worktree_root).unwrap();
        let canonical_allowed_root = std::fs::canonicalize(&allowed_root).unwrap();
        let canonical_worktree_root = std::fs::canonicalize(&worktree_root).unwrap();
        let rec = Arc::new(Rec::default());
        let (allow_remove, remove_gate) = oneshot::channel();
        let provider = Arc::new(BlockingRemoveProv::new(rec.clone(), remove_gate));
        let be = Arc::new(WorktreeBackend::new(
            Arc::new(FakeInner { rec: rec.clone() }),
            provider.clone(),
            crate::provider_path::WorktreeConfig {
                root: canonical_worktree_root.to_string_lossy().into_owned(),
                owner: "ownr".into(),
                run: "run7".into(),
            },
            Some(SessionCwd::parse(&canonical_allowed_root.to_string_lossy()).unwrap()),
            identity(),
        ));
        let sid = SessionId::parse("ctx-release-upgrade-waiter-canceled-g0").unwrap();
        be.configure_session(&sid, &spec(Some(&source.to_string_lossy())))
            .await
            .unwrap();

        let forget_be = be.clone();
        let forget_sid = sid.clone();
        let forget =
            tokio::spawn(async move { forget_be.forget_session_checked(&forget_sid).await });
        provider.wait_for_remove().await;
        let release_be = be.clone();
        let release_sid = sid.clone();
        let release =
            tokio::spawn(async move { release_be.release_session_checked(&release_sid).await });
        for _ in 0..100 {
            if be.cleanup_flight_strength(&sid) == Some(CleanupStrength::Release) {
                break;
            }
            tokio::task::yield_now().await;
        }
        assert_eq!(
            be.cleanup_flight_strength(&sid),
            Some(CleanupStrength::Release),
            "the stronger request must be owned before its first await"
        );
        release.abort();
        assert!(release.await.unwrap_err().is_cancelled());

        allow_remove.send(()).unwrap();
        forget.await.unwrap().unwrap();
        for _ in 0..100 {
            if rec
                .order
                .lock()
                .unwrap()
                .iter()
                .any(|step| step == "inner_release")
            {
                break;
            }
            tokio::task::yield_now().await;
        }
        assert_eq!(
            rec.order.lock().unwrap().as_slice(),
            ["inner_forget", "wt_remove", "inner_release"]
        );
        assert_eq!(rec.remove_count.load(Ordering::SeqCst), 1);
        assert!(be.map.lock().await.is_empty());
        std::fs::remove_dir_all(tmp).unwrap();
    }
}
