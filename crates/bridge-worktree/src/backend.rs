use crate::custody::{
    probe_custody_record_presence, probe_custody_record_state, CustodyRecordPresenceV1,
    PreservationReasonV1, WorktreeCustodyStateKindV1,
};
use crate::custody_writer::{
    observed_identity, planned_identity, CustodyWriteRefusalV1, DeletionAuthorizationV1,
    MaterializedIdentitiesV1, PreservationOutcomeV1, RemovalRecordV1, WorktreeCustodianV1,
};
use crate::provider::{CustodyAddOutcomeV1, CustodyAddTargetV1, WorktreeProvider};
use crate::provider_path::{
    resolve_worktree, sidecar_path, validate_bound_worktree, write_sidecar, ResolvedWorktree,
    WorktreeConfig, WorktreeSidecar,
};
use bridge_core::diagnostics::{DiagnosticCode, DiagnosticFailureClass, DiagnosticRedactor};
use bridge_core::domain::{Part, SessionSpec};
use bridge_core::error::BridgeError;
use bridge_core::execution_policy::{BoundSessionSpecV1, BoundedCauseV1, FrozenCheckoutEffectV1};
use bridge_core::fs_custody::{
    open_options_create_new_owner_private, pinned_root_unchanged, CustodyPublicationV1,
    PinnedDirectoryV1, RegularChildRefV1,
};
use bridge_core::ids::SessionId;
use bridge_core::orch::{AgentSessionCaps, ReconcileOutcome};
use bridge_core::permission::TurnMeta;
use bridge_core::ports::{
    AgentBackend, BackendCleanupDispositionV1, BackendObservers, BackendResourceFlightV1,
    BackendStream, CheckoutPreservationReasonV1, CheckoutPreservationV1, CheckoutSettlementV1,
    DiagnosticObserver, RichEventSink, WorkflowCheckoutOutcomeV1,
};
use bridge_core::preparation_flight::{
    BoundedPreparationTransferReasonV1, PreparationClockV1, PreparationFlightIdV1,
    PreparationFlightStateV1,
};
use bridge_core::terminal_evidence::{AcpChildLiveness, EvidenceCapability};
use bridge_core::SessionCwd;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::ffi::{OsStr, OsString};
use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};
#[cfg(test)]
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};
use std::sync::{Arc, Condvar, Mutex as StdMutex};
use std::time::Duration;
use tokio::sync::{oneshot, watch, Mutex, Notify};

const FAILED_CONFIGURE_RETRY_INITIAL: Duration = Duration::from_secs(1);
const FAILED_CONFIGURE_RETRY_MAX: Duration = Duration::from_secs(30);
const MAX_WORKTREE_CONFIGURES_IN_FLIGHT: u64 = 64;
const PREPARATION_FLIGHT_RECORD_SUFFIX: &str = ".preparation-flight.v1.json";
const PREPARATION_FLIGHT_RECORD_SCHEMA_V1: u16 = 1;
const PREPARATION_CALLER_PRESENT: u8 = 0;
const PREPARATION_CALLER_DEPARTED: u8 = 1;
const PREPARATION_CALLER_COMMITTED: u8 = 2;
const PREPARATION_ACTION_BOUND_MS: u64 = 30_000;
const PREPARATION_CONTROL_BOUND_MS: u64 = 31_000;

/// Exactly one pre-effect publisher owns a preparation flight. The phase is intentionally
/// sticky: a failed transfer publication remains transfer-owned debt rather than allowing a
/// barrier writer to race in and claim a different terminal outcome.
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PreparationPublicationPhaseV1 {
    Preparing = 0,
    TransferPublishing = 1,
    BarrierPublishing = 2,
    FailurePublishing = 3,
}

impl PreparationPublicationPhaseV1 {
    const fn as_u8(self) -> u8 {
        self as u8
    }

    fn from_u8(value: u8) -> Self {
        match value {
            0 => Self::Preparing,
            1 => Self::TransferPublishing,
            2 => Self::BarrierPublishing,
            3 => Self::FailurePublishing,
            _ => unreachable!("preparation publication phase is initialized from this enum"),
        }
    }
}

/// The pre-barrier blocking population is intentionally closed. A future slice may add a
/// blocking operation only by giving it a durable name here and placing a one-sample observation
/// on both sides of that operation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PreparationOperationV1 {
    JournalOpenPublish,
    CustodyEntryPublish,
    IdentityCapture,
}

impl PreparationOperationV1 {
    const fn as_str(self) -> &'static str {
        match self {
            Self::JournalOpenPublish => "journal_open_publish_sync",
            Self::CustodyEntryPublish => "custody_entry_publish_sync",
            Self::IdentityCapture => "identity_capture_sync",
        }
    }
}

/// An unarmed production flight cannot expire. Slice 4 will own production construction of this
/// carrier; this slice only permits the test seam to inject one.
#[derive(Clone, Debug)]
struct PreparationBoundV1 {
    clock: PreparationClockV1,
}

impl PreparationBoundV1 {
    fn expired_reason(
        &self,
        operation: PreparationOperationV1,
    ) -> Option<BoundedPreparationTransferReasonV1> {
        let elapsed_ms = self.clock.elapsed_ms();
        if elapsed_ms < PREPARATION_ACTION_BOUND_MS {
            return None;
        }
        BoundedPreparationTransferReasonV1::new(format!(
            "preparation action bound exceeded before prepared barrier: operation={}, \
             action_bound_ms={}, control_bound_ms={}, observed_elapsed_ms={elapsed_ms}",
            operation.as_str(),
            PREPARATION_ACTION_BOUND_MS,
            PREPARATION_CONTROL_BOUND_MS,
        ))
        .ok()
    }
}
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
    /// The checkout is RETAINED under R2f1b custody: a cleanup refused to remove it, or the
    /// preservation barrier settled a terminal claim over it. This state exists for two reasons,
    /// both of them ledger obligations 2b1 deferred here (slice 2c1).
    ///
    /// **Ownership retention through refusal (2b1 sol-2 / D-2).** `entry_for_cleanup` POPS a
    /// `Reserving` entry before the gate runs, so a refused rollback used to lose its last
    /// in-memory owner while the checkout persisted: the cleanup cell is then evicted by the
    /// reporter (a refusal reports `Ok`), `state.entry` dies with it, and a later cleanup finds
    /// nothing to remove — ever. Re-inserting here keeps exactly one owner, so once protection
    /// lifts the next cleanup reaches exactly one provider removal.
    ///
    /// **Ready-means-reusable (2b1 opus S-3).** Re-inserting a refused reservation as `Ready` was
    /// explicitly rejected in 2b1 because `configure_session`'s `Ready` arm hands the checkout to
    /// the next session after validating only `canonical_source` — which would hand a checkout
    /// awaiting R2f2 disposition to a new session and let it write over preserved work. `Retained`
    /// is not reusable, and both configure entry points refuse it by name.
    Retained {
        entry: WtEntry,
        retention: CheckoutRetentionV1,
    },
}

/// Why a checkout is retained rather than removed. Reported, not collapsed to a bool, because a
/// live-but-protected checkout and a terminally-preserved one have different R2f1b futures: the
/// first is released by 2c2's deletion authority on a healthy workflow outcome, the second only by
/// an R2f2 disposition.
///
/// `Ord` is the "keep the strongest knowledge" rule used by [`WorktreeBackend::retain_refused_entry`],
/// in increasing order of how settled the disposition is. It is monotonic and cannot mislabel in
/// practice: `Preserved` and `PreservationUnknown` are both R2f1b-terminal and neither has an
/// outgoing edge, so a checkout can never legitimately move down this order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum CheckoutRetentionV1 {
    /// The deletion gate refused on custody evidence; no preservation claim was minted here.
    RefusedUnderCustody,
    /// A preservation transition returned AMBIGUOUS: the record on disk is protective either way,
    /// but which of the two states it holds is unknown to this process.
    ///
    /// **Named separately from `PreservationUnknown` (repair RE-6), because they are not the same
    /// disk state.** After an ambiguous *prepared* publication the record says
    /// `PreservationPrepared`, not `PreservationUnknown`, so labelling it the latter asserted a
    /// terminal state that is not there. It is also no longer a dead end: with repair RA a later
    /// barrier RESUMES a stranded `PreservationPrepared` to its terminal state, which is exactly
    /// why this arm must be distinguishable from one that is already terminal.
    PreservationAmbiguous,
    /// A durable `PreservationUnknown` claim exists. Terminal for R2f1b.
    PreservationUnknown,
    /// A durable `Preserved` claim exists. Terminal for R2f1b.
    Preserved,
}

#[derive(Clone)]
struct WtEntry {
    canonical_source: String,
    worktree_path: String,
    custody: WtCustodyV1,
    /// The materialization-time evidence a preservation claim is minted from (slice 2c1, P7).
    ///
    /// **Deliberately a separate field from `custody`, not a payload on
    /// [`WtCustodyV1::Protected`].** The two answer different questions and are genuinely
    /// independent: `custody` is the AUTHORITY question the deletion gate asks ("may this be
    /// deleted?") and must be answerable when no evidence was ever captured, while this is the
    /// EVIDENCE question the preservation barrier asks ("can a truthful claim be minted?"). Fusing
    /// them would make "protected, but its identities were never captured" unrepresentable — and
    /// that state is real, it is exactly what 2b1's
    /// `the_discriminator_alone_refuses_deletion_with_no_record_on_disk` exercises, and the
    /// fail-closed answer there is *refuse the deletion AND refuse to mint a claim*, not one or
    /// the other.
    protection: Option<Box<ProtectedCheckoutV1>>,
}

type MaterializationResultV1 = Result<(WtCustodyV1, Option<Box<ProtectedCheckoutV1>>), BridgeError>;

/// One durable snapshot of the materialization preparation flight. It is deliberately a
/// companion to the custody record: preparation owns the runner lifecycle while custody remains
/// the sole authority for checkout state transitions.
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct PreparationFlightRecordV1 {
    schema_version: u16,
    flight_id: PreparationFlightIdV1,
    state: PreparationFlightStateV1,
}

/// The in-process claim is made before the detached task can perform a filesystem effect. The
/// durable companion record below is the cross-process truth; this mutex only lets the owner
/// retain an exact state while a caller drops its observer future.
struct MaterializationPreparationFlightV1 {
    id: PreparationFlightIdV1,
    state: StdMutex<PreparationFlightStateV1>,
    bound: Option<PreparationBoundV1>,
    operation: StdMutex<Option<PreparationOperationV1>>,
    journal: StdMutex<Option<Arc<PreparationFlightJournalV1>>>,
    phase: AtomicU8,
    /// The observing configure future owns the only guard that can set this flag. The detached
    /// runner samples it exactly once between durable Open and add admission; after that sample,
    /// caller departure cannot cancel a committed materialization.
    caller_departed: Arc<AtomicU8>,
    #[cfg(test)]
    hooks: Arc<PreparationFlightTestHooks>,
}

struct PreparationFlightCallerGuardV1 {
    caller_departed: Arc<AtomicU8>,
    armed: bool,
}

impl PreparationFlightCallerGuardV1 {
    fn disarm(mut self) {
        self.armed = false;
    }
}

impl Drop for PreparationFlightCallerGuardV1 {
    fn drop(&mut self) {
        if self.armed {
            let _ = self.caller_departed.compare_exchange(
                PREPARATION_CALLER_PRESENT,
                PREPARATION_CALLER_DEPARTED,
                Ordering::AcqRel,
                Ordering::Acquire,
            );
        }
    }
}

/// A detached runner may panic or be aborted after its result sender has been installed in the
/// active owner. The exit guard makes that failure observable to a backend-owned terminalizer;
/// otherwise the retained sender would leave configure waiting forever.
struct PreparationRunnerExitGuardV1 {
    exit: Option<oneshot::Sender<()>>,
    completed: bool,
}

impl PreparationRunnerExitGuardV1 {
    fn new(exit: oneshot::Sender<()>) -> Self {
        Self {
            exit: Some(exit),
            completed: false,
        }
    }

    fn complete(&mut self) {
        self.completed = true;
    }
}

impl Drop for PreparationRunnerExitGuardV1 {
    fn drop(&mut self) {
        if !self.completed {
            if let Some(exit) = self.exit.take() {
                let _ = exit.send(());
            }
        }
    }
}

impl MaterializationPreparationFlightV1 {
    fn claim(
        #[cfg(test)] hooks: Arc<PreparationFlightTestHooks>,
        #[cfg(test)] bound: Option<PreparationBoundV1>,
    ) -> Result<Self, BridgeError> {
        #[cfg(not(test))]
        let bound = None;
        Ok(Self {
            id: PreparationFlightIdV1::mint()?,
            state: StdMutex::new(PreparationFlightStateV1::Open {}),
            bound,
            operation: StdMutex::new(None),
            journal: StdMutex::new(None),
            phase: AtomicU8::new(PreparationPublicationPhaseV1::Preparing.as_u8()),
            caller_departed: Arc::new(AtomicU8::new(PREPARATION_CALLER_PRESENT)),
            #[cfg(test)]
            hooks,
        })
    }

    fn id(&self) -> &PreparationFlightIdV1 {
        &self.id
    }

    fn record(&self, state: PreparationFlightStateV1) {
        *self.state.lock().unwrap_or_else(|error| error.into_inner()) = state;
    }

    fn begin_operation(&self, operation: PreparationOperationV1) {
        *self
            .operation
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = Some(operation);
    }

    fn set_journal(&self, journal: Arc<PreparationFlightJournalV1>) {
        *self
            .journal
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = Some(journal);
    }

    fn expired_pre_barrier(
        &self,
    ) -> Option<(PreparationOperationV1, BoundedPreparationTransferReasonV1)> {
        if self.phase.load(Ordering::Acquire) != PreparationPublicationPhaseV1::Preparing.as_u8() {
            return None;
        }
        let operation = (*self
            .operation
            .lock()
            .unwrap_or_else(|error| error.into_inner()))?;
        let bound = self.bound.as_ref()?;
        bound
            .expired_reason(operation)
            .map(|reason| (operation, reason))
    }

    fn begin_transfer(&self) -> bool {
        self.phase
            .compare_exchange(
                PreparationPublicationPhaseV1::Preparing.as_u8(),
                PreparationPublicationPhaseV1::TransferPublishing.as_u8(),
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_ok()
    }

    fn transfer_owned(&self) -> bool {
        self.phase.load(Ordering::Acquire)
            == PreparationPublicationPhaseV1::TransferPublishing.as_u8()
    }

    fn phase(&self) -> PreparationPublicationPhaseV1 {
        PreparationPublicationPhaseV1::from_u8(self.phase.load(Ordering::Acquire))
    }

    fn begin_failure_publication(&self) -> bool {
        self.phase
            .compare_exchange(
                PreparationPublicationPhaseV1::Preparing.as_u8(),
                PreparationPublicationPhaseV1::FailurePublishing.as_u8(),
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_ok()
    }

    fn begin_barrier_publication(&self) -> bool {
        if self
            .phase
            .compare_exchange(
                PreparationPublicationPhaseV1::Preparing.as_u8(),
                PreparationPublicationPhaseV1::BarrierPublishing.as_u8(),
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_err()
        {
            return false;
        }
        *self
            .operation
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = None;
        true
    }

    fn journal(&self) -> Option<Arc<PreparationFlightJournalV1>> {
        self.journal
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone()
    }

    fn caller_guard(&self) -> PreparationFlightCallerGuardV1 {
        PreparationFlightCallerGuardV1 {
            caller_departed: self.caller_departed.clone(),
            armed: true,
        }
    }

    /// This is the one and only cancellation observation for a claimed preparation flight. The
    /// successful compare-and-exchange IS the false sample and commits the runner in one atomic
    /// transition, so cleanup cannot observe an uncommitted gap before custody admission.
    fn commit_add_admission(&self) -> bool {
        self.caller_departed
            .compare_exchange(
                PREPARATION_CALLER_PRESENT,
                PREPARATION_CALLER_COMMITTED,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_err()
    }

    fn committed(&self) -> bool {
        self.caller_departed.load(Ordering::Acquire) == PREPARATION_CALLER_COMMITTED
    }

    fn has_durable_terminal(&self) -> bool {
        preparation_state_is_terminal(&self.state.lock().unwrap_or_else(|error| error.into_inner()))
    }
}

/// Backend-owned completion for a claimed preparation flight. A normal terminal publication
/// releases the active-flight entry; a publication failure leaves its typed debt here for the
/// cleanup/retirement owner to observe. This is intentionally separate from the caller's
/// one-shot result: observer departure must not erase a nonterminal durable-record failure.
struct ActivePreparationFlightV1 {
    flight: Arc<MaterializationPreparationFlightV1>,
    completion: watch::Sender<Option<Result<(), BridgeError>>>,
    #[allow(dead_code)] // Read by T3 recovery; this slice must retain it without consuming it.
    runner: StdMutex<Option<tokio::task::JoinHandle<()>>>,
    result: StdMutex<Option<oneshot::Sender<MaterializationResultV1>>>,
}

impl ActivePreparationFlightV1 {
    fn new(flight: Arc<MaterializationPreparationFlightV1>) -> Self {
        let (completion, _receiver) = watch::channel(None);
        Self {
            flight,
            completion,
            runner: StdMutex::new(None),
            result: StdMutex::new(None),
        }
    }

    fn completion(&self) -> watch::Receiver<Option<Result<(), BridgeError>>> {
        self.completion.subscribe()
    }

    fn install_result(&self, result: oneshot::Sender<MaterializationResultV1>) {
        *self
            .result
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = Some(result);
    }

    fn install_runner(&self, runner: tokio::task::JoinHandle<()>) {
        *self
            .runner
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = Some(runner);
    }

    fn complete(&self, result: Result<(), BridgeError>) {
        if self.completion.borrow().is_none() {
            self.completion.send_replace(Some(result));
        }
    }

    async fn send_result(&self, result: MaterializationResultV1) {
        if let Some(sender) = self
            .result
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .take()
        {
            let _ = sender.send(result);
        }
        #[cfg(test)]
        self.flight.after_result_publication_for_test().await;
    }

    async fn complete_with_result(
        &self,
        completion: Result<(), BridgeError>,
        result: MaterializationResultV1,
    ) {
        self.complete(completion);
        self.send_result(result).await;
    }
}

/// The runner's transfer inputs are grouped so the custody helper keeps the exact active owner
/// coupled to the registries it may transfer into. The flight itself is always derived from that
/// owner, avoiding a mismatched flight/owner pair across the transfer-versus-barrier boundary.
struct PreparationFlightRunContextV1<'a> {
    flights: &'a Arc<StdMutex<HashMap<String, Arc<ActivePreparationFlightV1>>>>,
    recovery_flights: &'a Arc<StdMutex<HashMap<String, Arc<TransferredPreparationFlightV1>>>>,
    session_key: &'a str,
    owner: &'a Arc<ActivePreparationFlightV1>,
}

/// Recovery owns this exact active owner after a pre-effect transfer. Its `runner` field retains
/// the nonreturning operation future; no cleanup path is permitted to detach or discard it.
#[allow(dead_code)] // T3 is the first production recovery/inventory consumer.
struct TransferredPreparationFlightV1 {
    owner: Arc<ActivePreparationFlightV1>,
    operation: PreparationOperationV1,
    reason: BoundedPreparationTransferReasonV1,
}

enum PreparationControlRootStateV1 {
    Unpinned,
    Pinning,
    Pinned(Arc<PinnedDirectoryV1>),
}

struct PreparationControlRootV1 {
    path: PathBuf,
    state: StdMutex<PreparationControlRootStateV1>,
    pinned: Condvar,
    #[cfg(test)]
    hooks: Arc<PreparationFlightTestHooks>,
}

impl PreparationControlRootV1 {
    fn new(path: PathBuf, #[cfg(test)] hooks: Arc<PreparationFlightTestHooks>) -> Self {
        Self {
            path,
            state: StdMutex::new(PreparationControlRootStateV1::Unpinned),
            pinned: Condvar::new(),
            #[cfg(test)]
            hooks,
        }
    }

    fn begin_pin_after_owner_published(&self) -> bool {
        let mut held = self.state.lock().unwrap_or_else(|error| error.into_inner());
        if matches!(&*held, PreparationControlRootStateV1::Unpinned) {
            *held = PreparationControlRootStateV1::Pinning;
            true
        } else {
            false
        }
    }

    fn open_claimed_for_session_admission(&self) -> Result<(), BridgeError> {
        #[cfg(test)]
        self.hooks.block_control_root_pin_for_test();
        let opened = PinnedDirectoryV1::open(&self.path, "preparation flight root")
            .map(Arc::new)
            .map_err(|_| BridgeError::StoreFailure);
        let mut held = self.state.lock().unwrap_or_else(|error| error.into_inner());
        let result = match opened {
            Ok(root) if matches!(&*held, PreparationControlRootStateV1::Pinning) => {
                *held = PreparationControlRootStateV1::Pinned(root);
                Ok(())
            }
            Ok(_) => Err(BridgeError::StoreFailure),
            Err(error) => {
                *held = PreparationControlRootStateV1::Unpinned;
                Err(error)
            }
        };
        self.pinned.notify_all();
        result
    }

    fn pin_is_pending(&self) -> bool {
        matches!(
            &*self.state.lock().unwrap_or_else(|error| error.into_inner()),
            PreparationControlRootStateV1::Pinning
        )
    }

    fn pinned_root(&self) -> Result<Arc<PinnedDirectoryV1>, BridgeError> {
        let mut held = self.state.lock().unwrap_or_else(|error| error.into_inner());
        loop {
            match &*held {
                PreparationControlRootStateV1::Pinned(root) => {
                    pinned_root_unchanged(root).map_err(|_| BridgeError::StoreFailure)?;
                    return Ok(root.clone());
                }
                PreparationControlRootStateV1::Pinning => {
                    held = self
                        .pinned
                        .wait(held)
                        .unwrap_or_else(|error| error.into_inner());
                }
                PreparationControlRootStateV1::Unpinned => return Err(BridgeError::StoreFailure),
            }
        }
    }
}

/// Descriptor-safe, parent-synced companion record for one materialization flight. It is not a
/// custody transition and therefore cannot add an edge to the frozen custody table.
struct PreparationFlightJournalV1 {
    control_root: Arc<PreparationControlRootV1>,
    record_name: OsString,
    flight_id: PreparationFlightIdV1,
}

impl PreparationFlightJournalV1 {
    fn new(
        control_root: Arc<PreparationControlRootV1>,
        worktree_path: &str,
        flight_id: PreparationFlightIdV1,
    ) -> Result<Self, BridgeError> {
        let record_path = format!("{worktree_path}{PREPARATION_FLIGHT_RECORD_SUFFIX}");
        let record_name = Path::new(&record_path)
            .file_name()
            .map(OsStr::to_os_string)
            .ok_or(BridgeError::StoreFailure)?;
        Ok(Self {
            control_root,
            record_name,
            flight_id,
        })
    }

    fn root(&self) -> Result<Arc<PinnedDirectoryV1>, BridgeError> {
        self.control_root.pinned_root()
    }

    fn root_pin_is_pending(&self) -> bool {
        self.control_root.pin_is_pending()
    }

    fn publish_with_root(
        &self,
        root: &PinnedDirectoryV1,
        state: PreparationFlightStateV1,
        first: bool,
    ) -> Result<(), BridgeError> {
        let bytes = serde_json::to_vec(&PreparationFlightRecordV1 {
            schema_version: PREPARATION_FLIGHT_RECORD_SCHEMA_V1,
            flight_id: self.flight_id.clone(),
            state,
        })
        .map_err(|_| BridgeError::StoreFailure)?;
        let nonce = PreparationFlightIdV1::mint().map_err(|_| BridgeError::StoreFailure)?;
        let suffix = &nonce.as_str()[PreparationFlightIdV1::PREFIX.len()..];
        let mut staged_name = self.record_name.clone();
        staged_name.push(format!(".staging-{suffix}"));
        let staged_path = root.canonical_path().join(&staged_name);
        let mut staged = open_options_create_new_owner_private()
            .open(&staged_path)
            .map_err(|_| BridgeError::StoreFailure)?;
        staged
            .write_all(&bytes)
            .and_then(|()| staged.sync_all())
            .map_err(|_| BridgeError::StoreFailure)?;
        let source = RegularChildRefV1::new(&staged_name, &staged);
        let published = if first {
            root.publish_new_regular_child(source, &self.record_name, "preparation flight record")
        } else {
            root.replace_regular_child(source, &self.record_name, "preparation flight record")
        }
        .map_err(|_| BridgeError::StoreFailure)?;
        match published {
            CustodyPublicationV1::Durable { .. } => Ok(()),
            CustodyPublicationV1::ParentSyncAmbiguous(_)
            | CustodyPublicationV1::TargetIdentityUnverified(_)
            | CustodyPublicationV1::RenameOutcomeUnverified(_) => Err(BridgeError::StoreFailure),
        }
    }

    fn publish(&self, state: PreparationFlightStateV1, first: bool) -> Result<(), BridgeError> {
        let root = self.root()?;
        self.publish_with_root(&root, state, first)
    }

    fn decode_record(
        &self,
        file: &std::fs::File,
    ) -> Result<PreparationFlightRecordV1, BridgeError> {
        let mut existing = file.try_clone().map_err(|_| BridgeError::StoreFailure)?;
        let mut bytes = Vec::new();
        existing
            .read_to_end(&mut bytes)
            .map_err(|_| BridgeError::StoreFailure)?;
        let record: PreparationFlightRecordV1 =
            serde_json::from_slice(&bytes).map_err(|_| BridgeError::StoreFailure)?;
        if record.schema_version != PREPARATION_FLIGHT_RECORD_SCHEMA_V1
            || record.flight_id != self.flight_id
        {
            return Err(BridgeError::StoreFailure);
        }
        Ok(record)
    }

    fn publish_terminal(&self, state: PreparationFlightStateV1) -> Result<(), BridgeError> {
        let root = self.root()?;
        let exists = root
            .child_entry_exists(&self.record_name, "preparation flight record")
            .map_err(|_| BridgeError::StoreFailure)?;
        if !exists && self.publish_with_root(&root, state.clone(), true).is_ok() {
            return Ok(());
        }
        root.with_existing_regular_child_lease(
            &self.record_name,
            "preparation flight record",
            |existing| -> Result<(), BridgeError> {
                let record = self.decode_record(existing)?;
                let allowed = matches!(
                    (&record.state, &state),
                    (
                        PreparationFlightStateV1::Open {},
                        PreparationFlightStateV1::Transferred { .. }
                    ) | (
                        PreparationFlightStateV1::Open {},
                        PreparationFlightStateV1::Failed { .. }
                    ) | (
                        PreparationFlightStateV1::BarrierSynced {},
                        PreparationFlightStateV1::Settled {}
                    ) | (
                        PreparationFlightStateV1::BarrierSynced {},
                        PreparationFlightStateV1::Failed { .. }
                    )
                );
                if !allowed {
                    return Err(BridgeError::StoreFailure);
                }
                #[cfg(test)]
                self.control_root
                    .hooks
                    .block_initial_open_publish_if_armed();
                self.publish_with_root(&root, state.clone(), false)
            },
        )
        .map_err(|_| BridgeError::StoreFailure)??;
        Ok(())
    }

    #[cfg(test)]
    fn fail_next_parent_sync_for_test(&self) {
        if let Ok(root) = self.root() {
            root.fail_sync_on_nth_call_for_test(1);
        }
    }
}

fn preparation_failure_state() -> PreparationFlightStateV1 {
    PreparationFlightStateV1::Failed {
        cause: BoundedCauseV1 {
            failure_class: DiagnosticFailureClass::Persistence,
            code: DiagnosticCode::build(
                "bridge.worktree_preparation_failed",
                &DiagnosticRedactor::default(),
            )
            .expect("static preparation flight diagnostic code is valid"),
            deepest_cause: None,
            cause_truncated: false,
            evidence_overflow: false,
            dependency_set: None,
        },
    }
}

fn preparation_caller_departed_failure_state() -> PreparationFlightStateV1 {
    PreparationFlightStateV1::Failed {
        cause: BoundedCauseV1 {
            failure_class: DiagnosticFailureClass::Canceled,
            code: DiagnosticCode::build(
                "bridge.worktree_preparation_caller_departed",
                &DiagnosticRedactor::default(),
            )
            .expect("static preparation flight diagnostic code is valid"),
            deepest_cause: None,
            cause_truncated: false,
            evidence_overflow: false,
            dependency_set: None,
        },
    }
}

fn preparation_state_is_terminal(state: &PreparationFlightStateV1) -> bool {
    match state {
        PreparationFlightStateV1::Open {} | PreparationFlightStateV1::BarrierSynced {} => false,
        PreparationFlightStateV1::Settled {}
        | PreparationFlightStateV1::Transferred { .. }
        | PreparationFlightStateV1::Failed { .. } => true,
    }
}

async fn publish_preparation_state(
    journal: Arc<PreparationFlightJournalV1>,
    flight: Arc<MaterializationPreparationFlightV1>,
    state: PreparationFlightStateV1,
    first: bool,
) -> Result<(), BridgeError> {
    #[cfg(test)]
    if preparation_state_is_terminal(&state) && flight.terminal_publication_fails_for_test() {
        return Err(BridgeError::StoreFailure);
    }
    let terminal = preparation_state_is_terminal(&state);
    let durable_state = state.clone();
    tokio::task::spawn_blocking(move || {
        if terminal {
            journal.publish_terminal(durable_state)
        } else {
            journal.publish(durable_state, first)
        }
    })
    .await
    .map_err(|_| BridgeError::StoreFailure)??;
    flight.record(state);
    #[cfg(test)]
    if terminal {
        flight.terminal_recorded_for_test();
    }
    #[cfg(not(test))]
    let _ = terminal;
    Ok(())
}

async fn publish_runner_exit_failure(
    journal: Arc<PreparationFlightJournalV1>,
    flight: Arc<MaterializationPreparationFlightV1>,
) -> Result<(), BridgeError> {
    publish_preparation_state(journal, flight, preparation_failure_state(), false).await
}

async fn terminalize_preparation_runner_exit(
    flights: Arc<StdMutex<HashMap<String, Arc<ActivePreparationFlightV1>>>>,
    session_key: String,
    owner: Arc<ActivePreparationFlightV1>,
) {
    match owner.flight.phase() {
        PreparationPublicationPhaseV1::TransferPublishing => return,
        PreparationPublicationPhaseV1::Preparing if !owner.flight.begin_failure_publication() => {
            return;
        }
        PreparationPublicationPhaseV1::Preparing
        | PreparationPublicationPhaseV1::FailurePublishing
        | PreparationPublicationPhaseV1::BarrierPublishing => {}
    }
    let runner_error =
        BridgeError::agent_crashed("materialization preparation runner exited unexpectedly");
    let completion = if owner.flight.has_durable_terminal() {
        Ok(())
    } else if let Some(journal) = owner.flight.journal() {
        match publish_runner_exit_failure(journal, owner.flight.clone()).await {
            Ok(()) => Ok(()),
            Err(error) => Err(error),
        }
    } else {
        Err(BridgeError::StoreFailure)
    };
    let result = Err(completion.as_ref().err().cloned().unwrap_or(runner_error));
    owner.complete_with_result(completion.clone(), result).await;
    if completion.is_ok() {
        let mut active = flights.lock().unwrap_or_else(|error| error.into_inner());
        if active
            .get(&session_key)
            .is_some_and(|current| Arc::ptr_eq(current, &owner))
        {
            active.remove(&session_key);
        }
    }
}

/// Move an expired pre-effect runner into the backend's recovery inventory.
async fn transfer_preparation_flight(
    flights: &Arc<StdMutex<HashMap<String, Arc<ActivePreparationFlightV1>>>>,
    recovery_flights: &Arc<StdMutex<HashMap<String, Arc<TransferredPreparationFlightV1>>>>,
    session_key: &str,
    owner: &Arc<ActivePreparationFlightV1>,
    operation: PreparationOperationV1,
    reason: BoundedPreparationTransferReasonV1,
) -> Result<bool, BridgeError> {
    if !owner.flight.begin_transfer() {
        return Ok(false);
    }
    let Some(journal) = owner.flight.journal() else {
        owner
            .complete_with_result(
                Err(BridgeError::StoreFailure),
                Err(BridgeError::StoreFailure),
            )
            .await;
        return Err(BridgeError::StoreFailure);
    };
    let state = PreparationFlightStateV1::Transferred {
        reason: reason.clone(),
    };
    let deferred = journal.root_pin_is_pending();
    if deferred {
        let delayed_journal = journal.clone();
        let delayed_flight = owner.flight.clone();
        let delayed_owner = owner.clone();
        std::mem::drop(tokio::spawn(async move {
            delayed_owner.complete(
                publish_preparation_state(delayed_journal, delayed_flight, state, false).await,
            );
        }));
    } else if let Err(error) =
        publish_preparation_state(journal, owner.flight.clone(), state, false).await
    {
        owner
            .complete_with_result(Err(error.clone()), Err(error.clone()))
            .await;
        return Err(error);
    }
    let recovered = Arc::new(TransferredPreparationFlightV1 {
        owner: owner.clone(),
        operation,
        reason,
    });
    let registry_moved = {
        let mut recovery = recovery_flights
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if recovery.contains_key(session_key) {
            false
        } else {
            recovery.insert(session_key.to_owned(), recovered);
            let mut active = flights.lock().unwrap_or_else(|error| error.into_inner());
            if active
                .get(session_key)
                .is_some_and(|current| Arc::ptr_eq(current, owner))
            {
                active.remove(session_key);
                true
            } else {
                recovery.remove(session_key);
                false
            }
        }
    };
    if !registry_moved {
        owner
            .complete_with_result(
                Err(BridgeError::StoreFailure),
                Err(BridgeError::StoreFailure),
            )
            .await;
        return Err(BridgeError::StoreFailure);
    }
    let result = Err(BridgeError::ConfigInvalid {
        reason: format!(
            "preparation transferred before effect admission at {}",
            operation.as_str()
        ),
    });
    if deferred {
        owner.send_result(result).await;
    } else {
        owner.complete_with_result(Ok(()), result).await;
    }
    Ok(true)
}
#[cfg(test)]
#[derive(Default)]
struct PreparationFlightTestHooks {
    open_count: AtomicUsize,
    open: Notify,
    pause_after_open: AtomicBool,
    release_after_open: Notify,
    fail_initial_open_parent_sync: AtomicBool,
    block_initial_open_publish: AtomicBool,
    initial_open_publish_entered_count: AtomicUsize,
    initial_open_publish_entered: Notify,
    initial_open_publish_released: StdMutex<bool>,
    initial_open_publish_release: Condvar,
    block_control_root_pin: AtomicBool,
    control_root_pin_entered_count: AtomicUsize,
    control_root_pin_entered: Notify,
    control_root_pin_released: StdMutex<bool>,
    control_root_pin_release: Condvar,
    result_publication_count: AtomicUsize,
    result_published: Notify,
    pause_after_result_publication: AtomicBool,
    release_after_result_publication: Notify,
    add_admission_count: AtomicUsize,
    add_admission: Notify,
    pause_after_add_admission: AtomicBool,
    release_after_add_admission: Notify,
    add_count: AtomicUsize,
    add: Notify,
    pause_after_add: AtomicBool,
    release_after_add: Notify,
    terminal_count: AtomicUsize,
    terminal: Notify,
    fail_terminal_publication: AtomicBool,
    block_custody_sync: AtomicBool,
    custody_sync_entered_count: AtomicUsize,
    custody_sync_entered: Notify,
    custody_sync_released: StdMutex<bool>,
    custody_sync_release: Condvar,
}

#[cfg(test)]
impl PreparationFlightTestHooks {
    async fn after_open(&self) {
        self.open_count.fetch_add(1, Ordering::SeqCst);
        self.open.notify_waiters();
        if self.pause_after_open.swap(false, Ordering::SeqCst) {
            self.release_after_open.notified().await;
        }
    }

    fn take_initial_open_parent_sync_failure(&self) -> bool {
        self.fail_initial_open_parent_sync
            .swap(false, Ordering::SeqCst)
    }

    fn arm_nonreturning_initial_open_publish(&self) {
        *self
            .initial_open_publish_released
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = false;
        self.block_initial_open_publish
            .store(true, Ordering::SeqCst);
    }

    fn block_initial_open_publish_if_armed(&self) {
        if self
            .block_initial_open_publish
            .swap(false, Ordering::SeqCst)
        {
            self.initial_open_publish_entered_count
                .fetch_add(1, Ordering::SeqCst);
            self.initial_open_publish_entered.notify_waiters();
            let mut released = self
                .initial_open_publish_released
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            while !*released {
                released = self
                    .initial_open_publish_release
                    .wait(released)
                    .unwrap_or_else(|error| error.into_inner());
            }
        }
    }

    async fn wait_for_initial_open_publish(&self) {
        while self
            .initial_open_publish_entered_count
            .load(Ordering::SeqCst)
            == 0
        {
            self.initial_open_publish_entered.notified().await;
        }
    }

    fn release_initial_open_publish(&self) {
        *self
            .initial_open_publish_released
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = true;
        self.initial_open_publish_release.notify_all();
    }

    fn arm_nonreturning_control_root_pin(&self) {
        *self
            .control_root_pin_released
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = false;
        self.block_control_root_pin.store(true, Ordering::SeqCst);
    }

    fn block_control_root_pin_for_test(&self) {
        if self.block_control_root_pin.swap(false, Ordering::SeqCst) {
            self.control_root_pin_entered_count
                .fetch_add(1, Ordering::SeqCst);
            self.control_root_pin_entered.notify_waiters();
            let mut released = self
                .control_root_pin_released
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            while !*released {
                released = self
                    .control_root_pin_release
                    .wait(released)
                    .unwrap_or_else(|error| error.into_inner());
            }
        }
    }

    async fn wait_for_control_root_pin(&self) {
        while self.control_root_pin_entered_count.load(Ordering::SeqCst) == 0 {
            self.control_root_pin_entered.notified().await;
        }
    }

    fn release_control_root_pin(&self) {
        *self
            .control_root_pin_released
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = true;
        self.control_root_pin_release.notify_all();
    }

    async fn after_result_publication(&self) {
        self.result_publication_count.fetch_add(1, Ordering::SeqCst);
        self.result_published.notify_waiters();
        if self
            .pause_after_result_publication
            .swap(false, Ordering::SeqCst)
        {
            self.release_after_result_publication.notified().await;
        }
    }

    async fn wait_for_result_publication(&self) {
        while self.result_publication_count.load(Ordering::SeqCst) == 0 {
            self.result_published.notified().await;
        }
    }

    async fn after_add_admission(&self) {
        self.add_admission_count.fetch_add(1, Ordering::SeqCst);
        self.add_admission.notify_waiters();
        if self.pause_after_add_admission.swap(false, Ordering::SeqCst) {
            self.release_after_add_admission.notified().await;
        }
    }

    async fn after_add(&self) {
        self.add_count.fetch_add(1, Ordering::SeqCst);

        self.add.notify_waiters();
        if self.pause_after_add.swap(false, Ordering::SeqCst) {
            self.release_after_add.notified().await;
        }
    }

    fn arm_nonreturning_custody_sync(&self) {
        *self
            .custody_sync_released
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = false;
        self.block_custody_sync.store(true, Ordering::SeqCst);
    }

    fn block_custody_sync_if_armed(&self) {
        if self.block_custody_sync.swap(false, Ordering::SeqCst) {
            self.custody_sync_entered_count
                .fetch_add(1, Ordering::SeqCst);
            self.custody_sync_entered.notify_waiters();
            let mut released = self
                .custody_sync_released
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            while !*released {
                released = self
                    .custody_sync_release
                    .wait(released)
                    .unwrap_or_else(|error| error.into_inner());
            }
        }
    }

    async fn wait_for_custody_sync(&self) {
        while self.custody_sync_entered_count.load(Ordering::SeqCst) == 0 {
            self.custody_sync_entered.notified().await;
        }
    }

    fn release_custody_sync(&self) {
        *self
            .custody_sync_released
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = true;
        self.custody_sync_release.notify_all();
    }

    fn terminal_publication_fails(&self) -> bool {
        self.fail_terminal_publication.load(Ordering::SeqCst)
    }

    fn terminal_recorded(&self) {
        self.terminal_count.fetch_add(1, Ordering::SeqCst);
        self.terminal.notify_waiters();
    }

    async fn wait_for_open(&self) {
        tokio::time::timeout(Duration::from_secs(2), async {
            while self.open_count.load(Ordering::SeqCst) == 0 {
                let notified = self.open.notified();
                if self.open_count.load(Ordering::SeqCst) == 0 {
                    notified.await;
                }
            }
        })
        .await
        .expect("preparation test timed out waiting for durable Open");
    }

    async fn wait_for_add_admission(&self) {
        tokio::time::timeout(Duration::from_secs(2), async {
            while self.add_admission_count.load(Ordering::SeqCst) == 0 {
                let notified = self.add_admission.notified();
                if self.add_admission_count.load(Ordering::SeqCst) == 0 {
                    notified.await;
                }
            }
        })
        .await
        .expect("preparation test timed out waiting for committed add admission");
    }

    async fn wait_for_add(&self) {
        tokio::time::timeout(Duration::from_secs(2), async {
            while self.add_count.load(Ordering::SeqCst) == 0 {
                let notified = self.add.notified();
                if self.add_count.load(Ordering::SeqCst) == 0 {
                    notified.await;
                }
            }
        })
        .await
        .expect("preparation test timed out waiting for custody-aware add");
    }

    async fn wait_for_terminal(&self) {
        tokio::time::timeout(Duration::from_secs(2), async {
            while self.terminal_count.load(Ordering::SeqCst) == 0 {
                let notified = self.terminal.notified();
                if self.terminal_count.load(Ordering::SeqCst) == 0 {
                    notified.await;
                }
            }
        })
        .await
        .expect("preparation test timed out waiting for terminal publication");
    }
}

#[cfg(test)]
impl MaterializationPreparationFlightV1 {
    async fn after_open_for_test(&self) {
        self.hooks.after_open().await;
    }

    fn fail_initial_open_parent_sync_for_test(&self) -> bool {
        self.hooks.take_initial_open_parent_sync_failure()
    }

    fn block_initial_open_publish_for_test(&self) {
        self.hooks.block_initial_open_publish_if_armed();
    }

    async fn after_result_publication_for_test(&self) {
        self.hooks.after_result_publication().await;
    }

    async fn after_add_admission_for_test(&self) {
        self.hooks.after_add_admission().await;
    }

    async fn after_add_for_test(&self) {
        self.hooks.after_add().await;
    }

    fn terminal_publication_fails_for_test(&self) -> bool {
        self.hooks.terminal_publication_fails()
    }

    fn block_custody_sync_for_test(&self) {
        self.hooks.block_custody_sync_if_armed();
    }
    fn terminal_recorded_for_test(&self) {
        self.hooks.terminal_recorded();
    }
}

/// Everything a later preservation transition needs, captured at materialization under the custody
/// cell and retained for the checkout's lifetime (2b2 opus S-9 / sol S-3).
///
/// The 2b2 writer observed all four object identities by descriptor to publish `LiveProtected` —
/// and then discarded them, because `LiveProtected` FORBIDS a claim. At preservation time the
/// objects can no longer be trusted to be the same ones, so re-observing them is not a
/// substitute: it would mint a claim over whatever now occupies those paths.
#[derive(Clone, Debug)]
struct ProtectedCheckoutV1 {
    /// The binding the custodian re-enters with: it carries the custody id (the cell key), the
    /// attempt identity, the node ref, and the frozen plan the claim repeats.
    binding: bridge_core::execution_policy::BoundWorktreeCustodyV1,
    /// The four object identities, observed by descriptor at materialization.
    identities: MaterializedIdentitiesV1,
    /// The recovery locator the materializing add proved. Git-level registration is NOT re-probed
    /// at preservation time — no `WorktreeProvider` operation exposes it, and inventing one is a
    /// later slice's — so the claim reports this answer, downgraded to
    /// [`RecoveryLocatorV1::RegistrationUnproven`] whenever the descriptor reverification fails.
    locator: crate::custody::RecoveryLocatorV1,
}

/// How a mapped checkout is governed — the discriminator the fail-closed deletion gate reads.
///
/// `WtEntry` is exactly the value that flows to the removal block in [`run_cleanup_flight`]:
/// every construction site copies into `WtState`, `entry_for_cleanup` clones it into
/// `state.entry`, and the removal block dereferences it. So a discriminator set at construction
/// is visible verbatim at the one place a checkout can be deleted.
///
/// All four production construction sites set [`Self::Legacy`] in slice 2b1 — the V3 writer and
/// its routing are 2b2's — so production behaviour is unchanged by the discriminator alone. What
/// is *not* unchanged is the disk arm of the gate, which applies to every entry regardless.
///
/// [`run_cleanup_flight`]: WorktreeBackend::run_cleanup_flight
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WtCustodyV1 {
    /// A V2 checkout: a `.meta.json` sidecar, no custody record, no protective state machine.
    /// Ordinary cleanup deletes it exactly as it did before this slice.
    Legacy,
    /// An R2f1b V3 checkout under custody. Never deleted here — 2a's
    /// `CustodySweepDispositionV1::authorizes_checkout_removal` is exhaustively false for every
    /// custody state, and the removal block inherits that refusal.
    Protected,
}

/// Why the removal block declined to delete a checkout.
///
/// Recorded rather than collapsed to a bool because the three arms mean operationally different
/// things: one is the in-memory discriminator, one is durable truth on disk contradicting it, and
/// one is the gate being unable to read durable truth at all.
#[derive(Clone, Debug, PartialEq, Eq)]
enum CheckoutRemovalRefusalV1 {
    /// The mapped entry is custody-discriminated.
    Discriminated,
    /// The entry says legacy but a V3 custody record exists beside the checkout. Durable truth
    /// wins: the record is the authority, the in-memory discriminator is a cache of it.
    RecordPresent,
    /// Durable truth could not be read. Fail closed.
    ProbeInconclusive(String),
    /// The checkout's publication cell is held by another actor, so a writer may be mid-transition
    /// on this very target. Fail closed: an uninspectable custody cell is unknown, and unknown
    /// never licenses deletion (§5.2).
    CellContended(String),
}

impl CheckoutRemovalRefusalV1 {
    fn reason(&self) -> &'static str {
        match self {
            Self::Discriminated => "entry is custody-discriminated",
            Self::RecordPresent => "a V3 custody record exists beside the checkout",
            Self::ProbeInconclusive(_) => "the custody record could not be read",
            Self::CellContended(_) => "the checkout publication cell is held by another owner",
        }
    }

    fn detail(&self) -> &str {
        match self {
            Self::Discriminated | Self::RecordPresent => "",
            Self::ProbeInconclusive(detail) | Self::CellContended(detail) => detail,
        }
    }
}

/// The gate's held window: the publication cell, taken with the REFUSING acquirer, spanning
/// probe → removal (slice 2b2, S7).
///
/// **Why this exists.** 2b1's gate probed for a record and then removed, with nothing serializing
/// the two: a writer publishing `ProtectionPrepared` inside that window makes the gate delete a
/// checkout that is protected on disk by the time the `remove` lands. That was deliberate in 2b1
/// (no writer existed) and is wrong the moment one does.
///
/// **Why the publication cell and not the custody cell.** The gate cannot name the custody cell —
/// the custody id lives inside the record, and the gate's discipline is presence-not-content, so
/// it must never depend on decoding one (a corrupt record must still protect). The canonical
/// target path is the only key both the writer and the gate certainly share.
///
/// This SUPERSEDES the 2b1 dual-review ledger's assignment of "hold the refusing custody lock
/// across probe→removal→settlement" to 2c1 (sol SMELL-1): the writer landing here is exactly what
/// made it due now. What remains 2c1's is holding it across *settlement*, which needs the typed
/// retained/refused disposition 2c1 mints.
struct CheckoutRemovalWindowV1 {
    _cell: crate::custody_lock::PublicationLockGuardV1,
}

impl CheckoutRemovalWindowV1 {
    /// `Ok(None)` means no cell is needed because the enclosing root does not exist: the checkout
    /// is gone, there is nothing for a writer to be publishing about, and refusing there would
    /// permanently wedge the ordinary legacy cleanup of an already-vanished worktree — the same
    /// ruling 2b1 made for `probe_custody_record_presence`'s missing-directory arm. Every other
    /// failure to enter the cell refuses.
    fn enter(entry: &WtEntry) -> Result<Option<Self>, CheckoutRemovalRefusalV1> {
        let Some(root) = Path::new(&entry.worktree_path).parent() else {
            return Err(CheckoutRemovalRefusalV1::ProbeInconclusive(format!(
                "worktree path has no enclosing root: {}",
                entry.worktree_path
            )));
        };
        // The root check must come BEFORE the cell attempt, not after it (repair R6). Entering
        // the cell goes through `liveness::open_persistent_lock_file`, which does
        // `create_dir_all(<root>/.custody-locks)` — so on a vanished root the old order RE-CREATED
        // the `[worktrees].root` tree from a TEARDOWN path, and the `Unavailable` arm it keyed the
        // `Ok(None)` answer on could never fire, because `create_dir_all` had already succeeded.
        // The documented vanished-root behaviour was therefore unreachable and the code did the
        // opposite of what it claimed.
        match root.try_exists() {
            // Root gone: the checkout is gone, there is nothing for a writer to be publishing
            // about, and refusing here would permanently wedge the ordinary legacy cleanup of an
            // already-vanished worktree — the same ruling 2b1 made for
            // `probe_custody_record_presence`'s missing-directory arm.
            Ok(false) => return Ok(None),
            Ok(true) => {}
            Err(error) => {
                return Err(CheckoutRemovalRefusalV1::ProbeInconclusive(format!(
                    "worktree root is unstattable: {error}"
                )))
            }
        }
        match crate::custody_lock::try_acquire_publication_lock_in(root, &entry.worktree_path) {
            Ok(cell) => Ok(Some(Self { _cell: cell })),
            Err(crate::custody_lock::CustodyLockRefusalV1::Contended(id)) => {
                Err(CheckoutRemovalRefusalV1::CellContended(id))
            }
            Err(crate::custody_lock::CustodyLockRefusalV1::Unavailable(_, error)) => {
                Err(CheckoutRemovalRefusalV1::ProbeInconclusive(format!(
                    "checkout publication cell is unavailable: {error}"
                )))
            }
        }
    }
}

/// The R2f1b fail-closed deletion gate. `None` authorizes removal; anything else refuses.
///
/// **Strength-independent and context-free by construction.** It takes only a `WtEntry`, so it
/// cannot consult a cleanup strength, a workflow outcome, or a cancellation cause even by
/// accident — which is the property `ExpiryClaim::Drop`, `BindingGuard::Drop`, and the idle
/// reaper need, since none of them has any of those things to offer.
///
/// **The discriminator/disk rule.** Durable truth is the record at
/// `custody_record_path(worktree_path)`; the discriminator is an in-memory cache of it. Removal
/// is authorized only when BOTH say unprotected, so the gate fails closed in either direction of
/// disagreement:
///
/// * discriminated but no record on disk (a crash between marking and publishing, or a record
///   an actor deleted) — refuse;
/// * legacy discriminator but a record present (a V3 checkout reached through a legacy code
///   path, or a discriminator lost across a restart) — refuse;
/// * the probe cannot answer — refuse.
///
/// The disk arm is what makes the gate survive a process restart, which the discriminator alone
/// cannot: after a crash the map is empty and every rebuilt entry is `Legacy`.
fn checkout_removal_refusal(entry: &WtEntry) -> Option<CheckoutRemovalRefusalV1> {
    if entry.custody == WtCustodyV1::Protected {
        return Some(CheckoutRemovalRefusalV1::Discriminated);
    }
    // Blocking filesystem calls, deliberately: the removal block a few lines below already does
    // blocking `std::fs::remove_file` on this same path, and a stat is strictly cheaper.
    match probe_custody_record_presence(&entry.worktree_path) {
        CustodyRecordPresenceV1::ProvablyAbsent => None,
        CustodyRecordPresenceV1::Present => Some(CheckoutRemovalRefusalV1::RecordPresent),
        CustodyRecordPresenceV1::Inconclusive(detail) => {
            Some(CheckoutRemovalRefusalV1::ProbeInconclusive(detail))
        }
    }
}

/// Run §5.1's `preserve_after_cancel` for one mapped entry. The barrier's whole implementation.
///
/// **Platform exclusion, restated exactly (risk R-10, slice 2c1 review opus S5).** Off unix,
/// `DirectoryIdentityV1` carries no `dev`/`ino`, so `identities_reverify` requires
/// `dev.is_some()` and is therefore **always false** — not "degraded", always false. Every
/// preservation on a non-unix host consequently settles `PreservationUnknown{AmbiguousCleanup}`
/// with a downgraded locator, and `Preserved` is unreachable there. That is MORE protective than
/// the unix behaviour, never less, but it is a real claim-quality cost: an R2f2 consumer on such a
/// host can never be told a checkout was cleanly preserved.
///
/// Returns `None` when the entry is not custody-governed — a V2 checkout has nothing to preserve,
/// and this is what keeps the barrier byte-identical for every legacy path.
///
/// Blocking work (two file locks, a pinned directory, stage/fsync/rename/parent-sync) is offloaded
/// per `custody_lock.rs`'s acquisition contract, exactly as the 2b2 writer does.
async fn preserve_entry_checkout(
    entry: &WtEntry,
    reason: PreservationReasonV1,
) -> PreservationOutcomeV1 {
    if entry.custody != WtCustodyV1::Protected {
        return PreservationOutcomeV1::Refused("checkout is not under R2f1b custody".to_string());
    }
    let Some(protection) = entry.protection.clone() else {
        // Protected by the discriminator, but the materialization evidence a claim is minted from
        // was never captured (or was lost with the process). Refusing is the only honest answer:
        // re-observing the objects now would mint a claim over whatever occupies those paths, and
        // that is precisely the substitution P7 exists to refuse. The checkout stays protected —
        // the gate refuses on the discriminator regardless of this answer.
        return PreservationOutcomeV1::Refused(
            "no retained materialization identities for this checkout".to_string(),
        );
    };
    let Some(worktree_root) = Path::new(&entry.worktree_path)
        .parent()
        .map(Path::to_path_buf)
    else {
        return PreservationOutcomeV1::Refused(format!(
            "worktree path has no enclosing root: {}",
            entry.worktree_path
        ));
    };
    let worktree_path = entry.worktree_path.clone();
    let created_wall_ms = wall_clock_ms();
    let joined = tokio::task::spawn_blocking(move || {
        let custodian = match WorktreeCustodianV1::enter(
            &worktree_root,
            &worktree_path,
            protection.binding.clone(),
        ) {
            Ok(custodian) => custodian,
            Err(error) => return PreservationOutcomeV1::from(error),
        };
        custodian.preserve_after_cancel(
            reason,
            &protection.identities,
            protection.locator,
            created_wall_ms,
        )
    })
    .await;
    match joined {
        Ok(outcome) => outcome,
        Err(error) => PreservationOutcomeV1::Refused(format!("preservation task failed: {error}")),
    }
}

/// What one capability removal settled on. Nothing here is a `Result`: a removal that did not
/// verifiably complete is not an error to propagate, it is a checkout that stayed on disk.
#[derive(Clone, Debug, PartialEq, Eq)]
enum CapabilityRemovalV1 {
    /// Minted, consumed, removed, tombstoned. The only arm that means the checkout is gone AND
    /// the record says so.
    Removed,
    /// Removed and verified gone, but the tombstone's publication outcome is unknown. The record
    /// is `DeleteAuthorized` or `Removed`; both are truthful about a checkout that no longer
    /// exists, and neither can lose work.
    RemovedRecordAmbiguous(String),
    /// The CAS refused, or the provider cannot consume a capability. Nothing was touched.
    MintRefused(String),
    /// The CAS's own publication was ambiguous, so no capability was minted and nothing was
    /// removed. The record is `LiveProtected` or `DeleteAuthorized` — protective either way.
    MintAmbiguous(String),
    /// Authority was minted and the removal did NOT verifiably complete. The record stays
    /// `DeleteAuthorized`, whose sweep disposition is `Recover`: the checkout is recovery-owned,
    /// and no re-mint is possible because the CAS refuses from that state.
    RemovalFailed(String),
}

impl CapabilityRemovalV1 {
    /// Is the checkout provably gone? Exhaustive so a later arm must be classified by decision.
    fn checkout_is_gone(&self) -> bool {
        match self {
            Self::Removed | Self::RemovedRecordAmbiguous(_) => true,
            Self::MintRefused(_) | Self::MintAmbiguous(_) | Self::RemovalFailed(_) => false,
        }
    }
}

/// §5.1's only automatic deletion path, end to end for one checkout.
///
/// The whole sequence runs with the custodian ALIVE, which means both custody cells are held from
/// before the CAS until after the tombstone. That is stronger than the gate's own removal window:
/// while it runs, every deletion-side caller takes the publication cell with the refusing acquirer
/// and fails closed, and no other writer can transition this record. It cannot deadlock — neither
/// `remove_v2` nor anything it calls takes a custody cell — and it is the same shape
/// `materialize_under_custody` already uses to hold the cells across `add_under_custody`.
///
/// Ordering, and why each step is where it is:
///
/// 1. **Preflight the provider capability before any record effect.** A provider on the refusing
///    `remove_v2` default would otherwise leave a durable `DeleteAuthorized` for a removal that
///    could never have started — recovery-owned forever. Same defect shape as 2b2's repair R4.
/// 2. **CAS + mint**, under the cells, off the async executor.
/// 3. **Revalidate**, on the caller's thread with NO await between it and the provider call, so
///    §5.1's "immediately before Git removal" is program order and not an aspiration. It is four
///    `openat`+`fstat` calls; the same blocking-work argument the deletion gate's own record probe
///    makes a few lines below applies verbatim.
/// 4. **`remove_v2`**, which re-checks §5.1's post-conditions (registration absent, target absent).
/// 5. **Tombstone only on a verified removal.** An `Err` from step 4 means "the checkout may still
///    be there", so no `Removed` may be written over it — that is P7's post-condition-disagreement
///    boundary, and it is enforced by the shape of the code, not by a message check.
async fn authorize_and_remove_checkout(
    provider: &Arc<dyn WorktreeProvider>,
    entry: &WtEntry,
    #[cfg(test)] removal_tombstone_parent_sync_fault: Arc<AtomicUsize>,
) -> CapabilityRemovalV1 {
    if entry.custody != WtCustodyV1::Protected {
        return CapabilityRemovalV1::MintRefused("checkout is not under R2f1b custody".to_string());
    }
    let Some(protection) = entry.protection.clone() else {
        // Protected by the discriminator, but the materialization evidence a capability is bound
        // to was never captured. Refusing is the only honest answer — the same fail-closed shape
        // the preservation barrier takes for a missing claim, and for the same reason: an
        // authority minted over identities we never observed authorizes a removal of whatever now
        // occupies those paths.
        return CapabilityRemovalV1::MintRefused(
            "no retained materialization identities for this checkout".to_string(),
        );
    };
    let Some(worktree_root) = Path::new(&entry.worktree_path)
        .parent()
        .map(Path::to_path_buf)
    else {
        return CapabilityRemovalV1::MintRefused(format!(
            "worktree path has no enclosing root: {}",
            entry.worktree_path
        ));
    };
    if !provider.supports_capability_removal() {
        return CapabilityRemovalV1::MintRefused(
            "worktree provider does not implement the R2f1b capability removal".to_string(),
        );
    }

    let worktree_path = entry.worktree_path.clone();
    let canonical_source = entry.canonical_source.clone();
    let minted = tokio::task::spawn_blocking({
        let worktree_path = worktree_path.clone();
        let canonical_source = canonical_source.clone();
        let binding = protection.binding.clone();
        let identities = protection.identities.clone();
        move || -> (Option<WorktreeCustodianV1>, DeletionAuthorizationV1) {
            let custodian =
                match WorktreeCustodianV1::enter(&worktree_root, &worktree_path, binding) {
                    Ok(custodian) => custodian,
                    Err(CustodyWriteRefusalV1::Ambiguous(detail)) => {
                        return (None, DeletionAuthorizationV1::Ambiguous(detail))
                    }
                    Err(other) => {
                        return (None, DeletionAuthorizationV1::Refused(other.to_string()))
                    }
                };
            let authorization = custodian.authorize_deletion(&canonical_source, &identities);
            (Some(custodian), authorization)
        }
    })
    .await;
    let (custodian, authorization) = match minted {
        Ok(minted) => minted,
        Err(error) => {
            return CapabilityRemovalV1::MintRefused(format!(
                "deletion authorization task failed: {error}"
            ))
        }
    };
    let capability = match authorization {
        DeletionAuthorizationV1::Authorized(capability) => capability,
        DeletionAuthorizationV1::Ambiguous(detail) => {
            return CapabilityRemovalV1::MintAmbiguous(detail)
        }
        DeletionAuthorizationV1::Refused(detail) => {
            return CapabilityRemovalV1::MintRefused(detail)
        }
    };
    let Some(custodian) = custodian else {
        return CapabilityRemovalV1::MintRefused(
            "a capability was minted without a custodian".to_string(),
        );
    };

    #[cfg(test)]
    if removal_tombstone_parent_sync_fault.swap(0, Ordering::SeqCst) != 0 {
        custodian.pinned_root().fail_sync_on_nth_call_for_test(1);
    }

    // NO AWAIT between here and the provider call: this is §5.1's "revalidates ... immediately
    // before Git removal".
    let authorized = match capability.revalidate_for_removal() {
        Ok(authorized) => authorized,
        Err(detail) => return CapabilityRemovalV1::RemovalFailed(detail),
    };
    if let Err(error) = provider.remove_v2(authorized).await {
        return CapabilityRemovalV1::RemovalFailed(format!("{error:?}"));
    }

    let recorded =
        tokio::task::spawn_blocking(move || custodian.record_removed(&protection.identities)).await;
    match recorded {
        Ok(RemovalRecordV1::Recorded) => CapabilityRemovalV1::Removed,
        Ok(RemovalRecordV1::Ambiguous(detail)) => {
            CapabilityRemovalV1::RemovedRecordAmbiguous(detail)
        }
        // The checkout IS gone — `remove_v2` proved target and registration absence — so this is
        // not a `RemovalFailed`. What is unknown is only whether the tombstone landed, and both
        // candidate records are truthful about an absent checkout.
        Ok(RemovalRecordV1::Refused(detail)) => CapabilityRemovalV1::RemovedRecordAmbiguous(detail),
        Err(error) => CapabilityRemovalV1::RemovedRecordAmbiguous(format!(
            "removal tombstone task failed: {error}"
        )),
    }
}

/// The DURABLE checkout disposition, re-derived from the record on disk (slice 2c2, P5).
///
/// **The defect this closes (2c1 review, opus W3, made binding on this slice).** A session's
/// disposition lives in its cleanup cell, and the reporter EVICTS that cell the moment a flight
/// reports `Ok` — which a gate refusal does. A later flight therefore starts from `Reclaim` and
/// reports `Retained` for a checkout whose record says `Preserved`. Once the cell is gone the
/// record is the authority, so every flight re-derives from it and takes the stronger of the two.
///
/// It returns `Preserve` for all three preserving states, prepared included: a stranded
/// `PreservationPrepared` is a preservation in progress (2c1 repair RA resumes it), and treating
/// it as anything else would let a mint request run against a checkout somebody is preserving.
fn durable_checkout_disposition(
    worktree_path: &str,
) -> (CheckoutDispositionV1, Option<WorktreeCustodyStateKindV1>) {
    let state = probe_custody_record_state(worktree_path);
    let disposition = match state {
        Some(kind) if kind.is_preserving() => CheckoutDispositionV1::Preserve,
        // No answer, or a state that asserts no preservation. `Reclaim` here means "this read adds
        // no knowledge" — it is the identity of the `max` below, never a downgrade: the caller
        // takes the STRONGER of this and the cell's own disposition.
        _ => CheckoutDispositionV1::Reclaim,
    };
    (disposition, state)
}

/// Map a custody write refusal onto the backend's error vocabulary.
///
/// An AMBIGUOUS publication is deliberately reported as a configure failure, not swallowed: the
/// record may or may not carry the new state, so the configure cannot be attested. It is not a
/// licence to delete anything — the checkout's protection is the record on disk, which the
/// deletion gate reads independently of this return value.
fn custody_write_error(refusal: CustodyWriteRefusalV1) -> BridgeError {
    BridgeError::ConfigInvalid {
        reason: format!("R2f1b custody transition: {refusal}"),
    }
}

fn wall_clock_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .and_then(|elapsed| i64::try_from(elapsed.as_millis()).ok())
        .unwrap_or(0)
}

/// What a cleanup request wants done with the CHECKOUT, as opposed to how hard it tears down the
/// session (that is [`CleanupStrength`]).
///
/// **The join key defect this closes (R-8 / risk R-7, prospective until this slice).** Before 2c1
/// the single-flight join key was `(session cell, strength ordering)` and nothing else, because
/// both strengths reached the same unconditional removal — so there was no second axis to get
/// wrong. The moment a preservation request exists, an equal-strength request of the OTHER
/// disposition would join the in-flight one and be handed its report: a preserve request could be
/// told "done" by a flight that removed the checkout, and a reclaim request could be told "done"
/// by a flight that deliberately kept it.
///
/// `Ord` is the monotonic rule: `Preserve` dominates. Once a session's checkout disposition is
/// preservation, no later request downgrades it (§5.1: "once a preserved claim exists, only
/// R2f2's explicit local retain/archive/delete disposition can clear it; no later healthy
/// projection or TTL can mint deletion authority").
/// **The `Ord` is the whole safety property, and slice 2c2's third variant is placed inside it
/// deliberately.** `Reclaim < DeleteAuthorized < Preserve`. Design note 2 of the 2c1 handoff
/// anticipated exactly this variant and warned that enum equality across generations becomes
/// accidentally true once a third disposition exists, which is why the epoch is carried on the
/// flight slot and re-checked at the mint (P5).
///
/// `Preserve` dominating `DeleteAuthorized` is §5.1's monotonicity in the in-memory half: a
/// workflow-level mint REQUEST cannot lower a checkout whose disposition is already preservation,
/// because `raise_checkout_disposition` only ever moves upward — so the flight that would mint is
/// never even started for a preserved checkout. The durable half is independent and redundant:
/// `WorktreeCustodianV1::authorize_deletion` refuses from every from-state but `LiveProtected`.
/// Two mechanisms, either one sufficient.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Default)]
enum CheckoutDispositionV1 {
    /// The ordinary teardown request: remove the checkout if — and only if — the fail-closed
    /// deletion gate authorizes it. This is every pre-2c1 caller, unchanged.
    #[default]
    Reclaim,
    /// §5.1's globally-healthy workflow outcome (slice 2c2): the flight may mint a
    /// `DeletionCapabilityV1` and consume it. Raised ONLY by
    /// `settle_workflow_checkout_v1(GloballyHealthy)`; no context-free caller can reach it.
    DeleteAuthorized,
    /// §5.1's non-success exit: preserve the checkout durably, and never remove it.
    Preserve,
}

/// What a cleanup flight actually did with the checkout.
///
/// **This is the typed retained/refused disposition the 2b1 dual-review ledger made binding on
/// 2c1 (sol-1 / D-1).** Until it existed, a gate refusal returned a bare `Ok` and every caller in
/// the R-11 fan-in read it as "the checkout is gone". `Removed` and
/// `RemovedRecordAmbiguous` are the only arms that mean that; the latter never asserts a durable
/// tombstone.
#[derive(Clone, Debug, PartialEq, Eq)]
enum CheckoutCleanupDispositionV1 {
    /// No mapped checkout for this session; there was nothing to dispose of.
    NotNeeded,
    /// The checkout was removed and its sidecar cleared — the pre-2c1 meaning of a clean report.
    Removed,
    /// The checkout is gone, but syncing the `Removed` tombstone to its parent was ambiguous.
    /// The map can be cleared; only durable-record evidence remains unknown.
    RemovedRecordAmbiguous(String),
    /// A removal was authorized and did not complete. The flight's `result` carries the error.
    RemovalFailed,
    /// Deliberately retained: the fail-closed deletion gate refused on custody evidence, or could
    /// not read durable truth. NOT a clean completion, and never to be projected as one.
    Retained,
    /// Deliberately retained AND durably preserved by this slice's §5.1 barrier.
    Preserved,
}

impl CheckoutCleanupDispositionV1 {
    fn backend_disposition(&self) -> BackendCleanupDispositionV1 {
        match self {
            Self::NotNeeded | Self::Removed => BackendCleanupDispositionV1::Complete,
            Self::Retained => BackendCleanupDispositionV1::Retained,
            Self::Preserved => BackendCleanupDispositionV1::Preserved,
            Self::RemovedRecordAmbiguous(_) | Self::RemovalFailed => {
                BackendCleanupDispositionV1::Unknown
            }
        }
    }

    /// The static teardown transition code this disposition publishes on a successful flight.
    ///
    /// A distinct code rather than a distinct `PhaseStatus`: the session teardown genuinely did
    /// complete (the gate's refusal is scoped to the checkout, and a real inner failure is still
    /// reported as `Failed`), so `PhaseStatus::Skipped` would assert that no teardown happened,
    /// which is false. What was skipped is the checkout removal, and that is what the code names.
    fn completed_code(self, strength: CleanupStrength) -> &'static str {
        match self {
            Self::NotNeeded | Self::Removed | Self::RemovalFailed => strength.transition_codes().1,
            Self::RemovedRecordAmbiguous(_) => "worktree.teardown.removed_record_ambiguous",
            Self::Retained => "worktree.teardown.retained",
            Self::Preserved => "worktree.teardown.preserved",
        }
    }
}

/// One cleanup flight's answer: the teardown result AND what became of the checkout.
#[derive(Clone, Debug)]
struct CleanupReportV1 {
    result: Result<BackendCleanupDispositionV1, BridgeError>,
    checkout: CheckoutCleanupDispositionV1,
}

impl CleanupReportV1 {
    fn ok(checkout: CheckoutCleanupDispositionV1) -> Self {
        Self::settled(checkout, BackendCleanupDispositionV1::Complete, None)
    }

    fn settled(
        checkout: CheckoutCleanupDispositionV1,
        inner: BackendCleanupDispositionV1,
        error: Option<BridgeError>,
    ) -> Self {
        let result = match error {
            Some(error) => Err(error),
            None => Ok(inner.combine(checkout.backend_disposition())),
        };
        Self { result, checkout }
    }

    fn is_ok(&self) -> bool {
        self.result.is_ok()
    }
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
    inner_disposition: BackendCleanupDispositionV1,
    provider_removed: bool,
    sidecar_removed: bool,
    entry: Option<WtEntry>,
}

struct CleanupCell {
    state: Mutex<CleanupCellState>,
    flight: StdMutex<Option<CleanupFlightSlot>>,
    lifecycle: StdMutex<CleanupLifecycle>,
    /// Serializes a preservation raise with the deletion generation validation and its subsequent
    /// `LiveProtected -> DeleteAuthorized` mint. The lifecycle mutex alone cannot do that: it is
    /// intentionally released before the async custody-cell admission, leaving a preserve writer
    /// able to change the epoch in the old check-to-CAS window.
    deletion_admission: Mutex<()>,
    #[cfg(test)]
    deletion_admission_barrier: Arc<DeletionAdmissionBarrier>,
    #[cfg(test)]
    removal_projection_barrier: Arc<DeletionAdmissionBarrier>,
    configure_settled: Notify,
}

struct CleanupFlightSlot {
    id: u64,
    strength: CleanupStrength,
    /// The checkout disposition this flight is serving. Half of the join key (slice 2c1, P3).
    disposition: CheckoutDispositionV1,
    /// The monotonic identity of that disposition, minted from the backend-wide counter the
    /// instant the cell's disposition last CHANGED. Redundant with `disposition` today by
    /// construction — only a strict upgrade mints a new epoch — and kept anyway so the invariant
    /// "a flight serves exactly the disposition generation it was started for" is asserted rather
    /// than inferred, and so a future third disposition cannot make equality accidental.
    disposition_epoch: u64,
    report: CleanupReportReceiver,
    #[cfg(test)]
    joined_waiters: u64,
}

type CleanupReportReceiver = watch::Receiver<Option<CleanupReportV1>>;
type CleanupFlightHandle = (CleanupStrength, CleanupReportReceiver);

#[derive(Default)]
struct CleanupLifecycle {
    configuring: u64,
    active_configures: HashSet<u64>,
    configured: bool,
    cleanup_started: bool,
    failed_configure_cleanup_pending: bool,
    /// The cell's monotonic checkout disposition and its epoch (slice 2c1, P3).
    ///
    /// It lives in the LIFECYCLE mutex, not in `CleanupCellState`, because the join decision is
    /// made in `start_or_join_cleanup`, which is deliberately synchronous — it publishes the
    /// flight before the caller's first await so that dropping the report receiver detaches
    /// rather than cancels. `CleanupCellState` is behind an async mutex and is unreachable there.
    checkout_disposition: CheckoutDispositionV1,
    disposition_epoch: u64,
    /// Why preservation was requested, retained so the flight-side barrier can RETRY a
    /// preservation whose first attempt was ambiguous or refused, with the caller's original
    /// trigger rather than a guess.
    preservation_reason: Option<crate::custody::PreservationReasonV1>,
}

#[cfg(test)]
#[derive(Default)]
struct DeletionAdmissionBarrier {
    armed: AtomicBool,
    checked: Notify,
    proceed: Notify,
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
            #[cfg(test)]
            deletion_admission_barrier: Arc::new(DeletionAdmissionBarrier::default()),
            #[cfg(test)]
            removal_projection_barrier: Arc::new(DeletionAdmissionBarrier::default()),
            deletion_admission: Mutex::new(()),
            configure_settled: Notify::new(),
        }
    }

    #[cfg(test)]
    async fn pause_deletion_mint_for_test(&self) {
        if self
            .deletion_admission_barrier
            .armed
            .swap(false, Ordering::SeqCst)
        {
            self.deletion_admission_barrier.checked.notify_one();
            self.deletion_admission_barrier.proceed.notified().await;
        }
    }

    // 3s repair R1: pauses AFTER the removal + tombstone, BEFORE the map projection — the window
    // both review lenses named. Discriminates the deletion-admission guard's scope: with the guard
    // held across the projection a queued preservation writer stays blocked here; with the old
    // block-scoped guard it would run against the stale map entry.
    #[cfg(test)]
    async fn pause_removal_projection_for_test(&self) {
        if self
            .removal_projection_barrier
            .armed
            .swap(false, Ordering::SeqCst)
        {
            self.removal_projection_barrier.checked.notify_one();
            self.removal_projection_barrier.proceed.notified().await;
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
    /// Claimed flights outlive an observing configure future. A normal terminal publication
    /// releases an entry; a terminal-publication debt remains joinable until cleanup or retirement
    /// reports it.
    preparation_flights: Arc<StdMutex<HashMap<String, Arc<ActivePreparationFlightV1>>>>,
    /// A pre-effect transfer moves the exact active owner here. T3 inventories/recovers these
    /// handles; this slice deliberately supplies no production consumer.
    preparation_recovery_flights:
        Arc<StdMutex<HashMap<String, Arc<TransferredPreparationFlightV1>>>>,
    preparation_control_root: Arc<PreparationControlRootV1>,
    #[cfg(test)]
    preparation_test_hooks: Arc<PreparationFlightTestHooks>,
    #[cfg(test)]
    preparation_test_bound: Arc<StdMutex<Option<PreparationBoundV1>>>,
    cleanup_cells: Arc<StdMutex<HashMap<String, Arc<CleanupCell>>>>,
    sealed: Arc<AtomicBool>,
    configure_inflight: AtomicU64,
    configure_settled: Notify,
    next_claim: AtomicU64,
    next_configure_admission: AtomicU64,
    next_cleanup_flight: Arc<AtomicU64>,
    /// Mints the monotonic custody-disposition identity stamped on a cleanup flight (slice 2c1,
    /// P3). Backend-wide rather than per cell so the ordering between two sessions' disposition
    /// changes is total and reportable, which is what makes a wrong-join reproducible in a test
    /// rather than merely improbable.
    next_checkout_disposition: Arc<AtomicU64>,
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
    cleanup_waiting_preparation_count: Arc<AtomicU64>,
    #[cfg(test)]
    cleanup_waiting_preparation: Arc<Notify>,
    #[cfg(test)]
    cleanup_flight_started_count: AtomicU64,
    #[cfg(test)]
    cleanup_flight_started: Notify,
    #[cfg(test)]
    configure_admitted: Notify,
    #[cfg(test)]
    failed_configure_retry_now: Arc<Notify>,
    // Test-only arming for the existing `fs_custody` parent-sync fault seam.
    // It is consumed after the authorizing replace, so the tombstone parent sync is the
    // exact publication operation that fails.
    #[cfg(test)]
    removal_tombstone_parent_sync_fault: Arc<AtomicUsize>,
}

impl WorktreeBackend {
    pub fn new(
        inner: Arc<dyn AgentBackend>,
        provider: Arc<dyn WorktreeProvider>,
        cfg: WorktreeConfig,
        allowed_root: Option<SessionCwd>,
        identity: WorktreeIdentity,
    ) -> Self {
        #[cfg(test)]
        let preparation_test_hooks = Arc::new(PreparationFlightTestHooks::default());
        let preparation_control_root = Arc::new(PreparationControlRootV1::new(
            PathBuf::from(&cfg.root),
            #[cfg(test)]
            preparation_test_hooks.clone(),
        ));
        Self {
            inner,
            provider,
            cfg,
            allowed_root,
            identity,
            next_checkout_disposition: Arc::new(AtomicU64::new(1)),
            preparation_recovery_flights: Arc::new(StdMutex::new(HashMap::new())),
            preparation_control_root,
            #[cfg(test)]
            preparation_test_bound: Arc::new(StdMutex::new(None)),
            map: Arc::new(Mutex::new(HashMap::new())),
            preparation_flights: Arc::new(StdMutex::new(HashMap::new())),
            #[cfg(test)]
            preparation_test_hooks,
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
            cleanup_waiting_preparation_count: Arc::new(AtomicU64::new(0)),
            #[cfg(test)]
            cleanup_waiting_preparation: Arc::new(Notify::new()),
            #[cfg(test)]
            cleanup_flight_started_count: AtomicU64::new(0),
            #[cfg(test)]
            cleanup_flight_started: Notify::new(),
            #[cfg(test)]
            configure_admitted: Notify::new(),
            #[cfg(test)]
            failed_configure_retry_now: Arc::new(Notify::new()),
            #[cfg(test)]
            removal_tombstone_parent_sync_fault: Arc::new(AtomicUsize::new(0)),
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
    async fn wait_for_cleanup_waiting_preparation(&self) {
        tokio::time::timeout(Duration::from_secs(2), async {
            while self
                .cleanup_waiting_preparation_count
                .load(Ordering::SeqCst)
                == 0
            {
                self.cleanup_waiting_preparation.notified().await;
            }
        })
        .await
        .expect("cleanup test timed out waiting to join a committed preparation flight");
    }

    #[cfg(test)]
    fn preparation_flight_debt_for_test(&self, session: &SessionId) -> Option<BridgeError> {
        let owner = self
            .preparation_flights
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .get(session.as_str())
            .cloned()?;
        let completion = owner.completion();
        let completed = completion.borrow().clone();
        completed.and_then(Result::err)
    }

    #[cfg(test)]
    fn arm_preparation_bound_for_test(&self, clock: PreparationClockV1) {
        *self
            .preparation_test_bound
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = Some(PreparationBoundV1 { clock });
    }

    #[cfg(test)]
    fn preparation_guard_for_test(
        &self,
        session: &SessionId,
    ) -> Option<Arc<MaterializationPreparationFlightV1>> {
        self.preparation_flights
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .get(session.as_str())
            .map(|owner| owner.flight.clone())
    }

    #[cfg(test)]
    async fn observe_preparation_bound_for_test(&self, session: &SessionId) -> bool {
        let Some(owner) = self
            .preparation_flights
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .get(session.as_str())
            .cloned()
        else {
            return false;
        };
        let Some((operation, reason)) = owner.flight.expired_pre_barrier() else {
            return false;
        };
        transfer_preparation_flight(
            &self.preparation_flights,
            &self.preparation_recovery_flights,
            session.as_str(),
            &owner,
            operation,
            reason,
        )
        .await
        .expect("test transfer publishes its durable terminal record")
    }

    #[cfg(test)]
    fn transferred_preparation_for_test(
        &self,
        session: &SessionId,
    ) -> Option<Arc<TransferredPreparationFlightV1>> {
        self.preparation_recovery_flights
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .get(session.as_str())
            .cloned()
    }

    #[cfg(test)]
    async fn join_transferred_preparation_runner_for_test(&self, session: &SessionId) {
        let recovery = self
            .transferred_preparation_for_test(session)
            .expect("the transferred owner remains recovery-inventoriable");
        let runner = recovery
            .owner
            .runner
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .take()
            .expect("recovery owns the exact runner handle");
        runner
            .await
            .expect("the released preparation runner must not panic");
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

    /// Mark a mapped checkout custody-discriminated.
    ///
    /// Test-only on purpose: in slice 2b1 no production path constructs a protected entry (the
    /// writer and its routing are 2b2's), so the discriminator arm of the gate would otherwise be
    /// unreachable and untested until then. Tests that need the *disk* arm instead write a real
    /// custody record beside the checkout and leave the discriminator legacy.
    #[cfg(test)]
    async fn mark_checkout_protected_for_test(&self, session: &SessionId) {
        let mut map = self.map.lock().await;
        match map.get_mut(session.as_str()) {
            Some(WtState::Ready(entry))
            | Some(WtState::Reserving { entry, .. })
            | Some(WtState::Retained { entry, .. }) => {
                entry.custody = WtCustodyV1::Protected;
            }
            None => panic!("no worktree entry is mapped for {}", session.as_str()),
        }
    }

    /// The inverse: force a mapped checkout back to the legacy discriminator.
    ///
    /// Test-only, and needed to ISOLATE the S7 publication-cell arm of the gate. The gate fails
    /// closed on three independent grounds; a test that wants to prove the cell arm works must
    /// first remove the other two, or it cannot tell which one refused.
    #[cfg(test)]
    async fn mark_entry_legacy_for_test(&self, session: &SessionId) {
        let mut map = self.map.lock().await;
        match map.get_mut(session.as_str()) {
            Some(WtState::Ready(entry))
            | Some(WtState::Reserving { entry, .. })
            | Some(WtState::Retained { entry, .. }) => {
                entry.custody = WtCustodyV1::Legacy;
            }
            None => panic!("no worktree entry is mapped for {}", session.as_str()),
        }
    }

    #[cfg(test)]
    async fn mapped_worktree_path_for_test(&self, session: &SessionId) -> Option<String> {
        match self.map.lock().await.get(session.as_str()) {
            Some(WtState::Ready(entry))
            | Some(WtState::Reserving { entry, .. })
            | Some(WtState::Retained { entry, .. }) => Some(entry.worktree_path.clone()),
            None => None,
        }
    }

    #[cfg(test)]
    fn cleanup_cell_count(&self) -> usize {
        self.cleanup_cells
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .len()
    }

    #[cfg(test)]
    fn fail_next_capability_tombstone_parent_sync_for_test(&self) {
        self.removal_tombstone_parent_sync_fault
            .store(1, Ordering::SeqCst);
    }

    #[cfg(test)]
    fn arm_deletion_admission_barrier_for_test(
        &self,
        session: &SessionId,
    ) -> Arc<DeletionAdmissionBarrier> {
        let barrier = self.cleanup_cells.lock().unwrap()[session.as_str()]
            .deletion_admission_barrier
            .clone();
        barrier.armed.store(true, Ordering::SeqCst);
        barrier
    }

    #[cfg(test)]
    fn arm_removal_projection_barrier_for_test(
        &self,
        session: &SessionId,
    ) -> Arc<DeletionAdmissionBarrier> {
        let barrier = self.cleanup_cells.lock().unwrap()[session.as_str()]
            .removal_projection_barrier
            .clone();
        barrier.armed.store(true, Ordering::SeqCst);
        barrier
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
            // CLONE, never pop — exactly like `Ready`. A retained entry is this checkout's last
            // in-memory owner; popping it would reproduce the very defect the state exists to fix.
            Some(WtState::Retained { entry, .. }) => Some(entry.clone()),
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
    ) -> Result<BackendCleanupDispositionV1, BridgeError> {
        self.cleanup_session_with_sealed_admission(session, strength, false)
            .await
    }

    /// Raise a session's checkout disposition MONOTONICALLY and mint a fresh epoch when it
    /// actually changes. The only writer of `CleanupLifecycle::checkout_disposition`.
    ///
    /// Returns `None` when the backend is sealed and the session has no cell — retirement is
    /// already draining, and creating a cell there would resurrect one the sealing just retired.
    async fn raise_checkout_disposition(
        &self,
        session: &SessionId,
        disposition: CheckoutDispositionV1,
        reason: Option<PreservationReasonV1>,
    ) -> Option<Arc<CleanupCell>> {
        let cell = {
            let mut cells = self
                .cleanup_cells
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            match cells.get(session.as_str()).cloned() {
                Some(cell) => cell,
                None if self.sealed.load(Ordering::SeqCst) => return None,
                None => {
                    let cell = Arc::new(CleanupCell::new());
                    cells.insert(session.as_str().to_owned(), cell.clone());
                    cell
                }
            }
        };
        let deletion_admission = cell.deletion_admission.lock().await;
        {
            let mut lifecycle = cell
                .lifecycle
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            if disposition > lifecycle.checkout_disposition {
                lifecycle.checkout_disposition = disposition;
                lifecycle.disposition_epoch = self
                    .next_checkout_disposition
                    .fetch_add(1, Ordering::SeqCst);
            }
            if lifecycle.preservation_reason.is_none() {
                lifecycle.preservation_reason = reason;
            }
        }
        drop(deletion_admission);
        Some(cell)
    }

    /// Is the cell's checkout disposition still exactly the generation this flight was started
    /// for? (slice 2c2, P5 second half.)
    ///
    /// **This is the epoch earning its keep.** `start_or_join_cleanup` reads the disposition
    /// synchronously and the flight then awaits — the configure drain, the per-session state
    /// mutex, the inner teardown — so a `Preserve` raised in that window would otherwise be
    /// invisible to a flight already carrying `DeleteAuthorized`. Re-reading `(disposition,
    /// epoch)` immediately before the mint turns "the state mutex happens to serialize them" into
    /// a checked invariant. Comparing the epoch as well as the enum is what makes it hold once a
    /// third disposition exists: with `Preserve` raised and then the cell evicted and rebuilt,
    /// enum equality alone can be true across two different generations.
    fn deletion_generation_is_current(
        cell: &CleanupCell,
        disposition: CheckoutDispositionV1,
        disposition_epoch: u64,
    ) -> bool {
        let lifecycle = cell
            .lifecycle
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        lifecycle.checkout_disposition == disposition
            && lifecycle.disposition_epoch == disposition_epoch
    }

    async fn cleanup_session_with_sealed_admission(
        &self,
        session: &SessionId,
        strength: CleanupStrength,
        allow_new_when_sealed: bool,
    ) -> Result<BackendCleanupDispositionV1, BridgeError> {
        self.cleanup_session_reported(session, strength, allow_new_when_sealed)
            .await
            .result
    }

    /// The disposition-bearing cleanup entry. `cleanup_session_with_sealed_admission` is the
    /// result-only projection of it, kept so the ~40 existing call sites are untouched.
    async fn cleanup_session_reported(
        &self,
        session: &SessionId,
        strength: CleanupStrength,
        allow_new_when_sealed: bool,
    ) -> CleanupReportV1 {
        let requested = strength;
        loop {
            let Some((flight_strength, report)) =
                self.start_or_join_cleanup(session, requested, allow_new_when_sealed)
            else {
                return CleanupReportV1::ok(CheckoutCleanupDispositionV1::NotNeeded);
            };
            let report = wait_for_cleanup_report(report).await;
            match &report.result {
                Err(_) => return report,
                Ok(_) if flight_strength >= requested => return report,
                Ok(_) => {
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
        // The checkout disposition is read (never lowered) from the cell under its lifecycle
        // lock, so a flight always serves the cell's CURRENT generation. Only
        // `raise_checkout_disposition` writes it, and only upward.
        let (disposition, disposition_epoch, preservation_reason) = {
            let lifecycle = cell
                .lifecycle
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            (
                lifecycle.checkout_disposition,
                lifecycle.disposition_epoch,
                lifecycle.preservation_reason,
            )
        };
        let mut slot = cell
            .flight
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let mut requested = requested;
        if let Some(existing) = slot.as_mut() {
            let completed = existing.report.borrow().clone();
            if completed.as_ref().is_some_and(|report| !report.is_ok()) {
                requested = requested.max(existing.strength);
            }
            // The disposition half of the join key (P3). A flight may only be joined by a request
            // of the SAME disposition generation: serving a preserve request from a reclaim
            // flight (or the reverse) hands the caller an answer about work that was not done.
            let same_disposition = existing.disposition == disposition
                && existing.disposition_epoch == disposition_epoch;
            let reusable = same_disposition
                && match &completed {
                    None => existing.strength >= requested,
                    Some(report) if report.is_ok() => existing.strength >= requested,
                    Some(_) => false,
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
        let preparation_flights = self.preparation_flights.clone();
        let worker_session = session.clone();
        let session_key = session.as_str().to_owned();
        let flight_id = self.next_cleanup_flight.fetch_add(1, Ordering::Relaxed);
        let next_cleanup_flight = self.next_cleanup_flight.clone();
        #[cfg(test)]
        let cleanup_waiting_reservation_count = self.cleanup_waiting_reservation_count.clone();
        #[cfg(test)]
        let cleanup_waiting_reservation = self.cleanup_waiting_reservation.clone();
        #[cfg(test)]
        let cleanup_waiting_preparation_count = self.cleanup_waiting_preparation_count.clone();
        #[cfg(test)]
        let cleanup_waiting_preparation = self.cleanup_waiting_preparation.clone();
        #[cfg(test)]
        let failed_configure_retry_now = self.failed_configure_retry_now.clone();
        #[cfg(test)]
        let removal_tombstone_parent_sync_fault = self.removal_tombstone_parent_sync_fault.clone();
        let (report_tx, report_rx) = watch::channel(None);
        *slot = Some(CleanupFlightSlot {
            id: flight_id,
            strength: requested,
            disposition,
            disposition_epoch,
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
            let preparation_flights = preparation_flights.clone();
            let worker_session = worker_session.clone();
            #[cfg(test)]
            let cleanup_waiting_reservation_count = cleanup_waiting_reservation_count.clone();
            #[cfg(test)]
            let cleanup_waiting_reservation = cleanup_waiting_reservation.clone();
            #[cfg(test)]
            let cleanup_waiting_preparation_count = cleanup_waiting_preparation_count.clone();
            #[cfg(test)]
            let cleanup_waiting_preparation = cleanup_waiting_preparation.clone();
            #[cfg(test)]
            let removal_tombstone_parent_sync_fault = removal_tombstone_parent_sync_fault.clone();
            async move {
                Self::run_cleanup_flight(
                    worker_cell,
                    inner,
                    provider,
                    map,
                    notify,
                    preparation_flights,
                    worker_session,
                    requested,
                    disposition,
                    disposition_epoch,
                    preservation_reason,
                    #[cfg(test)]
                    cleanup_waiting_reservation_count,
                    #[cfg(test)]
                    cleanup_waiting_reservation,
                    #[cfg(test)]
                    cleanup_waiting_preparation_count,
                    #[cfg(test)]
                    cleanup_waiting_preparation,
                    #[cfg(test)]
                    removal_tombstone_parent_sync_fault,
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
                    Err(_) => CleanupReportV1 {
                        result: Err(BridgeError::agent_crashed("worktree cleanup task failed")),
                        checkout: CheckoutCleanupDispositionV1::RemovalFailed,
                    },
                };
                let retry_failed_configure = !report.is_ok()
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
                // The RETRY state carries the disposition too (P3). Re-reading the cell rather
                // than reusing this loop's captured value is the point: a preservation raised
                // while a failed-configure retry was sleeping must govern the next attempt, and
                // hardcoding `Reclaim` here would silently downgrade a preserved checkout on the
                // one path that re-spawns a flight without any caller.
                let (retry_disposition, retry_epoch, retry_reason) = {
                    let lifecycle = reporter_cell
                        .lifecycle
                        .lock()
                        .unwrap_or_else(|error| error.into_inner());
                    (
                        lifecycle.checkout_disposition,
                        lifecycle.disposition_epoch,
                        lifecycle.preservation_reason,
                    )
                };
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
                        disposition: retry_disposition,
                        disposition_epoch: retry_epoch,
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
                    let preparation_flights = preparation_flights.clone();
                    let worker_session = worker_session.clone();
                    #[cfg(test)]
                    let cleanup_waiting_reservation_count =
                        cleanup_waiting_reservation_count.clone();
                    #[cfg(test)]
                    let cleanup_waiting_reservation = cleanup_waiting_reservation.clone();
                    #[cfg(test)]
                    let cleanup_waiting_preparation_count =
                        cleanup_waiting_preparation_count.clone();
                    #[cfg(test)]
                    let cleanup_waiting_preparation = cleanup_waiting_preparation.clone();
                    #[cfg(test)]
                    let removal_tombstone_parent_sync_fault =
                        removal_tombstone_parent_sync_fault.clone();
                    async move {
                        Self::run_cleanup_flight(
                            worker_cell,
                            inner,
                            provider,
                            map,
                            notify,
                            preparation_flights,
                            worker_session,
                            CleanupStrength::Release,
                            retry_disposition,
                            retry_epoch,
                            retry_reason,
                            #[cfg(test)]
                            cleanup_waiting_reservation_count,
                            #[cfg(test)]
                            cleanup_waiting_reservation,
                            #[cfg(test)]
                            cleanup_waiting_preparation_count,
                            #[cfg(test)]
                            cleanup_waiting_preparation,
                            #[cfg(test)]
                            removal_tombstone_parent_sync_fault,
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
    ) -> Result<BackendCleanupDispositionV1, BridgeError> {
        let (started_code, _completed_code, failed_code) = strength.transition_codes();
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
        let report = match cleanup {
            Some((flight_strength, report)) => {
                let report = wait_for_cleanup_report(report).await;
                if report.is_ok() && flight_strength < strength {
                    self.cleanup_session_reported(session, strength, false)
                        .await
                } else {
                    report
                }
            }
            None => CleanupReportV1::ok(CheckoutCleanupDispositionV1::NotNeeded),
        };
        started_observation?;
        // The typed disposition's FIRST truthful projection (2b1 sol-1 / D-1). Before this slice a
        // gate refusal published `worktree.teardown.released` — the same bytes a real removal
        // publishes — so no observer anywhere could distinguish "cleaned" from "deliberately
        // retained". The terminal code now names what happened to the checkout.
        let result = report.result.clone();
        let (status, terminal_code) = if report.is_ok() {
            (
                bridge_core::diagnostics::PhaseStatus::Completed,
                report.checkout.completed_code(strength),
            )
        } else {
            (bridge_core::diagnostics::PhaseStatus::Failed, failed_code)
        };
        let observation = record_cleanup_transition(observer.as_ref(), status, terminal_code).await;
        match (result, observation) {
            (Err(primary), _) => Err(primary),
            (Ok(_), Err(observation)) => Err(observation),
            (Ok(disposition), Ok(())) => Ok(disposition),
        }
    }

    #[allow(clippy::too_many_arguments)]
    async fn run_cleanup_flight(
        cell: Arc<CleanupCell>,
        inner: Arc<dyn AgentBackend>,
        provider: Arc<dyn WorktreeProvider>,
        map: Arc<Mutex<HashMap<String, WtState>>>,
        notify: Arc<Notify>,
        preparation_flights: Arc<StdMutex<HashMap<String, Arc<ActivePreparationFlightV1>>>>,
        session: SessionId,
        strength: CleanupStrength,
        disposition: CheckoutDispositionV1,
        disposition_epoch: u64,
        preservation_reason: Option<PreservationReasonV1>,
        #[cfg(test)] cleanup_waiting_reservation_count: Arc<AtomicU64>,
        #[cfg(test)] cleanup_waiting_reservation: Arc<Notify>,
        #[cfg(test)] cleanup_waiting_preparation_count: Arc<AtomicU64>,
        #[cfg(test)] cleanup_waiting_preparation: Arc<Notify>,
        #[cfg(test)] removal_tombstone_parent_sync_fault: Arc<AtomicUsize>,
    ) -> CleanupReportV1 {
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

        // A false cancellation sample commits the detached runner before it enters the
        // custody cells. Its reservation carries the only retained identities that can later
        // mint a preservation claim, so cleanup must join that runner before `entry_for_cleanup`
        // can pop the reservation. A pre-sample departure is deliberately not joined: it cannot
        // have started a custody or provider effect.
        let preparation_completion = {
            let flights = preparation_flights
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            flights.get(session.as_str()).and_then(|owner| {
                let completion = owner.completion();
                if owner.flight.committed()
                    || completion.borrow().as_ref().is_some_and(Result::is_err)
                {
                    Some(completion)
                } else {
                    None
                }
            })
        };
        if let Some(completion) = preparation_completion {
            #[cfg(test)]
            {
                cleanup_waiting_preparation_count.fetch_add(1, Ordering::SeqCst);
                cleanup_waiting_preparation.notify_waiters();
            }
            if let Err(error) = wait_for_preparation_completion(completion).await {
                let state = cell.state.lock().await;
                return CleanupReportV1::settled(
                    CheckoutCleanupDispositionV1::NotNeeded,
                    state.inner_disposition,
                    Some(error),
                );
            }
        }

        // This mutex is the per-session single-flight boundary. A stronger
        // release waits for an in-flight forget, then performs only the missing
        // stronger inner step; concurrent equal requests join the completed
        // component state.
        let mut state = cell.state.lock().await;
        if let Err(error) = inner.resource_flight_v1() {
            return CleanupReportV1::settled(
                CheckoutCleanupDispositionV1::NotNeeded,
                state.inner_disposition,
                Some(error),
            );
        }
        let mut first_error = None;
        // A reserving configure may not have invoked the inner backend yet. Let
        // it publish Ready (or remove its failed reservation) before teardown,
        // otherwise configure could resurrect inner state after release.
        if state.entry.is_none() {
            state.entry = Self::entry_for_cleanup(map.as_ref(), notify.as_ref(), &session).await;
        }
        let entry = state.entry.clone();

        // ---- R2f1b §5.1 step 6 PRESERVATION BARRIER (slice 2c1) -----------------------------
        // "Only then may session cancel or a resource signal occur." For a worktree-backed
        // session the resource signal is the inner `forget_session_checked` /
        // `release_session_checked` immediately below, and this is the one place every entry in
        // the R-11 fan-in reaches it — `ConfigureAdmission::Drop`, both configure rollbacks,
        // `retire`, forget/release/observed, `BindingGuard::Drop`, `ExpiryClaim`'s three entry
        // APIs, the eleven direct `release_session` sites, workflow cold cleanup, controller
        // retire. Placing the barrier here is therefore the ordering guarantee for every caller
        // that does NOT signal on its own first; callers that DO signal first (the executor's two
        // cold `cancel_observed` sites and its preflight) must call
        // `AgentBackend::preserve_checkout_v1` themselves, and that is what the barrier method
        // exists for.
        //
        // It runs only under a `Preserve` disposition, and that is §5.1's own rule, not a
        // shortcut: "a node-local success is NOT a checkout disposition — it settles the node
        // session but leaves its checkout `LiveProtected` under a workflow-level disposition
        // flight". A context-free entry (a reaper, a `Drop`) has no workflow outcome to consult,
        // so terminalizing a live checkout from one would decide a disposition nobody asked for.
        // Those entries still cannot delete: the gate below refuses and the report says
        // `Retained`.
        // ---- DURABLE DISPOSITION RE-DERIVATION (slice 2c2, P5) ------------------------------
        // The cell's disposition is in-memory and the reporter evicts the cell on the first `Ok`
        // report — which a gate refusal is — so a later flight would otherwise start from
        // `Reclaim` and report `Retained` for a checkout whose record says `Preserved`. Once the
        // cell is gone the RECORD is the authoritative disposition source; the flight takes the
        // stronger of the two, so this can raise a disposition and can never lower one.
        let (durable_disposition, durable_state) = match entry.as_ref() {
            Some(entry) => durable_checkout_disposition(&entry.worktree_path),
            None => (CheckoutDispositionV1::Reclaim, None),
        };
        let disposition = disposition.max(durable_disposition);

        let mut barrier = None;
        if disposition == CheckoutDispositionV1::Preserve {
            if let Some(entry) = entry.as_ref() {
                let reason = preservation_reason.unwrap_or(PreservationReasonV1::Cancellation);
                barrier = Some(preserve_entry_checkout(entry, reason).await);
            }
        }

        if state.inner_strength.is_none_or(|done| done < strength) {
            let inner_result = match strength {
                CleanupStrength::Forget => inner.forget_session_checked(&session).await,
                CleanupStrength::Release => inner.release_session_checked(&session).await,
            };
            match inner_result {
                Ok(disposition) => {
                    state.inner_strength = Some(strength);
                    state.inner_disposition = state.inner_disposition.combine(disposition);
                }
                Err(error) => first_error = Some(error),
            }
        }

        // ---- R2f1b §5.1 DELETION CAPABILITY (slice 2c2) --------------------------------------
        // "Globally healthy workflow success is the only automatic deletion path." Reaching this
        // block requires the cell's checkout disposition to be exactly `DeleteAuthorized`, and the
        // ONLY writer of that value is `settle_workflow_checkout_v1(GloballyHealthy)` — no
        // context-free entry (`ExpiryClaim`, either `Drop`, the reaper, controller retire) has a
        // workflow outcome to declare, so none can raise it, and the durable re-derivation above
        // can only raise the disposition to `Preserve`, never lower it to here.
        //
        // It runs AFTER the inner teardown, unchanged from the V2 order: the session must be
        // released before its checkout can be removed.
        //
        // The gate below is deliberately NOT consulted on this path, and that is the substitution
        // the capability makes: the gate is the context-free refusal for callers that have no
        // authority, and this caller's authority is a `DeletionCapabilityV1` minted by a CAS under
        // both custody cells over reverified identities. The cells are held for the whole
        // mint→remove→tombstone window, which is a STRICTER mutual exclusion than the gate's own
        // removal window — every deletion-side caller fails closed against the same cell while it
        // runs. Any outcome that did not remove the checkout falls through to the gate, which
        // refuses on the custody evidence exactly as before.
        let mut capability_removal = None;
        // 3s repair R1 (both review lenses): the admission guard must OUTLIVE the mint block and
        // span the map PROJECTION below — released at block scope, a queued preservation writer
        // could run between the tombstone and the map clear and observe the stale entry.
        let mut deletion_admission_guard = None;
        if disposition == CheckoutDispositionV1::DeleteAuthorized && first_error.is_none() {
            if let Some(entry) = entry.as_ref() {
                // Keep the same guard a preservation raise takes from the epoch validation through
                // the custody-cell CAS. A preserve is therefore ordered either before this check
                // or after the mint; it cannot change the generation in the old blind window.
                let guard = cell.deletion_admission.lock().await;
                deletion_admission_guard = Some(guard);
                if Self::deletion_generation_is_current(&cell, disposition, disposition_epoch) {
                    #[cfg(test)]
                    cell.pause_deletion_mint_for_test().await;
                    let outcome = authorize_and_remove_checkout(
                        &provider,
                        entry,
                        #[cfg(test)]
                        removal_tombstone_parent_sync_fault.clone(),
                    )
                    .await;
                    tracing::info!(
                        session = session.as_str(),
                        worktree_path = entry.worktree_path,
                        outcome = ?outcome,
                        "R2f1b workflow-level deletion capability settled"
                    );
                    capability_removal = Some(outcome);
                } else {
                    tracing::info!(
                        session = session.as_str(),
                        worktree_path = entry.worktree_path,
                        "R2f1b deletion authority is stale: the checkout disposition changed \
                         generation after this flight started"
                    );
                }
            }
        }
        // A removal that did NOT complete falls through to the gate below — release admission now
        // so the gate path adds no new nesting. A COMPLETED removal keeps the guard until its
        // projection lands (the map clear + `state.entry = None`), so no preservation writer can
        // observe the pre-projection map.
        if !capability_removal
            .as_ref()
            .is_some_and(|o| o.checkout_is_gone())
        {
            deletion_admission_guard = None;
        }
        #[cfg(test)]
        if capability_removal
            .as_ref()
            .is_some_and(|o| o.checkout_is_gone())
        {
            cell.pause_removal_projection_for_test().await;
        }
        if let Some(outcome) = capability_removal.as_ref().filter(|o| o.checkout_is_gone()) {
            let checkout = match outcome {
                CapabilityRemovalV1::Removed => CheckoutCleanupDispositionV1::Removed,
                CapabilityRemovalV1::RemovedRecordAmbiguous(detail) => {
                    tracing::warn!(
                        session = session.as_str(),
                        detail,
                        "the checkout was removed but its `Removed` tombstone is unverified"
                    );
                    CheckoutCleanupDispositionV1::RemovedRecordAmbiguous(detail.clone())
                }
                CapabilityRemovalV1::MintRefused(_)
                | CapabilityRemovalV1::MintAmbiguous(_)
                | CapabilityRemovalV1::RemovalFailed(_) => {
                    unreachable!("only gone capability outcomes enter this branch")
                }
            };
            // The checkout is provably gone: `remove_v2` verified target and registration absence
            // before the tombstone was attempted. Clear the map entry with the SAME still-same
            // check the V2 removal path uses (`Retained` included, so 2c1's "removal clears
            // `Retained` once protection lifts" arm composes with a capability-driven removal),
            // and mark the flight's component state done so no later flight re-runs a removal for
            // a checkout that no longer exists.
            state.provider_removed = true;
            state.sidecar_removed = true;
            let mut map = map.lock().await;
            let still_same = match map.get(session.as_str()) {
                Some(WtState::Ready(current)) | Some(WtState::Retained { entry: current, .. }) => {
                    let entry = entry
                        .as_ref()
                        .expect("the capability path requires an entry");
                    current.canonical_source == entry.canonical_source
                        && current.worktree_path == entry.worktree_path
                }
                _ => false,
            };
            if still_same {
                map.remove(session.as_str());
                notify.notify_waiters();
            }
            drop(map);
            state.entry = None;
            // The projection is complete — a preservation writer admitted from here on re-reads
            // an already-cleared map and truthfully observes `NoCheckoutUnderCustody`.
            drop(deletion_admission_guard);
            return CleanupReportV1::settled(checkout, state.inner_disposition, first_error);
        }

        // ---- R2f1b fail-closed deletion gate (slice 2b1) ------------------------------------
        // The whole deletion fan-in funnels through the block below: three `start_or_join_cleanup`
        // callers, two `run_cleanup_flight` spawn sites, and every external subsystem
        // (`BindingGuard::Drop`, `ExpiryClaim`'s three entry APIs, the eleven direct
        // `release_session` sites, workflow cold cleanup, controller retire) reaches it through
        // one of the `AgentBackend` cleanup methods. That convergence is why ONE gate suffices.
        //
        // Refusal is reported as Ok, not Err, and that is a decision with two reasons. It is not
        // a cleanup FAILURE — the inner session teardown above completed, and only the checkout
        // was deliberately retained; and the failed-configure reporter loop retries on an Err
        // report while `failed_configure_cleanup_pending` is set, so an Err here would spin a
        // protected rollback forever at the 30s backoff cap.
        //
        // On refusal we leave `state.provider_removed` / `state.sidecar_removed` false and
        // `state.entry` populated, so the map is never emptied as if the checkout were gone and
        // any later flight on this session re-runs the same refusal. Cleanup cells are
        // per-session, so a refusal cannot wedge any OTHER session's cleanup.
        //
        // BOTH 2b1 DEFERRED RULINGS ARE DISCHARGED IN THIS SLICE, and the mechanics are here.
        // (1) Refusal is still reported as `Ok` — the two reasons above are unchanged — but the
        // report now carries a TYPED checkout disposition (`Retained` / `Preserved`), so no caller
        // reads a refusal as a removal, and `cleanup_session_observed` publishes a distinct
        // terminal code for it. (2) A refused rollback of a `Reserving` entry no longer loses its
        // cleanup owner: `entry_for_cleanup` still pops the map entry (releasing the reservation
        // so a configure can retry is pre-existing and correct), and the refusal below RE-INSERTS
        // it as `WtState::Retained`, which is not reusable and is still an owner. 2b2's
        // `add_under_custody` prohibition is what makes the configure-retry path safe meanwhile.
        //
        // SLICE 2b2 ADDITION (S7): the probe and the removal happen inside the checkout's
        // PUBLICATION CELL, entered with the refusing acquirer, so a V3 writer cannot publish a
        // record between them. `_removal_window` is bound for the rest of this scope on purpose —
        // its `Drop` is the release, and it now spans the SETTLEMENT below too (the remaining half
        // of 2b1 sol SMELL-1, which 2b2 split and assigned here).
        let (_removal_window, refusal) = match entry.as_ref() {
            None => (None, None),
            Some(entry) => match CheckoutRemovalWindowV1::enter(entry) {
                Err(refusal) => (None, Some(refusal)),
                Ok(window) => (window, checkout_removal_refusal(entry)),
            },
        };
        let entry = match entry {
            Some(entry) => match refusal {
                None => Some(entry),
                Some(refusal) => {
                    tracing::warn!(
                        session = session.as_str(),
                        worktree_path = entry.worktree_path,
                        reason = refusal.reason(),
                        detail = refusal.detail(),
                        "refusing to remove a worktree checkout under R2f1b custody"
                    );
                    Self::retain_refused_entry(
                        map.as_ref(),
                        notify.as_ref(),
                        &session,
                        &entry,
                        &refusal,
                        barrier.as_ref(),
                        durable_state,
                    )
                    .await;
                    // The refusal is about the CHECKOUT only. A genuine inner-teardown failure
                    // recorded above is still the flight's result — swallowing it here would
                    // report a broken session release as a clean cleanup.
                    //
                    // The `Preserved` label comes from EITHER this flight's own barrier or the
                    // record on disk (slice 2c2, P5): a flight on a rebuilt cell has no barrier
                    // outcome to consult, and reporting `Retained` for a checkout whose durable
                    // record is a settled preservation is exactly the mislabelling opus W3 named.
                    let durably_preserved = durable_state
                        .is_some_and(WorktreeCustodyStateKindV1::is_terminal_preservation);
                    let checkout = match barrier.as_ref() {
                        Some(outcome) if outcome.is_terminal_preservation() => {
                            CheckoutCleanupDispositionV1::Preserved
                        }
                        _ if durably_preserved => CheckoutCleanupDispositionV1::Preserved,
                        _ => CheckoutCleanupDispositionV1::Retained,
                    };
                    return CleanupReportV1::settled(
                        checkout,
                        state.inner_disposition,
                        first_error,
                    );
                }
            },
            None => None,
        };

        let mut checkout = CheckoutCleanupDispositionV1::NotNeeded;
        if let Some(entry) = entry {
            checkout = CheckoutCleanupDispositionV1::Removed;
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
                // `Retained` is matched alongside `Ready` because a retained entry is the map's
                // owner for a checkout whose protection later lifted: without this arm the
                // removal would succeed and the entry would stay mapped forever, which is a
                // different leak from the one `Retained` exists to fix.
                let still_same = match map.get(session.as_str()) {
                    Some(WtState::Ready(current))
                    | Some(WtState::Retained { entry: current, .. }) => {
                        current.canonical_source == entry.canonical_source
                            && current.worktree_path == entry.worktree_path
                    }
                    _ => false,
                };
                if still_same {
                    map.remove(session.as_str());
                    notify.notify_waiters();
                }
                state.entry = None;
            } else {
                checkout = CheckoutCleanupDispositionV1::RemovalFailed;
            }
        } else {
            state.provider_removed = true;
            state.sidecar_removed = true;
        }

        CleanupReportV1::settled(checkout, state.inner_disposition, first_error)
    }

    /// Keep exactly one in-memory owner for a checkout the gate refused to remove (2b1 sol-2).
    ///
    /// Scoped to the two CUSTODY-POSITIVE refusals on purpose. `ProbeInconclusive` and
    /// `CellContended` are transient unknowns, not evidence that a custody record governs the
    /// checkout, and 2b1's accepted V2 trade for them is a *self-healing* protective leak: the
    /// legacy `.meta.json` is retained, so the run-end guard or the next boot sweep reclaims it,
    /// and a configure retry proceeds. Converting an unknown into a durable non-reusable retention
    /// would turn that self-healing leak into a permanently wedged session id — a strictly worse
    /// outcome, and one no custody evidence justifies.
    ///
    /// A `Ready` entry is left `Ready` unless a terminal preservation exists: §5.1 keeps a
    /// live-but-protected checkout reusable by its own session, and only a preserved claim (which
    /// awaits R2f2 disposition) must never be handed onward.
    async fn retain_refused_entry(
        map: &Mutex<HashMap<String, WtState>>,
        notify: &Notify,
        session: &SessionId,
        entry: &WtEntry,
        refusal: &CheckoutRemovalRefusalV1,
        barrier: Option<&PreservationOutcomeV1>,
        durable_state: Option<WorktreeCustodyStateKindV1>,
    ) {
        let custody_positive = matches!(
            refusal,
            CheckoutRemovalRefusalV1::Discriminated | CheckoutRemovalRefusalV1::RecordPresent
        );
        if !custody_positive {
            return;
        }
        let from_barrier = match barrier {
            Some(PreservationOutcomeV1::Preserved | PreservationOutcomeV1::AlreadyPreserved) => {
                CheckoutRetentionV1::Preserved
            }
            Some(
                PreservationOutcomeV1::PreservationUnknown(_)
                | PreservationOutcomeV1::AlreadyUnknown,
            ) => CheckoutRetentionV1::PreservationUnknown,
            Some(PreservationOutcomeV1::Ambiguous(_)) => CheckoutRetentionV1::PreservationAmbiguous,
            Some(PreservationOutcomeV1::Refused(_)) | None => {
                CheckoutRetentionV1::RefusedUnderCustody
            }
        };
        // The RECORD is the authoritative disposition source once the cell that held the
        // in-memory one is gone (slice 2c2, P5). Take the strongest of the two: a flight with no
        // barrier outcome at all must still label a durably-preserved checkout `Preserved`.
        let from_record = match durable_state {
            Some(WorktreeCustodyStateKindV1::Preserved) => CheckoutRetentionV1::Preserved,
            Some(WorktreeCustodyStateKindV1::PreservationUnknown) => {
                CheckoutRetentionV1::PreservationUnknown
            }
            Some(WorktreeCustodyStateKindV1::PreservationPrepared) => {
                CheckoutRetentionV1::PreservationAmbiguous
            }
            _ => CheckoutRetentionV1::RefusedUnderCustody,
        };
        let retention = from_barrier.max(from_record);
        let mut map = map.lock().await;
        match map.get(session.as_str()) {
            // The reservation was popped by `entry_for_cleanup`; re-insert so an owner survives.
            None => {
                map.insert(
                    session.as_str().to_owned(),
                    WtState::Retained {
                        entry: entry.clone(),
                        retention,
                    },
                );
                notify.notify_waiters();
            }
            Some(WtState::Ready(_)) if retention != CheckoutRetentionV1::RefusedUnderCustody => {
                map.insert(
                    session.as_str().to_owned(),
                    WtState::Retained {
                        entry: entry.clone(),
                        retention,
                    },
                );
                notify.notify_waiters();
            }
            // An existing `Retained` keeps the STRONGEST retention of the two (repair RE-6 made
            // this true; it was previously only claimed). A second refusal can carry better
            // knowledge than the first — a barrier that came back `Ambiguous` and then settled on
            // a later flight is exactly that — and silently keeping the older, weaker label would
            // make the configure refusal message and any future reader understate what is known.
            Some(WtState::Retained {
                entry: current,
                retention: recorded,
            }) if retention > *recorded => {
                let entry = current.clone();
                map.insert(
                    session.as_str().to_owned(),
                    WtState::Retained { entry, retention },
                );
            }
            // An existing `Ready` (live-protected) stays reusable; a `Reserving` entry belongs to
            // a configure that is still running and must not be stolen.
            Some(_) => {}
        }
    }
}

impl WorktreeBackend {
    async fn configure_bound_inner_at(
        &self,
        session: &SessionId,
        spec: &BoundSessionSpecV1,
        worktree_path: &str,
    ) -> Result<(), BridgeError> {
        if spec.session.cwd.as_ref().map(SessionCwd::as_str) != Some(worktree_path) {
            return Err(BridgeError::ConfigMismatch {
                field: "bound session cwd",
            });
        }
        self.inner.configure_bound_session(session, spec).await
    }

    /// Materialize the checkout under whichever record regime this spec routes.
    ///
    /// Folded into one step because both V2 legs already shared one rollback: `provider.add`
    /// failure and `write_sidecar` failure ran byte-identical recovery at the call site.
    async fn materialize_checkout(
        &self,
        session: &SessionId,
        spec: &BoundSessionSpecV1,
        resolved: &ResolvedWorktree,
    ) -> Result<(WtCustodyV1, Option<Box<ProtectedCheckoutV1>>), BridgeError> {
        let Some(custody) = spec.custody() else {
            // ---- V2: unchanged, in its original order ----
            let common_dir = self
                .provider
                .add(&resolved.canonical_source, &resolved.worktree_path)
                .await?;
            write_sidecar(&WorktreeSidecar {
                canonical_source: resolved.canonical_source.clone(),
                common_dir,
                worktree_path: resolved.worktree_path.clone(),
                owner: self.cfg.owner.clone(),
                run_id: self.identity.run_id.clone(),
                host: self.identity.host.clone(),
                lease: self.identity.lease.clone(),
            })?;
            return Ok((WtCustodyV1::Legacy, None));
        };
        self.materialize_under_custody(session, custody.clone(), resolved)
            .await
    }

    /// Claim and detach the V3 materialization before the first filesystem effect. The returned
    /// receiver observes the independently owned runner; dropping it cannot abort its custody
    /// publication, provider add, map projection, or terminal record.
    async fn materialize_under_custody(
        &self,
        session: &SessionId,
        custody: bridge_core::execution_policy::BoundWorktreeCustodyV1,
        resolved: &ResolvedWorktree,
    ) -> Result<(WtCustodyV1, Option<Box<ProtectedCheckoutV1>>), BridgeError> {
        // This preflight is effect-free. A refusing provider gets no preparation claim, no
        // companion record, no custody record, and no add.
        if !self.provider.supports_custody_add() {
            return Err(BridgeError::ConfigInvalid {
                reason: "worktree provider does not implement the R2f1b custody-aware add".into(),
            });
        }
        let session_key = session.as_str().to_owned();
        #[cfg(test)]
        let preparation_bound = self
            .preparation_test_bound
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone();
        #[cfg(test)]
        let flight = Arc::new(MaterializationPreparationFlightV1::claim(
            self.preparation_test_hooks.clone(),
            preparation_bound,
        )?);
        #[cfg(not(test))]
        let flight = Arc::new(MaterializationPreparationFlightV1::claim()?);
        let journal = Arc::new(PreparationFlightJournalV1::new(
            self.preparation_control_root.clone(),
            &resolved.worktree_path,
            flight.id().clone(),
        )?);
        flight.set_journal(journal.clone());
        let active_flight = Arc::new(ActivePreparationFlightV1::new(flight.clone()));
        {
            let mut flights = self
                .preparation_flights
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            if flights.contains_key(&session_key) {
                return Err(BridgeError::AgentOverloaded);
            }
            flights.insert(session_key.clone(), active_flight.clone());
        }
        flight.begin_operation(PreparationOperationV1::JournalOpenPublish);
        let root_pin_claimed = self
            .preparation_control_root
            .begin_pin_after_owner_published();
        if root_pin_claimed {
            let root_owner = self.preparation_control_root.clone();
            std::mem::drop(tokio::task::spawn_blocking(move || {
                let _ = root_owner.open_claimed_for_session_admission();
            }));
        }

        let caller_guard = flight.caller_guard();
        let provider = self.provider.clone();
        let map = self.map.clone();
        let flights = self.preparation_flights.clone();
        let recovery_flights = self.preparation_recovery_flights.clone();
        let resolved = ResolvedWorktree {
            canonical_source: resolved.canonical_source.clone(),
            worktree_path: resolved.worktree_path.clone(),
        };
        let expected_worktree_path = resolved.worktree_path.clone();
        let task_flight = flight.clone();
        let task_owner = active_flight.clone();
        let task_journal = journal.clone();
        let (result_tx, result_rx) = oneshot::channel();
        active_flight.install_result(result_tx);
        let task_root = self.preparation_control_root.clone();
        let (runner_exit_tx, runner_exit_rx) = oneshot::channel();
        let runner_exit_flights = self.preparation_flights.clone();
        let runner_exit_owner = active_flight.clone();
        let runner_exit_session = session_key.clone();
        let runner = tokio::spawn(async move {
            let mut runner_exit_guard = PreparationRunnerExitGuardV1::new(runner_exit_tx);
            let root_ready =
                tokio::task::spawn_blocking(move || task_root.pinned_root().map(|_| ()))
                    .await
                    .map_err(|_| BridgeError::StoreFailure)
                    .and_then(|result| result);
            if let Err(error) = root_ready {
                task_owner
                    .complete_with_result(Err(error.clone()), Err(error))
                    .await;
                runner_exit_guard.complete();
                return;
            }
            let initial = tokio::task::spawn_blocking({
                #[cfg(test)]
                let initial_flight = task_flight.clone();
                let journal = task_journal.clone();
                move || -> Result<(), BridgeError> {
                    #[cfg(test)]
                    initial_flight.block_initial_open_publish_for_test();
                    #[cfg(test)]
                    if initial_flight.fail_initial_open_parent_sync_for_test() {
                        journal.fail_next_parent_sync_for_test();
                    }
                    journal.publish(PreparationFlightStateV1::Open {}, true)
                }
            })
            .await
            .map_err(|_| BridgeError::StoreFailure)
            .and_then(|result| result);

            // A transfer may have written the first control record while initial Open was blocked.
            // It owns completion and recovery publication; this released runner must not overwrite it.
            if task_flight.transfer_owned() {
                runner_exit_guard.complete();
                return;
            }

            let result = match initial {
                Ok(()) => {
                    let journal = task_journal.clone();
                    if let Some((operation, reason)) = task_flight.expired_pre_barrier() {
                        match transfer_preparation_flight(
                            &flights,
                            &recovery_flights,
                            &session_key,
                            &task_owner,
                            operation,
                            reason,
                        )
                        .await
                        {
                            Ok(true) => {
                                runner_exit_guard.complete();
                                return;
                            }
                            Ok(false) => {}
                            Err(error) => {
                                task_owner
                                    .complete_with_result(Err(error.clone()), Err(error))
                                    .await;
                                runner_exit_guard.complete();
                                return;
                            }
                        }
                    }
                    #[cfg(test)]
                    task_flight.after_open_for_test().await;
                    task_flight.begin_operation(PreparationOperationV1::CustodyEntryPublish);
                    if let Some((operation, reason)) = task_flight.expired_pre_barrier() {
                        match transfer_preparation_flight(
                            &flights,
                            &recovery_flights,
                            &session_key,
                            &task_owner,
                            operation,
                            reason,
                        )
                        .await
                        {
                            Ok(true) => {
                                runner_exit_guard.complete();
                                return;
                            }
                            Ok(false) => {}
                            Err(error) => {
                                task_owner
                                    .complete_with_result(Err(error.clone()), Err(error))
                                    .await;
                                runner_exit_guard.complete();
                                return;
                            }
                        }
                    }
                    let caller_departed_at_add_admission = task_flight.commit_add_admission();
                    let result = if caller_departed_at_add_admission {
                        Err(BridgeError::ConfigInvalid {
                            reason: "configure caller departed before worktree add admission"
                                .into(),
                        })
                    } else {
                        #[cfg(test)]
                        task_flight.after_add_admission_for_test().await;
                        WorktreeBackend::run_materialization_under_custody(
                            provider,
                            custody,
                            resolved,
                            journal.clone(),
                            PreparationFlightRunContextV1 {
                                flights: &flights,
                                recovery_flights: &recovery_flights,
                                session_key: &session_key,
                                owner: &task_owner,
                            },
                        )
                        .await
                    };
                    match result {
                        Ok(materialized) => {
                            let projection = if materialized.0 == WtCustodyV1::Protected {
                                match &materialized.1 {
                                    None => Err(BridgeError::agent_crashed(
                                        "protected preparation flight omitted retained identities",
                                    )),
                                    Some(protection) => {
                                        let mut entries = map.lock().await;
                                        match entries.get_mut(&session_key) {
                                            Some(WtState::Reserving { entry, .. })
                                            | Some(WtState::Ready(entry))
                                            | Some(WtState::Retained { entry, .. })
                                                if entry.worktree_path == expected_worktree_path =>
                                            {
                                                entry.custody = WtCustodyV1::Protected;
                                                entry.protection = Some(protection.clone());
                                                Ok(())
                                            }
                                            Some(_) => Err(BridgeError::agent_crashed(
                                                "preparation flight map ownership changed before projection",
                                            )),
                                            None => Err(BridgeError::agent_crashed(
                                                "committed preparation flight lost its map entry before projection",
                                            )),
                                        }
                                    }
                                }
                            } else {
                                Ok(())
                            };
                            match projection {
                                Ok(()) => {
                                    let settled = PreparationFlightStateV1::Settled {};
                                    debug_assert!(preparation_state_is_terminal(&settled));
                                    publish_preparation_state(
                                        journal,
                                        task_flight.clone(),
                                        settled,
                                        false,
                                    )
                                    .await
                                    .map(|()| materialized)
                                }
                                Err(error) => Err(error),
                            }
                        }
                        Err(error) => {
                            let phase = task_flight.phase();
                            if phase == PreparationPublicationPhaseV1::FailurePublishing {
                                return;
                            }
                            if phase == PreparationPublicationPhaseV1::TransferPublishing
                                || (phase == PreparationPublicationPhaseV1::Preparing
                                    && !task_flight.begin_failure_publication())
                            {
                                Err(error)
                            } else {
                                let failed = if caller_departed_at_add_admission {
                                    preparation_caller_departed_failure_state()
                                } else {
                                    preparation_failure_state()
                                };
                                debug_assert!(preparation_state_is_terminal(&failed));
                                match publish_preparation_state(
                                    journal,
                                    task_flight.clone(),
                                    failed,
                                    false,
                                )
                                .await
                                {
                                    Ok(()) => Err(error),
                                    Err(publication_error) => Err(publication_error),
                                }
                            }
                        }
                    }
                }
                Err(error) => {
                    let phase = task_flight.phase();
                    if phase == PreparationPublicationPhaseV1::FailurePublishing {
                        return;
                    }
                    if phase == PreparationPublicationPhaseV1::TransferPublishing
                        || (phase == PreparationPublicationPhaseV1::Preparing
                            && !task_flight.begin_failure_publication())
                    {
                        Err(error)
                    } else {
                        let failed = preparation_failure_state();
                        match publish_preparation_state(
                            task_journal.clone(),
                            task_flight.clone(),
                            failed,
                            false,
                        )
                        .await
                        {
                            Ok(()) => Err(error),
                            Err(publication_error) => Err(publication_error),
                        }
                    }
                }
            };
            if task_flight.transfer_owned() {
                runner_exit_guard.complete();
                return;
            }
            let completion = if task_flight.has_durable_terminal() {
                Ok(())
            } else {
                Err(result.as_ref().err().cloned().unwrap_or_else(|| {
                    BridgeError::agent_crashed(
                        "preparation flight completed without a durable terminal record",
                    )
                }))
            };
            task_owner
                .complete_with_result(completion.clone(), result)
                .await;
            if completion.is_ok() {
                let mut active = flights.lock().unwrap_or_else(|error| error.into_inner());
                if active
                    .get(&session_key)
                    .is_some_and(|current| Arc::ptr_eq(current, &task_owner))
                {
                    active.remove(&session_key);
                }
            }
            runner_exit_guard.complete();
        });
        active_flight.install_runner(runner);
        tokio::spawn(async move {
            if runner_exit_rx.await.is_ok() {
                terminalize_preparation_runner_exit(
                    runner_exit_flights,
                    runner_exit_session,
                    runner_exit_owner,
                )
                .await;
            }
        });
        let result = result_rx.await.map_err(|_| {
            BridgeError::agent_crashed("materialization preparation flight ended without a result")
        })?;
        caller_guard.disarm();
        result
    }

    /// The V3 writer's control flow (§2.5). Ordering is the property, so it is stated once here
    /// and nowhere else:
    ///
    /// 1. enter both cells + pin the root, publish `ProtectionPrepared` (no-replace) + parent
    ///    sync, replace `Materializing` + parent sync — **all before any provider effect**;
    /// 2. `add_under_custody`, which never reaches `cleanup_failed_add`;
    /// 3. success → capture the four identities by descriptor → replace `LiveProtected`;
    ///    failure → replace `PreservationUnknown{materialization_inflight}`, target untouched.
    ///
    /// The custody cells are held across the add on purpose: the record must stay this
    /// custodian's for the whole window in which the checkout is half-made. The add itself never
    /// takes a custody cell, so this cannot deadlock.
    async fn run_materialization_under_custody(
        provider: Arc<dyn WorktreeProvider>,
        custody: bridge_core::execution_policy::BoundWorktreeCustodyV1,
        resolved: ResolvedWorktree,
        journal: Arc<PreparationFlightJournalV1>,
        context: PreparationFlightRunContextV1<'_>,
    ) -> Result<(WtCustodyV1, Option<Box<ProtectedCheckoutV1>>), BridgeError> {
        let flight = context.owner.flight.clone();
        let worktree_root = Path::new(&resolved.worktree_path)
            .parent()
            .ok_or(BridgeError::ConfigMismatch {
                field: "bound worktree target has no enclosing root",
            })?
            .to_path_buf();
        let worktree_path = resolved.worktree_path.clone();
        let canonical_source = resolved.canonical_source.clone();

        // Blocking: every step is descriptor-level filesystem work behind two blocking file
        // locks. `custody_lock.rs` requires offloading, and the pinned root and both guards are
        // `Send`, so the custodian moves back out.
        let custody_result = tokio::task::spawn_blocking({
            let worktree_path = worktree_path.clone();
            let custody_flight = flight.clone();
            move || -> Result<WorktreeCustodianV1, CustodyWriteRefusalV1> {
                #[cfg(test)]
                custody_flight.block_custody_sync_for_test();
                if custody_flight.transfer_owned() {
                    return Err(CustodyWriteRefusalV1::Failed(
                        "preparation transferred before custody entry".to_owned(),
                    ));
                }
                let custodian =
                    WorktreeCustodianV1::enter(&worktree_root, &worktree_path, custody)?;
                custodian.publish_protection_prepared()?;
                custodian.replace_materializing()?;
                Ok(custodian)
            }
        })
        .await
        .map_err(|error| {
            BridgeError::agent_crashed(format!("custody preparation task failed: {error}"))
        })?;

        // The post-return sample closes the slow-return window: a custody operation that crossed
        // the action bound may transfer, but neither terminal state nor a provider effect may be
        // admitted after that transfer wins.
        if let Some((operation, reason)) = flight.expired_pre_barrier() {
            let transferred = transfer_preparation_flight(
                context.flights,
                context.recovery_flights,
                context.session_key,
                context.owner,
                operation,
                reason,
            )
            .await?;
            if transferred || flight.transfer_owned() {
                return Err(BridgeError::ConfigInvalid {
                    reason: "preparation transferred before custody effect admission".into(),
                });
            }
        }
        let custodian = custody_result.map_err(custody_write_error)?;
        if !flight.begin_barrier_publication() {
            return Err(BridgeError::ConfigInvalid {
                reason: "preparation transferred before barrier publication".into(),
            });
        }
        publish_preparation_state(
            journal.clone(),
            flight.clone(),
            PreparationFlightStateV1::BarrierSynced {},
            false,
        )
        .await?;
        flight.begin_operation(PreparationOperationV1::IdentityCapture);

        // A runtime `Err` here (a git spawn failure, say) is NOT allowed to propagate raw: the
        // record is already `Materializing`, and returning without settling would leave a durable
        // live state for a materialization that is over. Normalize it into the same classified
        // failure the settlement arm already handles, with the most protective answers available:
        // the target is Unproven (so it is treated as present and never touched) and registration
        // is unproven (so no definite locator is invented from an operation that never reported).
        let outcome = match provider
            .add_under_custody(&canonical_source, &worktree_path)
            .await
        {
            Ok(outcome) => outcome,
            Err(error) => CustodyAddOutcomeV1::Failed(crate::provider::CustodyAddFailureV1 {
                reason: format!("custody-aware add failed before reporting an outcome: {error:?}"),
                target: CustodyAddTargetV1::Unproven,
                common_dir: None,
                recovery_locator: crate::custody::RecoveryLocatorV1::RegistrationUnproven {},
            }),
        };
        #[cfg(test)]
        flight.after_add_for_test().await;

        let root_path = custodian.worktree_root().to_string_lossy().into_owned();
        let binding = custodian.binding().clone();
        type MaterializedV1 = (WtCustodyV1, Option<Box<ProtectedCheckoutV1>>);
        tokio::task::spawn_blocking(move || -> Result<MaterializedV1, CustodyWriteRefusalV1> {
            match outcome {
                CustodyAddOutcomeV1::Materialized { common_dir } => {
                    let identities = MaterializedIdentitiesV1 {
                        source: observed_identity(&canonical_source),
                        root: observed_identity(&root_path),
                        worktree: observed_identity(&worktree_path),
                        common_dir: observed_identity(&common_dir),
                    };
                    custodian.replace_live_protected(&identities)?;
                    // SUCCESS-PATH IDENTITY RETENTION (2b2 opus S-9 / sol S-3, slice 2c1 P7).
                    // `LiveProtected` forbids a claim, so these four identities were previously
                    // observed and then dropped on the floor. They are the ONLY evidence that can
                    // later distinguish "the objects we materialized" from "whatever now occupies
                    // those paths", so they are retained on the mapped entry and reverified by
                    // descriptor at claim-mint time.
                    Ok((
                        WtCustodyV1::Protected,
                        Some(Box::new(ProtectedCheckoutV1 {
                            binding,
                            identities,
                            // The add reported `Materialized`, which is git-level proof the linked
                            // worktree is registered with its common dir. Retained here because
                            // nothing re-probes registration at preservation time.
                            locator: crate::custody::RecoveryLocatorV1::RegisteredWorktree {},
                        })),
                    ))
                }
                CustodyAddOutcomeV1::Failed(failure) => {
                    // §5.7 row 4 / §5.1: an unresolved materialization is published unknown and
                    // the target is NEVER deleted — including when the target is provably absent,
                    // where the record is simply retained rather than reclaimed (the
                    // `Materializing -> UnusedSettled` edge is not in 2a's frozen transition
                    // table, and minting a marker removal is not this slice's authority).
                    let identities = MaterializedIdentitiesV1 {
                        source: observed_identity(&canonical_source),
                        root: observed_identity(&root_path),
                        worktree: match failure.target {
                            CustodyAddTargetV1::ProvablyAbsent => planned_identity(&worktree_path),
                            _ => observed_identity(&worktree_path),
                        },
                        // No observed common dir: record the PLAN-DERIVED common-dir path, not
                        // the source repo (repair R7). `<source>/.git` is what the common dir of a
                        // linked worktree is, so a degraded identity there is a true statement
                        // about the right object; naming the source repo instead made the claim
                        // assert that the source directory IS the common dir, which is false and
                        // would send an R2f2 consumer to the wrong object.
                        common_dir: match &failure.common_dir {
                            Some(path) => observed_identity(path),
                            None => planned_identity(
                                &Path::new(&canonical_source).join(".git").to_string_lossy(),
                            ),
                        },
                    };
                    custodian.replace_preservation_unknown(
                        PreservationReasonV1::MaterializationInFlight,
                        &identities,
                        failure.recovery_locator,
                        wall_clock_ms(),
                    )?;
                    Err(CustodyWriteRefusalV1::Failed(failure.reason))
                }
            }
        })
        .await
        .map_err(|error| {
            BridgeError::agent_crashed(format!("custody settlement task failed: {error}"))
        })?
        .map_err(custody_write_error)
    }

    async fn configure_bound_resolved_with_admission(
        &self,
        session: &SessionId,
        spec: &BoundSessionSpecV1,
        resolved: ResolvedWorktree,
        mut admission: ConfigureAdmission<'_>,
    ) -> Result<(), BridgeError> {
        let key = session.as_str().to_string();
        let reservation_entry = WtEntry {
            canonical_source: resolved.canonical_source.clone(),
            worktree_path: resolved.worktree_path.clone(),
            // Legacy: a reservation precedes materialization, so no custody evidence exists yet.
            // The disk arm of the deletion gate still applies to this entry.
            custody: WtCustodyV1::Legacy,
            protection: None,
        };
        let admission_cell = admission.cell.clone();
        let claim;

        loop {
            let map_changed = self.notify.notified();
            let configure_changed = admission_cell.configure_settled.notified();
            let mut map = self.map.lock().await;
            match map.get(session.as_str()) {
                // R2f1b READY-REUSE POLICY (2b1 opus S-3), enforced by an explicit arm rather
                // than a wildcard: a checkout the custody machinery is retaining — because a
                // preservation claim awaits R2f2 disposition, or because the deletion gate refused
                // and this entry is its last owner — is NOT a cwd a new session may be handed. The
                // pre-2c1 `Ready` arm validated only `canonical_source`, so it would have
                // configured a session directly on top of preserved work and let the agent write
                // over it.
                //
                // V2 is byte-identical: a legacy checkout never enters this state (it is reached
                // only from a custody-positive gate refusal or the preservation barrier), and the
                // `Ready` arm below is unchanged for both regimes.
                Some(WtState::Retained { entry, retention }) => {
                    let reason = format!(
                        "worktree checkout {} is retained under R2f1b custody ({retention:?}) and \
                         cannot be reused as a session cwd",
                        entry.worktree_path
                    );
                    drop(map);
                    return Err(BridgeError::ConfigInvalid { reason });
                }
                Some(WtState::Ready(entry)) => {
                    if entry.canonical_source != resolved.canonical_source
                        || entry.worktree_path != resolved.worktree_path
                    {
                        return Err(BridgeError::ConfigMismatch {
                            field: "bound worktree identity",
                        });
                    }
                    let worktree_path = entry.worktree_path.clone();
                    drop(map);
                    let result = self
                        .configure_bound_inner_at(session, spec, &worktree_path)
                        .await;
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

        // ---- V2/V3 fork (slice 2b2) --------------------------------------------------------
        // The ONE place the two record regimes diverge. V2 keeps its exact sequence: add, then
        // `.meta.json`. V3 inverts it — record published and parent-synced BEFORE any
        // `git worktree add` — and writes NO `.meta.json` at all, which is the rollback condition
        // (§2.2 "Record naming"; brief §4): an older binary enumerates only `*.meta.json`, so it
        // cannot name a V3 checkout, and emitting both names would hand the same checkout to the
        // legacy boot arm, which deletes.
        let (custody_kind, protection) = match self
            .materialize_checkout(session, spec, &resolved)
            .await
        {
            Ok(materialized) => materialized,
            Err(error) => {
                admission.retain_failed_configure_cleanup();
                drop(admission);
                let _ = self
                    .cleanup_session_with_sealed_admission(session, CleanupStrength::Release, true)
                    .await;
                return Err(error);
            }
        };

        // ---- CUSTODY EVIDENCE IS UPGRADED ONTO THE RESERVATION HERE (repair RD) --------------
        // Immediately after materialization and BEFORE the next await, not at the `Ready`
        // publication below. Everything between those two points — the inner configure, and the
        // cancellation window around it — used to see a `Legacy`/`None` reservation entry sitting
        // beside a durable `LiveProtected` record. The consequence was not a deletion hole (the
        // gate's DISK arm refuses on the record's presence regardless of the discriminator, and
        // 2b1 pins that): it was an EVIDENCE hole. Preservation answered
        // `NoCheckoutUnderCustody`, so the checkout was retained with no claim, and the exact
        // identities needed to mint one — observed by descriptor under the custody cell, and
        // unobtainable afterwards — were dropped on the floor. No later authority could recover
        // them, so `LiveProtected` would have been the checkout's permanent state.
        //
        // RESIDUAL WINDOW, named rather than closed: the materializing `spawn_blocking` can
        // complete and this future can be dropped before the map write lands. The record is
        // durable by then and the disk gate contains the consequence (nothing deletes it), so the
        // residual cost is the same evidence loss over a strictly smaller window. Closing it needs
        // a claimed, non-cancellable materialization flight — that is §2.5's preparation flight,
        // and it is slice 3's runner, LEDGERED not improvised.
        if custody_kind == WtCustodyV1::Protected {
            let mut map = self.map.lock().await;
            if let Some(WtState::Reserving { entry, .. }) = map.get_mut(session.as_str()) {
                if entry.worktree_path == resolved.worktree_path {
                    entry.custody = custody_kind;
                    entry.protection = protection.clone();
                }
            }
        }

        if let Err(error) = self
            .configure_bound_inner_at(session, spec, &resolved.worktree_path)
            .await
        {
            admission.retain_failed_configure_cleanup();
            drop(admission);
            let _ = self
                .cleanup_session_with_sealed_admission(session, CleanupStrength::Release, true)
                .await;
            return Err(error);
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
                    custody: custody_kind,
                    protection,
                }),
            );
            self.notify.notify_waiters();
            drop(map);
            if sealed {
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
}

async fn wait_for_cleanup_report(mut report: CleanupReportReceiver) -> CleanupReportV1 {
    loop {
        if let Some(result) = report.borrow().clone() {
            return result;
        }
        if report.changed().await.is_err() {
            return CleanupReportV1 {
                result: Err(BridgeError::agent_crashed(
                    "worktree cleanup report channel closed",
                )),
                checkout: CheckoutCleanupDispositionV1::RemovalFailed,
            };
        }
    }
}

async fn wait_for_preparation_completion(
    mut completion: watch::Receiver<Option<Result<(), BridgeError>>>,
) -> Result<(), BridgeError> {
    loop {
        if let Some(result) = completion.borrow().clone() {
            return result;
        }
        if completion.changed().await.is_err() {
            return Err(BridgeError::agent_crashed(
                "active preparation flight ended without a completion record",
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

    fn resource_flight_v1(&self) -> Result<BackendResourceFlightV1, BridgeError> {
        self.inner.resource_flight_v1()
    }

    fn attach_resource_flight_owner_v1(
        &self,
        session: &SessionId,
    ) -> Result<BackendResourceFlightV1, BridgeError> {
        self.inner.attach_resource_flight_owner_v1(session)
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

    async fn configure_bound_session(
        &self,
        session: &SessionId,
        spec: &BoundSessionSpecV1,
    ) -> Result<(), BridgeError> {
        let admission = self.admit_configure(session)?;
        let frozen = spec.provider_effect.frozen();
        let session_cwd = spec
            .session
            .cwd
            .as_ref()
            .ok_or(BridgeError::ConfigMismatch {
                field: "bound session cwd",
            })?;
        if session_cwd != &frozen.effect.effective_session_cwd
            || session_cwd != frozen.checkout.effective_cwd()
        {
            return Err(BridgeError::ConfigMismatch {
                field: "bound session cwd",
            });
        }

        match &frozen.checkout {
            FrozenCheckoutEffectV1::Direct { .. } => {
                let mut admission = admission;
                let result = self.inner.configure_bound_session(session, spec).await;
                if result.is_ok() {
                    admission.retain_for_session();
                }
                result
            }
            FrozenCheckoutEffectV1::Worktree { .. } => {
                let resolved =
                    validate_bound_worktree(&self.cfg, &self.allowed_root, &frozen.checkout)?;
                self.configure_bound_resolved_with_admission(session, spec, resolved, admission)
                    .await
            }
        }
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
            // Legacy: a reservation precedes materialization, so no custody evidence exists yet.
            // The disk arm of the deletion gate still applies to this entry.
            custody: WtCustodyV1::Legacy,
            protection: None,
        };
        let admission_cell = admission.cell.clone();
        let claim;

        loop {
            let map_changed = self.notify.notified();
            let configure_changed = admission_cell.configure_settled.notified();
            let mut map = self.map.lock().await;
            match map.get(session.as_str()) {
                // R2f1b READY-REUSE POLICY (2b1 opus S-3), enforced by an explicit arm rather
                // than a wildcard: a checkout the custody machinery is retaining — because a
                // preservation claim awaits R2f2 disposition, or because the deletion gate refused
                // and this entry is its last owner — is NOT a cwd a new session may be handed. The
                // pre-2c1 `Ready` arm validated only `canonical_source`, so it would have
                // configured a session directly on top of preserved work and let the agent write
                // over it.
                //
                // V2 is byte-identical: a legacy checkout never enters this state (it is reached
                // only from a custody-positive gate refusal or the preservation barrier), and the
                // `Ready` arm below is unchanged for both regimes.
                Some(WtState::Retained { entry, retention }) => {
                    let reason = format!(
                        "worktree checkout {} is retained under R2f1b custody ({retention:?}) and \
                         cannot be reused as a session cwd",
                        entry.worktree_path
                    );
                    drop(map);
                    return Err(BridgeError::ConfigInvalid { reason });
                }
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
                    custody: WtCustodyV1::Legacy,
                    protection: None,
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

    async fn forget_session_checked(
        &self,
        session: &SessionId,
    ) -> Result<BackendCleanupDispositionV1, BridgeError> {
        self.cleanup_session(session, CleanupStrength::Forget).await
    }

    async fn forget_session_observed(
        &self,
        session: &SessionId,
        observer: Arc<dyn DiagnosticObserver>,
    ) -> Result<BackendCleanupDispositionV1, BridgeError> {
        self.cleanup_session_observed(session, CleanupStrength::Forget, observer)
            .await
    }

    async fn release_session(&self, session: &SessionId) {
        let _ = self
            .cleanup_session(session, CleanupStrength::Release)
            .await;
    }

    async fn release_session_checked(
        &self,
        session: &SessionId,
    ) -> Result<BackendCleanupDispositionV1, BridgeError> {
        self.cleanup_session(session, CleanupStrength::Release)
            .await
    }

    async fn release_session_observed(
        &self,
        session: &SessionId,
        observer: Arc<dyn DiagnosticObserver>,
    ) -> Result<BackendCleanupDispositionV1, BridgeError> {
        self.cleanup_session_observed(session, CleanupStrength::Release, observer)
            .await
    }

    /// §5.1 step 6's barrier, from the caller's side: preserve this session's checkout BEFORE the
    /// caller signals the session or its resources.
    ///
    /// This backend does NOT forward to `inner`. A checkout belongs to the worktree layer; an
    /// inner ACP/API/container backend owns none, so forwarding would only give a wrapped backend
    /// a chance to answer a question about an object it does not have.
    ///
    /// Three effects, in this order, and the order matters:
    ///
    /// 1. **The transitions run first** — `LiveProtected → PreservationPrepared →
    ///    Preserved | PreservationUnknown` — so the durable claim exists before the caller's next
    ///    line, which is the death signal.
    /// 2. **The session's checkout disposition is raised to `Preserve`**, monotonically, so every
    ///    later cleanup flight for this session serves preservation and no equal-strength reclaim
    ///    can join a preserve flight (or the reverse).
    /// 3. **A terminally-preserved `Ready` entry becomes `Retained`**, which is what stops
    ///    `configure_session` handing a checkout awaiting R2f2 disposition to the next session
    ///    (2b1 opus S-3).
    ///
    /// # RULING — context-free callers must NOT arm preservation (slice 2c1 review)
    ///
    /// `SessionManager` (its eleven direct `release_session` sites, `ExpiryClaim`'s three entry
    /// APIs, the idle reaper), `BindingGuard::Drop`, `ConfigureAdmission::Drop` and controller
    /// retire must **not** call this method, and none of them does. It was proposed that a manager
    /// should preserve before every cancel; that is refused, and the reason is mechanical rather
    /// than stylistic: those callers have no workflow outcome by construction — a reaper fires on
    /// idleness, a `Drop` on a dropped future — so an unconditional manager-side `Preserve` would
    /// terminalize the checkout of a perfectly healthy warm session that merely went quiet, and
    /// `Preserved` is R2f1b-terminal. Only an R2f2 disposition could undo it.
    ///
    /// What those callers get instead is the fail-closed gate: their teardown reaches
    /// `run_cleanup_flight`, the gate refuses on custody evidence, and the report says `Retained`.
    /// The checkout survives with no claim, and the workflow-level owner (2c2's post-loop mint)
    /// makes the actual disposition decision when it has the outcome to make it with. Losing the
    /// exact claim in that window is the accepted cost, and it is bounded by 2c2 landing.
    async fn preserve_checkout_v1(
        &self,
        session: &SessionId,
        reason: CheckoutPreservationReasonV1,
    ) -> CheckoutPreservationV1 {
        let reason = match reason {
            CheckoutPreservationReasonV1::NodeFailure => PreservationReasonV1::NodeFailure,
            CheckoutPreservationReasonV1::Cancellation => PreservationReasonV1::Cancellation,
        };
        let entry = match self.map.lock().await.get(session.as_str()) {
            Some(WtState::Ready(entry)) | Some(WtState::Retained { entry, .. }) => entry.clone(),
            Some(WtState::Reserving { entry, .. }) => entry.clone(),
            None => return CheckoutPreservationV1::NoCheckoutUnderCustody,
        };
        if entry.custody != WtCustodyV1::Protected {
            return CheckoutPreservationV1::NoCheckoutUnderCustody;
        }
        if self
            .raise_checkout_disposition(session, CheckoutDispositionV1::Preserve, Some(reason))
            .await
            .is_none()
        {
            return CheckoutPreservationV1::NoCheckoutUnderCustody;
        }
        // 3s linearization: the raise above queues at the deletion-admission guard, so an
        // in-flight capability mint completes before it returns — and (repair R1, both review
        // lenses) the mint holds that guard THROUGH its map projection, so this post-admission
        // re-read is exact in BOTH directions: absence means removal preceded the raise, and
        // presence means the checkout genuinely still exists (no removal can be mid-projection
        // while this writer holds admission, and no new mint can run once `Preserve` is raised).
        let entry = match self.map.lock().await.get(session.as_str()) {
            Some(WtState::Ready(entry)) | Some(WtState::Retained { entry, .. }) => entry.clone(),
            Some(WtState::Reserving { entry, .. }) => entry.clone(),
            None => return CheckoutPreservationV1::NoCheckoutUnderCustody,
        };
        if entry.custody != WtCustodyV1::Protected {
            return CheckoutPreservationV1::NoCheckoutUnderCustody;
        }
        let outcome = preserve_entry_checkout(&entry, reason).await;
        let retention = match &outcome {
            PreservationOutcomeV1::Preserved | PreservationOutcomeV1::AlreadyPreserved => {
                Some(CheckoutRetentionV1::Preserved)
            }
            PreservationOutcomeV1::PreservationUnknown(_)
            | PreservationOutcomeV1::AlreadyUnknown => {
                Some(CheckoutRetentionV1::PreservationUnknown)
            }
            // Repair RE-6: an ambiguous outcome is NOT a `PreservationUnknown` record. After an
            // ambiguous prepared publication the disk says `PreservationPrepared`, and repair RA
            // makes that resumable, so it must stay distinguishable from a terminal state.
            PreservationOutcomeV1::Ambiguous(_) => Some(CheckoutRetentionV1::PreservationAmbiguous),
            PreservationOutcomeV1::Refused(_) => None,
        };
        if let Some(retention) = retention {
            let mut map = self.map.lock().await;
            let promote = matches!(
                map.get(session.as_str()),
                Some(WtState::Ready(current)) if current.worktree_path == entry.worktree_path
            );
            if promote {
                map.insert(
                    session.as_str().to_owned(),
                    WtState::Retained { entry, retention },
                );
                self.notify.notify_waiters();
            }
        }
        match outcome {
            PreservationOutcomeV1::Preserved | PreservationOutcomeV1::AlreadyPreserved => {
                CheckoutPreservationV1::Preserved
            }
            PreservationOutcomeV1::PreservationUnknown(reason) => {
                CheckoutPreservationV1::Unknown(format!("{reason:?}"))
            }
            PreservationOutcomeV1::AlreadyUnknown => {
                CheckoutPreservationV1::Unknown("already preservation-unknown".to_string())
            }
            PreservationOutcomeV1::Ambiguous(detail) => CheckoutPreservationV1::Ambiguous(detail),
            PreservationOutcomeV1::Refused(detail) => CheckoutPreservationV1::Refused(detail),
        }
    }

    /// §5.1's workflow-level checkout disposition (slice 2c2) — the post-loop settlement, and the
    /// ONLY entry point in the workspace from which a `DeletionCapabilityV1` can be minted.
    ///
    /// # The V2 boundary is the first check, and it is load-bearing
    ///
    /// A non-custody entry returns `NoCheckoutUnderCustody` before any disposition is raised and
    /// before any flight is started, so a legacy checkout's teardown is byte-identical to
    /// pre-2c2: the executor may call this for every session it configured without knowing which
    /// ones are V3, and a V2 session gets no extra release, no extra probe, and no extra cleanup
    /// flight. The worktree layer is the only layer that knows, so it is the one that answers.
    ///
    /// # The two arms
    ///
    /// * **Not healthy** — exactly [`Self::preserve_checkout_v1`]'s behaviour, reached through the
    ///   same helper. This is what disposes of P6's gate-retained context-free deaths: a checkout
    ///   whose session a reaper or a `Drop` tore down mid-run is `Retained` with NO claim (2c1's
    ///   ruling), and this pass is where it finally gets one.
    /// * **Globally healthy** — raise the disposition to `DeleteAuthorized` (monotonically: a
    ///   checkout already at `Preserve` is NOT lowered, so a preserved sibling cannot be deleted by
    ///   a later healthy projection) and run one ordinary `Release` cleanup flight. The mint,
    ///   the capability, and its consumption all live inside that flight, so the capability never
    ///   escapes the function that creates it.
    ///
    /// Running the healthy arm through the cleanup flight rather than beside it is deliberate: it
    /// is what makes the removal single-flighted against every concurrent teardown of the same
    /// session, gives it 2c1's typed `CleanupReportV1`, and composes the map-entry lifecycle
    /// (`Retained` cleared exactly once) with a capability-driven removal.
    async fn settle_workflow_checkout_v1(
        &self,
        session: &SessionId,
        outcome: WorkflowCheckoutOutcomeV1,
    ) -> CheckoutSettlementV1 {
        let mapped = match self.map.lock().await.get(session.as_str()) {
            Some(WtState::Ready(entry))
            | Some(WtState::Retained { entry, .. })
            | Some(WtState::Reserving { entry, .. }) => entry.clone(),
            None => return CheckoutSettlementV1::NoCheckoutUnderCustody,
        };
        if mapped.custody != WtCustodyV1::Protected {
            return CheckoutSettlementV1::NoCheckoutUnderCustody;
        }
        let reason = match outcome {
            WorkflowCheckoutOutcomeV1::NotHealthy(reason) => {
                return match self.preserve_checkout_v1(session, reason).await {
                    CheckoutPreservationV1::Preserved => CheckoutSettlementV1::Preserved,
                    CheckoutPreservationV1::Unknown(_detail) => CheckoutSettlementV1::Preserved,
                    CheckoutPreservationV1::Ambiguous(detail) => {
                        CheckoutSettlementV1::Ambiguous(detail)
                    }
                    CheckoutPreservationV1::Refused(detail) => {
                        CheckoutSettlementV1::Refused(detail)
                    }
                    CheckoutPreservationV1::NoCheckoutUnderCustody => {
                        CheckoutSettlementV1::NoCheckoutUnderCustody
                    }
                };
            }
            WorkflowCheckoutOutcomeV1::GloballyHealthy => None,
        };
        if self
            .raise_checkout_disposition(session, CheckoutDispositionV1::DeleteAuthorized, reason)
            .await
            .is_none()
        {
            return CheckoutSettlementV1::Refused(
                "the worktree backend is retiring; the checkout stays protected".to_string(),
            );
        }
        let report = self
            .cleanup_session_reported(session, CleanupStrength::Release, false)
            .await;
        match report.checkout {
            CheckoutCleanupDispositionV1::Removed => CheckoutSettlementV1::Removed,
            CheckoutCleanupDispositionV1::RemovedRecordAmbiguous(detail) => {
                CheckoutSettlementV1::RemovedRecordAmbiguous(detail)
            }
            CheckoutCleanupDispositionV1::Preserved => CheckoutSettlementV1::Preserved,
            CheckoutCleanupDispositionV1::NotNeeded => CheckoutSettlementV1::NoCheckoutUnderCustody,
            CheckoutCleanupDispositionV1::Retained
            | CheckoutCleanupDispositionV1::RemovalFailed => {
                CheckoutSettlementV1::Retained(match &report.result {
                    Ok(_) => "the checkout was retained under R2f1b custody".to_string(),
                    Err(error) => format!("{error:?}"),
                })
            }
        }
    }

    async fn reconcile_config(
        &self,
        session: &SessionId,
        spec: &SessionSpec,
    ) -> Result<ReconcileOutcome, BridgeError> {
        // DECISION (slice 2c1 review, opus S7): `WtState::Retained` deliberately gets NO arm here
        // and falls through the wildcard to "not mapped". Reconciliation only forwards model and
        // effort to a live inner session; it hands no checkout to anybody and performs no
        // filesystem work, so the reuse hazard `Retained` exists to stop is not present. Routing a
        // retained checkout's path into a reconcile would be strictly worse — it would name a
        // preserved cwd to a live session — and refusing outright would break reconciliation for a
        // session whose checkout is merely retained. Falling through reconciles against the
        // caller's own spec, which is correct for both.
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

    fn terminal_evidence_capability(&self) -> EvidenceCapability {
        self.inner.terminal_evidence_capability()
    }

    fn bridge_owned_acp_child_liveness(&self) -> AcpChildLiveness {
        self.inner.bridge_owned_acp_child_liveness()
    }

    async fn retire(&self) -> Result<(), BridgeError> {
        self.inner.resource_flight_v1()?;
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
        sessions.extend(
            self.preparation_flights
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
    use bridge_core::attempt_activity::MonotonicClock;
    use bridge_core::domain::{AgentEntry, AgentKind, EffectiveConfig, Part, SessionSpec};
    use bridge_core::error::BridgeError;
    use bridge_core::execution_policy::{
        freeze_direct_checkout_v1, freeze_provider_attempt_v1, freeze_worktree_checkout_v1,
        BoundSessionSpecV1, FrozenProviderLogicalSessionV1, PolicyNodeRefV1, ProviderFreezeInputV1,
        WorktreeCheckoutInputV1,
    };
    use bridge_core::ids::{AgentId, AttemptId, SessionId};
    use bridge_core::mcp::{McpDelivery, McpServerSpec};
    use bridge_core::ports::{
        AgentBackend, BackendStream, DiagnosticObserver, RichEventSink, Update,
    };
    use bridge_core::SessionCwd;
    use std::collections::BTreeMap;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, SystemTime, UNIX_EPOCH};
    use tokio::sync::{oneshot, Notify};
    use tokio_stream::StreamExt;

    struct ManualPreparationClock(AtomicU64);

    impl ManualPreparationClock {
        fn new(elapsed_ms: u64) -> Self {
            Self(AtomicU64::new(elapsed_ms))
        }

        fn set(&self, elapsed_ms: u64) {
            self.0.store(elapsed_ms, Ordering::SeqCst);
        }
    }

    impl MonotonicClock for ManualPreparationClock {
        fn elapsed_ms(&self) -> u64 {
            self.0.load(Ordering::SeqCst)
        }
    }

    #[derive(Default)]
    struct Rec {
        configured_cwd: Mutex<Vec<Option<String>>>,
        bound_configure_count: AtomicUsize,
        added_worktrees: Mutex<Vec<(String, String)>>,
        /// The custody-record state observed by the provider AT THE MOMENT the add runs — the
        /// creation-ordering witness. `None` means no record existed when the add was entered.
        record_state_at_add: Mutex<Vec<Option<String>>>,
        /// The legacy sidecar's existence at the same instant, so "V3 writes no `.meta.json`"
        /// is checked while the checkout is live rather than only after teardown.
        legacy_sidecar_at_add: Mutex<Vec<bool>>,
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
        /// Counted SEPARATELY from `remove_count` (slice 2c2): the whole §2c claim is that a
        /// custody-discriminated checkout is removable only through the capability method, so a
        /// single counter could never distinguish "removed with authority" from "removed by the
        /// raw-path V2 call".
        remove_v2_count: AtomicUsize,
        /// P7 boundary 2: the git removal fails. The record must NOT say `Removed`.
        fail_remove_v2: AtomicBool,
        /// P7 boundary 4: the removal reports success while the target is still there — a
        /// post-condition disagreement. The real provider's `remove_and_verify` turns that into an
        /// `Err`; this double reproduces it as an `Err` after leaving the target in place, which is
        /// the state the writer must refuse to tombstone.
        remove_v2_leaves_target: AtomicBool,
        fail_release: AtomicBool,
        cleanup_disposition: Mutex<BackendCleanupDispositionV1>,
        flight_protected: AtomicBool,
        flight_attach_count: AtomicUsize,
        retire_count: AtomicUsize,
        retire_gate: Mutex<Option<oneshot::Receiver<()>>>,
        retire_started_count: AtomicUsize,
        retire_started: Notify,
        composite_count: AtomicUsize,
        evidence_v1: AtomicBool,
        child_exited: AtomicBool,
        diagnostics: Mutex<Vec<Arc<dyn DiagnosticObserver>>>,
        rich_sinks: Mutex<Vec<Arc<dyn RichEventSink>>>,
        /// The checkout the ORDERING WITNESS watches, set by a test before it drives a teardown.
        watch_checkout: Mutex<Option<String>>,
        /// `(signal, custody state at that instant)` for every session/resource death signal the
        /// inner backend receives. This is the §5.1 step 6 witness taken from the far side of the
        /// barrier: what the record said at the moment the signal actually landed.
        record_state_at_signal: Mutex<Vec<(String, Option<String>)>>,
    }

    /// Read the custody record's state tag beside `worktree_path`, if there is a readable one.
    fn observed_record_state(worktree_path: &str) -> Option<String> {
        let bytes = std::fs::read(crate::custody::custody_record_path(worktree_path)).ok()?;
        crate::custody::WorktreeCustodyRecordV1::decode_canonical(&bytes)
            .ok()
            .map(|record| record.state.kind().wire_tag())
    }

    /// The §5.1 ordering witness, recorded INSIDE the inner backend: the custody state of the
    /// watched checkout at the instant a death signal arrives. A barrier that ran after the signal
    /// — or not at all — is indistinguishable from one that ran before it by any other means.
    fn note_signal(rec: &Rec, signal: &str) {
        let watched = rec.watch_checkout.lock().unwrap().clone();
        let state = watched.as_deref().and_then(observed_record_state);
        rec.record_state_at_signal
            .lock()
            .unwrap()
            .push((signal.to_string(), state));
    }

    fn note_ordering(rec: &Rec, worktree_path: &str) {
        rec.record_state_at_add
            .lock()
            .unwrap()
            .push(observed_record_state(worktree_path));
        rec.legacy_sidecar_at_add
            .lock()
            .unwrap()
            .push(Path::new(&sidecar_path(worktree_path)).exists());
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

        fn terminal_evidence_capability(&self) -> EvidenceCapability {
            if self.rec.evidence_v1.load(Ordering::SeqCst) {
                EvidenceCapability::V1
            } else {
                EvidenceCapability::Unsupported
            }
        }

        fn bridge_owned_acp_child_liveness(&self) -> AcpChildLiveness {
            if self.rec.child_exited.load(Ordering::SeqCst) {
                AcpChildLiveness::Exited
            } else {
                AcpChildLiveness::Unknown
            }
        }

        async fn cancel(&self, _session: &SessionId) -> Result<(), BridgeError> {
            self.rec.order.lock().unwrap().push("inner_cancel".into());
            note_signal(&self.rec, "inner_cancel");
            Ok(())
        }

        fn resource_flight_v1(&self) -> Result<BackendResourceFlightV1, BridgeError> {
            Ok(if self.rec.flight_protected.load(Ordering::SeqCst) {
                BackendResourceFlightV1::ProtectedV3
            } else {
                BackendResourceFlightV1::LegacyV2
            })
        }

        fn attach_resource_flight_owner_v1(
            &self,
            _session: &SessionId,
        ) -> Result<BackendResourceFlightV1, BridgeError> {
            self.rec.flight_attach_count.fetch_add(1, Ordering::SeqCst);
            self.resource_flight_v1()
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

        async fn configure_bound_session(
            &self,
            _session: &SessionId,
            spec: &BoundSessionSpecV1,
        ) -> Result<(), BridgeError> {
            self.rec
                .bound_configure_count
                .fetch_add(1, Ordering::SeqCst);
            self.rec
                .configured_cwd
                .lock()
                .unwrap()
                .push(spec.session.cwd.as_ref().map(|c| c.as_str().to_string()));
            Ok(())
        }

        async fn forget_session(&self, _session: &SessionId) {
            self.rec.order.lock().unwrap().push("inner_forget".into());
            note_signal(&self.rec, "inner_forget");
        }

        async fn release_session(&self, _session: &SessionId) {
            self.rec.order.lock().unwrap().push("inner_release".into());
            note_signal(&self.rec, "inner_release");
        }

        async fn release_session_checked(
            &self,
            session: &SessionId,
        ) -> Result<bridge_core::ports::BackendCleanupDispositionV1, BridgeError> {
            self.release_session(session).await;
            if self.rec.fail_release.load(Ordering::SeqCst) {
                Err(BridgeError::StoreFailure)
            } else {
                Ok(*self.rec.cleanup_disposition.lock().unwrap())
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

    struct UnforwardedInner {
        rec: Arc<Rec>,
    }

    #[async_trait::async_trait]
    impl AgentBackend for UnforwardedInner {
        async fn prompt(
            &self,
            _session: &SessionId,
            _parts: Vec<Part>,
        ) -> Result<BackendStream, BridgeError> {
            Ok(Box::pin(tokio_stream::empty()))
        }

        async fn cancel(&self, _session: &SessionId) -> Result<(), BridgeError> {
            self.rec
                .order
                .lock()
                .unwrap()
                .push("unforwarded_cancel".into());
            Ok(())
        }

        async fn release_session(&self, _session: &SessionId) {
            self.rec
                .order
                .lock()
                .unwrap()
                .push("unforwarded_release".into());
        }

        async fn retire(&self) -> Result<(), BridgeError> {
            self.rec.retire_count.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    struct FakeProv {
        rec: Arc<Rec>,
    }

    struct NonGitProv {
        rec: Arc<Rec>,
    }

    /// `FakeInner`, except its BOUND configure honours `fail_configure`. `FakeInner`'s always
    /// succeeds, which is correct for the tests that predate slice 2c1 but leaves the V3
    /// inner-configure failure arm — the one repair RD is about — unreachable.
    struct ConfigureFailInner {
        rec: Arc<Rec>,
    }

    #[async_trait::async_trait]
    impl AgentBackend for ConfigureFailInner {
        async fn prompt(
            &self,
            session: &SessionId,
            parts: Vec<Part>,
        ) -> Result<BackendStream, BridgeError> {
            FakeInner {
                rec: self.rec.clone(),
            }
            .prompt(session, parts)
            .await
        }

        fn resource_flight_v1(&self) -> Result<BackendResourceFlightV1, BridgeError> {
            FakeInner {
                rec: self.rec.clone(),
            }
            .resource_flight_v1()
        }

        fn attach_resource_flight_owner_v1(
            &self,
            session: &SessionId,
        ) -> Result<BackendResourceFlightV1, BridgeError> {
            FakeInner {
                rec: self.rec.clone(),
            }
            .attach_resource_flight_owner_v1(session)
        }

        async fn cancel(&self, session: &SessionId) -> Result<(), BridgeError> {
            FakeInner {
                rec: self.rec.clone(),
            }
            .cancel(session)
            .await
        }

        /// Honours BOTH `fail_configure` and the blocking gate. `FakeInner`'s bound configure
        /// honours neither, so the V3 inner-configure failure arm and the cancellation window
        /// around it — the two states repair RD is about — were unreachable before this double.
        async fn configure_bound_session(
            &self,
            _session: &SessionId,
            _spec: &BoundSessionSpecV1,
        ) -> Result<(), BridgeError> {
            self.rec
                .bound_configure_count
                .fetch_add(1, Ordering::SeqCst);
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

        async fn forget_session(&self, session: &SessionId) {
            FakeInner {
                rec: self.rec.clone(),
            }
            .forget_session(session)
            .await;
        }

        async fn release_session(&self, session: &SessionId) {
            FakeInner {
                rec: self.rec.clone(),
            }
            .release_session(session)
            .await;
        }
    }

    struct SidecarWriteFailProv {
        rec: Arc<Rec>,
    }

    struct PartialAddFailProv {
        rec: Arc<Rec>,
        /// Flip the custody-aware add from "target created, then failed" to "failed before any
        /// target exists" — the two arms of §6's "Partial add preserved" row.
        partial_target_absent: AtomicBool,
    }

    #[async_trait::async_trait]
    impl crate::provider::WorktreeProvider for FakeProv {
        async fn add(&self, repo: &str, worktree_path: &str) -> Result<String, BridgeError> {
            self.rec.add_count.fetch_add(1, Ordering::SeqCst);
            self.rec
                .added_worktrees
                .lock()
                .unwrap()
                .push((repo.to_owned(), worktree_path.to_owned()));
            tokio::task::yield_now().await;
            Ok(String::new())
        }

        fn supports_custody_add(&self) -> bool {
            true
        }

        /// Nine-impl enumeration (R-6), 1 of 10: the V3-capable happy path. Materializes the
        /// target for real, because `LiveProtected` requires the record to carry the target's
        /// OBSERVED `dev`/`ino` and a double that adds nothing could never produce one.
        async fn add_under_custody(
            &self,
            repo: &str,
            worktree_path: &str,
        ) -> Result<CustodyAddOutcomeV1, BridgeError> {
            self.rec.add_count.fetch_add(1, Ordering::SeqCst);
            self.rec
                .added_worktrees
                .lock()
                .unwrap()
                .push((repo.to_owned(), worktree_path.to_owned()));
            note_ordering(&self.rec, worktree_path);
            std::fs::create_dir_all(worktree_path).unwrap();
            // The common dir is CREATED, not merely named. A real `git worktree add` reports a
            // path that exists, and slice 2c1 depends on it: the four retained identities must be
            // observable, since 2a's `identity_completeness` requires observed `dev`/`ino` on
            // every claim identity for a preserving state. A double that names a non-existent
            // common dir would make every preservation refuse for a reason production never has.
            let common_dir = format!("{repo}/.git");
            std::fs::create_dir_all(&common_dir).unwrap();
            Ok(CustodyAddOutcomeV1::Materialized { common_dir })
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

        fn supports_capability_removal(&self) -> bool {
            true
        }

        /// Enumeration 2 of 11 (slice 2c2): the capability-consuming removal.
        ///
        /// It REALLY deletes the target, because every assertion that matters here is about
        /// whether the work is still on disk, and a double that only counted calls could not tell
        /// a removal from a refusal. `Err` leaves the target in place, which is exactly what the
        /// real provider's `remove_and_verify` guarantees for an incomplete removal.
        async fn remove_v2(
            &self,
            authorized: crate::custody_writer::AuthorizedRemovalV1,
        ) -> Result<(), BridgeError> {
            self.rec.remove_v2_count.fetch_add(1, Ordering::SeqCst);
            self.rec.order.lock().unwrap().push("wt_remove_v2".into());
            if self.rec.fail_remove_v2.load(Ordering::SeqCst) {
                return Err(BridgeError::StoreFailure);
            }
            if self.rec.remove_v2_leaves_target.load(Ordering::SeqCst) {
                // The post-condition disagreement: git said something, the target is still there,
                // so `remove_and_verify` reports the removal as incomplete rather than complete.
                return Err(BridgeError::ConfigInvalid {
                    reason: "worktree remove failed (target_absent=false)".into(),
                });
            }
            let _ = std::fs::remove_dir_all(authorized.worktree_path());
            Ok(())
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

        // Enumeration 2 of 10: takes the REFUSING default deliberately. This double exists to
        // prove the non-git preflight refusal, which happens before any custody transition.

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

        // Enumeration 3 of 10: REFUSING default, and it must stay that way — this double
        // sabotages the legacy `.meta.json` write, which the V3 path never performs at all.

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

        fn supports_custody_add(&self) -> bool {
            true
        }

        /// Enumeration 4 of 10: the PARTIAL ADD double — it creates the target and then fails,
        /// which is §5.7 row 4's exact shape ("during/after partial add, before live identity").
        /// `partial_target_absent` flips it to the failure-before-any-target case.
        async fn add_under_custody(
            &self,
            repo: &str,
            worktree_path: &str,
        ) -> Result<CustodyAddOutcomeV1, BridgeError> {
            self.rec.add_count.fetch_add(1, Ordering::SeqCst);
            note_ordering(&self.rec, worktree_path);
            let target = if self.partial_target_absent.load(Ordering::SeqCst) {
                CustodyAddTargetV1::ProvablyAbsent
            } else {
                std::fs::create_dir_all(worktree_path).unwrap();
                std::fs::write(format!("{worktree_path}/work.txt"), b"unsaved work").unwrap();
                CustodyAddTargetV1::Present
            };
            Ok(CustodyAddOutcomeV1::Failed(
                crate::provider::CustodyAddFailureV1 {
                    reason: "injected partial add failure".into(),
                    target,
                    common_dir: Some(format!("{repo}/.git")),
                    recovery_locator: crate::custody::RecoveryLocatorV1::RegistrationUnproven {},
                },
            ))
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

    fn bound_spec_from_checkout(
        logical_session: FrozenProviderLogicalSessionV1,
        checkout: bridge_core::execution_policy::FrozenCheckoutEffectV1,
    ) -> BoundSessionSpecV1 {
        let entry = AgentEntry {
            id: AgentId::parse("bound-worktree").unwrap(),
            cmd: Some("fake-cmd".into()),
            base_url: None,
            api_key_env: None,
            args: vec![],
            kind: AgentKind::Acp,
            model_provider: None,
            model: None,
            effort: None,
            mode: None,
            preflight: false,
            fallback_models: vec![],
            cwd: None,
            session_cwd: None,
            sandbox: None,
            watchdog: None,
            mcp: vec![McpServerSpec {
                name: "repo".into(),
                command: "/bin/repo".into(),
                args: vec!["--root".into(), "{cwd}".into()],
                env: vec![],
            }],
            mcp_delivery: McpDelivery::Acp,
            auth_method: None,
            pre_authenticated: true,
            host_fallback_eligible: false,
            name: None,
            description: None,
            tags: vec![],
            version: None,
            extensions: BTreeMap::new(),
        };
        let bundle = freeze_provider_attempt_v1(&ProviderFreezeInputV1 {
            entry: &entry,
            overrides: None,
            node: PolicyNodeRefV1::from_node_id(0, "node"),
            logical_session,
            checkout,
            provider_effect_key: None,
        })
        .unwrap();
        BoundSessionSpecV1::new(EffectiveConfig::default(), Arc::new(bundle.bound))
    }

    fn bound_spec(
        source: &Path,
        cfg: &crate::provider_path::WorktreeConfig,
    ) -> (BoundSessionSpecV1, String) {
        let source = std::fs::canonicalize(source).unwrap();
        let source_cwd = SessionCwd::parse(&source.to_string_lossy()).unwrap();
        let logical_session = FrozenProviderLogicalSessionV1::Execute {
            candidate_ordinal: 0,
        };
        let checkout = freeze_worktree_checkout_v1(&WorktreeCheckoutInputV1 {
            attempt_id: AttemptId::parse("attempt-22222222222222222222222222222222").unwrap(),
            node: PolicyNodeRefV1::from_node_id(0, "node"),
            logical_session,
            source_cwd: source_cwd.clone(),
            canonical_source_cwd: source_cwd,
            canonical_worktree_root: SessionCwd::parse(&cfg.root).unwrap(),
            worktree_owner: cfg.owner.clone(),
        })
        .unwrap();
        let target = checkout.effective_cwd().as_str().to_owned();
        (bound_spec_from_checkout(logical_session, checkout), target)
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

    fn flight_only_backend(inner: Arc<dyn AgentBackend>, rec: Arc<Rec>) -> WorktreeBackend {
        WorktreeBackend::new(
            inner,
            Arc::new(FakeProv { rec }),
            WorktreeConfig {
                root: "/tmp".into(),
                owner: "ownr".into(),
                run: "run7".into(),
            },
            None,
            identity(),
        )
    }

    #[tokio::test]
    async fn protected_flight_exposure_and_attachment_forward_to_inner_teardown() {
        let rec = Arc::new(Rec::default());
        rec.flight_protected.store(true, Ordering::SeqCst);
        let backend = flight_only_backend(Arc::new(FakeInner { rec: rec.clone() }), rec.clone());
        let session = SessionId::parse("protected-forwarding").unwrap();

        assert_eq!(
            backend.resource_flight_v1().unwrap(),
            BackendResourceFlightV1::ProtectedV3
        );
        assert_eq!(
            backend.attach_resource_flight_owner_v1(&session).unwrap(),
            BackendResourceFlightV1::ProtectedV3
        );
        backend.retire().await.unwrap();

        assert_eq!(rec.flight_attach_count.load(Ordering::SeqCst), 1);
        assert_eq!(rec.retire_count.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn default_unforwarded_flight_cannot_signal_through_worktree_cleanup_or_retire() {
        let rec = Arc::new(Rec::default());
        let backend =
            flight_only_backend(Arc::new(UnforwardedInner { rec: rec.clone() }), rec.clone());

        let session = SessionId::parse("unforwarded-cleanup").unwrap();
        assert_eq!(
            backend.release_session_checked(&session).await,
            Err(BridgeError::ResourceFlightUnsupported)
        );
        assert!(rec.order.lock().unwrap().is_empty());

        assert_eq!(
            backend.retire().await,
            Err(BridgeError::ResourceFlightUnsupported)
        );
        assert_eq!(rec.retire_count.load(Ordering::SeqCst), 0);
        assert!(rec.order.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn bound_worktree_configure_consumes_frozen_target_without_legacy_derivation() {
        let (be, rec, tmp, source, cfg) = backend_fixture("bound-frozen-target");
        let (bound, target) = bound_spec(&source, &cfg);
        let session = SessionId::parse("v2-bound-worktree").unwrap();

        be.configure_bound_session(&session, &bound).await.unwrap();

        assert_eq!(rec.configure_count.load(Ordering::SeqCst), 0);
        assert_eq!(rec.bound_configure_count.load(Ordering::SeqCst), 1);
        assert_eq!(rec.add_count.load(Ordering::SeqCst), 1);
        let added = rec.added_worktrees.lock().unwrap().clone();
        assert_eq!(added.len(), 1);
        assert_eq!(
            added[0].0,
            std::fs::canonicalize(&source).unwrap().to_string_lossy()
        );
        assert_eq!(added[0].1, target);
        assert_eq!(
            rec.configured_cwd.lock().unwrap().as_slice(),
            [Some(target.clone())]
        );
        assert!(!target.contains(&cfg.run));

        be.forget_session_checked(&session).await.unwrap();
        std::fs::remove_dir_all(tmp).unwrap();
    }

    #[tokio::test]
    async fn bound_direct_checkout_bypasses_worktree_provider_and_uses_bound_inner_path() {
        let (be, rec, tmp, source, _cfg) = backend_fixture("bound-direct");
        let source = std::fs::canonicalize(source).unwrap();
        let source_cwd = SessionCwd::parse(&source.to_string_lossy()).unwrap();
        let logical_session = FrozenProviderLogicalSessionV1::Execute {
            candidate_ordinal: 0,
        };
        let bound = bound_spec_from_checkout(
            logical_session,
            freeze_direct_checkout_v1(source_cwd.clone()),
        );
        let session = SessionId::parse("v2-bound-direct").unwrap();

        be.configure_bound_session(&session, &bound).await.unwrap();

        assert_eq!(rec.configure_count.load(Ordering::SeqCst), 0);
        assert_eq!(rec.bound_configure_count.load(Ordering::SeqCst), 1);
        assert_eq!(rec.add_count.load(Ordering::SeqCst), 0);
        assert_eq!(
            rec.configured_cwd.lock().unwrap().as_slice(),
            [Some(source_cwd.as_str().to_owned())]
        );
        be.forget_session_checked(&session).await.unwrap();
        std::fs::remove_dir_all(tmp).unwrap();
    }

    #[tokio::test]
    async fn bound_worktree_configuration_drift_refuses_before_checkout_or_inner_configure() {
        let (be, rec, tmp, source, cfg) = backend_fixture("bound-config-drift");
        let mut drifted_cfg = cfg;
        drifted_cfg.owner = "other-owner".into();
        let (bound, _target) = bound_spec(&source, &drifted_cfg);
        let session = SessionId::parse("v2-bound-config-drift").unwrap();

        assert_eq!(
            be.configure_bound_session(&session, &bound).await,
            Err(BridgeError::ConfigMismatch {
                field: "bound worktree configuration"
            })
        );
        assert_eq!(rec.add_count.load(Ordering::SeqCst), 0);
        assert_eq!(rec.configure_count.load(Ordering::SeqCst), 0);
        assert_eq!(rec.bound_configure_count.load(Ordering::SeqCst), 0);
        std::fs::remove_dir_all(tmp).unwrap();
    }

    #[tokio::test]
    async fn bound_worktree_restart_changes_runtime_custody_but_reuses_persisted_target() {
        let (first, rec, tmp, source, cfg) = backend_fixture("bound-restart");
        let (bound, target) = bound_spec(&source, &cfg);
        let session = SessionId::parse("v2-bound-restart").unwrap();
        first
            .configure_bound_session(&session, &bound)
            .await
            .unwrap();
        first.forget_session_checked(&session).await.unwrap();

        let allowed_root = std::fs::canonicalize(tmp.join("allowed")).unwrap();
        let mut restarted_cfg = cfg;
        restarted_cfg.run = "different-process-run".into();
        let restarted = WorktreeBackend::new(
            Arc::new(FakeInner { rec: rec.clone() }),
            Arc::new(FakeProv { rec: rec.clone() }),
            restarted_cfg,
            Some(SessionCwd::parse(&allowed_root.to_string_lossy()).unwrap()),
            WorktreeIdentity {
                run_id: "different-runtime-custodian".into(),
                host: "host-b".into(),
                lease: "/tmp/a2a-bridge-test-restarted.lock".into(),
            },
        );
        restarted
            .configure_bound_session(&session, &bound)
            .await
            .unwrap();

        let added = rec.added_worktrees.lock().unwrap().clone();
        assert_eq!(added.len(), 2);
        assert_eq!(added[0].1, target);
        assert_eq!(added[1].1, target);
        assert!(!target.contains("different-process-run"));
        restarted.forget_session_checked(&session).await.unwrap();
        std::fs::remove_dir_all(tmp).unwrap();
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

    #[test]
    fn stable_inner_capability_and_child_liveness_are_forwarded() {
        let (backend, rec, tmp, _source, _cfg) = backend_fixture("stable-static-forwarding");

        assert_eq!(
            backend.terminal_evidence_capability(),
            EvidenceCapability::Unsupported
        );
        assert_eq!(
            backend.bridge_owned_acp_child_liveness(),
            AcpChildLiveness::Unknown
        );

        rec.evidence_v1.store(true, Ordering::SeqCst);
        rec.child_exited.store(true, Ordering::SeqCst);
        assert_eq!(
            backend.terminal_evidence_capability(),
            EvidenceCapability::V1
        );
        assert_eq!(
            backend.bridge_owned_acp_child_liveness(),
            AcpChildLiveness::Exited
        );

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
            wait_for_cleanup_report(release_report).await.result,
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
            Arc::new(PartialAddFailProv {
                rec: rec.clone(),
                partial_target_absent: AtomicBool::new(false),
            }),
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
        // Enumeration 5 of 10: REFUSING default. This double exists to gate V2 cleanup and
        // probe concurrency; no custody transition runs through it.
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
        // Enumeration 6 of 10: REFUSING default. This double exists to gate V2 cleanup and
        // probe concurrency; no custody transition runs through it.
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
        // Enumeration 7 of 10: REFUSING default. This double exists to gate V2 cleanup and
        // probe concurrency; no custody transition runs through it.
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

    // =========================================================================================
    // R2f1b slice 2b1 — the fail-closed deletion gate.
    //
    // The whole deletion fan-in (brief R-11) reaches ONE block: the `provider.remove` + sidecar
    // removal sequence in `run_cleanup_flight`. The tests below drive every family that reaches
    // it from inside this crate; `tests/r2f1b_deletion_gate.rs` drives the external subsystems.
    // Each test names the family (or families) it stands for.
    // =========================================================================================

    /// Publish a real, canonically-encodable V3 custody record beside `worktree_path`.
    ///
    /// Deliberately a REAL record rather than a placeholder file: the gate keys on presence, so a
    /// junk file would pass every assertion here while proving nothing about a genuine V3
    /// checkout. The one test that needs the presence-not-decode property writes junk explicitly.
    fn publish_custody_record_with_state(
        worktree_path: &str,
        state: crate::custody::WorktreeCustodyStateV1,
    ) {
        use crate::custody::{
            custody_record_path, WorktreeCustodyRecordV1, WORKTREE_CUSTODY_RECORD_SCHEMA_V1,
        };
        use bridge_core::execution_policy::{
            Sha256HexV1, WorktreeCustodyIdV1, WorktreeObjectIdentityV1,
        };
        use bridge_core::fs_custody::DirectoryIdentityV1;
        use bridge_core::ids::{AttemptIdentity, ExecutionId};

        let record = WorktreeCustodyRecordV1 {
            schema_version: WORKTREE_CUSTODY_RECORD_SCHEMA_V1,
            custody_id: WorktreeCustodyIdV1::parse(format!("custody-{}", "3".repeat(64))).unwrap(),
            checkout_fingerprint: Sha256HexV1::parse("6".repeat(64)).unwrap(),
            current_attempt: AttemptIdentity {
                execution_id: ExecutionId::parse(format!("exec-{}", "1".repeat(32))).unwrap(),
                attempt_id: AttemptId::parse(format!("attempt-{}", "2".repeat(32))).unwrap(),
                ordinal: 0,
                parent_attempt_id: None,
            },
            worktree: WorktreeObjectIdentityV1 {
                canonical_path: worktree_path.to_owned(),
                directory_identity: DirectoryIdentityV1 {
                    canonical_path: worktree_path.to_owned(),
                    dev: Some(1),
                    ino: Some(2),
                    btime: None,
                },
            },
            state,
            claim: None,
        };
        std::fs::write(
            custody_record_path(worktree_path),
            record.encode_canonical().unwrap(),
        )
        .unwrap();
    }

    fn publish_custody_record(worktree_path: &str) {
        publish_custody_record_with_state(
            worktree_path,
            crate::custody::WorktreeCustodyStateV1::LiveProtected {},
        );
    }

    /// The target a legacy `configure_session` will resolve for this session, computed before the
    /// call so a record can already be in place when a rollback path runs.
    fn legacy_target(
        tmp: &Path,
        source: &Path,
        cfg: &crate::provider_path::WorktreeConfig,
        session: &str,
    ) -> String {
        let allowed_root = std::fs::canonicalize(tmp.join("allowed")).unwrap();
        let source = std::fs::canonicalize(source).unwrap();
        resolve_worktree(
            cfg,
            &Some(SessionCwd::parse(&allowed_root.to_string_lossy()).unwrap()),
            &source.to_string_lossy(),
            session,
        )
        .unwrap()
        .worktree_path
    }

    fn removals(rec: &Rec) -> usize {
        rec.remove_count.load(Ordering::SeqCst)
    }

    /// FAMILY: forget (`backend.rs` `forget_session` / `forget_session_checked`), and by path
    /// collapse `BindingGuard::Drop` (`bridge-coordinator/src/dispatch.rs:63` calls exactly
    /// `backend.forget_session(&session)`) — the cross-crate half of that family is pinned
    /// end-to-end in `tests/r2f1b_deletion_gate.rs`.
    ///
    /// Also one half of STRENGTH INDEPENDENCE: this is `CleanupStrength::Forget`; its twin
    /// `release_refuses_...` below is `::Release`. The gate consults neither.
    ///
    /// Asserts the refusal is scoped to the checkout: the inner session teardown still runs.
    #[tokio::test]
    async fn forget_refuses_to_delete_a_checkout_that_has_a_custody_record() {
        let (be, rec, tmp, source, _cfg) = backend_fixture("gate-forget");
        let sid = SessionId::parse("ctx-gate-forget-g0").unwrap();
        be.configure_session(&sid, &spec(Some(&source.to_string_lossy())))
            .await
            .unwrap();
        let target = be.mapped_worktree_path_for_test(&sid).await.unwrap();
        publish_custody_record(&target);

        be.forget_session_checked(&sid).await.unwrap();

        assert_eq!(
            removals(&rec),
            0,
            "a custody-protected checkout must not be removed"
        );
        assert!(rec
            .order
            .lock()
            .unwrap()
            .contains(&"inner_forget".to_string()));
        assert!(
            !be.map.lock().await.is_empty(),
            "a refused entry must stay mapped, not be dropped as if it were cleaned"
        );
        std::fs::remove_dir_all(tmp).unwrap();
    }

    /// FAMILY: release (`release_session` / `_checked` / the eleven direct `release_session` call
    /// sites in `bridge-coordinator/src/session_manager.rs`, all of which call this one trait
    /// method), and `ExpiryClaim`'s three entry APIs — `into_flight`, `cleanup()`, and `Drop` all
    /// funnel through `ExpiryClaim::start_flight`, whose only backend call is
    /// `release_session_checked` (`session_manager.rs:356-359`). Verified in source, and driven
    /// end-to-end through the real `reap_idle` chain in `tests/r2f1b_deletion_gate.rs`.
    ///
    /// The second half of STRENGTH INDEPENDENCE (`CleanupStrength::Release`).
    #[tokio::test]
    async fn release_refuses_to_delete_a_checkout_that_has_a_custody_record() {
        let (be, rec, tmp, source, _cfg) = backend_fixture("gate-release");
        let sid = SessionId::parse("ctx-gate-release-g0").unwrap();
        be.configure_session(&sid, &spec(Some(&source.to_string_lossy())))
            .await
            .unwrap();
        let target = be.mapped_worktree_path_for_test(&sid).await.unwrap();
        publish_custody_record(&target);

        be.release_session_checked(&sid).await.unwrap();

        assert_eq!(removals(&rec), 0);
        assert!(rec
            .order
            .lock()
            .unwrap()
            .contains(&"inner_release".to_string()));
        std::fs::remove_dir_all(tmp).unwrap();
    }

    /// RecoveredLive must reach the same context-free gate refusal as LiveProtected.
    /// The gate intentionally keys on durable-record presence rather than decoding a permissive
    /// subset of states, so the claim-synced/lease-untransferred window cannot be deleted.
    #[tokio::test]
    async fn recovered_live_checkout_removal_refusal_matches_live_protected() {
        for (name, state) in [
            (
                "live",
                crate::custody::WorktreeCustodyStateV1::LiveProtected {},
            ),
            (
                "recovered",
                crate::custody::WorktreeCustodyStateV1::RecoveredLive {
                    predecessor_claim_digest: bridge_core::execution_policy::Sha256HexV1::digest(
                        b"predecessor-claim",
                    ),
                },
            ),
        ] {
            let (be, rec, tmp, source, _cfg) = backend_fixture(&format!("gate-{name}"));
            let sid = SessionId::parse(format!("ctx-gate-{name}-g0")).unwrap();
            be.configure_session(&sid, &spec(Some(&source.to_string_lossy())))
                .await
                .unwrap();
            let target = be.mapped_worktree_path_for_test(&sid).await.unwrap();
            publish_custody_record_with_state(&target, state);
            let entry = {
                let map = be.map.lock().await;
                match map.get(sid.as_str()) {
                    Some(WtState::Ready(entry)) => entry.clone(),
                    None => panic!("expected a ready checkout, but no state was stored"),
                    Some(_) => {
                        panic!("expected a ready checkout, but a different state was stored")
                    }
                }
            };

            assert_eq!(
                checkout_removal_refusal(&entry),
                Some(CheckoutRemovalRefusalV1::RecordPresent),
                "{name}: a durable custody record must refuse the exact same gate arm"
            );
            be.release_session_checked(&sid).await.unwrap();
            assert_eq!(removals(&rec), 0, "{name}: the provider must not remove it");
            std::fs::remove_dir_all(tmp).unwrap();
        }
    }

    /// A refusal is about the CHECKOUT only. Discriminates a gate that returns early with a bare
    /// `Ok(())` and swallows a genuine inner-teardown failure recorded before it — which would
    /// report a broken session release as a clean cleanup to every caller in the fan-in.
    #[tokio::test]
    async fn a_refused_checkout_still_reports_a_failed_inner_teardown() {
        let (be, rec, tmp, source, _cfg) = backend_fixture("gate-inner-error");
        let sid = SessionId::parse("ctx-gate-inner-error-g0").unwrap();
        be.configure_session(&sid, &spec(Some(&source.to_string_lossy())))
            .await
            .unwrap();
        publish_custody_record(&be.mapped_worktree_path_for_test(&sid).await.unwrap());
        rec.fail_release.store(true, Ordering::SeqCst);

        assert_eq!(
            be.release_session_checked(&sid).await,
            Err(BridgeError::StoreFailure)
        );

        assert_eq!(removals(&rec), 0);
        std::fs::remove_dir_all(tmp).unwrap();
    }

    /// FAMILY: observed cleanup (`forget_session_observed` / `release_session_observed`), and by
    /// path collapse workflow cold cleanup — `cleanup_cold_session`
    /// (`bridge-workflow/src/executor.rs:966-987`) calls exactly those two observed methods and
    /// nothing else, which `cold_cleanup_reaches_the_backend_only_through_the_observed_methods`
    /// in that crate pins.
    #[tokio::test]
    async fn observed_cleanup_refuses_to_delete_a_checkout_that_has_a_custody_record() {
        let (be, rec, tmp, source, _cfg) = backend_fixture("gate-observed");
        let sid = SessionId::parse("ctx-gate-observed-g0").unwrap();
        be.configure_session(&sid, &spec(Some(&source.to_string_lossy())))
            .await
            .unwrap();
        publish_custody_record(&be.mapped_worktree_path_for_test(&sid).await.unwrap());
        let observer: Arc<dyn DiagnosticObserver> =
            Arc::new(bridge_core::diagnostics::NoopDiagnosticObserver::default());

        be.release_session_observed(&sid, observer.clone())
            .await
            .unwrap();
        be.forget_session_observed(&sid, observer).await.unwrap();

        assert_eq!(removals(&rec), 0);
        std::fs::remove_dir_all(tmp).unwrap();
    }

    /// FAMILY: retirement (`WorktreeBackend::retire`), and by path collapse controller retire —
    /// `ResilientWarm::retire` (`bridge-controller/src/resilient.rs:69-72`) and its transient
    /// respawn path (`:178`) both call `AgentBackend::retire`, which for a worktree backend is
    /// this method. Driven end-to-end through `ResilientWarm` in `tests/r2f1b_deletion_gate.rs`.
    ///
    /// Also the NO-WEDGE property the gate must not break: a refused session must not stop the
    /// drain, so an unprotected sibling in the same retirement is still removed and the inner
    /// backend still retires.
    #[tokio::test]
    async fn retire_refuses_a_protected_checkout_and_still_drains_an_unprotected_sibling() {
        let (be, rec, tmp, protected_source, _cfg) = backend_fixture("gate-retire");
        let plain_source = tmp.join("allowed").join("source2");
        std::fs::create_dir_all(&plain_source).unwrap();
        let protected = SessionId::parse("ctx-gate-retire-protected-g0").unwrap();
        let plain = SessionId::parse("ctx-gate-retire-plain-g0").unwrap();
        be.configure_session(&protected, &spec(Some(&protected_source.to_string_lossy())))
            .await
            .unwrap();
        be.configure_session(&plain, &spec(Some(&plain_source.to_string_lossy())))
            .await
            .unwrap();
        publish_custody_record(&be.mapped_worktree_path_for_test(&protected).await.unwrap());

        be.retire().await.unwrap();

        assert_eq!(
            removals(&rec),
            1,
            "exactly the unprotected sibling is removed"
        );
        assert_eq!(rec.retire_count.load(Ordering::SeqCst), 1);
        let map = be.map.lock().await;
        assert!(map.contains_key(protected.as_str()));
        assert!(!map.contains_key(plain.as_str()));
        drop(map);
        std::fs::remove_dir_all(tmp).unwrap();
    }

    /// FAMILY: bound-configure rollback. Every failure arm of
    /// `configure_bound_resolved_with_admission` calls
    /// `cleanup_session_with_sealed_admission(Release, true)`; this drives the inner-configure
    /// arm, with the custody record already in place because the rollback happens inside the
    /// call.
    #[tokio::test]
    async fn bound_configure_rollback_refuses_to_delete_a_protected_checkout() {
        let (be, rec, tmp, source, cfg) = backend_fixture("gate-bound-rollback");
        let (bound, target) = bound_spec(&source, &cfg);
        let sid = SessionId::parse("ctx-gate-bound-rollback-g0").unwrap();
        publish_custody_record(&target);
        rec.fail_configure.store(true, Ordering::SeqCst);

        // The bound path configures through `configure_bound_session`, whose fake always
        // succeeds; make the rollback happen at the sidecar-write arm instead by pointing the
        // spec's cwd elsewhere is not possible, so use a provider whose add fails: the
        // reservation entry exists at that point and the rollback flight sees it.
        let failing = Arc::new(WorktreeBackend::new(
            Arc::new(FakeInner { rec: rec.clone() }),
            Arc::new(NonGitProv { rec: rec.clone() }),
            cfg.clone(),
            Some(
                SessionCwd::parse(
                    &std::fs::canonicalize(tmp.join("allowed"))
                        .unwrap()
                        .to_string_lossy(),
                )
                .unwrap(),
            ),
            identity(),
        ));
        let before = removals(&rec);

        assert!(failing.configure_bound_session(&sid, &bound).await.is_err());

        assert_eq!(
            removals(&rec),
            before,
            "the bound rollback must not remove a custody-protected checkout"
        );
        drop(be);
        std::fs::remove_dir_all(tmp).unwrap();
    }

    /// FAMILY: legacy-configure rollback. Same shape as the bound arm, through
    /// `configure_session`'s inner-configure failure path.
    #[tokio::test]
    async fn legacy_configure_rollback_refuses_to_delete_a_protected_checkout() {
        let (be, rec, tmp, source, cfg) = backend_fixture("gate-legacy-rollback");
        let sid = SessionId::parse("ctx-gate-legacy-rollback-g0").unwrap();
        publish_custody_record(&legacy_target(&tmp, &source, &cfg, sid.as_str()));
        rec.fail_configure.store(true, Ordering::SeqCst);

        assert_eq!(
            be.configure_session(&sid, &spec(Some(&source.to_string_lossy())))
                .await,
            Err(BridgeError::StoreFailure)
        );

        assert_eq!(removals(&rec), 0);
        std::fs::remove_dir_all(tmp).unwrap();
    }

    /// FAMILY: `ConfigureAdmission::Drop`. Aborting a configure that is blocked inside the inner
    /// backend drops the admission with `cleanup_on_drop` armed, which starts an observer-free
    /// Release flight directly from the `Drop` impl — the most context-free entry in the whole
    /// fan-in, and the reason the gate takes only a `WtEntry`.
    #[tokio::test]
    async fn configure_admission_drop_cleanup_refuses_to_delete_a_protected_checkout() {
        let (be, rec, tmp, source, cfg) = backend_fixture("gate-admission-drop");
        let sid = SessionId::parse("ctx-gate-admission-drop-g0").unwrap();
        publish_custody_record(&legacy_target(&tmp, &source, &cfg, sid.as_str()));

        let _allow = rec.block_next_configure();
        let configure = tokio::spawn({
            let be = be.clone();
            let sid = sid.clone();
            let spec = spec(Some(&source.to_string_lossy()));
            async move { be.configure_session(&sid, &spec).await }
        });
        rec.wait_for_blocked_configure().await;
        configure.abort();
        assert!(configure.await.unwrap_err().is_cancelled());

        // Drain the detached flight the Drop impl started.
        be.release_session_checked(&sid).await.unwrap();

        assert_eq!(removals(&rec), 0);
        std::fs::remove_dir_all(tmp).unwrap();
    }

    /// DISAGREEMENT DIRECTION 1: the in-memory discriminator says protected while the disk says
    /// nothing. Discriminates a gate implemented as "consult the record only", which would delete
    /// a checkout whose record write is still in flight, or whose record an actor removed.
    #[tokio::test]
    async fn the_discriminator_alone_refuses_deletion_with_no_record_on_disk() {
        let (be, rec, tmp, source, _cfg) = backend_fixture("gate-discriminator");
        let sid = SessionId::parse("ctx-gate-discriminator-g0").unwrap();
        be.configure_session(&sid, &spec(Some(&source.to_string_lossy())))
            .await
            .unwrap();
        let target = be.mapped_worktree_path_for_test(&sid).await.unwrap();
        assert!(!Path::new(&crate::custody::custody_record_path(&target)).exists());
        be.mark_checkout_protected_for_test(&sid).await;

        be.release_session_checked(&sid).await.unwrap();

        assert_eq!(removals(&rec), 0);
        std::fs::remove_dir_all(tmp).unwrap();
    }

    /// DISAGREEMENT DIRECTION 2 is what every `publish_custody_record` test above exercises (a
    /// legacy discriminator with a record present). This pins the *undecodable* case of it:
    /// protection must not be removable by damaging the record, so the gate keys on presence and
    /// never on a successful decode.
    #[tokio::test]
    async fn a_corrupt_custody_record_still_refuses_deletion() {
        let (be, rec, tmp, source, _cfg) = backend_fixture("gate-corrupt");
        let sid = SessionId::parse("ctx-gate-corrupt-g0").unwrap();
        be.configure_session(&sid, &spec(Some(&source.to_string_lossy())))
            .await
            .unwrap();
        let target = be.mapped_worktree_path_for_test(&sid).await.unwrap();
        std::fs::write(crate::custody::custody_record_path(&target), b"{ truncated").unwrap();

        be.release_session_checked(&sid).await.unwrap();

        assert_eq!(removals(&rec), 0);
        std::fs::remove_dir_all(tmp).unwrap();
    }

    /// The INCONCLUSIVE arm: the enclosing directory exists but cannot be pinned, so durable
    /// truth is unreadable. Discriminates a gate that treats an unanswerable probe as "no record"
    /// — the failure mode that turns a transient filesystem condition into an irreversible
    /// deletion.
    #[tokio::test]
    async fn an_unreadable_custody_probe_refuses_deletion() {
        let (be, rec, tmp, source, cfg) = backend_fixture("gate-inconclusive");
        let sid = SessionId::parse("ctx-gate-inconclusive-g0").unwrap();
        be.configure_session(&sid, &spec(Some(&source.to_string_lossy())))
            .await
            .unwrap();
        // Replace the worktree ROOT with a regular file: it exists (so nothing is provably
        // absent) but no directory descriptor can be pinned on it.
        std::fs::remove_dir_all(&cfg.root).unwrap();
        std::fs::write(&cfg.root, b"not a directory").unwrap();

        be.release_session_checked(&sid).await.unwrap();

        assert_eq!(removals(&rec), 0);
        std::fs::remove_file(&cfg.root).unwrap();
        std::fs::remove_dir_all(tmp).unwrap();
    }

    /// IDEMPOTENCE + no wedge: a refusal must be repeatable and must leave the session cleanable
    /// again later (e.g. once R2f2 disposes of the claim). Discriminates a gate that poisons the
    /// cleanup cell, and one that lets the map entry go so a second attempt silently no-ops.
    #[tokio::test]
    async fn a_refused_checkout_re_refuses_and_becomes_deletable_once_its_record_is_gone() {
        let (be, rec, tmp, source, _cfg) = backend_fixture("gate-idempotent");
        let sid = SessionId::parse("ctx-gate-idempotent-g0").unwrap();
        be.configure_session(&sid, &spec(Some(&source.to_string_lossy())))
            .await
            .unwrap();
        let target = be.mapped_worktree_path_for_test(&sid).await.unwrap();
        publish_custody_record(&target);

        be.release_session_checked(&sid).await.unwrap();
        be.release_session_checked(&sid).await.unwrap();
        assert_eq!(removals(&rec), 0);
        assert!(be.map.lock().await.contains_key(sid.as_str()));

        std::fs::remove_file(crate::custody::custody_record_path(&target)).unwrap();
        be.release_session_checked(&sid).await.unwrap();

        assert_eq!(removals(&rec), 1, "the gate refuses, it does not disable");
        assert!(be.map.lock().await.is_empty());
        std::fs::remove_dir_all(tmp).unwrap();
    }

    // ---- slice 2c1: fail-closed preservation at the backend ----------------------------------

    /// A `DiagnosticObserver` that keeps the static codes it was handed, so the TYPED cleanup
    /// disposition can be checked at the layer that actually projects it.
    #[derive(Default)]
    struct CodeRec {
        codes: Mutex<Vec<(String, String)>>,
    }

    #[async_trait::async_trait]
    impl DiagnosticObserver for CodeRec {
        async fn record(
            &self,
            event: bridge_core::diagnostics::DiagnosticEvent,
        ) -> Result<(), BridgeError> {
            let transition = event.transition();
            self.codes.lock().unwrap().push((
                format!("{:?}", transition.status()),
                transition
                    .code()
                    .map(|code| code.as_str().to_owned())
                    .unwrap_or_default(),
            ));
            Ok(())
        }
    }

    fn signals(rec: &Rec) -> Vec<(String, Option<String>)> {
        rec.record_state_at_signal.lock().unwrap().clone()
    }

    /// Configure one V3 checkout and arm the ordering witness on it.
    async fn v3_session(
        name: &str,
    ) -> (
        Arc<WorktreeBackend>,
        Arc<Rec>,
        PathBuf,
        SessionId,
        String,
        BoundSessionSpecV1,
    ) {
        let (be, rec, tmp, source, cfg) = backend_fixture(name);
        let (bound, target) = bound_spec_v3(&source, &cfg);
        let session = SessionId::parse(format!("ctx-{name}-g0")).unwrap();
        be.configure_bound_session(&session, &bound).await.unwrap();
        assert_eq!(record_state_of(&target).as_deref(), Some("live_protected"));
        *rec.watch_checkout.lock().unwrap() = Some(target.clone());
        (be, rec, tmp, session, target, bound)
    }

    #[tokio::test]
    async fn inner_protective_cleanup_dispositions_survive_worktree_composition() {
        let rec = Arc::new(Rec::default());
        let backend = flight_only_backend(Arc::new(FakeInner { rec: rec.clone() }), rec.clone());

        for (index, disposition) in [
            BackendCleanupDispositionV1::Retained,
            BackendCleanupDispositionV1::Preserved,
            BackendCleanupDispositionV1::Unknown,
        ]
        .into_iter()
        .enumerate()
        {
            *rec.cleanup_disposition.lock().unwrap() = disposition;
            let session = SessionId::parse(format!("inner-disposition-{index}")).unwrap();
            assert_eq!(
                backend.release_session_checked(&session).await,
                Ok(disposition)
            );
        }

        assert_eq!(
            rec.order
                .lock()
                .unwrap()
                .iter()
                .filter(|step| step.as_str() == "inner_release")
                .count(),
            3,
            "one inner cleanup signal per composed flight"
        );
    }

    /// Task G carry-forward guard for ContainerRw/worktree composition. The exact two-field
    /// report remains private but load-bearing: exhaustive destructuring pins its fields,
    /// exhaustive matches pin both closed disposition sets, and this cross-product pins the fold.
    /// Only exact Complete + Complete may project Complete.
    #[test]
    fn task_g_cleanup_report_two_field_contract_and_fold_table_are_frozen() {
        fn protective_fold(
            inner: BackendCleanupDispositionV1,
            checkout: BackendCleanupDispositionV1,
        ) -> BackendCleanupDispositionV1 {
            use BackendCleanupDispositionV1::{Complete, Preserved, Retained, Unknown};
            match (inner, checkout) {
                (Unknown, _) | (_, Unknown) => Unknown,
                (Preserved, _) | (_, Preserved) => Preserved,
                (Retained, _) | (_, Retained) => Retained,
                (Complete, Complete) => Complete,
            }
        }
        fn checkout_variant(disposition: &CheckoutCleanupDispositionV1) -> &'static str {
            match disposition {
                CheckoutCleanupDispositionV1::NotNeeded => "not_needed",
                CheckoutCleanupDispositionV1::Removed => "removed",
                CheckoutCleanupDispositionV1::RemovedRecordAmbiguous(_) => {
                    "removed_record_ambiguous"
                }
                CheckoutCleanupDispositionV1::RemovalFailed => "removal_failed",
                CheckoutCleanupDispositionV1::Retained => "retained",
                CheckoutCleanupDispositionV1::Preserved => "preserved",
            }
        }

        let inner_dispositions = [
            BackendCleanupDispositionV1::Complete,
            BackendCleanupDispositionV1::Retained,
            BackendCleanupDispositionV1::Preserved,
            BackendCleanupDispositionV1::Unknown,
        ];
        let checkout_dispositions = [
            CheckoutCleanupDispositionV1::NotNeeded,
            CheckoutCleanupDispositionV1::Removed,
            CheckoutCleanupDispositionV1::RemovedRecordAmbiguous("ambiguous".into()),
            CheckoutCleanupDispositionV1::RemovalFailed,
            CheckoutCleanupDispositionV1::Retained,
            CheckoutCleanupDispositionV1::Preserved,
        ];

        for inner in inner_dispositions {
            for checkout in checkout_dispositions.iter().cloned() {
                let checkout_backend = checkout.backend_disposition();
                assert!(!checkout_variant(&checkout).is_empty());
                let report = CleanupReportV1::settled(checkout, inner, None);
                let CleanupReportV1 { result, checkout } = report;
                let expected = protective_fold(inner, checkout_backend);

                assert_eq!(result, Ok(expected));
                assert_eq!(checkout.backend_disposition(), checkout_backend);
                assert_eq!(
                    result == Ok(BackendCleanupDispositionV1::Complete),
                    inner == BackendCleanupDispositionV1::Complete
                        && checkout_backend == BackendCleanupDispositionV1::Complete,
                    "only exact Complete + Complete may fold to Complete"
                );
            }
        }
    }

    async fn assert_outer_preservation_composes_once(
        name: &str,
        inner: BackendCleanupDispositionV1,
        expected: BackendCleanupDispositionV1,
    ) {
        let (backend, rec, tmp, session, _target, _bound) = v3_session(name).await;
        *rec.cleanup_disposition.lock().unwrap() = inner;
        assert!(matches!(
            backend
                .preserve_checkout_v1(&session, CheckoutPreservationReasonV1::Cancellation,)
                .await,
            CheckoutPreservationV1::Preserved
        ));

        assert_eq!(
            backend.release_session_checked(&session).await,
            Ok(expected)
        );
        assert_eq!(
            rec.order
                .lock()
                .unwrap()
                .iter()
                .filter(|step| step.as_str() == "inner_release")
                .count(),
            1,
            "inner and outer protective outcomes must share one cleanup flight"
        );
        std::fs::remove_dir_all(tmp).unwrap();
    }

    #[tokio::test]
    async fn outer_preserved_and_inner_retained_do_not_collapse_or_double_signal() {
        assert_outer_preservation_composes_once(
            "compose-preserved-retained",
            BackendCleanupDispositionV1::Retained,
            BackendCleanupDispositionV1::Preserved,
        )
        .await;
    }

    #[tokio::test]
    async fn outer_retained_and_inner_preserved_do_not_collapse_or_double_signal() {
        let (backend, rec, tmp, session, target, _bound) =
            v3_session("compose-retained-preserved").await;
        *rec.cleanup_disposition.lock().unwrap() = BackendCleanupDispositionV1::Preserved;

        assert_eq!(
            backend.release_session_checked(&session).await,
            Ok(BackendCleanupDispositionV1::Preserved)
        );
        assert_eq!(record_state_of(&target).as_deref(), Some("live_protected"));
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
    async fn outer_preserved_and_inner_unknown_remain_unknown_without_double_signal() {
        assert_outer_preservation_composes_once(
            "compose-preserved-unknown",
            BackendCleanupDispositionV1::Unknown,
            BackendCleanupDispositionV1::Unknown,
        )
        .await;
    }

    /// §3 2c1 step 1, and the claim this whole slice rests on:
    /// **no failure, cancel, or ambiguity path reaches provider removal, reset, clean, or prune.**
    ///
    /// All three triggers are driven over the same live V3 checkout — a node failure, a
    /// cancellation, and (via an injected parent-sync fault) an AMBIGUOUS preservation — each
    /// followed by the teardown the executor really issues. The provider double records every
    /// `remove`; `WorktreeProvider` has no reset/clean/prune operation at all, which is itself the
    /// structural half of the claim and is asserted by the absence of any other mutating method.
    ///
    /// Discriminates a barrier that falls through to the removal block on an unknown outcome —
    /// the exact failure mode §5.2's "unknown is never deleted" rule exists to prevent.
    #[tokio::test]
    async fn failure_cancel_and_ambiguity_never_call_provider_remove_reset_clean_prune() {
        let (be, rec, tmp, session, target, _bound) = v3_session("v3-never-removes").await;

        for reason in [
            CheckoutPreservationReasonV1::NodeFailure,
            CheckoutPreservationReasonV1::Cancellation,
        ] {
            let outcome = be.preserve_checkout_v1(&session, reason).await;
            assert!(outcome.is_protective(), "{outcome:?}");
        }
        // The ambiguity arm: a preservation whose outcome cannot be determined must not license
        // anything either. The record is already terminal here, so the barrier answers from the
        // terminal arm; the ambiguous-publication arm itself is pinned in `custody_writer`.
        be.cancel(&session).await.unwrap();
        be.forget_session_checked(&session).await.unwrap();
        be.release_session_checked(&session).await.unwrap();
        be.retire().await.unwrap();

        assert_eq!(
            removals(&rec),
            0,
            "no failure, cancel or ambiguity path may reach provider removal"
        );
        assert!(
            !rec.order.lock().unwrap().iter().any(|op| op == "wt_remove"),
            "and none of them may reach the removal block at all"
        );
        assert_eq!(
            record_state_of(&target).as_deref(),
            Some("preserved"),
            "the checkout is preserved, not removed"
        );
        assert!(
            Path::new(&target).exists(),
            "and the work itself is still on disk"
        );
        std::fs::remove_dir_all(&tmp).unwrap();
    }

    /// §5.1 step 6 — "Only then may session cancel or a resource signal occur", witnessed from the
    /// FAR SIDE of the barrier: the state the record actually had at the instant each death signal
    /// landed in the inner backend.
    ///
    /// The three signals cover the backend's whole death-signal surface — `cancel` (the executor's
    /// two cold sites and its preflight call this before cleanup), `forget_session_checked` and
    /// `release_session_checked` (every remaining R-11 entry: both `Drop`s, the reaper, the eleven
    /// direct release sites, retire, cold cleanup).
    ///
    /// Discriminates a barrier placed INSIDE the removal block (which R-8 rejects as too late):
    /// the inner teardown runs first there, so every observation would be `live_protected`.
    #[tokio::test]
    async fn preservation_precedes_the_session_death_signal_at_every_backend_entry() {
        let (be, rec, tmp, session, _target, _bound) = v3_session("v3-barrier-order").await;

        be.preserve_checkout_v1(&session, CheckoutPreservationReasonV1::Cancellation)
            .await;
        be.cancel(&session).await.unwrap();
        be.forget_session_checked(&session).await.unwrap();
        be.release_session_checked(&session).await.unwrap();

        let observed = signals(&rec);
        assert_eq!(
            observed,
            vec![
                ("inner_cancel".to_string(), Some("preserved".to_string())),
                ("inner_forget".to_string(), Some("preserved".to_string())),
                ("inner_release".to_string(), Some("preserved".to_string())),
            ],
            "every death signal must find the claim already durable"
        );
        std::fs::remove_dir_all(&tmp).unwrap();
    }

    /// The FLIGHT-SIDE half of the barrier, and the one the caller-side witness above cannot
    /// discriminate: a session whose checkout disposition is preservation but whose caller did
    /// NOT preserve first.
    ///
    /// This is a real state, not a contrivance — `preserve_checkout_v1` raises the disposition and
    /// leaves it raised, so every later flight for that session must re-run the barrier, which is
    /// what makes a first attempt that came back `Ambiguous` or `Refused` retriable at all. It is
    /// also the shape every R-11 entry that never calls the barrier method (a reaper, a `Drop`,
    /// controller retire) sees once a preservation has been requested for the session.
    ///
    /// Discriminates the barrier's PLACEMENT inside the flight: move it below the inner teardown
    /// and the release lands while the record still says `live_protected`. The caller-side witness
    /// stays green under that mutation — there the record was already terminal before the flight
    /// began — which is exactly why this test exists.
    #[tokio::test]
    async fn the_flight_side_barrier_preserves_before_the_inner_teardown() {
        let (be, rec, tmp, session, target, _bound) = v3_session("v3-flight-barrier").await;
        be.raise_checkout_disposition(
            &session,
            CheckoutDispositionV1::Preserve,
            Some(PreservationReasonV1::NodeFailure),
        )
        .await
        .expect("the configured session has a cell");
        assert_eq!(
            record_state_of(&target).as_deref(),
            Some("live_protected"),
            "nothing has preserved it yet"
        );

        be.release_session_checked(&session).await.unwrap();

        assert_eq!(
            signals(&rec),
            vec![("inner_release".to_string(), Some("preserved".to_string()))],
            "the flight must preserve before it signals the inner backend"
        );
        assert_eq!(removals(&rec), 0);
        std::fs::remove_dir_all(&tmp).unwrap();
    }

    /// The control that makes the witness above mean something, and §5.1's own rule in force:
    /// **a node-local success is not a checkout disposition.** Without a preservation request the
    /// same three signals find the checkout still `LiveProtected` — the barrier is ordered, not
    /// unconditional — and the checkout is still not removed, because the gate refuses.
    ///
    /// Discriminates a barrier that terminalizes every teardown: that would let a reaper or a
    /// `Drop` — neither of which has a workflow outcome to consult — decide a disposition only the
    /// post-loop mint (2c2) is entitled to decide.
    #[tokio::test]
    async fn an_ordinary_teardown_leaves_a_live_checkout_live_and_still_undeletable() {
        let (be, rec, tmp, session, target, _bound) = v3_session("v3-no-request").await;

        be.cancel(&session).await.unwrap();
        be.release_session_checked(&session).await.unwrap();

        assert_eq!(
            signals(&rec),
            vec![
                (
                    "inner_cancel".to_string(),
                    Some("live_protected".to_string())
                ),
                (
                    "inner_release".to_string(),
                    Some("live_protected".to_string())
                ),
            ],
            "no request, no preservation"
        );
        assert_eq!(removals(&rec), 0, "and still no removal");
        assert_eq!(record_state_of(&target).as_deref(), Some("live_protected"));
        std::fs::remove_dir_all(&tmp).unwrap();
    }

    /// V2 POSITIVE CONTROL for the barrier: a legacy checkout has nothing under custody, so the
    /// barrier is a no-op answer and ordinary cleanup still removes it. Discriminates a barrier
    /// that probes or transitions on the legacy path, which would change V2 behaviour.
    #[tokio::test]
    async fn the_preservation_barrier_is_a_no_op_for_a_legacy_checkout() {
        let (be, rec, tmp, source, _cfg) = backend_fixture("v2-barrier-noop");
        let session = SessionId::parse("ctx-v2-barrier-noop-g0").unwrap();
        be.configure_session(&session, &spec(Some(&source.to_string_lossy())))
            .await
            .unwrap();

        let outcome = be
            .preserve_checkout_v1(&session, CheckoutPreservationReasonV1::NodeFailure)
            .await;

        assert_eq!(outcome, CheckoutPreservationV1::NoCheckoutUnderCustody);
        be.release_session_checked(&session).await.unwrap();
        assert_eq!(removals(&rec), 1, "V2 teardown is byte-identical");
        assert!(be.map.lock().await.is_empty());
        std::fs::remove_dir_all(&tmp).unwrap();
    }

    /// An unmapped session has no checkout, and the barrier says so without creating one.
    #[tokio::test]
    async fn the_preservation_barrier_answers_no_checkout_for_an_unmapped_session() {
        let (be, _rec, tmp, _source, _cfg) = backend_fixture("v3-unmapped");
        let session = SessionId::parse("ctx-v3-unmapped-g0").unwrap();

        let outcome = be
            .preserve_checkout_v1(&session, CheckoutPreservationReasonV1::Cancellation)
            .await;

        assert_eq!(outcome, CheckoutPreservationV1::NoCheckoutUnderCustody);
        assert_eq!(be.cleanup_cell_count(), 0, "and no cell is conjured for it");
        std::fs::remove_dir_all(&tmp).unwrap();
    }

    /// Repair RD (sol B-4 / opus S2) — the materialization's custody evidence must survive a
    /// FAILED INNER CONFIGURE, not only a successful one.
    ///
    /// Before the repair, `custody` and `protection` were written onto the map only at the `Ready`
    /// publication, which the inner-configure failure arm never reaches. The rollback flight then
    /// saw a `Legacy`/`None` reservation entry beside a durable `LiveProtected` record: the gate's
    /// disk arm still refused the removal, so nothing was lost from disk, but the four
    /// descriptor-observed identities were gone and no exact claim could ever be minted for that
    /// checkout again. `LiveProtected` would have been its permanent state.
    ///
    /// Discriminates the shipped ordering precisely: move the upgrade back to the `Ready` site and
    /// the barrier below answers `NoCheckoutUnderCustody`.
    #[tokio::test]
    async fn custody_evidence_survives_an_inner_configure_failure_after_materialization() {
        let (be, rec, tmp, source, cfg) = backend_fixture("v3-configure-failure");
        let (bound, target) = bound_spec_v3(&source, &cfg);
        let session = SessionId::parse("ctx-v3-configure-failure-g0").unwrap();
        rec.fail_configure.store(true, Ordering::SeqCst);
        let failing = Arc::new(WorktreeBackend::new(
            Arc::new(ConfigureFailInner { rec: rec.clone() }),
            Arc::new(FakeProv { rec: rec.clone() }),
            cfg.clone(),
            Some(
                SessionCwd::parse(
                    &std::fs::canonicalize(tmp.join("allowed"))
                        .unwrap()
                        .to_string_lossy(),
                )
                .unwrap(),
            ),
            identity(),
        ));

        assert!(failing
            .configure_bound_session(&session, &bound)
            .await
            .is_err());

        assert_eq!(removals(&rec), 0, "the record's disk arm still refuses");
        assert_eq!(record_state_of(&target).as_deref(), Some("live_protected"));
        // The evidence survived the rollback: the entry is custody-positive AND carries the
        // identities, so an exact claim is still mintable.
        let outcome = failing
            .preserve_checkout_v1(&session, CheckoutPreservationReasonV1::NodeFailure)
            .await;
        assert_eq!(
            outcome,
            CheckoutPreservationV1::Preserved,
            "a rolled-back materialization must still be able to mint its exact claim: {outcome:?}"
        );
        assert_eq!(record_state_of(&target).as_deref(), Some("preserved"));
        drop(be);
        std::fs::remove_dir_all(&tmp).unwrap();
    }

    /// Repair RD, the CANCELLATION arm — the same obligation when the configure future is dropped
    /// mid-inner-configure rather than failing.
    ///
    /// This is the arm that motivates "immediately after materialization, before the next await":
    /// a cancellation lands at an await point, and the only await between materialization and the
    /// `Ready` publication is the inner configure. The `ConfigureAdmission::Drop` rollback then
    /// runs against whatever the map holds.
    #[tokio::test]
    async fn custody_evidence_survives_a_cancelled_configure_after_materialization() {
        let (be, rec, tmp, source, cfg) = backend_fixture("v3-configure-cancel");
        let (bound, target) = bound_spec_v3(&source, &cfg);
        let session = SessionId::parse("ctx-v3-configure-cancel-g0").unwrap();
        let gated = Arc::new(WorktreeBackend::new(
            Arc::new(ConfigureFailInner { rec: rec.clone() }),
            Arc::new(FakeProv { rec: rec.clone() }),
            cfg.clone(),
            Some(
                SessionCwd::parse(
                    &std::fs::canonicalize(tmp.join("allowed"))
                        .unwrap()
                        .to_string_lossy(),
                )
                .unwrap(),
            ),
            identity(),
        ));

        let _allow = rec.block_next_configure();
        let configure = tokio::spawn({
            let gated = gated.clone();
            let session = session.clone();
            let bound = bound.clone();
            async move { gated.configure_bound_session(&session, &bound).await }
        });
        rec.wait_for_blocked_configure().await;
        configure.abort();
        assert!(configure.await.unwrap_err().is_cancelled());

        assert_eq!(record_state_of(&target).as_deref(), Some("live_protected"));
        let outcome = gated
            .preserve_checkout_v1(&session, CheckoutPreservationReasonV1::Cancellation)
            .await;
        assert_eq!(
            outcome,
            CheckoutPreservationV1::Preserved,
            "a cancelled configure must not strand its checkout without an exact claim: {outcome:?}"
        );
        assert_eq!(record_state_of(&target).as_deref(), Some("preserved"));
        assert_eq!(removals(&rec), 0);
        drop(be);
        std::fs::remove_dir_all(&tmp).unwrap();
    }

    /// P3 / R-8, RACE ORDER A — a preservation request must never be served by an in-flight
    /// RECLAIM flight of equal strength.
    ///
    /// The reclaim flight is held open inside the provider's `remove`; the preservation request
    /// then arrives at the same cell with the same `Release` strength, which is exactly the shape
    /// the pre-2c1 join key (`session cell`, `strength`) would have joined. The assertion is that
    /// a SECOND flight is started instead: `joined_waiters` stays 0 on the new slot.
    ///
    /// Discriminates the shipped 2b1/2b2 join rule verbatim — remove the disposition half of the
    /// key and the preserve request is handed the reclaim flight's report, i.e. told that a
    /// checkout was preserved by a flight whose whole job was to delete it.
    #[tokio::test]
    async fn a_preserve_request_never_joins_an_equal_strength_reclaim_flight() {
        let (be, rec, tmp, source, _cfg) = backend_fixture("v3-race-a");
        let session = SessionId::parse("ctx-v3-race-a-g0").unwrap();
        be.configure_session(&session, &spec(Some(&source.to_string_lossy())))
            .await
            .unwrap();

        let reclaim = be
            .start_or_join_cleanup(&session, CleanupStrength::Release, false)
            .expect("a reclaim flight starts");
        let (reclaim_strength, _reclaim_report) = reclaim;
        assert_eq!(reclaim_strength, CleanupStrength::Release);

        // The preservation request raises the cell's disposition, which mints a new epoch.
        be.raise_checkout_disposition(
            &session,
            CheckoutDispositionV1::Preserve,
            Some(PreservationReasonV1::Cancellation),
        )
        .await
        .expect("the session has a cell");
        let preserve = be
            .start_or_join_cleanup(&session, CleanupStrength::Release, false)
            .expect("a preserve flight starts");

        assert_eq!(
            be.cleanup_join_count(&session),
            0,
            "an equal-strength request of a DIFFERENT disposition must not join"
        );
        assert_eq!(preserve.0, CleanupStrength::Release);
        let report = wait_for_cleanup_report(preserve.1).await;
        assert!(report.is_ok());
        drop(rec);
        std::fs::remove_dir_all(&tmp).unwrap();
    }

    /// P3 / R-8, RACE ORDER B — the reverse: once a checkout's disposition is preservation, a
    /// later equal-strength RECLAIM request cannot downgrade it, and joins the preserve flight
    /// rather than starting a removal of its own.
    ///
    /// This is §5.1's monotonicity ("no later healthy projection or TTL can mint deletion
    /// authority") expressed in the join key. Discriminates a key that merely compares the two
    /// dispositions for inequality without ordering them: that would start a fresh RECLAIM flight
    /// for the second request, and the only thing standing between it and the checkout would be
    /// the gate.
    #[tokio::test]
    async fn a_later_reclaim_cannot_downgrade_a_preserved_checkouts_disposition() {
        let (be, rec, tmp, session, target, _bound) = v3_session("v3-race-b").await;

        be.preserve_checkout_v1(&session, CheckoutPreservationReasonV1::NodeFailure)
            .await;
        let preserve = be
            .start_or_join_cleanup(&session, CleanupStrength::Release, false)
            .expect("a preserve flight starts");
        let joined = be
            .start_or_join_cleanup(&session, CleanupStrength::Release, false)
            .expect("the equal-strength reclaim request resolves");

        assert_eq!(
            be.cleanup_join_count(&session),
            1,
            "the later request joins the PRESERVE flight rather than minting a reclaim"
        );
        let first = wait_for_cleanup_report(preserve.1).await;
        let second = wait_for_cleanup_report(joined.1).await;
        assert_eq!(first.checkout, CheckoutCleanupDispositionV1::Preserved);
        assert_eq!(second.checkout, CheckoutCleanupDispositionV1::Preserved);
        assert_eq!(removals(&rec), 0);
        assert_eq!(record_state_of(&target).as_deref(), Some("preserved"));
        std::fs::remove_dir_all(&tmp).unwrap();
    }

    /// P4 (2b1 sol-1 / D-1, BINDING) — a retained checkout is projected as RETAINED, not as a
    /// clean release.
    ///
    /// Before this slice the refusal published `worktree.teardown.released`, byte-identical to a
    /// real removal, so the whole R-11 fan-in read "the checkout is gone". All three outcomes are
    /// checked against each other here, because a code that is distinct but wrong is no better
    /// than one that is shared.
    #[tokio::test]
    async fn a_retained_checkout_publishes_its_own_teardown_code_not_a_released_one() {
        // (1) legacy checkout, ordinary removal.
        let (be, rec, tmp, source, _cfg) = backend_fixture("proj-v2");
        let session = SessionId::parse("ctx-proj-v2-g0").unwrap();
        be.configure_session(&session, &spec(Some(&source.to_string_lossy())))
            .await
            .unwrap();
        let codes = Arc::new(CodeRec::default());
        be.release_session_observed(&session, codes.clone())
            .await
            .unwrap();
        assert_eq!(removals(&rec), 1);
        assert_eq!(
            codes.codes.lock().unwrap().last().cloned(),
            Some(("Completed".into(), "worktree.teardown.released".into())),
            "a real removal keeps the exact code it always published"
        );
        std::fs::remove_dir_all(&tmp).unwrap();

        // (2) custody-protected checkout, gate refusal, no preservation requested.
        let (be, rec, tmp, source, _cfg) = backend_fixture("proj-retained");
        let session = SessionId::parse("ctx-proj-retained-g0").unwrap();
        be.configure_session(&session, &spec(Some(&source.to_string_lossy())))
            .await
            .unwrap();
        publish_custody_record(&be.mapped_worktree_path_for_test(&session).await.unwrap());
        let codes = Arc::new(CodeRec::default());
        be.release_session_observed(&session, codes.clone())
            .await
            .unwrap();
        assert_eq!(removals(&rec), 0);
        assert_eq!(
            codes.codes.lock().unwrap().last().cloned(),
            Some(("Completed".into(), "worktree.teardown.retained".into())),
            "a deliberately retained checkout must never project as released"
        );
        std::fs::remove_dir_all(&tmp).unwrap();

        // (3) custody-protected checkout with a durable preservation claim.
        let (be, rec, tmp, session, _target, _bound) = v3_session("proj-preserved").await;
        be.preserve_checkout_v1(&session, CheckoutPreservationReasonV1::NodeFailure)
            .await;
        let codes = Arc::new(CodeRec::default());
        be.release_session_observed(&session, codes.clone())
            .await
            .unwrap();
        assert_eq!(removals(&rec), 0);
        assert_eq!(
            codes.codes.lock().unwrap().last().cloned(),
            Some(("Completed".into(), "worktree.teardown.preserved".into())),
            "and a preserved one is distinguishable from a merely retained one"
        );
        std::fs::remove_dir_all(&tmp).unwrap();
    }

    /// P5 (2b1 sol-2 / D-2, BINDING) — a refused rollback of a `Reserving` entry keeps an owner,
    /// and once protection lifts that owner reaches EXACTLY ONE provider removal.
    ///
    /// This is the bound-configure rollback arm of the two 2b1 tests, extended per the ledger.
    /// Before this slice `entry_for_cleanup` popped the reservation, the reporter evicted the cell
    /// on the refusal's `Ok`, and the checkout was left on disk with no in-memory owner at all: a
    /// later cleanup found nothing and reported success forever.
    ///
    /// Discriminates the shipped 2b1 behaviour exactly — remove the `Retained` re-insertion and
    /// the final release removes nothing.
    #[tokio::test]
    async fn a_refused_bound_rollback_retains_its_owner_and_removes_exactly_once_later() {
        let (be, rec, tmp, source, cfg) = backend_fixture("retain-bound-rollback");
        let (bound, target) = bound_spec(&source, &cfg);
        let session = SessionId::parse("ctx-retain-bound-rollback-g0").unwrap();
        publish_custody_record(&target);
        // The bound fake's `configure_bound_session` always succeeds, so the rollback is driven
        // from the SIDECAR-WRITE arm — a `Reserving`-entry rollback whose provider removal
        // succeeds, which is what "exactly once" needs to be measurable at all.
        let failing = Arc::new(WorktreeBackend::new(
            Arc::new(FakeInner { rec: rec.clone() }),
            Arc::new(SidecarWriteFailProv { rec: rec.clone() }),
            cfg.clone(),
            Some(
                SessionCwd::parse(
                    &std::fs::canonicalize(tmp.join("allowed"))
                        .unwrap()
                        .to_string_lossy(),
                )
                .unwrap(),
            ),
            identity(),
        ));

        assert!(failing
            .configure_bound_session(&session, &bound)
            .await
            .is_err());

        assert_eq!(removals(&rec), 0, "the rollback must not delete it");
        assert!(
            matches!(
                failing.map.lock().await.get(session.as_str()),
                Some(WtState::Retained { .. })
            ),
            "a refused rollback must retain an owner, not drop the entry"
        );

        // R2f2 disposes of the claim; the retained owner must still reach the removal.
        std::fs::remove_file(crate::custody::custody_record_path(&target)).unwrap();
        failing.release_session_checked(&session).await.unwrap();
        failing.release_session_checked(&session).await.unwrap();

        assert_eq!(
            removals(&rec),
            1,
            "exactly one provider removal, once protection lifts"
        );
        assert!(failing.map.lock().await.is_empty());
        drop(be);
        std::fs::remove_dir_all(&tmp).unwrap();
    }

    /// P5, the legacy-configure rollback arm — same obligation through `configure_session`'s
    /// inner-configure failure path, which is a different rollback family.
    #[tokio::test]
    async fn a_refused_legacy_rollback_retains_its_owner_and_removes_exactly_once_later() {
        let (be, rec, tmp, source, cfg) = backend_fixture("retain-legacy-rollback");
        let session = SessionId::parse("ctx-retain-legacy-rollback-g0").unwrap();
        let target = legacy_target(&tmp, &source, &cfg, session.as_str());
        publish_custody_record(&target);
        rec.fail_configure.store(true, Ordering::SeqCst);

        assert_eq!(
            be.configure_session(&session, &spec(Some(&source.to_string_lossy())))
                .await,
            Err(BridgeError::StoreFailure)
        );

        assert_eq!(removals(&rec), 0);
        assert!(
            matches!(
                be.map.lock().await.get(session.as_str()),
                Some(WtState::Retained { .. })
            ),
            "the legacy rollback family must retain an owner too"
        );

        std::fs::remove_file(crate::custody::custody_record_path(&target)).unwrap();
        be.release_session_checked(&session).await.unwrap();
        be.release_session_checked(&session).await.unwrap();

        assert_eq!(removals(&rec), 1);
        assert!(be.map.lock().await.is_empty());
        std::fs::remove_dir_all(&tmp).unwrap();
    }

    /// The SCOPE of the retention, stated as a test because it is a deliberate narrowing: an
    /// INCONCLUSIVE probe is not custody evidence, so it must not convert 2b1's accepted
    /// self-healing V2 leak into a permanently non-reusable session.
    ///
    /// Discriminates a retention keyed on "the gate refused" rather than on "the gate refused ON
    /// CUSTODY EVIDENCE": that would wedge a session id for the lifetime of the process after one
    /// transient `EACCES` on the worktree root.
    #[tokio::test]
    async fn an_inconclusive_probe_refusal_does_not_retain_the_entry() {
        let (be, rec, tmp, source, cfg) = backend_fixture("retain-inconclusive");
        let session = SessionId::parse("ctx-retain-inconclusive-g0").unwrap();
        be.configure_session(&session, &spec(Some(&source.to_string_lossy())))
            .await
            .unwrap();
        std::fs::remove_dir_all(&cfg.root).unwrap();
        std::fs::write(&cfg.root, b"not a directory").unwrap();

        be.release_session_checked(&session).await.unwrap();

        assert_eq!(removals(&rec), 0, "an unknown probe still refuses removal");
        assert!(
            matches!(
                be.map.lock().await.get(session.as_str()),
                Some(WtState::Ready(_))
            ),
            "a Ready entry stays Ready and reusable: this is not custody evidence"
        );
        std::fs::remove_file(&cfg.root).unwrap();
        std::fs::remove_dir_all(&tmp).unwrap();
    }

    /// P6 (2b1 opus S-3, BINDING) — a checkout awaiting R2f2 disposition is NEVER handed to a new
    /// session as its cwd.
    ///
    /// The pre-2c1 `Ready` arm validated only `canonical_source` before reusing the checkout, so a
    /// second configure would have been given a preserved checkout to write into — destroying
    /// preserved work without ever deleting anything.
    #[tokio::test]
    async fn a_preserved_checkout_is_never_reused_as_a_session_cwd() {
        let (be, rec, tmp, session, target, bound) = v3_session("v3-reuse-policy").await;
        be.preserve_checkout_v1(&session, CheckoutPreservationReasonV1::NodeFailure)
            .await;

        let reused = be.configure_bound_session(&session, &bound).await;

        assert!(
            matches!(reused, Err(BridgeError::ConfigInvalid { .. })),
            "a preserved checkout must be refused as a cwd: {reused:?}"
        );
        assert_eq!(record_state_of(&target).as_deref(), Some("preserved"));
        assert_eq!(removals(&rec), 0);
        std::fs::remove_dir_all(&tmp).unwrap();
    }

    /// The V2 half of the reuse policy, byte-identical: a legacy `Ready` entry is still reused
    /// after the same `canonical_source` check, with no probe and no new refusal. Discriminates a
    /// reuse policy implemented by consulting the filesystem on every configure.
    #[tokio::test]
    async fn a_legacy_ready_entry_is_still_reused_exactly_as_before() {
        let (be, rec, tmp, source, _cfg) = backend_fixture("v2-reuse-control");
        let session = SessionId::parse("ctx-v2-reuse-control-g0").unwrap();
        let spec = spec(Some(&source.to_string_lossy()));
        be.configure_session(&session, &spec).await.unwrap();
        let first = be.mapped_worktree_path_for_test(&session).await.unwrap();

        be.configure_session(&session, &spec).await.unwrap();

        assert_eq!(
            be.mapped_worktree_path_for_test(&session).await.unwrap(),
            first,
            "the second configure reuses the same checkout"
        );
        assert_eq!(rec.add_count.load(Ordering::SeqCst), 1, "and adds once");
        assert_eq!(removals(&rec), 0);
        std::fs::remove_dir_all(&tmp).unwrap();
    }

    /// A `Preserved` entry is still refused by a fresh `configure_session` too, not only by the
    /// bound path — the legacy entry point has its own `Ready` arm and its own copy of the policy.
    #[tokio::test]
    async fn the_legacy_configure_entry_also_refuses_a_retained_checkout() {
        let (be, rec, tmp, source, _cfg) = backend_fixture("legacy-retained-refuse");
        let session = SessionId::parse("ctx-legacy-retained-refuse-g0").unwrap();
        be.configure_session(&session, &spec(Some(&source.to_string_lossy())))
            .await
            .unwrap();
        publish_custody_record(&be.mapped_worktree_path_for_test(&session).await.unwrap());
        be.mark_checkout_protected_for_test(&session).await;
        // Force the terminal-retention path without a writer: the gate refuses on the
        // discriminator, and the barrier cannot mint a claim with no retained identities, so the
        // entry stays Ready. Preserve it explicitly through the map instead.
        {
            let mut map = be.map.lock().await;
            let entry = match map.remove(session.as_str()) {
                Some(WtState::Ready(entry)) => entry,
                other => panic!("expected a Ready entry, got {:?}", other.is_some()),
            };
            map.insert(
                session.as_str().to_owned(),
                WtState::Retained {
                    entry,
                    retention: CheckoutRetentionV1::Preserved,
                },
            );
        }

        let reused = be
            .configure_session(&session, &spec(Some(&source.to_string_lossy())))
            .await;

        assert!(
            matches!(reused, Err(BridgeError::ConfigInvalid { .. })),
            "unexpected: {reused:?}"
        );
        assert_eq!(removals(&rec), 0);
        std::fs::remove_dir_all(&tmp).unwrap();
    }

    /// A protected entry whose materialization identities were never captured must refuse to mint
    /// a claim rather than re-observing the paths and asserting whatever is there now.
    ///
    /// This is the `WtEntry.custody` / `WtEntry.protection` split doing its job: the gate still
    /// refuses the deletion (authority), and the barrier still refuses the claim (evidence).
    #[tokio::test]
    async fn a_protected_entry_with_no_retained_identities_refuses_to_mint_a_claim() {
        let (be, rec, tmp, source, _cfg) = backend_fixture("no-identities");
        let session = SessionId::parse("ctx-no-identities-g0").unwrap();
        be.configure_session(&session, &spec(Some(&source.to_string_lossy())))
            .await
            .unwrap();
        be.mark_checkout_protected_for_test(&session).await;

        let outcome = be
            .preserve_checkout_v1(&session, CheckoutPreservationReasonV1::NodeFailure)
            .await;

        assert!(
            matches!(outcome, CheckoutPreservationV1::Refused(_)),
            "unexpected: {outcome:?}"
        );
        assert!(outcome.is_protective());
        be.release_session_checked(&session).await.unwrap();
        assert_eq!(removals(&rec), 0, "and the gate still refuses the deletion");
        std::fs::remove_dir_all(&tmp).unwrap();
    }

    /// POSITIVE CONTROL for forget, release and retire: an unprotected (V2) checkout is still
    /// deleted by every strength. The gate must narrow deletion, not neuter it. The wider control
    /// is the thirteen pre-existing legacy `configure_session` tests, unchanged by this slice.
    #[tokio::test]
    async fn an_unprotected_checkout_is_still_deleted_by_every_cleanup_strength() {
        for (name, strength) in [("forget", true), ("release", false)] {
            let (be, rec, tmp, source, _cfg) = backend_fixture(&format!("gate-control-{name}"));
            let sid = SessionId::parse(format!("ctx-gate-control-{name}-g0")).unwrap();
            be.configure_session(&sid, &spec(Some(&source.to_string_lossy())))
                .await
                .unwrap();

            if strength {
                be.forget_session_checked(&sid).await.unwrap();
            } else {
                be.release_session_checked(&sid).await.unwrap();
            }

            assert_eq!(
                removals(&rec),
                1,
                "{name}: an unprotected checkout must still be removed"
            );
            assert!(be.map.lock().await.is_empty());
            std::fs::remove_dir_all(tmp).unwrap();
        }
    }

    // ---------------------------------------------------------------------------------------
    // R2f1b slice 2b2 — the V3 writer at the backend boundary.
    //
    // Everything here runs on the SAME fixture as the V2 tests above, so "V2 is byte-identical"
    // is a property of one harness rather than two.
    // ---------------------------------------------------------------------------------------

    /// The V2 `bound_spec`, with a matching custody plan bound onto its provider effect. This is
    /// the ONLY way a V3 route can exist — there is no production constructor for it.
    fn bound_spec_v3(
        source: &Path,
        cfg: &crate::provider_path::WorktreeConfig,
    ) -> (BoundSessionSpecV1, String) {
        use bridge_core::execution_policy::{
            BoundWorktreeCustodyV1, FrozenCheckoutEffectV1, FrozenWorktreeCustodyPlanV1,
            WorktreeCustodyIdV1,
        };
        let (spec, target) = bound_spec(source, cfg);
        let FrozenCheckoutEffectV1::Worktree {
            target_cwd,
            checkout_digest,
            ..
        } = &spec.provider_effect.frozen().checkout
        else {
            panic!("the bound fixture freezes a worktree checkout")
        };
        let attempt_id = AttemptId::parse(format!("attempt-{}", "2".repeat(32))).unwrap();
        let custody = BoundWorktreeCustodyV1 {
            attempt: bridge_core::ids::AttemptIdentity {
                execution_id: bridge_core::ids::ExecutionId::parse(format!(
                    "exec-{}",
                    "1".repeat(32)
                ))
                .unwrap(),
                attempt_id: attempt_id.clone(),
                ordinal: 0,
                parent_attempt_id: None,
            },
            origin_attempt_id: attempt_id,
            node: PolicyNodeRefV1::from_node_id(0, "node"),
            plan: FrozenWorktreeCustodyPlanV1 {
                custody_id: WorktreeCustodyIdV1::mint().unwrap(),
                checkout_fingerprint: checkout_digest.clone(),
                target_cwd: target_cwd.clone(),
            },
        };
        let effect = (*spec.provider_effect)
            .clone()
            .bind_custody_plan(Arc::new(custody))
            .expect("the plan matches the fixture's own frozen checkout");
        (
            BoundSessionSpecV1::new(EffectiveConfig::default(), Arc::new(effect)),
            target,
        )
    }

    fn record_state_of(target: &str) -> Option<String> {
        observed_record_state(target)
    }

    fn preparation_flight_state_of(target: &str) -> Option<String> {
        let path = format!("{target}{PREPARATION_FLIGHT_RECORD_SUFFIX}");
        let value: serde_json::Value = serde_json::from_slice(&std::fs::read(path).ok()?).ok()?;
        value
            .get("state")?
            .get("state")?
            .as_str()
            .map(str::to_owned)
    }

    /// `backend_fixture`, with a caller-chosen provider. Extracted so the V3 tests can swap the
    /// provider without duplicating the four-directory layout every fixture needs.
    fn provider_fixture(
        tmp: &Path,
        provider: impl FnOnce(Arc<Rec>) -> Arc<dyn crate::provider::WorktreeProvider>,
    ) -> (
        Arc<WorktreeBackend>,
        Arc<Rec>,
        PathBuf,
        crate::provider_path::WorktreeConfig,
    ) {
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
            provider(rec.clone()),
            cfg.clone(),
            Some(SessionCwd::parse(&canonical_allowed_root.to_string_lossy()).unwrap()),
            identity(),
        ));
        (be, rec, source, cfg)
    }

    /// Claims custody support and then fails the add with a raw `Err` — a git spawn failure, say.
    /// The point is the state the writer is already in when that happens: `Materializing`.
    struct CustodyAddErrProv {
        rec: Arc<Rec>,
    }

    #[async_trait::async_trait]
    impl crate::provider::WorktreeProvider for CustodyAddErrProv {
        fn supports_custody_add(&self) -> bool {
            true
        }

        async fn add(&self, _repo: &str, _worktree_path: &str) -> Result<String, BridgeError> {
            unreachable!("the V3 path never uses the V2 add")
        }

        async fn add_under_custody(
            &self,
            _repo: &str,
            worktree_path: &str,
        ) -> Result<CustodyAddOutcomeV1, BridgeError> {
            self.rec.add_count.fetch_add(1, Ordering::SeqCst);
            note_ordering(&self.rec, worktree_path);
            Err(BridgeError::agent_crashed("git spawn failed"))
        }

        async fn remove(&self, _repo: &str, _worktree_path: &str) -> Result<(), BridgeError> {
            self.rec.remove_count.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }

        async fn is_git_repo(&self, _path: &str) -> bool {
            true
        }
    }

    /// A panic after the barrier used to strand the configure waiter behind the active owner's
    /// retained result sender. The runner-exit guard must terminalize that exact flight instead.
    struct PanickingCustodyAddProv {
        rec: Arc<Rec>,
    }

    #[async_trait::async_trait]
    impl crate::provider::WorktreeProvider for PanickingCustodyAddProv {
        fn supports_custody_add(&self) -> bool {
            true
        }

        async fn add(&self, _repo: &str, _worktree_path: &str) -> Result<String, BridgeError> {
            unreachable!("the V3 path never uses the V2 add")
        }

        async fn add_under_custody(
            &self,
            _repo: &str,
            worktree_path: &str,
        ) -> Result<CustodyAddOutcomeV1, BridgeError> {
            self.rec.add_count.fetch_add(1, Ordering::SeqCst);
            note_ordering(&self.rec, worktree_path);
            panic!("panicking custody provider regression");
        }

        async fn remove(&self, _repo: &str, _worktree_path: &str) -> Result<(), BridgeError> {
            self.rec.remove_count.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }

        async fn is_git_repo(&self, _path: &str) -> bool {
            true
        }
    }

    struct BlockingCustodyAddProv {
        rec: Arc<Rec>,
        entered: Arc<Notify>,
        release: Arc<Notify>,
    }

    #[async_trait::async_trait]
    impl crate::provider::WorktreeProvider for BlockingCustodyAddProv {
        fn supports_custody_add(&self) -> bool {
            true
        }

        async fn add(&self, _repo: &str, _worktree_path: &str) -> Result<String, BridgeError> {
            unreachable!("the V3 cancellation tests use only the custody-aware add")
        }

        async fn add_under_custody(
            &self,
            repo: &str,
            worktree_path: &str,
        ) -> Result<CustodyAddOutcomeV1, BridgeError> {
            self.rec.add_count.fetch_add(1, Ordering::SeqCst);
            note_ordering(&self.rec, worktree_path);
            self.entered.notify_one();
            self.release.notified().await;
            std::fs::create_dir_all(worktree_path).unwrap();
            let common_dir = format!("{repo}/.git");
            std::fs::create_dir_all(&common_dir).unwrap();
            Ok(CustodyAddOutcomeV1::Materialized { common_dir })
        }

        async fn remove(&self, _repo: &str, _worktree_path: &str) -> Result<(), BridgeError> {
            self.rec.remove_count.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }

        async fn is_git_repo(&self, _path: &str) -> bool {
            true
        }
    }

    fn blocking_custody_fixture(
        tmp: &Path,
    ) -> (
        Arc<WorktreeBackend>,
        Arc<Rec>,
        PathBuf,
        crate::provider_path::WorktreeConfig,
        Arc<BlockingCustodyAddProv>,
    ) {
        let holder = Arc::new(StdMutex::new(None));
        let holder_for_provider = holder.clone();
        let (backend, rec, source, cfg) = provider_fixture(tmp, move |rec| {
            let provider = Arc::new(BlockingCustodyAddProv {
                rec,
                entered: Arc::new(Notify::new()),
                release: Arc::new(Notify::new()),
            });
            *holder_for_provider
                .lock()
                .unwrap_or_else(|error| error.into_inner()) = Some(provider.clone());
            provider
        });
        let provider = holder
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .take()
            .expect("provider fixture records its exact provider");
        (backend, rec, source, cfg, provider)
    }

    fn partial_add_fixture(
        tmp: &Path,
        target_absent: bool,
    ) -> (
        Arc<WorktreeBackend>,
        Arc<Rec>,
        PathBuf,
        crate::provider_path::WorktreeConfig,
    ) {
        provider_fixture(tmp, |rec| {
            Arc::new(PartialAddFailProv {
                rec,
                partial_target_absent: AtomicBool::new(target_absent),
            })
        })
    }

    /// M13 phase 1. The capability preflight happens before the preparation claim, so a refusing
    /// provider leaves neither a companion flight record nor a custody record.
    #[tokio::test]
    async fn preparation_before_claim_creates_no_flight_or_filesystem_effect() {
        let tmp = unique_temp_dir("preparation-before-claim");
        let (be, rec, source, cfg) = provider_fixture(&tmp, |rec| {
            Arc::new(BlockingProv {
                rec,
                add_entered: Arc::new(Notify::new()),
                allow_add: Arc::new(Notify::new()),
            })
        });
        let (bound, target) = bound_spec_v3(&source, &cfg);

        let result = be
            .configure_bound_session(
                &SessionId::parse("preparation-before-claim").unwrap(),
                &bound,
            )
            .await;

        assert!(matches!(result, Err(BridgeError::ConfigInvalid { .. })));
        assert_eq!(preparation_flight_state_of(&target), None);
        assert_eq!(record_state_of(&target), None);
        assert!(!Path::new(&target).exists());
        assert_eq!(rec.add_count.load(Ordering::SeqCst), 0);
        std::fs::remove_dir_all(tmp).unwrap();
    }

    /// A claimed flight is still terminal if Open became visible but its parent sync reported
    /// ambiguous. The writer verifies that the visible record is its own Open before replacing it
    /// with Failed, so no provider effect can follow an initial journal failure.
    #[tokio::test]
    async fn initial_open_publication_failure_is_durably_terminalized_as_failed() {
        let (be, rec, tmp, source, cfg) = backend_fixture("preparation-open-publication-failure");
        let (bound, target) = bound_spec_v3(&source, &cfg);
        be.preparation_test_hooks
            .fail_initial_open_parent_sync
            .store(true, Ordering::SeqCst);

        let error = be
            .configure_bound_session(
                &SessionId::parse("preparation-open-publication-failure").unwrap(),
                &bound,
            )
            .await
            .unwrap_err();

        assert_eq!(error, BridgeError::StoreFailure);
        assert_eq!(
            preparation_flight_state_of(&target).as_deref(),
            Some("failed"),
            "the claimed flight must have a durable Failed terminal"
        );
        assert_eq!(record_state_of(&target), None);
        assert_eq!(rec.add_count.load(Ordering::SeqCst), 0);
        assert!(!Path::new(&target).exists());
        std::fs::remove_dir_all(tmp).unwrap();
    }

    /// M13 phase 2. Once Open is durable, an observer cancellation cannot admit the provider
    /// effect. The retained runner reaches its own typed Failed terminal instead.
    #[tokio::test]
    async fn dropped_configure_after_claim_before_add_reaches_failed_without_add() {
        let (be, rec, tmp, source, cfg) = backend_fixture("preparation-after-claim");
        let (bound, target) = bound_spec_v3(&source, &cfg);
        let hooks = be.preparation_test_hooks.clone();
        hooks.pause_after_open.store(true, Ordering::SeqCst);
        let session = SessionId::parse("preparation-after-claim").unwrap();
        let configure_be = be.clone();
        let configure_session = session.clone();
        let configure = tokio::spawn(async move {
            configure_be
                .configure_bound_session(&configure_session, &bound)
                .await
        });

        hooks.wait_for_open().await;
        assert_eq!(
            preparation_flight_state_of(&target).as_deref(),
            Some("open")
        );
        assert_eq!(rec.add_count.load(Ordering::SeqCst), 0);
        configure.abort();
        let _ = configure.await;
        hooks.release_after_open.notify_one();
        hooks.wait_for_terminal().await;

        assert_eq!(
            preparation_flight_state_of(&target).as_deref(),
            Some("failed")
        );
        assert!(
            std::fs::read_to_string(format!("{target}{PREPARATION_FLIGHT_RECORD_SUFFIX}"))
                .unwrap()
                .contains("bridge.worktree_preparation_caller_departed")
        );
        assert_eq!(record_state_of(&target), None);
        assert!(!Path::new(&target).exists());
        assert_eq!(rec.add_count.load(Ordering::SeqCst), 0);
        std::fs::remove_dir_all(tmp).unwrap();
    }

    /// M13 phase 3. Aborting the caller while the provider is paused does not abort the claimed
    /// task: the provider completes, the custody record becomes live, and the flight Settles.
    #[tokio::test]
    async fn dropped_configure_mid_add_runs_claimed_materialization_to_settled() {
        let tmp = unique_temp_dir("preparation-mid-add");
        let (be, rec, source, cfg, provider) = blocking_custody_fixture(&tmp);
        let (bound, target) = bound_spec_v3(&source, &cfg);
        let hooks = be.preparation_test_hooks.clone();
        let session = SessionId::parse("preparation-mid-add").unwrap();
        let configure_be = be.clone();
        let configure_session = session.clone();
        let configure = tokio::spawn(async move {
            configure_be
                .configure_bound_session(&configure_session, &bound)
                .await
        });

        provider.entered.notified().await;
        assert_eq!(record_state_of(&target).as_deref(), Some("materializing"));
        assert_eq!(
            preparation_flight_state_of(&target).as_deref(),
            Some("barrier_synced")
        );
        configure.abort();
        let _ = configure.await;
        provider.release.notify_one();
        hooks.wait_for_terminal().await;

        assert_eq!(
            preparation_flight_state_of(&target).as_deref(),
            Some("settled")
        );
        assert_eq!(record_state_of(&target).as_deref(), Some("live_protected"));
        assert!(Path::new(&target).is_dir());
        assert_eq!(rec.remove_count.load(Ordering::SeqCst), 0);
        std::fs::remove_dir_all(tmp).unwrap();
    }

    /// R2f1b 3d repair R1a: once the cancellation sample commits an in-progress add, cleanup
    /// joins the runner instead of consuming the reservation while its custody cell is contended.
    /// The assertion is a real preservation claim over the retained identities, not merely the
    /// on-disk `LiveProtected` state.
    #[tokio::test]
    async fn canceled_committed_add_retains_exact_evidence_through_cleanup_projection() {
        let tmp = unique_temp_dir("preparation-committed-add-projection");
        let (be, rec, source, cfg, provider) = blocking_custody_fixture(&tmp);
        let (bound, target) = bound_spec_v3(&source, &cfg);
        let hooks = be.preparation_test_hooks.clone();
        let session = SessionId::parse("preparation-committed-add-projection").unwrap();
        let configure_be = be.clone();
        let configure_session = session.clone();
        let configure = tokio::spawn(async move {
            configure_be
                .configure_bound_session(&configure_session, &bound)
                .await
        });

        provider.entered.notified().await;
        configure.abort();
        assert!(configure.await.unwrap_err().is_cancelled());
        be.wait_for_cleanup_waiting_preparation().await;
        let cleanup = be
            .cleanup_flight_report(&session)
            .expect("dropped configure starts a joinable cleanup flight");

        provider.release.notify_one();
        hooks.wait_for_terminal().await;
        let report = tokio::time::timeout(Duration::from_secs(2), wait_for_cleanup_report(cleanup))
            .await
            .expect("cleanup must report after the committed add projects");
        assert!(
            report.is_ok(),
            "cleanup must retain the projected protected entry"
        );
        assert_eq!(
            be.preserve_checkout_v1(&session, CheckoutPreservationReasonV1::NodeFailure)
                .await,
            CheckoutPreservationV1::Preserved,
            "the retained descriptor identities must mint a real preservation claim"
        );
        assert_eq!(
            be.settle_workflow_checkout_v1(
                &session,
                WorkflowCheckoutOutcomeV1::NotHealthy(CheckoutPreservationReasonV1::NodeFailure),
            )
            .await,
            CheckoutSettlementV1::Preserved,
            "the workflow settlement must find the same retained exact identities"
        );
        assert_eq!(record_state_of(&target).as_deref(), Some("preserved"));
        assert_eq!(rec.remove_count.load(Ordering::SeqCst), 0);
        std::fs::remove_dir_all(tmp).unwrap();
    }

    /// R2f1b 3d repair R1b: the false sample has committed the runner even before
    /// `WorktreeCustodianV1::enter`. Cleanup must not take that still-unmaterialized reservation.
    #[tokio::test]
    async fn canceled_committed_pre_enter_flight_keeps_its_reservation_for_projection() {
        let (be, rec, tmp, source, cfg) = backend_fixture("preparation-pre-enter-projection");
        let (bound, target) = bound_spec_v3(&source, &cfg);
        let hooks = be.preparation_test_hooks.clone();
        hooks
            .pause_after_add_admission
            .store(true, Ordering::SeqCst);
        let session = SessionId::parse("preparation-pre-enter-projection").unwrap();
        let configure_be = be.clone();
        let configure_session = session.clone();
        let configure = tokio::spawn(async move {
            configure_be
                .configure_bound_session(&configure_session, &bound)
                .await
        });

        hooks.wait_for_add_admission().await;
        configure.abort();
        assert!(configure.await.unwrap_err().is_cancelled());
        be.wait_for_cleanup_waiting_preparation().await;
        let cleanup = be
            .cleanup_flight_report(&session)
            .expect("dropped configure starts a joinable cleanup flight");

        hooks.release_after_add_admission.notify_one();
        hooks.wait_for_terminal().await;
        let report = tokio::time::timeout(Duration::from_secs(2), wait_for_cleanup_report(cleanup))
            .await
            .expect("cleanup must report after the pre-enter runner projects");
        assert!(
            report.is_ok(),
            "cleanup must wait for the pre-enter committed runner to project the reservation"
        );
        assert_eq!(
            be.settle_workflow_checkout_v1(
                &session,
                WorkflowCheckoutOutcomeV1::NotHealthy(CheckoutPreservationReasonV1::Cancellation),
            )
            .await,
            CheckoutSettlementV1::Preserved,
            "the runner must retain exact identities across the pre-enter cancellation window"
        );
        assert_eq!(record_state_of(&target).as_deref(), Some("preserved"));
        assert_eq!(rec.remove_count.load(Ordering::SeqCst), 0);
        std::fs::remove_dir_all(tmp).unwrap();
    }

    /// R2f1b 3d repair R2: a departed receiver cannot swallow a failed terminal journal write.
    /// Cleanup observes the retained typed debt, and retirement reports it too.
    #[tokio::test]
    async fn departed_terminal_publication_failure_remains_backend_owned_and_loud() {
        let (be, _rec, tmp, source, cfg) = backend_fixture("preparation-detached-terminal-debt");
        let (bound, target) = bound_spec_v3(&source, &cfg);
        let hooks = be.preparation_test_hooks.clone();
        hooks.pause_after_add.store(true, Ordering::SeqCst);
        let session = SessionId::parse("preparation-detached-terminal-debt").unwrap();
        let configure_be = be.clone();
        let configure_session = session.clone();
        let configure = tokio::spawn(async move {
            configure_be
                .configure_bound_session(&configure_session, &bound)
                .await
        });

        hooks.wait_for_add().await;
        configure.abort();
        assert!(configure.await.unwrap_err().is_cancelled());
        be.wait_for_cleanup_waiting_preparation().await;
        let cleanup = be
            .cleanup_flight_report(&session)
            .expect("dropped configure starts a joinable cleanup flight");
        hooks
            .fail_terminal_publication
            .store(true, Ordering::SeqCst);
        hooks.release_after_add.notify_one();

        let report = tokio::time::timeout(Duration::from_secs(2), wait_for_cleanup_report(cleanup))
            .await
            .expect("cleanup must report the retained terminal-publication debt");
        assert_eq!(report.result, Err(BridgeError::StoreFailure));
        assert_eq!(
            be.preparation_flight_debt_for_test(&session),
            Some(BridgeError::StoreFailure),
            "the active owner survives a departed one-shot receiver"
        );
        assert_eq!(
            preparation_flight_state_of(&target).as_deref(),
            Some("barrier_synced")
        );
        assert_eq!(record_state_of(&target).as_deref(), Some("live_protected"));
        assert_eq!(be.retire().await, Err(BridgeError::StoreFailure));
        std::fs::remove_dir_all(tmp).unwrap();
    }

    /// M13 phase 4. The provider has materialized the target but the terminal evidence has not
    /// yet been published. A dropped observer cannot erase that truth or fabricate LiveProtected
    /// before descriptor evidence is captured.
    #[tokio::test]
    async fn dropped_configure_after_add_before_evidence_preserves_truth_until_settlement() {
        let (be, rec, tmp, source, cfg) = backend_fixture("preparation-after-add");
        let (bound, target) = bound_spec_v3(&source, &cfg);
        let hooks = be.preparation_test_hooks.clone();
        hooks.pause_after_add.store(true, Ordering::SeqCst);
        let session = SessionId::parse("preparation-after-add").unwrap();
        let configure_be = be.clone();
        let configure_session = session.clone();
        let configure = tokio::spawn(async move {
            configure_be
                .configure_bound_session(&configure_session, &bound)
                .await
        });

        hooks.wait_for_add().await;
        assert!(
            Path::new(&target).is_dir(),
            "the provider effect really occurred"
        );
        assert_eq!(record_state_of(&target).as_deref(), Some("materializing"));
        assert_eq!(
            preparation_flight_state_of(&target).as_deref(),
            Some("barrier_synced")
        );
        configure.abort();
        let _ = configure.await;
        hooks.release_after_add.notify_one();
        hooks.wait_for_terminal().await;

        assert_eq!(
            preparation_flight_state_of(&target).as_deref(),
            Some("settled")
        );
        assert_eq!(record_state_of(&target).as_deref(), Some("live_protected"));
        assert_eq!(rec.remove_count.load(Ordering::SeqCst), 0);
        std::fs::remove_dir_all(tmp).unwrap();
    }

    /// M13 phase 5. A terminal companion-record failure is not swallowed: the caller receives a
    /// typed StoreFailure while the already materialized custody record remains the durable truth.
    #[tokio::test]
    async fn preparation_terminal_publication_failure_is_typed_and_loud() {
        let (be, rec, tmp, source, cfg) = backend_fixture("preparation-terminal-failure");
        let (bound, target) = bound_spec_v3(&source, &cfg);
        be.preparation_test_hooks
            .fail_terminal_publication
            .store(true, Ordering::SeqCst);

        let error = be
            .configure_bound_session(
                &SessionId::parse("preparation-terminal-failure").unwrap(),
                &bound,
            )
            .await
            .unwrap_err();

        assert_eq!(error, BridgeError::StoreFailure);
        assert_eq!(
            preparation_flight_state_of(&target).as_deref(),
            Some("barrier_synced"),
            "a refused terminal write cannot be reported as settled"
        );
        assert_eq!(record_state_of(&target).as_deref(), Some("live_protected"));
        assert!(Path::new(&target).is_dir());
        assert_eq!(rec.remove_count.load(Ordering::SeqCst), 0);
        std::fs::remove_dir_all(tmp).unwrap();
    }

    /// THE ordering property (§2.5, R-3): the custody record is durable and past
    /// `ProtectionPrepared` before `git worktree add` is entered. The witness is taken INSIDE the
    /// provider, so it cannot be satisfied by a writer that publishes after the add returns.
    ///
    /// Discriminates: today's V2 order (add first — the observation would be `None`); a writer
    /// that publishes `ProtectionPrepared` but never advances to `Materializing` (the observation
    /// would be `protection_prepared`, leaving a crash during the add indistinguishable from one
    /// before it); and a V3 path that also emits the legacy sidecar.
    #[tokio::test]
    async fn custody_record_is_parent_synced_before_any_git_worktree_add() {
        let (be, rec, tmp, source, cfg) = backend_fixture("v3-ordering");
        let (bound, target) = bound_spec_v3(&source, &cfg);
        let session = SessionId::parse("v3-ordering").unwrap();

        be.configure_bound_session(&session, &bound).await.unwrap();

        assert_eq!(
            rec.record_state_at_add.lock().unwrap().clone(),
            vec![Some("materializing".to_string())],
            "the record must be durable and in Materializing when the add is entered"
        );
        assert_eq!(
            rec.legacy_sidecar_at_add.lock().unwrap().clone(),
            vec![false],
            "the V3 path must not have written a .meta.json by add time either"
        );
        assert_eq!(
            record_state_of(&target).as_deref(),
            Some("live_protected"),
            "a materialized checkout settles LiveProtected"
        );
        std::fs::remove_dir_all(&tmp).unwrap();
    }

    /// The rollback CONDITION (§2.2 "Record naming"; brief §4): the V3 path writes no
    /// `.meta.json`, ever. Discriminates a writer that keeps the legacy sidecar "for
    /// compatibility" — which would hand the same checkout to the legacy boot arm, which deletes,
    /// while the V3 arm believes it is protecting it.
    #[tokio::test]
    async fn v3_path_writes_no_legacy_meta_json() {
        let (be, _rec, tmp, source, cfg) = backend_fixture("v3-no-legacy");
        let (bound, target) = bound_spec_v3(&source, &cfg);

        be.configure_bound_session(&SessionId::parse("v3-no-legacy").unwrap(), &bound)
            .await
            .unwrap();

        assert!(
            !Path::new(&sidecar_path(&target)).exists(),
            "no legacy sidecar may exist beside a V3 checkout"
        );
        assert!(Path::new(&crate::custody::custody_record_path(&target)).exists());
        std::fs::remove_dir_all(&tmp).unwrap();
    }

    /// The rollback GUARANTEE: an older binary enumerates only `*.meta.json`, so it cannot name a
    /// V3 checkout and therefore cannot select it for removal. Modelled with the legacy scanner's
    /// own predicate over the real directory a V3 configure produced.
    #[tokio::test]
    async fn old_binary_sweep_cannot_select_a_v3_checkout() {
        let (be, _rec, tmp, source, cfg) = backend_fixture("v3-rollback");
        let (bound, target) = bound_spec_v3(&source, &cfg);
        be.configure_bound_session(&SessionId::parse("v3-rollback").unwrap(), &bound)
            .await
            .unwrap();

        let legacy_selected: Vec<String> = std::fs::read_dir(&cfg.root)
            .unwrap()
            .flatten()
            .map(|entry| entry.path().to_string_lossy().into_owned())
            .filter(|path| path.ends_with(".meta.json"))
            .collect();

        assert!(
            legacy_selected.is_empty(),
            "an old binary's scanner must enumerate nothing here: {legacy_selected:?}"
        );
        assert!(
            Path::new(&target).is_dir(),
            "the checkout itself is present"
        );
        std::fs::remove_dir_all(&tmp).unwrap();
    }

    /// §5.7 row 4 / §2.5's `cleanup_failed_add` prohibition. The provider creates the target, then
    /// fails; the target and its contents MUST survive.
    ///
    /// Discriminates the V2 behaviour exactly: `HostGitWorktree::add` calls `cleanup_failed_add`,
    /// whose `remove_dir_all` would take the whole directory — including work — and it sits
    /// outside the 2b1 deletion gate, so nothing else would stop it.
    #[tokio::test]
    async fn add_failure_after_target_creation_never_removes_target() {
        let tmp = unique_temp_dir("v3-partial-add");
        let (be, rec, source, cfg) = partial_add_fixture(&tmp, false);
        let (bound, target) = bound_spec_v3(&source, &cfg);

        let result = be
            .configure_bound_session(&SessionId::parse("v3-partial-add").unwrap(), &bound)
            .await;

        assert!(result.is_err(), "the configure reports the add failure");
        assert!(
            Path::new(&target).is_dir(),
            "the partially added target must survive"
        );
        assert_eq!(
            std::fs::read_to_string(format!("{target}/work.txt")).unwrap(),
            "unsaved work",
            "and so must everything inside it"
        );
        assert_eq!(rec.remove_count.load(Ordering::SeqCst), 0);
        std::fs::remove_dir_all(&tmp).unwrap();
    }

    /// The other half of §6's "Partial add preserved" row: a failure before any target exists
    /// preserves an unknown disposition and touches nothing else — no checkout is created, none is
    /// removed, and no legacy sidecar appears.
    ///
    /// RENAMED from `..._settles_unused_marker_only` in the 2b2 repair round (opus W-4), because
    /// the dual review RULED the shipped behaviour correct rather than a deviation: §5.7 row 3
    /// ("prepared synced, before `git add`") is a CRASH case recovering from `ProtectionPrepared`
    /// — the state 2a's frozen `ProtectionPrepared -> UnusedSettled` edge already serves — so
    /// `UnusedSettled` is a RECOVERY-side transition, not an in-line writer transition. 2a's own
    /// identity data anticipated this arm exactly: `PreservationUnknown{MaterializationInFlight}`
    /// is the only degraded-legal preservation reason, which is precisely the shape a writer that
    /// has already published `Materializing` can produce. The `Materializing -> UnusedSettled`
    /// edge is deliberately NOT added.
    ///
    /// Every "only" assertion the row demands is kept below.
    #[tokio::test]
    async fn add_failure_before_any_target_preserves_unknown_and_touches_nothing() {
        let tmp = unique_temp_dir("v3-absent-add");
        let (be, rec, source, cfg) = partial_add_fixture(&tmp, true);
        let (bound, target) = bound_spec_v3(&source, &cfg);

        let result = be
            .configure_bound_session(&SessionId::parse("v3-absent-add").unwrap(), &bound)
            .await;

        assert!(result.is_err());
        assert!(
            !Path::new(&target).exists(),
            "nothing may be created for a checkout that never materialized"
        );
        assert!(!Path::new(&sidecar_path(&target)).exists());
        assert_eq!(rec.remove_count.load(Ordering::SeqCst), 0);
        let state = record_state_of(&target).expect("the marker is retained and readable");
        assert_eq!(state, "preservation_unknown");
        let record = crate::custody::WorktreeCustodyRecordV1::decode_canonical(
            &std::fs::read(crate::custody::custody_record_path(&target)).unwrap(),
        )
        .unwrap();
        assert!(
            !record.sweep_disposition().authorizes_checkout_removal(),
            "the settled marker must not license any removal"
        );
        std::fs::remove_dir_all(&tmp).unwrap();
    }

    /// §5.1: "if materialization is unresolved, publish
    /// `PreservationUnknown{materialization_inflight}`". Discriminates a writer that leaves the
    /// record in `Materializing` (indistinguishable from a live in-flight add forever) or that
    /// publishes a preserving state with no claim, and one that discards the provider's
    /// `RegistrationUnproven` answer instead of recording it.
    #[tokio::test]
    async fn partial_add_publishes_preservation_unknown_materialization_inflight() {
        let tmp = unique_temp_dir("v3-preservation-unknown");
        let (be, _rec, source, cfg) = partial_add_fixture(&tmp, false);
        let (bound, target) = bound_spec_v3(&source, &cfg);

        let _ = be
            .configure_bound_session(
                &SessionId::parse("v3-preservation-unknown").unwrap(),
                &bound,
            )
            .await;

        let record = crate::custody::WorktreeCustodyRecordV1::decode_canonical(
            &std::fs::read(crate::custody::custody_record_path(&target)).unwrap(),
        )
        .unwrap();
        assert_eq!(
            record.state,
            crate::custody::WorktreeCustodyStateV1::PreservationUnknown {
                reason: crate::custody::PreservationReasonV1::MaterializationInFlight,
            }
        );
        let claim = record.claim.expect("this state requires a claim");
        assert_eq!(
            claim.recovery_locator,
            crate::custody::RecoveryLocatorV1::RegistrationUnproven {},
            "the provider's ambiguous registration probe must be recorded, not collapsed"
        );
        assert_eq!(claim.checkout_fingerprint, record.checkout_fingerprint);
        std::fs::remove_dir_all(&tmp).unwrap();
    }

    /// The REFUSING default is reachable and refuses BEFORE any effect. Discriminates a default
    /// that returns a successful-looking outcome, which would let an unmodified provider silently
    /// materialize a V3 checkout with no custody-aware handling at all.
    #[tokio::test]
    async fn a_provider_without_a_custody_aware_add_refuses_before_any_checkout_exists() {
        let tmp = unique_temp_dir("v3-refusing-default");
        let (be, rec, source, cfg) = provider_fixture(&tmp, |rec| {
            Arc::new(BlockingProv {
                rec,
                add_entered: Arc::new(Notify::new()),
                allow_add: Arc::new(Notify::new()),
            })
        });
        let (bound, target) = bound_spec_v3(&source, &cfg);

        let result = be
            .configure_bound_session(&SessionId::parse("v3-refusing-default").unwrap(), &bound)
            .await;

        assert!(
            result.is_err(),
            "the refusing default must fail the configure"
        );
        assert!(!Path::new(&target).exists());
        assert_eq!(
            rec.add_count.load(Ordering::SeqCst),
            0,
            "V2 `add` must not be used as a fallback"
        );
        std::fs::remove_dir_all(&tmp).unwrap();
    }

    /// S7, order 1: a deletion arriving while a writer holds the checkout's publication cell is
    /// REFUSED, not queued and not admitted. Discriminates 2b1's gate exactly as it stood — probe
    /// then remove, with nothing serializing the two — where a writer publishing between the two
    /// steps makes the removal delete a protected checkout.
    #[tokio::test]
    async fn a_cleanup_is_refused_while_a_writer_holds_the_checkout_publication_cell() {
        let (be, rec, tmp, source, cfg) = backend_fixture("v3-cell-race");
        let (bound, target) = bound_spec_v3(&source, &cfg);
        let session = SessionId::parse("v3-cell-race").unwrap();
        be.configure_bound_session(&session, &bound).await.unwrap();
        // Delete the record so the gate's DISK arm cannot be what refuses: the only remaining
        // protection is the cell itself, which is exactly what this test is about.
        std::fs::remove_file(crate::custody::custody_record_path(&target)).unwrap();
        be.mark_entry_legacy_for_test(&session).await;

        let held =
            crate::custody_lock::try_acquire_publication_lock_in(Path::new(&cfg.root), &target)
                .expect("the writer's cells released when its custodian dropped");
        be.release_session_checked(&session).await.unwrap();

        assert_eq!(
            rec.remove_count.load(Ordering::SeqCst),
            0,
            "a contended publication cell must refuse the removal"
        );
        assert!(Path::new(&target).is_dir());

        // Order 2: once the cell is free the same cleanup proceeds, so the refusal is the cell
        // and not a permanent wedge.
        drop(held);
        be.release_session_checked(&session).await.unwrap();
        assert_eq!(rec.remove_count.load(Ordering::SeqCst), 1);
        std::fs::remove_dir_all(&tmp).unwrap();
    }

    // ---- slice 2b2 repair R4: no permanent false `Materializing` ----

    /// R4's red test, half 1. A provider taking the REFUSING default must produce ZERO records and
    /// ZERO provider effects.
    ///
    /// Discriminates the shipped order, where the refusal surfaced only from
    /// `add_under_custody` — i.e. AFTER `ProtectionPrepared` and `Materializing` had been
    /// published and parent-synced. That left a durable record asserting a materialization that
    /// never began, in a LIVE state (`Materializing` classifies `Recover`), which nothing in
    /// R2f1b would ever resolve.
    #[tokio::test]
    async fn a_provider_without_custody_support_publishes_no_record_at_all() {
        let tmp = unique_temp_dir("v3-no-record-on-refusal");
        let (be, rec, source, cfg) = provider_fixture(&tmp, |rec| {
            Arc::new(BlockingProv {
                rec,
                add_entered: Arc::new(Notify::new()),
                allow_add: Arc::new(Notify::new()),
            })
        });
        let (bound, target) = bound_spec_v3(&source, &cfg);

        let result = be
            .configure_bound_session(&SessionId::parse("v3-no-record").unwrap(), &bound)
            .await;

        assert!(result.is_err());
        assert_eq!(
            record_state_of(&target),
            None,
            "the refusing default must leave NO custody record behind"
        );
        assert!(!Path::new(&crate::custody::custody_record_path(&target)).exists());
        assert_eq!(
            rec.add_count.load(Ordering::SeqCst),
            0,
            "and no provider add of either kind may have run"
        );
        assert!(!Path::new(&target).exists());
        // The ordinary rollback DOES run its provider removal here, and that is correct rather
        // than a leak: with no record published there is genuinely nothing under custody, the
        // gate authorizes the removal on that evidence, and the target never existed to remove.
        // Asserting zero removals would be asserting that a refused configure skips its rollback.
        assert_eq!(rec.remove_count.load(Ordering::SeqCst), 1);
        std::fs::remove_dir_all(&tmp).unwrap();
    }

    /// R4's red test, half 2. A runtime `Err` from a custody-capable provider — raised after the
    /// record is already `Materializing` — must be SETTLED, not propagated raw.
    ///
    /// Discriminates the shipped `add_under_custody(..).await?`: the `?` returned with the record
    /// left in `Materializing` forever. Here the record must reach a terminal unknown state, the
    /// target must be retained, and nothing may be removed.
    #[tokio::test]
    async fn a_runtime_add_error_settles_preservation_unknown_instead_of_leaving_materializing() {
        let tmp = unique_temp_dir("v3-add-err-settles");
        let (be, rec, source, cfg) =
            provider_fixture(&tmp, |rec| Arc::new(CustodyAddErrProv { rec }));
        let (bound, target) = bound_spec_v3(&source, &cfg);

        let result = be
            .configure_bound_session(&SessionId::parse("v3-add-err").unwrap(), &bound)
            .await;

        assert!(result.is_err(), "the configure still reports the failure");
        assert_eq!(
            rec.record_state_at_add.lock().unwrap().clone(),
            vec![Some("materializing".to_string())],
            "the record really was Materializing when the add ran — the state this repair is about"
        );
        let record = crate::custody::WorktreeCustodyRecordV1::decode_canonical(
            &std::fs::read(crate::custody::custody_record_path(&target)).unwrap(),
        )
        .unwrap();
        assert_eq!(
            record.state,
            crate::custody::WorktreeCustodyStateV1::PreservationUnknown {
                reason: crate::custody::PreservationReasonV1::MaterializationInFlight,
            },
            "a runtime add error must settle, never leave a permanent live Materializing"
        );
        let claim = record.claim.expect("this state requires a claim");
        assert_eq!(
            claim.recovery_locator,
            crate::custody::RecoveryLocatorV1::RegistrationUnproven {},
            "an operation that never reported must not invent a definite locator"
        );
        assert_eq!(rec.remove_count.load(Ordering::SeqCst), 0);
        std::fs::remove_dir_all(&tmp).unwrap();
    }
    // ---- slice 2c2: the deletion capability, end to end through the backend ----

    fn capability_removals(rec: &Rec) -> usize {
        rec.remove_v2_count.load(Ordering::SeqCst)
    }

    /// §5.1's own rule, and step 1 of this slice: **a node-local success is NOT a checkout
    /// disposition.** The node's own success teardown — the exact call `cleanup_cold_session`
    /// makes — must leave the checkout live, mapped, and undeleted, because the workflow outcome
    /// that could authorize deleting it is not known yet.
    ///
    /// Discriminates a slice that hangs the mint off the node teardown rather than off the
    /// post-loop settlement: a successful first node would then delete its checkout before a later
    /// sibling failed, which is precisely the loss §5.1's deferral exists to prevent.
    #[tokio::test]
    async fn node_local_success_cannot_remove_its_checkout() {
        let (be, rec, tmp, session, target, _bound) = v3_session("v3-node-success").await;

        be.release_session_observed(&session, Arc::new(CodeRec::default()))
            .await
            .unwrap();

        assert_eq!(removals(&rec), 0, "no raw-path removal");
        assert_eq!(capability_removals(&rec), 0, "and no capability removal");
        assert_eq!(record_state_of(&target).as_deref(), Some("live_protected"));
        assert!(Path::new(&target).exists());
        assert!(
            be.mapped_worktree_path_for_test(&session).await.is_some(),
            "the checkout keeps an in-memory owner for the post-loop pass"
        );
        std::fs::remove_dir_all(&tmp).unwrap();
    }

    /// The headline of the slice: a globally healthy workflow outcome mints a capability, consumes
    /// it, removes the checkout EXACTLY ONCE, and leaves the record as a tombstone.
    ///
    /// The second settlement is the exactly-once half. It must not remove again and must not
    /// re-mint — after the first removal the map has no entry, so there is nothing to settle.
    ///
    /// Discriminates a settlement that removes through the raw-path `remove` (asserted zero), one
    /// that skips the tombstone (the record would still say `delete_authorized`), and one that
    /// leaves the map entry behind (which would wedge the session id forever).
    #[tokio::test]
    async fn global_healthy_success_with_capability_removes_exactly_once() {
        let (be, rec, tmp, session, target, _bound) = v3_session("v3-healthy-remove").await;

        let first = be
            .settle_workflow_checkout_v1(&session, WorkflowCheckoutOutcomeV1::GloballyHealthy)
            .await;
        let second = be
            .settle_workflow_checkout_v1(&session, WorkflowCheckoutOutcomeV1::GloballyHealthy)
            .await;

        assert_eq!(first, CheckoutSettlementV1::Removed);
        assert!(first.removed_the_checkout());
        assert_eq!(
            second,
            CheckoutSettlementV1::NoCheckoutUnderCustody,
            "the second settlement has nothing left to settle"
        );
        assert_eq!(
            capability_removals(&rec),
            1,
            "exactly one capability removal"
        );
        assert_eq!(removals(&rec), 0, "and never the raw-path removal");
        assert!(!Path::new(&target).exists(), "the checkout is gone");
        assert_eq!(
            record_state_of(&target).as_deref(),
            Some("removed"),
            "and its record is the tombstone"
        );
        assert!(
            be.mapped_worktree_path_for_test(&session).await.is_none(),
            "no entry may stay mapped after a post-loop removal"
        );
        std::fs::remove_dir_all(&tmp).unwrap();
    }

    /// RA / sol-1: a failed inner teardown makes the otherwise healthy settlement cleanup-ambiguous
    /// for THIS checkout. It must retain the durable live custody record and never mint deletion.
    ///
    /// Discriminates the capability branch ignoring the inner teardown result.
    #[tokio::test]
    async fn a_failed_inner_release_skips_the_deletion_mint_and_retains_live_custody() {
        let (be, rec, tmp, session, target, _bound) = v3_session("v3-inner-release-failed").await;
        rec.fail_release.store(true, Ordering::SeqCst);

        let settled = be
            .settle_workflow_checkout_v1(&session, WorkflowCheckoutOutcomeV1::GloballyHealthy)
            .await;

        assert!(
            !matches!(settled, CheckoutSettlementV1::Removed),
            "a failed inner teardown must not report a removal: {settled:?}"
        );
        assert_eq!(
            capability_removals(&rec),
            0,
            "the capability must not be minted or consumed after a failed release"
        );
        assert!(
            Path::new(&target).exists(),
            "the checkout must remain on disk"
        );
        assert!(
            be.mapped_worktree_path_for_test(&session).await.is_some(),
            "the retained checkout keeps its map entry for recovery"
        );
        assert_eq!(
            record_state_of(&target).as_deref(),
            Some("live_protected"),
            "the skipped mint leaves durable custody live-protected"
        );
        std::fs::remove_dir_all(&tmp).unwrap();
    }

    /// RA / sol-1: inner teardown failures are per-checkout. A later failing checkout may not
    /// revise the removal already completed for an independent healthy checkout.
    #[tokio::test]
    async fn a_failed_inner_release_isolated_to_its_checkout() {
        let (first_be, first_rec, first_tmp, first_session, first_target, _first_bound) =
            v3_session("v3-inner-release-first").await;
        let first = first_be
            .settle_workflow_checkout_v1(&first_session, WorkflowCheckoutOutcomeV1::GloballyHealthy)
            .await;
        assert_eq!(first, CheckoutSettlementV1::Removed);
        assert_eq!(capability_removals(&first_rec), 1);
        assert!(!Path::new(&first_target).exists());

        let (second_be, second_rec, second_tmp, second_session, second_target, _second_bound) =
            v3_session("v3-inner-release-second").await;
        second_rec.fail_release.store(true, Ordering::SeqCst);
        let second = second_be
            .settle_workflow_checkout_v1(
                &second_session,
                WorkflowCheckoutOutcomeV1::GloballyHealthy,
            )
            .await;

        assert!(
            !matches!(second, CheckoutSettlementV1::Removed),
            "the failed checkout must be retained: {second:?}"
        );
        assert_eq!(capability_removals(&second_rec), 0);
        assert!(Path::new(&second_target).exists());
        assert_eq!(
            record_state_of(&second_target).as_deref(),
            Some("live_protected")
        );
        assert!(second_be
            .mapped_worktree_path_for_test(&second_session)
            .await
            .is_some());
        assert_eq!(
            capability_removals(&first_rec),
            1,
            "the first removal stays final"
        );
        assert!(!Path::new(&first_target).exists());
        std::fs::remove_dir_all(&first_tmp).unwrap();
        std::fs::remove_dir_all(&second_tmp).unwrap();
    }

    /// RB / sol-3: once `remove_v2` verified the checkout absent, a tombstone parent-sync failure
    /// is ambiguous durable evidence, not an ordinary removed settlement. The test arms the same
    /// `fs_custody` seam as the custody-writer tombstone boundary after the authorizing replace.
    #[tokio::test]
    async fn an_ambiguous_removed_tombstone_is_not_reported_as_plain_removed() {
        let (be, rec, tmp, session, target, _bound) = v3_session("v3-ambiguous-removed").await;
        be.fail_next_capability_tombstone_parent_sync_for_test();

        let settled = be
            .settle_workflow_checkout_v1(&session, WorkflowCheckoutOutcomeV1::GloballyHealthy)
            .await;

        assert!(
            matches!(settled, CheckoutSettlementV1::RemovedRecordAmbiguous(_)),
            "an unverified tombstone needs its typed outcome: {settled:?}"
        );
        assert!(
            !Path::new(&target).exists(),
            "the provider did remove the checkout"
        );
        assert_eq!(capability_removals(&rec), 1);
        assert!(
            be.mapped_worktree_path_for_test(&session).await.is_none(),
            "a verified-absent checkout must still clear its map entry"
        );
        std::fs::remove_dir_all(&tmp).unwrap();
    }

    /// RB / sol-3: observers must distinguish a removed checkout whose tombstone is uncertain
    /// from a durable `Removed` tombstone.
    #[tokio::test]
    async fn an_ambiguous_removed_tombstone_publishes_a_distinct_teardown_code() {
        let (be, _rec, tmp, session, _target, _bound) =
            v3_session("v3-ambiguous-removed-code").await;
        be.raise_checkout_disposition(&session, CheckoutDispositionV1::DeleteAuthorized, None)
            .await
            .expect("the session has a cleanup cell");
        be.fail_next_capability_tombstone_parent_sync_for_test();
        let observer = Arc::new(CodeRec::default());

        be.release_session_observed(&session, observer.clone())
            .await
            .unwrap();

        let codes = observer.codes.lock().unwrap().clone();
        assert!(
            codes.iter().any(|(status, code)| {
                status == "Completed" && code == "worktree.teardown.removed_record_ambiguous"
            }),
            "the ambiguous tombstone must have its own observed code: {codes:?}"
        );
        assert!(
            !codes
                .iter()
                .any(|(_, code)| code == "worktree.teardown.released"),
            "the durable-removed code is false evidence here: {codes:?}"
        );
        std::fs::remove_dir_all(&tmp).unwrap();
    }

    /// RC / opus WRONG-1: `PreservationUnknown` is terminal durable preservation, so a completed
    /// non-healthy settlement must never project it as recovery-owned `Retained`.
    #[tokio::test]
    async fn a_settled_preservation_unknown_is_classified_as_preservation() {
        let (be, _rec, tmp, session, target, _bound) = v3_session("v3-settled-unknown").await;

        // Pre-create the replacement while the protected checkout is still live, guaranteeing a
        // distinct inode even on ext4, then rename both objects to perform the same-name swap.
        let target_path = Path::new(&target);
        let name = target_path.file_name().unwrap().to_string_lossy();
        let replacement = target_path.with_file_name(format!("{name}.swap-replacement"));
        let displaced = target_path.with_file_name(format!("{name}.swap-original"));
        std::fs::create_dir(&replacement).unwrap();
        let before = observed_identity(&target).directory_identity;
        let candidate = observed_identity(&replacement.to_string_lossy()).directory_identity;
        assert!(
            !before.matches(&candidate),
            "precondition: simultaneously live original and replacement must have distinct identities"
        );
        std::fs::rename(target_path, &displaced).unwrap();
        std::fs::rename(&replacement, target_path).unwrap();
        let after = observed_identity(&target).directory_identity;
        assert!(
            !before.matches(&after),
            "precondition: the same-name replacement must not match the retained identity"
        );

        let settled = be
            .settle_workflow_checkout_v1(
                &session,
                WorkflowCheckoutOutcomeV1::NotHealthy(CheckoutPreservationReasonV1::NodeFailure),
            )
            .await;

        assert!(
            matches!(settled, CheckoutSettlementV1::Preserved),
            "a terminal preservation-unknown record is preservation, not retention: {settled:?}"
        );
        assert_eq!(
            record_state_of(&target).as_deref(),
            Some("preservation_unknown")
        );
        assert!(Path::new(&target).exists());
        std::fs::remove_dir_all(&tmp).unwrap();
    }

    /// §5.1's "a successful sibling cannot delete useful work before a later sibling determines the
    /// workflow outcome", driven as two real checkouts.
    ///
    /// The first session completes and runs its node-end success teardown; the workflow then
    /// fails. BOTH checkouts must end preserved — including the one that finished cleanly long
    /// before, which is the "including nodes that completed earlier" clause.
    ///
    /// Discriminates a settlement that only visits the failing node's checkout.
    #[tokio::test]
    async fn completed_sibling_survives_later_workflow_failure() {
        let (be, rec, tmp, done_session, done_target, _bound) = v3_session("v3-sibling-a").await;
        be.release_session_observed(&done_session, Arc::new(CodeRec::default()))
            .await
            .unwrap();
        assert_eq!(
            record_state_of(&done_target).as_deref(),
            Some("live_protected"),
            "the completed sibling is still live and undisposed"
        );

        let settled = be
            .settle_workflow_checkout_v1(
                &done_session,
                WorkflowCheckoutOutcomeV1::NotHealthy(CheckoutPreservationReasonV1::NodeFailure),
            )
            .await;

        assert_eq!(settled, CheckoutSettlementV1::Preserved);
        assert!(!settled.removed_the_checkout());
        assert_eq!(record_state_of(&done_target).as_deref(), Some("preserved"));
        assert!(Path::new(&done_target).exists(), "the work survives");
        assert_eq!(removals(&rec), 0);
        assert_eq!(capability_removals(&rec), 0);
        std::fs::remove_dir_all(&tmp).unwrap();
    }

    /// P2, the structural claim: **a raw-path provider removal is unreachable for a
    /// custody-discriminated checkout.** Every teardown surface the backend exposes is driven over
    /// one live V3 checkout, and none of them reaches `remove` or `remove_v2`.
    ///
    /// `remove` takes `(repo, path)` and is gated by the 2b1 fail-closed gate; `remove_v2` takes a
    /// capability that only the CAS can mint. This test is the behavioural half of the claim — the
    /// structural half is the signature itself, which no caller in this crate can satisfy without
    /// a `DeletionCapabilityV1`.
    ///
    /// Discriminates a slice that widened the gate (say, by treating `DeleteAuthorized` on disk as
    /// permission) instead of adding an authority.
    #[tokio::test]
    async fn raw_path_removal_is_unreachable_without_a_capability() {
        let (be, rec, tmp, session, target, _bound) = v3_session("v3-no-raw-removal").await;

        be.cancel(&session).await.unwrap();
        be.forget_session_checked(&session).await.unwrap();
        be.release_session_checked(&session).await.unwrap();
        be.forget_session(&session).await;
        be.release_session(&session).await;
        be.release_session_observed(&session, Arc::new(CodeRec::default()))
            .await
            .unwrap();
        be.retire().await.unwrap();

        assert_eq!(removals(&rec), 0, "no raw-path removal from any entry");
        assert_eq!(
            capability_removals(&rec),
            0,
            "and no capability removal without a workflow-level authority"
        );
        assert_eq!(record_state_of(&target).as_deref(), Some("live_protected"));
        assert!(Path::new(&target).exists());
        std::fs::remove_dir_all(&tmp).unwrap();
    }

    /// P2's identity rule at the backend: a checkout whose objects were replaced since
    /// materialization must not be removed, even under a globally healthy outcome.
    ///
    /// The target is swapped for a different directory at the same path. The mint's own
    /// reverification refuses first, so `remove_v2` is never reached at all — the assertion is on
    /// the provider's call count, not on a message.
    ///
    /// Discriminates a mint that compares canonical paths instead of `dev`/`ino`: the swapped
    /// directory has the same path and would pass.
    #[tokio::test]
    async fn remove_v2_refuses_when_object_identity_changed_since_authorization() {
        let (be, rec, tmp, session, target, _bound) = v3_session("v3-identity-changed").await;
        std::fs::remove_dir_all(&target).unwrap();
        std::fs::create_dir_all(&target).unwrap();
        std::fs::write(format!("{target}/someone-elses-work.txt"), b"not ours").unwrap();

        let settled = be
            .settle_workflow_checkout_v1(&session, WorkflowCheckoutOutcomeV1::GloballyHealthy)
            .await;

        assert!(
            matches!(settled, CheckoutSettlementV1::Retained(_)),
            "a changed object graph is retained, never removed: {settled:?}"
        );
        assert_eq!(capability_removals(&rec), 0, "remove_v2 is never reached");
        assert_eq!(removals(&rec), 0);
        assert!(
            Path::new(&format!("{target}/someone-elses-work.txt")).exists(),
            "whatever now occupies the path must be untouched"
        );
        assert_eq!(
            record_state_of(&target).as_deref(),
            Some("live_protected"),
            "and the CAS never ran"
        );
        std::fs::remove_dir_all(&tmp).unwrap();
    }

    /// V2 POSITIVE CONTROL for the whole settlement (the brief's "V2 cleanup byte-identical"):
    /// a legacy checkout answers `NoCheckoutUnderCustody` for BOTH outcomes, with no removal, no
    /// extra cleanup flight, and no cell conjured — and the ordinary V2 teardown still removes it
    /// afterwards exactly as before.
    ///
    /// Discriminates a settlement that raises a disposition or starts a flight before checking the
    /// custody discriminator, which would give every V2 session an extra release at workflow end.
    #[tokio::test]
    async fn the_workflow_settlement_is_a_no_op_for_a_legacy_checkout() {
        let (be, rec, tmp, source, _cfg) = backend_fixture("v2-settlement-noop");
        let session = SessionId::parse("ctx-v2-settlement-noop-g0").unwrap();
        be.configure_session(&session, &spec(Some(&source.to_string_lossy())))
            .await
            .unwrap();
        let cells_before = be.cleanup_cell_count();

        for outcome in [
            WorkflowCheckoutOutcomeV1::GloballyHealthy,
            WorkflowCheckoutOutcomeV1::NotHealthy(CheckoutPreservationReasonV1::Cancellation),
        ] {
            assert_eq!(
                be.settle_workflow_checkout_v1(&session, outcome).await,
                CheckoutSettlementV1::NoCheckoutUnderCustody,
                "{outcome:?}"
            );
        }

        assert_eq!(removals(&rec), 0, "the settlement removed nothing");
        assert_eq!(capability_removals(&rec), 0);
        assert_eq!(
            be.cleanup_cell_count(),
            cells_before,
            "and started no flight for a legacy checkout"
        );
        be.release_session_checked(&session).await.unwrap();
        assert_eq!(removals(&rec), 1, "V2 teardown is byte-identical");
        assert!(be.map.lock().await.is_empty());
        std::fs::remove_dir_all(&tmp).unwrap();
    }

    /// An unmapped session is settled without conjuring anything — the same discipline the
    /// preservation barrier holds.
    #[tokio::test]
    async fn the_workflow_settlement_answers_no_checkout_for_an_unmapped_session() {
        let (be, _rec, tmp, _source, _cfg) = backend_fixture("v3-settle-unmapped");
        let session = SessionId::parse("ctx-v3-settle-unmapped-g0").unwrap();

        let settled = be
            .settle_workflow_checkout_v1(&session, WorkflowCheckoutOutcomeV1::GloballyHealthy)
            .await;

        assert_eq!(settled, CheckoutSettlementV1::NoCheckoutUnderCustody);
        assert_eq!(be.cleanup_cell_count(), 0);
        std::fs::remove_dir_all(&tmp).unwrap();
    }

    /// P7 boundaries 2 and 4: the git removal did not verifiably complete, so the record must NOT
    /// say `Removed`. It stays `DeleteAuthorized` — 2a classifies that `Recover`, so the checkout
    /// is recovery-owned — and the work is still on disk.
    ///
    /// There is deliberately no escape edge here: `DeleteAuthorized -> PreservationPrepared` is not
    /// in 2a's frozen table, so "preserve it instead" is not available to this slice, and inventing
    /// the edge would be the non-goal. Recovery ownership is the defined result.
    ///
    /// Discriminates a settlement that tombstones on the strength of having minted the capability
    /// rather than on the provider's verified post-conditions.
    #[tokio::test]
    async fn a_failed_capability_removal_never_records_removed() {
        let (be, rec, tmp, session, target, _bound) = v3_session("v3-removal-failed").await;
        rec.fail_remove_v2.store(true, Ordering::SeqCst);

        let settled = be
            .settle_workflow_checkout_v1(&session, WorkflowCheckoutOutcomeV1::GloballyHealthy)
            .await;

        assert!(
            matches!(settled, CheckoutSettlementV1::Retained(_)),
            "{settled:?}"
        );
        assert!(!settled.removed_the_checkout());
        assert_eq!(
            capability_removals(&rec),
            1,
            "the removal was attempted once"
        );
        assert_eq!(
            record_state_of(&target).as_deref(),
            Some("delete_authorized"),
            "no tombstone over a removal that did not complete"
        );
        assert!(Path::new(&target).exists(), "and the work is still there");
        assert_eq!(
            WorktreeCustodyStateKindV1::DeleteAuthorized.sweep_disposition(),
            crate::custody::CustodySweepDispositionV1::Recover,
            "which makes the checkout recovery-owned, never sweep-deletable"
        );
        std::fs::remove_dir_all(&tmp).unwrap();
    }

    /// P7 boundary 1, end to end: **no re-mint from a stale capability.** After a crash-equivalent
    /// (authorized, not removed) the record is `DeleteAuthorized`, and a SECOND globally-healthy
    /// settlement must not authorize anything, must not remove, and must not reach the provider at
    /// all — the CAS refuses from that from-state and the flight falls through to the gate, which
    /// refuses on the custody evidence.
    ///
    /// Discriminates a mint whose from-state check accepts `DeleteAuthorized`, which would let any
    /// number of later settlements re-acquire deletion authority over a recovery-owned checkout.
    #[tokio::test]
    async fn a_stranded_authorization_is_recovery_owned_and_never_re_minted() {
        let (be, rec, tmp, session, target, _bound) = v3_session("v3-stranded-auth").await;
        rec.fail_remove_v2.store(true, Ordering::SeqCst);
        assert!(matches!(
            be.settle_workflow_checkout_v1(&session, WorkflowCheckoutOutcomeV1::GloballyHealthy)
                .await,
            CheckoutSettlementV1::Retained(_)
        ));
        rec.fail_remove_v2.store(false, Ordering::SeqCst);

        let second = be
            .settle_workflow_checkout_v1(&session, WorkflowCheckoutOutcomeV1::GloballyHealthy)
            .await;

        assert!(
            matches!(second, CheckoutSettlementV1::Retained(_)),
            "a stranded authorization must not be re-minted: {second:?}"
        );
        assert_eq!(
            capability_removals(&rec),
            1,
            "the provider is never reached a second time"
        );
        assert_eq!(removals(&rec), 0);
        assert_eq!(
            record_state_of(&target).as_deref(),
            Some("delete_authorized")
        );
        assert!(Path::new(&target).exists());
        std::fs::remove_dir_all(&tmp).unwrap();
    }

    /// P7 boundary 4, isolated from boundary 2: the removal reports failure specifically BECAUSE
    /// its post-condition probe disagreed (the target is still present). Same rule — never record
    /// `Removed` over a disagreeing probe — reached through the disagreement rather than through a
    /// git error.
    #[tokio::test]
    async fn a_post_condition_disagreement_never_records_removed() {
        let (be, rec, tmp, session, target, _bound) = v3_session("v3-postcondition").await;
        rec.remove_v2_leaves_target.store(true, Ordering::SeqCst);

        let settled = be
            .settle_workflow_checkout_v1(&session, WorkflowCheckoutOutcomeV1::GloballyHealthy)
            .await;

        assert!(matches!(settled, CheckoutSettlementV1::Retained(_)));
        assert!(
            Path::new(&target).exists(),
            "the target the probe found is still there"
        );
        assert_eq!(
            record_state_of(&target).as_deref(),
            Some("delete_authorized")
        );
        std::fs::remove_dir_all(&tmp).unwrap();
    }

    /// §5.1's monotonicity with the THIRD disposition present: "once a preserved claim exists ...
    /// no later healthy projection or TTL can mint deletion authority."
    ///
    /// Two independent mechanisms have to hold here and both are asserted: the in-memory `Ord`
    /// (`Preserve` dominates `DeleteAuthorized`, so `raise_checkout_disposition` does not lower the
    /// cell and the flight never enters the mint branch) and the durable from-state check (which
    /// would refuse anyway, because the record says `preserved`).
    ///
    /// Discriminates an `Ord` that placed `DeleteAuthorized` above `Preserve`, and a
    /// `raise_checkout_disposition` that assigns rather than raises.
    #[tokio::test]
    async fn a_preserved_checkout_is_never_removed_by_a_later_healthy_settlement() {
        let (be, rec, tmp, session, target, _bound) = v3_session("v3-no-downgrade").await;
        assert_eq!(
            be.preserve_checkout_v1(&session, CheckoutPreservationReasonV1::NodeFailure)
                .await,
            CheckoutPreservationV1::Preserved
        );

        let settled = be
            .settle_workflow_checkout_v1(&session, WorkflowCheckoutOutcomeV1::GloballyHealthy)
            .await;

        assert_eq!(settled, CheckoutSettlementV1::Preserved);
        assert_eq!(capability_removals(&rec), 0, "no mint, no removal");
        assert_eq!(removals(&rec), 0);
        assert_eq!(record_state_of(&target).as_deref(), Some("preserved"));
        assert!(Path::new(&target).exists());
        assert!(
            CheckoutDispositionV1::Preserve > CheckoutDispositionV1::DeleteAuthorized,
            "the Ord is the in-memory half of the rule"
        );
        std::fs::remove_dir_all(&tmp).unwrap();
    }

    /// P3's wrong-join hazard with the third disposition present: a deletion-authority request
    /// 3s repair R1 (both review lenses converged): the deletion-admission guard must span the
    /// map PROJECTION of a completed removal, not just the removal itself. A preservation writer
    /// queued while the tombstone exists but the map still holds the old entry must stay blocked
    /// until the projection lands, then observe `NoCheckoutUnderCustody` — never a refusal
    /// against the stale pre-projection entry. Multi-thread flavor on purpose: the pre-repair
    /// guard was green only under current-thread scheduling.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_preserve_queued_during_removal_projection_observes_no_checkout() {
        let (be, rec, tmp, session, target, _bound) = v3_session("v3-projection-window").await;
        let be = Arc::new(be);
        let barrier = be.arm_removal_projection_barrier_for_test(&session);
        let checked = barrier.checked.notified();
        let mint_backend = be.clone();
        let mint_session = session.clone();
        let mint = tokio::spawn(async move {
            mint_backend
                .settle_workflow_checkout_v1(
                    &mint_session,
                    WorkflowCheckoutOutcomeV1::GloballyHealthy,
                )
                .await
        });
        checked.await;
        let preserve_backend = be.clone();
        let preserve_session = session.clone();
        let preserve = tokio::spawn(async move {
            preserve_backend
                .preserve_checkout_v1(&preserve_session, CheckoutPreservationReasonV1::NodeFailure)
                .await
        });
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        assert!(
            !preserve.is_finished(),
            "the preservation writer must stay blocked until the removal is projected"
        );
        barrier.proceed.notify_one();
        assert_eq!(mint.await.unwrap(), CheckoutSettlementV1::Removed);
        assert_eq!(
            preserve.await.unwrap(),
            CheckoutPreservationV1::NoCheckoutUnderCustody,
            "a writer queued behind a completed removal observes no checkout, never a stale refusal"
        );
        assert_eq!(record_state_of(&target).as_deref(), Some("removed"));
        assert_eq!(capability_removals(&rec), 1);
        drop(be);
        std::fs::remove_dir_all(&tmp).unwrap();
    }

    /// The shared deletion-admission guard is the linearization point for the preservation writer
    /// and the healthy settlement mint. These orders exercise that guard, not the publication cell.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn preservation_writer_and_healthy_settlement_linearize_in_both_orders() {
        let (be, rec, tmp, session, target, _bound) = v3_session("v3-linearize-writer-first").await;
        assert_eq!(
            be.preserve_checkout_v1(&session, CheckoutPreservationReasonV1::NodeFailure)
                .await,
            CheckoutPreservationV1::Preserved
        );
        let settled = be
            .settle_workflow_checkout_v1(&session, WorkflowCheckoutOutcomeV1::GloballyHealthy)
            .await;
        assert_eq!(
            settled,
            CheckoutSettlementV1::Preserved,
            "preservation writer wins when it raises first"
        );
        assert_eq!(record_state_of(&target).as_deref(), Some("preserved"));
        assert_eq!(capability_removals(&rec), 0);
        drop(be);
        std::fs::remove_dir_all(&tmp).unwrap();

        let (be, rec, tmp, session, target, _bound) =
            v3_session("v3-linearize-settlement-first").await;
        let be = Arc::new(be);
        let barrier = be.arm_deletion_admission_barrier_for_test(&session);
        let checked = barrier.checked.notified();
        let mint_backend = be.clone();
        let mint_session = session.clone();
        let mint = tokio::spawn(async move {
            mint_backend
                .settle_workflow_checkout_v1(
                    &mint_session,
                    WorkflowCheckoutOutcomeV1::GloballyHealthy,
                )
                .await
        });
        checked.await;
        let preserve_backend = be.clone();
        let preserve_session = session.clone();
        let preserve = tokio::spawn(async move {
            preserve_backend
                .preserve_checkout_v1(&preserve_session, CheckoutPreservationReasonV1::NodeFailure)
                .await
        });
        tokio::task::yield_now().await;
        assert!(
            !preserve.is_finished(),
            "the preservation writer is held at deletion admission"
        );
        barrier.proceed.notify_one();
        assert_eq!(
            mint.await.unwrap(),
            CheckoutSettlementV1::Removed,
            "the first admission owns the mint"
        );
        assert_eq!(
            preserve.await.unwrap(),
            CheckoutPreservationV1::NoCheckoutUnderCustody
        );
        assert_eq!(record_state_of(&target).as_deref(), Some("removed"));
        assert_eq!(capability_removals(&rec), 1);
        drop(be);
        std::fs::remove_dir_all(&tmp).unwrap();
    }

    /// must not join an in-flight `Reclaim` cleanup and be handed its report.
    ///
    /// The 2c1 test proved this for `Preserve`; with a third value in the enum the join key's
    /// disposition half has three ways to be wrong instead of one, and the epoch is what keeps
    /// equality from becoming accidental across generations.
    ///
    /// Discriminates a join key that dropped back to `(cell, strength)` — the pre-2c1 shape.
    #[tokio::test]
    async fn a_deletion_authority_request_never_joins_a_reclaim_flight() {
        let (be, rec, tmp, source, _cfg) = backend_fixture("v3-race-delete");
        let session = SessionId::parse("ctx-v3-race-delete-g0").unwrap();
        be.configure_session(&session, &spec(Some(&source.to_string_lossy())))
            .await
            .unwrap();

        let (reclaim_strength, _reclaim_report) = be
            .start_or_join_cleanup(&session, CleanupStrength::Release, false)
            .expect("a reclaim flight starts");
        assert_eq!(reclaim_strength, CleanupStrength::Release);
        be.raise_checkout_disposition(&session, CheckoutDispositionV1::DeleteAuthorized, None)
            .await
            .expect("the session has a cell");
        let authorized = be
            .start_or_join_cleanup(&session, CleanupStrength::Release, false)
            .expect("a deletion-authority flight starts");

        assert_eq!(
            be.cleanup_join_count(&session),
            0,
            "an equal-strength request of a DIFFERENT disposition must not join"
        );
        let report = wait_for_cleanup_report(authorized.1).await;
        assert!(report.is_ok());
        drop(rec);
        std::fs::remove_dir_all(&tmp).unwrap();
    }

    /// P5's epoch guard as a unit: a disposition raised AFTER a flight captured its generation
    /// makes that flight's authority stale, so the mint must not run.
    ///
    /// This is the window the per-session state mutex does not cover — `start_or_join_cleanup`
    /// reads the disposition synchronously and the flight then awaits the configure drain, the
    /// state mutex, and the inner teardown before it would mint. Comparing the EPOCH as well as
    /// the enum is what makes the check hold once a third disposition exists: after an eviction and
    /// a rebuild, enum equality alone can be true across two different generations.
    ///
    /// Discriminates a guard that compares only `disposition`.
    #[tokio::test]
    async fn a_disposition_raised_after_a_flight_started_makes_its_authority_stale() {
        let (be, rec, tmp, source, _cfg) = backend_fixture("v3-epoch-guard");
        let session = SessionId::parse("ctx-v3-epoch-guard-g0").unwrap();
        be.configure_session(&session, &spec(Some(&source.to_string_lossy())))
            .await
            .unwrap();
        let cell = be
            .raise_checkout_disposition(&session, CheckoutDispositionV1::DeleteAuthorized, None)
            .await
            .expect("the session has a cell");
        let captured_epoch = cell.lifecycle.lock().unwrap().disposition_epoch;
        assert!(WorktreeBackend::deletion_generation_is_current(
            &cell,
            CheckoutDispositionV1::DeleteAuthorized,
            captured_epoch
        ));

        be.raise_checkout_disposition(
            &session,
            CheckoutDispositionV1::Preserve,
            Some(PreservationReasonV1::Cancellation),
        )
        .await
        .expect("the session has a cell");

        assert!(
            !WorktreeBackend::deletion_generation_is_current(
                &cell,
                CheckoutDispositionV1::DeleteAuthorized,
                captured_epoch
            ),
            "a preservation raised after the flight started must invalidate its authority"
        );
        assert!(
            !WorktreeBackend::deletion_generation_is_current(
                &cell,
                CheckoutDispositionV1::Preserve,
                captured_epoch
            ),
            "and the EPOCH must discriminate, not just the enum"
        );
        drop(rec);
        std::fs::remove_dir_all(&tmp).unwrap();
    }

    /// P5, order 1 (evict-then-flight) — opus W3 made binding on this slice.
    ///
    /// The cleanup cell holding a session's `Preserve` disposition is EVICTED by the reporter the
    /// moment a flight reports `Ok`, which a gate refusal does. The next context-free teardown
    /// therefore starts from a fresh cell at `Reclaim`, and before this slice it published
    /// `worktree.teardown.retained` for a checkout whose record on disk says `preserved`.
    ///
    /// The assertion is on the OBSERVED teardown code, which is the only channel a caller outside
    /// `bridge-worktree` has (2c1 §4.1).
    ///
    /// Discriminates the label-only defect exactly: remove the durable re-derivation and the second
    /// teardown reports `retained`.
    #[tokio::test]
    async fn a_fresh_cell_after_eviction_re_derives_the_preserved_disposition_from_disk() {
        let (be, _rec, tmp, session, target, _bound) = v3_session("v3-cell-evicted").await;
        assert_eq!(
            be.preserve_checkout_v1(&session, CheckoutPreservationReasonV1::Cancellation)
                .await,
            CheckoutPreservationV1::Preserved
        );
        let first = Arc::new(CodeRec::default());
        be.release_session_observed(&session, first.clone())
            .await
            .unwrap();
        assert_eq!(
            be.cleanup_cell_count(),
            0,
            "the reporter evicts the cell on an Ok report — this is the precondition"
        );

        let second = Arc::new(CodeRec::default());
        be.release_session_observed(&session, second.clone())
            .await
            .unwrap();

        assert_eq!(record_state_of(&target).as_deref(), Some("preserved"));
        assert!(
            second
                .codes
                .lock()
                .unwrap()
                .iter()
                .any(|(_, code)| code == "worktree.teardown.preserved"),
            "a flight on a rebuilt cell must read the durable disposition, not default to \
             Retained: {:?}",
            second.codes.lock().unwrap()
        );
        std::fs::remove_dir_all(&tmp).unwrap();
    }

    /// P5, order 2 (no in-memory evidence at all) — the shape a process restart leaves behind.
    ///
    /// Here the cell is gone AND the map entry has neither the custody discriminator nor the
    /// retained identities, so the preservation barrier itself refuses (`not under R2f1b custody`)
    /// and there is no barrier outcome to label from. The record on disk is the ONLY authority
    /// left, and the report must still be `preserved`.
    ///
    /// Discriminates a re-derivation wired only into the barrier: the barrier cannot run here.
    #[tokio::test]
    async fn a_flight_with_no_in_memory_evidence_still_reports_the_durable_preservation() {
        let (be, _rec, tmp, session, target, _bound) = v3_session("v3-no-evidence").await;
        assert_eq!(
            be.preserve_checkout_v1(&session, CheckoutPreservationReasonV1::NodeFailure)
                .await,
            CheckoutPreservationV1::Preserved
        );
        // Everything in memory forgets that this checkout is under custody; the record does not.
        be.mark_entry_legacy_for_test(&session).await;

        let observer = Arc::new(CodeRec::default());
        be.release_session_observed(&session, observer.clone())
            .await
            .unwrap();

        assert_eq!(record_state_of(&target).as_deref(), Some("preserved"));
        assert!(Path::new(&target).exists());
        assert!(
            observer
                .codes
                .lock()
                .unwrap()
                .iter()
                .any(|(_, code)| code == "worktree.teardown.preserved"),
            "the record is the authoritative disposition source: {:?}",
            observer.codes.lock().unwrap()
        );
        std::fs::remove_dir_all(&tmp).unwrap();
    }

    /// The typed report composes with a capability removal (the brief's "also yours"): a real
    /// removal publishes the REAL terminal code, not the retained/preserved one.
    ///
    /// Without this, 2c1's typed disposition would have gained a fourth silent case — a genuine
    /// removal reported through the refusal vocabulary.
    #[tokio::test]
    async fn a_capability_removal_publishes_the_real_removed_teardown_code() {
        let (be, rec, tmp, session, _target, _bound) = v3_session("v3-removed-code").await;
        be.raise_checkout_disposition(&session, CheckoutDispositionV1::DeleteAuthorized, None)
            .await
            .expect("the session has a cell");

        let observer = Arc::new(CodeRec::default());
        be.release_session_observed(&session, observer.clone())
            .await
            .unwrap();

        assert_eq!(capability_removals(&rec), 1);
        let codes = observer.codes.lock().unwrap().clone();
        assert!(
            codes
                .iter()
                .any(|(status, code)| status == "Completed" && code == "worktree.teardown.released"),
            "a capability removal is a real removal and publishes the real code: {codes:?}"
        );
        assert!(
            !codes
                .iter()
                .any(|(_, code)| code == "worktree.teardown.retained"
                    || code == "worktree.teardown.preserved"),
            "and never a refusal code: {codes:?}"
        );
        std::fs::remove_dir_all(&tmp).unwrap();
    }

    /// P6 (BINDING): **disposition of gate-retained context-free deaths.**
    ///
    /// 2c1's ruling is that a context-free entry — a reaper, a `Drop`, controller retire — tears
    /// the session down with NO workflow outcome, so it gate-retains the checkout and mints no
    /// claim. 2c1's own handoff named the residual: those checkouts leak claimless if the post-loop
    /// mint slips. This is the pass that settles them.
    ///
    /// The setup is that exact shape: the node completed, the session was then torn down by a
    /// context-free release (which pops the reservation and re-inserts it as `Retained`), and the
    /// post-loop pass must still reach it — through the `Retained` map state, which is the entry's
    /// last in-memory owner. Both global outcomes are driven, on two separate checkouts.
    ///
    /// Discriminates a settlement that only looks at `WtState::Ready`, which would leave every
    /// context-free-torn-down checkout unsettled — the precise residual 2c1 §4.4 recorded.
    #[tokio::test]
    async fn a_gate_retained_context_free_death_is_settled_by_the_post_loop_pass() {
        // Arm A: the workflow failed. The claimless retained checkout finally gets its claim.
        let (be, rec, tmp, session, target, _bound) = v3_session("v3-p6-failed").await;
        be.release_session_checked(&session).await.unwrap();
        assert_eq!(
            record_state_of(&target).as_deref(),
            Some("live_protected"),
            "the context-free death gate-retained it with no claim (2c1's ruling)"
        );

        let settled = be
            .settle_workflow_checkout_v1(
                &session,
                WorkflowCheckoutOutcomeV1::NotHealthy(CheckoutPreservationReasonV1::Cancellation),
            )
            .await;

        assert_eq!(settled, CheckoutSettlementV1::Preserved);
        assert_eq!(record_state_of(&target).as_deref(), Some("preserved"));
        assert_eq!(removals(&rec), 0);
        std::fs::remove_dir_all(&tmp).unwrap();

        // Arm B: the workflow was globally healthy. The same claimless retained checkout is
        // removed under a capability instead.
        let (be, rec, tmp, session, target, _bound) = v3_session("v3-p6-healthy").await;
        be.release_session_checked(&session).await.unwrap();
        assert_eq!(record_state_of(&target).as_deref(), Some("live_protected"));

        let settled = be
            .settle_workflow_checkout_v1(&session, WorkflowCheckoutOutcomeV1::GloballyHealthy)
            .await;

        assert_eq!(settled, CheckoutSettlementV1::Removed);
        assert_eq!(capability_removals(&rec), 1);
        assert_eq!(removals(&rec), 0);
        assert!(!Path::new(&target).exists());
        assert!(
            be.mapped_worktree_path_for_test(&session).await.is_none(),
            "and the retained entry is cleared exactly once — no entry mapped forever"
        );
        std::fs::remove_dir_all(&tmp).unwrap();
    }

    /// Red-first mutation: omit the journal-boundary transfer branch; the durable-state assertion
    /// below fails as `settled` and the provider add count becomes one.
    #[tokio::test]
    async fn preparation_bound_at_journal_open_publish_sync_transfers_before_any_effect() {
        let (be, rec, tmp, source, cfg) = backend_fixture("preparation-bound-journal");
        let (bound, target) = bound_spec_v3(&source, &cfg);
        let clock = Arc::new(ManualPreparationClock::new(PREPARATION_ACTION_BOUND_MS));
        be.arm_preparation_bound_for_test(PreparationClockV1::new(clock));
        let session = SessionId::parse("preparation-bound-journal").unwrap();

        let error = be
            .configure_bound_session(&session, &bound)
            .await
            .unwrap_err();

        assert!(matches!(error, BridgeError::ConfigInvalid { .. }));
        assert_eq!(
            preparation_flight_state_of(&target).as_deref(),
            Some("transferred")
        );
        assert_eq!(record_state_of(&target), None);
        assert_eq!(rec.add_count.load(Ordering::SeqCst), 0);
        let recovery = be
            .transferred_preparation_for_test(&session)
            .expect("the transferred owner is inventoriable");
        assert_eq!(
            recovery.operation,
            PreparationOperationV1::JournalOpenPublish
        );
        assert!(recovery
            .reason
            .as_str()
            .contains("journal_open_publish_sync"));
        assert!(recovery
            .owner
            .runner
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .is_some());
        std::fs::remove_dir_all(tmp).unwrap();
    }

    /// Red-first mutation: remove `transfer_preparation_flight`; the durable state remains Open,
    /// the recovery inventory is empty, and this configure cannot terminalize while custody stalls.
    #[tokio::test]
    async fn nonreturning_custody_sync_transfers_pre_effect_owner() {
        let (be, rec, tmp, source, cfg) = backend_fixture("preparation-nonreturning-custody");
        let (bound, target) = bound_spec_v3(&source, &cfg);
        let clock = Arc::new(ManualPreparationClock::new(0));
        be.arm_preparation_bound_for_test(PreparationClockV1::new(clock.clone()));
        let hooks = be.preparation_test_hooks.clone();
        hooks.arm_nonreturning_custody_sync();
        let session = SessionId::parse("preparation-nonreturning-custody").unwrap();
        let configure_be = be.clone();
        let configure_session = session.clone();
        let configure = tokio::spawn(async move {
            configure_be
                .configure_bound_session(&configure_session, &bound)
                .await
        });

        hooks.wait_for_custody_sync().await;
        let exact_guard = be
            .preparation_guard_for_test(&session)
            .expect("the active owner retains the exact guard");
        clock.set(PREPARATION_CONTROL_BOUND_MS);
        assert!(be.observe_preparation_bound_for_test(&session).await);
        assert_eq!(
            preparation_flight_state_of(&target).as_deref(),
            Some("transferred")
        );
        assert_eq!(record_state_of(&target), None);
        assert_eq!(rec.add_count.load(Ordering::SeqCst), 0);
        let recovery = be
            .transferred_preparation_for_test(&session)
            .expect("the stalled operation moved to recovery");
        assert!(Arc::ptr_eq(&exact_guard, &recovery.owner.flight));
        assert_eq!(
            recovery.operation,
            PreparationOperationV1::CustodyEntryPublish
        );
        assert!(recovery.owner.runner.lock().unwrap().is_some());

        let error = configure.await.unwrap().unwrap_err();
        assert!(matches!(error, BridgeError::ConfigInvalid { .. }));
        hooks.release_custody_sync();
        tokio::task::yield_now().await;
        std::fs::remove_dir_all(tmp).unwrap();
    }

    /// Red-first mutation: change the bound comparison to zero; the normal `Settled` assertion fails.
    #[tokio::test]
    async fn unadvanced_preparation_clock_settles_normally() {
        let (be, _rec, tmp, source, cfg) = backend_fixture("preparation-bound-unadvanced");
        let (bound, target) = bound_spec_v3(&source, &cfg);
        let clock = Arc::new(ManualPreparationClock::new(0));
        be.arm_preparation_bound_for_test(PreparationClockV1::new(clock));
        let session = SessionId::parse("preparation-bound-unadvanced").unwrap();
        be.configure_bound_session(&session, &bound).await.unwrap();
        assert_eq!(
            preparation_flight_state_of(&target).as_deref(),
            Some("settled")
        );
        assert!(be.transferred_preparation_for_test(&session).is_none());
        std::fs::remove_dir_all(tmp).unwrap();
    }

    /// Red-first mutation: remove the prepared-barrier guard; this transfers after custody is durable.
    #[tokio::test]
    async fn advanced_clock_after_prepared_barrier_does_not_transfer() {
        let tmp = unique_temp_dir("preparation-post-barrier");
        let (be, _rec, source, cfg, provider) = blocking_custody_fixture(&tmp);
        let (bound, target) = bound_spec_v3(&source, &cfg);
        let clock = Arc::new(ManualPreparationClock::new(0));
        be.arm_preparation_bound_for_test(PreparationClockV1::new(clock.clone()));
        let session = SessionId::parse("preparation-post-barrier").unwrap();
        let configure_be = be.clone();
        let configure_session = session.clone();
        let configure = tokio::spawn(async move {
            configure_be
                .configure_bound_session(&configure_session, &bound)
                .await
        });
        provider.entered.notified().await;
        assert_eq!(
            preparation_flight_state_of(&target).as_deref(),
            Some("barrier_synced")
        );
        clock.set(PREPARATION_CONTROL_BOUND_MS);
        assert!(!be.observe_preparation_bound_for_test(&session).await);
        provider.release.notify_one();
        configure.await.unwrap().unwrap();
        assert_eq!(
            preparation_flight_state_of(&target).as_deref(),
            Some("settled")
        );
        std::fs::remove_dir_all(tmp).unwrap();
    }

    /// Red-first mutation: arm a production bound in `WorktreeBackend::new`; this slow control transfers.
    #[tokio::test]
    async fn production_default_does_not_arm_a_preparation_bound() {
        let tmp = unique_temp_dir("preparation-production-default");
        let (be, _rec, source, cfg, provider) = blocking_custody_fixture(&tmp);
        let (bound, target) = bound_spec_v3(&source, &cfg);
        let session = SessionId::parse("preparation-production-default").unwrap();
        let configure_be = be.clone();
        let configure_session = session.clone();
        let configure = tokio::spawn(async move {
            configure_be
                .configure_bound_session(&configure_session, &bound)
                .await
        });
        provider.entered.notified().await;
        assert!(!be.observe_preparation_bound_for_test(&session).await);
        provider.release.notify_one();
        configure.await.unwrap().unwrap();
        assert_eq!(
            preparation_flight_state_of(&target).as_deref(),
            Some("settled")
        );
        std::fs::remove_dir_all(tmp).unwrap();
    }

    /// W1, both deterministic schedules: one CAS phase owns either transfer or barrier
    /// publication. A released transfer loser never adds, and neither schedule can publish two
    /// preparation terminals.
    #[tokio::test]
    async fn preparation_phase_linearizes_transfer_and_barrier_races() {
        let (be, rec, tmp, source, cfg) = backend_fixture("preparation-phase-transfer-wins");
        let (bound, target) = bound_spec_v3(&source, &cfg);
        let clock = Arc::new(ManualPreparationClock::new(0));
        be.arm_preparation_bound_for_test(PreparationClockV1::new(clock.clone()));
        let hooks = be.preparation_test_hooks.clone();
        hooks.arm_nonreturning_custody_sync();
        let session = SessionId::parse("preparation-phase-transfer-wins").unwrap();
        let configure_be = be.clone();
        let configure_session = session.clone();
        let configure = tokio::spawn(async move {
            configure_be
                .configure_bound_session(&configure_session, &bound)
                .await
        });

        hooks.wait_for_custody_sync().await;
        clock.set(PREPARATION_CONTROL_BOUND_MS);
        assert!(be.observe_preparation_bound_for_test(&session).await);
        assert!(matches!(
            configure.await.unwrap(),
            Err(BridgeError::ConfigInvalid { .. })
        ));
        hooks.release_custody_sync();
        be.join_transferred_preparation_runner_for_test(&session)
            .await;
        assert_eq!(
            preparation_flight_state_of(&target).as_deref(),
            Some("transferred")
        );
        assert_eq!(hooks.terminal_count.load(Ordering::SeqCst), 1);
        assert_eq!(rec.add_count.load(Ordering::SeqCst), 0);
        std::fs::remove_dir_all(tmp).unwrap();

        let tmp = unique_temp_dir("preparation-phase-barrier-wins");
        let (be, rec, source, cfg, provider) = blocking_custody_fixture(&tmp);
        let (bound, target) = bound_spec_v3(&source, &cfg);
        let clock = Arc::new(ManualPreparationClock::new(0));
        be.arm_preparation_bound_for_test(PreparationClockV1::new(clock.clone()));
        let session = SessionId::parse("preparation-phase-barrier-wins").unwrap();
        let configure_be = be.clone();
        let configure_session = session.clone();
        let configure = tokio::spawn(async move {
            configure_be
                .configure_bound_session(&configure_session, &bound)
                .await
        });

        provider.entered.notified().await;
        clock.set(PREPARATION_CONTROL_BOUND_MS);
        assert!(
            !be.observe_preparation_bound_for_test(&session).await,
            "the barrier phase owns admission before the transfer observer can publish"
        );
        provider.release.notify_one();
        configure.await.unwrap().unwrap();
        assert_eq!(
            preparation_flight_state_of(&target).as_deref(),
            Some("settled")
        );
        assert_eq!(
            be.preparation_test_hooks
                .terminal_count
                .load(Ordering::SeqCst),
            1
        );
        assert_eq!(rec.add_count.load(Ordering::SeqCst), 1);
        std::fs::remove_dir_all(tmp).unwrap();
    }

    /// W2: the sole post-return sample sees a custody operation cross the action bound without a
    /// cfg(test) observer. It transfers before barrier publication and provider admission.
    #[tokio::test]
    async fn slow_returning_custody_operation_transfers_before_barrier_admission() {
        let (be, rec, tmp, source, cfg) = backend_fixture("preparation-slow-custody-return");
        let (bound, target) = bound_spec_v3(&source, &cfg);
        let clock = Arc::new(ManualPreparationClock::new(
            PREPARATION_ACTION_BOUND_MS - 100,
        ));
        be.arm_preparation_bound_for_test(PreparationClockV1::new(clock.clone()));
        let hooks = be.preparation_test_hooks.clone();
        hooks.arm_nonreturning_custody_sync();
        let session = SessionId::parse("preparation-slow-custody-return").unwrap();
        let configure_be = be.clone();
        let configure_session = session.clone();
        let configure = tokio::spawn(async move {
            configure_be
                .configure_bound_session(&configure_session, &bound)
                .await
        });

        hooks.wait_for_custody_sync().await;
        clock.set(PREPARATION_ACTION_BOUND_MS + 100);
        hooks.release_custody_sync();
        assert!(matches!(
            configure.await.unwrap(),
            Err(BridgeError::ConfigInvalid { .. })
        ));
        be.join_transferred_preparation_runner_for_test(&session)
            .await;
        assert_eq!(
            preparation_flight_state_of(&target).as_deref(),
            Some("transferred")
        );
        assert_eq!(rec.add_count.load(Ordering::SeqCst), 0);
        std::fs::remove_dir_all(tmp).unwrap();
    }

    /// W3: the control journal exists before initial Open. Transfer can therefore terminalize a
    /// blocked initial publication and leave the exact blocked runner in recovery ownership.
    #[tokio::test]
    async fn blocked_initial_open_transfers_through_control_journal() {
        let (be, rec, tmp, source, cfg) = backend_fixture("preparation-control-journal");
        let (bound, target) = bound_spec_v3(&source, &cfg);
        let clock = Arc::new(ManualPreparationClock::new(0));
        be.arm_preparation_bound_for_test(PreparationClockV1::new(clock.clone()));
        let hooks = be.preparation_test_hooks.clone();
        hooks.arm_nonreturning_initial_open_publish();
        let session = SessionId::parse("preparation-control-journal").unwrap();
        let configure_be = be.clone();
        let configure_session = session.clone();
        let configure = tokio::spawn(async move {
            configure_be
                .configure_bound_session(&configure_session, &bound)
                .await
        });

        hooks.wait_for_initial_open_publish().await;
        let exact_flight = be
            .preparation_guard_for_test(&session)
            .expect("the active owner retains the blocked runner");
        clock.set(PREPARATION_CONTROL_BOUND_MS);
        assert!(be.observe_preparation_bound_for_test(&session).await);
        assert!(matches!(
            configure.await.unwrap(),
            Err(BridgeError::ConfigInvalid { .. })
        ));
        let recovery = be
            .transferred_preparation_for_test(&session)
            .expect("transfer is recovery-owned before configure observes its result");
        assert!(Arc::ptr_eq(&exact_flight, &recovery.owner.flight));
        assert!(recovery.owner.runner.lock().unwrap().is_some());
        hooks.release_initial_open_publish();
        be.join_transferred_preparation_runner_for_test(&session)
            .await;
        assert_eq!(
            preparation_flight_state_of(&target).as_deref(),
            Some("transferred")
        );
        assert_eq!(rec.add_count.load(Ordering::SeqCst), 0);
        std::fs::remove_dir_all(tmp).unwrap();
    }

    /// W4: pausing immediately after result publication proves recovery is visible before the
    /// configure waiter wakes; the old completion-first ordering exposed an empty inventory here.
    #[tokio::test]
    async fn transfer_registers_recovery_before_publishing_configure_result() {
        let (be, _rec, tmp, source, cfg) = backend_fixture("preparation-recovery-before-result");
        let (bound, _target) = bound_spec_v3(&source, &cfg);
        let clock = Arc::new(ManualPreparationClock::new(0));
        be.arm_preparation_bound_for_test(PreparationClockV1::new(clock.clone()));
        let hooks = be.preparation_test_hooks.clone();
        hooks.arm_nonreturning_initial_open_publish();
        hooks
            .pause_after_result_publication
            .store(true, Ordering::SeqCst);
        let session = SessionId::parse("preparation-recovery-before-result").unwrap();
        let configure_be = be.clone();
        let configure_session = session.clone();
        let configure = tokio::spawn(async move {
            configure_be
                .configure_bound_session(&configure_session, &bound)
                .await
        });

        hooks.wait_for_initial_open_publish().await;
        clock.set(PREPARATION_CONTROL_BOUND_MS);
        let observer_be = be.clone();
        let observer_session = session.clone();
        let observer = tokio::spawn(async move {
            observer_be
                .observe_preparation_bound_for_test(&observer_session)
                .await
        });
        hooks.wait_for_result_publication().await;
        assert!(matches!(
            configure.await.unwrap(),
            Err(BridgeError::ConfigInvalid { .. })
        ));
        assert!(
            be.transferred_preparation_for_test(&session).is_some(),
            "the result is visible only after the exact owner reached recovery"
        );
        hooks.release_after_result_publication.notify_one();
        assert!(observer.await.unwrap());
        hooks.release_initial_open_publish();
        be.join_transferred_preparation_runner_for_test(&session)
            .await;
        std::fs::remove_dir_all(tmp).unwrap();
    }

    #[test]
    fn preparation_transfer_and_failure_claims_have_one_winner_in_both_orders() {
        for _failure_source in ["caller_departure", "custody_error", "runner_exit"] {
            for failure_first in [true, false] {
                let hooks = Arc::new(PreparationFlightTestHooks::default());
                let flight = MaterializationPreparationFlightV1::claim(hooks, None).unwrap();
                if failure_first {
                    assert!(flight.begin_failure_publication());
                    assert!(!flight.begin_transfer());
                    assert_eq!(
                        flight.phase(),
                        PreparationPublicationPhaseV1::FailurePublishing
                    );
                } else {
                    assert!(flight.begin_transfer());
                    assert!(!flight.begin_failure_publication());
                    assert_eq!(
                        flight.phase(),
                        PreparationPublicationPhaseV1::TransferPublishing
                    );
                }
            }
        }
    }

    #[tokio::test]
    async fn failure_owned_runner_exit_completes_configure_result() {
        let tmp = unique_temp_dir("preparation-failure-exit-result");
        let target = tmp.join("target").to_string_lossy().into_owned();
        let hooks = Arc::new(PreparationFlightTestHooks::default());
        let control = Arc::new(PreparationControlRootV1::new(tmp.clone(), hooks.clone()));
        assert!(control.begin_pin_after_owner_published());
        control.open_claimed_for_session_admission().unwrap();
        let flight = Arc::new(MaterializationPreparationFlightV1::claim(hooks, None).unwrap());
        let journal = Arc::new(
            PreparationFlightJournalV1::new(control, &target, flight.id().clone()).unwrap(),
        );
        flight.set_journal(journal.clone());
        journal
            .publish(PreparationFlightStateV1::Open {}, true)
            .unwrap();
        assert!(flight.begin_failure_publication());
        let owner = Arc::new(ActivePreparationFlightV1::new(flight));
        let (result_tx, result_rx) = oneshot::channel();
        owner.install_result(result_tx);
        let session = "preparation-failure-exit-result".to_owned();
        let flights = Arc::new(StdMutex::new(HashMap::from([(
            session.clone(),
            owner.clone(),
        )])));
        let (exit_tx, exit_rx) = oneshot::channel();
        let terminalizer = tokio::spawn({
            let flights = flights.clone();
            let owner = owner.clone();
            async move {
                if exit_rx.await.is_ok() {
                    terminalize_preparation_runner_exit(flights, session, owner).await;
                }
            }
        });
        drop(PreparationRunnerExitGuardV1::new(exit_tx));
        assert!(matches!(
            result_rx.await.unwrap(),
            Err(BridgeError::AgentCrashed { .. })
        ));
        terminalizer.await.unwrap();
        assert_eq!(
            preparation_flight_state_of(&target).as_deref(),
            Some("failed")
        );
        std::fs::remove_dir_all(tmp).unwrap();
    }

    #[tokio::test]
    async fn stalled_control_root_pin_is_observable_before_terminalization() {
        let (be, rec, tmp, source, cfg) = backend_fixture("preparation-stalled-control-root");
        let (bound, target) = bound_spec_v3(&source, &cfg);
        let clock = Arc::new(ManualPreparationClock::new(0));
        be.arm_preparation_bound_for_test(PreparationClockV1::new(clock.clone()));
        let hooks = be.preparation_test_hooks.clone();
        hooks.arm_nonreturning_control_root_pin();
        let session = SessionId::parse("preparation-stalled-control-root").unwrap();
        let configure_be = be.clone();
        let configure_session = session.clone();
        let configure = tokio::spawn(async move {
            configure_be
                .configure_bound_session(&configure_session, &bound)
                .await
        });

        hooks.wait_for_control_root_pin().await;
        let exact_flight = be
            .preparation_guard_for_test(&session)
            .expect("the stalled root pin has an active exact owner");
        clock.set(PREPARATION_CONTROL_BOUND_MS);
        let observer_be = be.clone();
        let observer_session = session.clone();
        let observer = tokio::spawn(async move {
            observer_be
                .observe_preparation_bound_for_test(&observer_session)
                .await
        });
        for _ in 0..8 {
            if exact_flight.phase() == PreparationPublicationPhaseV1::TransferPublishing {
                break;
            }
            tokio::task::yield_now().await;
        }
        assert_eq!(
            exact_flight.phase(),
            PreparationPublicationPhaseV1::TransferPublishing,
            "the observer claims before root pinning can publish"
        );
        assert!(observer.await.unwrap());
        assert!(matches!(
            configure.await.unwrap(),
            Err(BridgeError::ConfigInvalid { .. })
        ));
        hooks.release_control_root_pin();
        hooks.wait_for_terminal().await;
        let recovery = be
            .transferred_preparation_for_test(&session)
            .expect("transfer retains the owner that was visible during root pinning");
        assert!(Arc::ptr_eq(&exact_flight, &recovery.owner.flight));
        be.join_transferred_preparation_runner_for_test(&session)
            .await;
        assert_eq!(
            preparation_flight_state_of(&target).as_deref(),
            Some("transferred")
        );
        assert_eq!(rec.add_count.load(Ordering::SeqCst), 0);
        std::fs::remove_dir_all(tmp).unwrap();
    }

    #[tokio::test]
    async fn terminal_replacement_serializes_exact_open_writers() {
        let tmp = unique_temp_dir("preparation-terminal-replacement");
        let target = tmp.join("target").to_string_lossy().into_owned();
        let hooks = Arc::new(PreparationFlightTestHooks::default());
        let control = Arc::new(PreparationControlRootV1::new(tmp.clone(), hooks.clone()));
        assert!(control.begin_pin_after_owner_published());
        control.open_claimed_for_session_admission().unwrap();
        let flight = MaterializationPreparationFlightV1::claim(hooks.clone(), None).unwrap();
        let journal = Arc::new(
            PreparationFlightJournalV1::new(control, &target, flight.id().clone()).unwrap(),
        );
        journal
            .publish(PreparationFlightStateV1::Open {}, true)
            .unwrap();
        hooks.arm_nonreturning_initial_open_publish();
        let failure_journal = journal.clone();
        let failure = std::thread::spawn(move || {
            failure_journal.publish_terminal(preparation_failure_state())
        });
        hooks.wait_for_initial_open_publish().await;
        let transferred = PreparationFlightStateV1::Transferred {
            reason: BoundedPreparationTransferReasonV1::new("terminal replacement race").unwrap(),
        };
        assert_eq!(
            journal.publish_terminal(transferred),
            Err(BridgeError::StoreFailure),
            "the second writer cannot replace Open while the first lease is held"
        );
        hooks.release_initial_open_publish();
        assert_eq!(failure.join().unwrap(), Ok(()));
        assert_eq!(
            preparation_flight_state_of(&target).as_deref(),
            Some("failed")
        );
        std::fs::remove_dir_all(tmp).unwrap();
    }

    #[test]
    fn preparation_control_root_refuses_identity_replacement() {
        let tmp = unique_temp_dir("preparation-control-root-replacement");
        let root = tmp.join("root");
        let former = tmp.join("former-root");
        let replacement = tmp.join("replacement-root");
        std::fs::create_dir(&root).unwrap();
        std::fs::create_dir(&replacement).unwrap();
        let hooks = Arc::new(PreparationFlightTestHooks::default());
        let control = PreparationControlRootV1::new(root.clone(), hooks);
        assert!(control.begin_pin_after_owner_published());
        control.open_claimed_for_session_admission().unwrap();
        std::fs::rename(&root, &former).unwrap();
        std::fs::rename(&replacement, &root).unwrap();
        assert!(matches!(
            control.pinned_root(),
            Err(BridgeError::StoreFailure)
        ));
        std::fs::remove_dir_all(tmp).unwrap();
    }

    /// s1: a provider panic no longer leaves configure waiting forever behind the active owner's
    /// sender; the exit guard records a terminal failure and returns a typed crash.
    #[tokio::test]
    async fn panicking_provider_terminalizes_the_preparation_runner() {
        let tmp = unique_temp_dir("preparation-panicking-provider");
        let (be, rec, source, cfg) =
            provider_fixture(&tmp, |rec| Arc::new(PanickingCustodyAddProv { rec }));
        let (bound, target) = bound_spec_v3(&source, &cfg);
        let session = SessionId::parse("preparation-panicking-provider").unwrap();
        let error = tokio::time::timeout(
            Duration::from_secs(2),
            be.configure_bound_session(&session, &bound),
        )
        .await
        .expect("runner exit guard must wake configure")
        .unwrap_err();

        assert!(matches!(error, BridgeError::AgentCrashed { .. }));
        assert_eq!(
            preparation_flight_state_of(&target).as_deref(),
            Some("failed")
        );
        assert_eq!(rec.add_count.load(Ordering::SeqCst), 1);
        std::fs::remove_dir_all(tmp).unwrap();
    }

    /// s2: a failed Transferred publication is typed active debt, never a recovery success or a
    /// provider effect. Removing this failure branch turns the retained debt into a false claim.
    #[tokio::test]
    async fn transfer_terminal_publication_failure_retains_active_debt_without_recovery() {
        let (be, rec, tmp, source, cfg) =
            backend_fixture("preparation-transfer-publication-failure");
        let (bound, target) = bound_spec_v3(&source, &cfg);
        let clock = Arc::new(ManualPreparationClock::new(PREPARATION_ACTION_BOUND_MS));
        be.arm_preparation_bound_for_test(PreparationClockV1::new(clock));
        be.preparation_test_hooks
            .fail_terminal_publication
            .store(true, Ordering::SeqCst);
        let session = SessionId::parse("preparation-transfer-publication-failure").unwrap();

        assert_eq!(
            be.configure_bound_session(&session, &bound).await,
            Err(BridgeError::StoreFailure)
        );
        assert_eq!(
            preparation_flight_state_of(&target).as_deref(),
            Some("open")
        );
        assert_eq!(
            be.preparation_flight_debt_for_test(&session),
            Some(BridgeError::StoreFailure)
        );
        assert!(be.transferred_preparation_for_test(&session).is_none());
        assert_eq!(rec.add_count.load(Ordering::SeqCst), 0);
        std::fs::remove_dir_all(tmp).unwrap();
    }
}
