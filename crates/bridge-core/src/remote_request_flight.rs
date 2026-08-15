use crate::{
    execution_policy::Sha256HexV1,
    fs_custody::{
        open_regular_child, required_file_content_snapshot_v2, ChildNameV2, FileContentSnapshotV2,
        FsCustodyError, JournalMutationOutcomeV2, JournalRootCustodyV2, JournalRootOperationV2,
        ReservedNameNamespaceV2,
    },
    ids::{AttemptId, AttemptIdentity, ExecutionId},
    liveness::PersistentLockGuard,
    namespace_transaction::{NamespaceTransactionOutcomeV2, NamespaceTransactionV2},
    resource_flight::{
        DedicatedRemoteRequestIdV1, ResourceActionDispositionV1, ResourceActionResultV1,
    },
    retained_resource_flight::ResourceFlightOwnerV1,
};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::{
    collections::BTreeSet,
    future::Future,
    ops::Deref,
    pin::Pin,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc, Condvar, Mutex, MutexGuard,
    },
    task::{Context, Poll},
};
const SCHEMA: u8 = 1;
const CAPACITY: usize = 4096;
const ADMISSION_FOOTPRINT: usize = 4;
const WIRE_CAP: usize = 4096;
const CHECKPOINT_CHILD_V1: &str = "remote-request-checkpoint.json";
const ATTEMPT_LEASE_CHILD_V1: &str = "remote-request-attempt.lock";
const REQUEST_CHILD_PREFIX_V1: &str = "remote-request-authority-";
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TaskAProtectiveOutcomeV1 {
    Refused,
    Retained,
    Unknown,
    Unsupported,
    ProtectiveDebt,
}
#[derive(Clone, Debug, thiserror::Error, PartialEq, Eq)]
pub enum RemoteRequestFlightRefusalV1 {
    #[error("request journal capacity is exhausted")]
    Capacity,
    #[error("legacy request journal migration is required")]
    LegacyMigrationRequired,
    #[error("malformed request journal: {0}")]
    Malformed(String),
    #[error("foreign request schema: {0}")]
    ForeignSchema(&'static str),
    #[error("foreign attempt identity")]
    ForeignAttempt,
    #[error("request journal digest mismatch: {0}")]
    DigestMismatch(&'static str),
    #[error("request identity or ordinal collision")]
    IdentityCollision,
    #[error("request ordinal overflow")]
    OrdinalOverflow,
    #[error("request identity unavailable: {0}")]
    IdentityUnavailable(String),
    #[error("only a complete terminal outcome may be acknowledged")]
    TerminalNotComplete,
    #[error("request child state refuses transition: {0}")]
    InvalidStateTransition(&'static str),
    #[error("Task A {0:?}: {1}")]
    TaskA(TaskAProtectiveOutcomeV1, String),
    #[error("request journal requires B2 recovery: {0}")]
    ReopenRequired(&'static str),
    #[error("request attempt is already live")]
    AttemptLive,
    #[error("terminal publication was refused: {0}")]
    PublicationRefused(String),
    #[error("terminal publication acknowledgement did not echo the delivery identity")]
    PublicationAcknowledgementMismatch,
    #[error("attempt admission mutex is poisoned")]
    AdmissionMutexPoisoned,
    #[error("owned request journal mutex is poisoned")]
    RequestMutexPoisoned,
    #[error("terminal observation deadline elapsed")]
    ObservationTimedOut,
    #[error("terminal observation closed without an outcome")]
    ObservationClosed,
}
type Refusal = RemoteRequestFlightRefusalV1;
type FlightResult<T> = std::result::Result<T, Refusal>;
#[derive(Serialize, Deserialize)]
#[serde(remote = "AttemptIdentity", deny_unknown_fields)]
struct AttemptIdentityWireV1 {
    execution_id: ExecutionId,
    attempt_id: AttemptId,
    ordinal: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    parent_attempt_id: Option<AttemptId>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
enum ChildStateV1 {
    #[serde(rename = "reserved")]
    Active {},
    PreSendFailure {},
    IntentJournaled {},
    DispatchAuthorized {},
    ProviderSendArmed {},
    TerminalPendingPublication {
        result: ResourceActionResultV1,
        prompt_may_have_been_accepted: bool,
    },
    PublicationAcknowledged {
        delivery_id: RemoteRequestDeliveryIdV1,
    },
}
impl ChildStateV1 {
    fn is_terminal_pending(&self) -> bool {
        matches!(self, Self::TerminalPendingPublication { .. })
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RemoteRequestDeliveryIdV1 {
    #[serde(with = "AttemptIdentityWireV1")]
    attempt: AttemptIdentity,
    ordinal: u64,
    request_id: DedicatedRemoteRequestIdV1,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RemoteRequestTerminalPublicationV1 {
    delivery_id: RemoteRequestDeliveryIdV1,
    result: ResourceActionResultV1,
    prompt_may_have_been_accepted: bool,
}
impl RemoteRequestTerminalPublicationV1 {
    #[must_use]
    pub fn delivery_id(&self) -> &RemoteRequestDeliveryIdV1 {
        &self.delivery_id
    }
    #[must_use]
    pub fn result(&self) -> &ResourceActionResultV1 {
        &self.result
    }
    #[must_use]
    pub fn prompt_may_have_been_accepted(&self) -> bool {
        self.prompt_may_have_been_accepted
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RemoteRequestTerminalOutcomeV1 {
    publication: RemoteRequestTerminalPublicationV1,
}
impl RemoteRequestTerminalOutcomeV1 {
    #[must_use]
    pub fn delivery_id(&self) -> &RemoteRequestDeliveryIdV1 {
        self.publication.delivery_id()
    }
    #[must_use]
    pub fn result(&self) -> &ResourceActionResultV1 {
        self.publication.result()
    }
    #[must_use]
    pub fn prompt_may_have_been_accepted(&self) -> bool {
        self.publication.prompt_may_have_been_accepted()
    }
}
impl From<RemoteRequestTerminalPublicationV1> for RemoteRequestTerminalOutcomeV1 {
    fn from(publication: RemoteRequestTerminalPublicationV1) -> Self {
        Self { publication }
    }
}
/// Durable consumers must deduplicate on the complete delivery id before
/// acknowledging it. Recovery may invoke this method again after a crash;
/// exactly-once applies to the sink effect, not to callback count.
pub trait RemoteRequestResultPublisherV1: Send + Sync {
    fn publish_idempotent(
        &self,
        publication: &RemoteRequestTerminalPublicationV1,
    ) -> Result<RemoteRequestDeliveryIdV1, String>;
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CheckpointWireV1 {
    schema: u8,
    #[serde(with = "AttemptIdentityWireV1")]
    attempt: AttemptIdentity,
    next_ordinal: u64,
    identity_chain_digest: Sha256HexV1,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RequestChildWireV1 {
    schema: u8,
    #[serde(with = "AttemptIdentityWireV1")]
    attempt: AttemptIdentity,
    ordinal: u64,
    checkpoint_digest: Sha256HexV1,
    authority_digest: Sha256HexV1,
    request_id: DedicatedRemoteRequestIdV1,
    owner: ResourceFlightOwnerV1,
    status: ChildStateV1,
}
struct CensusChildV1 {
    wire: RequestChildWireV1,
    snapshot: FileContentSnapshotV2,
}
impl Deref for CensusChildV1 {
    type Target = RequestChildWireV1;
    fn deref(&self) -> &Self::Target {
        &self.wire
    }
}
struct CensusV1 {
    checkpoint: CheckpointWireV1,
    checkpoint_snapshot: FileContentSnapshotV2,
    children: Vec<CensusChildV1>,
    staged: bool,
}
pub struct RemoteRequestAuthorityV1 {
    attempt: AttemptIdentity,
    ordinal: u64,
    request_id: DedicatedRemoteRequestIdV1,
}
impl RemoteRequestAuthorityV1 {
    #[must_use]
    pub fn request_id(&self) -> &DedicatedRemoteRequestIdV1 {
        &self.request_id
    }
    fn delivery_id(&self) -> RemoteRequestDeliveryIdV1 {
        RemoteRequestDeliveryIdV1 {
            attempt: self.attempt.clone(),
            ordinal: self.ordinal,
            request_id: self.request_id.clone(),
        }
    }
}
pub struct RemoteRequestJournalV1 {
    custody: JournalRootCustodyV2,
    attempt: AttemptIdentity,
    capacity: usize,
    requires_reopen: bool,
    publisher: Arc<dyn RemoteRequestResultPublisherV1>,
    _attempt_lease: PersistentLockGuard,
    admission_mutex: Mutex<()>,
}
pub struct RemoteRequestDriverV1 {
    journal: Arc<Mutex<RemoteRequestJournalV1>>,
}
pub struct OwnedRemoteRequestV1 {
    journal: Arc<Mutex<RemoteRequestJournalV1>>,
    authority: RemoteRequestAuthorityV1,
    outcome_tx: tokio::sync::watch::Sender<Option<RemoteRequestTerminalOutcomeV1>>,
    live_waiters: Arc<AtomicUsize>,
    provider_send_claimed: AtomicBool,
    provider_send_armed: AtomicBool,
    settlement_attempted: AtomicBool,
    publication_flight: Mutex<PublicationFlightStateV1>,
    publication_settled: Condvar,
}
pub struct RemoteRequestObserverV1 {
    outcome_rx: tokio::sync::watch::Receiver<Option<RemoteRequestTerminalOutcomeV1>>,
    live_waiters: Arc<AtomicUsize>,
}
pub struct ArmedProviderSendV1<'a, F> {
    request: &'a OwnedRemoteRequestV1,
    inner: Option<Pin<Box<F>>>,
    arm_attempted: bool,
}
#[derive(Clone)]
enum PublicationFlightStateV1 {
    Idle,
    Driving,
    Finished(FlightResult<()>),
}
struct PublicationDriverGuardV1<'a> {
    request: &'a OwnedRemoteRequestV1,
    armed: bool,
}
impl<'a> PublicationDriverGuardV1<'a> {
    fn new(request: &'a OwnedRemoteRequestV1) -> Self {
        Self {
            request,
            armed: true,
        }
    }
    fn finish(mut self, result: FlightResult<()>) -> FlightResult<()> {
        let result = self.request.finish_publication_flight(result);
        self.armed = false;
        result
    }
}
impl Drop for PublicationDriverGuardV1<'_> {
    fn drop(&mut self) {
        if self.armed {
            let _ = self
                .request
                .finish_publication_flight(Err(Refusal::PublicationRefused(
                    "publication driver unwound".into(),
                )));
        }
    }
}
struct LiveWaiterGuardV1 {
    live_waiters: Arc<AtomicUsize>,
}
impl Drop for LiveWaiterGuardV1 {
    fn drop(&mut self) {
        self.live_waiters.fetch_sub(1, Ordering::AcqRel);
    }
}
fn failed_before_send() -> ResourceActionResultV1 {
    ResourceActionResultV1 {
        disposition: ResourceActionDispositionV1::Failed,
        duration_ms: 0,
        recovery_owner: None,
        cause: None,
    }
}
fn unknown_after_arm() -> ResourceActionResultV1 {
    ResourceActionResultV1 {
        disposition: ResourceActionDispositionV1::Unknown,
        ..failed_before_send()
    }
}
fn delivery_id(wire: &RequestChildWireV1) -> RemoteRequestDeliveryIdV1 {
    RemoteRequestDeliveryIdV1 {
        attempt: wire.attempt.clone(),
        ordinal: wire.ordinal,
        request_id: wire.request_id.clone(),
    }
}
fn hash<T: Serialize>(value: &T) -> Sha256HexV1 {
    Sha256HexV1::digest(&serde_json::to_vec(value).expect("journal digest input serializes"))
}
fn checkpoint_digest(attempt: &AttemptIdentity, ordinal: u64) -> Sha256HexV1 {
    hash(&("a2a.remote-request.checkpoint.v1", attempt, ordinal))
}
fn checkpoint(attempt: &AttemptIdentity, next_ordinal: u64) -> CheckpointWireV1 {
    CheckpointWireV1 {
        schema: SCHEMA,
        attempt: attempt.clone(),
        next_ordinal,
        identity_chain_digest: checkpoint_digest(attempt, next_ordinal),
    }
}
fn authority_digest(wire: &RequestChildWireV1) -> Sha256HexV1 {
    hash(&(
        "a2a.remote-request.authority.v1",
        &wire.attempt,
        wire.ordinal,
        &wire.checkpoint_digest,
        &wire.request_id,
        &wire.owner,
    ))
}
fn request_name(digest: &Sha256HexV1) -> ChildNameV2 {
    ChildNameV2::from_bytes(format!("{REQUEST_CHILD_PREFIX_V1}{}.json", digest.as_str()).as_bytes())
        .expect("digest child name is portable")
}
fn protective(kind: TaskAProtectiveOutcomeV1, reason: impl ToString) -> Refusal {
    Refusal::TaskA(kind, reason.to_string())
}
fn fs(error: FsCustodyError) -> Refusal {
    let kind = match error {
        FsCustodyError::Unsupported(_) => TaskAProtectiveOutcomeV1::Unsupported,
        _ => TaskAProtectiveOutcomeV1::Unknown,
    };
    protective(kind, error)
}
fn journal(outcome: JournalMutationOutcomeV2) -> Refusal {
    use JournalMutationOutcomeV2 as J;
    use TaskAProtectiveOutcomeV1 as P;
    match outcome {
        J::Refused(reason) => protective(P::Refused, reason),
        J::Retained(reason) => protective(P::Retained, reason),
        J::Unsupported(reason) => protective(P::Unsupported, reason),
        J::ProtectiveDebt(reason) => protective(P::ProtectiveDebt, reason),
        J::Complete => protective(P::Unknown, "invalid complete error"),
    }
}
fn mutation<T>(result: Result<T, JournalMutationOutcomeV2>) -> FlightResult<T> {
    result.map_err(journal)
}
fn sync(outcome: JournalMutationOutcomeV2) -> FlightResult<()> {
    match outcome {
        JournalMutationOutcomeV2::Complete => Ok(()),
        other => Err(journal(other)),
    }
}
fn transaction(outcome: NamespaceTransactionOutcomeV2) -> FlightResult<()> {
    use NamespaceTransactionOutcomeV2 as N;
    use TaskAProtectiveOutcomeV1 as P;
    match outcome {
        N::Complete(_) => Ok(()),
        N::Retained(_, reason) => Err(protective(P::Retained, reason)),
        N::ProtectiveDebt(reason) => Err(protective(P::ProtectiveDebt, reason)),
        N::Unsupported(reason) => Err(protective(P::Unsupported, reason)),
        N::NoEffect(_, reason) => Err(protective(P::Refused, reason)),
        N::Ready => Err(protective(P::Refused, "unexpected ready")),
    }
}
fn recovery(outcome: NamespaceTransactionOutcomeV2) -> FlightResult<()> {
    use NamespaceTransactionOutcomeV2 as N;
    use TaskAProtectiveOutcomeV1 as P;
    match outcome {
        N::Ready | N::Complete(_) | N::NoEffect(_, _) => Ok(()),
        N::Retained(_, reason) => Err(protective(P::Retained, reason)),
        N::ProtectiveDebt(reason) => Err(protective(P::ProtectiveDebt, reason)),
        N::Unsupported(reason) => Err(protective(P::Unsupported, reason)),
    }
}
fn encoded<T: Serialize>(value: &T) -> FlightResult<Vec<u8>> {
    let bytes = serde_json::to_vec(value)
        .map_err(|error| RemoteRequestFlightRefusalV1::Malformed(format!("{error:?}")))?;
    (bytes.len() <= WIRE_CAP)
        .then_some(bytes)
        .ok_or_else(|| RemoteRequestFlightRefusalV1::Malformed("wire exceeds cap".into()))
}
fn read_wire<T: DeserializeOwned>(
    op: &JournalRootOperationV2<'_>,
    name: &ChildNameV2,
) -> FlightResult<(T, FileContentSnapshotV2)> {
    let file = open_regular_child(op.root_file(), name.as_os_str(), "request wire").map_err(fs)?;
    let snapshot = required_file_content_snapshot_v2(&file, "request wire").map_err(fs)?;
    let bytes = op
        .read(name, snapshot, WIRE_CAP, "request wire")
        .map_err(fs)?;
    serde_json::from_slice(&bytes)
        .map(|wire| (wire, snapshot))
        .map_err(|error| RemoteRequestFlightRefusalV1::Malformed(error.to_string()))
}
fn canonical(id: &DedicatedRemoteRequestIdV1) -> bool {
    let value = id.as_str();
    value.len() == DedicatedRemoteRequestIdV1::ENCODED_LEN
        && value.starts_with(DedicatedRemoteRequestIdV1::PREFIX)
}
fn validate_owner(owner: &ResourceFlightOwnerV1) -> FlightResult<()> {
    let node_valid = crate::ids::NodeId::parse(owner.node_id.as_str()).is_ok();
    let key_valid = !owner.owner_key.is_empty()
        && owner.owner_key.len() <= WIRE_CAP
        && !owner.owner_key.chars().any(char::is_control);
    (node_valid && key_valid)
        .then_some(())
        .ok_or_else(|| Refusal::Malformed("invalid request owner".into()))
}
impl RemoteRequestJournalV1 {
    pub fn initialize(custody: JournalRootCustodyV2, attempt: AttemptIdentity) -> FlightResult<()> {
        Self::initialize_with_capacity(custody, attempt, CAPACITY)
    }
    fn initialize_with_capacity(
        custody: JournalRootCustodyV2,
        attempt: AttemptIdentity,
        capacity: usize,
    ) -> FlightResult<()> {
        if !(ADMISSION_FOOTPRINT + 1..=CAPACITY).contains(&capacity) {
            return Err(RemoteRequestFlightRefusalV1::Capacity);
        }
        {
            let op = custody
                .begin_operation("initialize request journal")
                .map_err(fs)?;
            if !op
                .enumerate(capacity + 1, "initialize request journal")
                .map_err(fs)?
                .is_empty()
            {
                return Err(RemoteRequestFlightRefusalV1::Malformed(
                    "request root is not empty".into(),
                ));
            }
            let lease_name = Self::attempt_lease_name();
            let staged = mutation(op.stage(&lease_name, b"", "stage attempt lease"))?;
            mutation(op.publish(&lease_name, staged, "publish attempt lease"))?;
            let checkpoint = checkpoint(&attempt, 0);
            let name = Self::checkpoint_name();
            let staged = mutation(op.stage(&name, &encoded(&checkpoint)?, "stage checkpoint"))?;
            mutation(op.publish(&name, staged, "publish checkpoint"))?;
            sync(op.sync("sync initialized request root"))?;
        }
        Ok(())
    }
    #[cfg(test)]
    pub fn open(custody: JournalRootCustodyV2, attempt: AttemptIdentity) -> FlightResult<Self> {
        Self::open_with_capacity(custody, attempt, CAPACITY)
    }
    #[cfg(test)]
    fn open_with_capacity(
        custody: JournalRootCustodyV2,
        attempt: AttemptIdentity,
        capacity: usize,
    ) -> FlightResult<Self> {
        Self::open_base(
            custody,
            attempt,
            capacity,
            Arc::new(tests::TestAckPublisherV1::default()),
        )
    }
    pub fn open_recovered(
        custody: JournalRootCustodyV2,
        attempt: AttemptIdentity,
        capacity: usize,
        publisher: Arc<dyn RemoteRequestResultPublisherV1>,
    ) -> FlightResult<Self> {
        let mut journal = Self::open_base(custody, attempt, capacity, publisher)?;
        journal.recover_send_states()?;
        Ok(journal)
    }
    fn attempt_lease_name() -> ChildNameV2 {
        ChildNameV2::from_bytes(ATTEMPT_LEASE_CHILD_V1.as_bytes())
            .expect("portable attempt lease name")
    }
    fn acquire_attempt_lease(custody: &JournalRootCustodyV2) -> FlightResult<PersistentLockGuard> {
        let name = Self::attempt_lease_name();
        let (lease, snapshot) = custody
            .acquire_existing_regular_child_lease(&name, "open attempt lifetime lease")
            .map_err(|error| {
                if matches!(
                    &error,
                    FsCustodyError::Io(_, source)
                        if source.kind() == std::io::ErrorKind::WouldBlock
                ) {
                    Refusal::AttemptLive
                } else {
                    fs(error)
                }
            })?;
        if snapshot.content_len != 0 {
            return Err(Refusal::Malformed("attempt lease is not empty".into()));
        }
        Ok(lease)
    }
    fn open_base(
        custody: JournalRootCustodyV2,
        attempt: AttemptIdentity,
        capacity: usize,
        publisher: Arc<dyn RemoteRequestResultPublisherV1>,
    ) -> FlightResult<Self> {
        if !(ADMISSION_FOOTPRINT + 1..=CAPACITY).contains(&capacity) {
            return Err(Refusal::Capacity);
        }
        let attempt_lease = Self::acquire_attempt_lease(&custody)?;
        let journal = Self {
            custody,
            attempt,
            capacity,
            requires_reopen: false,
            publisher,
            _attempt_lease: attempt_lease,
            admission_mutex: Mutex::new(()),
        };
        let operation = journal
            .custody
            .begin_operation("open request journal")
            .map_err(fs)?;
        journal.authorize_checkpoint(&operation)?;
        journal.scan_with(&operation, true)?;
        recovery(NamespaceTransactionV2::recover(
            &operation,
            "recover request transaction",
        ))?;
        let census = journal.scan(&operation)?;
        if census.staged {
            return Err(RemoteRequestFlightRefusalV1::ReopenRequired(
                "ambiguous staged child",
            ));
        }
        let ahead = census
            .children
            .iter()
            .filter(|child| child.ordinal >= census.checkpoint.next_ordinal)
            .collect::<Vec<_>>();
        let orphan = match ahead.as_slice() {
            [] => None,
            // An active child at exactly the checkpoint ordinal is the proven
            // step-5 orphan; a pre-send failure there is the resumable
            // intermediate of an interrupted heal (relabel done, checkpoint
            // advance pending).
            [child]
                if child.ordinal == census.checkpoint.next_ordinal
                    && matches!(
                        child.status,
                        ChildStateV1::Active {} | ChildStateV1::PreSendFailure {}
                    ) =>
            {
                Some(*child)
            }
            _ => return Err(Refusal::ReopenRequired("ambiguous request ordinal census")),
        };
        if let Some(child) = orphan {
            // Relabel first so a crash between the two durable steps leaves
            // the recognizable resumable intermediate, never an active child
            // stranded below the checkpoint.
            if child.status == (ChildStateV1::Active {}) {
                Self::replace_child(
                    &operation,
                    child,
                    ChildStateV1::PreSendFailure {},
                    "close orphan request",
                )?;
                sync(operation.sync("sync closed orphan"))?;
            }
            let next = census
                .checkpoint
                .next_ordinal
                .checked_add(1)
                .ok_or(Refusal::OrdinalOverflow)?;
            let value = checkpoint(&journal.attempt, next);
            let healed = NamespaceTransactionV2::replace(
                &operation,
                Self::checkpoint_name(),
                census.checkpoint_snapshot.object,
                &encoded(&value)?,
                "heal orphan checkpoint",
            );
            #[cfg(test)]
            task_a_transaction_boundary(TaskABoundaryV1::HealCheckpoint, healed)?;
            #[cfg(not(test))]
            transaction(healed)?;
            sync(operation.sync("sync healed checkpoint"))?;
        }
        for child in &census.children {
            match &child.status {
                ChildStateV1::PublicationAcknowledged { delivery_id: ack }
                    if *ack == delivery_id(child) =>
                {
                    Self::retire_child(&operation, child, "retire acknowledged request")?
                }
                ChildStateV1::PublicationAcknowledged { .. } => {
                    return Err(Refusal::DigestMismatch("publication acknowledgement"))
                }
                ChildStateV1::Active {}
                | ChildStateV1::PreSendFailure {}
                | ChildStateV1::IntentJournaled {}
                | ChildStateV1::DispatchAuthorized {}
                | ChildStateV1::ProviderSendArmed {}
                | ChildStateV1::TerminalPendingPublication { .. } => {}
            }
        }
        sync(operation.sync("sync reopened request root"))?;
        drop(operation);
        Ok(journal)
    }
    #[cfg(test)]
    #[must_use]
    fn requires_reopen(&self) -> bool {
        self.requires_reopen
    }
    fn checkpoint_name() -> ChildNameV2 {
        ChildNameV2::from_bytes(CHECKPOINT_CHILD_V1.as_bytes()).expect("portable checkpoint name")
    }
    fn validate_checkpoint(&self, checkpoint: &CheckpointWireV1) -> FlightResult<()> {
        if checkpoint.schema != SCHEMA {
            return Err(Refusal::ForeignSchema("checkpoint"));
        }
        if checkpoint.attempt != self.attempt {
            return Err(Refusal::ForeignAttempt);
        }
        if checkpoint.identity_chain_digest
            != checkpoint_digest(&self.attempt, checkpoint.next_ordinal)
        {
            return Err(Refusal::DigestMismatch("checkpoint"));
        }
        Ok(())
    }
    fn authorize_checkpoint(&self, op: &JournalRootOperationV2<'_>) -> FlightResult<()> {
        let names = match op.enumerate(self.capacity + 1, "authorize request checkpoint") {
            Ok(names) => names,
            Err(FsCustodyError::EnumerationLimitExceeded { .. }) => return Err(Refusal::Capacity),
            Err(error) => return Err(fs(error)),
        };
        if names.len() > self.capacity {
            return Err(Refusal::Capacity);
        }
        let name = Self::checkpoint_name();
        if !names.iter().any(|candidate| candidate == name.as_os_str()) {
            return Err(Refusal::Malformed("checkpoint is absent".into()));
        }
        let (checkpoint, _): (CheckpointWireV1, _) = read_wire(op, &name)?;
        self.validate_checkpoint(&checkpoint)
    }
    fn replace_child(
        op: &JournalRootOperationV2<'_>,
        child: &CensusChildV1,
        status: ChildStateV1,
        label: &str,
    ) -> FlightResult<()> {
        let mut successor = child.wire.clone();
        successor.status = status;
        let successor_bytes = encoded(&successor)?;
        let replaced = NamespaceTransactionV2::replace(
            op,
            request_name(&child.authority_digest),
            child.snapshot.object,
            &successor_bytes,
            label,
        );
        #[cfg(test)]
        let result = task_a_transaction_boundary(TaskABoundaryV1::Replace, replaced);
        #[cfg(not(test))]
        let result = transaction(replaced);
        result
    }
    fn retire_child(
        op: &JournalRootOperationV2<'_>,
        child: &CensusChildV1,
        label: &str,
    ) -> FlightResult<()> {
        let outcome = NamespaceTransactionV2::retire(
            op,
            request_name(&child.authority_digest),
            child.snapshot.object,
            label,
        );
        #[cfg(test)]
        return task_a_transaction_boundary(TaskABoundaryV1::Retire, outcome);
        #[cfg(not(test))]
        transaction(outcome)
    }
    fn scan(&self, op: &JournalRootOperationV2<'_>) -> FlightResult<CensusV1> {
        self.scan_with(op, false)
    }
    /// Residue-tolerant validation pass: with `tolerate_residue`, reserved
    /// Task A entries are skipped (recovery owns their classification) while
    /// every ordinary row is validated exactly as in a full scan. Running
    /// this before recovery makes an invalid attempt refuse byte-preserved.
    fn scan_with(
        &self,
        op: &JournalRootOperationV2<'_>,
        tolerate_residue: bool,
    ) -> FlightResult<CensusV1> {
        let names = match op.enumerate(self.capacity + 1, "request census") {
            Ok(names) => names,
            Err(FsCustodyError::EnumerationLimitExceeded { .. }) => return Err(Refusal::Capacity),
            Err(error) => return Err(fs(error)),
        };
        if names.len() > self.capacity {
            return Err(RemoteRequestFlightRefusalV1::Capacity);
        }
        if names.iter().any(|name| {
            name.to_str().is_some_and(|value| {
                value.starts_with("resource-flight-") || value.starts_with(".resource-flight-")
            })
        }) {
            return Err(RemoteRequestFlightRefusalV1::LegacyMigrationRequired);
        }
        let mut checkpoint = None;
        let mut lease = false;
        let mut children = Vec::new();
        let mut staged = false;
        for raw in &names {
            let value = raw
                .to_str()
                .ok_or_else(|| RemoteRequestFlightRefusalV1::Malformed("non-UTF-8 child".into()))?;
            let name = ChildNameV2::from_bytes(value.as_bytes())
                .map_err(|error| RemoteRequestFlightRefusalV1::Malformed(format!("{error:?}")))?;
            if name == Self::checkpoint_name() {
                checkpoint = Some(read_wire(op, &name)?);
                continue;
            }
            if name == Self::attempt_lease_name() {
                let file = open_regular_child(op.root_file(), name.as_os_str(), "attempt lease")
                    .map_err(fs)?;
                if required_file_content_snapshot_v2(&file, "attempt lease")
                    .map_err(fs)?
                    .content_len
                    != 0
                {
                    return Err(Refusal::Malformed("attempt lease is not empty".into()));
                }
                lease = true;
                continue;
            }
            let (read_name, final_name, is_staged) = if value.starts_with(REQUEST_CHILD_PREFIX_V1) {
                (name.clone(), name, false)
            } else if tolerate_residue && value.starts_with(".a2a-v2-") {
                continue;
            } else if value.starts_with(".a2a-v2-stg-") {
                let target = ChildNameV2::parse_reserved(ReservedNameNamespaceV2::Staging, &name)
                    .map_err(fs)?;
                if !target
                    .as_os_str()
                    .to_str()
                    .is_some_and(|value| value.starts_with(REQUEST_CHILD_PREFIX_V1))
                {
                    return Err(protective(
                        TaskAProtectiveOutcomeV1::ProtectiveDebt,
                        "foreign Task A residue",
                    ));
                }
                (name, target, true)
            } else if value.starts_with(".a2a-v2-") {
                return Err(protective(
                    TaskAProtectiveOutcomeV1::ProtectiveDebt,
                    "unrecovered Task A residue",
                ));
            } else {
                return Err(RemoteRequestFlightRefusalV1::Malformed(value.into()));
            };
            let (wire, snapshot): (RequestChildWireV1, _) = read_wire(op, &read_name)?;
            if wire.schema != SCHEMA {
                return Err(RemoteRequestFlightRefusalV1::ForeignSchema("request child"));
            }
            if wire.attempt != self.attempt {
                return Err(RemoteRequestFlightRefusalV1::ForeignAttempt);
            }
            validate_owner(&wire.owner)?;
            if wire.checkpoint_digest != checkpoint_digest(&self.attempt, wire.ordinal)
                || wire.authority_digest != authority_digest(&wire)
                || final_name != request_name(&wire.authority_digest)
            {
                return Err(RemoteRequestFlightRefusalV1::DigestMismatch(
                    "request child",
                ));
            }
            if !canonical(&wire.request_id) {
                return Err(RemoteRequestFlightRefusalV1::Malformed(
                    "non-canonical request identity".into(),
                ));
            }
            if let ChildStateV1::PublicationAcknowledged { delivery_id: ack } = &wire.status {
                if *ack != delivery_id(&wire) {
                    return Err(Refusal::DigestMismatch("publication acknowledgement"));
                }
            }
            if is_staged {
                if std::mem::replace(&mut staged, true) {
                    return Err(RemoteRequestFlightRefusalV1::IdentityCollision);
                }
            } else {
                children.push(CensusChildV1 { wire, snapshot });
            }
        }
        let (checkpoint, checkpoint_snapshot): (CheckpointWireV1, _) =
            checkpoint.ok_or_else(|| Refusal::Malformed("checkpoint is absent".into()))?;
        if !lease {
            return Err(Refusal::Malformed("attempt lease is absent".into()));
        }
        self.validate_checkpoint(&checkpoint)?;
        let mut ordinals = BTreeSet::new();
        let mut requests = BTreeSet::new();
        for child in children.iter() {
            if !ordinals.insert(child.ordinal) || !requests.insert(child.request_id.as_str()) {
                return Err(RemoteRequestFlightRefusalV1::IdentityCollision);
            }
        }
        Ok(CensusV1 {
            checkpoint,
            checkpoint_snapshot,
            children,
            staged,
        })
    }
    fn authority_for(wire: &RequestChildWireV1) -> RemoteRequestAuthorityV1 {
        RemoteRequestAuthorityV1 {
            attempt: wire.attempt.clone(),
            ordinal: wire.ordinal,
            request_id: wire.request_id.clone(),
        }
    }
    fn recovered_result(disposition: ResourceActionDispositionV1) -> ResourceActionResultV1 {
        ResourceActionResultV1 {
            disposition,
            duration_ms: 0,
            recovery_owner: None,
            cause: None,
        }
    }
    fn publication(child: &CensusChildV1) -> FlightResult<RemoteRequestTerminalPublicationV1> {
        match &child.status {
            ChildStateV1::TerminalPendingPublication {
                result,
                prompt_may_have_been_accepted,
            } => Ok(RemoteRequestTerminalPublicationV1 {
                delivery_id: delivery_id(child),
                result: result.clone(),
                prompt_may_have_been_accepted: *prompt_may_have_been_accepted,
            }),
            _ => Err(Refusal::InvalidStateTransition(
                "request has no pending terminal publication",
            )),
        }
    }
    fn recover_send_states(&mut self) -> FlightResult<()> {
        let operation = self
            .custody
            .begin_operation("recover remote request send states")
            .map_err(fs)?;
        let census = self.scan(&operation)?;
        if census.staged {
            return Err(Refusal::ReopenRequired("ambiguous staged child"));
        }
        let mut pending = Vec::new();
        for child in &census.children {
            let authority = Self::authority_for(child);
            let recovered = match &child.status {
                ChildStateV1::Active {}
                | ChildStateV1::PreSendFailure {}
                | ChildStateV1::IntentJournaled {}
                | ChildStateV1::DispatchAuthorized {} => Some((
                    Self::recovered_result(ResourceActionDispositionV1::Failed),
                    false,
                )),
                ChildStateV1::ProviderSendArmed {} => Some((
                    Self::recovered_result(ResourceActionDispositionV1::Unknown),
                    true,
                )),
                ChildStateV1::TerminalPendingPublication { .. } => None,
                ChildStateV1::PublicationAcknowledged { .. } => {
                    Self::retire_child(
                        &operation,
                        child,
                        "retire recovered publication acknowledgement",
                    )?;
                    continue;
                }
            };
            if let Some((result, accepted)) = recovered {
                Self::replace_child(
                    &operation,
                    child,
                    ChildStateV1::TerminalPendingPublication {
                        result,
                        prompt_may_have_been_accepted: accepted,
                    },
                    "terminalize recovered request",
                )?;
            }
            pending.push(authority);
        }
        sync(operation.sync("sync recovered send states"))?;
        drop(operation);
        for authority in pending {
            self.publish_pending(&authority, None)?;
        }
        Ok(())
    }
    fn transition_state(
        &mut self,
        authority: &RemoteRequestAuthorityV1,
        expected: fn(&ChildStateV1) -> bool,
        successor: ChildStateV1,
        label: &'static str,
        settle_on_failure: bool,
    ) -> FlightResult<()> {
        if self.requires_reopen {
            return Err(Refusal::ReopenRequired("prior transition was interrupted"));
        }
        let operation = self.custody.begin_operation(label).map_err(fs)?;
        let census = self.scan(&operation)?;
        let child = Self::child_for(&census, authority)?;
        if !expected(&child.status) {
            return Err(Refusal::InvalidStateTransition(label));
        }
        #[cfg(test)]
        let arm_effect_then_debt = (label == "arm provider send")
            .then(take_arm_effect_then_debt_for_test)
            .flatten();
        #[cfg(test)]
        if label == "arm provider send" && arm_effect_then_debt.is_none() {
            if let Some(injected) = take_task_a_boundary(TaskABoundaryV1::Replace) {
                drop(operation);
                return injected_task_a_no_effect(injected);
            }
        }
        let result = Self::replace_child(&operation, child, successor, label);
        drop(operation);
        #[cfg(test)]
        let result = match (result, arm_effect_then_debt) {
            (Ok(()), Some(fail_terminal_settlement)) => {
                if fail_terminal_settlement {
                    inject_terminal_settlement_failure_for_test();
                }
                Err(protective(
                    TaskAProtectiveOutcomeV1::ProtectiveDebt,
                    "injected effect-then-debt",
                ))
            }
            (result, _) => result,
        };
        if result.as_ref().is_err_and(|error| {
            !matches!(error, Refusal::TaskA(TaskAProtectiveOutcomeV1::Refused, _))
                && !settle_on_failure
        }) {
            self.requires_reopen = true;
        }
        result
    }
    pub fn record_intent(&mut self, authority: &RemoteRequestAuthorityV1) -> FlightResult<()> {
        self.transition_state(
            authority,
            |state| matches!(state, ChildStateV1::Active {}),
            ChildStateV1::IntentJournaled {},
            "journal request intent",
            false,
        )
    }
    pub fn authorize_dispatch(&mut self, authority: &RemoteRequestAuthorityV1) -> FlightResult<()> {
        self.transition_state(
            authority,
            |state| matches!(state, ChildStateV1::IntentJournaled {}),
            ChildStateV1::DispatchAuthorized {},
            "authorize request dispatch",
            false,
        )
    }
    pub fn arm_provider_send(&mut self, authority: &RemoteRequestAuthorityV1) -> FlightResult<()> {
        self.transition_state(
            authority,
            |state| matches!(state, ChildStateV1::DispatchAuthorized {}),
            ChildStateV1::ProviderSendArmed {},
            "arm provider send",
            true,
        )
    }
    pub fn settle(
        &mut self,
        authority: &RemoteRequestAuthorityV1,
        result: ResourceActionResultV1,
        prompt_may_have_been_accepted: bool,
    ) -> FlightResult<()> {
        self.settle_inner(authority, result, prompt_may_have_been_accepted, None)
    }
    fn terminal_winner(
        &mut self,
        authority: &RemoteRequestAuthorityV1,
        result: ResourceActionResultV1,
        accepted: bool,
        allow_armed_pre_send: bool,
    ) -> FlightResult<RemoteRequestTerminalPublicationV1> {
        if self.requires_reopen {
            return Err(Refusal::ReopenRequired("prior transition was interrupted"));
        }
        let operation = self
            .custody
            .begin_operation("persist terminal publication")
            .map_err(fs)?;
        let census = self.scan(&operation)?;
        let child = Self::child_for(&census, authority)?;
        if child.status.is_terminal_pending() {
            return Self::publication(child);
        }
        let valid = if accepted {
            matches!(child.status, ChildStateV1::ProviderSendArmed {})
        } else {
            // Only the arming wrapper's zero-poll failure branch may settle
            // an armed row as unaccepted: it positively owns the unpolled
            // future. Every ordinary settlement path must refuse, or a stale
            // acceptance flag could durably misreport an accepted send.
            matches!(
                child.status,
                ChildStateV1::Active {}
                    | ChildStateV1::PreSendFailure {}
                    | ChildStateV1::IntentJournaled {}
                    | ChildStateV1::DispatchAuthorized {}
            ) || (allow_armed_pre_send
                && matches!(child.status, ChildStateV1::ProviderSendArmed {}))
        };
        if !valid {
            return Err(Refusal::InvalidStateTransition(
                "persist terminal publication",
            ));
        }
        #[cfg(test)]
        if take_terminal_settlement_failure_for_test() {
            return Err(protective(
                TaskAProtectiveOutcomeV1::ProtectiveDebt,
                "injected terminal settlement failure",
            ));
        }
        let publication = RemoteRequestTerminalPublicationV1 {
            delivery_id: authority.delivery_id(),
            result: result.clone(),
            prompt_may_have_been_accepted: accepted,
        };
        let replaced = Self::replace_child(
            &operation,
            child,
            ChildStateV1::TerminalPendingPublication {
                result,
                prompt_may_have_been_accepted: accepted,
            },
            "persist terminal publication",
        );
        drop(operation);
        if replaced.as_ref().is_err_and(|error| {
            !matches!(error, Refusal::TaskA(TaskAProtectiveOutcomeV1::Refused, _))
        }) {
            self.requires_reopen = true;
        }
        replaced.map(|()| publication)
    }
    fn settle_inner(
        &mut self,
        authority: &RemoteRequestAuthorityV1,
        result: ResourceActionResultV1,
        accepted: bool,
        cut: Option<u8>,
    ) -> FlightResult<()> {
        let outcome = (|| {
            let publication = self.terminal_winner(authority, result, accepted, false)?;
            #[cfg(test)]
            boundary(cut, 0, "before publisher callback")?;
            self.publish_publication(authority, publication, cut)
        })();
        if outcome.is_err() {
            self.requires_reopen = true;
        }
        outcome
    }
    fn publish_pending(
        &mut self,
        authority: &RemoteRequestAuthorityV1,
        _cut: Option<u8>,
    ) -> FlightResult<()> {
        let operation = self
            .custody
            .begin_operation("read pending terminal publication")
            .map_err(fs)?;
        let census = self.scan(&operation)?;
        let publication = Self::publication(Self::child_for(&census, authority)?)?;
        drop(operation);
        self.publish_publication(authority, publication, _cut)
    }
    fn publish_publication(
        &mut self,
        authority: &RemoteRequestAuthorityV1,
        publication: RemoteRequestTerminalPublicationV1,
        cut: Option<u8>,
    ) -> FlightResult<()> {
        let acknowledgement = self
            .publisher
            .publish_idempotent(&publication)
            .map_err(Refusal::PublicationRefused)?;
        if acknowledgement != *publication.delivery_id() {
            return Err(Refusal::PublicationAcknowledgementMismatch);
        }
        self.finish_publication(authority, acknowledgement, cut)
    }
    fn finish_publication(
        &mut self,
        authority: &RemoteRequestAuthorityV1,
        acknowledgement: RemoteRequestDeliveryIdV1,
        _cut: Option<u8>,
    ) -> FlightResult<()> {
        self.transition_state(
            authority,
            ChildStateV1::is_terminal_pending,
            ChildStateV1::PublicationAcknowledged {
                delivery_id: acknowledgement,
            },
            "acknowledge terminal publication",
            false,
        )?;
        #[cfg(test)]
        boundary(_cut, 1, "after publication acknowledgement")?;
        self.retire_inner(authority, None)
    }
    pub fn admit(
        &mut self,
        owner: ResourceFlightOwnerV1,
    ) -> FlightResult<RemoteRequestAuthorityV1> {
        self.admit_with(owner, DedicatedRemoteRequestIdV1::mint)
    }
    fn admit_with<F>(
        &mut self,
        owner: ResourceFlightOwnerV1,
        mint: F,
    ) -> FlightResult<RemoteRequestAuthorityV1>
    where
        F: FnOnce() -> Result<DedicatedRemoteRequestIdV1, crate::error::BridgeError>,
    {
        self.admit_inner(owner, mint, None)
    }
    fn admit_inner<F>(
        &mut self,
        owner: ResourceFlightOwnerV1,
        mint: F,
        _cut: Option<u8>,
    ) -> FlightResult<RemoteRequestAuthorityV1>
    where
        F: FnOnce() -> Result<DedicatedRemoteRequestIdV1, crate::error::BridgeError>,
    {
        validate_owner(&owner)?;
        if self.requires_reopen {
            return Err(RemoteRequestFlightRefusalV1::ReopenRequired(
                "prior admission was interrupted",
            ));
        }
        let _admission = self
            .admission_mutex
            .lock()
            .map_err(|_| Refusal::AdmissionMutexPoisoned)?;
        let op = self
            .custody
            .begin_operation("admit remote request")
            .map_err(fs)?;
        let census = self.scan(&op)?;
        if census.children.iter().any(|child| {
            matches!(
                child.status,
                ChildStateV1::TerminalPendingPublication { .. }
                    | ChildStateV1::PublicationAcknowledged { .. }
            )
        }) {
            return Err(Refusal::ReopenRequired("terminal publication is pending"));
        }
        if census.staged
            || census.children.len().saturating_add(ADMISSION_FOOTPRINT) >= self.capacity
        {
            return Err(RemoteRequestFlightRefusalV1::Capacity);
        }
        let result = (|| {
            let request_id = mint().map_err(|error| {
                RemoteRequestFlightRefusalV1::IdentityUnavailable(error.to_string())
            })?;
            if !canonical(&request_id) {
                return Err(RemoteRequestFlightRefusalV1::Malformed(
                    "mint returned a non-canonical identity".into(),
                ));
            }
            if census
                .children
                .iter()
                .any(|child| child.request_id == request_id)
            {
                return Err(RemoteRequestFlightRefusalV1::IdentityCollision);
            }
            let active_next = match census.children.iter().map(|child| child.ordinal).max() {
                Some(ordinal) => ordinal
                    .checked_add(1)
                    .ok_or(RemoteRequestFlightRefusalV1::OrdinalOverflow)?,
                None => 0,
            };
            let ordinal = census.checkpoint.next_ordinal.max(active_next);
            let next = ordinal
                .checked_add(1)
                .ok_or(RemoteRequestFlightRefusalV1::OrdinalOverflow)?;
            let mut wire = RequestChildWireV1 {
                schema: SCHEMA,
                attempt: self.attempt.clone(),
                ordinal,
                checkpoint_digest: checkpoint_digest(&self.attempt, ordinal),
                authority_digest: Sha256HexV1::digest(b"pending"),
                request_id,
                owner,
                status: ChildStateV1::Active {},
            };
            wire.authority_digest = authority_digest(&wire);
            let name = request_name(&wire.authority_digest);
            #[cfg(test)]
            boundary(_cut, 0, "temporary write")?;
            let staged_bytes = encoded(&wire)?;
            let staged_actual = op.stage(&name, &staged_bytes, "stage request child");
            #[cfg(test)]
            let staged = task_a_journal_boundary(TaskABoundaryV1::Stage, staged_actual)?;
            #[cfg(not(test))]
            let staged = mutation(staged_actual)?;
            #[cfg(test)]
            boundary(_cut, 1, "temporary sync")?;
            #[cfg(test)]
            boundary(_cut, 2, "no-replace publication")?;
            let published = op.publish(&name, staged, "publish request child");
            #[cfg(test)]
            let _snapshot = task_a_journal_boundary(TaskABoundaryV1::Publish, published)?;
            #[cfg(not(test))]
            let _snapshot = mutation(published)?;
            #[cfg(test)]
            boundary(_cut, 3, "request root sync")?;
            let root_synced = op.sync("sync request root");
            #[cfg(test)]
            task_a_sync_boundary(TaskABoundaryV1::Sync, root_synced)?;
            #[cfg(not(test))]
            sync(root_synced)?;
            #[cfg(test)]
            boundary(_cut, 4, "checkpoint advance")?;
            let checkpoint = checkpoint(&self.attempt, next);
            let advanced = NamespaceTransactionV2::replace(
                &op,
                Self::checkpoint_name(),
                census.checkpoint_snapshot.object,
                &encoded(&checkpoint)?,
                "advance checkpoint",
            );
            #[cfg(test)]
            task_a_transaction_boundary(TaskABoundaryV1::Replace, advanced)?;
            #[cfg(not(test))]
            transaction(advanced)?;
            #[cfg(test)]
            boundary(_cut, 5, "checkpoint sync")?;
            let checkpoint_synced = op.sync("sync advanced checkpoint");
            #[cfg(test)]
            task_a_sync_boundary(TaskABoundaryV1::Sync, checkpoint_synced)?;
            #[cfg(not(test))]
            sync(checkpoint_synced)?;
            Ok(RemoteRequestAuthorityV1 {
                attempt: wire.attempt,
                ordinal: wire.ordinal,
                request_id: wire.request_id,
            })
        })();
        if result.is_err() {
            self.requires_reopen = true;
        }
        result
    }
    fn child_for<'a>(
        census: &'a CensusV1,
        authority: &RemoteRequestAuthorityV1,
    ) -> FlightResult<&'a CensusChildV1> {
        census
            .children
            .iter()
            .find(|child| {
                child.attempt == authority.attempt
                    && child.ordinal == authority.ordinal
                    && child.request_id == *authority.request_id()
            })
            .ok_or(Refusal::InvalidStateTransition(
                "request authority is absent",
            ))
    }
    #[cfg(test)]
    pub fn acknowledge(
        &mut self,
        authority: &RemoteRequestAuthorityV1,
        disposition: ResourceActionDispositionV1,
    ) -> FlightResult<()> {
        if disposition != ResourceActionDispositionV1::Complete {
            return Err(Refusal::TerminalNotComplete);
        }
        if self.requires_reopen {
            return Err(Refusal::ReopenRequired("prior transition was interrupted"));
        }
        let op = self
            .custody
            .begin_operation("acknowledge remote request")
            .map_err(fs)?;
        let census = self.scan(&op)?;
        if census.staged {
            return Err(Refusal::ReopenRequired("ambiguous staged child"));
        }
        let child = Self::child_for(&census, authority)?;
        let result = match &child.status {
            ChildStateV1::Active {} => Self::replace_child(
                &op,
                child,
                ChildStateV1::PublicationAcknowledged {
                    delivery_id: authority.delivery_id(),
                },
                "acknowledge terminal request",
            ),
            ChildStateV1::PublicationAcknowledged { delivery_id }
                if *delivery_id == authority.delivery_id() =>
            {
                Ok(())
            }
            ChildStateV1::PreSendFailure {}
            | ChildStateV1::IntentJournaled {}
            | ChildStateV1::DispatchAuthorized {}
            | ChildStateV1::ProviderSendArmed {}
            | ChildStateV1::TerminalPendingPublication { .. }
            | ChildStateV1::PublicationAcknowledged { .. } => Err(Refusal::InvalidStateTransition(
                "request cannot receive legacy acknowledgement",
            )),
        };
        drop(op);
        if result.is_err() {
            self.requires_reopen = true;
        }
        result
    }
    #[cfg(test)]
    pub fn retire(&mut self, authority: &RemoteRequestAuthorityV1) -> FlightResult<()> {
        self.retire_inner(authority, None)
    }
    fn retire_inner(
        &mut self,
        authority: &RemoteRequestAuthorityV1,
        _cut: Option<u8>,
    ) -> FlightResult<()> {
        if self.requires_reopen {
            return Err(Refusal::ReopenRequired("prior transition was interrupted"));
        }
        let op = self
            .custody
            .begin_operation("retire remote request")
            .map_err(fs)?;
        let census = self.scan(&op)?;
        let child = Self::child_for(&census, authority)?;
        if !matches!(
            &child.status,
            ChildStateV1::PublicationAcknowledged { delivery_id }
                if *delivery_id == authority.delivery_id()
        ) {
            return Err(Refusal::InvalidStateTransition(
                "request is not acknowledged",
            ));
        }
        let result = (|| {
            #[cfg(test)]
            boundary(_cut, 0, "before acknowledged unlink")?;
            Self::retire_child(&op, child, "unlink acknowledged request")?;
            #[cfg(test)]
            boundary(_cut, 1, "after acknowledged unlink")?;
            let retirement_synced = op.sync("sync acknowledged retirement");
            #[cfg(test)]
            task_a_sync_boundary(TaskABoundaryV1::Sync, retirement_synced)?;
            #[cfg(not(test))]
            sync(retirement_synced)?;
            #[cfg(test)]
            boundary(_cut, 2, "after acknowledged root sync")?;
            Ok(())
        })();
        drop(op);
        if result.is_err() {
            self.requires_reopen = true;
        }
        result
    }
}
impl RemoteRequestDriverV1 {
    pub fn open_recovered(
        custody: JournalRootCustodyV2,
        attempt: AttemptIdentity,
        capacity: usize,
        publisher: Arc<dyn RemoteRequestResultPublisherV1>,
    ) -> FlightResult<Self> {
        Ok(Self {
            journal: Arc::new(Mutex::new(RemoteRequestJournalV1::open_recovered(
                custody, attempt, capacity, publisher,
            )?)),
        })
    }

    fn lock(&self) -> FlightResult<MutexGuard<'_, RemoteRequestJournalV1>> {
        self.journal
            .lock()
            .map_err(|_| Refusal::RequestMutexPoisoned)
    }

    pub fn admit(&self, owner: ResourceFlightOwnerV1) -> FlightResult<OwnedRemoteRequestV1> {
        let authority = self.lock()?.admit(owner)?;
        let (outcome_tx, initial_receiver) = tokio::sync::watch::channel(None);
        drop(initial_receiver);
        Ok(OwnedRemoteRequestV1 {
            journal: Arc::clone(&self.journal),
            authority,
            outcome_tx,
            live_waiters: Arc::new(AtomicUsize::new(0)),
            provider_send_claimed: AtomicBool::new(false),
            provider_send_armed: AtomicBool::new(false),
            settlement_attempted: AtomicBool::new(false),
            publication_flight: Mutex::new(PublicationFlightStateV1::Idle),
            publication_settled: Condvar::new(),
        })
    }
}
impl OwnedRemoteRequestV1 {
    fn lock(&self) -> FlightResult<MutexGuard<'_, RemoteRequestJournalV1>> {
        self.journal
            .lock()
            .map_err(|_| Refusal::RequestMutexPoisoned)
    }

    #[must_use]
    pub fn request_id(&self) -> &DedicatedRemoteRequestIdV1 {
        self.authority.request_id()
    }

    pub fn journal_intent(&self) -> FlightResult<()> {
        self.lock()?.record_intent(&self.authority)
    }

    pub fn authorize_dispatch(&self) -> FlightResult<()> {
        self.lock()?.authorize_dispatch(&self.authority)
    }

    #[must_use]
    pub fn arm_provider_send<F>(&self, future: F) -> ArmedProviderSendV1<'_, F>
    where
        F: Future,
    {
        ArmedProviderSendV1 {
            request: self,
            inner: Some(Box::pin(future)),
            arm_attempted: false,
        }
    }

    fn arm_now(&self) -> FlightResult<()> {
        self.lock()?.arm_provider_send(&self.authority)?;
        self.provider_send_armed.store(true, Ordering::Release);
        Ok(())
    }

    #[must_use]
    pub fn observer(&self) -> RemoteRequestObserverV1 {
        RemoteRequestObserverV1 {
            outcome_rx: self.outcome_tx.subscribe(),
            live_waiters: Arc::clone(&self.live_waiters),
        }
    }

    #[must_use]
    pub fn live_waiters(&self) -> usize {
        self.live_waiters.load(Ordering::Acquire)
    }

    pub fn settle(
        &self,
        result: ResourceActionResultV1,
    ) -> FlightResult<RemoteRequestTerminalOutcomeV1> {
        let accepted = self.provider_send_armed.load(Ordering::Acquire);
        self.settle_with_acceptance(result, accepted)
    }

    fn settle_with_acceptance(
        &self,
        result: ResourceActionResultV1,
        accepted: bool,
    ) -> FlightResult<RemoteRequestTerminalOutcomeV1> {
        self.settle_with_acceptance_mode(result, accepted, false)
    }
    /// `failed_arm` is the arming wrapper's zero-poll privilege: it alone may
    /// settle an unaccepted terminal over a durably armed row.
    fn settle_with_acceptance_mode(
        &self,
        result: ResourceActionResultV1,
        accepted: bool,
        failed_arm: bool,
    ) -> FlightResult<RemoteRequestTerminalOutcomeV1> {
        self.settlement_attempted.store(true, Ordering::Release);
        let prior = { self.outcome_tx.borrow().clone() };
        let (outcome, publication) = if let Some(outcome) = prior {
            let publication = outcome.publication.clone();
            (outcome, publication)
        } else {
            let mut journal = self.lock()?;
            // A peer may have won while this caller waited for the journal.
            // Recheck under the same mutex that serializes durable CAS so the
            // winner is visible before its outbox can retire the row.
            let observed = { self.outcome_tx.borrow().clone() };
            let selected = if let Some(outcome) = observed {
                let publication = outcome.publication.clone();
                (outcome, publication)
            } else {
                let publication =
                    journal.terminal_winner(&self.authority, result, accepted, failed_arm)?;
                let outcome = RemoteRequestTerminalOutcomeV1::from(publication.clone());
                self.outcome_tx.send_replace(Some(outcome.clone()));
                (outcome, publication)
            };
            // The external publisher path reacquires this mutex only after
            // invoking its callback; make that lifetime structural here.
            drop(journal);
            selected
        };
        self.drive_publication(publication)?;
        Ok(outcome)
    }

    fn finish_publication_flight(&self, result: FlightResult<()>) -> FlightResult<()> {
        let mut flight = self
            .publication_flight
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *flight = PublicationFlightStateV1::Finished(result.clone());
        self.publication_settled.notify_all();
        result
    }

    fn drive_publication(
        &self,
        publication: RemoteRequestTerminalPublicationV1,
    ) -> FlightResult<()> {
        let mut flight = self
            .publication_flight
            .lock()
            .map_err(|_| Refusal::RequestMutexPoisoned)?;
        loop {
            match &*flight {
                PublicationFlightStateV1::Idle => {
                    *flight = PublicationFlightStateV1::Driving;
                    break;
                }
                PublicationFlightStateV1::Driving => {
                    flight = self
                        .publication_settled
                        .wait(flight)
                        .map_err(|_| Refusal::RequestMutexPoisoned)?;
                }
                PublicationFlightStateV1::Finished(result) => return result.clone(),
            }
        }
        drop(flight);
        let driving = PublicationDriverGuardV1::new(self);
        let result = (|| {
            let publisher = {
                let journal = self.lock()?;
                Arc::clone(&journal.publisher)
            };
            // No journal, admission, transition, or Task A operation lock is
            // held while the idempotent external sink is invoked.
            let acknowledgement = publisher
                .publish_idempotent(&publication)
                .map_err(Refusal::PublicationRefused)?;
            if acknowledgement != *publication.delivery_id() {
                return Err(Refusal::PublicationAcknowledgementMismatch);
            }
            self.lock()?
                .finish_publication(&self.authority, acknowledgement, None)
        })();
        driving.finish(result)
    }

    #[cfg(test)]
    fn crash_without_settlement_for_test(self) {
        self.settlement_attempted.store(true, Ordering::Release);
    }
}
impl Drop for OwnedRemoteRequestV1 {
    fn drop(&mut self) {
        if self.settlement_attempted.swap(true, Ordering::AcqRel) {
            return;
        }
        let accepted = self.provider_send_armed.load(Ordering::Acquire);
        let result = if accepted {
            unknown_after_arm()
        } else {
            failed_before_send()
        };
        let _ = self.settle_with_acceptance(result, accepted);
    }
}
impl<F: Future> Future for ArmedProviderSendV1<'_, F> {
    type Output = FlightResult<F::Output>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        if !this.arm_attempted {
            this.arm_attempted = true;
            if this
                .request
                .provider_send_claimed
                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                .is_err()
            {
                this.inner = None;
                return Poll::Ready(Err(Refusal::InvalidStateTransition(
                    "claim provider send wrapper",
                )));
            }
            if let Err(error) = this.request.arm_now() {
                // A refused arm is known pre-send. The inner future is
                // destroyed here without receiving its first poll.
                this.inner = None;
                let _ = this
                    .request
                    .settle_with_acceptance_mode(failed_before_send(), false, true);
                return Poll::Ready(Err(error));
            }
        }
        let poll = this
            .inner
            .as_mut()
            .expect("provider-send future polled after completion")
            .as_mut()
            .poll(cx);
        match poll {
            Poll::Pending => Poll::Pending,
            Poll::Ready(output) => {
                this.inner = None;
                Poll::Ready(Ok(output))
            }
        }
    }
}
impl RemoteRequestObserverV1 {
    pub async fn wait_until(
        &mut self,
        deadline: tokio::time::Instant,
    ) -> FlightResult<RemoteRequestTerminalOutcomeV1> {
        self.live_waiters.fetch_add(1, Ordering::AcqRel);
        let _waiter = LiveWaiterGuardV1 {
            live_waiters: Arc::clone(&self.live_waiters),
        };
        loop {
            if let Some(outcome) = { self.outcome_rx.borrow().clone() } {
                return Ok(outcome);
            }
            match tokio::time::timeout_at(deadline, self.outcome_rx.changed()).await {
                Ok(Ok(())) => {}
                Ok(Err(_)) => return Err(Refusal::ObservationClosed),
                Err(_) => return Err(Refusal::ObservationTimedOut),
            }
        }
    }
}
#[cfg(test)]
fn boundary(actual: Option<u8>, expected: u8, reason: &'static str) -> FlightResult<()> {
    (actual != Some(expected))
        .then_some(())
        .ok_or(RemoteRequestFlightRefusalV1::ReopenRequired(reason))
}
#[cfg(test)]
impl RemoteRequestJournalV1 {
    fn admit_with_boundary<F>(
        &mut self,
        owner: ResourceFlightOwnerV1,
        mint: F,
        boundary: u8,
    ) -> FlightResult<RemoteRequestAuthorityV1>
    where
        F: FnOnce() -> Result<DedicatedRemoteRequestIdV1, crate::error::BridgeError>,
    {
        self.admit_inner(owner, mint, Some(boundary))
    }
    fn retire_with_boundary(
        &mut self,
        authority: &RemoteRequestAuthorityV1,
        boundary: u8,
    ) -> FlightResult<()> {
        self.retire_inner(authority, Some(boundary))
    }
    fn settle_with_boundary(
        &mut self,
        authority: &RemoteRequestAuthorityV1,
        result: ResourceActionResultV1,
        prompt_may_have_been_accepted: bool,
        boundary: u8,
    ) -> FlightResult<()> {
        self.settle_inner(
            authority,
            result,
            prompt_may_have_been_accepted,
            Some(boundary),
        )
    }
}
#[cfg(test)]
#[derive(Clone, Copy)]
enum InjectedTaskAOutcomeV1 {
    Refused,
    Retained,
    IoUnknown,
    Unsupported,
    ProtectiveDebt,
}
#[cfg(test)]
impl InjectedTaskAOutcomeV1 {
    const PROTECTIVE: [Self; 5] = [
        Self::Refused,
        Self::Retained,
        Self::IoUnknown,
        Self::Unsupported,
        Self::ProtectiveDebt,
    ];
}
#[cfg(test)]
fn injected_task_a_no_effect(outcome: InjectedTaskAOutcomeV1) -> FlightResult<()> {
    use InjectedTaskAOutcomeV1 as I;
    use TaskAProtectiveOutcomeV1 as P;
    match outcome {
        I::Refused => Err(protective(P::Refused, "injected")),
        I::Retained => Err(protective(P::Retained, "injected")),
        I::IoUnknown => Err(fs(FsCustodyError::Io(
            "injected".into(),
            std::io::Error::other("injected"),
        ))),
        I::Unsupported => Err(protective(P::Unsupported, "injected")),
        I::ProtectiveDebt => Err(protective(P::ProtectiveDebt, "injected")),
    }
}
#[cfg(test)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TaskABoundaryV1 {
    Stage,
    Publish,
    Replace,
    Retire,
    Sync,
    HealCheckpoint,
}
#[cfg(test)]
thread_local! {
    static INJECTED_TASK_A_BOUNDARY: std::cell::RefCell<
        Option<(TaskABoundaryV1, InjectedTaskAOutcomeV1)>,
    > = const { std::cell::RefCell::new(None) };
    static INJECTED_ARM_EFFECT_THEN_DEBT: std::cell::Cell<Option<bool>> = const {
        std::cell::Cell::new(None)
    };
    static INJECTED_TERMINAL_SETTLEMENT_FAILURE: std::cell::Cell<bool> = const {
        std::cell::Cell::new(false)
    };
}
#[cfg(test)]
fn inject_arm_effect_then_debt_for_test(fail_terminal_settlement: bool) {
    INJECTED_ARM_EFFECT_THEN_DEBT.with(|slot| {
        assert!(slot.replace(Some(fail_terminal_settlement)).is_none());
    });
}
#[cfg(test)]
fn take_arm_effect_then_debt_for_test() -> Option<bool> {
    INJECTED_ARM_EFFECT_THEN_DEBT.with(|slot| slot.take())
}
#[cfg(test)]
fn inject_terminal_settlement_failure_for_test() {
    INJECTED_TERMINAL_SETTLEMENT_FAILURE.with(|slot| assert!(!slot.replace(true)));
}
#[cfg(test)]
fn take_terminal_settlement_failure_for_test() -> bool {
    INJECTED_TERMINAL_SETTLEMENT_FAILURE.with(|slot| slot.replace(false))
}
#[cfg(test)]
fn inject_task_a_boundary_for_test(boundary: TaskABoundaryV1, outcome: InjectedTaskAOutcomeV1) {
    INJECTED_TASK_A_BOUNDARY.with(|slot| {
        assert!(slot.borrow_mut().replace((boundary, outcome)).is_none());
    });
}
#[cfg(test)]
fn take_task_a_boundary(boundary: TaskABoundaryV1) -> Option<InjectedTaskAOutcomeV1> {
    INJECTED_TASK_A_BOUNDARY.with(|slot| {
        let mut slot = slot.borrow_mut();
        if slot.as_ref().is_some_and(|value| value.0 == boundary) {
            Some(slot.take().expect("matched boundary").1)
        } else {
            None
        }
    })
}
#[cfg(test)]
fn task_a_journal_boundary<T>(
    boundary: TaskABoundaryV1,
    actual: Result<T, JournalMutationOutcomeV2>,
) -> FlightResult<T> {
    use InjectedTaskAOutcomeV1 as I;
    use JournalMutationOutcomeV2 as J;
    match take_task_a_boundary(boundary) {
        None => mutation(actual),
        Some(I::IoUnknown) => Err(fs(FsCustodyError::Io(
            "injected".into(),
            std::io::Error::other("injected"),
        ))),
        Some(value) => mutation(Err(match value {
            I::Refused => J::Refused("injected".into()),
            I::Retained => J::Retained("injected".into()),
            I::Unsupported => J::Unsupported("injected".into()),
            I::ProtectiveDebt => J::ProtectiveDebt("injected".into()),
            I::IoUnknown => unreachable!(),
        })),
    }
}
#[cfg(test)]
fn task_a_transaction_boundary(
    boundary: TaskABoundaryV1,
    actual: NamespaceTransactionOutcomeV2,
) -> FlightResult<()> {
    use InjectedTaskAOutcomeV1 as I;
    use NamespaceTransactionOutcomeV2 as N;
    let Some(injected) = take_task_a_boundary(boundary) else {
        return transaction(actual);
    };
    if matches!(injected, I::IoUnknown) {
        return Err(fs(FsCustodyError::Io(
            "injected".into(),
            std::io::Error::other("injected"),
        )));
    }
    let N::Complete(ticket) = actual else {
        return transaction(actual);
    };
    transaction(match injected {
        I::Refused => N::NoEffect(ticket, "injected".into()),
        I::Retained => N::Retained(ticket, "injected".into()),
        I::Unsupported => N::Unsupported("injected".into()),
        I::ProtectiveDebt => N::ProtectiveDebt("injected".into()),
        I::IoUnknown => unreachable!(),
    })
}
#[cfg(test)]
fn task_a_boundary(boundary: TaskABoundaryV1) -> FlightResult<()> {
    task_a_journal_boundary(boundary, Ok(()))
}
#[cfg(test)]
fn task_a_sync_boundary(
    boundary: TaskABoundaryV1,
    actual: JournalMutationOutcomeV2,
) -> FlightResult<()> {
    use InjectedTaskAOutcomeV1 as I;
    use JournalMutationOutcomeV2 as J;
    match take_task_a_boundary(boundary) {
        None => sync(actual),
        Some(I::IoUnknown) => Err(fs(FsCustodyError::Io(
            "injected".into(),
            std::io::Error::other("injected"),
        ))),
        Some(value) => sync(match value {
            I::Refused => J::Refused("injected".into()),
            I::Retained => J::Retained("injected".into()),
            I::Unsupported => J::Unsupported("injected".into()),
            I::ProtectiveDebt => J::ProtectiveDebt("injected".into()),
            I::IoUnknown => unreachable!(),
        }),
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        fs_custody::{
            required_object_identity_v2, BirthTimeV1, CustodyIntentV2, CustodyOperationKindV2,
            JournalRootBindingV2, ObjectIdentityV2,
        },
        ids::{AttemptId, ExecutionId, NodeId},
        resource_flight::ResourceActionResultV1,
    };
    use std::{
        collections::VecDeque,
        fs,
        fs::File,
        os::unix::fs::MetadataExt as _,
        path::PathBuf,
        sync::{mpsc, Arc, Barrier, Mutex},
        time::Duration,
    };
    #[derive(Default)]
    pub(super) struct TestAckPublisherV1 {
        committed: Mutex<BTreeSet<Vec<u8>>>,
    }
    impl RemoteRequestResultPublisherV1 for TestAckPublisherV1 {
        fn publish_idempotent(
            &self,
            publication: &RemoteRequestTerminalPublicationV1,
        ) -> Result<RemoteRequestDeliveryIdV1, String> {
            self.committed
                .lock()
                .map_err(|_| "test sink lock poisoned".to_owned())?
                .insert(
                    serde_json::to_vec(publication.delivery_id())
                        .map_err(|error| error.to_string())?,
                );
            Ok(publication.delivery_id().clone())
        }
    }
    struct Case {
        _temp: tempfile::TempDir,
        anchor: PathBuf,
        root: PathBuf,
        binding: JournalRootBindingV2,
    }
    fn object(path: &std::path::Path) -> ObjectIdentityV2 {
        let metadata = fs::metadata(path).unwrap();
        required_object_identity_v2(
            metadata.dev(),
            metadata.ino(),
            BirthTimeV1::from_metadata(&metadata),
            "request fixture",
        )
        .unwrap()
    }
    fn case() -> Case {
        let temp = tempfile::tempdir().unwrap();
        let anchor = temp.path().join("anchor");
        let parent = anchor.join("parent");
        let root = parent.join("requests");
        let lock = parent.join("operation.lock");
        fs::create_dir_all(&root).unwrap();
        fs::write(&lock, b"").unwrap();
        let binding = JournalRootBindingV2::new(
            object(&anchor),
            ChildNameV2::from_bytes(b"parent").unwrap(),
            object(&parent),
            ChildNameV2::from_bytes(b"requests").unwrap(),
            object(&root),
            ChildNameV2::from_bytes(b"operation.lock").unwrap(),
            object(&lock),
        )
        .unwrap();
        Case {
            _temp: temp,
            anchor,
            root,
            binding,
        }
    }
    fn attempt() -> AttemptIdentity {
        AttemptIdentity {
            execution_id: ExecutionId::parse(format!("exec-{}", "1".repeat(32))).unwrap(),
            attempt_id: AttemptId::parse(format!("attempt-{}", "2".repeat(32))).unwrap(),
            ordinal: 7,
            parent_attempt_id: None,
        }
    }
    fn foreign_attempt() -> AttemptIdentity {
        AttemptIdentity {
            ordinal: 8,
            ..attempt()
        }
    }
    fn owner(index: usize) -> ResourceFlightOwnerV1 {
        ResourceFlightOwnerV1::new(NodeId::parse("node").unwrap(), format!("owner-{index}"))
            .unwrap()
    }
    fn request(index: usize) -> DedicatedRemoteRequestIdV1 {
        DedicatedRemoteRequestIdV1::parse(format!(
            "{}{:064x}",
            DedicatedRemoteRequestIdV1::PREFIX,
            index + 1
        ))
        .unwrap()
    }
    fn custody(case: &Case) -> JournalRootCustodyV2 {
        JournalRootCustodyV2::open(&case.anchor, &case.binding, "request journal").unwrap()
    }
    fn initialized(case: &Case, cap: usize) -> RemoteRequestJournalV1 {
        RemoteRequestJournalV1::initialize_with_capacity(custody(case), attempt(), cap).unwrap();
        RemoteRequestJournalV1::open_with_capacity(custody(case), attempt(), cap).unwrap()
    }
    fn request_paths(case: &Case) -> Vec<PathBuf> {
        fs::read_dir(&case.root)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|path| {
                let name = path.file_name().unwrap().to_string_lossy().into_owned();
                // Published request children only: reserved-namespace entries
                // (staging/intent/capture residue) are not requests.
                name.contains(REQUEST_CHILD_PREFIX_V1) && !name.starts_with(".a2a-v2-")
            })
            .collect()
    }
    fn root_bytes(case: &Case) -> Vec<(String, Vec<u8>)> {
        let mut entries = fs::read_dir(&case.root)
            .unwrap()
            .map(|entry| {
                let path = entry.unwrap().path();
                (
                    path.file_name().unwrap().to_string_lossy().into_owned(),
                    fs::read(path).unwrap(),
                )
            })
            .collect::<Vec<_>>();
        entries.sort_by(|left, right| left.0.cmp(&right.0));
        entries
    }
    fn unchecked(case: &Case, capacity: usize) -> FlightResult<RemoteRequestJournalV1> {
        RemoteRequestJournalV1::open_recovered(
            custody(case),
            attempt(),
            capacity,
            Arc::new(TestAckPublisherV1::default()),
        )
    }
    fn child(case: &Case) -> (PathBuf, RequestChildWireV1) {
        let path = request_paths(case).pop().unwrap();
        let wire = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        (path, wire)
    }
    fn rewrite_checkpoint(case: &Case, attempt: &AttemptIdentity, next_ordinal: u64) {
        fs::write(
            case.root.join(CHECKPOINT_CHILD_V1),
            serde_json::to_vec(&checkpoint(attempt, next_ordinal)).unwrap(),
        )
        .unwrap();
    }
    fn rewrite_child_ordinal(
        case: &Case,
        path: PathBuf,
        mut wire: RequestChildWireV1,
        ordinal: u64,
    ) {
        wire.ordinal = ordinal;
        wire.checkpoint_digest = checkpoint_digest(&wire.attempt, ordinal);
        wire.authority_digest = authority_digest(&wire);
        let replacement = case
            .root
            .join(request_name(&wire.authority_digest).as_os_str());
        fs::remove_file(path).unwrap();
        fs::write(replacement, serde_json::to_vec(&wire).unwrap()).unwrap();
    }
    fn install_active_children(case: &Case, count: usize) {
        for index in 0..count {
            let ordinal = index as u64;
            let mut wire = RequestChildWireV1 {
                schema: SCHEMA,
                attempt: attempt(),
                ordinal,
                checkpoint_digest: checkpoint_digest(&attempt(), ordinal),
                authority_digest: Sha256HexV1::digest(b"pending"),
                request_id: request(index),
                owner: owner(index),
                status: ChildStateV1::Active {},
            };
            wire.authority_digest = authority_digest(&wire);
            fs::write(
                case.root
                    .join(request_name(&wire.authority_digest).as_os_str()),
                serde_json::to_vec(&wire).unwrap(),
            )
            .unwrap();
        }
        rewrite_checkpoint(case, &attempt(), count as u64);
    }
    fn install_retire_residue(case: &Case, keep_capture: bool) {
        let (path, wire) = child(case);
        let target = request_name(&wire.authority_digest);
        let snapshot =
            required_file_content_snapshot_v2(&File::open(&path).unwrap(), "residue").unwrap();
        let intent = CustodyIntentV2::new(
            CustodyOperationKindV2::Retire,
            target,
            snapshot.object,
            snapshot,
        )
        .unwrap();
        let reserved = ReservedNameNamespaceV2::ALL.map(|namespace| {
            intent
                .reserved_name(namespace)
                .as_os_str()
                .to_str()
                .unwrap()
        });
        fs::write(
            case.root.join(
                intent
                    .reserved_name(ReservedNameNamespaceV2::TransactionIntent)
                    .as_os_str(),
            ),
            serde_json::to_vec(&serde_json::json!({
                "schema": 2,
                "operation": intent.parts().0,
                "target": intent.parts().1.as_os_str().to_str().unwrap(),
                "expected": intent.parts().2,
                "staged": intent.parts().3,
                "staged_sha256": null,
                "reserved": reserved,
            }))
            .unwrap(),
        )
        .unwrap();
        let capture = case.root.join(intent.capture_name().as_os_str());
        fs::rename(path, &capture).unwrap();
        if !keep_capture {
            fs::remove_file(capture).unwrap();
        }
    }
    #[test]
    fn remote_request_flight_checkpoint_and_census_are_strict_and_nonmutating() {
        for corrupt in ["unknown", "schema", "digest", "attempt"] {
            let case = case();
            drop(initialized(&case, 16));
            let path = case.root.join(CHECKPOINT_CHILD_V1);
            let mut value: serde_json::Value =
                serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
            match corrupt {
                "unknown" => value["extra"] = serde_json::json!(true),
                "schema" => value["schema"] = serde_json::json!(99),
                "digest" => value["identity_chain_digest"] = serde_json::json!("0".repeat(64)),
                _ => value["attempt"]["ordinal"] = serde_json::json!(8),
            }
            fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
            let before = fs::read(&path).unwrap();
            assert!(
                RemoteRequestJournalV1::open_with_capacity(custody(&case), attempt(), 16).is_err()
            );
            assert_eq!(fs::read(path).unwrap(), before);
        }
        let legacy = case();
        drop(initialized(&legacy, 16));
        fs::write(
            legacy.root.join("resource-flight-reservation-old.json"),
            b"legacy",
        )
        .unwrap();
        let before = fs::read_dir(&legacy.root).unwrap().count();
        assert!(matches!(
            RemoteRequestJournalV1::open_with_capacity(custody(&legacy), attempt(), 16),
            Err(RemoteRequestFlightRefusalV1::LegacyMigrationRequired)
        ));
        assert_eq!(fs::read_dir(&legacy.root).unwrap().count(), before);
        let over_cap = case();
        drop(initialized(&over_cap, 8));
        for index in 0..8 {
            fs::write(over_cap.root.join(format!("unknown-{index}")), b"x").unwrap();
        }
        let before = fs::read_dir(&over_cap.root).unwrap().count();
        assert!(matches!(
            RemoteRequestJournalV1::open_with_capacity(custody(&over_cap), attempt(), 8),
            Err(RemoteRequestFlightRefusalV1::Capacity)
        ));
        assert_eq!(fs::read_dir(&over_cap.root).unwrap().count(), before);
    }
    #[test]
    fn remote_request_flight_nested_unknown_fields_refuse_before_mint_or_mutation() {
        for child in [false, true] {
            let case = case();
            let mut journal = initialized(&case, 16);
            let path = if child {
                journal.admit_with(owner(0), || Ok(request(0))).unwrap();
                request_paths(&case).pop().unwrap()
            } else {
                case.root.join(CHECKPOINT_CHILD_V1)
            };
            drop(journal);
            let mut value: serde_json::Value =
                serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
            value["attempt"]["extra"] = serde_json::json!(true);
            fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
            let before = root_bytes(&case);
            let mut minted = 0;
            assert!(matches!(
                unchecked(&case, 16).and_then(|mut journal| journal.admit_with(owner(1), || {
                    minted += 1;
                    Ok(request(1))
                })),
                Err(RemoteRequestFlightRefusalV1::Malformed(_))
            ));
            assert_eq!(minted, 0);
            assert_eq!(root_bytes(&case), before);
        }
    }
    #[test]
    fn remote_request_authority_is_borrowed_and_duplicate_mint_refuses_without_mutation() {
        let case = case();
        let mut journal = initialized(&case, 16);
        let authority = journal.admit_with(owner(0), || Ok(request(0))).unwrap();
        let borrowed: &DedicatedRemoteRequestIdV1 = authority.request_id();
        assert_eq!(borrowed, &request(0));
        let before = root_bytes(&case);
        assert!(matches!(
            journal.admit_with(owner(1), || Ok(request(0))),
            Err(RemoteRequestFlightRefusalV1::IdentityCollision)
        ));
        assert_eq!(root_bytes(&case), before);
    }
    #[test]
    fn remote_request_flight_capacity_plus_two_is_capacity_without_mutation() {
        let case = case();
        let mut journal = initialized(&case, 8);
        for index in 0..9 {
            fs::write(case.root.join(format!("unknown-{index}")), b"x").unwrap();
        }
        let before = root_bytes(&case);
        let mut minted = 0;
        assert!(matches!(
            journal.admit_with(owner(0), || {
                minted += 1;
                Ok(request(0))
            }),
            Err(RemoteRequestFlightRefusalV1::Capacity)
        ));
        assert_eq!(minted, 0);
        assert_eq!(root_bytes(&case), before);
    }
    #[test]
    fn remote_request_flight_capacity_counts_permanent_lease_before_mint_or_mutation() {
        let case = case();
        let mut journal = initialized(&case, CAPACITY);
        install_active_children(&case, CAPACITY - 4);
        let mut minted = 0;
        let before = root_bytes(&case);
        assert!(matches!(
            journal.admit_with(owner(CAPACITY), || {
                minted += 1;
                Ok(request(CAPACITY))
            }),
            Err(RemoteRequestFlightRefusalV1::Capacity)
        ));
        assert_eq!(minted, 0);
        assert_eq!(root_bytes(&case), before);
    }
    #[test]
    fn remote_request_flight_interrupted_positive_edge_reopens_with_healing_headroom() {
        let case = case();
        let mut journal = initialized(&case, CAPACITY);
        install_active_children(&case, CAPACITY - ADMISSION_FOOTPRINT - 1);
        assert!(journal
            .admit_with_boundary(owner(CAPACITY), || Ok(request(CAPACITY)), 4)
            .is_err());
        drop(journal);

        drop(
            RemoteRequestJournalV1::open_with_capacity(custody(&case), attempt(), CAPACITY)
                .unwrap(),
        );
    }
    #[test]
    fn remote_request_flight_real_task_a_protective_outcome_is_not_flattened() {
        let case = case();
        let mut journal = initialized(&case, 16);
        let checkpoint_before = fs::read(case.root.join(CHECKPOINT_CHILD_V1)).unwrap();
        assert!(matches!(
            journal.admit_with(owner(0), || {
                let residue = ChildNameV2::reserved(
                    ReservedNameNamespaceV2::ReplacementCapture,
                    &RemoteRequestJournalV1::checkpoint_name(),
                )
                .unwrap();
                fs::write(case.root.join(residue.as_os_str()), b"injected").unwrap();
                Ok(request(0))
            }),
            Err(RemoteRequestFlightRefusalV1::TaskA(
                TaskAProtectiveOutcomeV1::ProtectiveDebt,
                _
            ))
        ));
        assert!(request_paths(&case).is_empty());
        assert_eq!(
            fs::read(case.root.join(CHECKPOINT_CHILD_V1)).unwrap(),
            checkpoint_before
        );
    }
    #[test]
    fn remote_request_flight_crash_cuts_never_create_partial_authority() {
        for boundary in 0..6 {
            let case = case();
            let mut journal = initialized(&case, 16);
            assert!(matches!(
                journal.admit_with_boundary(owner(0), || Ok(request(0)), boundary),
                Err(RemoteRequestFlightRefusalV1::ReopenRequired(_))
            ));
            assert!(journal.requires_reopen());
            for path in request_paths(&case) {
                let bytes = fs::read(path).unwrap();
                assert!(!bytes.is_empty());
                serde_json::from_slice::<RequestChildWireV1>(&bytes).unwrap();
            }
            drop(journal);
            let reopened =
                RemoteRequestJournalV1::open_with_capacity(custody(&case), attempt(), 16);
            if [1, 2].contains(&boundary) {
                assert!(reopened.is_err());
            } else {
                assert!(reopened.is_ok());
            }
        }
    }
    #[test]
    fn remote_request_flight_task_a_outcomes_are_never_success_flattened() {
        for outcome in InjectedTaskAOutcomeV1::PROTECTIVE {
            inject_task_a_boundary_for_test(TaskABoundaryV1::Stage, outcome);
            assert!(task_a_boundary(TaskABoundaryV1::Stage).is_err());
        }
        assert!(task_a_boundary(TaskABoundaryV1::Stage).is_ok());
    }

    #[test]
    fn remote_request_flight_owner_validation_precedes_mint_and_census_is_nonmutating() {
        let invalid = [
            ResourceFlightOwnerV1 {
                node_id: serde_json::from_str(r#""node""#).unwrap(),
                owner_key: String::new(),
            },
            ResourceFlightOwnerV1 {
                node_id: serde_json::from_str(r#""node with space""#).unwrap(),
                owner_key: "owner".into(),
            },
            ResourceFlightOwnerV1 {
                node_id: NodeId::parse("node").unwrap(),
                owner_key: "x".repeat(WIRE_CAP + 1),
            },
            ResourceFlightOwnerV1 {
                node_id: NodeId::parse("node").unwrap(),
                owner_key: "owner\nkey".into(),
            },
        ];
        for owner in invalid {
            let case = case();
            let mut journal = initialized(&case, 16);
            let before = root_bytes(&case);
            let mut minted = 0;
            assert!(matches!(
                journal.admit_with(owner, || {
                    minted += 1;
                    Ok(request(0))
                }),
                Err(RemoteRequestFlightRefusalV1::Malformed(_))
            ));
            assert_eq!(minted, 0);
            assert_eq!(root_bytes(&case), before);
        }

        let case = case();
        let mut journal = initialized(&case, 16);
        journal.admit_with(owner(0), || Ok(request(0))).unwrap();
        drop(journal);
        let (path, mut wire) = child(&case);
        wire.owner.owner_key.clear();
        fs::write(&path, serde_json::to_vec(&wire).unwrap()).unwrap();
        let before = root_bytes(&case);
        assert!(matches!(
            RemoteRequestJournalV1::open_with_capacity(custody(&case), attempt(), 16),
            Err(RemoteRequestFlightRefusalV1::Malformed(_))
        ));
        assert_eq!(root_bytes(&case), before);
    }

    #[test]
    fn remote_request_flight_child_corruption_refuses_without_authority_or_checkpoint_advance() {
        for corrupt in ["schema", "digest", "name"] {
            let case = case();
            let mut journal = initialized(&case, 16);
            journal.admit_with(owner(0), || Ok(request(0))).unwrap();
            drop(journal);
            let checkpoint_before = fs::read(case.root.join(CHECKPOINT_CHILD_V1)).unwrap();
            let (path, mut wire) = child(&case);
            match corrupt {
                "schema" => wire.schema = 99,
                "digest" => wire.authority_digest = Sha256HexV1::digest(b"corrupt"),
                _ => {
                    let wrong = request_name(&Sha256HexV1::digest(b"wrong-name"));
                    fs::rename(&path, case.root.join(wrong.as_os_str())).unwrap();
                }
            }
            if corrupt != "name" {
                fs::write(&path, serde_json::to_vec(&wire).unwrap()).unwrap();
            }
            let before = root_bytes(&case);
            let mut minted = 0;
            assert!(unchecked(&case, 16)
                .and_then(|mut journal| journal.admit_with(owner(1), || {
                    minted += 1;
                    Ok(request(1))
                }))
                .is_err());
            assert_eq!(minted, 0);
            assert_eq!(
                fs::read(case.root.join(CHECKPOINT_CHILD_V1)).unwrap(),
                checkpoint_before
            );
            assert_eq!(root_bytes(&case), before);
        }
    }

    #[test]
    fn remote_request_flight_terminal_state_unknown_fields_refuse_without_mutation() {
        let case = case();
        let mut journal = initialized(&case, 16);
        journal.admit_with(owner(0), || Ok(request(0))).unwrap();
        drop(journal);
        let (path, wire) = child(&case);
        let mut value = serde_json::to_value(wire).unwrap();
        value["status"]["unexpected"] = serde_json::Value::Bool(true);
        fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
        let before = root_bytes(&case);
        let mut minted = 0;
        assert!(unchecked(&case, 16)
            .and_then(|mut journal| journal.admit_with(owner(1), || {
                minted += 1;
                Ok(request(1))
            }))
            .is_err());
        assert_eq!(minted, 0);
        assert_eq!(root_bytes(&case), before);
    }

    #[test]
    fn remote_request_flight_reopen_closes_step_five_orphan_idempotently() {
        {
            let boundary = 4;
            let case = case();
            let mut journal = initialized(&case, 16);
            assert!(journal
                .admit_with_boundary(owner(0), || Ok(request(0)), boundary)
                .is_err());
            drop(journal);

            let reopened =
                RemoteRequestJournalV1::open_with_capacity(custody(&case), attempt(), 16).unwrap();
            let op = reopened
                .custody
                .begin_operation("inspect healed request")
                .unwrap();
            let census = reopened.scan(&op).unwrap();
            assert_eq!(census.checkpoint.next_ordinal, 1);
            assert_eq!(census.children.len(), 1);
            assert_eq!(census.children[0].status, ChildStateV1::PreSendFailure {});
            drop(op);
            drop(reopened);
            let healed = root_bytes(&case);

            drop(
                RemoteRequestJournalV1::open_with_capacity(custody(&case), attempt(), 16).unwrap(),
            );
            assert_eq!(root_bytes(&case), healed);
        }
    }
    #[test]
    fn remote_request_flight_reopen_preserves_issued_and_checkpoint_ambiguous_active_child() {
        for checkpoint_ambiguous in [false, true] {
            let case = case();
            let mut journal = initialized(&case, 16);
            if checkpoint_ambiguous {
                assert!(journal
                    .admit_with_boundary(owner(0), || Ok(request(0)), 5)
                    .is_err());
            } else {
                drop(journal.admit_with(owner(0), || Ok(request(0))).unwrap());
            }
            drop(journal);
            let before = root_bytes(&case);
            let reopened =
                RemoteRequestJournalV1::open_with_capacity(custody(&case), attempt(), 16).unwrap();
            let op = reopened.custody.begin_operation("inspect active").unwrap();
            let census = reopened.scan(&op).unwrap();
            assert_eq!(census.checkpoint.next_ordinal, 1);
            assert_eq!(census.children[0].status, ChildStateV1::Active {});
            drop(op);
            drop(reopened);
            assert_eq!(root_bytes(&case), before);
        }
    }

    #[test]
    fn remote_request_flight_interrupted_orphan_heal_resumes_and_stays_idempotent() {
        let case = case();
        let mut journal = initialized(&case, 16);
        assert!(journal
            .admit_with_boundary(owner(0), || Ok(request(0)), 4)
            .is_err());
        // Construct the relabel-first intermediate: the orphan already closed
        // as a pre-send failure, but the checkpoint has not advanced.
        let op = journal
            .custody
            .begin_operation("construct heal intermediate")
            .unwrap();
        let census = journal.scan(&op).unwrap();
        assert_eq!(census.checkpoint.next_ordinal, 0);
        assert_eq!(census.children[0].status, ChildStateV1::Active {});
        RemoteRequestJournalV1::replace_child(
            &op,
            &census.children[0],
            ChildStateV1::PreSendFailure {},
            "construct heal intermediate",
        )
        .unwrap();
        drop(op);
        drop(journal);

        // Reopen must resume the interrupted heal by advancing the checkpoint
        // past the already-closed orphan instead of refusing.
        let reopened =
            RemoteRequestJournalV1::open_with_capacity(custody(&case), attempt(), 16).unwrap();
        let op = reopened
            .custody
            .begin_operation("inspect resumed heal")
            .unwrap();
        let census = reopened.scan(&op).unwrap();
        assert_eq!(census.checkpoint.next_ordinal, 1);
        assert_eq!(census.children[0].status, ChildStateV1::PreSendFailure {});
        drop(op);
        drop(reopened);
        let healed = root_bytes(&case);
        drop(RemoteRequestJournalV1::open_with_capacity(custody(&case), attempt(), 16).unwrap());
        assert_eq!(root_bytes(&case), healed);
    }

    #[test]
    fn remote_request_flight_heal_checkpoint_seam_runs_the_real_adapter() {
        let case = case();
        let mut journal = initialized(&case, 16);
        assert!(journal
            .admit_with_boundary(owner(0), || Ok(request(0)), 4)
            .is_err());
        drop(journal);
        inject_task_a_boundary_for_test(
            TaskABoundaryV1::HealCheckpoint,
            InjectedTaskAOutcomeV1::IoUnknown,
        );
        assert!(RemoteRequestJournalV1::open_with_capacity(custody(&case), attempt(), 16).is_err());
        // The real adapter executed under injection, so relabel and checkpoint
        // advance are durable and the next reopen is clean.
        let reopened =
            RemoteRequestJournalV1::open_with_capacity(custody(&case), attempt(), 16).unwrap();
        let op = reopened
            .custody
            .begin_operation("inspect seam heal")
            .unwrap();
        let census = reopened.scan(&op).unwrap();
        assert_eq!(census.checkpoint.next_ordinal, 1);
        assert_eq!(census.children[0].status, ChildStateV1::PreSendFailure {});
        drop(op);
        drop(reopened);
    }

    #[test]
    fn remote_request_flight_admission_checkpoint_seam_runs_the_real_adapter() {
        let case = case();
        let mut journal = initialized(&case, 16);
        let checkpoint_before = fs::read(case.root.join(CHECKPOINT_CHILD_V1)).unwrap();
        inject_task_a_boundary_for_test(
            TaskABoundaryV1::Replace,
            InjectedTaskAOutcomeV1::IoUnknown,
        );
        assert!(journal.admit_with(owner(0), || Ok(request(0))).is_err());
        assert_ne!(
            fs::read(case.root.join(CHECKPOINT_CHILD_V1)).unwrap(),
            checkpoint_before,
            "the production checkpoint-advance adapter must execute"
        );
    }

    #[test]
    fn remote_request_flight_reopen_refuses_gapped_multiple_ahead_and_duplicate_censuses() {
        for shape in ["gap", "multiple", "duplicate"] {
            let case = case();
            let mut journal = initialized(&case, 16);
            journal.admit_with(owner(0), || Ok(request(0))).unwrap();
            if shape != "gap" {
                journal.admit_with(owner(1), || Ok(request(1))).unwrap();
            }
            drop(journal);
            match shape {
                "gap" => {
                    let (path, wire) = child(&case);
                    rewrite_child_ordinal(&case, path, wire, 2);
                }
                "multiple" => rewrite_checkpoint(&case, &attempt(), 0),
                _ => {
                    let (path, wire) = request_paths(&case)
                        .into_iter()
                        .map(|path| {
                            let wire = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
                            (path, wire)
                        })
                        .find(|(_, wire): &(PathBuf, RequestChildWireV1)| wire.ordinal == 1)
                        .unwrap();
                    rewrite_child_ordinal(&case, path, wire, 0);
                }
            }
            let before = root_bytes(&case);
            assert!(
                RemoteRequestJournalV1::open_with_capacity(custody(&case), attempt(), 16).is_err()
            );
            assert_eq!(root_bytes(&case), before, "{shape}");
        }
    }

    #[test]
    fn remote_request_flight_foreign_checkpoint_precedes_task_a_recovery() {
        let case = case();
        let foreign = foreign_attempt();
        RemoteRequestJournalV1::initialize_with_capacity(custody(&case), foreign.clone(), 16)
            .unwrap();
        let mut journal =
            RemoteRequestJournalV1::open_with_capacity(custody(&case), foreign.clone(), 16)
                .unwrap();
        journal.admit_with(owner(0), || Ok(request(0))).unwrap();
        drop(journal);
        install_retire_residue(&case, true);
        let before = root_bytes(&case);
        assert!(matches!(
            RemoteRequestJournalV1::open_with_capacity(custody(&case), attempt(), 16),
            Err(RemoteRequestFlightRefusalV1::ForeignAttempt)
        ));
        assert_eq!(root_bytes(&case), before);
    }

    #[test]
    fn remote_request_flight_mid_retire_residue_is_permanently_protective() {
        for cut in ["post-unlink", "post-zero-link"] {
            let case = case();
            let mut journal = initialized(&case, 16);
            journal.admit_with(owner(0), || Ok(request(0))).unwrap();
            drop(journal);
            install_retire_residue(&case, false);
            let before = root_bytes(&case);
            assert!(
                matches!(
                    RemoteRequestJournalV1::open_with_capacity(custody(&case), attempt(), 16),
                    Err(RemoteRequestFlightRefusalV1::TaskA(
                        TaskAProtectiveOutcomeV1::Retained,
                        _
                    ))
                ),
                "{cut}"
            );
            assert_eq!(root_bytes(&case), before, "{cut}");
        }
    }

    #[test]
    fn remote_request_flight_acknowledges_only_complete_and_retirement_frees_capacity() {
        use crate::resource_flight::ResourceActionDispositionV1 as D;

        let case = case();
        let mut journal = initialized(&case, 5);
        for index in 0..8 {
            let authority = journal
                .admit_with(owner(index), || Ok(request(index)))
                .unwrap();
            for refused in [D::Partial, D::Failed, D::Unknown, D::NotNeeded] {
                let before = root_bytes(&case);
                assert!(matches!(
                    journal.acknowledge(&authority, refused),
                    Err(RemoteRequestFlightRefusalV1::TerminalNotComplete)
                ));
                assert_eq!(root_bytes(&case), before);
            }
            journal.acknowledge(&authority, D::Complete).unwrap();
            assert!(matches!(
                child(&case).1.status,
                ChildStateV1::PublicationAcknowledged { .. }
            ));
            journal.retire(&authority).unwrap();
            assert!(request_paths(&case).is_empty());
            let op = journal
                .custody
                .begin_operation("inspect sequential checkpoint")
                .unwrap();
            assert_eq!(
                journal.scan(&op).unwrap().checkpoint.next_ordinal,
                index as u64 + 1
            );
        }
    }

    #[test]
    fn remote_request_flight_ack_before_unlink_and_after_unlink_reopen_self_heal() {
        use crate::resource_flight::ResourceActionDispositionV1 as D;

        let before_unlink = case();
        let mut journal = initialized(&before_unlink, 8);
        let authority = journal.admit_with(owner(0), || Ok(request(0))).unwrap();
        journal.acknowledge(&authority, D::Complete).unwrap();
        drop(journal);
        drop(
            RemoteRequestJournalV1::open_with_capacity(custody(&before_unlink), attempt(), 8)
                .unwrap(),
        );
        assert!(request_paths(&before_unlink).is_empty());

        for boundary in [1, 2] {
            let after_unlink = case();
            let mut journal = initialized(&after_unlink, 8);
            let authority = journal.admit_with(owner(0), || Ok(request(0))).unwrap();
            journal.acknowledge(&authority, D::Complete).unwrap();
            assert!(journal.retire_with_boundary(&authority, boundary).is_err());
            drop(journal);
            assert!(request_paths(&after_unlink).is_empty());
            drop(
                RemoteRequestJournalV1::open_with_capacity(custody(&after_unlink), attempt(), 8)
                    .unwrap(),
            );
            assert!(request_paths(&after_unlink).is_empty());
        }
    }

    #[test]
    fn remote_request_flight_real_task_a_boundaries_keep_typed_outcomes() {
        use crate::resource_flight::ResourceActionDispositionV1 as D;

        let outcomes = [
            (
                InjectedTaskAOutcomeV1::Refused,
                TaskAProtectiveOutcomeV1::Refused,
            ),
            (
                InjectedTaskAOutcomeV1::Retained,
                TaskAProtectiveOutcomeV1::Retained,
            ),
            (
                InjectedTaskAOutcomeV1::IoUnknown,
                TaskAProtectiveOutcomeV1::Unknown,
            ),
            (
                InjectedTaskAOutcomeV1::Unsupported,
                TaskAProtectiveOutcomeV1::Unsupported,
            ),
            (
                InjectedTaskAOutcomeV1::ProtectiveDebt,
                TaskAProtectiveOutcomeV1::ProtectiveDebt,
            ),
        ];
        for (outcome, expected) in outcomes {
            let stage_case = case();
            let mut stage_journal = initialized(&stage_case, 16);
            inject_task_a_boundary_for_test(TaskABoundaryV1::Stage, outcome);
            assert!(matches!(
                stage_journal.admit_with(owner(0), || Ok(request(0))),
                Err(RemoteRequestFlightRefusalV1::TaskA(kind, _)) if kind == expected
            ));
            assert!(request_paths(&stage_case).is_empty());
            assert!(
                std::fs::read_dir(&stage_case.root)
                    .unwrap()
                    .any(|entry| entry
                        .unwrap()
                        .file_name()
                        .to_string_lossy()
                        .starts_with(".a2a-v2-stg-")),
                "the production stage adapter must execute"
            );

            let publish_case = case();
            let mut publish_journal = initialized(&publish_case, 16);
            inject_task_a_boundary_for_test(TaskABoundaryV1::Publish, outcome);
            assert!(matches!(
                publish_journal.admit_with(owner(0), || Ok(request(0))),
                Err(RemoteRequestFlightRefusalV1::TaskA(kind, _)) if kind == expected
            ));
            assert_eq!(
                request_paths(&publish_case).len(),
                1,
                "the production publish adapter must execute"
            );

            let acknowledge_case = case();
            let mut acknowledge_journal = initialized(&acknowledge_case, 16);
            let authority = acknowledge_journal
                .admit_with(owner(0), || Ok(request(0)))
                .unwrap();
            let before = root_bytes(&acknowledge_case);
            inject_task_a_boundary_for_test(TaskABoundaryV1::Replace, outcome);
            assert!(matches!(
                acknowledge_journal.acknowledge(&authority, D::Complete),
                Err(RemoteRequestFlightRefusalV1::TaskA(kind, _)) if kind == expected
            ));
            assert_ne!(
                root_bytes(&acknowledge_case),
                before,
                "the production replace adapter must execute"
            );

            let retire_case = case();
            let mut retire_journal = initialized(&retire_case, 16);
            let authority = retire_journal
                .admit_with(owner(0), || Ok(request(0)))
                .unwrap();
            retire_journal.acknowledge(&authority, D::Complete).unwrap();
            let before = root_bytes(&retire_case);
            inject_task_a_boundary_for_test(TaskABoundaryV1::Retire, outcome);
            assert!(matches!(
                retire_journal.retire(&authority),
                Err(RemoteRequestFlightRefusalV1::TaskA(kind, _)) if kind == expected
            ));
            assert_ne!(root_bytes(&retire_case), before);
            assert!(request_paths(&retire_case).is_empty());
        }
    }

    #[derive(Clone, Copy)]
    enum PublisherReply {
        Echo,
        Mismatch,
        Refuse,
    }

    #[derive(Default)]
    struct RecordingPublisher {
        replies: Mutex<VecDeque<PublisherReply>>,
        calls: Mutex<Vec<RemoteRequestTerminalPublicationV1>>,
        committed: Mutex<BTreeSet<Vec<u8>>>,
    }

    impl RecordingPublisher {
        fn with_replies(replies: impl IntoIterator<Item = PublisherReply>) -> Arc<Self> {
            Arc::new(Self {
                replies: Mutex::new(replies.into_iter().collect()),
                ..Self::default()
            })
        }

        fn calls(&self) -> Vec<RemoteRequestTerminalPublicationV1> {
            self.calls.lock().unwrap().clone()
        }

        fn committed_len(&self) -> usize {
            self.committed.lock().unwrap().len()
        }
    }

    impl RemoteRequestResultPublisherV1 for RecordingPublisher {
        fn publish_idempotent(
            &self,
            publication: &RemoteRequestTerminalPublicationV1,
        ) -> Result<RemoteRequestDeliveryIdV1, String> {
            self.calls.lock().unwrap().push(publication.clone());
            self.committed
                .lock()
                .unwrap()
                .insert(serde_json::to_vec(publication.delivery_id()).unwrap());
            match self
                .replies
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or(PublisherReply::Echo)
            {
                PublisherReply::Echo => Ok(publication.delivery_id().clone()),
                PublisherReply::Mismatch => {
                    let mut mismatched = publication.delivery_id().clone();
                    mismatched.ordinal = mismatched.ordinal.checked_add(1).unwrap();
                    Ok(mismatched)
                }
                PublisherReply::Refuse => Err("injected publication refusal".into()),
            }
        }
    }

    struct BarrierPublisher {
        reply: PublisherReply,
        calls: std::sync::atomic::AtomicUsize,
        entered: Barrier,
        release: Barrier,
    }

    impl BarrierPublisher {
        fn new(reply: PublisherReply) -> Arc<Self> {
            Arc::new(Self {
                reply,
                calls: std::sync::atomic::AtomicUsize::new(0),
                entered: Barrier::new(2),
                release: Barrier::new(2),
            })
        }

        fn wait_until_entered(&self) {
            self.entered.wait();
        }

        fn release(&self) {
            self.release.wait();
        }

        fn calls(&self) -> usize {
            self.calls.load(Ordering::Acquire)
        }
    }

    impl RemoteRequestResultPublisherV1 for BarrierPublisher {
        fn publish_idempotent(
            &self,
            publication: &RemoteRequestTerminalPublicationV1,
        ) -> Result<RemoteRequestDeliveryIdV1, String> {
            if self.calls.fetch_add(1, Ordering::AcqRel) == 0 {
                self.entered.wait();
                self.release.wait();
            }
            match self.reply {
                PublisherReply::Echo => Ok(publication.delivery_id().clone()),
                PublisherReply::Refuse => Err("barrier publication refusal".into()),
                PublisherReply::Mismatch => unreachable!(),
            }
        }
    }

    fn terminal(disposition: ResourceActionDispositionV1) -> ResourceActionResultV1 {
        ResourceActionResultV1 {
            disposition,
            duration_ms: 0,
            recovery_owner: None,
            cause: None,
        }
    }

    fn initialize_root(case: &Case, attempt: AttemptIdentity, capacity: usize) {
        RemoteRequestJournalV1::initialize_with_capacity(custody(case), attempt, capacity).unwrap();
    }

    fn open_recovered(
        case: &Case,
        attempt: AttemptIdentity,
        capacity: usize,
        publisher: Arc<RecordingPublisher>,
    ) -> FlightResult<RemoteRequestJournalV1> {
        RemoteRequestJournalV1::open_recovered(custody(case), attempt, capacity, publisher)
    }

    #[test]
    fn remote_request_flight_invalid_row_refuses_before_any_recovery() {
        // An independently corrupt request row must refuse the whole attempt
        // BEFORE recovery may touch Task A residue: with both present, the
        // refusal is the corrupt row's Malformed, not recovery's protective
        // classification, and every root byte is preserved.
        let make_case = case;
        let case = case();
        let mut journal = initialized(&case, 16);
        drop(journal.admit_with(owner(0), || Ok(request(0))).unwrap());
        drop(journal.admit_with(owner(1), || Ok(request(1))).unwrap());
        drop(journal);

        let custody_handle = custody(&case);
        let op = custody_handle
            .begin_operation("construct stage residue")
            .unwrap();
        op.stage(
            &ChildNameV2::from_bytes(b"seed").unwrap(),
            b"residue",
            "construct stage residue",
        )
        .unwrap();
        drop(op);
        drop(custody_handle);
        let paths = request_paths(&case);
        assert_eq!(paths.len(), 2);
        fs::write(&paths[1], b"junk").unwrap();

        let before = root_bytes(&case);
        let publisher = RecordingPublisher::with_replies([]);
        let outcome = open_recovered(&case, attempt(), 16, publisher).err();
        assert!(
            matches!(outcome, Some(RemoteRequestFlightRefusalV1::Malformed(_))),
            "{outcome:?}"
        );
        assert_eq!(
            root_bytes(&case),
            before,
            "an invalid attempt must refuse byte-preserved before recovery"
        );

        // Without the corrupt sibling, the same residue still surfaces the
        // recovery-side protective classification after validation passes.
        let clean = make_case();
        let mut journal = initialized(&clean, 16);
        drop(journal.admit_with(owner(0), || Ok(request(0))).unwrap());
        drop(journal);
        let custody_handle = custody(&clean);
        let op = custody_handle
            .begin_operation("construct stage residue")
            .unwrap();
        op.stage(
            &ChildNameV2::from_bytes(b"seed").unwrap(),
            b"residue",
            "construct stage residue",
        )
        .unwrap();
        drop(op);
        drop(custody_handle);
        let publisher = RecordingPublisher::with_replies([]);
        let outcome = open_recovered(&clean, attempt(), 16, publisher).err();
        assert!(
            matches!(
                outcome,
                Some(RemoteRequestFlightRefusalV1::TaskA(
                    TaskAProtectiveOutcomeV1::ProtectiveDebt,
                    _
                )) | Some(RemoteRequestFlightRefusalV1::ReopenRequired(_))
            ),
            "{outcome:?}"
        );
    }

    #[test]
    fn remote_request_flight_task_c_recovers_every_durable_prefix_without_resend() {
        use ResourceActionDispositionV1 as D;

        for prefix in 0..5 {
            let case = case();
            let publisher = RecordingPublisher::with_replies([]);
            initialize_root(&case, attempt(), 16);
            let mut journal = open_recovered(&case, attempt(), 16, publisher.clone()).unwrap();
            let authority = journal.admit_with(owner(0), || Ok(request(0))).unwrap();
            if prefix >= 1 {
                journal.record_intent(&authority).unwrap();
            }
            if prefix >= 2 {
                journal.authorize_dispatch(&authority).unwrap();
            }
            if prefix >= 3 {
                journal.arm_provider_send(&authority).unwrap();
            }
            if prefix == 4 {
                assert!(journal
                    .settle_with_boundary(&authority, terminal(D::Partial), true, 0)
                    .is_err());
            }
            drop(journal);

            drop(open_recovered(&case, attempt(), 16, publisher.clone()).unwrap());
            let calls = publisher.calls();
            assert_eq!(calls.len(), 1, "prefix {prefix}");
            let expected = match prefix {
                0..=2 => (D::Failed, false),
                3 => (D::Unknown, true),
                _ => (D::Partial, true),
            };
            assert_eq!(
                (
                    calls[0].result().disposition.clone(),
                    calls[0].prompt_may_have_been_accepted()
                ),
                expected,
                "prefix {prefix}"
            );
            assert!(request_paths(&case).is_empty(), "prefix {prefix}");

            drop(open_recovered(&case, attempt(), 16, publisher.clone()).unwrap());
            assert_eq!(publisher.calls().len(), 1, "prefix {prefix} replayed");
        }
    }

    #[test]
    fn remote_request_flight_task_c_recovers_pre_send_failure_without_acceptance() {
        use ResourceActionDispositionV1 as D;

        let case = case();
        let publisher = RecordingPublisher::with_replies([]);
        initialize_root(&case, attempt(), 16);
        let mut journal =
            RemoteRequestJournalV1::open_with_capacity(custody(&case), attempt(), 16).unwrap();
        assert!(journal
            .admit_with_boundary(owner(0), || Ok(request(0)), 4)
            .is_err());
        drop(journal);
        drop(RemoteRequestJournalV1::open_with_capacity(custody(&case), attempt(), 16).unwrap());
        assert_eq!(child(&case).1.status, ChildStateV1::PreSendFailure {});

        drop(open_recovered(&case, attempt(), 16, publisher.clone()).unwrap());
        let calls = publisher.calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].result().disposition, D::Failed);
        assert!(!calls[0].prompt_may_have_been_accepted());
        assert!(request_paths(&case).is_empty());
    }

    #[test]
    fn remote_request_flight_task_c_attempt_lease_excludes_and_releases() {
        let case = case();
        let publisher = RecordingPublisher::with_replies([]);
        initialize_root(&case, attempt(), 16);
        let first = open_recovered(&case, attempt(), 16, publisher.clone()).unwrap();
        let before = root_bytes(&case);
        assert!(matches!(
            open_recovered(&case, attempt(), 16, publisher.clone()),
            Err(RemoteRequestFlightRefusalV1::AttemptLive)
        ));
        assert_eq!(root_bytes(&case), before);
        drop(first);
        drop(open_recovered(&case, attempt(), 16, publisher).unwrap());
    }

    #[test]
    fn remote_request_flight_task_c_attempt_lease_precedes_contended_operation() {
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::mpsc;
        use std::time::Duration;

        let case = case();
        let publisher = RecordingPublisher::with_replies([]);
        initialize_root(&case, attempt(), 16);
        let journal = open_recovered(&case, attempt(), 16, publisher.clone()).unwrap();
        let operation_held = Arc::new(AtomicBool::new(false));
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let thread_holding = Arc::clone(&operation_held);
        let thread = std::thread::spawn(move || {
            let mut journal = journal;
            let admitted = journal.admit_with(owner(0), || {
                thread_holding.store(true, Ordering::SeqCst);
                entered_tx.send(()).unwrap();
                release_rx.recv_timeout(Duration::from_secs(2)).unwrap();
                Ok(request(0))
            });
            (admitted, journal)
        });
        entered_rx.recv_timeout(Duration::from_secs(2)).unwrap();

        let before = root_bytes(&case);
        assert!(matches!(
            open_recovered(&case, attempt(), 16, publisher.clone()),
            Err(RemoteRequestFlightRefusalV1::AttemptLive)
        ));
        assert!(
            operation_held.load(Ordering::SeqCst),
            "the lease flock must answer while the first Task A operation is still held"
        );
        assert_eq!(root_bytes(&case), before);

        release_tx.send(()).unwrap();
        let (admitted, journal) = thread.join().unwrap();
        admitted.unwrap();
        drop(journal);
        drop(open_recovered(&case, attempt(), 16, publisher).unwrap());
    }

    #[test]
    fn remote_request_flight_task_c_old_or_corrupt_lease_is_nonmutating() {
        for nonempty in [false, true] {
            let case = case();
            initialize_root(&case, attempt(), 16);
            let lease = case.root.join(ATTEMPT_LEASE_CHILD_V1);
            if nonempty {
                fs::write(&lease, b"corrupt").unwrap();
            } else {
                fs::remove_file(&lease).unwrap();
            }
            let before = root_bytes(&case);
            assert!(
                open_recovered(&case, attempt(), 16, RecordingPublisher::with_replies([]),)
                    .is_err()
            );
            assert_eq!(root_bytes(&case), before);
        }
    }

    #[test]
    fn remote_request_flight_task_c_pending_outbox_replays_until_exact_ack() {
        use ResourceActionDispositionV1 as D;

        for first_reply in [PublisherReply::Refuse, PublisherReply::Mismatch] {
            let case = case();
            let publisher = RecordingPublisher::with_replies([first_reply, PublisherReply::Echo]);
            initialize_root(&case, attempt(), 16);
            let mut journal = open_recovered(&case, attempt(), 16, publisher.clone()).unwrap();
            let authority = journal.admit_with(owner(0), || Ok(request(0))).unwrap();
            journal.record_intent(&authority).unwrap();
            journal.authorize_dispatch(&authority).unwrap();
            journal.arm_provider_send(&authority).unwrap();
            assert!(journal
                .settle_with_boundary(&authority, terminal(D::Complete), true, 0)
                .is_err());
            drop(journal);

            let first = open_recovered(&case, attempt(), 16, publisher.clone());
            match first_reply {
                PublisherReply::Refuse => assert!(matches!(
                    first,
                    Err(RemoteRequestFlightRefusalV1::PublicationRefused(ref reason))
                        if reason == "injected publication refusal"
                )),
                PublisherReply::Mismatch => assert!(matches!(
                    first,
                    Err(RemoteRequestFlightRefusalV1::PublicationAcknowledgementMismatch)
                )),
                PublisherReply::Echo => unreachable!(),
            }
            assert!(child(&case).1.status.is_terminal_pending());
            drop(open_recovered(&case, attempt(), 16, publisher.clone()).unwrap());
            assert_eq!(publisher.calls().len(), 2);
            assert_eq!(publisher.committed_len(), 1);
            assert!(request_paths(&case).is_empty());
        }

        let case = case();
        let publisher = RecordingPublisher::with_replies([]);
        initialize_root(&case, attempt(), 16);
        let mut journal = open_recovered(&case, attempt(), 16, publisher.clone()).unwrap();
        let authority = journal.admit_with(owner(0), || Ok(request(0))).unwrap();
        assert!(journal
            .settle_with_boundary(&authority, terminal(D::Failed), false, 1)
            .is_err());
        drop(journal);
        assert_eq!(publisher.calls().len(), 1);
        drop(open_recovered(&case, attempt(), 16, publisher.clone()).unwrap());
        assert_eq!(publisher.calls().len(), 1);
        assert!(request_paths(&case).is_empty());
    }

    #[test]
    fn remote_request_flight_task_c_invalid_transition_is_nonmutating() {
        let case = case();
        let publisher = RecordingPublisher::with_replies([]);
        initialize_root(&case, attempt(), 16);
        let mut journal = open_recovered(&case, attempt(), 16, publisher).unwrap();
        let authority = journal.admit_with(owner(0), || Ok(request(0))).unwrap();
        let before = root_bytes(&case);
        assert!(matches!(
            journal.arm_provider_send(&authority),
            Err(RemoteRequestFlightRefusalV1::InvalidStateTransition(_))
        ));
        assert_eq!(root_bytes(&case), before);
    }

    #[test]
    fn remote_request_flight_task_c_pending_row_is_strict_and_nonmutating() {
        use ResourceActionDispositionV1 as D;

        let case = case();
        let publisher = RecordingPublisher::with_replies([]);
        initialize_root(&case, attempt(), 16);
        let mut journal = open_recovered(&case, attempt(), 16, publisher.clone()).unwrap();
        let authority = journal.admit_with(owner(0), || Ok(request(0))).unwrap();
        assert!(journal
            .settle_with_boundary(&authority, terminal(D::Failed), false, 0)
            .is_err());
        drop(journal);

        let (path, wire) = child(&case);
        let mut value = serde_json::to_value(wire).unwrap();
        value["status"]["unexpected"] = serde_json::Value::Bool(true);
        fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
        let before = root_bytes(&case);
        assert!(open_recovered(&case, attempt(), 16, publisher).is_err());
        assert_eq!(root_bytes(&case), before);
    }

    #[test]
    fn remote_request_flight_task_c_full_binding_never_aliases_attempts() {
        let left = case();
        let right = case();
        let publisher = RecordingPublisher::with_replies([]);
        initialize_root(&left, attempt(), 16);
        initialize_root(&right, foreign_attempt(), 16);
        let mut left_journal = open_recovered(&left, attempt(), 16, publisher.clone()).unwrap();
        let mut right_journal =
            open_recovered(&right, foreign_attempt(), 16, publisher.clone()).unwrap();
        let left_authority = left_journal
            .admit_with(owner(0), || Ok(request(0)))
            .unwrap();
        let right_authority = right_journal
            .admit_with(owner(0), || Ok(request(0)))
            .unwrap();
        assert_eq!(left_authority.request_id, right_authority.request_id);
        assert_ne!(left_authority.attempt, right_authority.attempt);
        assert_ne!(left_authority.delivery_id(), right_authority.delivery_id());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn remote_request_flight_task_d_first_poll_arms_before_inner_poll() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        let case = case();
        let publisher = RecordingPublisher::with_replies([]);
        initialize_root(&case, attempt(), 16);
        let driver =
            RemoteRequestDriverV1::open_recovered(custody(&case), attempt(), 16, publisher)
                .unwrap();
        let request = driver.admit(owner(0)).unwrap();
        request.journal_intent().unwrap();
        request.authorize_dispatch().unwrap();

        let polls = Arc::new(AtomicUsize::new(0));
        let inner_polls = Arc::clone(&polls);
        let send = request.arm_provider_send(std::future::poll_fn(|_| {
            inner_polls.fetch_add(1, Ordering::SeqCst);
            assert_eq!(child(&case).1.status, ChildStateV1::ProviderSendArmed {});
            std::task::Poll::Ready(41)
        }));
        assert_eq!(send.await.unwrap(), 41);
        assert_eq!(polls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn remote_request_flight_stale_unarmed_settlement_cannot_consume_an_armed_row() {
        // The exact arm/atomic handoff window: the durable armed row lands
        // while this handle's acceptance atomic still reads false. An
        // ordinary settlement using that stale flag must refuse instead of
        // durably publishing accepted=false over a possibly accepted send.
        let case = case();
        let publisher = RecordingPublisher::with_replies([]);
        initialize_root(&case, attempt(), 16);
        let driver =
            RemoteRequestDriverV1::open_recovered(custody(&case), attempt(), 16, publisher.clone())
                .unwrap();
        let request = driver.admit(owner(0)).unwrap();
        request.journal_intent().unwrap();
        request.authorize_dispatch().unwrap();
        request
            .lock()
            .unwrap()
            .arm_provider_send(&request.authority)
            .unwrap();
        let outcome = request.settle(failed_before_send()).err();
        assert!(
            matches!(
                outcome,
                Some(RemoteRequestFlightRefusalV1::InvalidStateTransition(_))
            ),
            "{outcome:?}"
        );
        assert!(
            publisher.calls().is_empty(),
            "no unaccepted terminal may publish over an armed row"
        );
        request.crash_without_settlement_for_test();
    }

    #[tokio::test(flavor = "current_thread")]
    async fn remote_request_flight_task_d_failed_arm_never_polls_and_settles_pre_send() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        let case = case();
        let publisher = RecordingPublisher::with_replies([]);
        initialize_root(&case, attempt(), 16);
        let driver =
            RemoteRequestDriverV1::open_recovered(custody(&case), attempt(), 16, publisher.clone())
                .unwrap();
        let request = driver.admit(owner(0)).unwrap();
        request.journal_intent().unwrap();
        request.authorize_dispatch().unwrap();

        let polls = Arc::new(AtomicUsize::new(0));
        let inner_polls = Arc::clone(&polls);
        inject_task_a_boundary_for_test(TaskABoundaryV1::Replace, InjectedTaskAOutcomeV1::Refused);
        let result = request
            .arm_provider_send(std::future::poll_fn(|_| {
                inner_polls.fetch_add(1, Ordering::SeqCst);
                std::task::Poll::Ready(())
            }))
            .await;
        assert!(matches!(
            result,
            Err(RemoteRequestFlightRefusalV1::TaskA(
                TaskAProtectiveOutcomeV1::Refused,
                _
            ))
        ));
        assert_eq!(polls.load(Ordering::SeqCst), 0);
        let calls = publisher.calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(
            calls[0].result().disposition,
            ResourceActionDispositionV1::Failed
        );
        assert!(!calls[0].prompt_may_have_been_accepted());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn remote_request_flight_task_d_duplicate_send_wrapper_has_zero_row_effect() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        let case = case();
        let publisher = RecordingPublisher::with_replies([]);
        initialize_root(&case, attempt(), 16);
        let driver =
            RemoteRequestDriverV1::open_recovered(custody(&case), attempt(), 16, publisher.clone())
                .unwrap();
        let request = driver.admit(owner(0)).unwrap();
        request.journal_intent().unwrap();
        request.authorize_dispatch().unwrap();

        let mut first =
            Box::pin(request.arm_provider_send(std::future::poll_fn(|_| Poll::<()>::Pending)));
        assert!(matches!(futures::poll!(first.as_mut()), Poll::Pending));
        let second_polls = Arc::new(AtomicUsize::new(0));
        let polled = Arc::clone(&second_polls);
        let mut second = Box::pin(request.arm_provider_send(std::future::poll_fn(move |_| {
            polled.fetch_add(1, Ordering::SeqCst);
            Poll::Ready(())
        })));
        let refusal = match futures::poll!(second.as_mut()) {
            Poll::Ready(Err(error)) => error,
            other => panic!("duplicate wrapper must refuse, got {other:?}"),
        };
        assert!(matches!(
            refusal,
            RemoteRequestFlightRefusalV1::InvalidStateTransition(_)
        ));
        assert_eq!(second_polls.load(Ordering::SeqCst), 0);
        assert!(publisher.calls().is_empty());
        assert_eq!(child(&case).1.status, ChildStateV1::ProviderSendArmed {});

        drop(second);
        drop(first);
        let post_drop_polls = Arc::new(AtomicUsize::new(0));
        let polled = Arc::clone(&post_drop_polls);
        let mut after_drop = Box::pin(request.arm_provider_send(std::future::poll_fn(move |_| {
            polled.fetch_add(1, Ordering::SeqCst);
            Poll::Ready(())
        })));
        assert!(matches!(
            futures::poll!(after_drop.as_mut()),
            Poll::Ready(Err(RemoteRequestFlightRefusalV1::InvalidStateTransition(_)))
        ));
        assert_eq!(post_drop_polls.load(Ordering::SeqCst), 0);
        assert!(publisher.calls().is_empty());
        assert_eq!(child(&case).1.status, ChildStateV1::ProviderSendArmed {});
        drop(after_drop);
        request.crash_without_settlement_for_test();
        drop(driver);
        drop(
            RemoteRequestDriverV1::open_recovered(custody(&case), attempt(), 16, publisher.clone())
                .unwrap(),
        );
        let calls = publisher.calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(
            calls[0].result().disposition,
            ResourceActionDispositionV1::Unknown
        );
        assert!(calls[0].prompt_may_have_been_accepted());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn remote_request_flight_task_d_effect_then_debt_recovers_failed_unaccepted() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        let case = case();
        let publisher =
            RecordingPublisher::with_replies([PublisherReply::Refuse, PublisherReply::Echo]);
        initialize_root(&case, attempt(), 16);
        let driver =
            RemoteRequestDriverV1::open_recovered(custody(&case), attempt(), 16, publisher.clone())
                .unwrap();
        let request = driver.admit(owner(0)).unwrap();
        request.journal_intent().unwrap();
        request.authorize_dispatch().unwrap();

        let polls = Arc::new(AtomicUsize::new(0));
        let inner_polls = Arc::clone(&polls);
        inject_arm_effect_then_debt_for_test(false);
        let result = request
            .arm_provider_send(std::future::poll_fn(|_| {
                inner_polls.fetch_add(1, Ordering::SeqCst);
                std::task::Poll::Ready(())
            }))
            .await;
        assert!(matches!(
            result,
            Err(RemoteRequestFlightRefusalV1::TaskA(
                TaskAProtectiveOutcomeV1::ProtectiveDebt,
                _
            ))
        ));
        assert_eq!(polls.load(Ordering::SeqCst), 0);
        drop(request);
        drop(driver);

        drop(
            RemoteRequestDriverV1::open_recovered(custody(&case), attempt(), 16, publisher.clone())
                .unwrap(),
        );
        let calls = publisher.calls();
        assert_eq!(calls.len(), 2);
        assert!(calls.iter().all(|call| {
            call.result().disposition == ResourceActionDispositionV1::Failed
                && !call.prompt_may_have_been_accepted()
        }));
        assert!(request_paths(&case).is_empty());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn remote_request_flight_task_d_failed_terminal_after_debt_recovers_unknown_accepted() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        let case = case();
        let publisher = RecordingPublisher::with_replies([]);
        initialize_root(&case, attempt(), 16);
        let driver =
            RemoteRequestDriverV1::open_recovered(custody(&case), attempt(), 16, publisher.clone())
                .unwrap();
        let request = driver.admit(owner(0)).unwrap();
        request.journal_intent().unwrap();
        request.authorize_dispatch().unwrap();

        let polls = Arc::new(AtomicUsize::new(0));
        let inner_polls = Arc::clone(&polls);
        inject_arm_effect_then_debt_for_test(true);
        let result = request
            .arm_provider_send(std::future::poll_fn(|_| {
                inner_polls.fetch_add(1, Ordering::SeqCst);
                std::task::Poll::Ready(())
            }))
            .await;
        assert!(matches!(
            result,
            Err(RemoteRequestFlightRefusalV1::TaskA(
                TaskAProtectiveOutcomeV1::ProtectiveDebt,
                _
            ))
        ));
        assert_eq!(polls.load(Ordering::SeqCst), 0);
        assert!(!take_terminal_settlement_failure_for_test());
        assert!(publisher.calls().is_empty());
        drop(request);
        drop(driver);

        drop(
            RemoteRequestDriverV1::open_recovered(custody(&case), attempt(), 16, publisher.clone())
                .unwrap(),
        );
        let calls = publisher.calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(
            calls[0].result().disposition,
            ResourceActionDispositionV1::Unknown
        );
        assert!(calls[0].prompt_may_have_been_accepted());
        assert!(request_paths(&case).is_empty());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn remote_request_flight_task_d_pre_and_post_arm_crash_recovery() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        for prefix in 1..=3 {
            let case = case();
            let publisher = RecordingPublisher::with_replies([]);
            initialize_root(&case, attempt(), 16);
            let driver = RemoteRequestDriverV1::open_recovered(
                custody(&case),
                attempt(),
                16,
                publisher.clone(),
            )
            .unwrap();
            let request = driver.admit(owner(0)).unwrap();
            request.journal_intent().unwrap();
            if prefix >= 2 {
                request.authorize_dispatch().unwrap();
            }
            let polls = Arc::new(AtomicUsize::new(0));
            if prefix == 2 {
                let inner_polls = Arc::clone(&polls);
                let send = request.arm_provider_send(std::future::poll_fn(|_| {
                    inner_polls.fetch_add(1, Ordering::SeqCst);
                    std::task::Poll::Ready(())
                }));
                drop(send);
            }
            if prefix == 3 {
                let inner_polls = Arc::clone(&polls);
                let mut send = Box::pin(request.arm_provider_send(std::future::poll_fn(|_| {
                    inner_polls.fetch_add(1, Ordering::SeqCst);
                    assert_eq!(child(&case).1.status, ChildStateV1::ProviderSendArmed {});
                    std::task::Poll::<()>::Pending
                })));
                assert!(matches!(
                    futures::poll!(send.as_mut()),
                    std::task::Poll::Pending
                ));
                drop(send);
            }
            request.crash_without_settlement_for_test();
            drop(driver);

            drop(
                RemoteRequestDriverV1::open_recovered(
                    custody(&case),
                    attempt(),
                    16,
                    publisher.clone(),
                )
                .unwrap(),
            );
            let calls = publisher.calls();
            assert_eq!(calls.len(), 1, "prefix {prefix}");
            let expected = if prefix == 3 {
                (ResourceActionDispositionV1::Unknown, true)
            } else {
                (ResourceActionDispositionV1::Failed, false)
            };
            assert_eq!(
                (
                    calls[0].result().disposition.clone(),
                    calls[0].prompt_may_have_been_accepted()
                ),
                expected,
                "prefix {prefix}"
            );
            assert_eq!(polls.load(Ordering::SeqCst), usize::from(prefix == 3));
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn remote_request_flight_task_d_peer_control_and_racing_settlement_are_bound() {
        let case = case();
        let publisher = RecordingPublisher::with_replies([]);
        initialize_root(&case, attempt(), 16);
        let driver =
            RemoteRequestDriverV1::open_recovered(custody(&case), attempt(), 16, publisher.clone())
                .unwrap();
        let left = driver.admit(owner(0)).unwrap();
        let right = driver.admit(owner(1)).unwrap();
        let mut right_observer = right.observer();
        left.journal_intent().unwrap();
        left.authorize_dispatch().unwrap();
        right.journal_intent().unwrap();
        right.authorize_dispatch().unwrap();

        let (first, second) = std::thread::scope(|scope| {
            let first =
                scope.spawn(|| left.settle(terminal(ResourceActionDispositionV1::Complete)));
            let second =
                scope.spawn(|| left.settle(terminal(ResourceActionDispositionV1::Partial)));
            (
                first.join().unwrap().unwrap(),
                second.join().unwrap().unwrap(),
            )
        });
        assert_eq!(first, second);
        assert_eq!(publisher.calls().len(), 1);
        assert!(matches!(
            right_observer.wait_until(tokio::time::Instant::now()).await,
            Err(RemoteRequestFlightRefusalV1::ObservationTimedOut)
        ));

        drop(left);
        assert_eq!(request_paths(&case).len(), 1);
        let right_outcome = right
            .settle(terminal(ResourceActionDispositionV1::Failed))
            .unwrap();
        assert_ne!(first.delivery_id(), right_outcome.delivery_id());
        assert_eq!(
            right_observer.wait_until(tokio::time::Instant::now()).await,
            Ok(right_outcome)
        );
        assert_eq!(publisher.calls().len(), 2);
    }

    #[test]
    fn remote_request_flight_task_d_refusing_publication_racers_join_same_refusal() {
        let case = case();
        let publisher = BarrierPublisher::new(PublisherReply::Refuse);
        initialize_root(&case, attempt(), 16);
        let driver =
            RemoteRequestDriverV1::open_recovered(custody(&case), attempt(), 16, publisher.clone())
                .unwrap();
        let request = driver.admit(owner(0)).unwrap();

        let (first, second, returned_early) = std::thread::scope(|scope| {
            let first =
                scope.spawn(|| request.settle(terminal(ResourceActionDispositionV1::Complete)));
            publisher.wait_until_entered();
            let (started_tx, started_rx) = mpsc::channel();
            let (done_tx, done_rx) = mpsc::channel();
            let request_ref = &request;
            let second = scope.spawn(move || {
                started_tx.send(()).unwrap();
                let result = request_ref.settle(terminal(ResourceActionDispositionV1::Partial));
                done_tx.send(()).unwrap();
                result
            });
            started_rx.recv().unwrap();
            let returned_early = done_rx.recv_timeout(Duration::from_secs(1)).is_ok();
            publisher.release();
            (
                first.join().unwrap(),
                second.join().unwrap(),
                returned_early,
            )
        });
        assert!(!returned_early, "a racer must join the live publication");
        assert_eq!(first, second);
        assert!(matches!(
            first,
            Err(RemoteRequestFlightRefusalV1::PublicationRefused(ref reason))
                if reason == "barrier publication refusal"
        ));
        assert_eq!(publisher.calls(), 1);
        assert!(child(&case).1.status.is_terminal_pending());
    }

    #[test]
    fn remote_request_flight_task_d_successful_publication_racers_join_once() {
        let case = case();
        let publisher = BarrierPublisher::new(PublisherReply::Echo);
        initialize_root(&case, attempt(), 16);
        let driver =
            RemoteRequestDriverV1::open_recovered(custody(&case), attempt(), 16, publisher.clone())
                .unwrap();
        let request = driver.admit(owner(0)).unwrap();

        let (first, second, returned_early) = std::thread::scope(|scope| {
            let first =
                scope.spawn(|| request.settle(terminal(ResourceActionDispositionV1::Complete)));
            publisher.wait_until_entered();
            let (started_tx, started_rx) = mpsc::channel();
            let (done_tx, done_rx) = mpsc::channel();
            let request_ref = &request;
            let second = scope.spawn(move || {
                started_tx.send(()).unwrap();
                let result = request_ref.settle(terminal(ResourceActionDispositionV1::Partial));
                done_tx.send(()).unwrap();
                result
            });
            started_rx.recv().unwrap();
            let returned_early = done_rx.recv_timeout(Duration::from_secs(1)).is_ok();
            publisher.release();
            (
                first.join().unwrap(),
                second.join().unwrap(),
                returned_early,
            )
        });
        assert!(!returned_early, "a racer must join the live publication");
        assert_eq!(first, second);
        let outcome = first.unwrap();
        assert_eq!(
            request.settle(terminal(ResourceActionDispositionV1::Failed)),
            Ok(outcome)
        );
        assert_eq!(publisher.calls(), 1);
        assert!(request_paths(&case).is_empty());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn remote_request_flight_task_d_timeout_drops_its_waiter() {
        let case = case();
        initialize_root(&case, attempt(), 16);
        let driver = RemoteRequestDriverV1::open_recovered(
            custody(&case),
            attempt(),
            16,
            RecordingPublisher::with_replies([]),
        )
        .unwrap();
        let request = driver.admit(owner(0)).unwrap();
        let mut observer = request.observer();
        let deadline = tokio::time::Instant::now();
        assert!(matches!(
            observer.wait_until(deadline).await,
            Err(RemoteRequestFlightRefusalV1::ObservationTimedOut)
        ));
        assert_eq!(request.live_waiters(), 0);
    }

    #[test]
    fn remote_request_flight_task_d_drop_retains_refused_publication_debt() {
        let case = case();
        let publisher =
            RecordingPublisher::with_replies([PublisherReply::Refuse, PublisherReply::Echo]);
        initialize_root(&case, attempt(), 16);
        let driver =
            RemoteRequestDriverV1::open_recovered(custody(&case), attempt(), 16, publisher.clone())
                .unwrap();
        let request = driver.admit(owner(0)).unwrap();
        assert!(matches!(
            request.settle(terminal(ResourceActionDispositionV1::Failed)),
            Err(RemoteRequestFlightRefusalV1::PublicationRefused(_))
        ));
        drop(request);
        assert_eq!(
            publisher.calls().len(),
            1,
            "drop must not retry publication"
        );
        assert!(child(&case).1.status.is_terminal_pending());
        assert!(matches!(
            driver.admit(owner(1)),
            Err(RemoteRequestFlightRefusalV1::ReopenRequired(_))
        ));
        drop(driver);

        let recovered =
            RemoteRequestDriverV1::open_recovered(custody(&case), attempt(), 16, publisher.clone())
                .unwrap();
        assert_eq!(publisher.calls().len(), 2);
        assert!(request_paths(&case).is_empty());
        drop(recovered.admit(owner(2)).unwrap());
    }
}
