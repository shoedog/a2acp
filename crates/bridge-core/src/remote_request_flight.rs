use crate::{
    execution_policy::Sha256HexV1,
    fs_custody::{
        open_regular_child, required_file_content_snapshot_v2, ChildNameV2, FileContentSnapshotV2,
        FsCustodyError, JournalMutationOutcomeV2, JournalRootCustodyV2, JournalRootOperationV2,
        ReservedNameNamespaceV2,
    },
    ids::{AttemptId, AttemptIdentity, ExecutionId},
    namespace_transaction::{NamespaceTransactionOutcomeV2, NamespaceTransactionV2},
    resource_flight::DedicatedRemoteRequestIdV1,
    retained_resource_flight::ResourceFlightOwnerV1,
};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::collections::BTreeSet;
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
#[serde(rename_all = "snake_case")]
enum TerminalOutcomeV1 {
    Complete,
    PreSendFailure,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
enum ChildStateV1 {
    Active,
    TerminalAcknowledged { outcome: TerminalOutcomeV1 },
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
struct CensusV1 {
    checkpoint: CheckpointWireV1,
    checkpoint_snapshot: FileContentSnapshotV2,
    children: Vec<RequestChildWireV1>,
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
        let census = journal.scan(&operation)?;
        if census.staged
            || census
                .children
                .iter()
                .any(|child| matches!(child.status, ChildStateV1::Active))
        {
            return Err(RemoteRequestFlightRefusalV1::ReopenRequired(
                "staged or unowned active child",
            ));
        }
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
            let (wire, _snapshot): (RequestChildWireV1, _) = read_wire(op, &read_name)?;
            if wire.schema != SCHEMA {
                return Err(RemoteRequestFlightRefusalV1::ForeignSchema("request child"));
            }
            if wire.attempt != self.attempt {
                return Err(RemoteRequestFlightRefusalV1::ForeignAttempt);
            }
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
                children.push(wire);
            }
        }
        let (checkpoint, checkpoint_snapshot): (CheckpointWireV1, _) =
            checkpoint.ok_or_else(|| Refusal::Malformed("checkpoint is absent".into()))?;
        if checkpoint.schema != SCHEMA {
            return Err(RemoteRequestFlightRefusalV1::ForeignSchema("checkpoint"));
        }
        if checkpoint.attempt != self.attempt {
            return Err(RemoteRequestFlightRefusalV1::ForeignAttempt);
        }
        if checkpoint.identity_chain_digest
            != checkpoint_digest(&self.attempt, checkpoint.next_ordinal)
        {
            return Err(RemoteRequestFlightRefusalV1::DigestMismatch("checkpoint"));
        }
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
                status: ChildStateV1::Active,
            };
            wire.authority_digest = authority_digest(&wire);
            let name = request_name(&wire.authority_digest);
            #[cfg(test)]
            boundary(_cut, 0, "temporary write")?;
            let staged = mutation(op.stage(&name, &encoded(&wire)?, "stage request child"))?;
            #[cfg(test)]
            boundary(_cut, 1, "temporary sync")?;
            #[cfg(test)]
            boundary(_cut, 2, "no-replace publication")?;
            let _snapshot = mutation(op.publish(&name, staged, "publish request child"))?;
            #[cfg(test)]
            boundary(_cut, 3, "request root sync")?;
            sync(op.sync("sync request root"))?;
            #[cfg(test)]
            boundary(_cut, 4, "checkpoint advance")?;
            let checkpoint = checkpoint(&self.attempt, next);
            transaction(NamespaceTransactionV2::replace(
                &op,
                Self::checkpoint_name(),
                census.checkpoint_snapshot.object,
                &encoded(&checkpoint)?,
                "advance checkpoint",
            ))?;
            #[cfg(test)]
            boundary(_cut, 5, "checkpoint sync")?;
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
}
#[cfg(test)]
#[derive(Clone, Copy)]
enum InjectedTaskAOutcomeV1 {
    Complete,
    Refused,
    Retained,
    Unknown,
    Unsupported,
    ProtectiveDebt,
}
#[cfg(test)]
impl InjectedTaskAOutcomeV1 {
    const PROTECTIVE: [Self; 5] = [
        Self::Refused,
        Self::Retained,
        Self::Unknown,
        Self::Unsupported,
        Self::ProtectiveDebt,
    ];
}
#[cfg(test)]
fn consume_injected_task_a_outcome(outcome: InjectedTaskAOutcomeV1) -> FlightResult<()> {
    let kind = match outcome {
        InjectedTaskAOutcomeV1::Complete => return Ok(()),
        InjectedTaskAOutcomeV1::Refused => TaskAProtectiveOutcomeV1::Refused,
        InjectedTaskAOutcomeV1::Retained => TaskAProtectiveOutcomeV1::Retained,
        InjectedTaskAOutcomeV1::Unknown => TaskAProtectiveOutcomeV1::Unknown,
        InjectedTaskAOutcomeV1::Unsupported => TaskAProtectiveOutcomeV1::Unsupported,
        InjectedTaskAOutcomeV1::ProtectiveDebt => TaskAProtectiveOutcomeV1::ProtectiveDebt,
    };
    Err(protective(kind, "injected"))
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        fs_custody::{
            required_object_identity_v2, BirthTimeV1, JournalRootBindingV2, ObjectIdentityV2,
        },
        ids::{AttemptId, ExecutionId, NodeId},
    };
    use std::{fs, os::unix::fs::MetadataExt as _, path::PathBuf};
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
            if boundary == 0 {
                assert!(reopened.is_ok());
            } else {
                assert!(matches!(
                    reopened,
                    Err(RemoteRequestFlightRefusalV1::ReopenRequired(_))
                ));
            }
        }
    }
    #[test]
    fn remote_request_flight_task_a_outcomes_are_never_success_flattened() {
        for outcome in InjectedTaskAOutcomeV1::PROTECTIVE {
            assert!(consume_injected_task_a_outcome(outcome).is_err());
        }
        assert!(consume_injected_task_a_outcome(InjectedTaskAOutcomeV1::Complete).is_ok());
    }
}
