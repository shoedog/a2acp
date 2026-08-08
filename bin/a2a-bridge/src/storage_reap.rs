//! `a2a-bridge storage reap --build-targets` — the FIRST destructive authority in the storage system
//! (R2f1b pre-slice-2 custody plan §3 S3, §5, §6).
//!
//! It consumes the S2 classifier ([`crate::storage_report`]) and may delete ONLY items classified
//! [`PayloadClass::BuildTarget`] or per-run [`PayloadClass::DependencyCache`]. It never deletes a
//! `SourceCheckout` (S4's authority), never `Evidence`, never `Unclassified`, never a container volume
//! (ADR-0021/0025 keep that authority), and never anything at, inside, or containing a D-2 protected
//! root.
//!
//! **An S2 report is an observation, not a warrant.** Every gate is rechecked AT THE DESTRUCTIVE
//! BOUNDARY, never at scan time, because the S2 evidence measured a just-released flock still reading
//! `Held` for subsequent probes — a lock reading lags reality in both directions.
//!
//! **The invariant is IDLE, not "completed".** A run is reapable when nothing is using it, and no
//! single gate establishes that. The licensing evidence is the CONJUNCTION of all of these:
//!
//! 1. **run-owner liveness** — the pid embedded in `impl-<pid>-<nonce>` must not be running. This is
//!    the gate that actually excludes a live run, and it exists because the operation lock does NOT:
//!    `implement_cmd` takes only an ADR-0025 run LEASE (`acquire_lease`, under
//!    `~/.a2a-bridge/leases`), while `.operation-locks/<id>.lock` is taken by `implement_resume` and
//!    `merge` alone. A reaper gated solely on that lock is INERT against an initial `implement` run:
//!    during its host-side phases there is no container in `ps` and no descriptor under `target/`;
//! 2. **the operation lock**, ACQUIRED and HELD across probe→delete — which excludes a concurrent
//!    `resume`/`merge`, and nothing else. It is never proof of idleness on its own;
//! 3. **descriptor pinning** — the scan root is held open
//!    ([`bridge_core::fs_custody::PinnedDirectoryV1`]) and its dev/ino re-verified before AND after
//!    every removal, closing the `verify_root` same-parent swap window the S2 review deferred here;
//! 4. **class re-derivation from on-disk evidence, for EVERY reapable class** — cargo markers for
//!    `BuildTarget`, and [`dependency_cache_provenance`] for `DependencyCache`, whose S2
//!    classification is BY NAME ALONE and therefore not proof of regenerability. Path identity is
//!    re-stat'ed with `symlink_metadata` at the same moment;
//! 5. **a host process / open-file / cwd probe** over the run directory, which must answer FREE;
//! 6. **the container axis** — the host `lsof` runs on the host kernel and cannot see descriptors
//!    inside a container VM, so when a runtime is configured an affirmative container answer is
//!    REQUIRED; with no runtime configured the axis is disclosed as uncovered, never read as absent.
//!
//! A failed or nondiscriminating probe PARKS the payload with a typed reason — it never defaults to
//! free. Evidence brackets the operation on both sides: a fsync'd INTENT record lands before the
//! first removal, and the outcome receipt lands before the operation lock is released.
//!
//! Truthfulness over optimism: a deletion that begins and does not complete is [`ItemOutcome::Partial`],
//! one whose result cannot be established is [`ItemOutcome::Unknown`]. Neither is ever reported as
//! success. Freed space is MEASURED (`statvfs` before/after per item) beside the logical and on-disk
//! sizes, and is labelled as measured rather than as authoritative accounting.
//!
//! Layout follows `storage_report`: the pure / FS-only cores + their unit tests live here, and the
//! host shell-outs (`lsof`, the real flock) live behind [`ReapEnv`] so the orchestration is behaviorally
//! testable — the carried demand from the S2 fold review.

use crate::storage_report as sr;
use std::path::{Path, PathBuf};

// ---------------------------------------------------------------------------------------------
// D-4 admission floor
// ---------------------------------------------------------------------------------------------

pub const GIB: u64 = 1024 * 1024 * 1024;

/// D-4: ONE floor, 50 GiB, config-overridable via `[storage].admission_floor_gib`. No watermark
/// ladder, no reservations, no quotas.
pub const DEFAULT_ADMISSION_FLOOR_GIB: u64 = 50;

/// Why a new run was refused admission. Typed and actionable: it names the floor, the observed value,
/// and the remedy — a bare "not enough disk" would leave the operator guessing at both.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AdmissionRefusal {
    BelowFloor {
        path: String,
        floor_bytes: u64,
        free_bytes: u64,
    },
    /// Free space could not be read at all. Fail-closed: an unmeasurable volume is not an empty one.
    Unmeasurable { path: String },
}

impl std::fmt::Display for AdmissionRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BelowFloor {
                path,
                floor_bytes,
                free_bytes,
            } => write!(
                f,
                "storage admission floor: refusing to start a new run — {} free on {path}, below the \
                 {} floor (D-4). Remedy: `a2a-bridge storage report` to see what is holding the space, \
                 then `a2a-bridge storage reap --build-targets --dry-run` and, once the plan reads \
                 right, without `--dry-run`. Override with `[storage] admission_floor_gib` (0 disables \
                 the check).",
                sr::human_bytes(Some(*free_bytes)),
                sr::human_bytes(Some(*floor_bytes)),
            ),
            Self::Unmeasurable { path } => write!(
                f,
                "storage admission floor: refusing to start a new run — free space on {path} could not \
                 be read, and an unreadable volume is not evidence of a free one (D-4). Remedy: check \
                 the path is mounted and readable, or set `[storage] admission_floor_gib = 0` to \
                 disable the check deliberately.",
            ),
        }
    }
}

impl std::error::Error for AdmissionRefusal {}

/// PURE. The D-4 gate over an already-taken measurement, so the decision is testable without a disk.
///
/// `floor_gib == 0` disables the check entirely — the deliberate operator opt-out. Any other floor
/// refuses on an unmeasurable volume rather than assuming space exists.
pub fn check_admission_floor(
    path: &Path,
    floor_gib: u64,
    free_bytes: Option<u64>,
) -> Result<(), AdmissionRefusal> {
    if floor_gib == 0 {
        return Ok(());
    }
    let floor_bytes = floor_gib.saturating_mul(GIB);
    match free_bytes {
        None => Err(AdmissionRefusal::Unmeasurable {
            path: sr::display_path(path),
        }),
        Some(free) if free < floor_bytes => Err(AdmissionRefusal::BelowFloor {
            path: sr::display_path(path),
            floor_bytes,
            free_bytes: free,
        }),
        Some(_) => Ok(()),
    }
}

/// The host-side D-4 check: measure, then judge. Used at the `implement` admission point.
pub fn admit_new_run(path: &Path, floor_gib: u64) -> Result<(), AdmissionRefusal> {
    let (free, _total) = sr::filesystem_space(path);
    check_admission_floor(path, floor_gib, free)
}

// ---------------------------------------------------------------------------------------------
// Typed park reasons
// ---------------------------------------------------------------------------------------------

/// Why a payload was NOT deleted. Every refusal names its mechanism: "never guess" means a reaper that
/// cannot discriminate must say which discrimination failed.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(tag = "reason", rename_all = "snake_case")]
pub enum ParkReason {
    /// D-2: at, inside, or containing a protected root. Checked by canonical identity BEFORE class.
    ProtectedRoot { root: String },
    /// Not a class this reaper has any authority over.
    NotReapableClass { class: String },
    /// A container volume: ADR-0021/0025 keeps that authority, and the row's `path` is a volume NAME
    /// rather than a filesystem path, so it is not addressable by this reaper at all.
    ContainerVolume,
    /// A payload with no owning run has no operation lock to hold, so there is no boundary to gate on.
    NoOwningRun,
    /// Outside the descriptor-pinned scan root (or not under its own run directory).
    NotUnderScanRoot { root: String },
    /// The enclosing `<root>/<run id>` is not a standalone clone, so the payload's provenance is not
    /// established. Regenerability is a property of a cargo workspace, not of a directory name.
    EnclosingRunNotAClone { detail: String },
    /// The path is a symlink at the boundary. Never followed, never deleted.
    PathIsSymlink,
    /// The path stopped being a readable directory between the scan and the boundary.
    PathNotADirectory { detail: String },
    /// The path no longer resolves to itself (an intermediate component became a link, or the leaf
    /// was swapped for a different inode) between the scan and the boundary.
    PathIdentityChanged { detail: String },
    /// The on-disk cargo markers no longer support the class the scan recorded.
    ClassifierDisagreement { expected: String, observed: String },
    /// Another operation (resume, merge) owns this run right now.
    OperationLockHeld { run_id: String },
    /// The operation lock could not be taken for a reason other than contention.
    OperationLockUnavailable { detail: String },
    /// The process that created this run is still alive. The operation lock does NOT exclude it: an
    /// initial `implement` run holds only its ADR-0025 run lease, and takes the operation lock never.
    RunOwnerAlive { pid: u32 },
    /// The run directory's name does not name its owning process, so the run cannot be shown idle.
    RunIdNotParseable { detail: String },
    /// Whether the owning process is alive could not be established.
    RunOwnerLivenessUnknown { detail: String },
    /// A container runtime is configured but did not affirmatively answer for this payload. The host
    /// `lsof` probe cannot see inside a container VM, so an unanswered container axis is a blind spot,
    /// not an absence.
    ContainerAxisUnanswered,
    /// A `DependencyCache` matched only by NAME, with no on-disk proof it is regenerable.
    NoCacheProvenance { detail: String },
    /// The crash-durable intent record could not be established before the first removal.
    IntentRecordUnavailable { detail: String },
    /// A live process, open file, cwd, or container consumer was found at the boundary.
    LiveConsumer { kind: String, detail: String },
    /// The consumer probe did not produce a discriminating answer. NEVER read as free.
    ConsumerProbeFailed { detail: String },
    /// The pinned scan root's dev/ino changed — the tree was swapped under us.
    ScanRootIdentityChanged { detail: String },

    // -- S4 (clone reaper) ---------------------------------------------------------------------
    /// A `[worktrees]` payload. Its custody handle is the ADR-0025 sidecar lease, not the per-run
    /// operation lock, and a linked worktree is removed with `git worktree remove`, never `rm -rf`.
    /// Read from the typed [`sr::ItemSource`], not inferred.
    WorktreeCustody,
    /// The on-disk `.git` shape does not prove standalone-clone semantics at the boundary. A linked
    /// worktree shares its source's object store, and an ambiguous shape proves nothing at all.
    NotAStandaloneClone { detail: String },
    /// `git status` reported state that is not on any commit: modified, staged, untracked, a
    /// non-disposable ignored entry, or a dirty submodule. Uncommitted bytes are BY DEFINITION not on
    /// main, so no containment verdict can license deleting them.
    GitStateNotClean { detail: String },
    /// The git-state probe itself did not answer. A `status` that could not run is not a clean tree.
    GitStateUnknown { detail: String },
    /// HEAD has no commit. There is nothing to ask the containment question about.
    UnbornHead,
    /// `origin` is not a local path (or is absent), so there is no local source repository whose live
    /// refs the containment query can be asked of. No network is ever contacted to fill the gap.
    OriginNotLocal { detail: String },
    /// The D-1 gate answered something other than `yes(head)`/`yes(tree)`. `no` and `unknown` BOTH park:
    /// one means the content is demonstrably not on main, the other means the probe could not tell.
    NotOnSourceMain { verdict: String, detail: String },
    /// A ref other than HEAD holds commits that are on neither HEAD nor source main. Containment proves
    /// HEAD; the deletion takes the WHOLE object store, so every ref must be accounted for.
    RefsNotContained { detail: String },
    /// The index carries a flag (`assume-unchanged`, `skip-worktree`, sparse) that makes
    /// `git status --porcelain` silent about tracked bytes it can no longer see.
    IndexFlagsHideState { detail: String },
    /// An initialized submodule: its object store lives in `.git/modules` and dies with the clone, and
    /// the superproject's gitlink SHA proves nothing about where those bytes are.
    InitializedSubmodule { detail: String },
    /// The checkpoint's recorded source repository disagrees with `remote.origin.url`, so the
    /// containment proof was asked of a repository this run was not cloned from.
    OriginDisagreesWithCheckpoint { detail: String },
    /// Source main moved while the containment probes ran: the verdict describes no single history.
    SourceMainMoved { detail: String },
    /// `.git` changed identity around the git probes, so their answers describe a different repository.
    GitIdentityChanged { detail: String },
    /// The run's Evidence could not be preserved outside the clone. Evidence has its own retention
    /// (plan §5) and must never die with the parent it describes.
    EvidencePreservationFailed { detail: String },
    /// The fold receipt could not be established before the deletion.
    FoldReceiptUnavailable { detail: String },
    /// The exact-mechanism removal guard refused: the path is not `<root>/<run id>`, has no `.git`, or
    /// is (or contains) the source repository itself.
    RemovalGuardRefused { detail: String },
}

impl ParkReason {
    pub fn summary(&self) -> String {
        match self {
            Self::ProtectedRoot { root } => format!("protected root (D-2): {root}"),
            Self::NotReapableClass { class } => {
                format!("class {class} is not a class this command may remove")
            }
            Self::ContainerVolume => {
                "container volume (its path is a volume NAME, not a filesystem path) — ADR-0021/0025 \
                 authority, not a storage reaper's"
                    .to_string()
            }
            Self::NoOwningRun => "no owning run, so no operation lock to hold".to_string(),
            Self::NotUnderScanRoot { root } => {
                format!("not under the pinned scan root {root}")
            }
            Self::EnclosingRunNotAClone { detail } => {
                format!("enclosing run is not a standalone clone: {detail}")
            }
            Self::PathIsSymlink => "path is a symlink at the boundary".to_string(),
            Self::PathNotADirectory { detail } => format!("path is not a directory: {detail}"),
            Self::PathIdentityChanged { detail } => format!("path identity changed: {detail}"),
            Self::ClassifierDisagreement { expected, observed } => format!(
                "classifier disagreed at the boundary: scan said {expected}, on-disk evidence says \
                 {observed}"
            ),
            Self::OperationLockHeld { run_id } => {
                format!("operation lock for run {run_id} is held by another operation")
            }
            Self::OperationLockUnavailable { detail } => {
                format!("operation lock unavailable: {detail}")
            }
            Self::RunOwnerAlive { pid } => format!(
                "the process that created this run (pid {pid}) is STILL ALIVE — an initial \
                 `implement` run holds only its run lease, so the operation lock does not exclude it"
            ),
            Self::RunIdNotParseable { detail } => format!("run owner unidentifiable: {detail}"),
            Self::RunOwnerLivenessUnknown { detail } => {
                format!("run owner liveness unknown: {detail}")
            }
            Self::ContainerAxisUnanswered => "a container runtime is configured but did not answer \
                 for this payload; the host `lsof` probe cannot see inside a container VM, so the \
                 container axis is unchecked"
                .to_string(),
            Self::NoCacheProvenance { detail } => {
                format!("dependency cache has no regenerability provenance: {detail}")
            }
            Self::IntentRecordUnavailable { detail } => {
                format!("crash-durable intent record could not be written: {detail}")
            }
            Self::LiveConsumer { kind, detail } => format!("live {kind} consumer: {detail}"),
            Self::ConsumerProbeFailed { detail } => {
                format!("consumer probe did not answer: {detail}")
            }
            Self::ScanRootIdentityChanged { detail } => {
                format!("scan root identity changed: {detail}")
            }
            Self::WorktreeCustody => "a `[worktrees]` payload: its custody handle is the ADR-0025 \
                 sidecar lease (not the per-run operation lock) and it is removed with `git worktree \
                 remove`, never `rm -rf`"
                .to_string(),
            Self::NotAStandaloneClone { detail } => {
                format!("not provably a standalone clone: {detail}")
            }
            Self::GitStateNotClean { detail } => format!(
                "working tree carries state that is on no commit, so it cannot be on main: {detail}"
            ),
            Self::GitStateUnknown { detail } => {
                format!("git state could not be established: {detail}")
            }
            Self::UnbornHead => {
                "HEAD is unborn — there is no commit to ask the containment question about".to_string()
            }
            Self::OriginNotLocal { detail } => format!(
                "no local source repository to ask about containment: {detail} (no network is ever \
                 contacted to fill the gap)"
            ),
            Self::NotOnSourceMain { verdict, detail } => format!(
                "content is not verifiably on source main (verdict {verdict}): {detail}"
            ),
            Self::RefsNotContained { detail } => format!(
                "a ref other than HEAD holds commits that are on neither HEAD nor source main — \
                 containment proves HEAD, but the deletion takes the whole object store: {detail}"
            ),
            Self::IndexFlagsHideState { detail } => format!(
                "the index carries flags that make `git status` blind to tracked bytes: {detail}"
            ),
            Self::InitializedSubmodule { detail } => format!(
                "an initialized submodule's object store lives in `.git/modules` and would die with \
                 the clone: {detail}"
            ),
            Self::OriginDisagreesWithCheckpoint { detail } => format!(
                "`remote.origin.url` disagrees with the checkpoint's recorded source repository, so \
                 the containment proof was asked of the wrong repository: {detail}"
            ),
            Self::SourceMainMoved { detail } => format!(
                "source main moved while the containment probes ran: {detail}"
            ),
            Self::GitIdentityChanged { detail } => {
                format!("`.git` changed around the probes: {detail}")
            }
            Self::EvidencePreservationFailed { detail } => format!(
                "run evidence could not be preserved outside the clone: {detail}"
            ),
            Self::FoldReceiptUnavailable { detail } => {
                format!("fold receipt could not be written before the deletion: {detail}")
            }
            Self::RemovalGuardRefused { detail } => {
                format!("exact-mechanism removal guard refused: {detail}")
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Outcomes
// ---------------------------------------------------------------------------------------------

/// What actually happened to one payload.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum ItemOutcome {
    /// `--dry-run`: every gate passed and NOTHING was touched.
    Planned,
    Deleted,
    /// The removal began and the payload is still present. Not success, and not a clean refusal.
    Partial {
        detail: String,
    },
    /// The removal's result could not be established (e.g. the pinned root changed under it).
    Unknown {
        detail: String,
    },
    Parked {
        reason: ParkReason,
    },
}

impl ItemOutcome {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Planned => "planned",
            Self::Deleted => "deleted",
            Self::Partial { .. } => "PARTIAL",
            Self::Unknown { .. } => "UNKNOWN",
            Self::Parked { .. } => "parked",
        }
    }
    pub fn detail(&self) -> Option<String> {
        match self {
            Self::Planned | Self::Deleted => None,
            Self::Partial { detail } | Self::Unknown { detail } => Some(detail.clone()),
            Self::Parked { reason } => Some(reason.summary()),
        }
    }
}

/// One payload's record: what it was, what was done, and the gate evidence that licensed it.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct ReapItem {
    pub path: String,
    /// What `path` names, carried through from the scan's own declaration. A receipt that dropped it
    /// would push the volume-vs-path inference onto whoever reads the record — the very inference the
    /// typed field exists to remove.
    pub source: sr::ItemSource,
    pub class: String,
    pub run_id: Option<String>,
    pub logical_bytes: Option<u64>,
    pub disk_bytes: Option<u64>,
    /// `statvfs` free-space delta measured across THIS item's removal. Signed and best-effort: a
    /// concurrent writer moves it, so it is recorded as MEASURED, never as authoritative accounting.
    /// `None` when either measurement failed or nothing was removed.
    pub freed_bytes_measured: Option<i64>,
    /// Flattened so a receipt reads `"outcome": "deleted"` at the item's own level rather than
    /// nesting a one-key object — the receipt is meant to be greppable.
    #[serde(flatten)]
    pub outcome: ItemOutcome,
    /// The gates that passed, in order, before the outcome above. This is the receipt's evidence.
    pub gates: Vec<String>,
}

/// The JSON receipt written beside a run's checkpoint evidence (plan §7: Evidence class, never
/// auto-deleted with its subject).
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct ReapReceipt {
    pub schema: String,
    pub run_id: String,
    pub scan_root: String,
    pub scan_root_identity: String,
    pub dry_run: bool,
    pub at_epoch_secs: u64,
    pub items: Vec<ReapItem>,
}

pub const RECEIPT_SCHEMA: &str = "a2a-bridge.storage-reap.v1";
pub const INTENT_SCHEMA: &str = "a2a-bridge.storage-reap-intent.v1";

/// Disclosed side effect of `--dry-run`, in the style of `storage_report`'s `LOCK_PROBE_DISCLOSURE`.
/// A dry run deletes nothing and writes no evidence, but it evaluates the gates for real — and the
/// operation-lock gate is only meaningful if the lock is actually taken, which CREATES
/// `<root>/.operation-locks/<run id>.lock` when a run has never been resumed or merged. Claiming
/// "nothing was touched" would be false.
pub const DRY_RUN_SIDE_EFFECT: &str =
    "a dry run deletes nothing and writes no evidence. Its ONE state-visible effect: taking each \
     run's operation lock creates `<root>/.operation-locks/<run id>.lock` if it does not exist \
     (persistent lock paths are never unlinked). Gates are otherwise evaluated read-only.";

/// The crash-durable record written BEFORE the first removal of a run: what was about to be deleted
/// and the gate evidence that licensed it. A receipt alone describes an end state that a crash may
/// have prevented from ever existing; this describes the intent, so the two bracket the operation.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct ReapIntent {
    pub schema: String,
    pub run_id: String,
    pub scan_root: String,
    pub scan_root_identity: String,
    pub at_epoch_secs: u64,
    /// The payloads about to be removed, with the gates each one passed.
    pub candidates: Vec<ReapItem>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize)]
pub struct ReapReport {
    pub scan_root: String,
    pub dry_run: bool,
    pub items: Vec<ReapItem>,
    /// Evidence paths written (empty on a dry run — a dry run performs no deletion and writes no
    /// evidence; see [`DRY_RUN_SIDE_EFFECT`] for its one state-visible effect).
    pub receipts: Vec<String>,
    /// Crash-durable intent records written before the removals they describe.
    pub intents: Vec<String>,
    /// Receipts that could NOT be written. Non-empty must fail the command's exit status: a reap
    /// whose record was lost is not a successful reap, and degrading that to a printed note would let
    /// an automated caller read success.
    pub receipt_failures: Vec<String>,
    pub free_bytes_before: Option<u64>,
    pub free_bytes_after: Option<u64>,
    pub notes: Vec<String>,
}

impl ReapReport {
    pub fn count(&self, label: &str) -> usize {
        self.items
            .iter()
            .filter(|i| i.outcome.label() == label)
            .count()
    }
    /// Sum of `disk_bytes` over items actually removed. Separate from the measured `statvfs` delta:
    /// one is what the payload occupied, the other is what the volume reported back.
    pub fn removed_disk_bytes(&self) -> u64 {
        self.items
            .iter()
            .filter(|i| matches!(i.outcome, ItemOutcome::Deleted))
            .filter_map(|i| i.disk_bytes)
            .fold(0u64, u64::saturating_add)
    }
    /// Sum of `disk_bytes` over items a dry run PLANNED to remove — what the operator is being asked to
    /// authorize. Never mixed into `removed_disk_bytes`: one is a proposal, the other is a fact.
    pub fn planned_disk_bytes(&self) -> u64 {
        self.items
            .iter()
            .filter(|i| matches!(i.outcome, ItemOutcome::Planned))
            .filter_map(|i| i.disk_bytes)
            .fold(0u64, u64::saturating_add)
    }
}

// ---------------------------------------------------------------------------------------------
// D-2 protected roots
// ---------------------------------------------------------------------------------------------

/// D-2 protected roots, resolved to canonical identity. Matched by PATH COMPONENTS, never by string
/// prefix (`/a/bc` must not match `/a/b`) and never by age or size.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ProtectedRoots {
    roots: Vec<PathBuf>,
}

impl ProtectedRoots {
    /// Resolve configured roots. An entry that is ABSENT protects nothing and is noted; an entry that
    /// exists but cannot be resolved is a hard error — an unreadable protected root is exactly the case
    /// where guessing is unacceptable.
    pub fn resolve(raw: &[String], notes: &mut Vec<String>) -> Result<Self, String> {
        let mut roots = Vec::new();
        for r in raw {
            let p = Path::new(r);
            match std::fs::symlink_metadata(p) {
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    notes.push(format!(
                        "protected root {r:?} does not exist — it protects nothing and was skipped"
                    ));
                }
                Err(e) => {
                    return Err(format!(
                        "protected root {r:?} is unreadable ({e}) — refusing to reap while a D-2 root \
                         cannot be identified"
                    ));
                }
                Ok(_) => match std::fs::canonicalize(p) {
                    Ok(c) => roots.push(c),
                    Err(e) => {
                        return Err(format!(
                            "protected root {r:?} has no canonical path ({e}) — refusing to reap while \
                             a D-2 root cannot be identified"
                        ));
                    }
                },
            }
        }
        roots.sort();
        roots.dedup();
        Ok(Self { roots })
    }

    pub fn is_empty(&self) -> bool {
        self.roots.is_empty()
    }

    pub fn paths(&self) -> &[PathBuf] {
        &self.roots
    }

    /// Refuse `candidate` when it IS a protected root, is INSIDE one, or CONTAINS one (deleting a
    /// parent deletes the protected child just as surely).
    pub fn refuse(&self, candidate: &Path) -> Option<ParkReason> {
        self.roots
            .iter()
            .find(|r| candidate.starts_with(r) || r.starts_with(candidate))
            .map(|r| ParkReason::ProtectedRoot {
                root: r.to_string_lossy().into_owned(),
            })
    }
}

// ---------------------------------------------------------------------------------------------
// Consumer probe (process / open file / cwd) — the seam and its pure parser
// ---------------------------------------------------------------------------------------------

/// The answer to "does anything still hold this payload?".
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConsumerProbe {
    /// The probe answered COMPLETELY and found nothing.
    Free,
    Held {
        detail: String,
    },
    /// The probe could not answer. This parks the payload; it is never read as free.
    Failed {
        detail: String,
    },
}

/// How the `lsof` process itself ended. Separate from its OUTPUT, because the two answer different
/// questions: the status says whether the probe is ADMISSIBLE at all, the artifact says what it found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LsofStatus {
    /// The process could not be started.
    NotSpawned,
    Exited(i32),
    /// Killed by a signal — the walk was cut short, so its silence means nothing.
    Signaled(i32),
}

/// PURE. Interpret one `lsof -w -t +D <path>` result.
///
/// Two-stage, and the distinction matters. **Admissibility comes from the status**: `lsof` documents
/// exit 0 (found something) and exit 1 (found nothing); a signal or any other code means the walk was
/// cut short or failed, so its silence is not evidence of an empty directory. **Content comes from the
/// artifact**: within an admissible run, only a clean empty stdout with an empty stderr may read
/// `Free`, because exit 1 alone cannot tell "no consumers" from "could not look". Output that is not a
/// pid list means we do not know what `lsof` was telling us — the same rule
/// `storage_report::ps_outcome` applies to a container runtime's `ps`.
pub fn lsof_outcome(status: LsofStatus, stdout: &str, stderr: &str) -> ConsumerProbe {
    match status {
        LsofStatus::NotSpawned => {
            return ConsumerProbe::Failed {
                detail:
                    "`lsof` could not be run (absent or not executable); with no open-file probe the \
                     payload cannot be shown free"
                        .into(),
            };
        }
        LsofStatus::Signaled(sig) => {
            return ConsumerProbe::Failed {
                detail: format!(
                    "`lsof` was killed by signal {sig} — the directory walk was cut short, so its \
                     empty result is not evidence of an idle payload"
                ),
            };
        }
        // The only two statuses `lsof` documents. Anything else is a failure mode we cannot interpret.
        LsofStatus::Exited(0) | LsofStatus::Exited(1) => {}
        LsofStatus::Exited(code) => {
            return ConsumerProbe::Failed {
                detail: format!(
                    "`lsof` exited {code}, which is neither its documented 0 (found) nor 1 (none \
                     found) — its answer is not interpretable"
                ),
            };
        }
    }
    let lines: Vec<&str> = stdout
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    if !lines.is_empty() {
        if lines.iter().all(|l| l.parse::<u32>().is_ok()) {
            return ConsumerProbe::Held {
                detail: format!("open file / cwd held by pid(s) {}", lines.join(", ")),
            };
        }
        return ConsumerProbe::Failed {
            detail: format!(
                "`lsof -t` returned {} line(s) that are not pids — its answer is INCOMPLETE and the \
                 payload stays parked",
                lines.len()
            ),
        };
    }
    let err = stderr.trim();
    if !err.is_empty() {
        return ConsumerProbe::Failed {
            detail: format!(
                "`lsof` reported an error: {}",
                err.lines().next().unwrap_or(err)
            ),
        };
    }
    ConsumerProbe::Free
}

// ---------------------------------------------------------------------------------------------
// Run-owner liveness
// ---------------------------------------------------------------------------------------------

/// Is the process that created a run still running?
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PidLiveness {
    Alive,
    Dead,
    /// Could not be established. Parks, like every other failed probe.
    Unknown(String),
}

/// PURE. The pid embedded in a quarantine clone's directory name.
///
/// `implement::task_id` builds run ids as `impl-<pid>-<nonce>`, and that pid is the ONLY on-disk link
/// between a run directory and the process that owns it: the ADR-0025 run lease is keyed on
/// `<pid>-<nonce>` with a DIFFERENT nonce, so the lease path cannot be derived from the run id.
///
/// Fail-closed by construction: anything that is not exactly `impl-<u32>-<nonce>` is an error, never a
/// "probably fine". A run directory whose owner cannot be named cannot be shown idle.
pub fn run_owner_pid(run_id: &str) -> Result<u32, String> {
    let parts: Vec<&str> = run_id.split('-').collect();
    if parts.len() != 3 || parts[0] != "impl" || parts[2].is_empty() {
        return Err(format!(
            "run id {run_id:?} is not the `impl-<pid>-<nonce>` shape, so its owning process cannot be \
             identified"
        ));
    }
    match parts[1].parse::<u32>() {
        Ok(pid) if pid > 0 => Ok(pid),
        _ => Err(format!(
            "run id {run_id:?} carries no usable pid in its second segment"
        )),
    }
}

// ---------------------------------------------------------------------------------------------
// Dependency-cache provenance
// ---------------------------------------------------------------------------------------------

/// Cheap on-disk proof that a `DependencyCache` really is a regenerable package cache.
///
/// The S2 classifier matches `node_modules` and `.venv` BY NAME ALONE — unlike `BuildTarget`, which
/// `is_cargo_target` backs with real cargo markers. A name is not provenance: a directory called
/// `node_modules` holding a user's own files is not regenerable, and deleting it is unrecoverable.
///
/// - `node_modules` needs a `package.json` beside it (npm's own layout: the manifest that regenerates
///   the tree sits in the directory that contains `node_modules`);
/// - `.venv` needs `pyvenv.cfg` inside it — the marker `python -m venv` writes and the one every
///   venv-aware tool keys on.
pub fn dependency_cache_provenance(path: &Path) -> Result<String, String> {
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| "cache directory has no readable name".to_string())?;
    match name {
        "node_modules" => {
            let manifest = path
                .parent()
                .ok_or_else(|| "`node_modules` has no parent directory".to_string())?
                .join("package.json");
            if sr::real_file(&manifest) {
                Ok(format!(
                    "`package.json` beside it at {}",
                    manifest.display()
                ))
            } else {
                Err(format!(
                    "no `package.json` beside it (looked for {}) — a directory named `node_modules` \
                     without a manifest is not provably regenerable",
                    manifest.display()
                ))
            }
        }
        ".venv" => {
            let cfg = path.join("pyvenv.cfg");
            if sr::real_file(&cfg) {
                Ok(format!("`pyvenv.cfg` inside it at {}", cfg.display()))
            } else {
                Err(format!(
                    "no `pyvenv.cfg` inside it (looked for {}) — a directory named `.venv` without \
                     the venv marker is not provably regenerable",
                    cfg.display()
                ))
            }
        }
        other => Err(format!(
            "`{other}` is not a dependency-cache shape this reaper can prove regenerable"
        )),
    }
}

// ---------------------------------------------------------------------------------------------
// The injectable environment seam
// ---------------------------------------------------------------------------------------------

/// Why the operation lock could not be taken. Typed rather than string-sniffed: contention ("someone
/// else owns this run right now") and a broken namespace ("we cannot tell who owns it") are different
/// facts, and only the first is routine.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LockFailure {
    Contended,
    Unavailable(String),
}

/// Every runtime / filesystem effect the reaper performs, behind one seam so the orchestration is
/// behaviorally testable (the S2 fold review's carried demand).
pub trait ReapEnv {
    /// The held operation lock. Dropping it releases; the orchestrator holds it across probe→delete.
    type Lock;

    /// Acquire `<implement_root>/.operation-locks/<run id>.lock`, the same mutex `implement_resume`
    /// and `merge` take.
    fn acquire_operation_lock(
        &self,
        implement_root: &Path,
        run_id: &str,
    ) -> Result<Self::Lock, LockFailure>;

    /// Is the process that owns a run directory still running? `kill(pid, 0)` semantics: `EPERM`
    /// counts as ALIVE (a process we may not signal is still a process).
    fn process_alive(&self, pid: u32) -> PidLiveness;

    /// Process / open-file / cwd consumers of `path`. Called once per RUN DIRECTORY, not per payload:
    /// `+D` is a full recursive walk, and the run directory already contains every payload.
    fn probe_consumers(&self, path: &Path) -> ConsumerProbe;

    /// Free bytes on the filesystem holding `path`.
    fn free_bytes(&self, path: &Path) -> Option<u64>;

    /// Recursively remove `path`.
    fn remove_tree(&self, path: &Path) -> Result<(), String>;

    fn now_epoch_secs(&self) -> u64;

    /// Write the crash-durable INTENT record into `evidence_dir` and FSYNC it, returning the path.
    /// This lands before the first removal so a crash mid-reap leaves a record of what was about to
    /// go — a receipt written only afterwards describes a state that may never have been reached.
    fn write_intent(&self, evidence_dir: &Path, json: &str) -> Result<String, String>;

    /// Write a receipt into `evidence_dir`, returning the path written.
    fn write_receipt(&self, evidence_dir: &Path, json: &str) -> Result<String, String>;

    /// Write (and FSYNC) `file_name` into `dir`, replacing any existing file of that name, returning
    /// the path written. Separate from [`ReapEnv::write_receipt`] because S4's fold receipt has a
    /// STABLE per-run name in a namespace that outlives the clone, and is written TWICE: once as the
    /// crash-durable intent before the removal, once with the outcome before the lock is released.
    /// A timestamped name would leave those two as unrelated files.
    fn write_named(&self, dir: &Path, file_name: &str, json: &str) -> Result<String, String>;

    /// Copy Evidence out of a payload that is about to be deleted, returning the relative names
    /// copied. Evidence has its own retention (plan §5) and must never die with its parent.
    ///
    /// `sources` may mix directories (copied recursively) and regular files, because a run's evidence
    /// is not one directory: the sidecar sits at `.git/a2a-bridge`, while `.git/A2A_TASK.md` and
    /// `.git/A2A_COMMIT_MSG` sit beside it.
    ///
    /// Symlinks and other non-regular entries are NEVER followed and NEVER skipped — they are an
    /// error, so the caller parks. Skipping them would delete a clone while reporting its evidence
    /// preserved; following one would copy whatever a `:rw` container aimed it at. Every write is
    /// durably synced before returning: an unsynced copy of a record whose original is about to be
    /// unlinked is not a preserved record.
    fn copy_evidence(&self, sources: &[PathBuf], to: &Path) -> Result<Vec<String>, String>;

    /// Fsync a directory so its entries are durable. Called on the receipt namespace and its parent
    /// BEFORE the first removal: a receipt whose directory entry has not reached the disk does not
    /// survive the crash it exists to describe. A failure here parks the payload.
    fn sync_dir(&self, dir: &Path) -> Result<(), String>;

    /// Operator-visible progress. A `+D` walk over a 20 GiB target takes long enough to read as a
    /// hang, so the command narrates itself. Default: silent (tests observe the journal instead).
    fn progress(&self, message: &str) {
        let _ = message;
    }
}

// ---------------------------------------------------------------------------------------------
// The orchestrator
// ---------------------------------------------------------------------------------------------

pub struct ReapRequest<'a> {
    /// The already-`verify_root`-checked `.a2a-implement` root.
    pub scan_root: &'a Path,
    /// The S2 classifier's items — OBSERVATIONS, re-derived at the boundary before anything is removed.
    pub items: &'a [sr::ReportItem],
    pub protected: &'a ProtectedRoots,
    pub dry_run: bool,
    /// How many container runtimes this config declares. Non-zero means an affirmative container
    /// answer is REQUIRED before any payload may be removed; zero means the axis is uncovered and
    /// that fact is disclosed on the receipt rather than silently treated as "no containers".
    pub runtimes_configured: usize,
}

/// A payload that survived the lock-free admission gates and is queued for its boundary gates.
struct Candidate<'a> {
    item: &'a sr::ReportItem,
    path: PathBuf,
    /// Carried explicitly rather than re-derived from `item.run_id` later: admission already proved
    /// it is a non-empty single component, and re-unwrapping it downstream would reintroduce a
    /// default-on-`None` path that could silently group payloads under an empty run id.
    run_id: String,
    run_dir: PathBuf,
    gates: Vec<String>,
}

/// Reap build targets (and per-run dependency caches) under one implement root.
///
/// Two phases, deliberately: the ADMISSION gates (protected roots, class authority, containment)
/// take no lock and change no state, so an inadmissible payload never causes the reaper to touch the
/// run's lock namespace at all. Only surviving candidates reach the BOUNDARY gates, which run under
/// the run's held operation lock.
pub fn reap_build_targets<E: ReapEnv>(req: ReapRequest<'_>, env: &E) -> ReapReport {
    let mut report = ReapReport {
        scan_root: sr::display_path(req.scan_root),
        dry_run: req.dry_run,
        ..Default::default()
    };
    report.free_bytes_before = env.free_bytes(req.scan_root);

    // The scan root itself is checked against D-2 BEFORE anything is enumerated or locked.
    if let Some(reason) = req.protected.refuse(req.scan_root) {
        report.notes.push(format!(
            "REFUSED: the whole scan root is {} — nothing was examined",
            reason.summary()
        ));
        for it in req.items {
            report.items.push(park(it, reason.clone()));
        }
        return report;
    }

    // Descriptor-pin the scan root. `verify_root` is two syscalls on a PATH and cannot see a leaf
    // swapped by a hostile parent afterwards; from here on the root has a held descriptor whose
    // dev/ino is re-verified before AND after every removal.
    let pin = match bridge_core::fs_custody::PinnedDirectoryV1::open(req.scan_root, "storage reap")
    {
        Ok(p) => p,
        Err(e) => {
            let reason = ParkReason::ScanRootIdentityChanged {
                detail: format!("scan root could not be descriptor-pinned: {e}"),
            };
            report.notes.push(format!("REFUSED: {}", reason.summary()));
            for it in req.items {
                report.items.push(park(it, reason.clone()));
            }
            return report;
        }
    };
    let root = pin.canonical_path().to_path_buf();

    let mut by_run: std::collections::BTreeMap<String, Vec<Candidate<'_>>> = Default::default();
    for it in req.items {
        match admit(it, &root, req.protected) {
            Err(reason) => report.items.push(park(it, reason)),
            Ok(c) => by_run.entry(c.run_id.clone()).or_default().push(c),
        }
    }

    for (run_id, candidates) in by_run {
        // GATE 1 — run-owner liveness, BEFORE the lock. This is the gate that actually excludes a
        // live run: `implement_cmd` takes only its ADR-0025 run lease and never the operation lock,
        // so the lock alone excludes `resume`/`merge` and nothing else. Checked pre-lock deliberately
        // — it needs no lock to be meaningful, and it avoids writing into a LIVE run's lock namespace
        // just to discover we must leave it alone.
        let owner_gate = match run_owner_pid(&run_id) {
            Err(detail) => Some(ParkReason::RunIdNotParseable { detail }),
            Ok(pid) => match env.process_alive(pid) {
                PidLiveness::Alive => Some(ParkReason::RunOwnerAlive { pid }),
                PidLiveness::Unknown(detail) => {
                    Some(ParkReason::RunOwnerLivenessUnknown { detail })
                }
                PidLiveness::Dead => None,
            },
        };
        if let Some(reason) = owner_gate {
            env.progress(&format!("run {run_id}: {}", reason.summary()));
            for c in candidates {
                report.items.push(park(c.item, reason.clone()));
            }
            continue;
        }
        let owner_gate_evidence = format!(
            "run owner: pid {} is not running (the run crashed or completed); note that the \
             operation lock below excludes only `resume`/`merge`, never an initial `implement`",
            run_owner_pid(&run_id).unwrap_or(0)
        );

        // GATE 2 — the operation lock, held across probe→delete. It excludes a concurrent resume or
        // merge; combined with gate 1 it makes the run idle on both axes the host can see.
        let guard = match env.acquire_operation_lock(&root, &run_id) {
            Err(LockFailure::Contended) => {
                for c in candidates {
                    report.items.push(park(
                        c.item,
                        ParkReason::OperationLockHeld {
                            run_id: run_id.clone(),
                        },
                    ));
                }
                continue;
            }
            Err(LockFailure::Unavailable(detail)) => {
                for c in candidates {
                    report.items.push(park(
                        c.item,
                        ParkReason::OperationLockUnavailable {
                            detail: detail.clone(),
                        },
                    ));
                }
                continue;
            }
            Ok(g) => g,
        };

        // GATE 3 — ONE consumer probe for the whole run directory, under the held lock. `lsof +D` is
        // a full recursive walk and the run directory already contains every payload, so probing per
        // payload would walk the same bytes twice and widen the probe→delete window for no evidence.
        let run_dir = root.join(&run_id);
        env.progress(&format!(
            "run {run_id}: probing {} for open files / cwds (recursive; can take a while on a large \
             target)",
            run_dir.display()
        ));
        let probe = env.probe_consumers(&run_dir);

        let mut gated: Vec<Gated<'_>> = Vec::new();
        for mut c in candidates {
            c.gates.push(owner_gate_evidence.clone());
            c.gates.push(format!(
                "operation lock: HELD for run {run_id} across probe and delete (excludes \
                 resume/merge)"
            ));
            match gate_one(c, &pin, &probe, req.runtimes_configured) {
                Ok(g) => gated.push(g),
                Err(parked) => report.items.push(parked),
            }
        }

        if gated.is_empty() {
            drop(guard);
            continue;
        }

        if req.dry_run {
            for g in gated {
                report.items.push(g.into_planned());
            }
            drop(guard);
            continue;
        }

        // GATE 4 — the crash-durable INTENT record, fsync'd BEFORE the first removal. A receipt
        // written only afterwards describes an end state a crash may have prevented from existing;
        // this brackets the operation from the other side. Failure to establish it parks the run.
        let intent_items: Vec<ReapItem> = gated.iter().map(Gated::as_planned).collect();
        match write_run_intent(&run_id, &intent_items, &pin, env) {
            Ok(path) => report.intents.push(path),
            Err(detail) => {
                env.progress(&format!(
                    "run {run_id}: intent record unavailable ({detail})"
                ));
                for g in gated {
                    report
                        .items
                        .push(g.parked(ParkReason::IntentRecordUnavailable {
                            detail: detail.clone(),
                        }));
                }
                drop(guard);
                continue;
            }
        }

        let mut recorded = Vec::new();
        for g in gated {
            env.progress(&format!(
                "removing {} ({})",
                g.path.display(),
                sr::human_bytes(g.disk)
            ));
            let outcome = remove_one(g, &pin, env);
            recorded.push(outcome.clone());
            report.items.push(outcome);
        }

        // The receipt lands BEFORE the lock is released, so no racing resume or merge can interleave
        // between the removals and the record of them.
        write_run_receipt(&run_id, &recorded, &pin, env, &mut report);
        drop(guard);
    }

    report.free_bytes_after = env.free_bytes(&root);
    report.items.sort_by(|a, b| a.path.cmp(&b.path));
    report
}

/// The lock-free admission gates, in refusal order. Returns the park reason for anything this
/// command has no authority over — before any lock is taken and before any state is touched.
fn admit<'a>(
    it: &'a sr::ReportItem,
    root: &Path,
    protected: &ProtectedRoots,
) -> Result<Candidate<'a>, ParkReason> {
    // FIRST, and BY THE SCANNER'S OWN DECLARATION rather than by class or by string shape. A container
    // volume's `path` is a volume NAME, not a filesystem path — and `classify_volume` maps
    // `a2a-*-cache-<hash>` / `a2a-*-target-<hash>` into `DependencyCache` / `BuildTarget`, the very
    // classes this reaper deletes, so discriminating on class alone would miss those rows entirely.
    //
    // `source` is the primary discrimination (the S3 review's carried finding: destructive code must
    // not INFER volume-vs-path). The `is_absolute` check stays as defence in depth — belt and braces,
    // never the load-bearing gate.
    let path = PathBuf::from(&it.path);
    if !it.source.is_filesystem_path()
        || it.class == sr::PayloadClass::ContainerOrImage
        || !path.is_absolute()
    {
        return Err(ParkReason::ContainerVolume);
    }
    // A `[worktrees]` payload has no operation lock and no `impl-<pid>-<nonce>` owner: neither of this
    // command's idleness gates applies to one, so it is refused by source rather than gated on.
    if it.source == sr::ItemSource::WorktreePath {
        return Err(ParkReason::WorktreeCustody);
    }

    // D-2 BEFORE classification: a protected root is refused whatever class the scan gave it.
    if let Some(reason) = protected.refuse(&path) {
        return Err(reason);
    }
    if !matches!(
        it.class,
        sr::PayloadClass::BuildTarget | sr::PayloadClass::DependencyCache
    ) {
        return Err(ParkReason::NotReapableClass {
            class: it.class.label().to_string(),
        });
    }
    // No run ⇒ no operation lock ⇒ no boundary to gate the deletion on.
    let Some(run_id) = it.run_id.clone().filter(|r| !r.is_empty()) else {
        return Err(ParkReason::NoOwningRun);
    };
    let run_dir = root.join(&run_id);
    // Component-wise containment (never a string prefix), and strictly INSIDE its own run directory:
    // the run directory itself is a `SourceCheckout`, which is S4's authority, not S3's.
    //
    // No `is_absolute` check here: the volume gate above already refused every relative path, and
    // `root` is the pinned canonical root, so `starts_with(root)` cannot succeed for one.
    if !path.starts_with(root) || path == root || !path.starts_with(&run_dir) || path == run_dir {
        return Err(ParkReason::NotUnderScanRoot {
            root: root.to_string_lossy().into_owned(),
        });
    }
    if let Some(reason) = protected.refuse(&run_dir) {
        return Err(reason);
    }
    Ok(Candidate {
        item: it,
        path,
        run_id,
        run_dir,
        gates: vec![
            format!(
                "source: the scan declared this row `{}` — a filesystem path this command may address \
                 (a volume name is not, and is refused by this field, not by its shape)",
                it.source.label()
            ),
            format!(
                "D-2 protected roots: {} checked, none contains or is contained by this payload",
                protected.paths().len()
            ),
        ],
    })
}

/// The parked record for an item that never reached a boundary gate. Shared with the clone reaper:
/// both commands must report a refusal with the size the operator saw in `storage report`.
pub(crate) fn park(it: &sr::ReportItem, reason: ParkReason) -> ReapItem {
    ReapItem {
        path: it.path.clone(),
        source: it.source,
        class: it.class.label().to_string(),
        run_id: it.run_id.clone(),
        logical_bytes: it.measured.logical_bytes,
        disk_bytes: it.measured.disk_bytes,
        freed_bytes_measured: None,
        outcome: ItemOutcome::Parked { reason },
        gates: Vec::new(),
    }
}

/// `(dev, ino)` of a real directory at `path`. A symlink or non-directory is an error, never a
/// silently-followed success.
#[cfg(unix)]
pub(crate) fn dir_dev_ino(path: &Path) -> Result<(u64, u64), String> {
    use std::os::unix::fs::MetadataExt as _;
    let md = std::fs::symlink_metadata(path).map_err(|e| format!("{}: {e}", path.display()))?;
    if md.file_type().is_symlink() {
        return Err(format!("{} is a symlink", path.display()));
    }
    if !md.is_dir() {
        return Err(format!("{} is not a directory", path.display()));
    }
    Ok((md.dev(), md.ino()))
}

#[cfg(not(unix))]
pub(crate) fn dir_dev_ino(path: &Path) -> Result<(u64, u64), String> {
    Err(format!(
        "{}: filesystem identity (dev/ino) is unavailable on this platform, so a directory swap \
         cannot be detected",
        path.display()
    ))
}

/// Re-verify that the pinned scan root's PATH still resolves to the descriptor we pinned. This is the
/// swap check: an attacker controlling the parent can rename the root away and put another directory
/// in its place, and every path-based operation would then land in the replacement.
pub(crate) fn pinned_root_unchanged(
    pin: &bridge_core::fs_custody::PinnedDirectoryV1,
) -> Result<(), String> {
    let (dev, ino) = dir_dev_ino(pin.canonical_path())?;
    let want = pin.identity();
    if want.dev != Some(dev) || want.ino != Some(ino) {
        return Err(format!(
            "pinned scan root {} now resolves to a different directory (dev/ino {dev}/{ino}, pinned \
             {:?}/{:?})",
            pin.canonical_path().display(),
            want.dev,
            want.ino
        ));
    }
    Ok(())
}

pub(crate) fn root_identity_label(pin: &bridge_core::fs_custody::PinnedDirectoryV1) -> String {
    let id = pin.identity();
    match (id.dev, id.ino) {
        (Some(d), Some(i)) => format!("dev {d} / ino {i}"),
        _ => "unavailable".to_string(),
    }
}

/// A payload that passed every boundary gate and is queued for removal, carrying the identity the
/// removal will re-verify and the gate evidence the intent record and receipt will both cite.
struct Gated<'a> {
    item: &'a sr::ReportItem,
    path: PathBuf,
    identity: (u64, u64),
    logical: Option<u64>,
    disk: Option<u64>,
    gates: Vec<String>,
}

impl Gated<'_> {
    fn base(&self, outcome: ItemOutcome) -> ReapItem {
        ReapItem {
            path: self.item.path.clone(),
            source: self.item.source,
            class: self.item.class.label().to_string(),
            run_id: self.item.run_id.clone(),
            logical_bytes: self.logical,
            disk_bytes: self.disk,
            freed_bytes_measured: None,
            outcome,
            gates: self.gates.clone(),
        }
    }
    /// The `--dry-run` record, and the shape the intent record lists.
    fn as_planned(&self) -> ReapItem {
        self.base(ItemOutcome::Planned)
    }
    fn into_planned(self) -> ReapItem {
        self.base(ItemOutcome::Planned)
    }
    fn parked(self, reason: ParkReason) -> ReapItem {
        self.base(ItemOutcome::Parked { reason })
    }
}

/// The boundary gates, run under the held operation lock and the run-owner gate. Every one re-derives
/// its fact from the filesystem right now; nothing here trusts the scan. Returns the queued payload,
/// or the parked record explaining which gate refused it.
///
/// `probe` is the ONE consumer answer for the whole run directory (see gate 3 in the caller).
// `ReapItem` is the parked record and is genuinely large (paths + gate evidence); boxing the error
// keeps the success path cheap without discarding any of the refusal's detail.
#[allow(clippy::result_large_err)]
fn gate_one<'a>(
    c: Candidate<'a>,
    pin: &bridge_core::fs_custody::PinnedDirectoryV1,
    probe: &ConsumerProbe,
    runtimes_configured: usize,
) -> Result<Gated<'a>, ReapItem> {
    let Candidate {
        item,
        path,
        run_id: _,
        run_dir,
        mut gates,
    } = c;
    macro_rules! bail {
        ($reason:expr) => {{
            return Err(ReapItem {
                path: item.path.clone(),
                source: item.source,
                class: item.class.label().to_string(),
                run_id: item.run_id.clone(),
                // Seeded from the SCAN's measurement so a payload parked at the boundary still
                // reports the size the operator saw in `storage report`.
                logical_bytes: item.measured.logical_bytes,
                disk_bytes: item.measured.disk_bytes,
                freed_bytes_measured: None,
                outcome: ItemOutcome::Parked { reason: $reason },
                gates,
            });
        }};
    }

    // 1. The pinned root must still be the root we pinned.
    if let Err(detail) = pinned_root_unchanged(pin) {
        bail!(ParkReason::ScanRootIdentityChanged { detail });
    }
    gates.push(format!(
        "scan root: descriptor-pinned and re-verified ({})",
        root_identity_label(pin)
    ));

    // 2. Path identity: a real directory, not a symlink, still resolving to itself.
    let identity = match dir_dev_ino(&path) {
        Ok(id) => id,
        Err(detail) => {
            if std::fs::symlink_metadata(&path)
                .map(|m| m.file_type().is_symlink())
                .unwrap_or(false)
            {
                bail!(ParkReason::PathIsSymlink);
            }
            bail!(ParkReason::PathNotADirectory { detail });
        }
    };
    match std::fs::canonicalize(&path) {
        Ok(c) if c == path => {}
        Ok(c) => bail!(ParkReason::PathIdentityChanged {
            detail: format!("{} now resolves to {}", path.display(), c.display()),
        }),
        Err(e) => bail!(ParkReason::PathIdentityChanged {
            detail: format!("{} has no canonical path: {e}", path.display()),
        }),
    }
    gates.push(format!(
        "path identity: real directory, no symlink, dev {} / ino {}",
        identity.0, identity.1
    ));

    // 3. The enclosing run must be a standalone clone. Regenerability is a property of a cargo
    //    workspace inside a known checkout, never of a directory named `target`.
    let (enclosing, _kind, note) =
        sr::classify_checkout(&sr::git_shape(&run_dir), sr::CheckoutKind::StandaloneClone);
    if enclosing != sr::PayloadClass::SourceCheckout {
        bail!(ParkReason::EnclosingRunNotAClone {
            detail: note.unwrap_or_else(|| "unrecognized checkout".into()),
        });
    }
    gates.push(format!(
        "enclosing run: standalone clone at {}",
        run_dir.display()
    ));

    // 4. Re-derive the class from ON-DISK EVIDENCE, for EVERY reapable class. `BuildTarget` gets the
    //    cargo markers via `is_cargo_target`; `DependencyCache` gets provenance of its own, because
    //    the S2 classifier matches `node_modules`/`.venv` by NAME ALONE and a name is not proof of
    //    regenerability.
    let observed = sr::classify_nested(&path);
    if observed != Some(item.class) {
        bail!(ParkReason::ClassifierDisagreement {
            expected: item.class.label().to_string(),
            observed: observed
                .map(|c| c.label().to_string())
                .unwrap_or_else(|| "not a separately-classifiable payload".into()),
        });
    }
    let provenance = if item.class == sr::PayloadClass::DependencyCache {
        match dependency_cache_provenance(&path) {
            Ok(p) => format!("dependency-cache provenance: {p}"),
            Err(detail) => bail!(ParkReason::NoCacheProvenance { detail }),
        }
    } else {
        "cargo markers present (`is_cargo_target`)".to_string()
    };
    gates.push(format!(
        "classifier: re-derived {} from on-disk evidence at the boundary - {provenance}",
        item.class.label()
    ));

    // 5. Consumers the scan established. A `Held` on any axis is a refusal. `Unknown` is NOT itself a
    //    refusal here - it means "not probed" - but see gate 6, where unprobed IS the blind spot.
    for (kind, state) in [
        ("run lease", item.consumers.run_lease),
        ("container mount", item.consumers.container_mount),
    ] {
        if state == sr::HolderState::Held {
            bail!(ParkReason::LiveConsumer {
                kind: kind.to_string(),
                detail: "reported held by the scan; a reaper never overrides a positive holder"
                    .into(),
            });
        }
    }

    // 6. The container axis. The host `lsof` runs on the HOST kernel and cannot see descriptors
    //    inside a container VM, so an `lsof`-free reading says nothing about a container holding this
    //    payload. When a runtime is configured, an affirmative answer is REQUIRED.
    let container_gate = if runtimes_configured == 0 {
        "container axis: not covered - no container runtime is configured, so no container answer \
         was sought (disclosed rather than read as `no containers`)"
            .to_string()
    } else if item.consumers.container_mount == sr::HolderState::Free {
        format!(
            "container axis: {runtimes_configured} runtime(s) answered, none mounts this payload"
        )
    } else {
        bail!(ParkReason::ContainerAxisUnanswered);
    };
    gates.push(container_gate);

    // 7. The host probe result for this run directory, taken under the held lock.
    match probe {
        ConsumerProbe::Held { detail } => bail!(ParkReason::LiveConsumer {
            kind: "process/open-file/cwd".to_string(),
            detail: detail.clone(),
        }),
        ConsumerProbe::Failed { detail } => bail!(ParkReason::ConsumerProbeFailed {
            detail: detail.clone()
        }),
        ConsumerProbe::Free => {}
    }
    gates.push(
        "consumer probe: process / open file / cwd over the run directory answered FREE under the \
         held lock"
            .to_string(),
    );

    // 8. Measure only now, so the recorded size is the size of the thing about to go.
    let measured = sr::measure_tree(&path, &[]);
    gates.push(format!(
        "measured: {} logical / {} on disk",
        sr::human_bytes(measured.logical_bytes),
        sr::human_bytes(measured.disk_bytes)
    ));

    Ok(Gated {
        item,
        path,
        identity,
        logical: measured.logical_bytes,
        disk: measured.disk_bytes,
        gates,
    })
}

/// The removal itself, with the LAST identity checks immediately before the unlink and one more
/// after. The gates above took time, and time is the whole hazard.
fn remove_one<E: ReapEnv>(
    g: Gated<'_>,
    pin: &bridge_core::fs_custody::PinnedDirectoryV1,
    env: &E,
) -> ReapItem {
    let path = g.path.clone();
    let identity = g.identity;

    if let Err(detail) = pinned_root_unchanged(pin) {
        return g.parked(ParkReason::ScanRootIdentityChanged { detail });
    }
    match dir_dev_ino(&path) {
        Ok(now) if now == identity => {}
        Ok(now) => {
            return g.parked(ParkReason::PathIdentityChanged {
                detail: format!(
                "{} changed identity between the gates and the removal (dev/ino {}/{} to {}/{})",
                path.display(),
                identity.0,
                identity.1,
                now.0,
                now.1
            ),
            })
        }
        Err(detail) => return g.parked(ParkReason::PathNotADirectory { detail }),
    }

    let mut out = g.base(ItemOutcome::Planned);
    out.gates.push(
        "boundary recheck: root and payload identity unchanged immediately before removal"
            .to_string(),
    );

    let before = env.free_bytes(pin.canonical_path());
    let removal = env.remove_tree(&path);
    let after = env.free_bytes(pin.canonical_path());
    out.freed_bytes_measured = match (before, after) {
        (Some(b), Some(a)) => Some(a as i64 - b as i64),
        _ => None,
    };
    let gone = std::fs::symlink_metadata(&path).is_err();
    out.outcome = match (removal, gone) {
        (Ok(()), true) => ItemOutcome::Deleted,
        // Reported success, payload still there: not success.
        (Ok(()), false) => ItemOutcome::Partial {
            detail: format!(
                "the removal reported success but {} is still present",
                path.display()
            ),
        },
        // Reported failure, payload still there: a genuine partial removal.
        (Err(e), false) => ItemOutcome::Partial { detail: e },
        // Reported failure, payload gone: we cannot attest WHAT was removed or how much of it.
        (Err(e), true) => ItemOutcome::Unknown {
            detail: format!("removal reported an error ({e}) but the path is gone"),
        },
    };
    // A root swapped DURING the removal means we cannot attest what the removal landed on.
    if let Err(detail) = pinned_root_unchanged(pin) {
        out.outcome = ItemOutcome::Unknown {
            detail: format!("the pinned scan root changed during the removal: {detail}"),
        };
    } else {
        out.gates
            .push("scan root identity re-verified AFTER the removal".to_string());
    }
    out
}

/// Write the crash-durable intent record for one run, before any of its removals.
fn write_run_intent<E: ReapEnv>(
    run_id: &str,
    candidates: &[ReapItem],
    pin: &bridge_core::fs_custody::PinnedDirectoryV1,
    env: &E,
) -> Result<String, String> {
    pinned_root_unchanged(pin)?;
    let intent = ReapIntent {
        schema: INTENT_SCHEMA.to_string(),
        run_id: run_id.to_string(),
        scan_root: pin.canonical_path().to_string_lossy().into_owned(),
        scan_root_identity: root_identity_label(pin),
        at_epoch_secs: env.now_epoch_secs(),
        candidates: candidates.to_vec(),
    };
    let json = serde_json::to_string_pretty(&intent).map_err(|e| e.to_string())?;
    let run_dir = pin.canonical_path().join(run_id);
    env.write_intent(&sr::evidence_dir(&run_dir), &json)
}

fn write_run_receipt<E: ReapEnv>(
    run_id: &str,
    items: &[ReapItem],
    pin: &bridge_core::fs_custody::PinnedDirectoryV1,
    env: &E,
    report: &mut ReapReport,
) {
    // Never write into a root we can no longer attest: the receipt would land in the replacement.
    if let Err(detail) = pinned_root_unchanged(pin) {
        let msg = format!(
            "receipt for run {run_id} NOT written: the pinned scan root changed ({detail})"
        );
        report.notes.push(msg.clone());
        report.receipt_failures.push(msg);
        return;
    }
    let run_dir = pin.canonical_path().join(run_id);
    let receipt = ReapReceipt {
        schema: RECEIPT_SCHEMA.to_string(),
        run_id: run_id.to_string(),
        scan_root: pin.canonical_path().to_string_lossy().into_owned(),
        scan_root_identity: root_identity_label(pin),
        dry_run: false,
        at_epoch_secs: env.now_epoch_secs(),
        items: items.to_vec(),
    };
    let json = match serde_json::to_string_pretty(&receipt) {
        Ok(j) => j,
        Err(e) => {
            let msg = format!("receipt for run {run_id} NOT written: could not be encoded ({e})");
            report.notes.push(msg.clone());
            report.receipt_failures.push(msg);
            return;
        }
    };
    match env.write_receipt(&sr::evidence_dir(&run_dir), &json) {
        Ok(p) => report.receipts.push(p),
        Err(e) => {
            // The payloads are already gone. Losing the record is a FAILURE of the command, not a
            // footnote: the full record is echoed into the notes so it is not lost entirely, and
            // `receipt_failures` makes the caller fail rather than read a zero exit as success.
            report.notes.push(format!(
                "receipt for run {run_id} NOT written ({e}); the record of this reap survives only \
                 in this report: {json}"
            ));
            report.receipt_failures.push(format!("run {run_id}: {e}"));
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------------------------

fn row(c: [&str; 5]) -> String {
    format!(
        "{:<9}  {:<16}  {:>10}  {:>10}  {}\n",
        c[0], c[1], c[2], c[3], c[4]
    )
}

pub fn render_text(r: &ReapReport) -> String {
    render_report(
        r,
        "--build-targets",
        "DESTRUCTIVE: build targets and per-run dependency caches that passed every boundary gate \
         were REMOVED.",
    )
}

/// The shared reap table. `class_flag` names the invocation the report belongs to and `destructive` is
/// the one-line banner for a real (non-dry) run — the two commands differ in exactly those two strings,
/// and a second copy of this renderer would be a second place for the dry-run disclosure to rot.
pub(crate) fn render_report(r: &ReapReport, class_flag: &str, destructive: &str) -> String {
    let mut s = String::new();
    if r.dry_run {
        s.push_str(&format!("a2a-bridge storage reap {class_flag} --dry-run\n"));
        s.push_str(
            "DRY RUN: every gate below was evaluated for real. No payload was deleted and no \
                    evidence was written.\n",
        );
        s.push_str(&format!(
            "ONE STATE-VISIBLE EFFECT: {DRY_RUN_SIDE_EFFECT}\n\n"
        ));
    } else {
        s.push_str(&format!("a2a-bridge storage reap {class_flag}\n"));
        s.push_str(destructive);
        s.push_str("\n\n");
    }
    s.push_str(&format!("scan root: {}\n\n", r.scan_root));
    s.push_str(&row(["outcome", "class", "logical", "on disk", "path"]));
    s.push_str(&row([
        "---------",
        "----------------",
        "----------",
        "----------",
        "----",
    ]));
    for it in &r.items {
        s.push_str(&row([
            it.outcome.label(),
            &it.class,
            &sr::human_bytes(it.logical_bytes),
            &sr::human_bytes(it.disk_bytes),
            &it.path,
        ]));
        if let Some(d) = it.outcome.detail() {
            s.push_str(&format!("           ↳ {d}\n"));
        }
        // A dry run exists to be READ before the operator authorizes a deletion, so the gate evidence
        // belongs on the page they are reading — not only in `--json`.
        if r.dry_run {
            for g in &it.gates {
                s.push_str(&format!("           · {g}\n"));
            }
        }
    }
    s.push('\n');
    s.push_str(&format!(
        "planned {} | deleted {} | parked {} | PARTIAL {} | UNKNOWN {}\n",
        r.count("planned"),
        r.count("deleted"),
        r.count("parked"),
        r.count("PARTIAL"),
        r.count("UNKNOWN"),
    ));
    if r.dry_run {
        s.push_str(&format!(
            "planned (on-disk sizes, nothing removed): {}\n",
            sr::human_bytes(Some(r.planned_disk_bytes()))
        ));
    }
    s.push_str(&format!(
        "removed (on-disk sizes): {}\n",
        sr::human_bytes(Some(r.removed_disk_bytes()))
    ));
    let freed = match (r.free_bytes_before, r.free_bytes_after) {
        (Some(b), Some(a)) => format!(
            "{} → {}",
            sr::human_bytes(Some(b)),
            sr::human_bytes(Some(a))
        ),
        _ => "unknown".to_string(),
    };
    s.push_str(&format!("volume free (MEASURED, statvfs): {freed}\n"));
    if !r.receipts.is_empty() {
        s.push_str("\nreceipts:\n");
        for p in &r.receipts {
            s.push_str(&format!("  {p}\n"));
        }
    }
    if !r.notes.is_empty() {
        s.push_str("\nnotes:\n");
        for n in &r.notes {
            s.push_str(&format!("  - {n}\n"));
        }
    }
    s
}

pub const REAP_USAGE: &str = "\
usage: a2a-bridge storage reap --build-targets [--dry-run] [--config <f>] [--json]

DESTRUCTIVE. Deletes build targets (and per-run dependency caches) under this config's
`<allowed_cwd_root>/.a2a-implement` root, and NOTHING else: never a source checkout, never evidence,
never an unclassified item, never a container volume, and never anything at, inside, or containing a
D-2 protected root.

  --build-targets     REQUIRED (or `--clones`). Names the payload classes this invocation may remove.
                      There is no default class: a bare `storage reap` is refused, and the two class
                      flags may not be combined — they are different authorities with different gates
                      and different receipts. See `storage reap --clones --help` for the other one.
  --dry-run           evaluate every boundary gate for real, delete nothing and write no evidence.
                      Its one state-visible effect is the operation-lock namespace (see below).
  --config <path>     registry config (default: ./a2a-bridge.toml).
  --json              machine-readable output instead of the table.

THE INVARIANT IS `IDLE`, NOT `COMPLETED`: a run is reapable when nothing is using it. No single gate
establishes that, so all of these must hold, rechecked AT THE BOUNDARY rather than at scan time:

  run owner      the pid in `impl-<pid>-<nonce>` is not running. NOTE the lease-vs-lock nuance: an
                 initial `implement` run holds only its ADR-0025 run lease and never the operation
                 lock, so the lock alone would NOT exclude it. This gate does.
  operation lock held across probe->delete. It excludes a concurrent `resume`/`merge` — and only
                 those. It is not evidence of idleness by itself.
  pinned root    the scan root is descriptor-pinned; its dev/ino is re-verified before AND after each
                 removal, and the payload's own identity immediately before the unlink.
  class          re-derived from ON-DISK evidence for every reapable class: cargo markers for a build
                 target, `package.json`/`pyvenv.cfg` provenance for a dependency cache (whose report
                 classification is by NAME alone, which is not proof of regenerability).
  consumers      one recursive process/open-file/cwd probe over the run directory must answer FREE.
  container axis with a runtime configured, an affirmative container answer is REQUIRED — the host
                 probe cannot see inside a container VM. With none configured the axis is reported as
                 uncovered rather than treated as empty.

A failed or nondiscriminating probe PARKS the payload with a typed reason; it never defaults to free.

Evidence brackets the operation: an fsync'd INTENT record is written before the first removal, and the
outcome receipt before the operation lock is released — both beside the run's checkpoint evidence. If a
receipt cannot be written the command FAILS, because a reap whose record was lost is not a clean reap.";

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::collections::BTreeSet;
    use std::rc::Rc;

    // -----------------------------------------------------------------------------------------
    // The injectable environment: every runtime/filesystem effect is observable and scriptable.
    // -----------------------------------------------------------------------------------------

    /// Every effect the orchestrator performs, in ORDER. R4's crash-durability ordering is an
    /// assertion about this sequence, not about any single call.
    #[derive(Clone, Debug, PartialEq, Eq)]
    enum Ev {
        Lock(String),
        Probe(String),
        Intent(String),
        Remove(String),
        Receipt(String),
        Unlock(String),
    }

    #[derive(Default)]
    struct Journal {
        locks_held: Vec<String>,
        /// Ordered effect log — the R4 ordering proof.
        events: Vec<Ev>,
        /// `(path, was the run's operation lock held when the probe ran)`.
        probe_witness: Vec<(String, bool)>,
        /// `(path, was the run's operation lock held when the removal ran)`.
        remove_witness: Vec<(String, bool)>,
        removed: Vec<String>,
        receipts: Vec<(String, String)>,
        intents: Vec<(String, String)>,
        progress: Vec<String>,
        free: u64,
    }

    impl Journal {
        fn kinds(&self) -> Vec<&'static str> {
            self.events
                .iter()
                .map(|e| match e {
                    Ev::Lock(_) => "lock",
                    Ev::Probe(_) => "probe",
                    Ev::Intent(_) => "intent",
                    Ev::Remove(_) => "remove",
                    Ev::Receipt(_) => "receipt",
                    Ev::Unlock(_) => "unlock",
                })
                .collect()
        }
    }

    struct FakeLock {
        run_id: String,
        j: Rc<RefCell<Journal>>,
    }

    impl Drop for FakeLock {
        fn drop(&mut self) {
            self.j.borrow_mut().locks_held.retain(|r| r != &self.run_id);
            let ev = Ev::Unlock(self.run_id.clone());
            self.j.borrow_mut().events.push(ev);
        }
    }

    /// Scripted `probe_consumers`. A closure (not a value) so a test can also MUTATE the world at the
    /// moment of the probe — the swap-window case needs the root replaced mid-boundary.
    type ProbeFn = RefCell<Box<dyn FnMut(&Path) -> ConsumerProbe>>;
    /// Scripted `remove_tree`, so partial and lying removals are expressible.
    type RemoveFn = RefCell<Box<dyn FnMut(&Path) -> Result<(), String>>>;

    struct FakeEnv {
        j: Rc<RefCell<Journal>>,
        contended: BTreeSet<String>,
        lock_broken: Option<String>,
        probe: ProbeFn,
        remove: RemoveFn,
        receipt_error: Option<String>,
        intent_error: Option<String>,
        /// Pids the fake host considers running. The fixture's run id is `impl-1-aa`, so pid 1 is the
        /// owner; leaving this empty models a CRASHED run (owner gone), which is the reapable case.
        alive_pids: BTreeSet<u32>,
        now: u64,
    }

    impl FakeEnv {
        fn new() -> Self {
            Self {
                j: Rc::new(RefCell::new(Journal {
                    free: 1_000_000,
                    ..Default::default()
                })),
                contended: BTreeSet::new(),
                lock_broken: None,
                probe: RefCell::new(Box::new(|_| ConsumerProbe::Free)),
                remove: RefCell::new(Box::new(|p| {
                    std::fs::remove_dir_all(p).map_err(|e| e.to_string())
                })),
                receipt_error: None,
                intent_error: None,
                alive_pids: BTreeSet::new(),
                now: 1_700_000_000,
            }
        }
        fn removed(&self) -> Vec<String> {
            self.j.borrow().removed.clone()
        }
    }

    impl ReapEnv for FakeEnv {
        type Lock = FakeLock;

        fn acquire_operation_lock(
            &self,
            implement_root: &Path,
            run_id: &str,
        ) -> Result<FakeLock, LockFailure> {
            if self.contended.contains(run_id) {
                return Err(LockFailure::Contended);
            }
            if let Some(e) = &self.lock_broken {
                return Err(LockFailure::Unavailable(e.clone()));
            }
            // Mirrors `acquire_persistent_lock_in`: taking the lock CREATES the namespace and the
            // lock file. Modelled here so the dry-run side-effect disclosure is testable against the
            // real write set rather than against a fake that touches nothing.
            let dir = implement_root.join(sr::OPERATION_LOCK_DIR);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join(format!("{run_id}.lock")), b"").unwrap();
            self.j.borrow_mut().locks_held.push(run_id.to_string());
            self.j
                .borrow_mut()
                .events
                .push(Ev::Lock(run_id.to_string()));
            Ok(FakeLock {
                run_id: run_id.to_string(),
                j: Rc::clone(&self.j),
            })
        }

        fn process_alive(&self, pid: u32) -> PidLiveness {
            if self.alive_pids.contains(&pid) {
                PidLiveness::Alive
            } else {
                PidLiveness::Dead
            }
        }

        fn probe_consumers(&self, path: &Path) -> ConsumerProbe {
            let held = !self.j.borrow().locks_held.is_empty();
            self.j
                .borrow_mut()
                .probe_witness
                .push((path.display().to_string(), held));
            self.j
                .borrow_mut()
                .events
                .push(Ev::Probe(path.display().to_string()));
            (self.probe.borrow_mut())(path)
        }

        fn free_bytes(&self, _path: &Path) -> Option<u64> {
            Some(self.j.borrow().free)
        }

        fn remove_tree(&self, path: &Path) -> Result<(), String> {
            let held = !self.j.borrow().locks_held.is_empty();
            self.j
                .borrow_mut()
                .remove_witness
                .push((path.display().to_string(), held));
            self.j
                .borrow_mut()
                .events
                .push(Ev::Remove(path.display().to_string()));
            let r = (self.remove.borrow_mut())(path);
            if std::fs::symlink_metadata(path).is_err() {
                // Whatever the call reported, the payload is gone: the volume gained space.
                self.j.borrow_mut().removed.push(path.display().to_string());
                self.j.borrow_mut().free += 4096;
            }
            r
        }

        fn now_epoch_secs(&self) -> u64 {
            self.now
        }

        fn write_intent(&self, evidence_dir: &Path, json: &str) -> Result<String, String> {
            if let Some(e) = &self.intent_error {
                return Err(e.clone());
            }
            std::fs::create_dir_all(evidence_dir).map_err(|e| e.to_string())?;
            let p = evidence_dir.join(format!("storage-reap-intent-{}.json", self.now));
            std::fs::write(&p, json).map_err(|e| e.to_string())?;
            self.j
                .borrow_mut()
                .intents
                .push((p.display().to_string(), json.to_string()));
            self.j
                .borrow_mut()
                .events
                .push(Ev::Intent(p.display().to_string()));
            Ok(p.display().to_string())
        }

        fn write_receipt(&self, evidence_dir: &Path, json: &str) -> Result<String, String> {
            if let Some(e) = &self.receipt_error {
                return Err(e.clone());
            }
            std::fs::create_dir_all(evidence_dir).map_err(|e| e.to_string())?;
            let p = evidence_dir.join(format!("storage-reap-{}.json", self.now));
            std::fs::write(&p, json).map_err(|e| e.to_string())?;
            self.j
                .borrow_mut()
                .receipts
                .push((p.display().to_string(), json.to_string()));
            self.j
                .borrow_mut()
                .events
                .push(Ev::Receipt(p.display().to_string()));
            Ok(p.display().to_string())
        }

        /// S3 never writes a stably-named file (its receipts are timestamped and live beside the
        /// checkpoint). Modelled anyway so the seam is exercised by both commands' fakes.
        fn write_named(&self, dir: &Path, file_name: &str, json: &str) -> Result<String, String> {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
            let p = dir.join(file_name);
            std::fs::write(&p, json).map_err(|e| e.to_string())?;
            Ok(p.display().to_string())
        }

        fn copy_evidence(&self, sources: &[PathBuf], to: &Path) -> Result<Vec<String>, String> {
            let mut out = Vec::new();
            for from in sources {
                if from.is_file() {
                    std::fs::create_dir_all(to).map_err(|e| e.to_string())?;
                    let name = from.file_name().unwrap_or_default().to_string_lossy();
                    std::fs::copy(from, to.join(name.as_ref())).map_err(|e| e.to_string())?;
                    out.push(name.into_owned());
                    continue;
                }
                if !from.is_dir() {
                    continue;
                }
                std::fs::create_dir_all(to).map_err(|e| e.to_string())?;
                for e in std::fs::read_dir(from).map_err(|e| e.to_string())? {
                    let e = e.map_err(|e| e.to_string())?;
                    if e.path().is_file() {
                        std::fs::copy(e.path(), to.join(e.file_name()))
                            .map_err(|e| e.to_string())?;
                        out.push(e.file_name().to_string_lossy().into_owned());
                    }
                }
            }
            Ok(out)
        }

        fn sync_dir(&self, _dir: &Path) -> Result<(), String> {
            Ok(())
        }

        fn progress(&self, message: &str) {
            self.j.borrow_mut().progress.push(message.to_string());
        }
    }

    // -----------------------------------------------------------------------------------------
    // Fixture
    // -----------------------------------------------------------------------------------------

    fn write(p: &Path, bytes: usize) {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, vec![b'x'; bytes]).unwrap();
    }

    /// A real cargo target: the name (checked by the classifier) plus the on-disk markers
    /// `is_cargo_target` requires.
    fn cargo_target(dir: &Path) {
        write(&dir.join("CACHEDIR.TAG"), 43);
        std::fs::create_dir_all(dir.join("debug")).unwrap();
        write(&dir.join("debug/blob"), 4096);
    }

    struct Fx {
        _td: tempfile::TempDir,
        root: PathBuf,
        implement: PathBuf,
        clone: PathBuf,
        target: PathBuf,
        user_repo: PathBuf,
    }

    /// One standalone clone under `.a2a-implement` carrying a cargo target, a node_modules cache, a
    /// checkpoint-evidence sidecar and source — plus a user checkout OUTSIDE the bridge root.
    fn fx() -> Fx {
        let td = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(td.path()).unwrap();
        let implement = root.join(".a2a-implement");
        let clone = implement.join("impl-1-aa");
        std::fs::create_dir_all(clone.join(".git")).unwrap();
        write(&clone.join(".git/HEAD"), 41);
        write(
            &clone.join(".git/a2a-bridge/implement-checkpoint.json"),
            512,
        );
        write(&clone.join("src/lib.rs"), 100);
        cargo_target(&clone.join("target"));
        write(&clone.join("node_modules/pkg/index.js"), 200);
        // npm provenance: the manifest that regenerates `node_modules`, beside it.
        write(&clone.join("package.json"), 40);

        let user_repo = root.join("user-repo");
        write(&user_repo.join("secret.txt"), 64);

        let target = clone.join("target");
        Fx {
            _td: td,
            root,
            implement,
            clone,
            target,
            user_repo,
        }
    }

    fn scan(implement: &Path) -> Vec<sr::ReportItem> {
        let mut notes = Vec::new();
        sr::scan_implement_root(implement, &mut notes)
    }

    /// Default: ZERO container runtimes configured, i.e. the container axis is out of scope and the
    /// reap may proceed with a disclosure. Tests that care about the container gate use `run_with`.
    fn run(f: &Fx, items: &[sr::ReportItem], env: &FakeEnv, dry_run: bool) -> ReapReport {
        run_with(f, items, env, dry_run, 0)
    }

    fn run_with(
        f: &Fx,
        items: &[sr::ReportItem],
        env: &FakeEnv,
        dry_run: bool,
        runtimes_configured: usize,
    ) -> ReapReport {
        reap_build_targets(
            ReapRequest {
                scan_root: &f.implement,
                items,
                protected: &ProtectedRoots::default(),
                dry_run,
                runtimes_configured,
            },
            env,
        )
    }

    /// Look an item up by its LEXICAL path. Never by `display_path`: that canonicalizes, and a test
    /// that swaps a payload for a symlink would then look up the symlink's TARGET and miss the very
    /// refusal it is asserting.
    fn item_for<'a>(r: &'a ReapReport, path: &Path) -> &'a ReapItem {
        let want = path.to_string_lossy().into_owned();
        r.items
            .iter()
            .find(|i| i.path == want)
            .unwrap_or_else(|| panic!("no reap item for {want}; got {:?}", r.items))
    }

    fn parked_reason(r: &ReapReport, path: &Path) -> ParkReason {
        match &item_for(r, path).outcome {
            ItemOutcome::Parked { reason } => reason.clone(),
            other => panic!("expected {} parked, got {other:?}", path.display()),
        }
    }

    // -----------------------------------------------------------------------------------------
    // Dry run and the happy path
    // -----------------------------------------------------------------------------------------

    /// Discriminates a `--dry-run` that is only a print flag: one that still deletes, or still writes
    /// a receipt, fails here because the target tree and the evidence dir are asserted intact.
    #[test]
    fn dry_run_evaluates_every_gate_and_touches_nothing() {
        let f = fx();
        let items = scan(&f.implement);
        let env = FakeEnv::new();
        let r = run(&f, &items, &env, true);

        assert_eq!(item_for(&r, &f.target).outcome, ItemOutcome::Planned);
        assert!(
            f.target.join("debug/blob").exists(),
            "dry run deleted the payload"
        );
        assert!(env.removed().is_empty(), "dry run called remove_tree");
        assert!(r.receipts.is_empty(), "dry run wrote a receipt");
        assert!(
            !sr::evidence_dir(&f.clone)
                .join(format!("storage-reap-{}.json", env.now))
                .exists(),
            "dry run left a receipt file on disk"
        );
        assert!(
            !item_for(&r, &f.target).gates.is_empty(),
            "no gate evidence"
        );
    }

    /// Discriminates a reaper that removes the whole run directory (a prefix sweep) instead of the
    /// exact payload: the checkout's source, `.git`, and evidence sidecar must all survive.
    #[test]
    fn a_real_reap_removes_the_exact_target_and_leaves_the_checkout() {
        let f = fx();
        let items = scan(&f.implement);
        let env = FakeEnv::new();
        let r = run(&f, &items, &env, false);

        assert_eq!(item_for(&r, &f.target).outcome, ItemOutcome::Deleted);
        assert!(!f.target.exists(), "the build target was not removed");
        assert!(f.clone.join("src/lib.rs").exists(), "source was destroyed");
        assert!(f.clone.join(".git/HEAD").exists(), "`.git` was destroyed");
        assert!(
            f.clone
                .join(".git/a2a-bridge/implement-checkpoint.json")
                .exists(),
            "evidence was destroyed"
        );
        assert!(f.user_repo.join("secret.txt").exists(), "escaped the root");
    }

    /// Discriminates a reaper that reports the payload's own size as "space reclaimed". The volume's
    /// answer is measured separately (`statvfs` before/after) and reported as its own number.
    #[test]
    fn freed_space_is_measured_from_the_volume_beside_the_payload_sizes() {
        let f = fx();
        let items = scan(&f.implement);
        let env = FakeEnv::new();
        let r = run(&f, &items, &env, false);

        let it = item_for(&r, &f.target);
        assert!(
            it.logical_bytes.unwrap() >= 4096,
            "logical bytes unmeasured"
        );
        assert!(it.disk_bytes.unwrap() > 0, "on-disk bytes unmeasured");
        assert_eq!(
            it.freed_bytes_measured,
            Some(4096),
            "the per-item statvfs delta was not measured"
        );
        assert_eq!(r.free_bytes_before, Some(1_000_000));
        assert_eq!(r.free_bytes_after, Some(1_000_000 + 2 * 4096));
    }

    // -----------------------------------------------------------------------------------------
    // D-2 protected roots
    // -----------------------------------------------------------------------------------------

    /// Discriminates a reaper that consults the protected list only for classes it would otherwise
    /// delete, or that never consults it at all: a protected root INSIDE the payload must refuse the
    /// payload, because deleting the parent deletes the protected child.
    #[test]
    fn a_protected_root_inside_the_payload_refuses_it() {
        let f = fx();
        let items = scan(&f.implement);
        let env = FakeEnv::new();
        let mut notes = Vec::new();
        let protected = ProtectedRoots::resolve(
            &[f.target.join("debug").to_string_lossy().into_owned()],
            &mut notes,
        )
        .unwrap();
        let r = reap_build_targets(
            ReapRequest {
                scan_root: &f.implement,
                items: &items,
                protected: &protected,
                dry_run: false,
                runtimes_configured: 0,
            },
            &env,
        );
        assert!(
            matches!(
                parked_reason(&r, &f.target),
                ParkReason::ProtectedRoot { .. }
            ),
            "a payload containing a D-2 protected root was not refused"
        );
        assert!(f.target.join("debug/blob").exists(), "deleted a D-2 root");
    }

    /// Discriminates a reaper whose protected-root check runs after classification: the refusal must
    /// come from the path's canonical identity, so it fires even in a dry run's plan.
    #[test]
    fn a_payload_under_a_protected_root_is_refused_even_in_dry_run() {
        let f = fx();
        let items = scan(&f.implement);
        let env = FakeEnv::new();
        let mut notes = Vec::new();
        let protected =
            ProtectedRoots::resolve(&[f.clone.to_string_lossy().into_owned()], &mut notes).unwrap();
        let r = reap_build_targets(
            ReapRequest {
                scan_root: &f.implement,
                items: &items,
                protected: &protected,
                dry_run: true,
                runtimes_configured: 0,
            },
            &env,
        );
        assert!(matches!(
            parked_reason(&r, &f.target),
            ParkReason::ProtectedRoot { .. }
        ));
    }

    /// Discriminates a protected-root matcher written as `starts_with` over strings: `/x/ab` is not
    /// under `/x/a`, and treating it as such would refuse unrelated payloads (and, inverted, would
    /// let `/x/a-evil` masquerade as protected).
    #[test]
    fn protected_roots_match_path_components_not_string_prefixes() {
        let td = tempfile::tempdir().unwrap();
        let base = std::fs::canonicalize(td.path()).unwrap();
        std::fs::create_dir_all(base.join("a")).unwrap();
        std::fs::create_dir_all(base.join("ab")).unwrap();
        let mut notes = Vec::new();
        let p =
            ProtectedRoots::resolve(&[base.join("a").to_string_lossy().into_owned()], &mut notes)
                .unwrap();
        assert!(p.refuse(&base.join("a")).is_some());
        assert!(p.refuse(&base.join("a/inner")).is_some());
        assert!(
            p.refuse(&base.join("ab")).is_none(),
            "sibling `ab` was matched as if it were under `a`"
        );
    }

    /// Discriminates a resolver that swallows an unreadable protected root: not being able to identify
    /// a D-2 root is exactly when the reaper must refuse rather than proceed.
    #[test]
    fn an_unresolvable_protected_root_refuses_rather_than_being_skipped() {
        let td = tempfile::tempdir().unwrap();
        let base = std::fs::canonicalize(td.path()).unwrap();
        let dangling = base.join("dangling");
        #[cfg(unix)]
        std::os::unix::fs::symlink(base.join("nowhere"), &dangling).unwrap();
        let mut notes = Vec::new();
        let e = ProtectedRoots::resolve(&[dangling.to_string_lossy().into_owned()], &mut notes)
            .unwrap_err();
        assert!(e.contains("canonical"), "unexpected refusal text: {e}");

        // An entry that is simply ABSENT protects nothing: noted, not fatal.
        let mut notes2 = Vec::new();
        let ok = ProtectedRoots::resolve(
            &[base.join("never-existed").to_string_lossy().into_owned()],
            &mut notes2,
        )
        .unwrap();
        assert!(ok.is_empty());
        assert!(notes2.iter().any(|n| n.contains("does not exist")));
    }

    // -----------------------------------------------------------------------------------------
    // Class authority
    // -----------------------------------------------------------------------------------------

    /// Discriminates a reaper that sweeps the run directory by prefix: `SourceCheckout` is S4's
    /// authority, `Evidence` is never cache, and `Unclassified` must be refused by name.
    #[test]
    fn only_build_targets_and_per_run_dependency_caches_are_ever_removed() {
        let f = fx();
        write(&f.implement.join("stray-dir/notes.txt"), 7);
        let items = scan(&f.implement);
        let env = FakeEnv::new();
        let r = run(&f, &items, &env, false);

        for it in &r.items {
            let cls = it.class.as_str();
            if matches!(it.outcome, ItemOutcome::Deleted) {
                assert!(
                    cls == "BuildTarget" || cls == "DependencyCache",
                    "deleted a {cls}: {}",
                    it.path
                );
            }
        }
        assert!(f.clone.join("src/lib.rs").exists());
        assert!(f.implement.join("stray-dir/notes.txt").exists());
        assert!(matches!(
            parked_reason(&r, &f.clone),
            ParkReason::NotReapableClass { .. }
        ));
        assert!(matches!(
            parked_reason(&r, &sr::evidence_dir(&f.clone)),
            ParkReason::NotReapableClass { .. }
        ));
    }

    /// Discriminates a reaper that treats a `ContainerOrImage` row as just another oversized payload.
    /// Volumes stay under ADR-0021/0025 authority; S3 never touches one, and its `path` is a volume
    /// NAME, so a filesystem removal there would be aimed at a relative path in the process cwd.
    #[test]
    fn a_container_volume_row_is_refused_by_name() {
        let f = fx();
        let items = vec![sr::ReportItem {
            path: "a2a-impl-target-0123456789abcdef".into(),
            source: sr::ItemSource::VolumeName,
            class: sr::PayloadClass::ContainerOrImage,
            checkout_kind: None,
            run_id: Some("impl-1-aa".into()),
            measured: sr::Measured::default(),
            consumers: sr::LiveConsumers::default(),
            git: None,
            note: None,
        }];
        let env = FakeEnv::new();
        let r = run(&f, &items, &env, false);
        assert_eq!(
            r.items[0].outcome,
            ItemOutcome::Parked {
                reason: ParkReason::ContainerVolume
            }
        );
        assert!(env.removed().is_empty());
    }

    /// `classify_volume` maps `a2a-*-cache-<hash>` / `a2a-*-target-<hash>` to `DependencyCache` /
    /// `BuildTarget` — the SAME classes this reaper deletes — while their `path` is a volume NAME.
    /// Discriminates a reaper whose only defence against them is the containment gate: refusing a
    /// volume must not depend on the accident that a relative name fails an absolute-path check
    /// downstream, and the operator must be told it was refused as a VOLUME, not as a stray path.
    #[test]
    fn a_cache_volume_classified_as_a_reapable_class_is_still_refused_as_a_volume() {
        let f = fx();
        let items = vec![
            sr::ReportItem {
                path: "a2a-verify-cache-0123456789abcdef".into(),
                source: sr::ItemSource::VolumeName,
                class: sr::PayloadClass::DependencyCache,
                checkout_kind: None,
                run_id: Some("impl-1-aa".into()),
                measured: sr::Measured::default(),
                consumers: sr::LiveConsumers::default(),
                git: None,
                note: None,
            },
            sr::ReportItem {
                path: "a2a-impl-lsp-target-0123456789abcdef".into(),
                source: sr::ItemSource::VolumeName,
                class: sr::PayloadClass::BuildTarget,
                checkout_kind: None,
                run_id: None,
                measured: sr::Measured::default(),
                consumers: sr::LiveConsumers::default(),
                git: None,
                note: None,
            },
        ];
        let env = FakeEnv::new();
        let r = run(&f, &items, &env, false);
        for it in &r.items {
            assert_eq!(
                it.outcome,
                ItemOutcome::Parked {
                    reason: ParkReason::ContainerVolume
                },
                "{} was not refused as a container volume",
                it.path
            );
        }
        assert!(env.removed().is_empty());
    }

    /// The S3 review's carried finding: destructive code must not INFER volume-vs-path. Discriminates
    /// the inference itself — a row the scanner declared a VOLUME, carrying a reapable class AND a
    /// path that would pass every path-shaped check (it is absolute, canonical, and a real cargo
    /// target on this disk). The old `class == ContainerOrImage || !path.is_absolute()` rule deletes
    /// it; the typed `source` field refuses it.
    #[test]
    fn a_volume_row_is_refused_by_its_typed_source_not_by_the_shape_of_its_path() {
        let f = fx();
        let items = vec![sr::ReportItem {
            path: sr::display_path(&f.target),
            source: sr::ItemSource::VolumeName,
            class: sr::PayloadClass::BuildTarget,
            checkout_kind: None,
            run_id: Some("impl-1-aa".into()),
            measured: sr::Measured::default(),
            consumers: sr::LiveConsumers::default(),
            git: None,
            note: None,
        }];
        let env = FakeEnv::new();
        let r = run(&f, &items, &env, false);
        assert_eq!(
            r.items[0].outcome,
            ItemOutcome::Parked {
                reason: ParkReason::ContainerVolume
            },
            "an absolute-pathed volume row was not refused by its declared source"
        );
        assert!(f.target.join("debug/blob").exists());
        assert!(env.removed().is_empty());
    }

    /// Discriminates a reaper that reaps whatever the scan hands it regardless of which root it came
    /// from. A `[worktrees]` payload has no `impl-<pid>-<nonce>` owner and no per-run operation lock,
    /// so NEITHER of this command's idleness gates applies — it must be refused by source, before any
    /// gate that would silently pass for want of evidence.
    #[test]
    fn a_worktree_sourced_item_is_refused_by_source() {
        let f = fx();
        let items = vec![sr::ReportItem {
            path: sr::display_path(&f.target),
            source: sr::ItemSource::WorktreePath,
            class: sr::PayloadClass::BuildTarget,
            checkout_kind: None,
            run_id: Some("impl-1-aa".into()),
            measured: sr::Measured::default(),
            consumers: sr::LiveConsumers::default(),
            git: None,
            note: None,
        }];
        let env = FakeEnv::new();
        let r = run(&f, &items, &env, false);
        assert_eq!(
            r.items[0].outcome,
            ItemOutcome::Parked {
                reason: ParkReason::WorktreeCustody
            }
        );
        assert!(f.target.join("debug/blob").exists());
    }

    /// Discriminates a reaper that reaps a nested payload with no owning run: with no run there is no
    /// operation lock to hold, so there is no boundary to gate the deletion on.
    #[test]
    fn a_payload_with_no_owning_run_is_parked() {
        let f = fx();
        let mut items = scan(&f.implement);
        for it in &mut items {
            it.run_id = None;
        }
        let env = FakeEnv::new();
        let r = run(&f, &items, &env, false);
        assert!(matches!(
            parked_reason(&r, &f.target),
            ParkReason::NoOwningRun
        ));
        assert!(f.target.exists());
    }

    // -----------------------------------------------------------------------------------------
    // The operation lock
    // -----------------------------------------------------------------------------------------

    /// Discriminates a reaper that probes and deletes without the run's operation lock. The S2
    /// evidence measured a just-released flock still reading `Held` for later probes, so a probe taken
    /// outside the lock is inadmissible: this asserts the lock was HELD at both the probe and the
    /// removal, not merely acquired somewhere in the command.
    #[test]
    fn the_operation_lock_is_held_across_probe_and_delete() {
        let f = fx();
        let items = scan(&f.implement);
        let env = FakeEnv::new();
        let _ = run(&f, &items, &env, false);
        let j = env.j.borrow();
        assert!(!j.probe_witness.is_empty(), "no consumer probe ran at all");
        assert!(
            j.probe_witness.iter().all(|(_, held)| *held),
            "a consumer probe ran without the operation lock held: {:?}",
            j.probe_witness
        );
        assert!(!j.remove_witness.is_empty(), "nothing was removed");
        assert!(
            j.remove_witness.iter().all(|(_, held)| *held),
            "a removal ran without the operation lock held: {:?}",
            j.remove_witness
        );
    }

    /// Discriminates a reaper that treats "the lock is busy" as a soft condition. A held operation
    /// lock means a resume or merge owns the run; every payload of that run must be parked untouched.
    #[test]
    fn a_contended_operation_lock_parks_every_payload_of_that_run() {
        let f = fx();
        let items = scan(&f.implement);
        let mut env = FakeEnv::new();
        env.contended.insert("impl-1-aa".to_string());
        let r = run(&f, &items, &env, false);
        assert!(matches!(
            parked_reason(&r, &f.target),
            ParkReason::OperationLockHeld { .. }
        ));
        assert!(f.target.exists(), "deleted while another operation held it");
        assert!(env.removed().is_empty());
    }

    /// Discriminates a reaper that collapses "cannot take the lock" into "nobody holds it". A broken
    /// lock namespace is a failed probe, and a failed probe never licenses a deletion.
    #[test]
    fn an_unavailable_operation_lock_parks_rather_than_proceeding() {
        let f = fx();
        let items = scan(&f.implement);
        let mut env = FakeEnv::new();
        env.lock_broken = Some("read-only filesystem".into());
        let r = run(&f, &items, &env, false);
        assert!(matches!(
            parked_reason(&r, &f.target),
            ParkReason::OperationLockUnavailable { .. }
        ));
        assert!(f.target.exists());
    }

    /// Discriminates a reaper that holds the lock for the whole command rather than per run: the lock
    /// must be released once its run's payloads are done, or a long reap starves every concurrent
    /// resume.
    #[test]
    fn the_operation_lock_is_released_when_its_run_is_done() {
        let f = fx();
        let items = scan(&f.implement);
        let env = FakeEnv::new();
        let _ = run(&f, &items, &env, false);
        assert!(
            env.j.borrow().locks_held.is_empty(),
            "operation locks outlived the reap: {:?}",
            env.j.borrow().locks_held
        );
    }

    // -----------------------------------------------------------------------------------------
    // Consumer probe
    // -----------------------------------------------------------------------------------------

    /// Discriminates a reaper that deletes a payload a live process still has open. The probe answers
    /// `Held`, which is a refusal, not advice.
    #[test]
    fn a_live_open_file_consumer_parks_the_payload() {
        let f = fx();
        let items = scan(&f.implement);
        let env = FakeEnv::new();
        *env.probe.borrow_mut() = Box::new(|_| ConsumerProbe::Held {
            detail: "pid 4242".into(),
        });
        let r = run(&f, &items, &env, false);
        assert!(matches!(
            parked_reason(&r, &f.target),
            ParkReason::LiveConsumer { .. }
        ));
        assert!(f.target.exists());
        assert!(env.removed().is_empty());
    }

    /// Discriminates a reaper that reads a failed probe as "nothing found". `lsof` exits 1 both for
    /// "no consumers" and for a genuine error; a probe that could not answer must park, never free.
    #[test]
    fn a_failed_consumer_probe_parks_and_never_reads_as_free() {
        let f = fx();
        let items = scan(&f.implement);
        let env = FakeEnv::new();
        *env.probe.borrow_mut() = Box::new(|_| ConsumerProbe::Failed {
            detail: "lsof not installed".into(),
        });
        let r = run(&f, &items, &env, false);
        assert!(matches!(
            parked_reason(&r, &f.target),
            ParkReason::ConsumerProbeFailed { .. }
        ));
        assert!(f.target.exists());
    }

    /// Discriminates a reaper that only probes processes and ignores the container-mount / run-lease
    /// evidence the S2 scan already established for this item.
    #[test]
    fn a_container_mount_holder_from_the_scan_parks_the_payload() {
        let f = fx();
        let mut items = scan(&f.implement);
        for it in &mut items {
            it.consumers.container_mount = sr::HolderState::Held;
        }
        let env = FakeEnv::new();
        let r = run(&f, &items, &env, false);
        assert!(matches!(
            parked_reason(&r, &f.target),
            ParkReason::LiveConsumer { .. }
        ));
        assert!(f.target.exists());
    }

    // -----------------------------------------------------------------------------------------
    // Boundary re-verification: class, identity, containment
    // -----------------------------------------------------------------------------------------

    /// Discriminates a reaper that trusts the scan's class label. Between the scan and the boundary
    /// the cargo markers can be gone (or never have been the thing the name suggested); the class must
    /// be re-derived from on-disk evidence immediately before removal.
    #[test]
    fn a_classifier_disagreement_at_the_boundary_parks_the_payload() {
        let f = fx();
        let items = scan(&f.implement);
        // Between scan and boundary the cargo evidence disappears — what is left is a directory named
        // `target` holding someone's data, which `is_cargo_target` must refuse to promote.
        std::fs::remove_file(f.target.join("CACHEDIR.TAG")).unwrap();
        std::fs::remove_dir_all(f.target.join("debug")).unwrap();
        write(&f.target.join("irreplaceable.bin"), 32);
        let env = FakeEnv::new();
        let r = run(&f, &items, &env, false);
        assert!(
            matches!(
                parked_reason(&r, &f.target),
                ParkReason::ClassifierDisagreement { .. }
            ),
            "a stale class label licensed the deletion"
        );
        assert!(f.target.join("irreplaceable.bin").exists());
    }

    /// Discriminates a reaper that follows a symlink at the destructive boundary. A `:rw` container
    /// can replace the payload with a link to a user checkout between scan and reap.
    #[test]
    #[cfg(unix)]
    fn a_symlinked_payload_is_refused_and_never_followed() {
        let f = fx();
        let items = scan(&f.implement);
        std::fs::remove_dir_all(&f.target).unwrap();
        std::os::unix::fs::symlink(&f.user_repo, &f.target).unwrap();
        let env = FakeEnv::new();
        let r = run(&f, &items, &env, false);
        assert!(matches!(
            parked_reason(&r, &f.target),
            ParkReason::PathIsSymlink
        ));
        assert!(
            f.user_repo.join("secret.txt").exists(),
            "followed a symlink out of the bridge root and deleted a user checkout"
        );
    }

    /// Discriminates a reaper that trusts the item's `path` string. Anything not strictly inside the
    /// pinned scan root — and inside its own run directory — is outside this command's authority.
    #[test]
    fn an_item_outside_the_pinned_scan_root_is_refused() {
        let f = fx();
        let outside = f.root.join("elsewhere-target");
        cargo_target(&outside);
        let items = vec![sr::ReportItem {
            path: sr::display_path(&outside),
            source: sr::ItemSource::ImplementPath,
            class: sr::PayloadClass::BuildTarget,
            checkout_kind: None,
            run_id: Some("impl-1-aa".into()),
            measured: sr::Measured::default(),
            consumers: sr::LiveConsumers::default(),
            git: None,
            note: None,
        }];
        let env = FakeEnv::new();
        let r = run(&f, &items, &env, false);
        assert!(matches!(
            parked_reason(&r, &outside),
            ParkReason::NotUnderScanRoot { .. }
        ));
        assert!(outside.join("debug/blob").exists());
    }

    /// Discriminates a reaper that accepts any directory named `target` under the implement root. The
    /// enclosing run must itself be a standalone clone: regenerability is a property of a cargo
    /// workspace inside a known checkout, not of a directory name.
    #[test]
    fn a_target_whose_enclosing_run_is_not_a_clone_is_refused() {
        let f = fx();
        let orphan = f.implement.join("impl-9-orphan");
        cargo_target(&orphan.join("target"));
        let items = scan(&f.implement);
        let env = FakeEnv::new();
        let r = run(&f, &items, &env, false);
        assert!(matches!(
            parked_reason(&r, &orphan.join("target")),
            ParkReason::EnclosingRunNotAClone { .. }
        ));
        assert!(orphan.join("target/debug/blob").exists());
        // The well-formed clone in the same root is unaffected.
        assert_eq!(item_for(&r, &f.target).outcome, ItemOutcome::Deleted);
    }

    /// Discriminates a reaper that resolves paths against a root it only checked once. `verify_root`
    /// is two syscalls on a path, so a hostile parent can swap the root's leaf afterwards; the pinned
    /// descriptor's dev/ino must be re-verified immediately before the removal.
    #[test]
    #[cfg(unix)]
    fn a_scan_root_swapped_before_the_removal_refuses_the_deletion() {
        let f = fx();
        let items = scan(&f.implement);
        let env = FakeEnv::new();
        let implement = f.implement.clone();
        let decoy = f.root.join("decoy-implement");
        std::fs::create_dir_all(&decoy).unwrap();
        // Swap the root's leaf for a DIFFERENT directory during the probe — i.e. after the pin, before
        // the removal, exactly the window `verify_root` cannot close.
        *env.probe.borrow_mut() = Box::new(move |_| {
            let _ = std::fs::rename(
                &implement,
                implement.parent().unwrap().join(".swapped-away"),
            );
            let _ = std::fs::rename(&decoy, &implement);
            ConsumerProbe::Free
        });
        let r = run(&f, &items, &env, false);
        assert!(
            matches!(
                parked_reason(&r, &f.target),
                ParkReason::ScanRootIdentityChanged { .. }
            ),
            "deleted through a swapped scan root"
        );
        assert!(env.removed().is_empty(), "a removal ran after the swap");
    }

    // -----------------------------------------------------------------------------------------
    // Truthful partial / unknown recording
    // -----------------------------------------------------------------------------------------

    /// Discriminates a reaper that maps any removal error to a clean refusal. A removal that began and
    /// left the payload present is `Partial`, and the operator must be told so.
    #[test]
    fn a_removal_that_left_the_payload_present_is_recorded_partial() {
        let f = fx();
        let items = scan(&f.implement);
        let env = FakeEnv::new();
        *env.remove.borrow_mut() = Box::new(|p| {
            // Half the tree goes; the rest stays.
            let _ = std::fs::remove_dir_all(p.join("debug"));
            Err("permission denied on CACHEDIR.TAG".into())
        });
        let r = run(&f, &items, &env, false);
        match &item_for(&r, &f.target).outcome {
            ItemOutcome::Partial { detail } => assert!(detail.contains("permission denied")),
            other => panic!("expected Partial, got {other:?}"),
        }
        assert!(f.target.exists());
    }

    /// Discriminates a reaper that reports success whenever the payload is gone. When the removal
    /// reported an error the result is not established: the honest record is `Unknown`, not `Deleted`.
    #[test]
    fn a_removal_that_errored_but_emptied_the_path_is_recorded_unknown() {
        let f = fx();
        let items = scan(&f.implement);
        let env = FakeEnv::new();
        *env.remove.borrow_mut() = Box::new(|p| {
            let _ = std::fs::remove_dir_all(p);
            Err("interrupted".into())
        });
        let r = run(&f, &items, &env, false);
        match &item_for(&r, &f.target).outcome {
            ItemOutcome::Unknown { detail } => assert!(detail.contains("interrupted")),
            other => panic!("expected Unknown, got {other:?}"),
        }
    }

    // -----------------------------------------------------------------------------------------
    // Receipts
    // -----------------------------------------------------------------------------------------

    /// Discriminates a receipt that records only a path list. The plan's Evidence-class receipt must
    /// name WHAT was deleted, WHEN, and the gate evidence that licensed it.
    #[test]
    fn the_receipt_names_what_when_and_the_gate_evidence() {
        let f = fx();
        let items = scan(&f.implement);
        let env = FakeEnv::new();
        let r = run(&f, &items, &env, false);
        assert_eq!(r.receipts.len(), 1, "expected one receipt for one run");
        let raw = std::fs::read_to_string(&r.receipts[0]).unwrap();
        let v: serde_json::Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(v["schema"], RECEIPT_SCHEMA);
        assert_eq!(v["run_id"], "impl-1-aa");
        assert_eq!(v["at_epoch_secs"], env.now);
        assert_eq!(v["dry_run"], false);
        assert!(
            v["scan_root_identity"]
                .as_str()
                .is_some_and(|s| !s.is_empty()),
            "receipt does not attest the pinned root identity"
        );
        let entry = v["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|i| i["path"] == sr::display_path(&f.target))
            .expect("target missing from the receipt");
        assert_eq!(entry["outcome"], "deleted");
        assert!(entry["disk_bytes"].as_u64().unwrap() > 0);
        let gates = entry["gates"].as_array().unwrap();
        assert!(
            gates.iter().any(|g| g.as_str().unwrap().contains("lock"))
                && gates
                    .iter()
                    .any(|g| g.as_str().unwrap().contains("consumer"))
                && gates
                    .iter()
                    .any(|g| g.as_str().unwrap().contains("classifier")),
            "gate evidence is not recorded on the receipt: {gates:?}"
        );
        // Evidence class: the receipt lives beside the checkpoint and survives the reap.
        assert!(r.receipts[0].starts_with(&sr::display_path(&sr::evidence_dir(&f.clone))));
    }

    /// Discriminates a reaper that loses the record when the evidence path is unwritable. The reap
    /// happened; the record of it must then survive in the report's notes.
    #[test]
    fn an_unwritable_receipt_path_is_disclosed_in_the_notes_with_the_record() {
        let f = fx();
        let items = scan(&f.implement);
        let mut env = FakeEnv::new();
        env.receipt_error = Some("read-only filesystem".into());
        let r = run(&f, &items, &env, false);
        assert!(r.receipts.is_empty());
        assert!(
            r.notes
                .iter()
                .any(|n| n.contains("NOT written") && n.contains("impl-1-aa")),
            "the lost receipt was not disclosed: {:?}",
            r.notes
        );
    }

    // -----------------------------------------------------------------------------------------
    // R1(a) — run-owner liveness. The operation lock does NOT exclude an initial `implement` run.
    // -----------------------------------------------------------------------------------------

    /// THE load-bearing gate. `implement_cmd` takes an ADR-0025 run LEASE
    /// (`acquire_lease(&instance_id)`, `~/.a2a-bridge/leases/<pid>-<nonce>.lock`) and never touches
    /// `.operation-locks/<id>.lock` — only `implement_resume` and `merge` do. So a reaper gated
    /// solely on the operation lock is INERT against a live initial run: during host-side phases
    /// there is no container in `docker ps` and no descriptor under `target/`, and the run's build
    /// target reaps out from under it while the receipt cites "operation lock HELD" as its licence.
    /// Discriminates exactly that reaper.
    #[test]
    fn a_run_whose_owning_process_is_still_alive_is_parked() {
        let f = fx();
        let items = scan(&f.implement);
        let mut env = FakeEnv::new();
        env.alive_pids.insert(1); // the fixture run is `impl-1-aa`
        let r = run(&f, &items, &env, false);
        assert_eq!(
            parked_reason(&r, &f.target),
            ParkReason::RunOwnerAlive { pid: 1 },
            "a LIVE run's build target was reaped"
        );
        assert!(f.target.join("debug/blob").exists());
        assert!(env.removed().is_empty());
    }

    /// The other half of the invariant, so the gate cannot be satisfied by refusing everything: a
    /// CRASHED run (owner gone) is exactly the case this reaper exists for. Its target is
    /// regenerable, and reaping it is the point — no checkpoint-phase gating, which would strand a
    /// crashed run stuck in `InLoop`.
    #[test]
    fn a_crashed_runs_target_is_still_reapable() {
        let f = fx();
        let items = scan(&f.implement);
        let env = FakeEnv::new(); // no live pids ⇒ owner is gone
        let r = run(&f, &items, &env, false);
        assert_eq!(item_for(&r, &f.target).outcome, ItemOutcome::Deleted);
        assert!(!f.target.exists());
    }

    /// Discriminates a reaper that treats an unrecognized run-directory name as "probably fine". If
    /// the name does not identify the owning process, the run cannot be shown idle — the ADR-0025
    /// lease is keyed on a DIFFERENT nonce, so there is no second way to find the owner.
    #[test]
    fn a_run_id_that_does_not_name_its_owner_is_parked() {
        let f = fx();
        let legacy = f.implement.join("legacy-run");
        std::fs::create_dir_all(legacy.join(".git")).unwrap();
        write(&legacy.join(".git/HEAD"), 41);
        cargo_target(&legacy.join("target"));
        let items = scan(&f.implement);
        let env = FakeEnv::new();
        let r = run(&f, &items, &env, false);
        assert!(
            matches!(
                parked_reason(&r, &legacy.join("target")),
                ParkReason::RunIdNotParseable { .. }
            ),
            "an unidentifiable run's target was reaped"
        );
        assert!(legacy.join("target/debug/blob").exists());
    }

    /// Discriminates a lenient parser: a prefix match on `impl-`, or a pid segment accepted without
    /// parsing, would hand the liveness gate a pid it never verified.
    #[test]
    fn run_owner_pid_accepts_only_the_implement_task_id_shape() {
        assert_eq!(run_owner_pid("impl-4242-k3x9").unwrap(), 4242);
        for bad in [
            "legacy-run",
            "impl-4242",
            "impl-4242-k3x9-extra",
            "impl--k3x9",
            "impl-abc-k3x9",
            "impl-0-k3x9",
            "notimpl-1-a",
            "impl-4242-",
        ] {
            assert!(run_owner_pid(bad).is_err(), "{bad:?} was accepted");
        }
    }

    // -----------------------------------------------------------------------------------------
    // R1(b) — the container axis
    // -----------------------------------------------------------------------------------------

    /// The host `lsof` cannot see inside a container VM (OrbStack runs a separate kernel), so an
    /// `lsof`-free reading says nothing about a container holding the payload. Discriminates a reaper
    /// that treats `container_mount == Unknown` as permission when a runtime IS configured.
    #[test]
    fn an_unanswered_container_axis_parks_when_a_runtime_is_configured() {
        let f = fx();
        let items = scan(&f.implement); // container_mount defaults to Unknown
        let env = FakeEnv::new();
        let r = run_with(&f, &items, &env, false, 1);
        assert_eq!(
            parked_reason(&r, &f.target),
            ParkReason::ContainerAxisUnanswered,
            "an unchecked container axis licensed a deletion"
        );
        assert!(f.target.exists());
    }

    /// The affirmative case, so the gate is not simply "always refuse when a runtime exists".
    #[test]
    fn an_affirmative_free_container_answer_proceeds() {
        let f = fx();
        let mut items = scan(&f.implement);
        for it in &mut items {
            it.consumers.container_mount = sr::HolderState::Free;
        }
        let env = FakeEnv::new();
        let r = run_with(&f, &items, &env, false, 1);
        assert_eq!(item_for(&r, &f.target).outcome, ItemOutcome::Deleted);
    }

    /// With no runtime configured there is no container axis to answer. Discriminates a reaper that
    /// silently reads that as "no containers hold it" instead of "this axis was not covered".
    #[test]
    fn a_zero_runtime_config_proceeds_with_the_uncovered_axis_disclosed() {
        let f = fx();
        let items = scan(&f.implement);
        let env = FakeEnv::new();
        let r = run_with(&f, &items, &env, false, 0);
        assert_eq!(item_for(&r, &f.target).outcome, ItemOutcome::Deleted);
        let gates = &item_for(&r, &f.target).gates;
        assert!(
            gates.iter().any(|g| g.contains("container axis")
                && (g.contains("not covered") || g.contains("uncovered"))),
            "the uncovered container axis was not disclosed in the gate evidence: {gates:?}"
        );
    }

    // -----------------------------------------------------------------------------------------
    // R2 — dependency-cache provenance
    // -----------------------------------------------------------------------------------------

    /// `classify_nested` matches `node_modules` BY NAME ALONE — unlike `BuildTarget`, which
    /// `is_cargo_target` backs with real markers. Discriminates a boundary re-derivation that inherits
    /// that weakness: a directory named `node_modules` holding a user's own files is not regenerable,
    /// and deleting it is unrecoverable.
    #[test]
    fn a_node_modules_without_a_manifest_is_parked() {
        let f = fx();
        std::fs::remove_file(f.clone.join("package.json")).unwrap();
        write(&f.clone.join("node_modules/irreplaceable.txt"), 64);
        let items = scan(&f.implement);
        let env = FakeEnv::new();
        let r = run(&f, &items, &env, false);
        assert!(
            matches!(
                parked_reason(&r, &f.clone.join("node_modules")),
                ParkReason::NoCacheProvenance { .. }
            ),
            "a name-only `node_modules` was deleted"
        );
        assert!(f.clone.join("node_modules/irreplaceable.txt").exists());
        // The cargo-backed payload in the same run is unaffected.
        assert_eq!(item_for(&r, &f.target).outcome, ItemOutcome::Deleted);
    }

    /// `.venv` needs the marker `python -m venv` writes. Discriminates a provenance rule that only
    /// covers npm.
    #[test]
    fn a_venv_is_reapable_only_with_its_pyvenv_marker() {
        let f = fx();
        let venv = f.clone.join(".venv");
        write(&venv.join("lib/site-packages/thing.py"), 128);
        let items = scan(&f.implement);
        let env = FakeEnv::new();
        let r = run(&f, &items, &env, false);
        assert!(matches!(
            parked_reason(&r, &venv),
            ParkReason::NoCacheProvenance { .. }
        ));
        assert!(venv.exists());

        let f2 = fx();
        let venv2 = f2.clone.join(".venv");
        write(&venv2.join("lib/site-packages/thing.py"), 128);
        write(&venv2.join("pyvenv.cfg"), 32);
        let items2 = scan(&f2.implement);
        let env2 = FakeEnv::new();
        let r2 = run(&f2, &items2, &env2, false);
        assert_eq!(item_for(&r2, &venv2).outcome, ItemOutcome::Deleted);
    }

    /// The pure rule, including the case that must never be silently accepted: an unknown cache name.
    #[test]
    fn dependency_cache_provenance_requires_a_real_marker() {
        let td = tempfile::tempdir().unwrap();
        let base = std::fs::canonicalize(td.path()).unwrap();
        let nm = base.join("node_modules");
        std::fs::create_dir_all(&nm).unwrap();
        assert!(dependency_cache_provenance(&nm).is_err());
        write(&base.join("package.json"), 10);
        assert!(dependency_cache_provenance(&nm)
            .unwrap()
            .contains("package"));

        let venv = base.join(".venv");
        std::fs::create_dir_all(&venv).unwrap();
        assert!(dependency_cache_provenance(&venv).is_err());
        write(&venv.join("pyvenv.cfg"), 10);
        assert!(dependency_cache_provenance(&venv)
            .unwrap()
            .contains("pyvenv"));

        let other = base.join("vendor");
        std::fs::create_dir_all(&other).unwrap();
        assert!(dependency_cache_provenance(&other).is_err());
    }

    // -----------------------------------------------------------------------------------------
    // R4 — crash-durable evidence ordering
    // -----------------------------------------------------------------------------------------

    /// Discriminates the ordering that makes a crash unreadable: a receipt written only AFTER the
    /// removals describes an end state a crash may have prevented from ever existing, and a receipt
    /// written after the lock is dropped can interleave with a racing resume. Intent must precede the
    /// first removal; the receipt must precede the unlock.
    #[test]
    fn intent_precedes_the_first_removal_and_the_receipt_precedes_the_unlock() {
        let f = fx();
        let items = scan(&f.implement);
        let env = FakeEnv::new();
        let _ = run(&f, &items, &env, false);
        let j = env.j.borrow();
        let kinds = j.kinds();
        let intent = kinds.iter().position(|k| *k == "intent");
        let first_remove = kinds.iter().position(|k| *k == "remove");
        let receipt = kinds.iter().position(|k| *k == "receipt");
        let unlock = kinds.iter().position(|k| *k == "unlock");
        assert!(intent.is_some(), "no intent record was written: {kinds:?}");
        assert!(
            intent < first_remove,
            "the intent record did not precede the first removal: {kinds:?}"
        );
        assert!(
            receipt.is_some() && receipt < unlock,
            "the receipt did not precede the lock release: {kinds:?}"
        );
        // And the intent names what was about to go, with its gate evidence.
        let (_, json) = &j.intents[0];
        let v: serde_json::Value = serde_json::from_str(json).unwrap();
        assert_eq!(v["schema"], INTENT_SCHEMA);
        let cands = v["candidates"].as_array().unwrap();
        assert!(cands
            .iter()
            .any(|c| c["path"] == sr::display_path(&f.target)));
        assert!(!cands[0]["gates"].as_array().unwrap().is_empty());
    }

    /// Discriminates a reaper that proceeds when it could not establish the crash record. Without the
    /// intent record a crash mid-reap leaves no evidence of what was in flight.
    #[test]
    fn an_unwritable_intent_record_parks_the_runs_payloads() {
        let f = fx();
        let items = scan(&f.implement);
        let mut env = FakeEnv::new();
        env.intent_error = Some("read-only filesystem".into());
        let r = run(&f, &items, &env, false);
        assert!(matches!(
            parked_reason(&r, &f.target),
            ParkReason::IntentRecordUnavailable { .. }
        ));
        assert!(f.target.exists());
        assert!(env.removed().is_empty());
    }

    /// Discriminates a receipt failure reduced to an in-memory note: payloads were removed and the
    /// durable record was lost, so the COMMAND must fail. A printed note lets an automated caller read
    /// a zero exit status as "reaped and recorded".
    #[test]
    fn a_lost_receipt_is_a_command_failure_not_a_note() {
        let f = fx();
        let items = scan(&f.implement);
        let mut env = FakeEnv::new();
        env.receipt_error = Some("read-only filesystem".into());
        let r = run(&f, &items, &env, false);
        assert!(!env.removed().is_empty(), "nothing was removed to record");
        assert!(
            !r.receipt_failures.is_empty(),
            "a lost receipt did not surface as a failure the command can fail on"
        );
        assert!(r.notes.iter().any(|n| n.contains("NOT written")));
    }

    // -----------------------------------------------------------------------------------------
    // R5 — ergonomics and honest disclosure
    // -----------------------------------------------------------------------------------------

    /// A content fingerprint of a tree: every relative path plus its type. Two runs with the same
    /// fingerprint means nothing was created or removed.
    fn tree_paths(root: &Path) -> Vec<String> {
        let mut out = Vec::new();
        let mut stack = vec![root.to_path_buf()];
        while let Some(p) = stack.pop() {
            let md = std::fs::symlink_metadata(&p).unwrap();
            out.push(p.strip_prefix(root).unwrap().to_string_lossy().into_owned());
            if md.is_dir() && !md.file_type().is_symlink() {
                for e in std::fs::read_dir(&p).unwrap() {
                    stack.push(e.unwrap().path());
                }
            }
        }
        out.sort();
        out
    }

    /// Discriminates the banner claim "NOTHING was touched". Taking each run's operation lock CREATES
    /// `.operation-locks/<id>.lock` when the run has never been resumed or merged — so a dry run does
    /// have one state-visible effect, and it must be declared rather than hidden (the same discipline
    /// S2 applied to its flock probe).
    #[test]
    fn a_dry_run_writes_only_the_operation_lock_namespace() {
        let f = fx();
        let items = scan(&f.implement);
        let before = tree_paths(&f.implement);
        let env = FakeEnv::new();
        let r = run(&f, &items, &env, true);
        let after = tree_paths(&f.implement);
        let added: Vec<&String> = after.iter().filter(|p| !before.contains(p)).collect();
        assert!(
            !added.is_empty(),
            "the fake no longer models the lock write; the disclosure test proves nothing"
        );
        for p in &added {
            assert!(
                p.starts_with(sr::OPERATION_LOCK_DIR),
                "a dry run wrote outside the operation-lock namespace: {p}"
            );
        }
        assert!(
            before.iter().all(|p| after.contains(p)),
            "a dry run removed something"
        );
        assert!(
            !r.dry_run || DRY_RUN_SIDE_EFFECT.contains("operation-locks"),
            "the side effect is not disclosed"
        );
        let text = render_text(&r);
        assert!(
            !text.contains("NOTHING was touched"),
            "the banner still claims nothing was touched"
        );
        assert!(
            text.contains(".operation-locks"),
            "the banner does not name its one state-visible effect: {text}"
        );
    }

    /// Discriminates gate evidence that only reaches `--json`. A dry run exists to be READ before the
    /// operator authorizes a deletion; the reasons must be on the page they are reading.
    #[test]
    fn dry_run_text_output_shows_the_per_item_gate_evidence() {
        let f = fx();
        let items = scan(&f.implement);
        let env = FakeEnv::new();
        let r = run(&f, &items, &env, true);
        let text = render_text(&r);
        assert!(text.contains("operation lock"), "no lock gate line: {text}");
        assert!(text.contains("consumer probe"), "no probe gate line");
        assert!(text.contains("classifier"), "no classifier gate line");
        assert!(text.contains("run owner"), "no run-owner gate line");
    }

    /// Discriminates a per-payload `lsof +D`. `+D` is a full recursive walk; the run directory already
    /// contains every payload, so probing per payload walks the same bytes twice and doubles the
    /// window between probe and delete.
    #[test]
    fn the_consumer_probe_runs_once_per_run_directory() {
        let f = fx();
        let items = scan(&f.implement); // this run has TWO payloads: target + node_modules
        let env = FakeEnv::new();
        let r = run(&f, &items, &env, false);
        assert_eq!(r.count("deleted"), 2, "fixture should offer two payloads");
        let j = env.j.borrow();
        assert_eq!(
            j.probe_witness.len(),
            1,
            "the consumer probe ran {} times for one run: {:?}",
            j.probe_witness.len(),
            j.probe_witness
        );
        assert_eq!(
            j.probe_witness[0].0,
            f.clone.display().to_string(),
            "the probe did not target the run directory"
        );
        assert!(
            !j.progress.is_empty(),
            "a long `+D` walk and per-payload removals emitted no progress at all"
        );
    }

    // -----------------------------------------------------------------------------------------
    // `[storage]` config surface
    // -----------------------------------------------------------------------------------------

    const BASE_CONFIG: &str = "\
default = \"codex\"
[server]
addr = \"127.0.0.1:0\"
[[agents]]
id = \"codex\"
cmd = \"codex\"
";

    /// Discriminates a `[storage]` section whose absence silently disables the D-4 floor. An operator
    /// who never wrote the section must still get the 50 GiB floor the plan mandates.
    #[test]
    fn an_absent_storage_section_still_carries_the_fifty_gib_floor() {
        let cfg = crate::config::RegistryConfig::parse(BASE_CONFIG).unwrap();
        assert_eq!(cfg.storage.admission_floor_gib, DEFAULT_ADMISSION_FLOOR_GIB);
        assert_eq!(cfg.storage.admission_floor_gib, 50);
        assert!(cfg.storage.protected_roots.is_empty());
    }

    /// Discriminates a floor key that is parsed but not honoured, and a protected-roots list that is
    /// dropped on the floor.
    #[test]
    fn the_storage_section_overrides_the_floor_and_declares_protected_roots() {
        let raw = format!(
            "{BASE_CONFIG}[storage]\nadmission_floor_gib = 120\nprotected_roots = [\"/a\", \"/b\"]\n"
        );
        let cfg = crate::config::RegistryConfig::parse(&raw).unwrap();
        assert_eq!(cfg.storage.admission_floor_gib, 120);
        assert_eq!(cfg.storage.protected_roots, vec!["/a", "/b"]);
        assert!(check_admission_floor(Path::new("/x"), 120, Some(100 * GIB)).is_err());
    }

    /// Discriminates a `[storage]` table that silently discards a misspelled key — the exact failure
    /// mode `deny_unknown_fields` exists to prevent, and the one that would quietly leave a floor or a
    /// protected-root list unenforced.
    #[test]
    fn a_misspelled_storage_key_is_refused_rather_than_discarded() {
        let raw = format!("{BASE_CONFIG}[storage]\nadmission_floor_gb = 120\n");
        let e = crate::config::RegistryConfig::parse(&raw)
            .expect_err("a misspelled storage key was accepted");
        assert!(
            format!("{e:?}").contains("admission_floor_gb"),
            "the refusal does not name the offending key: {e:?}"
        );
    }

    // -----------------------------------------------------------------------------------------
    // The `lsof` parse seam (pure)
    // -----------------------------------------------------------------------------------------

    /// Discriminates a probe that reads `lsof`'s exit status. `lsof -t` exits 1 for "found nothing"
    /// AND for a genuine failure, so only the artifact discriminates.
    #[test]
    fn lsof_outcome_reads_the_artifact_never_the_exit_status() {
        assert_eq!(
            lsof_outcome(LsofStatus::Exited(1), "", ""),
            ConsumerProbe::Free
        );
        match lsof_outcome(LsofStatus::Exited(0), "4242\n5151\n", "") {
            ConsumerProbe::Held { detail } => assert!(detail.contains("4242")),
            other => panic!("expected Held, got {other:?}"),
        }
        assert!(matches!(
            lsof_outcome(
                LsofStatus::Exited(1),
                "",
                "lsof: status error on /x: Permission denied"
            ),
            ConsumerProbe::Failed { .. }
        ));
        assert!(
            matches!(
                lsof_outcome(LsofStatus::Exited(0), "COMMAND PID USER\nbash 12 me\n", ""),
                ConsumerProbe::Failed { .. }
            ),
            "output that is not a pid list was treated as an answer"
        );
        assert!(matches!(
            lsof_outcome(LsofStatus::NotSpawned, "", ""),
            ConsumerProbe::Failed { .. }
        ));
    }

    /// R3. Discriminates a probe that reads only `Command::output()`'s `Ok`. A signalled `lsof` is
    /// reaped successfully and yields empty stdout — indistinguishable, by artifact alone, from "the
    /// directory is idle". Its walk was cut short, so its silence is not evidence. Same for any exit
    /// code outside the two `lsof` documents.
    #[test]
    fn lsof_outcome_refuses_a_signalled_or_undocumented_status() {
        match lsof_outcome(LsofStatus::Signaled(9), "", "") {
            ConsumerProbe::Failed { detail } => {
                assert!(detail.contains("signal 9"), "unexpected detail: {detail}")
            }
            other => panic!("a signalled lsof was read as {other:?}"),
        }
        for code in [2, 3, 127, -1] {
            assert!(
                matches!(
                    lsof_outcome(LsofStatus::Exited(code), "", ""),
                    ConsumerProbe::Failed { .. }
                ),
                "exit {code} was treated as an interpretable answer"
            );
        }
        // The two documented statuses stay interpretable.
        assert_eq!(
            lsof_outcome(LsofStatus::Exited(1), "", ""),
            ConsumerProbe::Free
        );
        assert!(matches!(
            lsof_outcome(LsofStatus::Exited(0), "77\n", ""),
            ConsumerProbe::Held { .. }
        ));
    }

    // -----------------------------------------------------------------------------------------
    // D-4 admission floor
    // -----------------------------------------------------------------------------------------

    /// Discriminates a floor that refuses with a bare "not enough disk": the refusal must name the
    /// floor, the observed free space, and the remedy.
    #[test]
    fn admission_below_the_floor_is_refused_with_an_actionable_message() {
        let p = Path::new("/tmp");
        let e = check_admission_floor(p, 50, Some(10 * GIB)).unwrap_err();
        assert_eq!(
            e,
            AdmissionRefusal::BelowFloor {
                path: sr::display_path(p),
                floor_bytes: 50 * GIB,
                free_bytes: 10 * GIB,
            }
        );
        let msg = e.to_string();
        assert!(msg.contains("50.0 GiB"), "floor not named: {msg}");
        assert!(msg.contains("10.0 GiB"), "observed value not named: {msg}");
        assert!(msg.contains("storage report"), "remedy not named: {msg}");
        assert!(msg.contains("storage reap"), "remedy not named: {msg}");
        assert!(
            msg.contains("admission_floor_gib"),
            "override not named: {msg}"
        );
    }

    /// Discriminates an off-by-one floor and a floor that only fires far below its value.
    #[test]
    fn admission_at_or_above_the_floor_is_admitted() {
        let p = Path::new("/tmp");
        assert!(check_admission_floor(p, 50, Some(50 * GIB)).is_ok());
        assert!(check_admission_floor(p, 50, Some(50 * GIB + 1)).is_ok());
        assert!(check_admission_floor(p, 50, Some(50 * GIB - 1)).is_err());
    }

    /// Discriminates a floor that treats an unreadable `statvfs` as "plenty of room". A failed probe
    /// is never evidence of freedom — the same rule the reaper's consumer probe follows.
    #[test]
    fn admission_refuses_an_unmeasurable_volume_and_a_zero_floor_disables_the_check() {
        let p = Path::new("/tmp");
        assert!(matches!(
            check_admission_floor(p, 50, None),
            Err(AdmissionRefusal::Unmeasurable { .. })
        ));
        assert!(
            check_admission_floor(p, 0, None).is_ok(),
            "the documented opt-out (`admission_floor_gib = 0`) does not disable the check"
        );
        assert!(check_admission_floor(p, 0, Some(0)).is_ok());
    }
}
