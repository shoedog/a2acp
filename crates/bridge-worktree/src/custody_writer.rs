//! The R2f1b V3 custody writer (slice 2b2) — the first writer of [`WorktreeCustodyRecordV1`].
//!
//! # Creation ordering, inverted
//!
//! Today's V2 bound path is `git worktree add` → `write_sidecar`: the checkout exists for a
//! window in which nothing on disk says it is owned, and `write_sidecar` itself does no
//! `sync_all` and no parent sync, so a crash can lose the record entirely. §2.5 inverts that:
//!
//! ```text
//! acquire publication cell → acquire custody cell → pin the worktree root by descriptor
//!   → stage + fsync the record → NO-REPLACE publish `ProtectionPrepared` → parent sync
//!   → replace `Materializing` → parent sync
//!     → git worktree add (custody-aware; never `cleanup_failed_add`)
//!   → capture source/root/worktree/common-dir identities BY DESCRIPTOR
//!   → replace `LiveProtected` → parent sync
//! ```
//!
//! Every state after the first is a REPLACE, which is why 2b1's replace primitive had to precede
//! this module.
//!
//! # Production reachability
//!
//! This writer is **production-unreachable** by construction (§5.2, and the slice's §2c
//! self-pass): it runs only when a [`BoundWorktreeCustodyV1`] reaches the backend, which requires
//! a `FrozenR2f1bContractV1` to have been admitted, and no production path constructs one. It
//! lands now so that the ordering, the crash matrix, and the add prohibition are settled before
//! the slice that makes V3 reachable.
//!
//! # Staged-source residue policy (2b2 obligation from the PARKED-1 review, opus S-9)
//!
//! This module is the first production-shaped caller of the `fs_custody` publication primitives,
//! so it owns the answer to "what happens to the staged temp when the outcome is ambiguous".
//!
//! * **Naming.** Every publication stages under `<target>.custody.v1.json.staging-<32 hex>` with
//!   a freshly minted nonce. Two consequences, both load-bearing: a later attempt (retry, next
//!   transition, another process) can never collide with its own or anyone's residue, so the
//!   sequence CONVERGES rather than wedging on `EEXIST`; and the name matches NEITHER sweep
//!   pattern — `is_custody_record_name` requires the exact `.custody.v1.json` ending, and the
//!   legacy scanner requires `.meta.json` — so residue is inert to both arms and can never be
//!   read as a record or selected as a checkout.
//! * **The writer unlinks the staging name in NO arm.** Not on success, not on failure, not on
//!   the staging error path. `std::fs::remove_file` addresses a NAME; what this code holds is a
//!   descriptor. Between `create_new` returning and any later unlink, the name can have been
//!   exchanged (recreated after a rename consumed it, or swapped while we hold the fd), so an
//!   unlink is a deletion of an object whose identity was never proved — the one act this whole
//!   design refuses. The durable arm's unlink was the sharpest case: a committed rename frees the
//!   source name, so the call is a no-op EXCEPT when another actor has since created a file
//!   there, which is precisely when it destroys a foreign object.
//! * **What actually happens to the staged file, arm by arm.** After
//!   [`CustodyPublicationV1::Durable`], `ParentSyncAmbiguous`, and `TargetIdentityUnverified` the
//!   rename committed, so the source NAME is already free and there is no residue — the "leave it
//!   in place" rule is vacuously satisfied for those three. Residue genuinely survives in exactly
//!   two situations: a true `Err` refusal (the ordinary no-replace `EEXIST`, where another owner
//!   published first — §5.7 row 2's "quarantine temp"), and `RenameOutcomeUnverified` where the
//!   source name is occupied by something we cannot prove is ours. In the second, leaving it is
//!   not merely conservative but mandatory: unlinking would destroy a foreign file.
//! * **Reclamation owner — stated truthfully.** It is NOT the boot sweep: `scan_worktree_records`
//!   matches `*.meta.json` and `is_custody_record_name` only, and a staging name satisfies
//!   neither *by design*, so no sweep will ever see one. The owner is the operator, via the
//!   storage report, which classifies residue as `Evidence` with a note naming it (see
//!   `storage_report::WorktreeRecordKindV1::CustodyStagingResidue`) and leaves disposition to the
//!   owner. Residue is bounded (one small file per failed publication), inert, and
//!   self-describing: its name records the exact target it was staged for.
//!
//! # Fault-seam counting (PARKED-1 review, opus S-6)
//!
//! `PinnedDirectoryV1`'s publication-rename countdown is ONE counter shared by publish AND
//! replace on a directory. A crash-matrix test arming "call N" over a
//! publish-then-replace-then-replace sequence must count every rename. The sequence this module
//! performs is exactly: call 1 = `ProtectionPrepared` publish, call 2 = `Materializing` replace,
//! call 3 = the terminal replace (`LiveProtected` / `PreservationUnknown`).

use crate::custody::{
    custody_record_path, ClaimPresenceV1, IdentityCompletenessV1, PreservationReasonV1,
    PreservedWorktreeClaimV1, RecoveryLocatorV1, WorktreeCustodyRecordV1, WorktreeCustodyStateV1,
    CUSTODY_RECORD_SUFFIX, WORKTREE_CUSTODY_RECORD_SCHEMA_V1,
};
use crate::custody_lock::{
    acquire_custody_lock_blocking_in, acquire_publication_lock_blocking_in, CustodyLockGuardV1,
    CustodyLockRefusalV1, PublicationLockGuardV1,
};
use bridge_core::execution_policy::{
    BoundWorktreeCustodyV1, WorktreeCustodyIdV1, WorktreeObjectIdentityV1,
};
use bridge_core::fs_custody::{
    open_options_create_new_owner_private, CustodyPublicationV1, DirectoryIdentityV1,
    FsCustodyError, PinnedDirectoryV1, RegularChildRefV1,
};
use std::ffi::{OsStr, OsString};
use std::io::Write as _;
use std::path::{Path, PathBuf};

/// Why a custody transition could not be completed.
///
/// [`Self::Ambiguous`] is separate from [`Self::Failed`] on purpose: an ambiguous publication may
/// have taken effect, so the record on disk may already be the new state. A caller must not treat
/// it as "nothing happened" — in particular it must not conclude the checkout is unprotected.
#[derive(Debug, thiserror::Error)]
pub enum CustodyWriteRefusalV1 {
    #[error("custody cell could not be entered: {0}")]
    Lock(#[from] CustodyLockRefusalV1),
    #[error("custody record could not be built: {0}")]
    Record(#[from] crate::custody::CustodyRecordDecodeErrorV1),
    #[error("custody publication failed before any effect: {0}")]
    Failed(String),
    /// The publication may or may not have taken effect. Protective: unknown never licenses
    /// deletion, and never licenses believing the checkout is unprotected either.
    #[error("custody publication outcome is ambiguous: {0}")]
    Ambiguous(String),
}

impl From<FsCustodyError> for CustodyWriteRefusalV1 {
    fn from(error: FsCustodyError) -> Self {
        Self::Failed(error.to_string())
    }
}

/// The four object identities §2.2's claim records, captured by descriptor at materialization.
///
/// Risk R-8's accepted disposition: `FrozenWorktreeCustodyPlanV1` carries only
/// `{custody_id, checkout_fingerprint, target_cwd}`, so three of the four identities do not exist
/// in the contract at all and must be observed when the objects do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MaterializedIdentitiesV1 {
    pub source: WorktreeObjectIdentityV1,
    pub root: WorktreeObjectIdentityV1,
    pub worktree: WorktreeObjectIdentityV1,
    pub common_dir: WorktreeObjectIdentityV1,
}

/// A plan-derived identity: the canonical path with no observed `dev`/`ino`.
///
/// Legal only where §2.2 says no live identity exists yet — `ProtectionPrepared`,
/// `UnusedSettled`, `Materializing`, and `PreservationUnknown{materialization_inflight}`. The
/// decoder enforces that rule ([`IdentityCompletenessV1`]); this function just produces the shape.
#[must_use]
pub fn planned_identity(path: &str) -> WorktreeObjectIdentityV1 {
    WorktreeObjectIdentityV1 {
        canonical_path: path.to_string(),
        directory_identity: DirectoryIdentityV1 {
            canonical_path: path.to_string(),
            dev: None,
            ino: None,
        },
    }
}

/// Observe a directory's identity BY DESCRIPTOR (`O_NOFOLLOW`), never by re-canonicalizing a
/// string (§2.2). Falls back to the degraded shape when the object cannot be opened, so a claim
/// can still be published for a checkout whose objects have already moved.
#[must_use]
pub fn observed_identity(path: &str) -> WorktreeObjectIdentityV1 {
    let Ok(file) = bridge_core::fs_custody::open_directory_no_follow_raw(Path::new(path)) else {
        return planned_identity(path);
    };
    let Ok(metadata) = file.metadata() else {
        return planned_identity(path);
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt as _;
        WorktreeObjectIdentityV1 {
            canonical_path: path.to_string(),
            directory_identity: DirectoryIdentityV1 {
                canonical_path: path.to_string(),
                dev: Some(metadata.dev()),
                ino: Some(metadata.ino()),
            },
        }
    }
    #[cfg(not(unix))]
    {
        let _ = metadata;
        planned_identity(path)
    }
}

/// A held custody cell over one checkout, with its worktree root pinned by descriptor.
///
/// The guards are fields so their lifetime is exactly this value's: every transition published
/// through a `WorktreeCustodianV1` is serialized against every sweep, deletion, and other writer
/// for the same checkout, and the cells release when the custodian is dropped.
pub struct WorktreeCustodianV1 {
    custody: BoundWorktreeCustodyV1,
    worktree_path: String,
    record_name: OsString,
    root: PinnedDirectoryV1,
    root_path: PathBuf,
    // Declared in acquisition order; Rust drops fields in declaration order, so the publication
    // cell (outer) releases AFTER the custody cell (inner) — the reverse-of-acquisition order a
    // nested lock discipline requires.
    _custody_cell: CustodyLockGuardV1,
    _publication_cell: PublicationLockGuardV1,
}

impl std::fmt::Debug for WorktreeCustodianV1 {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("WorktreeCustodianV1")
            .field("worktree_path", &self.worktree_path)
            .field("custody_id", &self.custody.plan.custody_id.as_str())
            .finish()
    }
}

impl WorktreeCustodianV1 {
    /// Enter both cells and pin the worktree root. **Blocking** — the writer is the one caller
    /// entitled to the blocking acquirers (`custody_lock.rs`, contention), and every call site
    /// must therefore be off the async executor.
    ///
    /// `worktree_root` is the canonical `[worktrees].root`; `worktree_path` is the frozen target,
    /// whose parent must be that root (already enforced by `validate_bound_worktree`).
    pub fn enter(
        worktree_root: &Path,
        worktree_path: &str,
        custody: BoundWorktreeCustodyV1,
    ) -> Result<Self, CustodyWriteRefusalV1> {
        let publication_cell =
            acquire_publication_lock_blocking_in(worktree_root, worktree_path, &|| {
                tracing::info!(
                    worktree_path,
                    "waiting for the checkout publication cell before a custody transition"
                );
            })?;
        let custody_cell =
            acquire_custody_lock_blocking_in(worktree_root, &custody.plan.custody_id, &|| {
                tracing::info!(
                    custody_id = custody.plan.custody_id.as_str(),
                    "waiting for the custody cell before a custody transition"
                );
            })?;
        let root = PinnedDirectoryV1::open(worktree_root, "worktree custody root")?;
        let record_name = record_file_name(worktree_path)?;
        Ok(Self {
            custody,
            worktree_path: worktree_path.to_string(),
            record_name,
            root,
            root_path: worktree_root.to_path_buf(),
            _custody_cell: custody_cell,
            _publication_cell: publication_cell,
        })
    }

    #[must_use]
    pub fn custody_id(&self) -> &WorktreeCustodyIdV1 {
        &self.custody.plan.custody_id
    }

    #[must_use]
    pub fn worktree_path(&self) -> &str {
        &self.worktree_path
    }

    #[must_use]
    pub fn worktree_root(&self) -> &Path {
        &self.root_path
    }

    /// The pinned root, exposed so a test can arm `fs_custody`'s fault seams against the very
    /// descriptor the writer publishes through. There is no other way to inject a rename or sync
    /// fault at the exact call the crash matrix names.
    #[must_use]
    pub fn pinned_root(&self) -> &PinnedDirectoryV1 {
        &self.root
    }

    /// Publish `ProtectionPrepared` with the NO-REPLACE primitive, before any provider effect.
    ///
    /// No-replace is the right primitive exactly once, here: an existing record at this name is
    /// another owner's protection, and clobbering it would silently take custody of a checkout
    /// someone else is protecting. Every later transition replaces, because by then the record at
    /// that name is this custodian's own.
    pub fn publish_protection_prepared(&self) -> Result<(), CustodyWriteRefusalV1> {
        let record = self.record(WorktreeCustodyStateV1::ProtectionPrepared {}, None, None)?;
        self.stage_and_settle(&record, PublicationModeV1::NoReplace)
    }

    /// Replace with `Materializing` — the state `git worktree add` runs under (§2.5).
    pub fn replace_materializing(&self) -> Result<(), CustodyWriteRefusalV1> {
        let record = self.record(WorktreeCustodyStateV1::Materializing {}, None, None)?;
        self.stage_and_settle(&record, PublicationModeV1::Replace)
    }

    /// Replace with `LiveProtected`, carrying the observed target identity the sweep compares by
    /// descriptor.
    pub fn replace_live_protected(
        &self,
        identities: &MaterializedIdentitiesV1,
    ) -> Result<(), CustodyWriteRefusalV1> {
        let record = self.record(
            WorktreeCustodyStateV1::LiveProtected {},
            Some(identities.worktree.clone()),
            None,
        )?;
        self.stage_and_settle(&record, PublicationModeV1::Replace)
    }

    /// Replace with `PreservationUnknown { reason }` and its required claim.
    ///
    /// The only preservation transition this slice performs, and it is the one the add path
    /// mandates: §5.7 row 4 ("during/after partial add, before live identity → report
    /// preservation unknown; never delete target") and §5.1 ("if materialization is unresolved,
    /// publish `PreservationUnknown{materialization_inflight}`"). Failure/cancel preservation —
    /// `LiveProtected → PreservationPrepared → Preserved` — is 2c1's and is NOT implemented here.
    pub fn replace_preservation_unknown(
        &self,
        reason: PreservationReasonV1,
        identities: &MaterializedIdentitiesV1,
        recovery_locator: RecoveryLocatorV1,
        created_wall_ms: i64,
    ) -> Result<(), CustodyWriteRefusalV1> {
        let state = WorktreeCustodyStateV1::PreservationUnknown { reason };
        let claim = PreservedWorktreeClaimV1 {
            schema_version: WORKTREE_CUSTODY_RECORD_SCHEMA_V1,
            custody_id: self.custody.plan.custody_id.clone(),
            execution_id: self.custody.attempt.execution_id.clone(),
            origin_attempt_id: self.custody.origin_attempt_id.clone(),
            current_attempt: self.custody.attempt.clone(),
            node: self.custody.node.clone(),
            checkout_fingerprint: self.custody.plan.checkout_fingerprint.clone(),
            source: identities.source.clone(),
            root: identities.root.clone(),
            worktree: identities.worktree.clone(),
            common_dir: identities.common_dir.clone(),
            reason,
            created_wall_ms,
            recovery_locator,
        };
        let record = self.record(state, Some(identities.worktree.clone()), Some(claim))?;
        self.stage_and_settle(&record, PublicationModeV1::Replace)
    }

    fn record(
        &self,
        state: WorktreeCustodyStateV1,
        worktree: Option<WorktreeObjectIdentityV1>,
        claim: Option<PreservedWorktreeClaimV1>,
    ) -> Result<WorktreeCustodyRecordV1, CustodyWriteRefusalV1> {
        // Degraded-vs-observed is decided by the STATE's own settled rule, not by what happens to
        // be observable: 2a published `identity_completeness` as data precisely so the writer,
        // the reader, and 2c/2d cannot each re-derive it.
        let worktree = worktree.unwrap_or_else(|| match state.identity_completeness() {
            IdentityCompletenessV1::MayBeDegraded => {
                planned_identity(self.custody.plan.target_cwd.as_str())
            }
            IdentityCompletenessV1::Complete => {
                observed_identity(self.custody.plan.target_cwd.as_str())
            }
        });
        debug_assert!(
            (state.claim_presence() == ClaimPresenceV1::Required) == claim.is_some()
                || state.claim_presence() == ClaimPresenceV1::Optional,
            "a state's claim rule is 2a's data, not the writer's choice"
        );
        let record = WorktreeCustodyRecordV1 {
            schema_version: WORKTREE_CUSTODY_RECORD_SCHEMA_V1,
            custody_id: self.custody.plan.custody_id.clone(),
            checkout_fingerprint: self.custody.plan.checkout_fingerprint.clone(),
            current_attempt: self.custody.attempt.clone(),
            worktree,
            state,
            claim,
        };
        record.validate_for_publication()?;
        Ok(record)
    }

    /// Stage the encoded record as a fresh, nonce-named private file, fsync IT, then publish or
    /// replace under the pinned root and parent-sync.
    fn stage_and_settle(
        &self,
        record: &WorktreeCustodyRecordV1,
        mode: PublicationModeV1,
    ) -> Result<(), CustodyWriteRefusalV1> {
        let bytes = record.encode_canonical()?;
        let staged_name = staged_record_name(&self.record_name)?;
        let staged_path = self.root_path.join(&staged_name);
        let mut file = open_options_create_new_owner_private()
            .open(&staged_path)
            .map_err(|error| {
                CustodyWriteRefusalV1::Failed(format!(
                    "custody record could not be staged: {error}"
                ))
            })?;
        let staged = (|| -> Result<(), CustodyWriteRefusalV1> {
            file.write_all(&bytes).map_err(|error| {
                CustodyWriteRefusalV1::Failed(format!(
                    "custody record could not be written: {error}"
                ))
            })?;
            // The publication primitives sync the DIRECTORY; syncing the file's own bytes is the
            // documented caller obligation, and skipping it would publish a name that can survive
            // a crash pointing at empty content.
            file.sync_all().map_err(|error| {
                CustodyWriteRefusalV1::Failed(format!(
                    "custody record could not be synced: {error}"
                ))
            })
        })();
        if let Err(error) = staged {
            // No unlink here either. The tempting argument — "nothing was published, so the
            // staged object is provably ours" — is FALSE: what we hold is a descriptor, and the
            // NAME can have been exchanged since `create_new` returned. `remove_file` addresses
            // the name, not our descriptor, so it would delete whatever now occupies it.
            self.quarantine_residue(&staged_path, &error.to_string());
            return Err(error);
        }

        let source = RegularChildRefV1::new(OsStr::new(&staged_name), &file);
        let published = match mode {
            PublicationModeV1::NoReplace => self.root.publish_new_regular_child(
                source,
                &self.record_name,
                "worktree custody record",
            ),
            PublicationModeV1::Replace => self.root.replace_regular_child(
                source,
                &self.record_name,
                "worktree custody record",
            ),
        };
        match published {
            // A true `Err` PROVES the rename did not happen (for a no-replace publish that
            // includes the ordinary `EEXIST`, where another owner published first). The staged
            // object is provably still ours — but §5.7 row 2 says "quarantine temp", so it is
            // left in place, not unlinked: an unreferenced, inert, named artifact is a better
            // recovery signal than a silent deletion, and the residue naming rules make it
            // harmless.
            Err(error) => {
                self.quarantine_residue(&staged_path, &error.to_string());
                Err(error.into())
            }
            Ok(outcome) => {
                self.settle_residue(&staged_path, &outcome);
                match outcome.ambiguity() {
                    None => Ok(()),
                    Some(detail) => Err(CustodyWriteRefusalV1::Ambiguous(detail.to_string())),
                }
            }
        }
    }

    /// The staged-source residue policy, in one place. See the module docs.
    ///
    /// **This function unlinks nothing, in any arm, and that is the whole rule.** The durable arm
    /// used to `remove_file(staged_path)` on the reasoning that "a committed rename frees the
    /// source name, so the unlink is a harmless no-op". That reasoning is exactly backwards: if
    /// the name is free the call does nothing, and the ONLY circumstance in which it does
    /// anything is the one where another actor has created a file at that name since the rename —
    /// i.e. it deletes a foreign object, and one whose identity was never checked. Pinned by
    /// `a_durable_publication_never_unlinks_the_staging_pathname`.
    fn settle_residue(&self, staged_path: &Path, outcome: &CustodyPublicationV1) {
        if outcome.is_durable() {
            // The rename consumed the source name. Nothing to do — and nothing we are entitled
            // to do to whatever may occupy that name now.
            return;
        }
        self.quarantine_residue(staged_path, outcome.ambiguity().unwrap_or_default());
    }

    fn quarantine_residue(&self, staged_path: &Path, detail: &str) {
        tracing::warn!(
            worktree_path = self.worktree_path,
            staged = %staged_path.display(),
            detail,
            "quarantining a staged custody record: unlinking an object whose identity or effect \
             is unproven is a destructive act on unknown evidence. The residue matches neither \
             sweep pattern, carries a unique nonce so no later attempt collides with it, and is \
             surfaced by the storage report for owner disposition."
        );
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PublicationModeV1 {
    NoReplace,
    Replace,
}

fn record_file_name(worktree_path: &str) -> Result<OsString, CustodyWriteRefusalV1> {
    Path::new(&custody_record_path(worktree_path))
        .file_name()
        .map(OsStr::to_os_string)
        .ok_or_else(|| {
            CustodyWriteRefusalV1::Failed(format!(
                "worktree target has no file name: {worktree_path}"
            ))
        })
}

/// `<record name>.staging-<32 hex>`.
///
/// The nonce is what makes a retry converge instead of colliding with its own residue, and the
/// suffix ORDER matters: appending after `.custody.v1.json` means `is_custody_record_name` (which
/// requires that exact ending) rejects it, so no scan can read residue as a record.
fn staged_record_name(record_name: &OsStr) -> Result<OsString, CustodyWriteRefusalV1> {
    let nonce = WorktreeCustodyIdV1::mint()
        .map_err(|error| CustodyWriteRefusalV1::Failed(format!("staging nonce: {error:?}")))?;
    let mut name = record_name.to_os_string();
    // The typed id is `"custody-" + 64 lowercase hex`; take the HEX, not the prefix, so the
    // residue suffix is a pure hex token `is_staged_custody_residue` can recognise.
    let hex = &nonce.as_str()[WorktreeCustodyIdV1::PREFIX.len()..][..STAGING_NONCE_HEX_LEN];
    name.push(format!(".staging-{hex}"));
    Ok(name)
}

/// The staging nonce's exact length, in hex characters.
pub const STAGING_NONCE_HEX_LEN: usize = 32;

/// Is `name` a staged custody-publication residue?
///
/// Recognition is EXACT — `<non-empty stem>.custody.v1.json.staging-` followed by exactly
/// [`STAGING_NONCE_HEX_LEN`] LOWERCASE hex characters and nothing else. Loose recognition is not a
/// harmless nicety: the storage report classifies whatever this accepts as bridge-owned
/// `Evidence`, so an over-permissive predicate would label an operator's own file beside a
/// checkout as bridge residue and offer it for disposition on that basis.
#[must_use]
pub fn is_staged_custody_residue(name: &str) -> bool {
    let base = name.rsplit_once('/').map_or(name, |(_, base)| base);
    let marker = const_format_staging_marker();
    let Some(index) = base.rfind(&marker) else {
        return false;
    };
    // A non-empty target stem is required, exactly as `is_custody_record_name` requires one:
    // `.custody.v1.json.staging-<hex>` naming no target is not this writer's artifact.
    if index == 0 {
        return false;
    }
    let nonce = &base[index + marker.len()..];
    nonce.len() == STAGING_NONCE_HEX_LEN
        && nonce
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn const_format_staging_marker() -> String {
    format!("{CUSTODY_RECORD_SUFFIX}.staging-")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::custody::{read_custody_record_in, WorktreeCustodyStateKindV1};
    use bridge_core::execution_policy::{
        FrozenWorktreeCustodyPlanV1, PolicyNodeRefV1, Sha256HexV1,
    };
    use bridge_core::fs_custody::PublicationRenameFaultV1;
    use bridge_core::ids::{AttemptId, AttemptIdentity, ExecutionId};
    use bridge_core::SessionCwd;
    use std::ffi::OsString;

    fn root(name: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "a2a-bridge-custody-writer-{name}-{}-{nanos}",
            std::process::id()
        ));
        std::fs::create_dir_all(&path).unwrap();
        std::fs::canonicalize(&path).unwrap()
    }

    fn binding(target: &Path) -> BoundWorktreeCustodyV1 {
        let attempt_id = AttemptId::parse(format!("attempt-{}", "2".repeat(32))).unwrap();
        BoundWorktreeCustodyV1 {
            attempt: AttemptIdentity {
                execution_id: ExecutionId::parse(format!("exec-{}", "1".repeat(32))).unwrap(),
                attempt_id: attempt_id.clone(),
                ordinal: 0,
                parent_attempt_id: None,
            },
            origin_attempt_id: attempt_id,
            node: PolicyNodeRefV1::from_node_id(0, "node"),
            plan: FrozenWorktreeCustodyPlanV1 {
                custody_id: WorktreeCustodyIdV1::mint().unwrap(),
                checkout_fingerprint: Sha256HexV1::parse("6".repeat(64)).unwrap(),
                target_cwd: SessionCwd::parse(&target.to_string_lossy()).unwrap(),
            },
        }
    }

    fn identities(target: &Path) -> MaterializedIdentitiesV1 {
        let path = target.to_string_lossy().into_owned();
        MaterializedIdentitiesV1 {
            source: observed_identity(&path),
            root: observed_identity(&target.parent().unwrap().to_string_lossy()),
            worktree: observed_identity(&path),
            common_dir: planned_identity("/src/.git"),
        }
    }

    fn record_state(worktree_root: &Path, target: &Path) -> Option<WorktreeCustodyStateKindV1> {
        let pinned = PinnedDirectoryV1::open(worktree_root, "test").ok()?;
        let name = Path::new(&custody_record_path(&target.to_string_lossy()))
            .file_name()?
            .to_os_string();
        read_custody_record_in(&pinned, &name)
            .ok()
            .map(|record| record.state.kind())
    }

    fn residue(worktree_root: &Path) -> Vec<String> {
        std::fs::read_dir(worktree_root)
            .unwrap()
            .flatten()
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| is_staged_custody_residue(name))
            .collect()
    }

    /// §5.7 row 3, and the ordering the whole slice rests on: the record is durable, in
    /// `ProtectionPrepared`, before any provider effect could run. Discriminates a writer that
    /// stages the record but never publishes it, and one that publishes a state the sweep would
    /// not protect.
    #[test]
    fn protection_prepared_is_published_and_readable_before_any_provider_effect() {
        let worktree_root = root("prepared");
        let target = worktree_root.join("ownr-run7-abc");

        let custodian =
            WorktreeCustodianV1::enter(&worktree_root, &target.to_string_lossy(), binding(&target))
                .unwrap();
        custodian.publish_protection_prepared().unwrap();

        assert_eq!(
            record_state(&worktree_root, &target),
            Some(WorktreeCustodyStateKindV1::ProtectionPrepared)
        );
        assert!(
            !target.exists(),
            "the checkout must not exist yet: the record precedes `git worktree add`"
        );
        assert!(
            !custodian
                .pinned_root()
                .canonical_path()
                .join("ownr-run7-abc.meta.json")
                .exists(),
            "the V3 path must never emit the legacy sidecar name"
        );
        assert!(
            residue(&worktree_root).is_empty(),
            "a durable publish clears its own staging"
        );
        drop(custodian);
        std::fs::remove_dir_all(&worktree_root).unwrap();
    }

    /// §5.7 row 1 + row 2, together: an already-published record at the target name refuses the
    /// no-replace publish (another owner got there first), the FINAL record is untouched, and the
    /// staged temp is quarantined rather than silently deleted.
    ///
    /// Discriminates a writer that uses the REPLACING primitive for the first publication — which
    /// would clobber another owner's protection and silently take custody of their checkout.
    #[test]
    fn a_foreign_record_refuses_the_first_publication_and_the_temp_is_quarantined() {
        let worktree_root = root("foreign-record");
        let target = worktree_root.join("ownr-run7-abc");
        let first =
            WorktreeCustodianV1::enter(&worktree_root, &target.to_string_lossy(), binding(&target))
                .unwrap();
        first.publish_protection_prepared().unwrap();
        let owner = std::fs::read(custody_record_path(&target.to_string_lossy())).unwrap();
        drop(first);

        let second =
            WorktreeCustodianV1::enter(&worktree_root, &target.to_string_lossy(), binding(&target))
                .unwrap();
        let refused = second.publish_protection_prepared();

        assert!(
            matches!(refused, Err(CustodyWriteRefusalV1::Failed(_))),
            "a taken record name is a provable refusal, never an ambiguity: {refused:?}"
        );
        assert_eq!(
            std::fs::read(custody_record_path(&target.to_string_lossy())).unwrap(),
            owner,
            "the first owner's record must be byte-identical afterwards"
        );
        assert_eq!(
            residue(&worktree_root).len(),
            1,
            "the staged temp is quarantined (§5.7 row 2), not unlinked"
        );
        drop(second);
        std::fs::remove_dir_all(&worktree_root).unwrap();
    }

    /// The staged residue is invisible to BOTH sweep patterns. Discriminates a staging name built
    /// as `<nonce>.custody.v1.json` or `<target>.meta.json.tmp`: the first would be scanned as an
    /// undecodable record on every boot, the second handed to the legacy arm, which deletes.
    #[test]
    fn staged_residue_matches_neither_sweep_pattern_and_never_collides_with_itself() {
        let worktree_root = root("residue-naming");
        let target = worktree_root.join("ownr-run7-abc");
        let name = record_file_name(&target.to_string_lossy()).unwrap();

        let first = staged_record_name(&name).unwrap();
        let second = staged_record_name(&name).unwrap();

        assert_ne!(
            first, second,
            "a fresh nonce is what makes a retry converge"
        );
        for staged in [&first, &second] {
            let staged = staged.to_string_lossy().into_owned();
            assert!(is_staged_custody_residue(&staged));
            assert!(
                !crate::custody::is_custody_record_name(&staged),
                "residue must not be selectable by the V3 scan: {staged}"
            );
            assert!(
                !staged.ends_with(".meta.json"),
                "residue must not be selectable by the legacy scan: {staged}"
            );
            assert_eq!(
                Path::new(&staged).components().count(),
                1,
                "the staging name must stay a single path component"
            );
        }
        std::fs::remove_dir_all(&worktree_root).unwrap();
    }

    /// The full ordered sequence, and the state the sweep sees at each step. Discriminates a
    /// writer that skips `Materializing` (so a crash during the add is indistinguishable from a
    /// crash before it) and one that publishes an illegal edge.
    #[test]
    fn the_transition_sequence_is_prepared_then_materializing_then_live_protected() {
        let worktree_root = root("sequence");
        let target = worktree_root.join("ownr-run7-abc");
        let custodian =
            WorktreeCustodianV1::enter(&worktree_root, &target.to_string_lossy(), binding(&target))
                .unwrap();

        custodian.publish_protection_prepared().unwrap();
        assert_eq!(
            record_state(&worktree_root, &target),
            Some(WorktreeCustodyStateKindV1::ProtectionPrepared)
        );
        custodian.replace_materializing().unwrap();
        assert_eq!(
            record_state(&worktree_root, &target),
            Some(WorktreeCustodyStateKindV1::Materializing)
        );
        std::fs::create_dir_all(&target).unwrap();
        custodian
            .replace_live_protected(&identities(&target))
            .unwrap();
        assert_eq!(
            record_state(&worktree_root, &target),
            Some(WorktreeCustodyStateKindV1::LiveProtected)
        );

        for (from, to) in [
            (
                WorktreeCustodyStateKindV1::ProtectionPrepared,
                WorktreeCustodyStateKindV1::Materializing,
            ),
            (
                WorktreeCustodyStateKindV1::Materializing,
                WorktreeCustodyStateKindV1::LiveProtected,
            ),
        ] {
            assert!(
                crate::custody::transition_is_legal(from, to),
                "the writer must only publish edges 2a froze as legal: {from:?} -> {to:?}"
            );
        }
        assert!(residue(&worktree_root).is_empty());
        drop(custodian);
        std::fs::remove_dir_all(&worktree_root).unwrap();
    }

    /// §5.7 row 5's shape at the writer: a rename that took effect but whose parent sync did not
    /// complete is reported AMBIGUOUS, never as success and never as "nothing happened".
    /// Discriminates a writer that unwraps the publication outcome, or that maps every non-
    /// `Durable` arm to a plain failure the caller would read as "the record was not written".
    ///
    /// RENAMED in the 2b2 repair round (opus W-4a): this test never asserted "the residue is
    /// kept", and could not — the rename COMMITTED here, so the source name is already free and
    /// the residue assertion below is `== 0`. The residue-kept rule is pinned by
    /// `a_foreign_record_refuses_the_first_publication_and_the_temp_is_quarantined` (the `Err`
    /// arm) and `a_durable_publication_never_unlinks_the_staging_pathname` (the identity hazard).
    #[test]
    fn an_unsynced_parent_is_reported_ambiguous_with_the_record_already_durable() {
        let worktree_root = root("parent-sync");
        let target = worktree_root.join("ownr-run7-abc");
        let custodian =
            WorktreeCustodianV1::enter(&worktree_root, &target.to_string_lossy(), binding(&target))
                .unwrap();
        custodian.pinned_root().fail_sync_on_nth_call_for_test(1);

        let outcome = custodian.publish_protection_prepared();

        assert!(
            matches!(outcome, Err(CustodyWriteRefusalV1::Ambiguous(_))),
            "unexpected outcome: {outcome:?}"
        );
        assert_eq!(
            record_state(&worktree_root, &target),
            Some(WorktreeCustodyStateKindV1::ProtectionPrepared),
            "the rename DID take effect; the record is on disk and protective"
        );
        assert_eq!(
            residue(&worktree_root).len(),
            0,
            "the source name is free after a committed rename, so nothing was left to keep"
        );
        drop(custodian);
        std::fs::remove_dir_all(&worktree_root).unwrap();
    }

    /// The PARKED-1 review's fault-counting obligation (opus S-6), pinned as executable evidence:
    /// `PinnedDirectoryV1`'s publication-rename countdown is ONE counter shared by publish AND
    /// replace, so "call 3" in a publish→replace→replace sequence is the THIRD rename, not the
    /// third replace. Discriminates a crash-matrix test that counts only replaces — it would arm
    /// a fault that lands on a different transition than its own name claims.
    #[test]
    fn the_publication_fault_countdown_counts_publishes_and_replaces_together() {
        let worktree_root = root("fault-counting");
        let target = worktree_root.join("ownr-run7-abc");
        let custodian =
            WorktreeCustodianV1::enter(&worktree_root, &target.to_string_lossy(), binding(&target))
                .unwrap();
        custodian
            .pinned_root()
            .fail_publication_rename_on_nth_call_for_test(
                3,
                PublicationRenameFaultV1::BeforeEffect,
            );

        custodian
            .publish_protection_prepared()
            .expect("rename call 1 is not the armed one");
        custodian
            .replace_materializing()
            .expect("rename call 2 is not the armed one");
        std::fs::create_dir_all(&target).unwrap();
        let third = custodian.replace_live_protected(&identities(&target));

        assert!(
            third.is_err(),
            "the third RENAME must be the armed one, counting the publish"
        );
        assert_eq!(
            record_state(&worktree_root, &target),
            Some(WorktreeCustodyStateKindV1::Materializing),
            "a before-effect fault leaves the previous state authoritative"
        );
        drop(custodian);
        std::fs::remove_dir_all(&worktree_root).unwrap();
    }

    /// An error-after-effect rename (a network filesystem's retried RPC) must NOT be reported as
    /// "nothing happened". Discriminates a writer built on the raw errno instead of the
    /// identity-classified outcome the PARKED-1 fix introduced: the replacement really landed, so
    /// treating it as un-happened would leave the caller believing a stale state is current.
    #[test]
    fn an_error_after_effect_rename_is_reported_durable_not_failed() {
        let worktree_root = root("after-effect");
        let target = worktree_root.join("ownr-run7-abc");
        let custodian =
            WorktreeCustodianV1::enter(&worktree_root, &target.to_string_lossy(), binding(&target))
                .unwrap();
        custodian.publish_protection_prepared().unwrap();
        custodian
            .pinned_root()
            .fail_publication_rename_on_nth_call_for_test(1, PublicationRenameFaultV1::AfterEffect);

        custodian
            .replace_materializing()
            .expect("a rename that took effect is durable, whatever the syscall reported");

        assert_eq!(
            record_state(&worktree_root, &target),
            Some(WorktreeCustodyStateKindV1::Materializing)
        );
        drop(custodian);
        std::fs::remove_dir_all(&worktree_root).unwrap();
    }

    /// An undecidable rename is protective: the caller is told the outcome is unverified rather
    /// than being handed either "it landed" or "nothing happened".
    ///
    /// RENAMED in the 2b2 repair round (opus W-4a): the old name claimed this kept its residue,
    /// which it cannot — the injected `UnlinkSourceOnly` fault removes the source by definition,
    /// so there is nothing left to keep. What this test actually discriminates is a writer that
    /// collapses `RenameOutcomeUnverified` into either a clean `Err` (caller concludes the record
    /// was not written, and may keep treating a superseded state as current) or an `Ok`.
    #[test]
    fn an_undecidable_rename_refuses_ambiguously_without_claiming_an_effect() {
        let worktree_root = root("undecidable");
        let target = worktree_root.join("ownr-run7-abc");
        let custodian =
            WorktreeCustodianV1::enter(&worktree_root, &target.to_string_lossy(), binding(&target))
                .unwrap();
        custodian
            .pinned_root()
            .fail_publication_rename_on_nth_call_for_test(
                1,
                PublicationRenameFaultV1::UnlinkSourceOnly,
            );

        let outcome = custodian.publish_protection_prepared();

        assert!(
            matches!(outcome, Err(CustodyWriteRefusalV1::Ambiguous(_))),
            "an unverifiable rename must be ambiguous, not a clean failure: {outcome:?}"
        );
        drop(custodian);
        std::fs::remove_dir_all(&worktree_root).unwrap();
    }

    /// The custody cells really are held for the custodian's lifetime, and in the declared order.
    /// Discriminates a writer that takes the cells and drops them before publishing (leaving the
    /// probe→publish window the S7 gate wiring exists to close) and a field order whose `Drop`
    /// releases the outer cell first.
    #[test]
    fn both_cells_are_held_for_the_custodians_lifetime_and_released_on_drop() {
        let worktree_root = root("cells");
        let target = worktree_root.join("ownr-run7-abc");
        let held = binding(&target);
        let custodian =
            WorktreeCustodianV1::enter(&worktree_root, &target.to_string_lossy(), held.clone())
                .unwrap();

        assert!(
            crate::custody_lock::try_acquire_publication_lock_in(
                &worktree_root,
                &target.to_string_lossy()
            )
            .is_err(),
            "the publication cell must be held while a custodian exists"
        );
        assert!(
            crate::custody_lock::try_acquire_custody_lock_in(&worktree_root, &held.plan.custody_id)
                .is_err(),
            "the custody cell must be held while a custodian exists"
        );

        drop(custodian);
        assert!(crate::custody_lock::try_acquire_publication_lock_in(
            &worktree_root,
            &target.to_string_lossy()
        )
        .is_ok());
        assert!(crate::custody_lock::try_acquire_custody_lock_in(
            &worktree_root,
            &held.plan.custody_id
        )
        .is_ok());
        std::fs::remove_dir_all(&worktree_root).unwrap();
    }

    /// `PreservationUnknown` carries the claim its own state rule REQUIRES, with the recovery
    /// locator the provider probe produced. Discriminates a writer that publishes the state with
    /// no claim (the decoder would refuse, so this would be a hard failure rather than a silent
    /// one) and one that hardcodes a locator instead of carrying the probe's answer — which is
    /// how `RegistrationUnproven` becomes unreachable in production.
    #[test]
    fn preservation_unknown_carries_its_required_claim_and_the_probed_locator() {
        let worktree_root = root("preservation-unknown");
        let target = worktree_root.join("ownr-run7-abc");
        std::fs::create_dir_all(&target).unwrap();
        let custodian =
            WorktreeCustodianV1::enter(&worktree_root, &target.to_string_lossy(), binding(&target))
                .unwrap();
        custodian.publish_protection_prepared().unwrap();
        custodian.replace_materializing().unwrap();

        custodian
            .replace_preservation_unknown(
                PreservationReasonV1::MaterializationInFlight,
                &identities(&target),
                RecoveryLocatorV1::RegistrationUnproven {},
                1_700_000_000_000,
            )
            .unwrap();

        let pinned = PinnedDirectoryV1::open(&worktree_root, "test").unwrap();
        let name: OsString = Path::new(&custody_record_path(&target.to_string_lossy()))
            .file_name()
            .unwrap()
            .to_os_string();
        let record = read_custody_record_in(&pinned, &name).unwrap();
        let claim = record.claim.clone().expect("this state requires a claim");
        assert_eq!(claim.reason, PreservationReasonV1::MaterializationInFlight);
        assert_eq!(
            claim.recovery_locator,
            RecoveryLocatorV1::RegistrationUnproven {}
        );
        assert_eq!(claim.custody_id, record.custody_id);
        assert!(
            !record.sweep_disposition().authorizes_checkout_removal(),
            "no custody state this writer publishes may authorize a removal"
        );
        drop(custodian);
        std::fs::remove_dir_all(&worktree_root).unwrap();
    }

    // ---- slice 2b2 repair R3: residue identity safety and honest policy ----

    /// R3's red test — the identity hazard, reproduced deterministically.
    ///
    /// The shipped durable arm did `remove_file(staged_path)` on the reasoning that a committed
    /// rename leaves the source name free, so the unlink is a harmless no-op. It is a no-op in
    /// exactly the case where it does nothing, and in the ONLY case where it does something —
    /// another actor created a file at that name after the rename — it deletes a foreign object
    /// whose identity was never checked. Here that actor is the test: the file at the staging
    /// name is emphatically not ours, and it must survive.
    ///
    /// Discriminates the shipped code precisely: re-add the unlink and this goes red.
    #[test]
    fn a_durable_publication_never_unlinks_the_staging_pathname() {
        let worktree_root = root("durable-no-unlink");
        let target = worktree_root.join("ownr-run7-abc");
        let custodian =
            WorktreeCustodianV1::enter(&worktree_root, &target.to_string_lossy(), binding(&target))
                .unwrap();
        // Someone else's file, sitting at the name our staging would have used.
        let foreign = worktree_root.join(format!(
            "ownr-run7-abc{CUSTODY_RECORD_SUFFIX}.staging-{}",
            "a".repeat(STAGING_NONCE_HEX_LEN)
        ));
        std::fs::write(&foreign, b"another actor's bytes").unwrap();

        custodian.settle_residue(
            &foreign,
            &CustodyPublicationV1::Durable {
                retried_rename: None,
            },
        );

        assert!(
            foreign.exists(),
            "a durable publication must not unlink whatever occupies the staging name"
        );
        assert_eq!(
            std::fs::read(&foreign).unwrap(),
            b"another actor's bytes",
            "and must certainly not have replaced its contents"
        );
        drop(custodian);
        std::fs::remove_dir_all(&worktree_root).unwrap();
    }

    /// The same rule for the ambiguous arm that can actually leave residue: an unverified rename
    /// whose source name is occupied by an object we cannot prove is ours. Leaving it is not
    /// merely conservative here, it is mandatory — unlinking would destroy a foreign file.
    #[test]
    fn an_unverified_rename_never_unlinks_what_occupies_the_staging_name() {
        let worktree_root = root("unverified-no-unlink");
        let target = worktree_root.join("ownr-run7-abc");
        let custodian =
            WorktreeCustodianV1::enter(&worktree_root, &target.to_string_lossy(), binding(&target))
                .unwrap();
        let foreign = worktree_root.join(format!(
            "ownr-run7-abc{CUSTODY_RECORD_SUFFIX}.staging-{}",
            "b".repeat(STAGING_NONCE_HEX_LEN)
        ));
        std::fs::write(&foreign, b"not ours either").unwrap();

        for outcome in [
            CustodyPublicationV1::RenameOutcomeUnverified("undecidable".into()),
            CustodyPublicationV1::ParentSyncAmbiguous("sync".into()),
            CustodyPublicationV1::TargetIdentityUnverified("identity".into()),
        ] {
            custodian.settle_residue(&foreign, &outcome);
            assert!(
                foreign.exists(),
                "no publication arm may unlink the staging name: {outcome:?}"
            );
        }
        assert_eq!(std::fs::read(&foreign).unwrap(), b"not ours either");
        drop(custodian);
        std::fs::remove_dir_all(&worktree_root).unwrap();
    }

    /// Residue recognition is EXACT (sol W-7). The storage report labels whatever this accepts as
    /// bridge-owned `Evidence`, so an over-permissive predicate would present an operator's own
    /// file as bridge residue. Discriminates the shipped `is_ascii_hexdigit` + any-length check,
    /// which accepted uppercase and every wrong length.
    #[test]
    fn staged_residue_recognition_requires_exactly_thirty_two_lowercase_hex() {
        let stem = format!("wt{CUSTODY_RECORD_SUFFIX}.staging-");
        let ok = format!("{stem}{}", "0123456789abcdef".repeat(2));
        assert!(is_staged_custody_residue(&ok));
        assert!(is_staged_custody_residue(&format!("/wt-root/{ok}")));

        for bad in [
            format!("{stem}{}", "a".repeat(STAGING_NONCE_HEX_LEN - 1)), // short
            format!("{stem}{}", "a".repeat(STAGING_NONCE_HEX_LEN + 1)), // long
            format!("{stem}{}", "A".repeat(STAGING_NONCE_HEX_LEN)),     // uppercase
            format!("{stem}{}", "g".repeat(STAGING_NONCE_HEX_LEN)),     // non-hex
            stem.clone(),                                               // no nonce
            format!(
                "{CUSTODY_RECORD_SUFFIX}.staging-{}",
                "a".repeat(STAGING_NONCE_HEX_LEN)
            ), // no stem
            "wt.custody.v1.json".to_string(),                           // a real record
            "wt.meta.json".to_string(),                                 // a legacy sidecar
        ] {
            assert!(
                !is_staged_custody_residue(&bad),
                "must not be recognised as residue: {bad}"
            );
        }
    }

    /// The names this writer really mints satisfy the tightened predicate. Without this, the
    /// exactness test above could pass against a generator that produces something else entirely.
    #[test]
    fn every_minted_staging_name_satisfies_the_exact_predicate() {
        let name = record_file_name("/wt-root/ownr-run7-abc").unwrap();
        for _ in 0..16 {
            let staged = staged_record_name(&name).unwrap();
            assert!(
                is_staged_custody_residue(&staged.to_string_lossy()),
                "minted name must be recognised: {staged:?}"
            );
        }
    }
}
