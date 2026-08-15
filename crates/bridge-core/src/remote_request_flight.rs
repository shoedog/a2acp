use crate::{
    execution_policy::Sha256HexV1,
    fs_custody::{
        open_regular_child, required_file_content_snapshot_v2, ChildNameV2, FileContentSnapshotV2,
        FsCustodyError, JournalMutationOutcomeV2, JournalRootCustodyV2, JournalRootOperationV2,
        ReservedNameNamespaceV2,
    },
    ids::{AttemptId, AttemptIdentity, ExecutionId},
    namespace_transaction::{NamespaceTransactionOutcomeV2, NamespaceTransactionV2},
    resource_flight::{DedicatedRemoteRequestIdV1, ResourceActionDispositionV1},
    retained_resource_flight::ResourceFlightOwnerV1,
};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::{collections::BTreeSet, ops::Deref};
const SCHEMA: u8 = 1;
const CAPACITY: usize = 4096;
const ADMISSION_FOOTPRINT: usize = 3;
const WIRE_CAP: usize = 4096;
const CHECKPOINT_CHILD_V1: &str = "remote-request-checkpoint.json";
const REQUEST_CHILD_PREFIX_V1: &str = "remote-request-authority-";
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TaskAProtectiveOutcomeV1 {
    Refused,
    Retained,
    Unknown,
    Unsupported,
    ProtectiveDebt,
}
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
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
    Active {},
    PreSendFailure {},
    TerminalAcknowledged {},
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
    request_id: DedicatedRemoteRequestIdV1,
}
impl RemoteRequestAuthorityV1 {
    #[must_use]
    pub fn request_id(&self) -> &DedicatedRemoteRequestIdV1 {
        &self.request_id
    }
}
pub struct RemoteRequestJournalV1 {
    custody: JournalRootCustodyV2,
    attempt: AttemptIdentity,
    capacity: usize,
    requires_reopen: bool,
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
    pub fn initialize(
        custody: JournalRootCustodyV2,
        attempt: AttemptIdentity,
    ) -> FlightResult<Self> {
        Self::initialize_with_capacity(custody, attempt, CAPACITY)
    }
    fn initialize_with_capacity(
        custody: JournalRootCustodyV2,
        attempt: AttemptIdentity,
        capacity: usize,
    ) -> FlightResult<Self> {
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
            let checkpoint = checkpoint(&attempt, 0);
            let name = Self::checkpoint_name();
            let staged = mutation(op.stage(&name, &encoded(&checkpoint)?, "stage checkpoint"))?;
            mutation(op.publish(&name, staged, "publish checkpoint"))?;
            sync(op.sync("sync checkpoint"))?;
        }
        Ok(Self {
            custody,
            attempt,
            capacity,
            requires_reopen: false,
        })
    }
    pub fn open(custody: JournalRootCustodyV2, attempt: AttemptIdentity) -> FlightResult<Self> {
        Self::open_with_capacity(custody, attempt, CAPACITY)
    }
    fn open_with_capacity(
        custody: JournalRootCustodyV2,
        attempt: AttemptIdentity,
        capacity: usize,
    ) -> FlightResult<Self> {
        let journal = Self {
            custody,
            attempt,
            capacity,
            requires_reopen: false,
        };
        let operation = journal
            .custody
            .begin_operation("open request journal")
            .map_err(fs)?;
        journal.authorize_checkpoint(&operation)?;
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
            [child]
                if child.ordinal == census.checkpoint.next_ordinal
                    && child.status == (ChildStateV1::Active {}) =>
            {
                Some(*child)
            }
            _ => return Err(Refusal::ReopenRequired("ambiguous request ordinal census")),
        };
        if let Some(child) = orphan {
            let next = census
                .checkpoint
                .next_ordinal
                .checked_add(1)
                .ok_or(Refusal::OrdinalOverflow)?;
            let value = checkpoint(&journal.attempt, next);
            #[cfg(test)]
            task_a_boundary(TaskABoundaryV1::Replace)?;
            transaction(NamespaceTransactionV2::replace(
                &operation,
                Self::checkpoint_name(),
                census.checkpoint_snapshot.object,
                &encoded(&value)?,
                "heal orphan checkpoint",
            ))?;
            sync(operation.sync("sync healed checkpoint"))?;
            Self::replace_child(
                &operation,
                child,
                ChildStateV1::PreSendFailure {},
                "close orphan request",
            )?;
        }
        for child in &census.children {
            match child.status {
                ChildStateV1::Active {} => {}
                ChildStateV1::TerminalAcknowledged {} => {
                    Self::retire_child(&operation, child, "retire acknowledged request")?
                }
                ChildStateV1::PreSendFailure {} => {}
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
        #[cfg(test)]
        task_a_boundary(TaskABoundaryV1::Replace)?;
        transaction(NamespaceTransactionV2::replace(
            op,
            request_name(&child.authority_digest),
            child.snapshot.object,
            &encoded(&successor)?,
            label,
        ))
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
            let (read_name, final_name, is_staged) = if value.starts_with(REQUEST_CHILD_PREFIX_V1) {
                (name.clone(), name, false)
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
        let op = self
            .custody
            .begin_operation("admit remote request")
            .map_err(fs)?;
        let census = self.scan(&op)?;
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
            #[cfg(test)]
            task_a_boundary(TaskABoundaryV1::Stage)?;
            let staged = mutation(op.stage(&name, &encoded(&wire)?, "stage request child"))?;
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
            #[cfg(test)]
            task_a_boundary(TaskABoundaryV1::Sync)?;
            sync(op.sync("sync request root"))?;
            #[cfg(test)]
            boundary(_cut, 4, "checkpoint advance")?;
            let checkpoint = checkpoint(&self.attempt, next);
            #[cfg(test)]
            task_a_boundary(TaskABoundaryV1::Replace)?;
            transaction(NamespaceTransactionV2::replace(
                &op,
                Self::checkpoint_name(),
                census.checkpoint_snapshot.object,
                &encoded(&checkpoint)?,
                "advance checkpoint",
            ))?;
            #[cfg(test)]
            boundary(_cut, 5, "checkpoint sync")?;
            #[cfg(test)]
            task_a_boundary(TaskABoundaryV1::Sync)?;
            sync(op.sync("sync advanced checkpoint"))?;
            Ok(RemoteRequestAuthorityV1 {
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
            .find(|child| child.request_id == *authority.request_id())
            .ok_or(Refusal::InvalidStateTransition(
                "request authority is absent",
            ))
    }
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
        let result = match child.status {
            ChildStateV1::Active {} => Self::replace_child(
                &op,
                child,
                ChildStateV1::TerminalAcknowledged {},
                "acknowledge terminal request",
            ),
            ChildStateV1::TerminalAcknowledged {} => Ok(()),
            ChildStateV1::PreSendFailure {} => Err(Refusal::InvalidStateTransition(
                "pre-send failure cannot be acknowledged",
            )),
        };
        drop(op);
        if result.is_err() {
            self.requires_reopen = true;
        }
        result
    }
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
        if child.status != (ChildStateV1::TerminalAcknowledged {}) {
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
            #[cfg(test)]
            task_a_boundary(TaskABoundaryV1::Sync)?;
            sync(op.sync("sync acknowledged retirement"))?;
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
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TaskABoundaryV1 {
    Stage,
    Publish,
    Replace,
    Retire,
    Sync,
}
#[cfg(test)]
thread_local! {
    static INJECTED_TASK_A_BOUNDARY: std::cell::RefCell<
        Option<(TaskABoundaryV1, InjectedTaskAOutcomeV1)>,
    > = const { std::cell::RefCell::new(None) };
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
            std::io::Error::new(std::io::ErrorKind::Other, "injected"),
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
            std::io::Error::new(std::io::ErrorKind::Other, "injected"),
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
mod tests {
    use super::*;
    use crate::{
        fs_custody::{
            required_object_identity_v2, BirthTimeV1, CustodyIntentV2, CustodyOperationKindV2,
            JournalRootBindingV2, ObjectIdentityV2,
        },
        ids::{AttemptId, ExecutionId, NodeId},
    };
    use std::{fs, fs::File, os::unix::fs::MetadataExt as _, path::PathBuf};
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
        RemoteRequestJournalV1::initialize_with_capacity(custody(case), attempt(), cap).unwrap()
    }
    fn request_paths(case: &Case) -> Vec<PathBuf> {
        fs::read_dir(&case.root)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|path| {
                path.file_name()
                    .unwrap()
                    .to_string_lossy()
                    .contains(REQUEST_CHILD_PREFIX_V1)
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
    fn unchecked(case: &Case, capacity: usize) -> RemoteRequestJournalV1 {
        RemoteRequestJournalV1 {
            custody: custody(case),
            attempt: attempt(),
            capacity,
            requires_reopen: false,
        }
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
                unchecked(&case, 16).admit_with(owner(1), || {
                    minted += 1;
                    Ok(request(1))
                }),
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
    fn remote_request_flight_capacity_precedes_id_mint_and_positive_edge_admits() {
        let case = case();
        let mut journal = initialized(&case, 8);
        let mut minted = 0;
        for index in 0..5 {
            journal
                .admit_with(owner(index), || {
                    minted += 1;
                    Ok(request(index))
                })
                .unwrap();
        }
        assert_eq!(minted, 5);
        let (checkpoint, _): (CheckpointWireV1, _) = read_wire(
            &journal.custody.begin_operation("read checkpoint").unwrap(),
            &RemoteRequestJournalV1::checkpoint_name(),
        )
        .unwrap();
        assert_eq!(checkpoint.next_ordinal, 5);
        let before = fs::read_dir(&case.root).unwrap().count();
        assert!(matches!(
            journal.admit_with(owner(6), || {
                minted += 1;
                Ok(request(6))
            }),
            Err(RemoteRequestFlightRefusalV1::Capacity)
        ));
        assert_eq!(minted, 5);
        assert_eq!(fs::read_dir(&case.root).unwrap().count(), before);
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
                .admit_with(owner(1), || {
                    minted += 1;
                    Ok(request(1))
                })
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
            .admit_with(owner(1), || {
                minted += 1;
                Ok(request(1))
            })
            .is_err());
        assert_eq!(minted, 0);
        assert_eq!(root_bytes(&case), before);
    }

    #[test]
    fn remote_request_flight_reopen_closes_step_five_orphan_idempotently() {
        for boundary in [4] {
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
        let mut journal =
            RemoteRequestJournalV1::initialize_with_capacity(custody(&case), foreign.clone(), 16)
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
            assert_eq!(child(&case).1.status, ChildStateV1::TerminalAcknowledged {});
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
            let before = root_bytes(&stage_case);
            inject_task_a_boundary_for_test(TaskABoundaryV1::Stage, outcome);
            assert!(matches!(
                stage_journal.admit_with(owner(0), || Ok(request(0))),
                Err(RemoteRequestFlightRefusalV1::TaskA(kind, _)) if kind == expected
            ));
            assert!(request_paths(&stage_case).is_empty());
            assert_eq!(root_bytes(&stage_case), before);

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
            assert_eq!(root_bytes(&acknowledge_case), before);

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
}
