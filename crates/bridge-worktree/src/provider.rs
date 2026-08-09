use crate::custody::RecoveryLocatorV1;
use bridge_core::error::BridgeError;

/// What a custody-aware add observed at the target after it failed.
///
/// A three-way answer, not a bool, because "I could not tell" and "nothing is there" must not
/// collapse: only the second may ever license touching the target, and an unproven probe that
/// read as absent would be the exact fail-open this operation exists to remove.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CustodyAddTargetV1 {
    /// Probed and provably nothing exists at the target path.
    ProvablyAbsent,
    /// Something exists at the target path. It is work, and work is preserved.
    Present,
    /// The probe could not answer. Callers must treat this exactly as [`Self::Present`].
    Unproven,
}

impl CustodyAddTargetV1 {
    /// Whether the caller may treat the target as nonexistent. Exhaustive by design: a new arm
    /// must be classified by decision, not by falling through a wildcard into "nothing there".
    #[must_use]
    pub fn is_provably_absent(self) -> bool {
        match self {
            Self::ProvablyAbsent => true,
            Self::Present | Self::Unproven => false,
        }
    }
}

/// A custody-aware add that failed, classified so the writer can publish a truthful record.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CustodyAddFailureV1 {
    pub reason: String,
    pub target: CustodyAddTargetV1,
    /// The source's absolute git common dir, if it could still be read. `None` means the writer
    /// has no observed common-dir object and must record a degraded, plan-derived identity — the
    /// shape §2.2 permits exactly where no live identity exists.
    pub common_dir: Option<String>,
    /// How a recovery consumer would reach whatever survived. The `Err` arm of the registration
    /// probe maps to [`RecoveryLocatorV1::RegistrationUnproven`] — 2a's docstring'd obligation,
    /// and the only producer of that variant.
    pub recovery_locator: RecoveryLocatorV1,
}

/// The outcome of [`WorktreeProvider::add_under_custody`].
///
/// **Design note (slice 2b2, structural decision 2).** Identities come back through the RETURN
/// TYPE, not an out-param record the provider fills in. Two reasons:
///
/// 1. **The provider does not know the identities the claim needs.** Three of the claim's four
///    `WorktreeObjectIdentityV1`s (source, root, common dir) plus the target's own `dev`/`ino`
///    are captured BY DESCRIPTOR by the writer, under the custody lock, after the add returns —
///    risk R-8's accepted disposition. A provider handed a record to fill would either have to
///    re-open those objects itself (a second, unsynchronised identity capture) or leave them
///    blank, and a partially-filled out-param cannot be told from a fully-filled one by type.
/// 2. **What the provider uniquely knows is exactly what is returned here**: the common dir, and
///    the two probe answers (`target`, `recovery_locator`) that only the git-level operation can
///    produce. Returning precisely that, and nothing else, keeps the trait honest — a test double
///    cannot accidentally satisfy the identity contract by leaving an out-param untouched.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CustodyAddOutcomeV1 {
    /// The checkout materialized. `common_dir` is the source's absolute git common dir.
    Materialized { common_dir: String },
    /// The add failed. **The target was not repaired by deletion** (§2.5).
    Failed(CustodyAddFailureV1),
}

/// Materializes/removes per-session git worktrees. Host impl shells out to git; tests use a fake.
#[async_trait::async_trait]
pub trait WorktreeProvider: Send + Sync {
    /// Create a detached worktree of `repo` at `worktree_path` (base ref = repo HEAD).
    /// Returns the source's ABSOLUTE git common-dir (for the sidecar / crash-prune).
    async fn add(&self, repo: &str, worktree_path: &str) -> Result<String, BridgeError>;

    /// Does this provider implement [`Self::add_under_custody`]?
    ///
    /// A side-effect-free preflight, and it exists because the alternative is unsafe: the V3
    /// writer publishes `ProtectionPrepared` and `Materializing` BEFORE it calls the add, so
    /// discovering the refusing default afterwards would leave a durable record asserting a
    /// materialization that never began — and `Materializing` is a live state the sweep classifies
    /// `Recover`, so nothing would ever resolve it (slice 2b2 repair R4).
    ///
    /// **Implementation obligation:** override this in lockstep with [`Self::add_under_custody`].
    /// Both default to the refusing pair, so an untouched impl is consistent by construction; an
    /// impl that overrides one and not the other is not, and the writer trusts this answer.
    fn supports_custody_add(&self) -> bool {
        false
    }

    /// The R2f1b custody-aware add (§2.5, R-7). **`cleanup_failed_add` is forbidden here**: a
    /// partial add becomes a preserved or unknown custody state, never a `remove_dir_all`.
    ///
    /// A separate operation rather than a change to [`Self::add`], because `add` serves V2 and V2
    /// must stay byte-identical — and because the provider is type-erased behind
    /// `Arc<dyn WorktreeProvider>`, so an inherent method on the host impl would be unreachable
    /// from the backend (R-6).
    ///
    /// The default REFUSES. `Err` means "this provider has no custody-aware add" and is
    /// categorically different from `Ok(Failed(..))`, which means "the add ran and did not
    /// succeed"; conflating them would let a provider that never implemented custody look like
    /// one whose add merely failed, and the writer would publish a record for a checkout no
    /// provider is protecting.
    async fn add_under_custody(
        &self,
        repo: &str,
        worktree_path: &str,
    ) -> Result<CustodyAddOutcomeV1, BridgeError> {
        let _ = (repo, worktree_path);
        Err(BridgeError::ConfigInvalid {
            reason: "worktree provider does not implement the R2f1b custody-aware add".into(),
        })
    }

    /// Remove the worktree + prune the source's dangling registration. Already
    /// absent targets are idempotent; an incomplete real cleanup is an error.
    async fn remove(&self, repo: &str, worktree_path: &str) -> Result<(), BridgeError>;

    /// Does this provider implement [`Self::remove_v2`]?
    ///
    /// A side-effect-free preflight, for the same reason `supports_custody_add` is one (slice 2b2
    /// repair R4): the capability is minted by a CAS that durably publishes `DeleteAuthorized`
    /// BEFORE any provider call, and discovering the refusing default afterwards would strand the
    /// checkout in a state whose sweep disposition is `Recover` — recovery-owned forever, for a
    /// removal that could never have started.
    ///
    /// **Implementation obligation:** override this in lockstep with [`Self::remove_v2`].
    fn supports_capability_removal(&self) -> bool {
        false
    }

    /// R2f1b §5.1's capability-consuming removal: *"`HostGitWorktree::remove_v2` takes that
    /// capability — **not a raw path** — revalidates source/root/target/common-dir identities
    /// immediately before Git removal, and verifies registration + target absence afterward."*
    ///
    /// The signature is the enforcement. There is no `repo`/`worktree_path` parameter: the paths
    /// come out of the [`AuthorizedRemovalV1`], which can only be built by consuming a
    /// [`DeletionCapabilityV1`] through its descriptor revalidation, which in turn can only be
    /// minted by the `LiveProtected -> DeleteAuthorized` CAS under both custody cells. A caller
    /// holding only a path cannot reach this method at all.
    ///
    /// The default REFUSES, and unlike `preserve_checkout_v1`'s no-op default this one has to:
    /// §5.4's rule is that a refusing default belongs on any method whose silent absence would let
    /// an *effect* happen unguarded, and here the absent effect is the removal itself. A provider
    /// that silently answered `Ok` would tell the custody writer to publish a `Removed` tombstone
    /// over a checkout that is still on disk.
    ///
    /// [`AuthorizedRemovalV1`]: crate::custody_writer::AuthorizedRemovalV1
    /// [`DeletionCapabilityV1`]: crate::custody_writer::DeletionCapabilityV1
    async fn remove_v2(
        &self,
        authorized: crate::custody_writer::AuthorizedRemovalV1,
    ) -> Result<(), BridgeError> {
        let _ = authorized;
        Err(BridgeError::ConfigInvalid {
            reason: "worktree provider does not implement the R2f1b capability removal".into(),
        })
    }

    /// True if `path` is inside a git work tree.
    async fn is_git_repo(&self, path: &str) -> bool;
}

pub(crate) fn add_argv<'a>(repo: &'a str, wt: &'a str, base: &'a str) -> Vec<&'a str> {
    vec!["-C", repo, "worktree", "add", "--detach", wt, base]
}

pub(crate) fn remove_argv<'a>(repo: &'a str, wt: &'a str) -> Vec<&'a str> {
    vec!["-C", repo, "worktree", "remove", "--force", wt]
}

pub(crate) fn is_repo_argv(path: &str) -> Vec<&str> {
    vec!["-C", path, "rev-parse", "--is-inside-work-tree"]
}

pub(crate) fn prune_argv(repo: &str) -> Vec<&str> {
    vec!["-C", repo, "worktree", "prune"]
}

pub(crate) fn list_porcelain_argv(repo: &str) -> Vec<&str> {
    vec!["-C", repo, "worktree", "list", "--porcelain", "-z"]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn argv_builders_emit_expected_git_invocations() {
        assert_eq!(
            add_argv("/repo", "/wt/x", "HEAD"),
            vec!["-C", "/repo", "worktree", "add", "--detach", "/wt/x", "HEAD"]
        );
        assert_eq!(
            remove_argv("/repo", "/wt/x"),
            vec!["-C", "/repo", "worktree", "remove", "--force", "/wt/x"]
        );
        assert_eq!(
            is_repo_argv("/some/dir"),
            vec!["-C", "/some/dir", "rev-parse", "--is-inside-work-tree"]
        );
        assert_eq!(
            prune_argv("/repo"),
            vec!["-C", "/repo", "worktree", "prune"]
        );
        assert_eq!(
            list_porcelain_argv("/repo"),
            vec!["-C", "/repo", "worktree", "list", "--porcelain", "-z"]
        );
    }
}
