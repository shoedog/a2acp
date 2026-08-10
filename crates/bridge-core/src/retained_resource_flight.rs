//! Journal-backed retained resource flight runner.
//!
//! Design notes:
//! * `FileResourceFlightJournal` is an append-only JSONL durable substrate.
//!   Every record is sequenced, fsynced, and bounded per flight. Intent
//!   admission reserves the lifecycle entries required to finish, so capacity
//!   refuses before dispatch but never turns a journaled flight into an
//!   unaccounted terminal transition.
//! * `ResourceFlightRegistryV1` owns the node-owner to generation/request
//!   mapping. Its mutex is the in-process reservation CAS and its journal
//!   reservation is the durable CAS: a live generation returns the same Arc,
//!   while a dead, journaled generation is recovered as `Unknown` instead of
//!   being re-minted. Each dedicated request receives its own key and flight.
//!   Slice 5 owns the durable `NodeCleanupRecordV2.collateral` writer; this
//!   module publishes the flight-side aggregation value only.
//! * `join_blocking` waits on `std::sync::Condvar`, never a Tokio primitive;
//!   it is safe on `Supervised::terminate_blocking`'s bare std::thread.
//!
//! Lock declaration: all flight transitions use only `transition`; registry
//! acquisition uses only `ResourceFlightRegistryV1::flights`. Journal append is
//! called while its owning lock is held, but no callback runs under either lock:
//! result publication is outside both locks and no callback may re-enter a
//! flight or registry. This module neither takes a custody file cell nor
//! `deletion_admission`, preserving the custody-lock prohibition.

use crate::attempt_activity::MonotonicClock;
use crate::execution_policy::{BoundedCauseV1, CollateralDispositionV1, Sha256HexV1};
use crate::ids::{AttemptId, NodeId};
use crate::resource_flight::{
    BoundedRecoveryReasonV1, ProcessStartIdentityV1, RecoveryOwnerV1, ResourceActionDispositionV1,
    ResourceActionResultV1, ResourceFlightIdV1, ResourceFlightStateV1, ResourceIdentityV1,
};
use ring::digest::{digest, SHA256};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};

pub const RESOURCE_FLIGHT_JOURNAL_SCHEMA_V1: u16 = 1;
// Dispatch, the observed signal batch, an optional guard transfer, and terminal
// settlement must all remain appendable after intent becomes durable.
const LIFECYCLE_SLOTS: u8 = 4;
// A protected process reserves its complete destructive lifecycle before the
// child exists: capture or binding failure, intent, dispatch, one observed
// signal batch, optional guard transfer, terminal settlement, and one owner
// evidence margin. Unused slots are harmless; their purpose is to keep later
// owner churn from consuming the rows needed to settle the process flight.
const PROCESS_LIFECYCLE_SLOTS: u8 = 7;

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResourceFlightOwnerV1 {
    pub node_id: NodeId,
    pub owner_key: String,
}

impl ResourceFlightOwnerV1 {
    pub fn new(
        node_id: NodeId,
        owner_key: impl Into<String>,
    ) -> Result<Self, RetainedResourceFlightError> {
        let owner_key = owner_key.into();
        if owner_key.is_empty() {
            return Err(RetainedResourceFlightError::InvalidOwner);
        }
        Ok(Self { node_id, owner_key })
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ResourceFlightKeyV1 {
    AcpGeneration { generation: String },
    ContainerGeneration { generation: String },
    DedicatedRemoteRequest { request_id: String },
}

impl ResourceFlightKeyV1 {
    #[must_use]
    pub fn from_identity(identity: &ResourceIdentityV1) -> Self {
        match identity {
            ResourceIdentityV1::AcpProcess { generation, .. } => Self::AcpGeneration {
                generation: generation.clone(),
            },
            ResourceIdentityV1::ManagedContainer { generation, .. } => Self::ContainerGeneration {
                generation: generation.clone(),
            },
            ResourceIdentityV1::DedicatedRemoteRequest { request_id } => {
                Self::DedicatedRemoteRequest {
                    request_id: request_id.clone(),
                }
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResourceActionIntentV1 {
    pub initiator: ResourceFlightOwnerV1,
    pub capability_digest: Sha256HexV1,
    pub cause: Option<BoundedCauseV1>,
}

/// One observed process-authority call. `errno` is captured immediately after
/// a failed syscall; no signal attempt is allowed to disappear into a
/// best-effort branch.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProcessSignalObservationV1 {
    pub pid: u32,
    pub expected_start_time_ticks: u64,
    pub signal: i32,
    pub return_code: i32,
    pub errno: Option<i32>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProcessBindingStageV1 {
    Spawn,
    ImmutableStart,
    JournalBinding,
}

/// Deliberately carries no retained capability. A recovered intent has no API
/// that can reconstruct a signal target from this metadata, a PID, or a name.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case", deny_unknown_fields)]
pub enum ResourceFlightJournalEventV1 {
    FlightReserved {
        key: ResourceFlightKeyV1,
        attempt_id: AttemptId,
    },
    OwnerAttached {
        owner: ResourceFlightOwnerV1,
    },
    OwnerDetached {
        owner: ResourceFlightOwnerV1,
    },
    ProcessLifecycleReserved {
        reserved_lifecycle_slots: u8,
    },
    ProcessTreeCaptured {
        identity: ResourceIdentityV1,
    },
    ProcessBindingFailed {
        stage: ProcessBindingStageV1,
        pid: Option<u32>,
    },
    IntentJournaled {
        intent: ResourceActionIntentV1,
        owner_snapshot: Vec<ResourceFlightOwnerV1>,
        reserved_lifecycle_slots: u8,
    },
    CollateralDiscovered {
        owner: ResourceFlightOwnerV1,
    },
    DispatchStarted {},
    ProcessSignalsObserved {
        observations: Vec<ProcessSignalObservationV1>,
    },
    GuardTransferred {
        recovery_owner: RecoveryOwnerV1,
        deadline_ms: u64,
    },
    Settled {
        result: ResourceActionResultV1,
    },
}

impl ResourceFlightJournalEventV1 {
    fn reserved(&self) -> u8 {
        match self {
            Self::ProcessLifecycleReserved {
                reserved_lifecycle_slots,
            }
            | Self::IntentJournaled {
                reserved_lifecycle_slots,
                ..
            } => *reserved_lifecycle_slots,
            _ => 0,
        }
    }

    fn requires_reserved_slot(&self) -> bool {
        matches!(
            self,
            Self::DispatchStarted {}
                | Self::ProcessSignalsObserved { .. }
                | Self::GuardTransferred { .. }
                | Self::Settled { .. }
        )
    }

    fn consumes_process_reservation_when_present(&self) -> bool {
        match self {
            Self::ProcessTreeCaptured { .. }
            | Self::ProcessBindingFailed { .. }
            | Self::IntentJournaled { .. } => true,
            Self::OwnerDetached { owner } => owner.node_id.as_str() == "process-spawn",
            _ => false,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResourceFlightJournalRecordV1 {
    pub schema_version: u16,
    pub sequence: u64,
    pub event: ResourceFlightJournalEventV1,
}

/// Durable key-to-flight binding. The reservation is written before the
/// flight-local `FlightReserved` event, so a crash in that gap refuses a new
/// capability rather than creating a second flight for the same generation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResourceFlightReservationRecordV1 {
    pub schema_version: u16,
    pub key: ResourceFlightKeyV1,
    pub attempt_id: AttemptId,
    pub resource_flight_id: ResourceFlightIdV1,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ResourceFlightReservationOutcomeV1 {
    Created,
    Existing(ResourceFlightReservationRecordV1),
}

/// Result of the journal's terminal compare-and-set. A terminal append is
/// linearized with inspection of the existing journal rows, so callers never
/// append a second `Settled` record for the same flight.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ResourceFlightTerminalAppendOutcomeV1 {
    /// The journal appended `Settled` under its append lock. The returned
    /// rows are the exact durable snapshot used to choose the terminal
    /// sequence, so recovery publishes every owner recorded before terminal.
    Appended {
        records: Vec<ResourceFlightJournalRecordV1>,
    },
    AlreadySettled {
        existing: ResourceActionResultV1,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum ResourceFlightJournalError {
    #[error("resource-flight journal is full")]
    Full,
    #[error("resource-flight journal sequence is invalid")]
    Sequence,
    #[error("resource-flight journal accounting is invalid")]
    Accounting,
    #[error("resource-flight journal schema is unsupported")]
    Schema,
    #[error("resource-flight journal I/O at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("resource-flight journal decode at {path}: {source}")]
    Decode {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error("resource-flight journal encode at {path}: {source}")]
    Encode {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
}

pub trait ResourceFlightJournal: Send + Sync {
    /// Atomically reserve one durable flight id for a key. An existing binding
    /// is returned verbatim and must never be overwritten or re-minted.
    fn reserve_flight(
        &self,
        reservation: &ResourceFlightReservationRecordV1,
    ) -> Result<ResourceFlightReservationOutcomeV1, ResourceFlightJournalError>;

    /// Success means the row is durable enough to survive a process crash.
    fn append(
        &self,
        id: &ResourceFlightIdV1,
        record: &ResourceFlightJournalRecordV1,
    ) -> Result<(), ResourceFlightJournalError>;
    /// Atomically choose the next sequence and append one `Settled` record,
    /// or return the terminal result which was already durable. Callers never
    /// supply a sequence: inspection, sequence assignment, and append are one
    /// append-lock operation, so recovery cannot race a non-terminal row with
    /// a stale sequence.
    fn append_terminal(
        &self,
        id: &ResourceFlightIdV1,
        result: &ResourceActionResultV1,
    ) -> Result<ResourceFlightTerminalAppendOutcomeV1, ResourceFlightJournalError>;

    fn records(
        &self,
        id: &ResourceFlightIdV1,
    ) -> Result<Vec<ResourceFlightJournalRecordV1>, ResourceFlightJournalError>;
}

fn outstanding(
    records: &[ResourceFlightJournalRecordV1],
) -> Result<usize, ResourceFlightJournalError> {
    let mut slots = 0_usize;
    for (index, record) in records.iter().enumerate() {
        if record.schema_version != RESOURCE_FLIGHT_JOURNAL_SCHEMA_V1 {
            return Err(ResourceFlightJournalError::Schema);
        }
        if record.sequence != u64::try_from(index + 1).unwrap_or(u64::MAX) {
            return Err(ResourceFlightJournalError::Sequence);
        }
        let before = slots;
        slots = slots
            .checked_add(usize::from(record.event.reserved()))
            .ok_or(ResourceFlightJournalError::Accounting)?;
        let consumed = usize::from(
            record.event.requires_reserved_slot()
                || (before != 0 && record.event.consumes_process_reservation_when_present()),
        );
        if consumed != 0 {
            slots = slots
                .checked_sub(consumed)
                .ok_or(ResourceFlightJournalError::Accounting)?;
        }
    }
    Ok(slots)
}

fn settled_result(records: &[ResourceFlightJournalRecordV1]) -> Option<ResourceActionResultV1> {
    records.iter().rev().find_map(|row| match &row.event {
        ResourceFlightJournalEventV1::Settled { result } => Some(result.clone()),
        _ => None,
    })
}

fn admission_check(
    records: &[ResourceFlightJournalRecordV1],
    cap: usize,
    record: &ResourceFlightJournalRecordV1,
) -> Result<(), ResourceFlightJournalError> {
    let slots = outstanding(records)?;
    if record.schema_version != RESOURCE_FLIGHT_JOURNAL_SCHEMA_V1 {
        return Err(ResourceFlightJournalError::Schema);
    }
    if record.sequence != u64::try_from(records.len() + 1).unwrap_or(u64::MAX) {
        return Err(ResourceFlightJournalError::Sequence);
    }
    let consumed = usize::from(
        record.event.requires_reserved_slot()
            || (slots != 0 && record.event.consumes_process_reservation_when_present()),
    );
    if consumed == 1 && slots == 0 {
        return Err(ResourceFlightJournalError::Accounting);
    }
    let slots_after = slots
        .checked_add(usize::from(record.event.reserved()))
        .and_then(|value| value.checked_sub(consumed))
        .ok_or(ResourceFlightJournalError::Accounting)?;
    let needed = records
        .len()
        .checked_add(1)
        .and_then(|value| value.checked_add(slots_after))
        .ok_or(ResourceFlightJournalError::Full)?;
    if needed > cap {
        return Err(ResourceFlightJournalError::Full);
    }
    Ok(())
}

/// Bounded in-memory implementation used by the V2 compatibility adapter and
/// deterministic unit tests. V3 callers supply a durable journal instead.
#[derive(Debug)]
pub struct InMemoryResourceFlightJournal {
    cap: usize,
    state: Mutex<InMemoryResourceFlightJournalState>,
}

#[derive(Default, Debug)]
struct InMemoryResourceFlightJournalState {
    records: BTreeMap<ResourceFlightIdV1, Vec<ResourceFlightJournalRecordV1>>,
    reservations: BTreeMap<ResourceFlightKeyV1, ResourceFlightReservationRecordV1>,
}

impl InMemoryResourceFlightJournal {
    #[must_use]
    pub fn new(cap: usize) -> Self {
        Self {
            cap,
            state: Mutex::new(InMemoryResourceFlightJournalState::default()),
        }
    }
}

impl ResourceFlightJournal for InMemoryResourceFlightJournal {
    fn reserve_flight(
        &self,
        reservation: &ResourceFlightReservationRecordV1,
    ) -> Result<ResourceFlightReservationOutcomeV1, ResourceFlightJournalError> {
        if reservation.schema_version != RESOURCE_FLIGHT_JOURNAL_SCHEMA_V1 {
            return Err(ResourceFlightJournalError::Schema);
        }
        let mut state = self
            .state
            .lock()
            .map_err(|_| ResourceFlightJournalError::Accounting)?;
        match state.reservations.get(&reservation.key) {
            Some(existing) => Ok(ResourceFlightReservationOutcomeV1::Existing(
                existing.clone(),
            )),
            None => {
                state
                    .reservations
                    .insert(reservation.key.clone(), reservation.clone());
                Ok(ResourceFlightReservationOutcomeV1::Created)
            }
        }
    }

    fn append(
        &self,
        id: &ResourceFlightIdV1,
        record: &ResourceFlightJournalRecordV1,
    ) -> Result<(), ResourceFlightJournalError> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| ResourceFlightJournalError::Accounting)?;
        let rows = state.records.entry(id.clone()).or_default();
        if settled_result(rows).is_some() {
            return Err(ResourceFlightJournalError::Accounting);
        }
        admission_check(rows, self.cap, record)?;
        rows.push(record.clone());
        Ok(())
    }

    fn append_terminal(
        &self,
        id: &ResourceFlightIdV1,
        result: &ResourceActionResultV1,
    ) -> Result<ResourceFlightTerminalAppendOutcomeV1, ResourceFlightJournalError> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| ResourceFlightJournalError::Accounting)?;
        let rows = state.records.entry(id.clone()).or_default();
        if let Some(existing) = settled_result(rows) {
            return Ok(ResourceFlightTerminalAppendOutcomeV1::AlreadySettled { existing });
        }
        let record = ResourceFlightJournalRecordV1 {
            schema_version: RESOURCE_FLIGHT_JOURNAL_SCHEMA_V1,
            sequence: u64::try_from(rows.len() + 1).unwrap_or(u64::MAX),
            event: ResourceFlightJournalEventV1::Settled {
                result: result.clone(),
            },
        };
        admission_check(rows, self.cap, &record)?;
        rows.push(record);
        Ok(ResourceFlightTerminalAppendOutcomeV1::Appended {
            records: rows.clone(),
        })
    }

    fn records(
        &self,
        id: &ResourceFlightIdV1,
    ) -> Result<Vec<ResourceFlightJournalRecordV1>, ResourceFlightJournalError> {
        let state = self
            .state
            .lock()
            .map_err(|_| ResourceFlightJournalError::Accounting)?;
        let rows = state.records.get(id).cloned().unwrap_or_default();
        let _ = outstanding(&rows)?;
        Ok(rows)
    }
}

/// Durable JSONL journal. The attempt persistence owner must create and pin its
/// root before use; `open` refuses an absent/non-directory root.
pub struct FileResourceFlightJournal {
    root: PathBuf,
    cap: usize,
    append_lock: Mutex<()>,
}

impl FileResourceFlightJournal {
    pub fn open(root: impl Into<PathBuf>, cap: usize) -> Result<Self, ResourceFlightJournalError> {
        let root = root.into();
        let metadata =
            std::fs::metadata(&root).map_err(|source| ResourceFlightJournalError::Io {
                path: root.clone(),
                source,
            })?;
        if !metadata.is_dir() {
            return Err(ResourceFlightJournalError::Io {
                path: root,
                source: std::io::Error::new(std::io::ErrorKind::NotADirectory, "journal root"),
            });
        }
        Ok(Self {
            root,
            cap,
            append_lock: Mutex::new(()),
        })
    }

    fn path(&self, id: &ResourceFlightIdV1) -> PathBuf {
        self.root.join(format!("{}.jsonl", id.as_str()))
    }

    fn reservation_path(
        &self,
        key: &ResourceFlightKeyV1,
    ) -> Result<PathBuf, ResourceFlightJournalError> {
        let encoded =
            serde_json::to_vec(key).map_err(|source| ResourceFlightJournalError::Encode {
                path: self.root.clone(),
                source,
            })?;
        let digest = digest(&SHA256, &encoded);
        let key_digest = digest
            .as_ref()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        Ok(self
            .root
            .join(format!("resource-flight-reservation-{key_digest}.json")))
    }

    fn read_reservation(
        &self,
        path: &Path,
    ) -> Result<ResourceFlightReservationRecordV1, ResourceFlightJournalError> {
        let encoded = std::fs::read(path).map_err(|source| ResourceFlightJournalError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        let reservation: ResourceFlightReservationRecordV1 = serde_json::from_slice(&encoded)
            .map_err(|source| ResourceFlightJournalError::Decode {
                path: path.to_path_buf(),
                source,
            })?;
        if reservation.schema_version != RESOURCE_FLIGHT_JOURNAL_SCHEMA_V1 {
            return Err(ResourceFlightJournalError::Schema);
        }
        Ok(reservation)
    }

    fn read(
        &self,
        path: &Path,
    ) -> Result<Vec<ResourceFlightJournalRecordV1>, ResourceFlightJournalError> {
        let encoded = match std::fs::read(path) {
            Ok(encoded) => encoded,
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(source) => {
                return Err(ResourceFlightJournalError::Io {
                    path: path.to_path_buf(),
                    source,
                })
            }
        };
        let mut rows = Vec::new();
        let complete_len = encoded
            .iter()
            .rposition(|byte| *byte == b'\n')
            .map_or(0, |index| index.saturating_add(1));
        if complete_len != encoded.len() {
            // `append` writes a JSON value and its delimiter separately. A
            // crash can therefore leave one unterminated tail. It was never a
            // complete journal record; discard and durably truncate only that
            // tail, while `outstanding` below continues to reject real holes.
            let file = OpenOptions::new()
                .write(true)
                .open(path)
                .map_err(|source| ResourceFlightJournalError::Io {
                    path: path.to_path_buf(),
                    source,
                })?;
            file.set_len(u64::try_from(complete_len).unwrap_or(u64::MAX))
                .and_then(|()| file.sync_all())
                .map_err(|source| ResourceFlightJournalError::Io {
                    path: path.to_path_buf(),
                    source,
                })?;
            File::open(&self.root)
                .and_then(|dir| dir.sync_all())
                .map_err(|source| ResourceFlightJournalError::Io {
                    path: self.root.clone(),
                    source,
                })?;
        }
        if complete_len != 0 {
            for line in encoded[..complete_len.saturating_sub(1)].split(|byte| *byte == b'\n') {
                rows.push(serde_json::from_slice(line).map_err(|source| {
                    ResourceFlightJournalError::Decode {
                        path: path.to_path_buf(),
                        source,
                    }
                })?);
            }
        }
        let _ = outstanding(&rows)?;
        Ok(rows)
    }
}

impl ResourceFlightJournal for FileResourceFlightJournal {
    fn reserve_flight(
        &self,
        reservation: &ResourceFlightReservationRecordV1,
    ) -> Result<ResourceFlightReservationOutcomeV1, ResourceFlightJournalError> {
        if reservation.schema_version != RESOURCE_FLIGHT_JOURNAL_SCHEMA_V1 {
            return Err(ResourceFlightJournalError::Schema);
        }
        let _write = self
            .append_lock
            .lock()
            .map_err(|_| ResourceFlightJournalError::Accounting)?;
        let path = self.reservation_path(&reservation.key)?;
        let encoded = serde_json::to_vec(reservation).map_err(|source| {
            ResourceFlightJournalError::Encode {
                path: path.clone(),
                source,
            }
        })?;
        let mut temp_number = 0_u64;
        loop {
            let temp = self.root.join(format!(
                ".resource-flight-reservation-{}-{}-{temp_number}.tmp",
                std::process::id(),
                reservation.resource_flight_id.as_str()
            ));
            temp_number = temp_number.saturating_add(1);
            let mut file = match OpenOptions::new().create_new(true).write(true).open(&temp) {
                Ok(file) => file,
                Err(source) if source.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(source) => return Err(ResourceFlightJournalError::Io { path: temp, source }),
            };
            file.write_all(&encoded)
                .and_then(|()| file.sync_all())
                .map_err(|source| ResourceFlightJournalError::Io {
                    path: temp.clone(),
                    source,
                })?;
            match std::fs::hard_link(&temp, &path) {
                Ok(()) => {
                    let _ = std::fs::remove_file(&temp);
                    File::open(&self.root)
                        .and_then(|dir| dir.sync_all())
                        .map_err(|source| ResourceFlightJournalError::Io {
                            path: self.root.clone(),
                            source,
                        })?;
                    return Ok(ResourceFlightReservationOutcomeV1::Created);
                }
                Err(source) if source.kind() == std::io::ErrorKind::AlreadyExists => {
                    let _ = std::fs::remove_file(&temp);
                    match self.read_reservation(&path) {
                        Ok(existing) => {
                            if existing.key != reservation.key {
                                return Err(ResourceFlightJournalError::Accounting);
                            }
                            return Ok(ResourceFlightReservationOutcomeV1::Existing(existing));
                        }
                        Err(ResourceFlightJournalError::Decode { .. })
                            if std::fs::metadata(&path)
                                .map(|metadata| metadata.len() == 0)
                                .unwrap_or(false) =>
                        {
                            std::fs::remove_file(&path).map_err(|source| {
                                ResourceFlightJournalError::Io {
                                    path: path.clone(),
                                    source,
                                }
                            })?;
                        }
                        Err(error) => return Err(error),
                    }
                }
                Err(source) => {
                    let _ = std::fs::remove_file(&temp);
                    return Err(ResourceFlightJournalError::Io {
                        path: path.clone(),
                        source,
                    });
                }
            }
        }
    }

    fn append(
        &self,
        id: &ResourceFlightIdV1,
        record: &ResourceFlightJournalRecordV1,
    ) -> Result<(), ResourceFlightJournalError> {
        let _write = self
            .append_lock
            .lock()
            .map_err(|_| ResourceFlightJournalError::Accounting)?;
        let path = self.path(id);
        let rows = self.read(&path)?;
        if settled_result(&rows).is_some() {
            return Err(ResourceFlightJournalError::Accounting);
        }
        admission_check(&rows, self.cap, record)?;
        let encoded =
            serde_json::to_vec(record).map_err(|source| ResourceFlightJournalError::Encode {
                path: path.clone(),
                source,
            })?;
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .map_err(|source| ResourceFlightJournalError::Io {
                path: path.clone(),
                source,
            })?;
        file.write_all(&encoded)
            .and_then(|()| file.write_all(b"\n"))
            .and_then(|()| file.sync_all())
            .map_err(|source| ResourceFlightJournalError::Io {
                path: path.clone(),
                source,
            })?;
        File::open(&self.root)
            .and_then(|dir| dir.sync_all())
            .map_err(|source| ResourceFlightJournalError::Io {
                path: self.root.clone(),
                source,
            })
    }

    fn append_terminal(
        &self,
        id: &ResourceFlightIdV1,
        result: &ResourceActionResultV1,
    ) -> Result<ResourceFlightTerminalAppendOutcomeV1, ResourceFlightJournalError> {
        let _write = self
            .append_lock
            .lock()
            .map_err(|_| ResourceFlightJournalError::Accounting)?;
        let path = self.path(id);
        let mut rows = self.read(&path)?;
        if let Some(existing) = settled_result(&rows) {
            return Ok(ResourceFlightTerminalAppendOutcomeV1::AlreadySettled { existing });
        }
        let record = ResourceFlightJournalRecordV1 {
            schema_version: RESOURCE_FLIGHT_JOURNAL_SCHEMA_V1,
            sequence: u64::try_from(rows.len() + 1).unwrap_or(u64::MAX),
            event: ResourceFlightJournalEventV1::Settled {
                result: result.clone(),
            },
        };
        admission_check(&rows, self.cap, &record)?;
        let encoded =
            serde_json::to_vec(&record).map_err(|source| ResourceFlightJournalError::Encode {
                path: path.clone(),
                source,
            })?;
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .map_err(|source| ResourceFlightJournalError::Io {
                path: path.clone(),
                source,
            })?;
        file.write_all(&encoded)
            .and_then(|()| file.write_all(b"\n"))
            .and_then(|()| file.sync_all())
            .map_err(|source| ResourceFlightJournalError::Io {
                path: path.clone(),
                source,
            })?;
        File::open(&self.root)
            .and_then(|dir| dir.sync_all())
            .map_err(|source| ResourceFlightJournalError::Io {
                path: self.root.clone(),
                source,
            })?;
        rows.push(record);
        Ok(ResourceFlightTerminalAppendOutcomeV1::Appended { records: rows })
    }

    fn records(
        &self,
        id: &ResourceFlightIdV1,
    ) -> Result<Vec<ResourceFlightJournalRecordV1>, ResourceFlightJournalError> {
        let _read = self
            .append_lock
            .lock()
            .map_err(|_| ResourceFlightJournalError::Accounting)?;
        self.read(&self.path(id))
    }
}

/// Receives one flight-side aggregation per owner. Slice 5 is the sole
/// production writer of the durable `NodeCleanupRecordV2.collateral` row.
pub trait ResourceFlightResultPublisher: Send + Sync {
    fn publish(&self, aggregation: NodeCleanupAggregationV1);
}

#[derive(Default)]
pub struct NoopResourceFlightResultPublisher;
impl ResourceFlightResultPublisher for NoopResourceFlightResultPublisher {
    fn publish(&self, _: NodeCleanupAggregationV1) {}
}

/// Flight-side input to Slice 5's `NodeCleanupRecordV2.collateral` writer.
/// `collateral` describes the shared action for this owner; Slice 5 joins the
/// per-owner values to form its durable `CollateralResultV1` count.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NodeCleanupAggregationV1 {
    pub owner: ResourceFlightOwnerV1,
    pub resource_flight_id: ResourceFlightIdV1,
    pub result: ResourceActionResultV1,
    pub collateral: CollateralDispositionV1,
    pub affected_owner_count: u32,
}

fn collateral_disposition(
    recipient_count: usize,
    result: &ResourceActionResultV1,
) -> CollateralDispositionV1 {
    if recipient_count < 2 {
        return CollateralDispositionV1::None;
    }
    match &result.disposition {
        ResourceActionDispositionV1::Complete => CollateralDispositionV1::Complete,
        ResourceActionDispositionV1::Partial | ResourceActionDispositionV1::Failed => {
            CollateralDispositionV1::Partial
        }
        ResourceActionDispositionV1::Unknown => CollateralDispositionV1::Unknown,
        ResourceActionDispositionV1::NotNeeded => CollateralDispositionV1::None,
    }
}

fn publish_result(
    publisher: &dyn ResourceFlightResultPublisher,
    resource_flight_id: &ResourceFlightIdV1,
    recipients: impl IntoIterator<Item = ResourceFlightOwnerV1>,
    result: &ResourceActionResultV1,
) {
    let recipients: Vec<_> = recipients.into_iter().collect();
    let collateral = collateral_disposition(recipients.len(), result);
    let affected_owner_count = u32::try_from(recipients.len())
        .expect("recipient collection must fit the CollateralResultV1 u32 wire field");
    for owner in recipients {
        publisher.publish(NodeCleanupAggregationV1 {
            owner,
            resource_flight_id: resource_flight_id.clone(),
            result: result.clone(),
            collateral: collateral.clone(),
            affected_owner_count,
        });
    }
}

#[derive(Debug, thiserror::Error)]
pub enum RetainedResourceFlightError {
    #[error(transparent)]
    Journal(#[from] ResourceFlightJournalError),
    #[error("flight transition mutex is poisoned")]
    Poisoned,
    #[error("resource-flight owner key is empty")]
    InvalidOwner,
    #[error("resource-flight admission is closed")]
    AdmissionClosed,
    #[error("resource-flight intent initiator is not attached")]
    OwnerNotAttached,
    #[error("resource-flight transition refused in {state:?}")]
    TransitionRefused { state: Box<ResourceFlightStateV1> },
    #[error("owned process tree key does not match flight")]
    ProcessTreeKeyMismatch,
    #[error("resource-flight reservation belongs to a different attempt")]
    ReservationAttemptMismatch,
    #[error("resource-flight reservation has no live capability: {resource_flight_id:?}")]
    ReservationUnavailable {
        resource_flight_id: ResourceFlightIdV1,
    },
    #[error("resource-flight terminal refusal after journal failure: {cause}")]
    TerminalRefusal {
        cause: Arc<ResourceFlightJournalError>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OwnerSnapshotV1 {
    pub owners: Vec<ResourceFlightOwnerV1>,
}

/// Proof of journaled dispatch admission. It has no effectful method.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JournaledDispatchAdmissionV1 {
    _resource_flight_id: ResourceFlightIdV1,
}

#[derive(Clone)]
pub struct RetainedResourceFlightConfigV1 {
    pub flight_id: ResourceFlightIdV1,
    pub attempt_id: AttemptId,
    pub key: ResourceFlightKeyV1,
    pub journal: Arc<dyn ResourceFlightJournal>,
    pub clock: Arc<dyn MonotonicClock>,
    pub result_publisher: Arc<dyn ResourceFlightResultPublisher>,
}

struct State {
    state: ResourceFlightStateV1,
    owners: BTreeSet<ResourceFlightOwnerV1>,
    snapshot: BTreeSet<ResourceFlightOwnerV1>,
    collateral: BTreeSet<ResourceFlightOwnerV1>,
    delivered: BTreeSet<ResourceFlightOwnerV1>,
    process_lifecycle_reserved: bool,
    sequence: u64,
    next_guard: u64,
    guards: BTreeSet<u64>,
    refusal: Option<Arc<ResourceFlightJournalError>>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            state: ResourceFlightStateV1::Open {},
            owners: BTreeSet::new(),
            snapshot: BTreeSet::new(),
            collateral: BTreeSet::new(),
            delivered: BTreeSet::new(),
            process_lifecycle_reserved: false,
            sequence: 1,
            next_guard: 1,
            guards: BTreeSet::new(),
            refusal: None,
        }
    }
}

struct SettlementV1 {
    result: ResourceActionResultV1,
    recipients: Vec<ResourceFlightOwnerV1>,
    appended: bool,
}

struct DeferredPublicationV1 {
    resource_flight_id: ResourceFlightIdV1,
    recipients: BTreeSet<ResourceFlightOwnerV1>,
    result: ResourceActionResultV1,
}

type RecoveredReservationV1 = (ResourceFlightReservationV1, Option<DeferredPublicationV1>);

enum JournaledRecoveryV1 {
    Existing(ResourceActionResultV1),
    Won {
        result: ResourceActionResultV1,
        recipients: BTreeSet<ResourceFlightOwnerV1>,
    },
}

pub struct RetainedResourceFlight {
    id: ResourceFlightIdV1,
    attempt_id: AttemptId,
    key: ResourceFlightKeyV1,
    journal: Arc<dyn ResourceFlightJournal>,
    clock: Arc<dyn MonotonicClock>,
    publisher: Arc<dyn ResourceFlightResultPublisher>,
    transition: Mutex<State>,
    settled: Condvar,
}

impl RetainedResourceFlight {
    fn create(
        config: RetainedResourceFlightConfigV1,
    ) -> Result<Arc<Self>, RetainedResourceFlightError> {
        let flight = Arc::new(Self {
            id: config.flight_id,
            attempt_id: config.attempt_id,
            key: config.key,
            journal: config.journal,
            clock: config.clock,
            publisher: config.result_publisher,
            transition: Mutex::new(State::default()),
            settled: Condvar::new(),
        });
        let mut state = flight.lock()?;
        flight.append(
            &mut state,
            ResourceFlightJournalEventV1::FlightReserved {
                key: flight.key.clone(),
                attempt_id: flight.attempt_id.clone(),
            },
        )?;
        drop(state);
        Ok(flight)
    }

    #[must_use]
    pub fn flight_id(&self) -> &ResourceFlightIdV1 {
        &self.id
    }
    #[must_use]
    pub fn key(&self) -> &ResourceFlightKeyV1 {
        &self.key
    }
    pub fn state(&self) -> Result<ResourceFlightStateV1, RetainedResourceFlightError> {
        let state = self.lock()?;
        if let Some(cause) = &state.refusal {
            return Err(RetainedResourceFlightError::TerminalRefusal {
                cause: Arc::clone(cause),
            });
        }
        Ok(state.state.clone())
    }

    fn lock(&self) -> Result<std::sync::MutexGuard<'_, State>, RetainedResourceFlightError> {
        self.transition
            .lock()
            .map_err(|_| RetainedResourceFlightError::Poisoned)
    }

    fn append(
        &self,
        state: &mut State,
        event: ResourceFlightJournalEventV1,
    ) -> Result<(), RetainedResourceFlightError> {
        self.journal.append(
            &self.id,
            &ResourceFlightJournalRecordV1 {
                schema_version: RESOURCE_FLIGHT_JOURNAL_SCHEMA_V1,
                sequence: state.sequence,
                event,
            },
        )?;
        state.sequence = state.sequence.saturating_add(1);
        Ok(())
    }

    /// Reconcile a terminal record written by recovery before this live
    /// capability attempts its next transition. The journal remains the
    /// terminal authority, so a recovered result must be adopted locally.
    fn adopt_durable_terminal_locked(
        &self,
        state: &mut State,
    ) -> Result<Option<ResourceActionResultV1>, RetainedResourceFlightError> {
        let rows = self.journal.records(&self.id)?;
        let Some(result) = settled_result(&rows) else {
            return Ok(None);
        };
        state.sequence = u64::try_from(rows.len() + 1).unwrap_or(u64::MAX);
        state.state = ResourceFlightStateV1::Settled {
            result: result.clone(),
        };
        self.settled.notify_all();
        Ok(Some(result))
    }

    pub fn attach_owner(
        &self,
        owner: ResourceFlightOwnerV1,
    ) -> Result<bool, RetainedResourceFlightError> {
        let mut state = self.lock()?;
        if !matches!(state.state, ResourceFlightStateV1::Open {}) {
            return Err(RetainedResourceFlightError::AdmissionClosed);
        }
        if state.owners.contains(&owner) {
            return Ok(false);
        }
        self.append(
            &mut state,
            ResourceFlightJournalEventV1::OwnerAttached {
                owner: owner.clone(),
            },
        )?;
        state.owners.insert(owner);
        Ok(true)
    }

    /// Legacy V2 keeps the live owner set but deliberately spends no bounded
    /// journal rows on warm-session churn. Its compatibility journal is
    /// process-local evidence, while signal intent and terminal rows must stay
    /// available no matter how many sessions the long-lived backend served.
    pub(crate) fn attach_owner_legacy_v2(
        &self,
        owner: ResourceFlightOwnerV1,
    ) -> Result<bool, RetainedResourceFlightError> {
        let mut state = self.lock()?;
        if !matches!(state.state, ResourceFlightStateV1::Open {}) {
            return Err(RetainedResourceFlightError::AdmissionClosed);
        }
        Ok(state.owners.insert(owner))
    }

    /// Ordinary session release detaches only that owner while admission remains open.
    /// It never signals or retires the process generation.
    pub fn detach_owner(
        &self,
        owner: &ResourceFlightOwnerV1,
    ) -> Result<bool, RetainedResourceFlightError> {
        let mut state = self.lock()?;
        if !matches!(state.state, ResourceFlightStateV1::Open {}) {
            return Err(RetainedResourceFlightError::AdmissionClosed);
        }
        if !state.owners.remove(owner) {
            return Ok(false);
        }
        if let Err(error) = self.append(
            &mut state,
            ResourceFlightJournalEventV1::OwnerDetached {
                owner: owner.clone(),
            },
        ) {
            state.owners.insert(owner.clone());
            return Err(error);
        }
        Ok(true)
    }

    pub(crate) fn detach_owner_legacy_v2(
        &self,
        owner: &ResourceFlightOwnerV1,
    ) -> Result<bool, RetainedResourceFlightError> {
        let mut state = self.lock()?;
        if !matches!(state.state, ResourceFlightStateV1::Open {}) {
            return Err(RetainedResourceFlightError::AdmissionClosed);
        }
        Ok(state.owners.remove(owner))
    }

    /// Reserve every row that a protected process may need before the spawn
    /// syscall. The journal append lock makes this atomic with concurrent
    /// evidence admission, whose rows can use only capacity beyond these
    /// outstanding slots.
    pub(crate) fn reserve_process_lifecycle(&self) -> Result<(), RetainedResourceFlightError> {
        let mut state = self.lock()?;
        if !matches!(state.state, ResourceFlightStateV1::Open {}) {
            return Err(RetainedResourceFlightError::AdmissionClosed);
        }
        if state.process_lifecycle_reserved {
            return Ok(());
        }
        self.append(
            &mut state,
            ResourceFlightJournalEventV1::ProcessLifecycleReserved {
                reserved_lifecycle_slots: PROCESS_LIFECYCLE_SLOTS,
            },
        )?;
        state.process_lifecycle_reserved = true;
        Ok(())
    }

    /// The single transition lock linearizes attachment with deterministic close.
    pub fn close_admission(&self) -> Result<OwnerSnapshotV1, RetainedResourceFlightError> {
        let mut state = self.lock()?;
        if !matches!(state.state, ResourceFlightStateV1::Open {}) {
            return Err(RetainedResourceFlightError::TransitionRefused {
                state: Box::new(state.state.clone()),
            });
        }
        state.snapshot = state.owners.clone();
        state.state = ResourceFlightStateV1::AdmissionClosed {};
        Ok(OwnerSnapshotV1 {
            owners: state.snapshot.iter().cloned().collect(),
        })
    }

    /// Stores exact owner snapshot and reserves terminal journal capacity before
    /// dispatch can become possible.
    pub fn journal_intent(
        &self,
        intent: ResourceActionIntentV1,
    ) -> Result<(), RetainedResourceFlightError> {
        let mut state = self.lock()?;
        if !matches!(state.state, ResourceFlightStateV1::AdmissionClosed {}) {
            return Err(RetainedResourceFlightError::TransitionRefused {
                state: Box::new(state.state.clone()),
            });
        }
        if !state.snapshot.contains(&intent.initiator) {
            return Err(RetainedResourceFlightError::OwnerNotAttached);
        }
        let snapshot = state.snapshot.iter().cloned().collect();
        let reserved_lifecycle_slots = if state.process_lifecycle_reserved {
            0
        } else {
            LIFECYCLE_SLOTS
        };
        self.append(
            &mut state,
            ResourceFlightJournalEventV1::IntentJournaled {
                intent,
                owner_snapshot: snapshot,
                reserved_lifecycle_slots,
            },
        )?;
        state.state = ResourceFlightStateV1::IntentJournaled {};
        Ok(())
    }

    /// Late discovery is separate from attachment: it is journaled after close,
    /// capacity bounded, and races settlement through the same transition lock.
    pub fn discover_collateral_owner(
        &self,
        owner: ResourceFlightOwnerV1,
    ) -> Result<bool, RetainedResourceFlightError> {
        let mut state = self.lock()?;
        if !matches!(
            state.state,
            ResourceFlightStateV1::AdmissionClosed {}
                | ResourceFlightStateV1::IntentJournaled {}
                | ResourceFlightStateV1::Signaling {}
        ) {
            return Err(RetainedResourceFlightError::TransitionRefused {
                state: Box::new(state.state.clone()),
            });
        }
        if state.snapshot.contains(&owner) || state.collateral.contains(&owner) {
            return Ok(false);
        }
        self.append(
            &mut state,
            ResourceFlightJournalEventV1::CollateralDiscovered {
                owner: owner.clone(),
            },
        )?;
        state.collateral.insert(owner);
        Ok(true)
    }

    /// Journal-before-dispatch transition for effectful adapters. There is no method in
    /// this module that can convert this admission into an OS/container call.
    pub fn begin_journaled_dispatch(
        &self,
    ) -> Result<JournaledDispatchAdmissionV1, RetainedResourceFlightError> {
        let mut state = self.lock()?;
        if self.adopt_durable_terminal_locked(&mut state)?.is_some() {
            return Err(RetainedResourceFlightError::TransitionRefused {
                state: Box::new(state.state.clone()),
            });
        }
        if !matches!(state.state, ResourceFlightStateV1::IntentJournaled {}) {
            return Err(RetainedResourceFlightError::TransitionRefused {
                state: Box::new(state.state.clone()),
            });
        }
        if let Err(error) =
            self.append(&mut state, ResourceFlightJournalEventV1::DispatchStarted {})
        {
            if self.adopt_durable_terminal_locked(&mut state)?.is_some() {
                return Err(RetainedResourceFlightError::TransitionRefused {
                    state: Box::new(state.state.clone()),
                });
            }
            return Err(error);
        }
        state.state = ResourceFlightStateV1::Signaling {};
        Ok(JournaledDispatchAdmissionV1 {
            _resource_flight_id: self.id.clone(),
        })
    }

    /// Append the complete, bounded outcome set for one signal dispatch. The row
    /// consumes the slot reserved by `journal_intent`; even failed syscalls are
    /// evidence and must be present before settlement.
    pub fn journal_process_signals(
        &self,
        observations: Vec<ProcessSignalObservationV1>,
    ) -> Result<(), RetainedResourceFlightError> {
        let mut state = self.lock()?;
        if !matches!(state.state, ResourceFlightStateV1::Signaling {}) {
            return Err(RetainedResourceFlightError::TransitionRefused {
                state: Box::new(state.state.clone()),
            });
        }
        self.append(
            &mut state,
            ResourceFlightJournalEventV1::ProcessSignalsObserved { observations },
        )
    }

    fn settle_locked(
        &self,
        state: &mut State,
        proposed: ResourceActionResultV1,
    ) -> Result<SettlementV1, RetainedResourceFlightError> {
        if let Some(cause) = &state.refusal {
            return Err(RetainedResourceFlightError::TerminalRefusal {
                cause: Arc::clone(cause),
            });
        }
        if !matches!(state.state, ResourceFlightStateV1::Signaling {}) {
            return Err(RetainedResourceFlightError::TransitionRefused {
                state: Box::new(state.state.clone()),
            });
        }
        if let Some(existing) = self.adopt_durable_terminal_locked(state)? {
            return Ok(SettlementV1 {
                result: existing,
                recipients: Vec::new(),
                appended: false,
            });
        }
        match self.journal.append_terminal(&self.id, &proposed) {
            Ok(ResourceFlightTerminalAppendOutcomeV1::Appended { .. }) => {
                state.sequence = state.sequence.saturating_add(1);
                state.state = ResourceFlightStateV1::Settled {
                    result: proposed.clone(),
                };
                let targets: Vec<_> = state.snapshot.union(&state.collateral).cloned().collect();
                let recipients = targets
                    .into_iter()
                    .filter(|owner| state.delivered.insert(owner.clone()))
                    .collect();
                self.settled.notify_all();
                Ok(SettlementV1 {
                    result: proposed,
                    recipients,
                    appended: true,
                })
            }
            Ok(ResourceFlightTerminalAppendOutcomeV1::AlreadySettled { existing }) => {
                state.state = ResourceFlightStateV1::Settled {
                    result: existing.clone(),
                };
                self.settled.notify_all();
                Ok(SettlementV1 {
                    result: existing,
                    recipients: Vec::new(),
                    appended: false,
                })
            }
            Err(cause) => {
                let cause = Arc::new(cause);
                state.refusal = Some(Arc::clone(&cause));
                self.settled.notify_all();
                Err(RetainedResourceFlightError::TerminalRefusal { cause })
            }
        }
    }

    fn publish(&self, recipients: Vec<ResourceFlightOwnerV1>, result: &ResourceActionResultV1) {
        publish_result(self.publisher.as_ref(), &self.id, recipients, result);
    }

    pub fn settle(
        &self,
        result: ResourceActionResultV1,
    ) -> Result<ResourceActionResultV1, RetainedResourceFlightError> {
        let settlement = {
            let mut state = self.lock()?;
            self.settle_locked(&mut state, result)?
        };
        self.publish(settlement.recipients, &settlement.result);
        Ok(settlement.result)
    }

    pub fn join_blocking(&self) -> Result<ResourceActionResultV1, RetainedResourceFlightError> {
        let mut state = self.lock()?;
        loop {
            if let Some(cause) = &state.refusal {
                return Err(RetainedResourceFlightError::TerminalRefusal {
                    cause: Arc::clone(cause),
                });
            }
            if let ResourceFlightStateV1::Settled { result } = &state.state {
                return Ok(result.clone());
            }
            state = self
                .settled
                .wait(state)
                .map_err(|_| RetainedResourceFlightError::Poisoned)?;
        }
    }

    pub fn retain(
        self: &Arc<Self>,
    ) -> Result<RetainedResourceFlightGuardV1, RetainedResourceFlightError> {
        let mut state = self.lock()?;
        let token = state.next_guard;
        state.next_guard = state.next_guard.saturating_add(1);
        state.guards.insert(token);
        Ok(RetainedResourceFlightGuardV1 {
            flight: Arc::clone(self),
            token,
        })
    }

    fn settle_unknown(&self) -> Result<ResourceActionResultV1, RetainedResourceFlightError> {
        let proposed = ResourceActionResultV1 {
            disposition: ResourceActionDispositionV1::Unknown,
            duration_ms: self.clock.elapsed_ms(),
            recovery_owner: None,
            cause: None,
        };
        let settlement = {
            let mut state = self.lock()?;
            self.settle_locked(&mut state, proposed)?
        };
        self.publish(settlement.recipients, &settlement.result);
        Ok(settlement.result)
    }

    /// No production timer is started here. At a supplied clock deadline this
    /// consumes the exact guard, journals transfer before terminal settlement,
    /// and returns that guard to the recovery owner.
    pub fn transfer_cleanup_deadline(
        self: &Arc<Self>,
        deadline_ms: u64,
        guard: RetainedResourceFlightGuardV1,
        reason: BoundedRecoveryReasonV1,
    ) -> Result<CleanupDeadlineTransferV1, RetainedResourceFlightError> {
        if self.clock.elapsed_ms() < deadline_ms {
            return Ok(CleanupDeadlineTransferV1::NotDue { guard });
        }
        if !Arc::ptr_eq(self, &guard.flight) {
            return Ok(CleanupDeadlineTransferV1::Unknown {
                result: self.settle_unknown()?,
                guard,
            });
        }
        let recovery_owner = RecoveryOwnerV1 {
            attempt_id: self.attempt_id.clone(),
            resource_flight_id: self.id.clone(),
            reason,
        };
        let result = ResourceActionResultV1 {
            disposition: ResourceActionDispositionV1::Partial,
            duration_ms: self.clock.elapsed_ms(),
            recovery_owner: Some(recovery_owner.clone()),
            cause: None,
        };
        let settlement = {
            let mut state = self.lock()?;
            if !state.guards.contains(&guard.token) {
                drop(state);
                return Ok(CleanupDeadlineTransferV1::Unknown {
                    result: self.settle_unknown()?,
                    guard,
                });
            }
            if let Some(existing) = self.adopt_durable_terminal_locked(&mut state)? {
                return Ok(CleanupDeadlineTransferV1::Unknown {
                    result: existing,
                    guard,
                });
            }
            if !matches!(state.state, ResourceFlightStateV1::Signaling {}) {
                return Err(RetainedResourceFlightError::TransitionRefused {
                    state: Box::new(state.state.clone()),
                });
            }
            if let Err(error) = self.append(
                &mut state,
                ResourceFlightJournalEventV1::GuardTransferred {
                    recovery_owner: recovery_owner.clone(),
                    deadline_ms,
                },
            ) {
                let Some(existing) = self.adopt_durable_terminal_locked(&mut state)? else {
                    return Err(error);
                };
                return Ok(CleanupDeadlineTransferV1::Unknown {
                    result: existing,
                    guard,
                });
            }
            self.settle_locked(&mut state, result)?
        };
        let appended = settlement.appended;
        self.publish(settlement.recipients, &settlement.result);
        if !appended {
            return Ok(CleanupDeadlineTransferV1::Unknown {
                result: settlement.result,
                guard,
            });
        }
        Ok(CleanupDeadlineTransferV1::Transferred {
            recovery_owner,
            guard,
        })
    }

    fn recover_journaled_intent_as_unknown(
        journal: &dyn ResourceFlightJournal,
        id: &ResourceFlightIdV1,
        duration_ms: u64,
    ) -> Result<Option<JournaledRecoveryV1>, RetainedResourceFlightError> {
        let rows = journal.records(id)?;
        if let Some(existing) = settled_result(&rows) {
            return Ok(Some(JournaledRecoveryV1::Existing(existing)));
        }
        if !rows.iter().any(|row| {
            matches!(
                &row.event,
                ResourceFlightJournalEventV1::IntentJournaled { .. }
            )
        }) {
            return Ok(None);
        }
        let result = ResourceActionResultV1 {
            disposition: ResourceActionDispositionV1::Unknown,
            duration_ms,
            recovery_owner: None,
            cause: None,
        };
        match journal.append_terminal(id, &result)? {
            ResourceFlightTerminalAppendOutcomeV1::Appended { records } => {
                let recipients = records.iter().fold(BTreeSet::new(), |mut recipients, row| {
                    match &row.event {
                        ResourceFlightJournalEventV1::IntentJournaled {
                            owner_snapshot, ..
                        } => {
                            recipients.extend(owner_snapshot.iter().cloned());
                        }
                        ResourceFlightJournalEventV1::CollateralDiscovered { owner } => {
                            recipients.insert(owner.clone());
                        }
                        _ => {}
                    }
                    recipients
                });
                Ok(Some(JournaledRecoveryV1::Won { result, recipients }))
            }
            ResourceFlightTerminalAppendOutcomeV1::AlreadySettled { existing } => {
                Ok(Some(JournaledRecoveryV1::Existing(existing)))
            }
        }
    }

    /// This recovery route deliberately takes only a journal, flight id, and
    /// result publisher. It has neither a resource identity nor any dispatch
    /// capability. It publishes only when its terminal CAS wins.
    pub fn recover_dead_journaled_intent_as_unknown(
        journal: &dyn ResourceFlightJournal,
        id: &ResourceFlightIdV1,
        duration_ms: u64,
        publisher: &dyn ResourceFlightResultPublisher,
    ) -> Result<Option<ResourceActionResultV1>, RetainedResourceFlightError> {
        match Self::recover_journaled_intent_as_unknown(journal, id, duration_ms)? {
            Some(JournaledRecoveryV1::Won { result, recipients }) => {
                publish_result(publisher, id, recipients, &result);
                Ok(Some(result))
            }
            Some(JournaledRecoveryV1::Existing(_)) | None => Ok(None),
        }
    }

    pub(crate) fn journal_process_tree(
        &self,
        identity: ResourceIdentityV1,
    ) -> Result<(), RetainedResourceFlightError> {
        if ResourceFlightKeyV1::from_identity(&identity) != self.key {
            return Err(RetainedResourceFlightError::ProcessTreeKeyMismatch);
        }
        let mut state = self.lock()?;
        if !matches!(state.state, ResourceFlightStateV1::Open {}) {
            return Err(RetainedResourceFlightError::TransitionRefused {
                state: Box::new(state.state.clone()),
            });
        }
        self.append(
            &mut state,
            ResourceFlightJournalEventV1::ProcessTreeCaptured { identity },
        )
    }

    /// Protective pre-bind evidence. The caller follows this with an ordinary
    /// empty-signal flight settlement; it never invents a PID capability from
    /// this diagnostic row.
    pub fn journal_process_binding_failure(
        &self,
        stage: ProcessBindingStageV1,
        pid: Option<u32>,
    ) -> Result<(), RetainedResourceFlightError> {
        let mut state = self.lock()?;
        if !matches!(state.state, ResourceFlightStateV1::Open {}) {
            return Err(RetainedResourceFlightError::TransitionRefused {
                state: Box::new(state.state.clone()),
            });
        }
        self.append(
            &mut state,
            ResourceFlightJournalEventV1::ProcessBindingFailed { stage, pid },
        )
    }
}

/// Exact, non-cloneable retention. Drop only releases an in-memory token; it
/// cannot signal anything.
pub struct RetainedResourceFlightGuardV1 {
    flight: Arc<RetainedResourceFlight>,
    token: u64,
}
impl RetainedResourceFlightGuardV1 {
    #[must_use]
    pub fn resource_flight_id(&self) -> &ResourceFlightIdV1 {
        self.flight.flight_id()
    }
}
impl Drop for RetainedResourceFlightGuardV1 {
    fn drop(&mut self) {
        if let Ok(mut state) = self.flight.transition.lock() {
            state.guards.remove(&self.token);
        }
    }
}

pub enum CleanupDeadlineTransferV1 {
    NotDue {
        guard: RetainedResourceFlightGuardV1,
    },
    Transferred {
        recovery_owner: RecoveryOwnerV1,
        guard: RetainedResourceFlightGuardV1,
    },
    Unknown {
        result: ResourceActionResultV1,
        guard: RetainedResourceFlightGuardV1,
    },
}

/// Sole live capability for an owned process generation. Capture-only values
/// remain available for persisted-identity decoding tests, but cannot signal.
#[derive(Clone)]
pub struct OwnedProcessTreeV1 {
    identity: ResourceIdentityV1,
    pub(crate) process: Option<Arc<crate::process::OwnedProcessStateV1>>,
}
impl std::fmt::Debug for OwnedProcessTreeV1 {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OwnedProcessTreeV1")
            .field("identity", &self.identity)
            .field("live_capability", &self.process.is_some())
            .finish()
    }
}
impl OwnedProcessTreeV1 {
    pub fn capture(
        generation: impl Into<String>,
        spawn_nonce: [u8; 32],
        pid: u32,
        pgid: Option<u32>,
        immutable_start: ProcessStartIdentityV1,
    ) -> Result<Self, RetainedResourceFlightError> {
        if immutable_start.pid != pid {
            return Err(RetainedResourceFlightError::ProcessTreeKeyMismatch);
        }
        let value = digest(&SHA256, &spawn_nonce)
            .as_ref()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        let spawn_nonce_sha256 = Sha256HexV1::parse(value)
            .map_err(|_| RetainedResourceFlightError::ProcessTreeKeyMismatch)?;
        Ok(Self {
            identity: ResourceIdentityV1::AcpProcess {
                generation: generation.into(),
                spawn_nonce_sha256,
                pid,
                pgid,
                immutable_start,
            },
            process: None,
        })
    }
    #[must_use]
    pub fn identity(&self) -> &ResourceIdentityV1 {
        &self.identity
    }
    pub(crate) fn from_bound(
        identity: ResourceIdentityV1,
        process: Arc<crate::process::OwnedProcessStateV1>,
    ) -> Self {
        Self {
            identity,
            process: Some(process),
        }
    }
    pub fn journal_capture(
        &self,
        flight: &RetainedResourceFlight,
    ) -> Result<(), RetainedResourceFlightError> {
        flight.journal_process_tree(self.identity.clone())
    }
}

#[derive(Clone)]
pub enum ResourceFlightReservationV1 {
    Created(Arc<RetainedResourceFlight>),
    Joined(Arc<RetainedResourceFlight>),
    /// A durable reservation survived without a live retained capability. Its
    /// terminal result is recovered from the original journal, never by
    /// constructing another flight from PID/name/metadata.
    Recovered {
        resource_flight_id: ResourceFlightIdV1,
        result: ResourceActionResultV1,
    },
}
impl ResourceFlightReservationV1 {
    #[must_use]
    pub fn flight(&self) -> Option<&Arc<RetainedResourceFlight>> {
        match self {
            Self::Created(flight) | Self::Joined(flight) => Some(flight),
            Self::Recovered { .. } => None,
        }
    }

    #[must_use]
    pub fn recovered_result(&self) -> Option<&ResourceActionResultV1> {
        match self {
            Self::Recovered { result, .. } => Some(result),
            Self::Created(_) | Self::Joined(_) => None,
        }
    }
}

/// Per-attempt reservation map. The in-memory map holds a strong Arc for the
/// registry lifetime; the journal's create-new reservation file is the
/// durable CAS that prevents a dead generation from being re-minted.
pub struct ResourceFlightRegistryV1 {
    attempt_id: AttemptId,
    flights: Mutex<BTreeMap<ResourceFlightKeyV1, Arc<RetainedResourceFlight>>>,
}
/// The registry is the only public acquisition path for a retained flight.
/// `RetainedResourceFlight::create` is module-private, so downstream code
/// cannot mint a capability that bypasses the reservation CAS.
///
/// ```compile_fail,E0624
/// use bridge_core::resource_flight::RetainedResourceFlight;
/// let _ = RetainedResourceFlight::create;
/// ```
impl ResourceFlightRegistryV1 {
    #[must_use]
    pub fn new(attempt_id: AttemptId) -> Self {
        Self {
            attempt_id,
            flights: Mutex::new(BTreeMap::new()),
        }
    }

    fn recovered_reservation(
        config: &RetainedResourceFlightConfigV1,
        reservation: ResourceFlightReservationRecordV1,
    ) -> Result<RecoveredReservationV1, RetainedResourceFlightError> {
        if reservation.attempt_id != config.attempt_id {
            return Err(RetainedResourceFlightError::ReservationAttemptMismatch);
        }
        let resource_flight_id = reservation.resource_flight_id;
        match RetainedResourceFlight::recover_journaled_intent_as_unknown(
            config.journal.as_ref(),
            &resource_flight_id,
            config.clock.elapsed_ms(),
        )? {
            Some(JournaledRecoveryV1::Existing(result)) => Ok((
                ResourceFlightReservationV1::Recovered {
                    resource_flight_id,
                    result,
                },
                None,
            )),
            Some(JournaledRecoveryV1::Won { result, recipients }) => Ok((
                ResourceFlightReservationV1::Recovered {
                    resource_flight_id: resource_flight_id.clone(),
                    result: result.clone(),
                },
                Some(DeferredPublicationV1 {
                    resource_flight_id,
                    recipients,
                    result,
                }),
            )),
            None => Err(RetainedResourceFlightError::ReservationUnavailable { resource_flight_id }),
        }
    }

    pub fn reserve(
        &self,
        config: RetainedResourceFlightConfigV1,
    ) -> Result<ResourceFlightReservationV1, RetainedResourceFlightError> {
        if self.attempt_id != config.attempt_id {
            return Err(RetainedResourceFlightError::ReservationAttemptMismatch);
        }
        let (result, deferred_publication) = {
            let mut flights = self
                .flights
                .lock()
                .map_err(|_| RetainedResourceFlightError::Poisoned)?;
            if let Some(flight) = flights.get(&config.key) {
                return Ok(ResourceFlightReservationV1::Joined(Arc::clone(flight)));
            }
            let reservation = ResourceFlightReservationRecordV1 {
                schema_version: RESOURCE_FLIGHT_JOURNAL_SCHEMA_V1,
                key: config.key.clone(),
                attempt_id: config.attempt_id.clone(),
                resource_flight_id: config.flight_id.clone(),
            };
            match config.journal.reserve_flight(&reservation)? {
                ResourceFlightReservationOutcomeV1::Created => {
                    let key = config.key.clone();
                    let flight = RetainedResourceFlight::create(config)?;
                    flights.insert(key, Arc::clone(&flight));
                    return Ok(ResourceFlightReservationV1::Created(flight));
                }
                ResourceFlightReservationOutcomeV1::Existing(reservation) => {
                    Self::recovered_reservation(&config, reservation)?
                }
            }
        };
        if let Some(DeferredPublicationV1 {
            resource_flight_id,
            recipients,
            result,
        }) = deferred_publication
        {
            publish_result(
                config.result_publisher.as_ref(),
                &resource_flight_id,
                recipients,
                &result,
            );
        }
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    use std::sync::{mpsc, Barrier};
    use std::time::Duration;

    #[derive(Default)]
    struct Publisher(Mutex<Vec<NodeCleanupAggregationV1>>);
    impl ResourceFlightResultPublisher for Publisher {
        fn publish(&self, aggregation: NodeCleanupAggregationV1) {
            self.0.lock().unwrap().push(aggregation);
        }
    }

    struct FailingTerminalAppendJournal {
        inner: InMemoryResourceFlightJournal,
    }

    impl ResourceFlightJournal for FailingTerminalAppendJournal {
        fn reserve_flight(
            &self,
            reservation: &ResourceFlightReservationRecordV1,
        ) -> Result<ResourceFlightReservationOutcomeV1, ResourceFlightJournalError> {
            self.inner.reserve_flight(reservation)
        }

        fn append(
            &self,
            id: &ResourceFlightIdV1,
            record: &ResourceFlightJournalRecordV1,
        ) -> Result<(), ResourceFlightJournalError> {
            self.inner.append(id, record)
        }

        fn append_terminal(
            &self,
            _: &ResourceFlightIdV1,
            _: &ResourceActionResultV1,
        ) -> Result<ResourceFlightTerminalAppendOutcomeV1, ResourceFlightJournalError> {
            Err(ResourceFlightJournalError::Io {
                path: PathBuf::from("injected-terminal-append"),
                source: std::io::Error::other("injected terminal append failure"),
            })
        }

        fn records(
            &self,
            id: &ResourceFlightIdV1,
        ) -> Result<Vec<ResourceFlightJournalRecordV1>, ResourceFlightJournalError> {
            self.inner.records(id)
        }
    }

    struct RecoverySnapshotJournal {
        inner: InMemoryResourceFlightJournal,
        pause_next_records: AtomicBool,
        snapshot_taken: Barrier,
        allow_terminal_append: Barrier,
    }

    impl RecoverySnapshotJournal {
        fn new(cap: usize) -> Self {
            Self {
                inner: InMemoryResourceFlightJournal::new(cap),
                pause_next_records: AtomicBool::new(false),
                snapshot_taken: Barrier::new(2),
                allow_terminal_append: Barrier::new(2),
            }
        }
    }

    impl ResourceFlightJournal for RecoverySnapshotJournal {
        fn reserve_flight(
            &self,
            reservation: &ResourceFlightReservationRecordV1,
        ) -> Result<ResourceFlightReservationOutcomeV1, ResourceFlightJournalError> {
            self.inner.reserve_flight(reservation)
        }

        fn append(
            &self,
            id: &ResourceFlightIdV1,
            record: &ResourceFlightJournalRecordV1,
        ) -> Result<(), ResourceFlightJournalError> {
            self.inner.append(id, record)
        }

        fn append_terminal(
            &self,
            id: &ResourceFlightIdV1,
            result: &ResourceActionResultV1,
        ) -> Result<ResourceFlightTerminalAppendOutcomeV1, ResourceFlightJournalError> {
            self.inner.append_terminal(id, result)
        }

        fn records(
            &self,
            id: &ResourceFlightIdV1,
        ) -> Result<Vec<ResourceFlightJournalRecordV1>, ResourceFlightJournalError> {
            let rows = self.inner.records(id)?;
            if self.pause_next_records.swap(false, Ordering::SeqCst) {
                self.snapshot_taken.wait();
                self.allow_terminal_append.wait();
            }
            Ok(rows)
        }
    }

    struct ReentrantPublisher {
        registry: Arc<ResourceFlightRegistryV1>,
        reentrant_config: Mutex<Option<RetainedResourceFlightConfigV1>>,
        calls: AtomicU64,
    }

    impl ResourceFlightResultPublisher for ReentrantPublisher {
        fn publish(&self, _: NodeCleanupAggregationV1) {
            self.calls.fetch_add(1, Ordering::SeqCst);
            if let Some(config) = self.reentrant_config.lock().unwrap().take() {
                assert!(matches!(
                    self.registry.reserve(config),
                    Ok(ResourceFlightReservationV1::Recovered { .. })
                ));
            }
        }
    }

    struct ManualClock(AtomicU64);
    impl ManualClock {
        fn new(value: u64) -> Self {
            Self(AtomicU64::new(value))
        }
        fn set(&self, value: u64) {
            self.0.store(value, Ordering::SeqCst);
        }
    }
    impl MonotonicClock for ManualClock {
        fn elapsed_ms(&self) -> u64 {
            self.0.load(Ordering::SeqCst)
        }
    }

    fn id(digit: char) -> ResourceFlightIdV1 {
        ResourceFlightIdV1::parse(format!(
            "{}{}",
            ResourceFlightIdV1::PREFIX,
            digit.to_string().repeat(64)
        ))
        .unwrap()
    }
    fn attempt() -> AttemptId {
        AttemptId::parse(format!("attempt-{}", "1".repeat(32))).unwrap()
    }
    fn owner(node: &str, key: &str) -> ResourceFlightOwnerV1 {
        ResourceFlightOwnerV1::new(NodeId::parse(node).unwrap(), key).unwrap()
    }
    fn sha(digit: char) -> Sha256HexV1 {
        Sha256HexV1::parse(digit.to_string().repeat(64)).unwrap()
    }
    fn intent(initiator: ResourceFlightOwnerV1) -> ResourceActionIntentV1 {
        ResourceActionIntentV1 {
            initiator,
            capability_digest: sha('a'),
            cause: None,
        }
    }
    fn complete() -> ResourceActionResultV1 {
        ResourceActionResultV1 {
            disposition: ResourceActionDispositionV1::Complete,
            duration_ms: 8,
            recovery_owner: None,
            cause: None,
        }
    }
    fn config(
        digit: char,
        journal: Arc<dyn ResourceFlightJournal>,
        clock: Arc<dyn MonotonicClock>,
        publisher: Arc<dyn ResourceFlightResultPublisher>,
    ) -> RetainedResourceFlightConfigV1 {
        RetainedResourceFlightConfigV1 {
            flight_id: id(digit),
            attempt_id: attempt(),
            key: ResourceFlightKeyV1::AcpGeneration {
                generation: "shared-generation".to_string(),
            },
            journal,
            clock,
            result_publisher: publisher,
        }
    }
    fn create(
        cap: usize,
    ) -> (
        Arc<RetainedResourceFlight>,
        Arc<ManualClock>,
        Arc<Publisher>,
    ) {
        let clock = Arc::new(ManualClock::new(8));
        let publisher = Arc::new(Publisher::default());
        let reservation = ResourceFlightRegistryV1::new(attempt())
            .reserve(config(
                '1',
                Arc::new(InMemoryResourceFlightJournal::new(cap)),
                clock.clone(),
                publisher.clone(),
            ))
            .unwrap();
        let flight = Arc::clone(reservation.flight().unwrap());
        (flight, clock, publisher)
    }
    fn journaled() -> (
        Arc<RetainedResourceFlight>,
        Arc<ManualClock>,
        Arc<Publisher>,
    ) {
        let (flight, clock, publisher) = create(24);
        let primary = owner("node_a", "session-a");
        flight.attach_owner(primary.clone()).unwrap();
        flight.close_admission().unwrap();
        flight.journal_intent(intent(primary)).unwrap();
        (flight, clock, publisher)
    }

    #[test]
    fn admission_closed_without_journaled_intent_permits_no_signal() {
        let (flight, _, _) = create(24);
        flight.attach_owner(owner("node_a", "session-a")).unwrap();
        flight.close_admission().unwrap();
        let error = flight.begin_journaled_dispatch();
        assert!(matches!(
            error,
            Err(RetainedResourceFlightError::TransitionRefused { state })
                if matches!(state.as_ref(), ResourceFlightStateV1::AdmissionClosed {})
        ));
        assert!(matches!(
            flight.state().unwrap(),
            ResourceFlightStateV1::AdmissionClosed {}
        ));
    }

    #[test]
    fn bounded_journal_capacity_refuses_admission_before_provider_work() {
        let (flight, _, _) = create(2);
        let primary = owner("node_a", "session-a");
        flight.attach_owner(primary.clone()).unwrap();
        flight.close_admission().unwrap();
        assert!(matches!(
            flight.journal_intent(intent(primary)),
            Err(RetainedResourceFlightError::Journal(
                ResourceFlightJournalError::Full
            ))
        ));
        assert!(flight.begin_journaled_dispatch().is_err());
    }

    #[test]
    fn deterministic_owner_snapshot_order_and_collateral_discovery_delivery() {
        let (flight, _, publisher) = create(24);
        let z = owner("node_z", "session-z");
        let a = owner("node_a", "session-a");
        let discovered = owner("node_c", "session-c");
        flight.attach_owner(z.clone()).unwrap();
        flight.attach_owner(a.clone()).unwrap();
        assert_eq!(
            flight.close_admission().unwrap().owners,
            vec![a.clone(), z.clone()]
        );
        flight.journal_intent(intent(a.clone())).unwrap();
        assert!(flight
            .discover_collateral_owner(discovered.clone())
            .unwrap());
        assert!(!flight
            .discover_collateral_owner(discovered.clone())
            .unwrap());
        flight.begin_journaled_dispatch().unwrap();
        flight.settle(complete()).unwrap();
        let publications = publisher.0.lock().unwrap();
        let delivered: BTreeSet<_> = publications
            .iter()
            .map(|aggregation| aggregation.owner.clone())
            .collect();
        assert_eq!(delivered, BTreeSet::from([a, z, discovered]));
        assert_eq!(publications.len(), 3);
        assert!(publications.iter().all(|aggregation| {
            &aggregation.resource_flight_id == flight.flight_id()
                && aggregation.result == complete()
                && aggregation.collateral == CollateralDispositionV1::Complete
                && aggregation.affected_owner_count == 3
        }));
    }

    #[test]
    fn attach_after_close_is_refused() {
        let (flight, _, _) = create(24);
        flight.close_admission().unwrap();
        assert!(matches!(
            flight.attach_owner(owner("node_b", "session-b")),
            Err(RetainedResourceFlightError::AdmissionClosed)
        ));
    }

    #[test]
    fn attach_vs_close_race_attach_wins_then_close_snapshots_it() {
        let (flight, _, _) = create(24);
        let barrier = Arc::new(Barrier::new(2));
        let runner = flight.clone();
        let child_barrier = barrier.clone();
        let child = std::thread::spawn(move || {
            let result = runner.attach_owner(owner("node_b", "session-b"));
            child_barrier.wait();
            result
        });
        barrier.wait();
        let snapshot = flight.close_admission().unwrap();
        assert!(child.join().unwrap().unwrap());
        assert_eq!(snapshot.owners, vec![owner("node_b", "session-b")]);
    }

    #[test]
    fn attach_vs_close_race_close_wins_then_attach_refuses() {
        let (flight, _, _) = create(24);
        flight.close_admission().unwrap();
        let result = std::thread::spawn(move || flight.attach_owner(owner("node_b", "session-b")))
            .join()
            .unwrap();
        assert!(matches!(
            result,
            Err(RetainedResourceFlightError::AdmissionClosed)
        ));
    }

    #[test]
    fn second_requester_joins_same_live_generation_flight() {
        let registry = ResourceFlightRegistryV1::new(attempt());
        let journal: Arc<dyn ResourceFlightJournal> =
            Arc::new(InMemoryResourceFlightJournal::new(32));
        let clock: Arc<dyn MonotonicClock> = Arc::new(ManualClock::new(0));
        let publisher: Arc<dyn ResourceFlightResultPublisher> = Arc::new(Publisher::default());
        let first = registry
            .reserve(config(
                '1',
                journal.clone(),
                clock.clone(),
                publisher.clone(),
            ))
            .unwrap();
        let second = registry
            .reserve(config('2', journal, clock, publisher))
            .unwrap();
        assert!(matches!(&first, ResourceFlightReservationV1::Created(_)));
        assert!(matches!(&second, ResourceFlightReservationV1::Joined(_)));
        assert!(Arc::ptr_eq(
            first.flight().unwrap(),
            second.flight().unwrap()
        ));
    }

    #[test]
    fn registry_retains_generation_flight_after_requester_handles_drop() {
        let registry = ResourceFlightRegistryV1::new(attempt());
        let journal: Arc<dyn ResourceFlightJournal> =
            Arc::new(InMemoryResourceFlightJournal::new(32));
        let clock: Arc<dyn MonotonicClock> = Arc::new(ManualClock::new(0));
        let publisher: Arc<dyn ResourceFlightResultPublisher> = Arc::new(Publisher::default());
        let first = registry
            .reserve(config(
                '1',
                journal.clone(),
                clock.clone(),
                publisher.clone(),
            ))
            .unwrap();
        let original_id = first.flight().unwrap().flight_id().clone();
        drop(first);

        let second = registry
            .reserve(config('2', journal, clock, publisher))
            .unwrap();
        assert!(matches!(&second, ResourceFlightReservationV1::Joined(_)));
        assert_eq!(second.flight().unwrap().flight_id(), &original_id);
    }

    #[test]
    fn one_flight_per_generation_keying_and_dedicated_request_cardinality() {
        let registry = ResourceFlightRegistryV1::new(attempt());
        let journal: Arc<dyn ResourceFlightJournal> =
            Arc::new(InMemoryResourceFlightJournal::new(32));
        let clock: Arc<dyn MonotonicClock> = Arc::new(ManualClock::new(0));
        let publisher: Arc<dyn ResourceFlightResultPublisher> = Arc::new(Publisher::default());
        let shared = registry
            .reserve(config(
                '1',
                journal.clone(),
                clock.clone(),
                publisher.clone(),
            ))
            .unwrap();
        let mut request_a = config('2', journal.clone(), clock.clone(), publisher.clone());
        request_a.key = ResourceFlightKeyV1::DedicatedRemoteRequest {
            request_id: "request-a".to_string(),
        };
        let mut request_b = config('3', journal, clock, publisher);
        request_b.key = ResourceFlightKeyV1::DedicatedRemoteRequest {
            request_id: "request-b".to_string(),
        };
        let a = registry.reserve(request_a).unwrap();
        let b = registry.reserve(request_b).unwrap();
        assert!(!Arc::ptr_eq(shared.flight().unwrap(), a.flight().unwrap()));
        assert!(!Arc::ptr_eq(a.flight().unwrap(), b.flight().unwrap()));
    }

    #[test]
    fn a_generation_flight_is_unobtainable_outside_the_registry_reservation() {
        let registry = ResourceFlightRegistryV1::new(attempt());
        let journal: Arc<dyn ResourceFlightJournal> =
            Arc::new(InMemoryResourceFlightJournal::new(32));
        let clock: Arc<dyn MonotonicClock> = Arc::new(ManualClock::new(0));
        let publisher: Arc<dyn ResourceFlightResultPublisher> = Arc::new(Publisher::default());

        let first = registry
            .reserve(config(
                '1',
                journal.clone(),
                clock.clone(),
                publisher.clone(),
            ))
            .unwrap();
        let second = registry
            .reserve(config('2', journal, clock, publisher))
            .unwrap();

        assert!(matches!(&first, ResourceFlightReservationV1::Created(_)));
        assert!(matches!(&second, ResourceFlightReservationV1::Joined(_)));
        assert!(Arc::ptr_eq(
            first.flight().unwrap(),
            second.flight().unwrap()
        ));
    }

    #[test]
    fn a_dead_journaled_flight_settles_unknown_and_publishes_to_every_owner() {
        let root = tempfile::tempdir().unwrap();
        let journal: Arc<dyn ResourceFlightJournal> =
            Arc::new(FileResourceFlightJournal::open(root.path(), 32).unwrap());
        let clock: Arc<dyn MonotonicClock> = Arc::new(ManualClock::new(17));
        let publisher = Arc::new(Publisher::default());
        let first_registry = ResourceFlightRegistryV1::new(attempt());
        let first = first_registry
            .reserve(config(
                '1',
                journal.clone(),
                clock.clone(),
                publisher.clone(),
            ))
            .unwrap();
        let flight = Arc::clone(first.flight().unwrap());
        let primary = owner("node_a", "session-a");
        let peer = owner("node_b", "session-b");
        let collateral = owner("node_c", "session-c");
        flight.attach_owner(primary.clone()).unwrap();
        flight.attach_owner(peer.clone()).unwrap();
        flight.close_admission().unwrap();
        flight.journal_intent(intent(primary.clone())).unwrap();
        flight
            .discover_collateral_owner(collateral.clone())
            .unwrap();
        let original_id = flight.flight_id().clone();
        drop(flight);
        drop(first);
        drop(first_registry);

        let reopened: Arc<dyn ResourceFlightJournal> =
            Arc::new(FileResourceFlightJournal::open(root.path(), 32).unwrap());
        let recovered = ResourceFlightRegistryV1::new(attempt())
            .reserve(config(
                '2',
                reopened.clone(),
                clock.clone(),
                publisher.clone(),
            ))
            .unwrap();
        assert!(matches!(
            recovered,
            ResourceFlightReservationV1::Recovered {
                resource_flight_id,
                result: ResourceActionResultV1 {
                    disposition: ResourceActionDispositionV1::Unknown,
                    ..
                },
            } if resource_flight_id == original_id
        ));
        {
            let publications = publisher.0.lock().unwrap();
            assert_eq!(publications.len(), 3);
            assert_eq!(
                publications
                    .iter()
                    .map(|aggregation| aggregation.owner.clone())
                    .collect::<BTreeSet<_>>(),
                BTreeSet::from([primary, peer, collateral])
            );
            assert!(publications.iter().all(|aggregation| {
                aggregation.resource_flight_id == original_id
                    && aggregation.result.disposition == ResourceActionDispositionV1::Unknown
                    && aggregation.collateral == CollateralDispositionV1::Unknown
            }));
        }

        let second_recovery = ResourceFlightRegistryV1::new(attempt())
            .reserve(config('3', reopened, clock, publisher.clone()))
            .unwrap();
        assert!(matches!(
            second_recovery,
            ResourceFlightReservationV1::Recovered { .. }
        ));
        assert_eq!(publisher.0.lock().unwrap().len(), 3);
    }

    #[test]
    fn durable_generation_reservation_recovers_dead_journaled_flight_as_unknown_without_reminting()
    {
        let root = tempfile::tempdir().unwrap();
        let journal: Arc<dyn ResourceFlightJournal> =
            Arc::new(FileResourceFlightJournal::open(root.path(), 24).unwrap());
        let clock: Arc<dyn MonotonicClock> = Arc::new(ManualClock::new(3));
        let publisher: Arc<dyn ResourceFlightResultPublisher> = Arc::new(Publisher::default());
        let first_registry = ResourceFlightRegistryV1::new(attempt());
        let first = first_registry
            .reserve(config(
                '1',
                journal.clone(),
                clock.clone(),
                publisher.clone(),
            ))
            .unwrap();
        let flight = Arc::clone(first.flight().unwrap());
        let primary = owner("node_a", "session-a");
        flight.attach_owner(primary.clone()).unwrap();
        flight.close_admission().unwrap();
        flight.journal_intent(intent(primary)).unwrap();
        let original_id = flight.flight_id().clone();
        drop(flight);
        drop(first);
        drop(first_registry);

        let reopened: Arc<dyn ResourceFlightJournal> =
            Arc::new(FileResourceFlightJournal::open(root.path(), 24).unwrap());
        let recovered = ResourceFlightRegistryV1::new(attempt())
            .reserve(config('2', reopened.clone(), clock, publisher))
            .unwrap();
        let ResourceFlightReservationV1::Recovered {
            resource_flight_id,
            result,
        } = recovered
        else {
            panic!("a dead generation must not mint a second flight")
        };
        assert_eq!(resource_flight_id, original_id);
        assert_eq!(result.disposition, ResourceActionDispositionV1::Unknown);
        assert!(reopened.records(&id('2')).unwrap().is_empty());
        assert!(reopened
            .records(&original_id)
            .unwrap()
            .iter()
            .any(|record| matches!(
                &record.event,
                ResourceFlightJournalEventV1::Settled {
                    result: ResourceActionResultV1 {
                        disposition: ResourceActionDispositionV1::Unknown,
                        ..
                    }
                }
            )));
    }

    #[test]
    fn dead_flight_with_journaled_intent_settles_unknown_without_identity_reconstruction() {
        let root = tempfile::tempdir().unwrap();
        let journal = Arc::new(FileResourceFlightJournal::open(root.path(), 24).unwrap());
        let clock = Arc::new(ManualClock::new(3));
        let publisher = Arc::new(Publisher::default());
        let registry = ResourceFlightRegistryV1::new(attempt());
        let reservation = registry
            .reserve(config('1', journal.clone(), clock, publisher.clone()))
            .unwrap();
        let flight = Arc::clone(reservation.flight().unwrap());
        let primary = owner("node_a", "session-a");
        flight.attach_owner(primary.clone()).unwrap();
        flight.close_admission().unwrap();
        flight.journal_intent(intent(primary)).unwrap();
        let flight_id = flight.flight_id().clone();
        drop(flight);
        drop(reservation);
        drop(registry);
        let reopened = FileResourceFlightJournal::open(root.path(), 24).unwrap();
        let result = RetainedResourceFlight::recover_dead_journaled_intent_as_unknown(
            &reopened,
            &flight_id,
            17,
            publisher.as_ref(),
        )
        .unwrap()
        .unwrap();
        assert_eq!(result.disposition, ResourceActionDispositionV1::Unknown);
        assert!(
            RetainedResourceFlight::recover_dead_journaled_intent_as_unknown(
                &reopened,
                &flight_id,
                17,
                publisher.as_ref()
            )
            .unwrap()
            .is_none()
        );
    }

    #[test]
    fn crash_reopen_recovers_journaled_intent() {
        let root = tempfile::tempdir().unwrap();
        let journal = Arc::new(FileResourceFlightJournal::open(root.path(), 24).unwrap());
        let clock = Arc::new(ManualClock::new(3));
        let publisher = Arc::new(Publisher::default());
        let registry = ResourceFlightRegistryV1::new(attempt());
        let reservation = registry
            .reserve(config('1', journal, clock, publisher))
            .unwrap();
        let flight = Arc::clone(reservation.flight().unwrap());
        let first = owner("node_a", "session-a");
        flight.attach_owner(first.clone()).unwrap();
        flight.close_admission().unwrap();
        flight.journal_intent(intent(first)).unwrap();
        let flight_id = flight.flight_id().clone();
        drop(flight);
        drop(reservation);
        drop(registry);
        let reopened = FileResourceFlightJournal::open(root.path(), 24).unwrap();
        let records = reopened.records(&flight_id).unwrap();
        assert!(records.iter().any(|row| matches!(
            &row.event,
            ResourceFlightJournalEventV1::IntentJournaled { .. }
        )));
    }

    #[test]
    fn cleanup_deadline_transfers_exact_guard_before_terminal() {
        let (flight, clock, publisher) = journaled();
        flight.begin_journaled_dispatch().unwrap();
        let guard = flight.retain().unwrap();
        clock.set(60);
        let transfer = flight
            .transfer_cleanup_deadline(60, guard, BoundedRecoveryReasonV1::new("deadline").unwrap())
            .unwrap();
        let CleanupDeadlineTransferV1::Transferred {
            recovery_owner,
            guard,
        } = transfer
        else {
            panic!("exact guard must transfer")
        };
        assert_eq!(guard.resource_flight_id(), flight.flight_id());
        assert_eq!(&recovery_owner.resource_flight_id, flight.flight_id());
        assert!(matches!(
            flight.state().unwrap(),
            ResourceFlightStateV1::Settled {
                result: ResourceActionResultV1 {
                    disposition: ResourceActionDispositionV1::Partial,
                    ..
                }
            }
        ));
        assert_eq!(publisher.0.lock().unwrap().len(), 1);
        let rows = flight.journal.records(flight.flight_id()).unwrap();
        let guard_sequence = rows
            .iter()
            .find(|row| {
                matches!(
                    &row.event,
                    ResourceFlightJournalEventV1::GuardTransferred { .. }
                )
            })
            .unwrap()
            .sequence;
        let settled_sequence = rows
            .iter()
            .find(|row| matches!(&row.event, ResourceFlightJournalEventV1::Settled { .. }))
            .unwrap()
            .sequence;
        assert!(guard_sequence < settled_sequence);
    }

    #[test]
    fn cleanup_deadline_adopts_recovered_terminal_without_transfer() {
        let journal: Arc<dyn ResourceFlightJournal> =
            Arc::new(InMemoryResourceFlightJournal::new(24));
        let clock = Arc::new(ManualClock::new(8));
        let publisher = Arc::new(Publisher::default());
        let reservation = ResourceFlightRegistryV1::new(attempt())
            .reserve(config(
                '1',
                journal.clone(),
                clock.clone(),
                publisher.clone(),
            ))
            .unwrap();
        let flight = Arc::clone(reservation.flight().unwrap());
        let primary = owner("node_a", "session-a");
        flight.attach_owner(primary.clone()).unwrap();
        flight.close_admission().unwrap();
        flight.journal_intent(intent(primary)).unwrap();
        flight.begin_journaled_dispatch().unwrap();
        let guard = flight.retain().unwrap();
        let flight_id = flight.flight_id().clone();
        clock.set(60);

        assert!(matches!(
            RetainedResourceFlight::recover_dead_journaled_intent_as_unknown(
                journal.as_ref(),
                &flight_id,
                60,
                publisher.as_ref(),
            )
            .unwrap(),
            Some(ResourceActionResultV1 {
                disposition: ResourceActionDispositionV1::Unknown,
                ..
            })
        ));
        let CleanupDeadlineTransferV1::Unknown { result, guard } = flight
            .transfer_cleanup_deadline(60, guard, BoundedRecoveryReasonV1::new("deadline").unwrap())
            .unwrap()
        else {
            panic!("adopting a recovery terminal must not report a transfer")
        };
        assert_eq!(result.disposition, ResourceActionDispositionV1::Unknown);
        assert_eq!(guard.resource_flight_id(), flight.flight_id());
        assert_eq!(publisher.0.lock().unwrap().len(), 1);
        assert!(!journal
            .records(&flight_id)
            .unwrap()
            .iter()
            .any(|row| matches!(
                &row.event,
                ResourceFlightJournalEventV1::GuardTransferred { .. }
            )));
    }

    #[test]
    fn failed_cleanup_guard_transfer_settles_unknown_and_preserves_foreign_guard() {
        let (first, clock, publisher) = journaled();
        first.begin_journaled_dispatch().unwrap();
        clock.set(60);
        let (second, second_clock, _) = journaled();
        let foreign_guard = second.retain().unwrap();
        let CleanupDeadlineTransferV1::Unknown { result, guard } = first
            .transfer_cleanup_deadline(
                60,
                foreign_guard,
                BoundedRecoveryReasonV1::new("deadline").unwrap(),
            )
            .unwrap()
        else {
            panic!("foreign guard must survive a failed transfer")
        };
        assert_eq!(result.disposition, ResourceActionDispositionV1::Unknown);
        assert_eq!(publisher.0.lock().unwrap().len(), 1);
        second.begin_journaled_dispatch().unwrap();
        second_clock.set(60);
        assert!(matches!(
            second
                .transfer_cleanup_deadline(
                    60,
                    guard,
                    BoundedRecoveryReasonV1::new("deadline").unwrap(),
                )
                .unwrap(),
            CleanupDeadlineTransferV1::Transferred { .. }
        ));
    }

    #[test]
    fn join_blocking_requires_no_async_runtime() {
        let (flight, _, _) = journaled();
        flight.begin_journaled_dispatch().unwrap();
        let waiting = flight.clone();
        let waiter = std::thread::spawn(move || waiting.join_blocking().unwrap());
        flight.settle(complete()).unwrap();
        assert_eq!(waiter.join().unwrap(), complete());
    }

    #[test]
    fn owned_process_tree_captures_nonce_identity_and_only_journals() {
        let (flight, _, _) = create(24);
        let process = OwnedProcessTreeV1::capture(
            "shared-generation",
            [7; 32],
            42,
            Some(40),
            ProcessStartIdentityV1 {
                pid: 42,
                start_time_ticks: 1,
                executable_sha256: None,
            },
        )
        .unwrap();
        process.journal_capture(&flight).unwrap();
        let rows = flight.journal.records(flight.flight_id()).unwrap();
        assert!(matches!(
            &rows.last().unwrap().event,
            ResourceFlightJournalEventV1::ProcessTreeCaptured { .. }
        ));
    }

    #[test]
    fn journaled_intent_reserves_exact_capacity_through_terminal_settlement() {
        let (flight, _, _) = create(7);
        let primary = owner("node_a", "session-a");
        flight.attach_owner(primary.clone()).unwrap();
        flight.close_admission().unwrap();
        flight.journal_intent(intent(primary)).unwrap();
        flight.begin_journaled_dispatch().unwrap();
        flight.settle(complete()).unwrap();
    }

    #[test]
    fn collateral_discovered_while_signaling_is_delivered_once_at_settlement() {
        let (flight, _, publisher) = journaled();
        let collateral = owner("node_b", "session-b");
        flight.begin_journaled_dispatch().unwrap();
        assert!(flight
            .discover_collateral_owner(collateral.clone())
            .unwrap());
        flight.settle(complete()).unwrap();
        let publications = publisher.0.lock().unwrap();
        assert_eq!(publications.len(), 2);
        assert_eq!(
            publications
                .iter()
                .map(|aggregation| aggregation.owner.clone())
                .collect::<BTreeSet<_>>(),
            BTreeSet::from([owner("node_a", "session-a"), collateral])
        );
        assert!(publications
            .iter()
            .all(|aggregation| aggregation.affected_owner_count == 2));
    }

    #[test]
    fn collateral_discovery_after_settlement_is_refused() {
        let (flight, _, _) = journaled();
        flight.begin_journaled_dispatch().unwrap();
        flight.settle(complete()).unwrap();
        assert!(matches!(
            flight.discover_collateral_owner(owner("node_b", "session-b")),
            Err(RetainedResourceFlightError::TransitionRefused { state })
                if matches!(state.as_ref(), ResourceFlightStateV1::Settled { .. })
        ));
    }

    #[test]
    fn terminal_append_failure_refuses_bare_thread_joiner() {
        let journal: Arc<dyn ResourceFlightJournal> = Arc::new(FailingTerminalAppendJournal {
            inner: InMemoryResourceFlightJournal::new(24),
        });
        let clock: Arc<dyn MonotonicClock> = Arc::new(ManualClock::new(8));
        let publisher: Arc<dyn ResourceFlightResultPublisher> = Arc::new(Publisher::default());
        let reservation = ResourceFlightRegistryV1::new(attempt())
            .reserve(config('1', journal, clock, publisher))
            .unwrap();
        let flight = Arc::clone(reservation.flight().unwrap());
        let primary = owner("node_a", "session-a");
        flight.attach_owner(primary.clone()).unwrap();
        flight.close_admission().unwrap();
        flight.journal_intent(intent(primary)).unwrap();
        flight.begin_journaled_dispatch().unwrap();

        let (ready_tx, ready_rx) = mpsc::channel();
        let (result_tx, result_rx) = mpsc::channel();
        let waiting = Arc::clone(&flight);
        std::thread::spawn(move || {
            ready_tx.send(()).unwrap();
            result_tx.send(waiting.join_blocking()).unwrap();
        });
        ready_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        assert!(matches!(
            flight.settle(complete()),
            Err(RetainedResourceFlightError::TerminalRefusal { cause })
                if matches!(cause.as_ref(), ResourceFlightJournalError::Io { .. })
        ));
        assert!(matches!(
            result_rx.recv_timeout(Duration::from_secs(1)).unwrap(),
            Err(RetainedResourceFlightError::TerminalRefusal { cause })
                if matches!(cause.as_ref(), ResourceFlightJournalError::Io { .. })
        ));
    }

    #[test]
    fn live_flight_adopts_recovery_terminal_and_publishes_once() {
        let journal: Arc<dyn ResourceFlightJournal> =
            Arc::new(InMemoryResourceFlightJournal::new(24));
        let clock: Arc<dyn MonotonicClock> = Arc::new(ManualClock::new(12));
        let publisher = Arc::new(Publisher::default());
        let publisher_port: Arc<dyn ResourceFlightResultPublisher> = publisher.clone();
        let live = ResourceFlightRegistryV1::new(attempt())
            .reserve(config(
                '1',
                journal.clone(),
                clock.clone(),
                publisher_port.clone(),
            ))
            .unwrap();
        let flight = Arc::clone(live.flight().unwrap());
        let primary = owner("node_a", "session-a");
        flight.attach_owner(primary.clone()).unwrap();
        flight.close_admission().unwrap();
        flight.journal_intent(intent(primary)).unwrap();
        flight.begin_journaled_dispatch().unwrap();

        let recovered = ResourceFlightRegistryV1::new(attempt())
            .reserve(config('2', journal.clone(), clock, publisher_port))
            .unwrap();
        assert!(matches!(
            recovered,
            ResourceFlightReservationV1::Recovered {
                result: ResourceActionResultV1 {
                    disposition: ResourceActionDispositionV1::Unknown,
                    ..
                },
                ..
            }
        ));

        assert_eq!(
            flight.settle(complete()).unwrap().disposition,
            ResourceActionDispositionV1::Unknown,
            "the driver must return the terminal result that won the journal CAS"
        );
        assert_eq!(
            flight.join_blocking().unwrap().disposition,
            ResourceActionDispositionV1::Unknown
        );
        let rows = journal.records(flight.flight_id()).unwrap();
        assert_eq!(
            rows.iter()
                .filter(|row| matches!(&row.event, ResourceFlightJournalEventV1::Settled { .. }))
                .count(),
            1
        );
        let publications = publisher.0.lock().unwrap();
        assert_eq!(publications.len(), 1);
        assert_eq!(
            publications[0].result.disposition,
            ResourceActionDispositionV1::Unknown
        );
    }

    #[test]
    fn live_flight_converges_when_recovery_settles_before_dispatch() {
        let journal: Arc<dyn ResourceFlightJournal> =
            Arc::new(InMemoryResourceFlightJournal::new(24));
        let clock: Arc<dyn MonotonicClock> = Arc::new(ManualClock::new(12));
        let publisher = Arc::new(Publisher::default());
        let publisher_port: Arc<dyn ResourceFlightResultPublisher> = publisher.clone();
        let live = ResourceFlightRegistryV1::new(attempt())
            .reserve(config(
                '1',
                journal.clone(),
                clock.clone(),
                publisher_port.clone(),
            ))
            .unwrap();
        let flight = Arc::clone(live.flight().unwrap());
        let primary = owner("node_a", "session-a");
        flight.attach_owner(primary.clone()).unwrap();
        flight.close_admission().unwrap();
        flight.journal_intent(intent(primary)).unwrap();

        let recovered = ResourceFlightRegistryV1::new(attempt())
            .reserve(config('2', journal.clone(), clock, publisher_port))
            .unwrap();
        assert!(matches!(
            recovered,
            ResourceFlightReservationV1::Recovered { .. }
        ));
        assert!(matches!(
            flight.begin_journaled_dispatch(),
            Err(RetainedResourceFlightError::TransitionRefused { state })
                if matches!(
                    state.as_ref(),
                    ResourceFlightStateV1::Settled {
                        result: ResourceActionResultV1 {
                            disposition: ResourceActionDispositionV1::Unknown,
                            ..
                        }
                    }
                )
        ));
        assert_eq!(
            flight.join_blocking().unwrap().disposition,
            ResourceActionDispositionV1::Unknown
        );
        assert_eq!(publisher.0.lock().unwrap().len(), 1);
    }

    #[test]
    fn recovery_terminal_cas_uses_sequence_after_a_stale_snapshot() {
        let journal = Arc::new(RecoverySnapshotJournal::new(24));
        let journal_port: Arc<dyn ResourceFlightJournal> = journal.clone();
        let clock: Arc<dyn MonotonicClock> = Arc::new(ManualClock::new(12));
        let publisher = Arc::new(Publisher::default());
        let publisher_port: Arc<dyn ResourceFlightResultPublisher> = publisher.clone();
        let reservation = ResourceFlightRegistryV1::new(attempt())
            .reserve(config('1', journal_port, clock, publisher_port))
            .unwrap();
        let flight = Arc::clone(reservation.flight().unwrap());
        let primary = owner("node_a", "session-a");
        let collateral = owner("node_b", "session-b");
        flight.attach_owner(primary.clone()).unwrap();
        flight.close_admission().unwrap();
        flight.journal_intent(intent(primary.clone())).unwrap();
        flight.begin_journaled_dispatch().unwrap();
        let flight_id = flight.flight_id().clone();

        journal.pause_next_records.store(true, Ordering::SeqCst);
        let recovery_journal = journal.clone();
        let recovery_publisher = publisher.clone();
        let recovery_id = flight_id.clone();
        let recovery = std::thread::spawn(move || {
            RetainedResourceFlight::recover_dead_journaled_intent_as_unknown(
                recovery_journal.as_ref(),
                &recovery_id,
                12,
                recovery_publisher.as_ref(),
            )
        });
        journal.snapshot_taken.wait();
        assert!(flight
            .discover_collateral_owner(collateral.clone())
            .unwrap());
        journal.allow_terminal_append.wait();

        assert!(matches!(
            recovery.join().unwrap().unwrap(),
            Some(ResourceActionResultV1 {
                disposition: ResourceActionDispositionV1::Unknown,
                ..
            })
        ));
        flight.settle(complete()).unwrap();
        assert_eq!(
            flight.join_blocking().unwrap().disposition,
            ResourceActionDispositionV1::Unknown
        );
        let rows = journal.records(&flight_id).unwrap();
        assert_eq!(
            rows.last().unwrap().sequence,
            u64::try_from(rows.len()).unwrap()
        );
        assert_eq!(
            rows.iter()
                .filter(|row| matches!(&row.event, ResourceFlightJournalEventV1::Settled { .. }))
                .count(),
            1
        );
        let publications = publisher.0.lock().unwrap();
        assert_eq!(publications.len(), 2);
        assert_eq!(
            publications
                .iter()
                .map(|publication| publication.owner.clone())
                .collect::<BTreeSet<_>>(),
            BTreeSet::from([primary, collateral])
        );
    }

    #[test]
    fn concurrent_recovery_has_one_terminal_cas_winner() {
        let journal: Arc<dyn ResourceFlightJournal> =
            Arc::new(InMemoryResourceFlightJournal::new(24));
        let clock: Arc<dyn MonotonicClock> = Arc::new(ManualClock::new(12));
        let publisher = Arc::new(Publisher::default());
        let publisher_port: Arc<dyn ResourceFlightResultPublisher> = publisher.clone();
        let registry = ResourceFlightRegistryV1::new(attempt());
        let reservation = registry
            .reserve(config('1', journal.clone(), clock, publisher_port))
            .unwrap();
        let flight = Arc::clone(reservation.flight().unwrap());
        let primary = owner("node_a", "session-a");
        flight.attach_owner(primary.clone()).unwrap();
        flight.close_admission().unwrap();
        flight.journal_intent(intent(primary)).unwrap();
        let flight_id = flight.flight_id().clone();
        drop(flight);
        drop(reservation);
        drop(registry);

        let barrier = Arc::new(Barrier::new(3));
        let mut joiners = Vec::new();
        for _ in 0..2 {
            let journal = journal.clone();
            let publisher = publisher.clone();
            let flight_id = flight_id.clone();
            let barrier = barrier.clone();
            joiners.push(std::thread::spawn(move || {
                barrier.wait();
                RetainedResourceFlight::recover_dead_journaled_intent_as_unknown(
                    journal.as_ref(),
                    &flight_id,
                    12,
                    publisher.as_ref(),
                )
                .unwrap()
            }));
        }
        barrier.wait();
        assert_eq!(
            joiners
                .into_iter()
                .map(|joiner| joiner.join().unwrap())
                .filter(Option::is_some)
                .count(),
            1
        );
        assert_eq!(publisher.0.lock().unwrap().len(), 1);
    }

    #[test]
    fn zero_length_reservation_is_replaced_by_a_complete_durable_record() {
        let root = tempfile::tempdir().unwrap();
        let journal = FileResourceFlightJournal::open(root.path(), 24).unwrap();
        let key = ResourceFlightKeyV1::AcpGeneration {
            generation: "shared-generation".to_string(),
        };
        let reservation = ResourceFlightReservationRecordV1 {
            schema_version: RESOURCE_FLIGHT_JOURNAL_SCHEMA_V1,
            key: key.clone(),
            attempt_id: attempt(),
            resource_flight_id: id('1'),
        };
        let path = journal.reservation_path(&key).unwrap();
        std::fs::File::create(&path).unwrap();
        assert!(matches!(
            journal.reserve_flight(&reservation).unwrap(),
            ResourceFlightReservationOutcomeV1::Created
        ));
        assert_eq!(journal.read_reservation(&path).unwrap(), reservation);
    }

    #[test]
    fn torn_journal_tail_is_truncated_and_recovery_settles_unknown() {
        let root = tempfile::tempdir().unwrap();
        let journal = Arc::new(FileResourceFlightJournal::open(root.path(), 24).unwrap());
        let clock: Arc<dyn MonotonicClock> = Arc::new(ManualClock::new(12));
        let publisher = Arc::new(Publisher::default());
        let publisher_port: Arc<dyn ResourceFlightResultPublisher> = publisher.clone();
        let reservation = ResourceFlightRegistryV1::new(attempt())
            .reserve(config('1', journal.clone(), clock, publisher_port))
            .unwrap();
        let flight = Arc::clone(reservation.flight().unwrap());
        let primary = owner("node_a", "session-a");
        flight.attach_owner(primary.clone()).unwrap();
        flight.close_admission().unwrap();
        flight.journal_intent(intent(primary)).unwrap();
        let flight_id = flight.flight_id().clone();
        let path = journal.path(&flight_id);
        let mut tail = std::fs::OpenOptions::new().append(true).open(path).unwrap();
        tail.write_all(b"{\"torn\"").unwrap();
        tail.sync_all().unwrap();

        let result = RetainedResourceFlight::recover_dead_journaled_intent_as_unknown(
            journal.as_ref(),
            &flight_id,
            12,
            publisher.as_ref(),
        )
        .unwrap()
        .unwrap();
        assert_eq!(result.disposition, ResourceActionDispositionV1::Unknown);
        let rows = journal.records(&flight_id).unwrap();
        assert!(rows.iter().any(|row| matches!(
            &row.event,
            ResourceFlightJournalEventV1::IntentJournaled { .. }
        )));
        assert!(matches!(
            &rows.last().unwrap().event,
            ResourceFlightJournalEventV1::Settled {
                result: ResourceActionResultV1 {
                    disposition: ResourceActionDispositionV1::Unknown,
                    ..
                }
            }
        ));
    }

    #[test]
    fn registry_rejects_a_fast_path_join_from_another_attempt() {
        let registry = ResourceFlightRegistryV1::new(attempt());
        let journal: Arc<dyn ResourceFlightJournal> =
            Arc::new(InMemoryResourceFlightJournal::new(24));
        let clock: Arc<dyn MonotonicClock> = Arc::new(ManualClock::new(0));
        let publisher: Arc<dyn ResourceFlightResultPublisher> = Arc::new(Publisher::default());
        let mut mismatched = config('1', journal, clock, publisher);
        mismatched.attempt_id = AttemptId::parse(format!("attempt-{}", "2".repeat(32))).unwrap();
        assert!(matches!(
            registry.reserve(mismatched),
            Err(RetainedResourceFlightError::ReservationAttemptMismatch)
        ));
    }

    #[test]
    fn recovery_publication_runs_after_releasing_registry_mutex() {
        let root = tempfile::tempdir().unwrap();
        let journal: Arc<dyn ResourceFlightJournal> =
            Arc::new(FileResourceFlightJournal::open(root.path(), 24).unwrap());
        let clock: Arc<dyn MonotonicClock> = Arc::new(ManualClock::new(12));
        let recovery_registry = Arc::new(ResourceFlightRegistryV1::new(attempt()));
        let publisher = Arc::new(ReentrantPublisher {
            registry: recovery_registry.clone(),
            reentrant_config: Mutex::new(None),
            calls: AtomicU64::new(0),
        });
        let publisher_port: Arc<dyn ResourceFlightResultPublisher> = publisher.clone();
        let first_registry = ResourceFlightRegistryV1::new(attempt());
        let first = first_registry
            .reserve(config(
                '1',
                journal.clone(),
                clock.clone(),
                publisher_port.clone(),
            ))
            .unwrap();
        let flight = Arc::clone(first.flight().unwrap());
        let primary = owner("node_a", "session-a");
        flight.attach_owner(primary.clone()).unwrap();
        flight.close_admission().unwrap();
        flight.journal_intent(intent(primary)).unwrap();
        drop(flight);
        drop(first);
        drop(first_registry);

        *publisher.reentrant_config.lock().unwrap() = Some(config(
            '3',
            journal.clone(),
            clock.clone(),
            publisher_port.clone(),
        ));
        let recovery_config = config('2', journal, clock, publisher_port);
        let (result_tx, result_rx) = mpsc::channel();
        let registry = recovery_registry.clone();
        std::thread::spawn(move || {
            result_tx
                .send(registry.reserve(recovery_config).is_ok())
                .unwrap();
        });
        assert!(result_rx.recv_timeout(Duration::from_secs(1)).unwrap());
        assert_eq!(publisher.calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn process_lifecycle_events_have_exact_wire_goldens() {
        let detached = ResourceFlightJournalEventV1::OwnerDetached {
            owner: owner("node_a", "session-a"),
        };
        assert_eq!(
            serde_json::to_string(&detached).unwrap(),
            r#"{"event":"owner_detached","owner":{"node_id":"node_a","owner_key":"session-a"}}"#
        );

        let binding = ResourceFlightJournalEventV1::ProcessBindingFailed {
            stage: ProcessBindingStageV1::ImmutableStart,
            pid: Some(42),
        };
        assert_eq!(
            serde_json::to_string(&binding).unwrap(),
            r#"{"event":"process_binding_failed","stage":"immutable_start","pid":42}"#
        );

        let signals = ResourceFlightJournalEventV1::ProcessSignalsObserved {
            observations: vec![ProcessSignalObservationV1 {
                pid: 42,
                expected_start_time_ticks: 7,
                signal: 15,
                return_code: 0,
                errno: None,
            }],
        };
        assert_eq!(
            serde_json::to_string(&signals).unwrap(),
            r#"{"event":"process_signals_observed","observations":[{"pid":42,"expected_start_time_ticks":7,"signal":15,"return_code":0,"errno":null}]}"#
        );

        let reservation = ResourceFlightJournalEventV1::ProcessLifecycleReserved {
            reserved_lifecycle_slots: PROCESS_LIFECYCLE_SLOTS,
        };
        assert_eq!(
            serde_json::to_string(&reservation).unwrap(),
            r#"{"event":"process_lifecycle_reserved","reserved_lifecycle_slots":7}"#
        );
    }

    #[test]
    fn process_lifecycle_event_reader_rejects_unknown_event_fields_and_stages() {
        for invalid in [
            r#"{"event":"future_process_event"}"#,
            r#"{"event":"owner_detached","owner":{"node_id":"node_a","owner_key":"session-a"},"future":true}"#,
            r#"{"event":"owner_detached","owner":{"node_id":"node_a","owner_key":"session-a","future":true}}"#,
            r#"{"event":"process_binding_failed","stage":"future_stage","pid":42}"#,
            r#"{"event":"process_signals_observed","observations":[{"pid":42,"expected_start_time_ticks":7,"signal":15,"return_code":0,"errno":null,"future":true}]}"#,
        ] {
            assert!(
                serde_json::from_str::<ResourceFlightJournalEventV1>(invalid).is_err(),
                "must reject unsupported durable wire shape: {invalid}"
            );
        }
    }

    #[test]
    fn process_lifecycle_reservation_protects_terminal_capacity() {
        let (flight, _, _) = create(10);
        let primary = owner("node_a", "session-a");
        flight.attach_owner(primary).unwrap();
        flight.reserve_process_lifecycle().unwrap();
        let spawn_owner = owner("process-spawn", "spawn-owner");
        flight.attach_owner_legacy_v2(spawn_owner.clone()).unwrap();
        assert!(flight.detach_owner(&spawn_owner).unwrap());
        assert!(matches!(
            flight.attach_owner(owner("node_b", "session-b")),
            Err(RetainedResourceFlightError::Journal(
                ResourceFlightJournalError::Full
            ))
        ));
        flight
            .journal_process_tree(ResourceIdentityV1::AcpProcess {
                generation: "shared-generation".into(),
                spawn_nonce_sha256: sha('b'),
                pid: 42,
                pgid: Some(42),
                immutable_start: ProcessStartIdentityV1 {
                    pid: 42,
                    start_time_ticks: 7,
                    executable_sha256: None,
                },
            })
            .unwrap();
    }
}
