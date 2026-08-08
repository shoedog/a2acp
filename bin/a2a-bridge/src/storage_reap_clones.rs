//! `a2a-bridge storage reap --clones` — the SECOND destructive authority in the storage system
//! (R2f1b pre-slice-2 custody plan §3 S4, §5, §7), and the only one whose mistake destroys bytes that
//! exist nowhere else.
//!
//! S3 deletes regenerable payloads: a wrongly-reaped build target costs a rebuild. This command deletes
//! a `.a2a-implement` quarantine clone — a working checkout with its own `--no-hardlinks` object store,
//! carrying commits that may exist on no other disk. So the licensing evidence is strictly larger than
//! S3's: everything S3 requires, PLUS proof that the content is already somewhere it will survive.
//!
//! Owner ruling **D-1**: pre-squash bot commits need not survive; content verifiably on the SOURCE
//! repository's `main` suffices. That ruling is what makes this command legal at all, and it is also
//! exactly what the gates must prove — not assume.
//!
//! The gates, in refusal order, every one re-derived AT THE DESTRUCTIVE BOUNDARY under the run's HELD
//! operation lock (an S2 report is an observation, never a warrant):
//!
//! 1. **everything S3 gates on** — D-2 protected roots by canonical identity, the typed
//!    [`sr::ItemSource`] (a volume name is not a path; a `[worktrees]` payload is not this command's
//!    custody), a descriptor-pinned scan root re-verified before and after, the `impl-<pid>-<nonce>`
//!    run-owner liveness park, the operation lock HELD across probe→delete, one `lsof` consumer probe
//!    per run directory, and an affirmative container answer whenever a runtime is configured. HERE the
//!    operation lock is genuinely discriminating: `implement_resume` and `merge` both take it, and both
//!    would be operating on the very directory this command removes;
//! 2. **shape** — `git_shape` must prove standalone-clone `.git` DIRECTORY semantics. A linked worktree
//!    shares its source repository's object store and is removed with `git worktree remove`; an
//!    ambiguous `.git` proves nothing. Both park (worktree checkouts remain under the existing ADR-0025
//!    sidecar sweep authority, not this command's);
//! 3. **git state** — `git status --porcelain` must be CLEAN, and the things porcelain CANNOT see are
//!    checked separately, because each of them is silent by design: `ls-files -v` for
//!    `--assume-unchanged` / `--skip-worktree` / sparse entries (which suppress the porcelain line for
//!    modified tracked bytes), and the submodule gate for initialized submodules (a CLEAN submodule
//!    emits no entry at all while its object store sits in `.git/modules`). An ignored entry is
//!    disposable only if S3's own on-disk provenance says so. An unborn HEAD parks;
//! 4. **content on main** — [`sr::on_source_main_with_lookback`] must answer `yes(head)` (HEAD is an
//!    ancestor of source main) or `yes(tree)` (HEAD's exact tree is on source main under a different
//!    commit — the squash landing). `no` parks and `unknown` parks: one means demonstrably not landed,
//!    the other means the probe could not tell, and neither is a warrant. The source repository is
//!    found through the clone's own `origin` URL and only when that URL is a LOCAL PATH — a hosted
//!    origin parks, because the containment query must be asked of a local repository's live refs and
//!    this command contacts no network — and, when the run's checkpoint records one, `origin` must
//!    AGREE with it: `origin` lives inside the `:rw` mount, so an agent can repoint the proof.
//!
//!    **S4b/F1 — CHAINED ORIGINS.** An `implement` run may be started from another run's clone (a
//!    `--repo` pointed at a quarantine directory), so `origin` can name a SIBLING clone under the same
//!    `.a2a-implement` scan root rather than the repository the work is meant to land in. Asking the
//!    D-1 question of a sibling answers it about the wrong repository — a forensic pass over this
//!    host's 81 parked clones found 68 in exactly that shape. So when the resolved origin is itself
//!    inside the pinned scan root, [`resolve_origin_root`] FOLLOWS the chain, hop by hop, until it
//!    reaches a repository OUTSIDE the root: that is the true source, and containment (and the ref
//!    sweep's "independently on source main" test) is asked of it. Every hop is bounded and proved —
//!    at most [`MAX_ORIGIN_HOPS`] edges, no repeated path (a cycle parks), each hop must exist, be a
//!    local path, be a real git repository, and agree with its OWN run checkpoint. Anything else parks
//!    with verdict `unknown` and the chain walked so far recorded on the refusal. The full chain is
//!    written to the fold receipt;
//!
//!    **S4b/F2 — the OWNER DISPOSITION LICENSE.** Plan §3 S4 always licensed two things: content
//!    verifiably on main, "or whose run the owner has dispositioned as abandoned". `--disposition-list
//!    <file>` supplies the second. For a run id named in that owner-authored file, the content-on-main
//!    gate above — and only that gate, including its source resolution, its ref sweep and its
//!    moving-history recheck — is REPLACED by the license. EVERY other gate still runs and still
//!    refuses: a dispositioned clone that is DIRTY, whose owning pid is alive, whose operation lock is
//!    held, whose container axis is unanswered or whose consumers probe is not FREE parks exactly as
//!    it would without the license, with the conflict named. An owner can disposition a RUN; nobody
//!    dispositions bytes that are on no commit and that no one has read. The receipt records the
//!    license — who authorized it, why, which file, and that file's sha256 — under the verdict
//!    `owner-disposition`, and NEVER claims the content is on main;
//! 5. **every other ref** — containment proves HEAD, but the deletion takes the WHOLE object store.
//!    `refs/heads/*`, `refs/tags/*` and `refs/stash` are swept, and each tip must be HEAD, an ancestor
//!    of HEAD, or independently on source main. This is not hypothetical: `head_guard` deliberately
//!    LEAVES a clone whose agent committed or switched branch, and `restore_branch` puts HEAD back
//!    without deleting the branch the agent made.
//!
//!    Objects reachable from NO ref (the re-authored commit an `Unlanded` merge leaves behind) are out
//!    of scope by design: they are deterministic recomputations of inputs this command preserves, not
//!    unique custody, and `git gc` would drop them in the source repository too;
//! 6. **the fold receipt, BEFORE the deletion**, in a SIBLING namespace that outlives the clone
//!    (`<root>/.receipts/<run id>-fold.json`, fsync'd), plus the clone's `.git/a2a-bridge/` evidence
//!    AND its `.git/A2A_TASK.md` / `.git/A2A_COMMIT_MSG` copied to `<root>/.receipts/<run id>-evidence/`.
//!    Evidence has its own retention (plan §5) and never dies with the parent it describes. A
//!    structural preflight runs first and refuses AMBIGUITY on either side (a symlinked sidecar, a
//!    non-regular entry within it, a symlinked or misplaced `.receipts`) while letting genuine ABSENCE
//!    through — the two were once the same `Ok(empty)`, which let a clone be deleted with "evidence
//!    preserved" on its receipt. Every durability barrier is propagated, and the namespace is fsync'd
//!    before the first removal. Any of these failing PARKS the clone — a deletion whose record could
//!    not be established does not happen;
//! 7. **exact-mechanism removal** — the `merge::reap_clone` guard shape (canonical path equal to
//!    `<root>/<run id>`, has a real `.git`, is not and does not contain the source repository) plus
//!    S3's dev/ino rechecks immediately before the unlink. Never a broad prefix, never a follow.
//!
//! The fold receipt is written twice on purpose: once as the crash-durable INTENT (`disposition:
//! planned_delete`) before the first removal, once with the outcome before the operation lock is
//! released. A record written only afterwards describes an end state a crash may have prevented from
//! ever existing.
//!
//! Fail-closed by design, and deliberately so: a retained clone costs 20–180 MiB of disk; a wrongly
//! reaped one costs work that exists nowhere else. A squash that REWROTE the tree therefore reads `no`
//! and the clone is kept.
//!
//! # S4b residuals, disclosed rather than discovered
//!
//! - **A chain hop with no checkpoint contributes no cross-check of its own.** Each hop is bound to
//!   its own run's `implement-checkpoint.json`, which the bridge writes before the agent runs; a hop
//!   with no readable checkpoint (a legacy run, a hand-made repository) is followed on its
//!   `remote.origin.url` alone. What closes the gap is not the per-hop check but
//!   [`corroborate_terminal_root`]: whatever chain of URLs was followed, the repository it lands on
//!   must contain this clone's own lineage root (and its checkpoint's base commit, when that resolves
//!   in the clone) before any containment verdict is asked of it. A rewritten `origin` can therefore
//!   point the walk somewhere; it cannot make a repository the clone never came from answer for it.
//! - **A chain broken by a PRIOR invocation does not self-heal.** Within one invocation, deletion
//!   order is irrelevant: every candidate's chain is resolved during admission, before the first
//!   removal, and every terminal root is outside the scan root and so survives sibling deletions. But
//!   if an earlier run reaped a parent clone, the child's `origin` names a directory that is simply
//!   gone, and nothing ever rewrites it — the child parks `unknown` forever. No bytes are lost, and
//!   the designed recovery is the F2 license, which needs no source repository at all. (An earlier
//!   draft of this note claimed a second invocation would resolve it. That was false, and the test
//!   that would have caught it had been narrowed to a single row.)

use crate::storage_reap as rp;
use crate::storage_report as sr;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// How far back along source main the exact-tree (squash-landing) search looks, when the operator has
/// not configured `[storage] clone_reap_lookback`.
///
/// Higher than S2's audit-time [`sr::SOURCE_MAIN_LOOKBACK`] of 500 on measured grounds: at 500,
/// "lookback exhausted" is the COMMON verdict on this repository's own backlog, and an exhausted
/// lookback is `unknown` — which parks. A window that parks the population it exists to clean is not a
/// safety property, it is a broken probe. 2000 commits of `git log --format=%H %T` is one cheap
/// revision walk per clone.
pub const DEFAULT_CLONE_REAP_LOOKBACK: u32 = 2000;

pub const FOLD_RECEIPT_SCHEMA: &str = "a2a-bridge.clone-fold-receipt.v1";

/// Disposition values a fold receipt can carry. `PLANNED_DELETE` is the intent written before the
/// removal; the others replace it once the outcome is known.
pub const DISPOSITION_PLANNED: &str = "planned_delete";
pub const DISPOSITION_DELETED: &str = "deleted";
pub const DISPOSITION_PARTIAL: &str = "partial";
pub const DISPOSITION_UNKNOWN: &str = "unknown";
pub const DISPOSITION_ABORTED: &str = "aborted_before_removal";

/// Ignored entries that may be present in an otherwise-clean clone without blocking its deletion:
/// exactly the classes the report itself calls regenerable ([`sr::PayloadClass::BuildTarget`] and
/// [`sr::PayloadClass::DependencyCache`]). Every OTHER ignored entry blocks — an ignored file is
/// invisible to `git`, so it is invisible to the containment proof, and this list is the whole of what
/// this command is willing to assume about invisible bytes.
pub const DISPOSABLE_IGNORED: [&str; 3] = ["target", "node_modules", ".venv"];

/// How many offending `git status` entries a refusal quotes before it stops. Bounded so a clone with a
/// hundred thousand untracked files cannot turn one park reason into a memory-sized string.
const MAX_QUOTED_STATUS_ENTRIES: usize = 5;

// ---------------------------------------------------------------------------------------------
// Fold receipt (plan §7)
// ---------------------------------------------------------------------------------------------

/// WHICH evidence answered the D-1 question, not merely that something did. `yes(tree)` without naming
/// the matched commit is unauditable: the operator cannot go and look.
///
/// One value here is NOT a containment answer at all: [`VERDICT_OWNER_DISPOSITION`] records that the
/// owner licensed the deletion of an abandoned run instead. Its fields are disjoint from the proof
/// fields on purpose — a reader can never mistake a license for a landing.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct ContainmentEvidence {
    /// `yes(head)` | `yes(tree)` | `owner-disposition` (`no` / `unknown` never reach a receipt — they
    /// park).
    pub verdict: String,
    /// The ref treated as the source's main branch (`main`, `master`, or its own HEAD branch).
    pub main_ref: Option<String>,
    /// For `yes(tree)`: the commit on source main whose tree is byte-identical to this HEAD's.
    pub matched_commit: Option<String>,
    /// F2, `owner-disposition` only: the free text naming who authorized the license.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub authorized_by: Option<String>,
    /// F2, `owner-disposition` only: the free text saying why.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// F2, `owner-disposition` only: the canonical path of the list file the license was read from.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub list_file: Option<String>,
    /// F2, `owner-disposition` only: that file's sha256, so the exact authorized bytes are auditable
    /// after the fact — a list edited later cannot retroactively describe this deletion.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub list_sha256: Option<String>,
    pub detail: String,
}

impl ContainmentEvidence {
    /// A proof-shaped verdict, with the license fields absent.
    fn proved(
        verdict: String,
        main_ref: Option<String>,
        matched: Option<String>,
        detail: String,
    ) -> Self {
        Self {
            verdict,
            main_ref,
            matched_commit: matched,
            authorized_by: None,
            reason: None,
            list_file: None,
            list_sha256: None,
            detail,
        }
    }

    /// Was this deletion licensed by an owner disposition rather than by a containment proof?
    pub fn is_owner_disposition(&self) -> bool {
        self.verdict == VERDICT_OWNER_DISPOSITION
    }
}

/// R2. One ref the deletion took, recorded for the record — never judged.
///
/// The gating sweep ([`refs_disposition`]) is part of the content-on-main proof, so an owner
/// disposition replaces it: the run is abandoned, tips and all, and that is the owner's call to make.
/// What is NOT the owner's call is whether the loss is auditable afterwards. An inventory makes it so:
/// after the clone is gone, the receipt still says which tips existed, where they pointed, and whether
/// each was inside the history HEAD carried.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct RefFact {
    pub refname: String,
    /// The tip's commit, or `None` when it could not be peeled to one (an annotated tag on a blob).
    pub oid: Option<String>,
    pub is_head: bool,
    /// `None` = the ancestry probe did not answer. Recorded as unknown rather than guessed — this is
    /// an inventory, so an unanswered probe is a fact about the inventory, not a refusal.
    pub is_ancestor_of_head: Option<bool>,
}

/// One reported row underneath a clone, and whether it is still on disk after the removal attempt.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct DescendantPresence {
    pub path: String,
    pub present: bool,
}

/// The durable identity of a run whose clone is gone (plan §7). Written beside — never inside — the
/// clone, because its whole purpose is to outlive it.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct FoldReceipt {
    pub schema: String,
    pub run_id: String,
    /// From the run's checkpoint when it is readable. `None` is recorded honestly rather than guessed:
    /// a legacy or half-written checkpoint must not fabricate an identity.
    pub task_id: Option<String>,
    pub branch: Option<String>,
    /// The pre-squash HEAD — the identity that squash-merging strands, and the reason this file exists.
    pub head: Option<String>,
    pub tree: Option<String>,
    pub base: Option<String>,
    pub base_ref: Option<String>,
    /// The TRUE source repository the containment proof was asked of — the end of the F1 origin chain,
    /// not necessarily the repository this clone's `origin` names. `None` only on the F2 licensed path,
    /// where no source repository is required and one may not even exist any more.
    pub source_repo: Option<String>,
    /// F1: every `origin` edge walked to reach `source_repo`, in order. One entry for the ordinary
    /// directly-cloned run; more for a chained implement. Recorded so the reader can see WHICH
    /// repository answered the D-1 question and how it was found.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub origin_chain: Vec<String>,
    pub clone_path: String,
    pub containment: ContainmentEvidence,
    /// R2: on an owner-licensed deletion, every ref that existed when the clone was removed. The
    /// license replaces the ref SWEEP (a gate); it does not excuse losing the RECORD of what was
    /// taken. Empty on the proved path, where every tip had to pass the sweep to get here.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub ref_inventory: Vec<RefFact>,
    /// Plan §5's durability coordinate at disposition time. `OnMain{...}` for a containment proof,
    /// `OwnerDisposition{...}` for an F2 license — never `OnMain` for the latter, because nothing
    /// proved the content is on main.
    pub durability: String,
    pub disposition: String,
    pub logical_bytes: Option<u64>,
    pub disk_bytes: Option<u64>,
    /// Where the clone's `.git/a2a-bridge/` evidence was copied before the removal.
    pub evidence_preserved_at: Option<String>,
    pub evidence_files: Vec<String>,
    /// Why a non-`deleted` disposition ended that way. `None` on a clean deletion.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failure_detail: Option<String>,
    /// For a removal that began and did not cleanly finish: what is ACTUALLY still on disk underneath
    /// the clone, restat'ed after the attempt. The report is transient; this is the durable statement,
    /// and on a `partial`/`unknown` outcome it is the only honest one available.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub descendant_presence: Vec<DescendantPresence>,
    /// Every gate that licensed the deletion, in order.
    pub gates: Vec<String>,
    pub scan_root: String,
    pub scan_root_identity: String,
    pub at_epoch_secs: u64,
}

/// `<root>/.receipts/<run id>-fold.json`.
pub fn fold_receipt_name(run_id: &str) -> String {
    format!("{run_id}-fold.json")
}

/// `<root>/.receipts/<run id>-evidence/`.
pub fn evidence_dir_name(run_id: &str) -> String {
    format!("{run_id}-evidence")
}

pub fn receipts_dir(root: &Path) -> PathBuf {
    root.join(sr::RECEIPTS_DIR)
}

// ---------------------------------------------------------------------------------------------
// Pure: git status disposition
// ---------------------------------------------------------------------------------------------

/// PURE. Does `git status --porcelain --ignored=traditional --ignore-submodules=none` output describe a
/// tree whose every byte is on a commit?
///
/// `Ok(summary)` only for output that is empty or consists solely of ignored entries under
/// [`DISPOSABLE_IGNORED`]. EVERYTHING else refuses, including output this parser cannot interpret: a
/// status line it does not understand is not a clean tree.
///
/// Quoted paths (git's `core.quotePath` escaping, used for non-ASCII and control characters) are never
/// unquoted here and never read as disposable — matching S2's disclosed non-UTF-8 posture, where
/// ambiguity is refused rather than resolved by guesswork.
pub fn status_disposition(stdout: &str, clone: &Path) -> Result<String, String> {
    let mut blocking: Vec<String> = Vec::new();
    let mut blocking_total = 0usize;
    let mut disposable: Vec<String> = Vec::new();
    for line in stdout.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let interpretable = line.len() > 3 && line.is_char_boundary(2) && line.is_char_boundary(3);
        let code = if interpretable { &line[..2] } else { "" };
        let path = if interpretable { &line[3..] } else { "" };
        if code == "!!" && ignored_entry_is_disposable(path, clone) {
            disposable.push(path.to_string());
            continue;
        }
        blocking_total += 1;
        if blocking.len() < MAX_QUOTED_STATUS_ENTRIES {
            blocking.push(line.to_string());
        }
    }
    if blocking_total > 0 {
        return Err(format!(
            "{blocking_total} entry/entries are not on any commit — first {}: {}",
            blocking.len(),
            blocking.join(" | ")
        ));
    }
    Ok(format!(
        "`git status --porcelain` clean ({} disposable ignored entr{} skipped: {})",
        disposable.len(),
        if disposable.len() == 1 { "y" } else { "ies" },
        if disposable.is_empty() {
            "none".to_string()
        } else {
            disposable.join(", ")
        }
    ))
}

/// FS. Is an ignored entry one of the regenerable classes, PROVED ON DISK?
///
/// The name is a candidate filter, never the evidence — the same rule S3 already enforces at its own
/// boundary, reused here by calling S3's own provenance functions:
///
/// - the LAST path component must be `target`, `node_modules` or `.venv`, so a nested
///   `crates/foo/target/` qualifies (it is a real cargo target) while `my-target-notes/` and
///   `src/target-list.txt` do not;
/// - the entry must be a real DIRECTORY — a FILE named `target` (which git reports as `!! target`, with
///   no trailing slash) is never disposable;
/// - and the S3 markers must hold: cargo's own artifacts for `target`, the `package.json` sibling for
///   `node_modules`, `pyvenv.cfg` inside `.venv`. A directory called `target` holding a user's data is
///   not regenerable, and it is invisible to the containment proof precisely because it is ignored.
fn ignored_entry_is_disposable(path: &str, clone: &Path) -> bool {
    let p = path.trim().trim_end_matches('/');
    if p.starts_with('"') || p.is_empty() {
        return false; // quoted/escaped path: refused rather than decoded
    }
    let Some(last) = p.rsplit('/').next() else {
        return false;
    };
    if !DISPOSABLE_IGNORED.contains(&last) {
        return false;
    }
    let full = clone.join(p);
    if !sr::real_dir(&full) {
        return false;
    }
    match last {
        "target" => sr::is_cargo_target(&full),
        // S3's own provenance rule, called rather than restated: `package.json` beside a
        // `node_modules`, `pyvenv.cfg` inside a `.venv`.
        _ => rp::dependency_cache_provenance(&full).is_ok(),
    }
}

// ---------------------------------------------------------------------------------------------
// Pure: checkpoint facts
// ---------------------------------------------------------------------------------------------

/// The identity fields the fold receipt borrows from a run's checkpoint. Every one optional: a legacy
/// or half-written checkpoint must degrade to "unknown", never to a fabricated identity, and must never
/// block the reap (the checkpoint is a convenience here, not a gate).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CheckpointFacts {
    pub task_id: Option<String>,
    pub base: Option<String>,
    pub base_ref: Option<String>,
    pub branch: Option<String>,
    pub phase: Option<String>,
    /// The source repository the bridge cloned FROM, recorded before the agent ran. Cross-checked
    /// against `remote.origin.url`, which lives inside a directory the agent could write.
    pub source_repo: Option<String>,
}

/// PURE. Read the identity fields out of a checkpoint document.
///
/// Deliberately `serde_json::Value` rather than `ImplementCheckpoint`: a strict decode fails whole on a
/// schema-version drift or one unknown field, and would then discard the task id of every legacy run in
/// the backlog — the exact identities the receipt exists to preserve.
pub fn checkpoint_facts(json: &str) -> CheckpointFacts {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(json) else {
        return CheckpointFacts::default();
    };
    let s = |k: &str| {
        v.get(k)
            .and_then(|x| x.as_str())
            .filter(|x| !x.is_empty())
            .map(str::to_string)
    };
    CheckpointFacts {
        task_id: s("task_id").or_else(|| s("resume_id")),
        base: s("base_commit"),
        base_ref: s("base_ref"),
        branch: s("branch"),
        phase: s("phase"),
        source_repo: s("source_repo"),
    }
}

/// FS. The checkpoint facts for a clone, or defaults when there is no readable checkpoint.
fn read_checkpoint_facts(clone: &Path) -> CheckpointFacts {
    let p = sr::evidence_dir(clone).join("implement-checkpoint.json");
    if !sr::real_file(&p) {
        return CheckpointFacts::default();
    }
    std::fs::read_to_string(&p)
        .map(|s| checkpoint_facts(&s))
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------------------------
// F2 — the owner disposition license (plan §3 S4: "or whose run the owner has dispositioned as
// abandoned")
// ---------------------------------------------------------------------------------------------

/// The containment verdict a receipt carries when the deletion was licensed by an OWNER DISPOSITION
/// rather than by a content-on-main proof.
///
/// Deliberately not spelled as one of the `yes(...)` labels, and deliberately not routed through the
/// same fields: the whole failure mode this guards against is a receipt that reads as "the content is
/// on main" when nothing established that. What it says is narrower and true — a person with the
/// authority to abandon this run did so, in writing, in a file whose bytes are hashed here.
pub const VERDICT_OWNER_DISPOSITION: &str = "owner-disposition";

/// The owner-authored license document, exactly as it appears on disk.
///
/// `deny_unknown_fields` is load-bearing, not tidiness: this file AUTHORIZES DELETIONS. A misspelled
/// `run_ids` key that parsed as "no runs" would silently authorize nothing (merely confusing), but a
/// stray or renamed field in the other direction — `"runs"`, `"run_id"`, a copied-in comment key — is
/// how a list that LOOKS like it names five runs comes to name none of them, or how a future field
/// with real meaning gets ignored by an old binary. A license that cannot be read exactly is refused
/// rather than interpreted.
#[derive(Clone, Debug, PartialEq, Eq, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DispositionDocument {
    /// Free text: who authorized the disposition. Never parsed, only recorded.
    pub authorized_by: String,
    /// Free text: why. Never parsed, only recorded.
    pub reason: String,
    /// The `impl-<pid>-<nonce>` run ids the owner is dispositioning as abandoned.
    pub run_ids: Vec<String>,
}

/// A validated license plus the identity of the file it came from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DispositionList {
    pub authorized_by: String,
    pub reason: String,
    pub run_ids: BTreeSet<String>,
    /// The canonical path of the list file.
    pub list_file: String,
    /// sha256 of the file's exact bytes.
    pub list_sha256: String,
}

impl DispositionList {
    /// Does this license name `run_id`? Exact match on the run directory name — never a prefix, never
    /// a pattern. A license is an enumeration of runs, not a rule that could match one nobody meant.
    pub fn licenses(&self, run_id: &str) -> bool {
        self.run_ids.contains(run_id)
    }

    /// The receipt evidence for a licensed deletion. Says what happened (an owner licensed it) and,
    /// explicitly, what did NOT (a containment proof).
    pub fn evidence(&self, run_id: &str) -> ContainmentEvidence {
        ContainmentEvidence {
            verdict: VERDICT_OWNER_DISPOSITION.to_string(),
            main_ref: None,
            matched_commit: None,
            authorized_by: Some(self.authorized_by.clone()),
            reason: Some(self.reason.clone()),
            list_file: Some(self.list_file.clone()),
            list_sha256: Some(self.list_sha256.clone()),
            detail: format!(
                "the owner dispositioned run {run_id} as ABANDONED in {} (sha256 {}), authorized by \
                 {:?}: {:?}. This is a LICENSE, NOT a containment proof: nothing here establishes \
                 that this clone's content is on the source repository's main, and the D-1 \
                 content-on-main gate did not pass for it. Every other gate did.",
                self.list_file, self.list_sha256, self.authorized_by, self.reason
            ),
        }
    }

    /// Plan §5's durability coordinate for a licensed deletion — never `OnMain{...}`.
    fn durability(&self) -> String {
        format!(
            "OwnerDisposition{{authorized_by={:?}, list={}, sha256={}}}",
            self.authorized_by, self.list_file, self.list_sha256
        )
    }
}

/// PURE. Decode and validate a license document.
///
/// Every refusal here is a REFUSAL TO DELETE, so each one is deliberate: an empty document, a document
/// with an unknown field, an empty `authorized_by` or `reason` (an unattributable license is not an
/// audit trail), an empty `run_ids` (a list that authorizes nothing is far more likely a truncated or
/// mis-generated file than an intentional no-op), and any run id that is not a single, ordinary path
/// component — a run id is matched against a directory NAME, and one carrying `/` or `..` could only
/// ever be a mistake or an attempt to make it mean something else.
pub fn parse_disposition_document(raw: &str) -> Result<DispositionDocument, String> {
    if raw.trim().is_empty() {
        return Err("the disposition list is empty — an empty file is not a license".into());
    }
    let doc: DispositionDocument = serde_json::from_str(raw).map_err(|e| {
        format!(
            "not a valid disposition list: {e}. Expected exactly \
             {{\"authorized_by\": \"...\", \"reason\": \"...\", \"run_ids\": [\"impl-...\"]}} with no \
             other fields"
        )
    })?;
    if doc.authorized_by.trim().is_empty() {
        return Err("`authorized_by` is empty — a deletion license must name its authority".into());
    }
    if doc.reason.trim().is_empty() {
        return Err("`reason` is empty — a deletion license must say why".into());
    }
    if doc.run_ids.is_empty() {
        return Err(
            "`run_ids` is empty — a list that authorizes no run is refused rather than run as a \
             no-op (it is far more likely truncated than intentional)"
                .into(),
        );
    }
    for id in &doc.run_ids {
        let bad = id.trim().is_empty()
            || id.contains('/')
            || id.contains('\\')
            || id == "."
            || id == ".."
            || id.contains("..")
            || id.starts_with('.');
        if bad {
            return Err(format!(
                "run id {id:?} is not a single ordinary path component — a run id names a directory \
                 under the scan root and nothing else"
            ));
        }
    }
    Ok(doc)
}

/// FS. Read, hash and validate the license file.
///
/// R3. ONE `open` with `O_NOFOLLOW`, then `fstat` and read THROUGH THAT SAME DESCRIPTOR. The obvious
/// spelling — `symlink_metadata`, then `std::fs::read` — validates one file and reads another: the
/// two syscalls resolve the path independently, so what was checked and what is hashed onto the
/// receipt need not be the same bytes. That is worth closing on a DELETION LICENSE, whose receipt's
/// whole job is to say which bytes authorized the removal. (The window is narrow and a same-user
/// process able to substitute the file already has the authority to delete these clones itself — this
/// is hardening, not a defect fix — but the fix is a few lines and the property it buys is exact.)
///
/// `O_NOFOLLOW` fails outright on a symlinked leaf, so a link is refused rather than resolved, and
/// the `fstat` proves REGULAR — a FIFO would otherwise let the "license" be produced on demand by
/// whatever is on the other end.
pub fn load_disposition_list(path: &Path) -> Result<DispositionList, String> {
    use std::io::Read as _;
    use std::os::unix::fs::OpenOptionsExt as _;

    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
        .map_err(|e| match e.raw_os_error() {
            // ELOOP (Linux) / ELOOP on macOS too for O_NOFOLLOW on a symlink.
            Some(libc::ELOOP) => format!(
                "{} is a SYMLINK — a deletion license must be a regular file opened directly, so the \
                 bytes hashed onto the receipt are the bytes at the path recorded on it",
                path.display()
            ),
            _ => format!("{} is unreadable: {e}", path.display()),
        })?;
    let md = file
        .metadata()
        .map_err(|e| format!("{} could not be stat'ed: {e}", path.display()))?;
    if !md.is_file() {
        return Err(format!(
            "{} is not a regular file (a directory, FIFO or device cannot be a license)",
            path.display()
        ));
    }
    // Read from the SAME descriptor that was just validated — no second path resolution.
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)
        .map_err(|e| format!("{} is unreadable: {e}", path.display()))?;
    if bytes.is_empty() {
        return Err(format!(
            "{} is empty — an empty file is not a license",
            path.display()
        ));
    }
    let raw = String::from_utf8(bytes.clone())
        .map_err(|e| format!("{} is not valid UTF-8: {e}", path.display()))?;
    let doc = parse_disposition_document(&raw).map_err(|e| format!("{}: {e}", path.display()))?;
    let canonical = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    Ok(DispositionList {
        authorized_by: doc.authorized_by,
        reason: doc.reason,
        run_ids: doc.run_ids.into_iter().collect(),
        list_file: canonical.to_string_lossy().into_owned(),
        list_sha256: bridge_core::execution_policy::Sha256HexV1::digest(&bytes)
            .as_str()
            .to_string(),
    })
}

// ---------------------------------------------------------------------------------------------
// F1 — chained-origin root resolution
// ---------------------------------------------------------------------------------------------

/// How many `origin` edges [`resolve_origin_root`] will follow before giving up.
///
/// Eight, because a chain that deep is not a workflow: `implement --repo <another run's clone>` nests
/// one level in normal use and two under a resume of a chained run. A bound is required regardless —
/// without one, a chain of clones inside the scan root is a walk whose length is chosen by whatever
/// wrote those repositories' configs.
pub const MAX_ORIGIN_HOPS: usize = 8;

/// The `origin` edges walked from a clone to the repository the D-1 question is actually asked of.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OriginChain {
    /// One entry per edge, in walk order: `<from> --origin--> <to>`.
    pub hops: Vec<String>,
    /// The repository OUTSIDE the pinned scan root the chain terminates at.
    pub root: PathBuf,
}

/// Is `p` a git repository at all? Checked before its config is read, so a directory that merely
/// happens to sit at the end of an `origin` URL cannot be treated as a link in the chain.
///
/// A `.git` DIRECTORY (ordinary repo), a `.git` FILE (linked worktree / submodule — still a repository
/// whose config git will read), or a bare layout. Anything else is not a repository, and a chain that
/// runs into one is broken, not merely unusual.
fn looks_like_git_repo(p: &Path) -> bool {
    let dot = p.join(".git");
    if sr::real_dir(&dot) || sr::real_file(&dot) {
        return true;
    }
    sr::real_file(&p.join("HEAD"))
        && sr::real_dir(&p.join("objects"))
        && sr::real_dir(&p.join("refs"))
}

/// Walk `origin` from `clone` until a repository OUTSIDE `scan_root` is reached — the TRUE source.
///
/// The problem this solves is not hypothetical: an `implement` run started with `--repo <another run's
/// clone>` records that sibling as its origin, so the D-1 containment query, asked of `origin`, is
/// asked of a quarantine directory rather than of the repository the work lands in. A forensic pass
/// over this host's 81 parked clones found 68 in that shape — every one of their verdicts was about
/// the wrong repository.
///
/// Bounded and proved at every hop, because each hop is a `remote.origin.url` read out of a directory
/// that a `:rw` container could write:
///
/// - the FIRST edge keeps S4's existing refusal shape ([`rp::ParkReason::OriginNotLocal`]) — a hosted
///   or absent origin on the clone itself means there is no local repository to ask, as before;
/// - every LATER edge that cannot be followed parks with verdict `unknown` and the chain walked so far
///   on the refusal: the probe could not establish which repository to ask, which is not the same as
///   an answer of `no`;
/// - each hop must resolve to a LOCAL directory that EXISTS ([`sr::local_source_path`]) and must be a
///   real git repository ([`looks_like_git_repo`]) before its config is read;
/// - each hop must AGREE with its own run's checkpoint, exactly as S4 already required of the first —
///   the checkpoint is written by the bridge before the agent runs, so it is the one statement about
///   provenance the agent did not author. Applying it per hop is what stops a chain from being walked
///   somewhere the runs themselves never pointed;
/// - a repeated path is a CYCLE and parks (`A -> B -> A` would otherwise loop);
/// - at most [`MAX_ORIGIN_HOPS`] edges.
///
/// The FINAL root satisfies the same rules as S4's single-hop origin did: it is a local path resolved
/// through [`sr::local_source_path`], and it is outside the pinned scan root — which is also what makes
/// [`removal_guard`]'s D-2-shaped check ("the source repository is neither the clone nor inside it")
/// meaningful again, since a sibling clone under the scan root could satisfy that check while being
/// the very kind of directory this command deletes.
pub fn resolve_origin_root(clone: &Path, scan_root: &Path) -> Result<OriginChain, rp::ParkReason> {
    let scan_root = std::fs::canonicalize(scan_root).unwrap_or_else(|_| scan_root.to_path_buf());
    let start = std::fs::canonicalize(clone).unwrap_or_else(|_| clone.to_path_buf());
    let mut visited: BTreeSet<PathBuf> = BTreeSet::new();
    visited.insert(start.clone());
    let mut hops: Vec<String> = Vec::new();
    let mut current = start;

    loop {
        let first_edge = hops.is_empty();
        let walked = hops.join(" | ");
        // The first edge keeps S4's typed refusal; a broken LATER edge is an `unknown` containment
        // verdict, because what failed is establishing WHICH repository to ask.
        let refuse = |detail: String| -> rp::ParkReason {
            if first_edge {
                rp::ParkReason::OriginNotLocal { detail }
            } else {
                rp::ParkReason::NotOnSourceMain {
                    verdict: "unknown".into(),
                    detail: format!("{detail}; origin chain walked so far: [{walked}]"),
                }
            }
        };

        let url = match sr::git_str(&current, &["config", "--get", "remote.origin.url"]) {
            Ok(u) if !u.is_empty() => u,
            Ok(_) | Err(_) => {
                return Err(refuse(format!(
                    "{} has no `remote.origin.url`",
                    sr::display_path(&current)
                )))
            }
        };
        let Some(next) = sr::local_source_path(&current, &url) else {
            return Err(refuse(format!(
                "`remote.origin.url` {url:?} of {} is not a local directory",
                sr::display_path(&current)
            )));
        };

        hops.push(format!(
            "{} --origin--> {}",
            sr::display_path(&current),
            sr::display_path(&next)
        ));

        // The per-hop checkpoint cross-check, S4's rule applied along the whole chain: `origin` is a
        // config value inside a directory a `:rw` agent could write, and the checkpoint is not.
        if let Some(recorded) = read_checkpoint_facts(&current).source_repo.as_deref() {
            let recorded_canon =
                std::fs::canonicalize(recorded).unwrap_or_else(|_| PathBuf::from(recorded));
            if recorded_canon != next {
                return Err(rp::ParkReason::OriginDisagreesWithCheckpoint {
                    detail: format!(
                        "at hop {}, the run at {} records source repo {} in its checkpoint while its \
                         `remote.origin.url` resolves to {}",
                        hops.len(),
                        sr::display_path(&current),
                        recorded_canon.display(),
                        next.display()
                    ),
                });
            }
        }

        if !visited.insert(next.clone()) {
            return Err(rp::ParkReason::NotOnSourceMain {
                verdict: "unknown".into(),
                detail: format!(
                    "the `origin` chain is a CYCLE — {} was already visited: [{}]",
                    sr::display_path(&next),
                    hops.join(" | ")
                ),
            });
        }

        // R1. EVERY hop must be a real repository — the terminal one included. `git -C <dir>` walks
        // UP to find a repository, so a hop naming a plain directory that happens to sit inside some
        // repository's tree would have that ENCLOSING repository answer the D-1 question. (The
        // `GIT_CEILING_DIRECTORIES` in `sr::git_ro` now blocks the ascent as well, so this refuses
        // rather than silently answering from the wrong place; both halves are load-bearing, one for
        // the verdict and one for the refusal being typed.)
        if !looks_like_git_repo(&next) {
            return Err(refuse(format!(
                "the `origin` chain reaches {}, which exists but is NOT a git repository — the true \
                 source cannot be identified{}",
                sr::display_path(&next),
                if hops.len() > 1 {
                    format!(": [{}]", hops.join(" | "))
                } else {
                    String::new()
                }
            )));
        }

        // Outside the scan root: this is the repository the work is meant to land in.
        if !next.starts_with(&scan_root) {
            return Ok(OriginChain { hops, root: next });
        }

        // Still inside the scan root: `next` is a SIBLING quarantine clone, never the true source, so
        // the walk continues. The bound is on the IN-ROOT walk, which is the part whose length is
        // chosen by whatever wrote those repositories' configs.
        if hops.len() > MAX_ORIGIN_HOPS {
            return Err(rp::ParkReason::NotOnSourceMain {
                verdict: "unknown".into(),
                detail: format!(
                    "the `origin` chain has followed {} edges without leaving the scan root {} \
                     (limit {MAX_ORIGIN_HOPS}) — refusing to keep walking: [{}]",
                    hops.len(),
                    sr::display_path(&scan_root),
                    hops.join(" | ")
                ),
            });
        }
        current = next;
    }
}

/// R4. Does the repository the chain landed on actually have anything to do with this clone?
///
/// Every hop of a chain is a `remote.origin.url` read out of a directory a `:rw` container could
/// write, and a hop whose run has no readable checkpoint contributes no cross-check at all. So the
/// TERMINAL root is corroborated against the clone's OWN history, which no config value can move:
///
/// - the clone's LINEAGE ROOT — `rev-list --max-parents=0 HEAD`, the initial commit(s) of the history
///   the clone carries. A clone shares those with the repository it was cloned from however much work
///   is piled on top, and a repository that does not have them is not that repository. This is the
///   corroborator that always applies;
/// - and, when the run's checkpoint records a `base_commit` that the CLONE can actually resolve (a
///   legacy or placeholder value that resolves nowhere corroborates nothing), the root must contain
///   that too.
///
/// Both are `object_present` questions asked of the root — definite answers, never network. Anything
/// uncorroborated parks: a containment verdict computed against a repository this clone demonstrably
/// never came from is a verdict about the wrong history, which is the whole defect S4b exists to fix.
///
/// The LICENSED path does not call this: it asks no containment question, so it needs no source.
fn corroborate_terminal_root(
    clone: &Path,
    head: &str,
    root: &Path,
    chain: &OriginChain,
) -> Result<String, rp::ParkReason> {
    let unknown = |detail: String| rp::ParkReason::NotOnSourceMain {
        verdict: "unknown".into(),
        detail: format!(
            "{detail}. The chain walked was [{}] — a containment verdict from an uncorroborated \
             repository would be a verdict about the wrong history",
            chain.hops.join(" | ")
        ),
    };

    let roots = sr::git_str(clone, &["rev-list", "--max-parents=0", head]).map_err(|e| {
        unknown(format!(
            "the clone's own lineage root is unreadable ({e}), so the terminal root {} could not be \
             corroborated",
            sr::display_path(root)
        ))
    })?;
    let roots: Vec<&str> = roots
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    if roots.is_empty() {
        return Err(unknown(format!(
            "the clone has no lineage root commit, so the terminal root {} could not be corroborated",
            sr::display_path(root)
        )));
    }
    let mut matched: Option<&str> = None;
    for r in &roots {
        match sr::object_present(root, r) {
            Ok(true) => {
                matched = Some(r);
                break;
            }
            Ok(false) => {}
            Err(e) => {
                return Err(unknown(format!(
                    "the terminal root {} could not be asked whether it holds this clone's lineage \
                     root {r}: {e}",
                    sr::display_path(root)
                )))
            }
        }
    }
    let Some(matched) = matched else {
        return Err(unknown(format!(
            "the terminal root {} does NOT contain this clone's lineage root ({}) — the chain leads \
             to a repository this clone never came from",
            sr::display_path(root),
            roots.join(", ")
        )));
    };

    // The checkpoint's base, when it is a commit the clone itself can resolve.
    let mut base_note = "no resolvable checkpoint base to cross-check".to_string();
    if let Some(base) = read_checkpoint_facts(clone).base.as_deref() {
        if sr::object_present(clone, base).unwrap_or(false) {
            match sr::object_present(root, base) {
                Ok(true) => base_note = format!("and the checkpoint's base commit {base}"),
                Ok(false) => {
                    return Err(unknown(format!(
                        "the terminal root {} does not contain the base commit {base} that this \
                         run's own checkpoint records",
                        sr::display_path(root)
                    )))
                }
                Err(e) => {
                    return Err(unknown(format!(
                        "the terminal root {} could not be asked about the checkpoint base {base}: \
                         {e}",
                        sr::display_path(root)
                    )))
                }
            }
        } else {
            base_note = format!(
                "checkpoint base {base:?} resolves nowhere in the clone, so it corroborates \
                         nothing and was not used"
            );
        }
    }
    Ok(format!(
        "terminal root corroborated: {} holds this clone's lineage root {matched} ({base_note})",
        sr::display_path(root)
    ))
}

/// Everything about a clone's `origin` that is established BEFORE any deletion happens (R6).
///
/// The chain is walked during ADMISSION, not at the boundary, and that ordering is the whole point: a
/// parent clone and the child whose `origin` names it can both be candidates in one invocation, and
/// removing the parent first used to leave the child unable to resolve anything — permanently, since
/// nothing ever rewrites a dead `origin`. Resolving first makes deletion order irrelevant, because the
/// terminal root is OUTSIDE the scan root by construction and therefore survives every sibling
/// removal. The boundary re-verifies the root's identity before trusting the cached answer.
#[derive(Clone, Debug)]
pub struct OriginResolution {
    /// The walk's outcome, cached from admission.
    pub chain: Result<OriginChain, rp::ParkReason>,
    /// R5. The clone's OWN `origin` as an existing local directory, whatever became of the rest of
    /// the walk. This is what [`removal_guard`] needs for its "the source is neither the clone nor
    /// inside it" clause, and it is `None` only when that directory is genuinely absent — the
    /// parent-already-reaped case, where there is no source for a removal to endanger.
    pub immediate: Option<PathBuf>,
    /// dev/ino of the terminal root when it was resolved, so the boundary can prove it is the same
    /// directory.
    pub root_identity: Option<(u64, u64)>,
    /// The first hop as resolved at admission, re-checked at the boundary: `origin` lives inside the
    /// `:rw` mount, and the walk above ran before the operation lock was taken.
    pub first_hop: Option<PathBuf>,
}

/// Walk and cache one clone's origin chain. Runs during admission, under no lock, changing nothing.
pub fn resolve_origin(clone: &Path, scan_root: &Path) -> OriginResolution {
    let chain = resolve_origin_root(clone, scan_root);
    let immediate = immediate_origin(clone);
    let root_identity = chain
        .as_ref()
        .ok()
        .and_then(|c| rp::dir_dev_ino(&c.root).ok());
    OriginResolution {
        first_hop: immediate.clone(),
        chain,
        immediate,
        root_identity,
    }
}

/// The clone's own `remote.origin.url` as an EXISTING local directory, or `None`.
fn immediate_origin(clone: &Path) -> Option<PathBuf> {
    let url = sr::git_str(clone, &["config", "--get", "remote.origin.url"]).ok()?;
    if url.is_empty() {
        return None;
    }
    sr::local_source_path(clone, &url)
}

// ---------------------------------------------------------------------------------------------
// The boundary git derivation
// ---------------------------------------------------------------------------------------------

/// Everything the D-1 gate established about one clone, re-derived at the boundary.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CloneFacts {
    pub branch: Option<String>,
    pub head: String,
    pub tree: Option<String>,
    /// The TRUE source repository — the end of the F1 `origin` chain. `None` only on the F2 licensed
    /// path, where no containment query is asked and the source may not exist any more.
    pub source_repo: Option<PathBuf>,
    /// F1: every `origin` edge walked to reach `source_repo`.
    pub origin_chain: Vec<String>,
    /// The fully-qualified branch the containment verdict was computed against, and its OID at that
    /// moment — the identity of the history the proof rests on, re-checked after every probe. `None`
    /// on the F2 licensed path: no history was searched, so none is claimed.
    pub main_ref_full: Option<String>,
    pub main_oid: Option<String>,
    pub containment: ContainmentEvidence,
    /// R2: on the licensed path, every ref the deletion took — recorded, never judged. Empty on the
    /// proved path, where [`refs_disposition`] gated each tip instead.
    pub ref_inventory: Vec<RefFact>,
    /// The gate evidence, in evaluation order.
    pub gates: Vec<String>,
}

/// A refusal detail, annotated when the run carries an owner disposition license (F2).
///
/// The license replaces the CONTENT-ON-MAIN proof and nothing else, and the gate that just refused is
/// something else. Saying so on the refusal itself is the difference between an operator concluding
/// "my list did not work" and reading what actually happened.
fn licensed_conflict(licensed: Option<&DispositionList>, detail: String) -> String {
    match licensed {
        None => detail,
        Some(d) => format!(
            "{detail} — NOTE: this run IS on the owner disposition list {} (authorized by {:?}), but \
             that license replaces the content-on-main proof ONLY. It says the RUN is abandoned; it \
             cannot say that bytes on no commit are recoverable, and nobody has read them. \
             Fail-closed: the clone is kept.",
            d.list_file, d.authorized_by
        ),
    }
}

/// Re-derive shape, git state and containment for `clone`, refusing with a typed park reason.
///
/// The whole derivation is bracketed by a `.git` [`sr::ShapeFingerprint`] check on BOTH sides: shape
/// alone is not identity, and one `.git` directory swapped for another mid-probe would have every
/// answer below describing a different repository.
///
/// `disposition` is the F2 license, if the operator supplied one. When it names this `run_id`, the
/// content-on-main block (source resolution, containment, the ref sweep, the moving-history recheck)
/// is REPLACED by the license — and nothing else is. Every gate before it still runs and still refuses.
pub fn derive_clone_facts(
    clone: &Path,
    run_id: &str,
    resolution: &OriginResolution,
    lookback: u32,
    disposition: Option<&DispositionList>,
) -> Result<CloneFacts, rp::ParkReason> {
    let mut gates: Vec<String> = Vec::new();
    let licensed = disposition.filter(|d| d.licenses(run_id));

    // 1. Shape: standalone-clone `.git` DIRECTORY semantics, proved on disk.
    let before = sr::shape_fingerprint(clone);
    let (class, kind, note) =
        sr::classify_checkout(&before.shape, sr::CheckoutKind::StandaloneClone);
    if class != sr::PayloadClass::SourceCheckout || kind != Some(sr::CheckoutKind::StandaloneClone)
    {
        return Err(rp::ParkReason::NotAStandaloneClone {
            detail: licensed_conflict(
                licensed,
                note.unwrap_or_else(|| format!("unrecognized `.git` shape: {:?}", before.shape)),
            ),
        });
    }
    gates.push(format!(
        "shape: `.git` is a real DIRECTORY (standalone clone with its own object store), identity {}",
        match before.dev_ino {
            Some((d, i)) => format!("dev {d} / ino {i}"),
            None => "unavailable".to_string(),
        }
    ));

    // 2. HEAD. `symbolic-ref` first: it resolves on an unborn HEAD, which `rev-parse HEAD` cannot.
    let branch = sr::git_str(clone, &["symbolic-ref", "--quiet", "--short", "HEAD"]).ok();
    let unborn = || rp::ParkReason::UnbornHead {
        detail: licensed
            .map(|_| licensed_conflict(licensed, "HEAD is unborn".to_string()))
            .map(|d| d.trim_start_matches("HEAD is unborn — ").to_string()),
    };
    let head = match sr::git_str(clone, &["rev-parse", "HEAD"]) {
        Ok(h) if !h.is_empty() => h,
        Ok(_) => return Err(unborn()),
        Err(e) => {
            if branch.is_some() {
                return Err(unborn());
            }
            return Err(rp::ParkReason::GitStateUnknown {
                detail: licensed_conflict(licensed, format!("HEAD unresolvable: {e}")),
            });
        }
    };

    // 3. Git state. `--ignored=traditional` collapses ignored DIRECTORIES to one entry (a `-uall`
    //    expansion over a cargo target is millions of lines); `--ignore-submodules=none` is explicit so
    //    a dirty submodule cannot hide behind a default.
    let status = match sr::git_ro(
        clone,
        &[
            "status",
            "--porcelain",
            "--ignored=traditional",
            "--ignore-submodules=none",
        ],
    ) {
        Ok(out) if out.status.success() => String::from_utf8_lossy(&out.stdout).into_owned(),
        Ok(out) => {
            return Err(rp::ParkReason::GitStateUnknown {
                detail: licensed_conflict(
                    licensed,
                    format!(
                        "`git status` exited {:?}: {}",
                        out.status.code(),
                        String::from_utf8_lossy(&out.stderr).trim()
                    ),
                ),
            })
        }
        Err(e) => {
            return Err(rp::ParkReason::GitStateUnknown {
                detail: licensed_conflict(licensed, format!("`git status` could not run: {e}")),
            })
        }
    };
    match status_disposition(&status, clone) {
        Ok(summary) => gates.push(format!("git state: {summary}")),
        Err(detail) => {
            return Err(rp::ParkReason::GitStateNotClean {
                detail: licensed_conflict(licensed, detail),
            })
        }
    }

    // 3b. What `git status` CANNOT see. `--assume-unchanged` and `--skip-worktree` tell git to stop
    //     consulting the worktree for those paths, so modified tracked bytes produce no porcelain line
    //     at all; a sparse checkout does the same wholesale. `ls-files -v` is the only view that shows
    //     the flags themselves.
    match sr::git_str(clone, &["ls-files", "-v"]) {
        Ok(listing) => match index_flags_disposition(&listing) {
            Ok(summary) => gates.push(format!("index flags: {summary}")),
            Err(detail) => {
                return Err(rp::ParkReason::IndexFlagsHideState {
                    detail: licensed_conflict(licensed, detail),
                })
            }
        },
        Err(e) => {
            return Err(rp::ParkReason::GitStateUnknown {
                detail: licensed_conflict(
                    licensed,
                    format!("`git ls-files -v` could not answer: {e}"),
                ),
            })
        }
    }
    if let Some(detail) = sparse_checkout_indicator(clone) {
        return Err(rp::ParkReason::IndexFlagsHideState {
            detail: licensed_conflict(licensed, detail),
        });
    }

    // 3c. Submodules. A CLEAN submodule emits no porcelain entry, yet its object store lives in
    //     `<clone>/.git/modules/<name>` and dies with the clone — and the superproject's gitlink is a
    //     SHA, which says where the bytes should be, never that they are anywhere else. Fail-closed:
    //     any initialized submodule parks. (This workspace has none, so the rule costs nothing here.)
    match submodule_state(clone) {
        Ok(summary) => gates.push(format!("submodules: {summary}")),
        Err(detail) => {
            return Err(rp::ParkReason::InitializedSubmodule {
                detail: licensed_conflict(licensed, detail),
            })
        }
    }

    // The clone's own tree id, needed by both paths below (it is the pre-squash identity the fold
    // receipt exists to preserve, whether or not anyone proved where it landed).
    let tree = sr::git_str(clone, &["rev-parse", &format!("{head}^{{tree}}")]).ok();

    // 4–5c. THE CONTENT-ON-MAIN GATE — or, for a run the owner has dispositioned (F2), the license
    //       that REPLACES it. Everything in this block, and only this block, is what the license
    //       stands in for: resolving the source repository, the D-1 verdict, the ref sweep, and the
    //       moving-history recheck. Every gate above already ran unconditionally, and every gate in
    //       `reap_one` below still runs.
    let source_repo: Option<PathBuf>;
    let origin_chain: Vec<String>;
    let mut main_full_ref: Option<String> = None;
    let mut main_oid_before: Option<String> = None;
    let mut ref_facts: Vec<RefFact> = Vec::new();
    let containment: ContainmentEvidence;

    if let Some(d) = licensed {
        containment = d.evidence(run_id);
        // R5. The chain is cached from admission and is BEST-EFFORT here: it is recorded and, when it
        // resolved, handed to the removal guard — but it can never park a licensed clone. A chained
        // clone whose parent was reaped by an EARLIER invocation has a dead `origin` by construction
        // and nothing will ever rewrite it; that population is exactly what a disposition clears.
        //
        // When the walk failed but the clone's own `origin` still names an existing directory, the
        // guard gets THAT: dropping its "the source is neither the clone nor inside it" clause is
        // only honest when there is no source directory at all.
        match &resolution.chain {
            Ok(chain) => {
                source_repo = Some(chain.root.clone());
                origin_chain = chain.hops.clone();
            }
            Err(reason) => {
                source_repo = resolution.immediate.clone();
                origin_chain = vec![format!(
                    "origin chain UNRESOLVED and not required: {} (recorded, not refused — the \
                     license does not rest on a source repository){}",
                    reason.summary(),
                    match &resolution.immediate {
                        Some(p) => format!(
                            ". The clone's own `origin` still names the existing directory {}, which \
                             the removal guard was given",
                            sr::display_path(p)
                        ),
                        None => ". The clone's own `origin` names no existing directory either, so \
                                 the removal guard has no source to protect"
                            .to_string(),
                    }
                )];
            }
        }
        gates.push(format!(
            "content on main (D-1): NOT PROVED AND NOT CLAIMED. Replaced by the OWNER DISPOSITION \
             LICENSE — {}",
            containment.detail
        ));
        // R2. The gating sweep is replaced; the INVENTORY is not. What the deletion takes has to stay
        // enumerable from the receipt once the object store is gone.
        let (facts, note) = ref_inventory(clone, &head);
        gates.push(ref_inventory_summary(&facts, note.as_deref()));
        ref_facts = facts;
        gates.push(format!(
            "source-main identity: not read — it is part of the content-on-main proof this license \
             replaces. Origin chain: [{}]",
            origin_chain.join(" | ")
        ));
    } else {
        // 4. The local source repository, RESOLVED BEFORE ANY DELETION (R6) and re-verified here. S4
        //    read the clone's OWN `origin` and stopped there; S4b/F1 follows the chain out of the
        //    scan root, because an `origin` that names a SIBLING clone would have the D-1 question
        //    asked of a quarantine directory. Hosted or absent origin on the clone itself still
        //    parks: no network is ever contacted.
        let chain = match &resolution.chain {
            Ok(c) => c.clone(),
            Err(reason) => return Err(reason.clone()),
        };
        let resolved = chain.root.clone();

        // 4a. The cached answer must still describe the repository now on disk. The walk ran during
        //     admission, before this run's operation lock was held, and `origin` lives inside the
        //     `:rw` mount — so both ends are re-checked: the clone's first hop, and the terminal
        //     root's directory identity.
        match (resolution.root_identity, rp::dir_dev_ino(&resolved)) {
            (Some(before), Ok(now)) if before == now => {}
            (Some(before), Ok(now)) => {
                return Err(rp::ParkReason::SourceRootIdentityChanged {
                    detail: format!(
                    "{} was dev/ino {}/{} when the chain was resolved and {}/{} at the boundary",
                    sr::display_path(&resolved),
                    before.0,
                    before.1,
                    now.0,
                    now.1
                ),
                })
            }
            (_, Err(e)) => {
                return Err(rp::ParkReason::SourceRootIdentityChanged {
                    detail: format!(
                        "{} is no longer a readable directory: {e}",
                        resolved.display()
                    ),
                })
            }
            (None, _) => {
                return Err(rp::ParkReason::SourceRootIdentityChanged {
                    detail: format!(
                        "{} had no readable identity when the chain was resolved",
                        resolved.display()
                    ),
                })
            }
        }
        // The first hop is re-read, but only a hop that now names a DIFFERENT EXISTING directory is a
        // refusal. A hop that resolves to NOTHING is the ordinary R6 case — this invocation just
        // reaped the parent clone — and it cannot redirect anything: the verdict is computed against
        // the cached terminal root, whose own identity was proved immediately above. Parking on it
        // would reinstate exactly the stranding R6 exists to remove.
        let now_first = immediate_origin(clone);
        match (&now_first, &resolution.first_hop) {
            (Some(now), Some(before)) if now != before => {
                return Err(rp::ParkReason::SourceRootIdentityChanged {
                    detail: format!(
                        "the clone's `remote.origin.url` resolved to {} when the chain was walked \
                         and {} at the boundary",
                        sr::display_path(before),
                        sr::display_path(now)
                    ),
                })
            }
            (Some(_), None) => {
                return Err(rp::ParkReason::SourceRootIdentityChanged {
                    detail: format!(
                    "the clone's `remote.origin.url` named no existing directory when the chain \
                         was walked and names {} at the boundary",
                    sr::display_path(now_first.as_deref().unwrap_or(Path::new("?")))
                ),
                })
            }
            _ => {}
        }

        // 4b. R4 — corroborate the terminal root against the clone's OWN history before asking it
        //     anything. A chain hop whose run has no checkpoint contributes no cross-check, so this
        //     is what stands between a rewritten `origin` and a verdict from a foreign repository.
        let corroboration = corroborate_terminal_root(clone, &head, &resolved, &chain)?;
        gates.push(format!(
            "origin chain (F1): {} hop(s) followed to the source repository {}, which is OUTSIDE the \
             pinned scan root — [{}]. Every hop was a local directory and a real git repository, each \
             agreed with its own run's checkpoint where one exists, the chain was resolved BEFORE any \
             deletion in this run and the terminal root re-verified unchanged at this boundary{}, \
             and {corroboration}",
            chain.hops.len(),
            sr::display_path(&resolved),
            chain.hops.join(" | "),
            if now_first.is_none() && resolution.first_hop.is_some() {
                " (the first hop no longer exists — a sibling clone this same invocation reaped; the \
                 verdict rests on the terminal root, which is outside the scan root and therefore \
                 untouched)"
            } else {
                ""
            }
        ));

        // 5. Containment — the D-1 gate, asked of the TRUE source. `no` and `unknown` BOTH park. The
        //    main ref is resolved as a FULLY QUALIFIED branch and its OID is read before AND after the
        //    probes: a verdict computed while main moved underneath is a verdict about no single
        //    history.
        let full_ref = sr::resolve_source_main(&resolved).map_err(|reason| {
            rp::ParkReason::NotOnSourceMain {
                verdict: "unknown".into(),
                detail: format!("source main unresolvable: {reason}"),
            }
        })?;
        let oid_before =
            sr::ref_oid(&resolved, &full_ref).map_err(|e| rp::ParkReason::NotOnSourceMain {
                verdict: "unknown".into(),
                detail: format!("source main {full_ref} has no readable commit: {e}"),
            })?;
        let (main_ref, verdict) =
            sr::on_source_main_with_lookback(&resolved, clone, &head, lookback);
        containment = match &verdict {
            sr::OnSourceMain::YesHead => ContainmentEvidence::proved(
                verdict.label(),
                main_ref.clone(),
                Some(head.clone()),
                format!(
                    "HEAD {head} is an ancestor of {}'s {}",
                    sr::display_path(&resolved),
                    main_ref.clone().unwrap_or_else(|| "main".into()),
                ),
            ),
            sr::OnSourceMain::YesTree { commit } => ContainmentEvidence::proved(
                verdict.label(),
                main_ref.clone(),
                Some(commit.clone()),
                format!(
                    "HEAD's exact tree is the tree of {commit} on {}'s {} (squash landing; the \
                     commit id differs, the content is byte-identical)",
                    sr::display_path(&resolved),
                    main_ref.clone().unwrap_or_else(|| "main".into())
                ),
            ),
            sr::OnSourceMain::No => {
                return Err(rp::ParkReason::NotOnSourceMain {
                    verdict: verdict.label(),
                    detail: format!(
                        "neither HEAD nor its exact tree is on {}'s {} within the {lookback}-commit \
                         lookback, and that history is EXHAUSTED (so the search was complete). This \
                         covers both never-landed work and a squash that REWROTE the tree; either \
                         way the clone is kept (fail-closed by design)",
                        sr::display_path(&resolved),
                        main_ref.clone().unwrap_or_else(|| "main".into())
                    ),
                })
            }
            sr::OnSourceMain::Unknown { reason } => {
                return Err(rp::ParkReason::NotOnSourceMain {
                    verdict: verdict.label(),
                    detail: reason.clone(),
                })
            }
        };
        gates.push(format!(
            "content on main (D-1): {} — {}",
            containment.verdict, containment.detail
        ));

        // 5b. EVERY OTHER REF, against the SAME true source. Containment above proves HEAD; the
        //     removal takes the whole object store. Production leaves exactly these shapes behind:
        //     `head_guard` refuses to advance and LEAVES the clone when an agent commits or switches
        //     branch, and `restore_branch` puts HEAD back without deleting the branch the agent made.
        //     A stash is constructible in any `:rw` clone and hangs off `refs/stash` alone.
        let refs_summary = refs_disposition(clone, &head, &resolved, lookback)?;
        gates.push(format!("refs: {refs_summary}"));

        // 5c. Source main must not have moved under the probes.
        let oid_after =
            sr::ref_oid(&resolved, &full_ref).map_err(|e| rp::ParkReason::SourceMainMoved {
                detail: format!("{full_ref} became unreadable during the probes: {e}"),
            })?;
        if oid_after != oid_before {
            return Err(rp::ParkReason::SourceMainMoved {
                detail: format!(
                    "{full_ref} was {oid_before} when the containment search began and {oid_after} \
                     when it ended"
                ),
            });
        }
        gates.push(format!(
            "source main identity: {full_ref} @ {oid_before}, unchanged across every containment \
             probe"
        ));
        main_full_ref = Some(full_ref);
        main_oid_before = Some(oid_before);
        source_repo = Some(resolved);
        origin_chain = chain.hops;
    }

    // 6. Re-confirm `.git` identity: the swap could have landed while git was running, and every answer
    //    above would then describe a different repository.
    let after = sr::shape_fingerprint(clone);
    if after != before {
        return Err(rp::ParkReason::GitIdentityChanged {
            detail: format!(
                "expected {:?}/{:?}, saw {:?}/{:?}",
                before.shape, before.dev_ino, after.shape, after.dev_ino
            ),
        });
    }
    gates.push("`.git` identity: re-verified unchanged across every git probe".to_string());

    Ok(CloneFacts {
        branch,
        head,
        tree,
        source_repo,
        origin_chain,
        main_ref_full: main_full_ref,
        main_oid: main_oid_before,
        containment,
        ref_inventory: ref_facts,
        gates,
    })
}

/// F1. Every ref of the clone must be accounted for before its object store is destroyed.
///
/// `refs/heads/*`, `refs/tags/*` and `refs/stash` are swept. A tip passes when it IS `head`, is an
/// ANCESTOR of `head` (its commits are inside the history containment already proved), or is
/// INDEPENDENTLY on source main by the same head/tree test. Anything else parks, naming the ref.
///
/// `refs/remotes/*` are deliberately not swept: they are the frozen snapshot of what `origin` handed
/// this clone at clone time, so their objects came FROM the source repository rather than being unique
/// to the clone.
///
/// Out of scope, by design: objects reachable from NO ref (e.g. the re-authored commit an `Unlanded`
/// merge leaves dangling). Those are deterministic recomputations of inputs this command preserves —
/// the clone's HEAD, its branch and its checkpoint — not unique custody, and `git gc` would drop them
/// in the source repository too.
fn refs_disposition(
    clone: &Path,
    head: &str,
    source_repo: &Path,
    lookback: u32,
) -> Result<String, rp::ParkReason> {
    let listing = sr::git_str(
        clone,
        &[
            "for-each-ref",
            "--format=%(refname)",
            "refs/heads/",
            "refs/tags/",
            "refs/stash",
        ],
    )
    .map_err(|e| rp::ParkReason::GitStateUnknown {
        detail: format!("`git for-each-ref` could not enumerate this clone's refs: {e}"),
    })?;

    let mut checked = 0usize;
    let mut via_head = 0usize;
    let mut via_ancestry = 0usize;
    let mut via_main = 0usize;
    for refname in listing.lines().map(str::trim).filter(|l| !l.is_empty()) {
        checked += 1;
        // Peel to a commit. A tag on a blob or tree cannot be judged by a commit-containment test at
        // all, so it parks rather than being waved through.
        let oid = sr::git_str(
            clone,
            &["rev-parse", "--verify", &format!("{refname}^{{commit}}")],
        )
        .map_err(|e| rp::ParkReason::RefsNotContained {
            detail: format!("{refname} does not resolve to a commit ({e}) — it cannot be judged"),
        })?;
        if oid == head {
            via_head += 1;
            continue;
        }
        match sr::git_ro(clone, &["merge-base", "--is-ancestor", &oid, head]) {
            Ok(out) if out.status.code() == Some(0) => {
                via_ancestry += 1;
                continue;
            }
            Ok(out) if out.status.code() == Some(1) => {}
            Ok(out) => {
                return Err(rp::ParkReason::RefsNotContained {
                    detail: format!(
                        "{refname} ({oid}): ancestry test exited {:?} — not interpretable",
                        out.status.code()
                    ),
                })
            }
            Err(e) => {
                return Err(rp::ParkReason::RefsNotContained {
                    detail: format!("{refname} ({oid}): ancestry test could not run: {e}"),
                })
            }
        }
        // Not inside HEAD's history: ask the D-1 question of this tip on its own.
        let (_, verdict) = sr::on_source_main_with_lookback(source_repo, clone, &oid, lookback);
        if verdict.is_landed() {
            via_main += 1;
            continue;
        }
        return Err(rp::ParkReason::RefsNotContained {
            detail: format!(
                "{refname} ({oid}) is neither HEAD, nor an ancestor of HEAD, nor on source main \
                 (verdict {}) — deleting the clone would destroy the only copy of its commits",
                verdict.label()
            ),
        });
    }
    Ok(format!(
        "{checked} ref(s) swept (refs/heads/*, refs/tags/*, refs/stash): {via_head} is HEAD, \
         {via_ancestry} ancestor(s) of HEAD, {via_main} independently on source main"
    ))
}

/// R2. Enumerate every ref of `clone` WITHOUT judging any of them.
///
/// Same sweep set as [`refs_disposition`] (`refs/heads/*`, `refs/tags/*`, `refs/stash`) and
/// deliberately the same shape of probe, so the inventory an owner-licensed deletion leaves behind is
/// directly comparable with the refusal a gated one would have produced. It NEVER returns an error:
/// every failure becomes a recorded `None` plus a note. A probe that cannot answer must not turn a
/// licensed deletion into a park — the license already settled whether the run dies.
///
/// Needs only the clone, so it works on the licensed path where no source repository was resolved.
fn ref_inventory(clone: &Path, head: &str) -> (Vec<RefFact>, Option<String>) {
    let listing = match sr::git_str(
        clone,
        &[
            "for-each-ref",
            "--format=%(refname)",
            "refs/heads/",
            "refs/tags/",
            "refs/stash",
        ],
    ) {
        Ok(l) => l,
        Err(e) => {
            return (
                Vec::new(),
                Some(format!(
                "the refs of this clone could not be enumerated ({e}), so the inventory below is \
                     EMPTY because nothing could be read — not because the clone had no refs"
            )),
            )
        }
    };
    let mut facts = Vec::new();
    let mut unreadable = 0usize;
    for refname in listing.lines().map(str::trim).filter(|l| !l.is_empty()) {
        let oid = sr::git_str(
            clone,
            &["rev-parse", "--verify", &format!("{refname}^{{commit}}")],
        )
        .ok();
        let Some(oid) = oid else {
            unreadable += 1;
            facts.push(RefFact {
                refname: refname.to_string(),
                oid: None,
                is_head: false,
                is_ancestor_of_head: None,
            });
            continue;
        };
        let is_head = oid == head;
        let is_ancestor_of_head = if is_head {
            Some(true)
        } else {
            match sr::git_ro(clone, &["merge-base", "--is-ancestor", &oid, head]) {
                Ok(out) if out.status.code() == Some(0) => Some(true),
                Ok(out) if out.status.code() == Some(1) => Some(false),
                _ => None,
            }
        };
        facts.push(RefFact {
            refname: refname.to_string(),
            oid: Some(oid),
            is_head,
            is_ancestor_of_head,
        });
    }
    let note = (unreadable > 0).then(|| {
        format!(
            "{unreadable} ref(s) could not be peeled to a commit and are recorded without an oid"
        )
    });
    (facts, note)
}

/// A one-line human summary of an inventory, for the gate text: the tips that were NOT inside HEAD's
/// history are named individually, because those are the ones whose commits the deletion took with it.
fn ref_inventory_summary(facts: &[RefFact], note: Option<&str>) -> String {
    let outside: Vec<String> = facts
        .iter()
        .filter(|f| !f.is_head && f.is_ancestor_of_head != Some(true))
        .map(|f| {
            format!(
                "{} ({})",
                f.refname,
                f.oid.clone().unwrap_or_else(|| "unpeelable".into())
            )
        })
        .collect();
    format!(
        "ref inventory (RECORDED, not gated — the owner disposition replaces the ref sweep): {} \
         ref(s) existed; {} carried commits outside HEAD's history and went with the clone{}{}",
        facts.len(),
        outside.len(),
        if outside.is_empty() {
            String::new()
        } else {
            format!(": {}", outside.join(", "))
        },
        note.map(|n| format!(". {n}")).unwrap_or_default()
    )
}

/// PURE. `git ls-files -v` marks every index entry with a status letter; `H` is the ordinary "cached"
/// state. A LOWERCASE letter means `--assume-unchanged` and `S` means `--skip-worktree` — both tell git
/// to stop consulting the worktree for that path, so modified bytes there produce NO porcelain line.
/// Anything that is not `H` therefore parks, including letters this parser does not recognize.
pub fn index_flags_disposition(listing: &str) -> Result<String, String> {
    let mut flagged: Vec<String> = Vec::new();
    let mut total = 0usize;
    let mut entries = 0usize;
    for line in listing.lines() {
        if line.trim().is_empty() {
            continue;
        }
        entries += 1;
        let first = line.chars().next().unwrap_or('?');
        if first == 'H' {
            continue;
        }
        total += 1;
        if flagged.len() < MAX_QUOTED_STATUS_ENTRIES {
            flagged.push(line.trim().to_string());
        }
    }
    if total > 0 {
        return Err(format!(
            "{total} index entry/entries carry a non-`H` flag (assume-unchanged, skip-worktree, \
             unmerged or sparse), so `git status` is blind to their worktree bytes — first {}: {}",
            flagged.len(),
            flagged.join(" | ")
        ));
    }
    Ok(format!(
        "{entries} index entr(y/ies), all plain `H` (cached)"
    ))
}

/// Is this clone sparse? A sparse checkout is a whole-tree version of the same blindness: paths outside
/// the cone are simply not in the worktree, and `git status` reports nothing about them.
fn sparse_checkout_indicator(clone: &Path) -> Option<String> {
    for key in ["core.sparseCheckout", "index.sparse"] {
        if let Ok(v) = sr::git_str(clone, &["config", "--get", key]) {
            if v.trim().eq_ignore_ascii_case("true") {
                return Some(format!(
                    "`{key}` is true — this is a sparse checkout, and `git status` says nothing about \
                     paths outside the cone"
                ));
            }
        }
    }
    let file = clone.join(".git").join("info").join("sparse-checkout");
    sr::real_file(&file).then(|| {
        format!(
            "{} exists — this clone has a sparse-checkout pattern set",
            file.display()
        )
    })
}

/// PURE. `git submodule status` marks each submodule: `-` = NOT initialized (no object store here),
/// anything else (` `, `+`, `U`) = initialized. Fail-closed: an unparseable line is treated as
/// initialized, because "I could not tell" is not "there is nothing there".
pub fn submodule_disposition(status: &str) -> Result<String, String> {
    let mut initialized: Vec<String> = Vec::new();
    let mut uninitialized = 0usize;
    for line in status.lines() {
        if line.trim().is_empty() {
            continue;
        }
        if line.starts_with('-') {
            uninitialized += 1;
            continue;
        }
        if initialized.len() < MAX_QUOTED_STATUS_ENTRIES {
            initialized.push(line.trim().to_string());
        } else {
            initialized.push("…".into());
        }
    }
    if !initialized.is_empty() {
        return Err(format!(
            "{} initialized submodule(s) whose object stores live under `.git/modules` and would be \
             deleted with this clone: {}",
            initialized.len(),
            initialized.join(" | ")
        ));
    }
    Ok(format!(
        "none initialized ({uninitialized} declared but uninitialized)"
    ))
}

/// FS + git. The submodule gate: a non-empty `.git/modules` is decisive on its own (those ARE the
/// object stores), and a declared `.gitmodules` is checked with `git submodule status`.
fn submodule_state(clone: &Path) -> Result<String, String> {
    let modules = clone.join(".git").join("modules");
    if sr::real_dir(&modules) {
        let empty = std::fs::read_dir(&modules)
            .map(|mut d| d.next().is_none())
            .map_err(|e| format!("{} is unreadable: {e}", modules.display()))?;
        if !empty {
            return Err(format!(
                "{} holds submodule object stores that exist nowhere else",
                modules.display()
            ));
        }
    }
    if !sr::real_file(&clone.join(".gitmodules")) {
        return Ok("no `.gitmodules`, no `.git/modules` content".to_string());
    }
    let status = sr::git_str(clone, &["submodule", "status"])
        .map_err(|e| format!("`.gitmodules` is present and `git submodule status` failed: {e}"))?;
    submodule_disposition(&status)
}

// ---------------------------------------------------------------------------------------------
// Evidence preservation: the structural preflight
// ---------------------------------------------------------------------------------------------

/// The Evidence a run carries OUTSIDE its worktree, and therefore outside every git gate above:
/// the sidecar directory plus the two `.git`-scoped files the implement loop writes.
/// `.git/A2A_TASK.md` is the out-of-band task the fix loop re-reads (tweak.rs) and
/// `.git/A2A_COMMIT_MSG` is the agent-written hand-off message (implement.rs).
pub const GIT_SCOPED_EVIDENCE: [&str; 2] = ["A2A_TASK.md", "A2A_COMMIT_MSG"];

/// What the preflight found: exactly which paths will be preserved, and a human summary.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EvidencePlan {
    pub sources: Vec<PathBuf>,
    pub summary: String,
}

/// PURE-ish (FS reads only), NON-MUTATING. Establish that this run's Evidence can be preserved
/// safely — before any deletion, and cheaply enough to run in a `--dry-run` plan.
///
/// It exists because "no evidence to copy" and "evidence I cannot interpret" are different facts with
/// opposite consequences, and the earlier shape returned `Ok(empty)` for both. Every refusal here is
/// AMBIGUITY, never absence:
///
/// - the receipt namespace `<root>/.receipts`, if it exists, must be a real directory whose canonical
///   parent IS the pinned scan root — a symlinked `.receipts` would put the record somewhere the
///   deletion destroys, or somewhere it does not belong;
/// - the sidecar `<clone>/.git/a2a-bridge`, if present, must be a real directory containing only real
///   files and directories. A symlink is ambiguous on both sides (following it copies whatever a `:rw`
///   container aimed it at; skipping it deletes the clone while claiming preservation), and a FIFO or
///   socket would block a copy that tried to open it;
/// - the `.git`-scoped evidence files, if present, must be regular files.
///
/// A genuinely ABSENT sidecar is fine and says so in the summary.
pub fn evidence_preflight(clone: &Path, root: &Path) -> Result<EvidencePlan, rp::ParkReason> {
    let ambiguous = |detail: String| rp::ParkReason::EvidencePreservationFailed { detail };

    // 1. The destination namespace.
    let receipts = receipts_dir(root);
    match std::fs::symlink_metadata(&receipts) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => {
            return Err(ambiguous(format!(
                "{} is unreadable: {e}",
                receipts.display()
            )))
        }
        Ok(md) if md.file_type().is_symlink() => {
            return Err(ambiguous(format!(
            "{} is a SYMLINK — the receipt namespace must be a real directory directly under the \
                 pinned scan root, or a receipt could be written into the very tree being deleted",
            receipts.display()
        )))
        }
        Ok(md) if !md.is_dir() => {
            return Err(ambiguous(format!(
                "{} exists and is not a directory",
                receipts.display()
            )))
        }
        Ok(_) => {
            let canon = std::fs::canonicalize(&receipts).map_err(|e| {
                ambiguous(format!("{} has no canonical path: {e}", receipts.display()))
            })?;
            let root_canon = std::fs::canonicalize(root)
                .map_err(|e| ambiguous(format!("{} has no canonical path: {e}", root.display())))?;
            if canon.parent() != Some(root_canon.as_path()) {
                return Err(ambiguous(format!(
                    "{} resolves to {}, which is not directly under the pinned scan root {}",
                    receipts.display(),
                    canon.display(),
                    root_canon.display()
                )));
            }
        }
    }

    // 2. The sidecar.
    let mut sources = Vec::new();
    let sidecar = sr::evidence_dir(clone);
    let mut sidecar_note = format!("no `{}` sidecar (nothing to preserve)", sidecar.display());
    match std::fs::symlink_metadata(&sidecar) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => {
            return Err(ambiguous(format!(
                "{} is unreadable: {e}",
                sidecar.display()
            )))
        }
        Ok(md) if md.file_type().is_symlink() => {
            return Err(ambiguous(format!(
                "{} is a SYMLINK — a link here is ambiguous in both directions (following it would \
                 preserve whatever it points at as this run's evidence; skipping it would delete the \
                 clone while reporting evidence preserved)",
                sidecar.display()
            )))
        }
        Ok(md) if !md.is_dir() => {
            return Err(ambiguous(format!(
                "{} is not a directory",
                sidecar.display()
            )))
        }
        Ok(_) => {
            let count = check_evidence_tree(&sidecar)?;
            sidecar_note = format!("sidecar {} ({count} entr(y/ies))", sidecar.display());
            sources.push(sidecar);
        }
    }

    // 3. The `.git`-scoped evidence files.
    let mut extras = Vec::new();
    for name in GIT_SCOPED_EVIDENCE {
        let p = clone.join(".git").join(name);
        match std::fs::symlink_metadata(&p) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(ambiguous(format!("{} is unreadable: {e}", p.display()))),
            Ok(md) if md.is_file() && !md.file_type().is_symlink() => {
                extras.push(name);
                sources.push(p);
            }
            Ok(_) => {
                return Err(ambiguous(format!(
                    "{} exists but is not a regular file",
                    p.display()
                )))
            }
        }
    }

    Ok(EvidencePlan {
        summary: format!(
            "{sidecar_note}; `.git`-scoped evidence: {}",
            if extras.is_empty() {
                "none".to_string()
            } else {
                extras.join(", ")
            }
        ),
        sources,
    })
}

/// Every entry under an evidence tree must be a real file or a real directory. Returns how many were
/// seen; refuses anything it cannot copy safely.
fn check_evidence_tree(dir: &Path) -> Result<usize, rp::ParkReason> {
    let ambiguous = |detail: String| rp::ParkReason::EvidencePreservationFailed { detail };
    let mut seen = 0usize;
    let entries = std::fs::read_dir(dir)
        .map_err(|e| ambiguous(format!("{} is unreadable: {e}", dir.display())))?;
    for entry in entries {
        let entry =
            entry.map_err(|e| ambiguous(format!("{} is unreadable: {e}", dir.display())))?;
        let p = entry.path();
        if entry.file_name().to_str().is_none() {
            return Err(ambiguous(format!(
                "{} has a non-UTF-8 name — refusing to preserve it under a guessed name",
                p.to_string_lossy()
            )));
        }
        let md = std::fs::symlink_metadata(&p)
            .map_err(|e| ambiguous(format!("{} is unreadable: {e}", p.display())))?;
        if md.file_type().is_symlink() {
            return Err(ambiguous(format!(
                "{} is a symlink — never followed, and never silently skipped",
                p.display()
            )));
        }
        if md.is_dir() {
            seen += check_evidence_tree(&p)?;
            continue;
        }
        if !md.is_file() {
            return Err(ambiguous(format!(
                "{} is neither a regular file nor a directory (FIFO, socket or device) — it cannot be \
                 preserved, and skipping it would delete the clone while claiming it was",
                p.display()
            )));
        }
        seen += 1;
    }
    Ok(seen)
}

// ---------------------------------------------------------------------------------------------
// The exact-mechanism removal guard
// ---------------------------------------------------------------------------------------------

/// The `merge::reap_clone` guard, re-stated where the removal happens: a clone may be removed only when
/// its canonical path is EXACTLY `<root>/<run id>`, it holds a real `.git`, and it neither is nor
/// contains the source repository. Never a broad prefix; never an inferred parent.
///
/// `source` is `None` only on the F2 licensed path when the origin chain did not resolve. There is then
/// no source repository to protect from this removal — and, precisely because none resolved, none can
/// be at risk of it. The path and `.git` checks are unchanged, and the evidence string says which of
/// the two shapes was proved rather than implying the stronger one.
pub fn removal_guard(
    clone: &Path,
    root: &Path,
    run_id: &str,
    source: Option<&Path>,
) -> Result<String, String> {
    let expected = root.join(run_id);
    if clone != expected {
        return Err(format!(
            "{} is not the expected `<root>/<run id>` path {}",
            clone.display(),
            expected.display()
        ));
    }
    if !sr::real_dir(&clone.join(".git")) {
        return Err(format!(
            "{} has no real `.git` directory — refusing to remove a directory that is not a clone",
            clone.display()
        ));
    }
    let Some(source) = source else {
        return Ok(format!(
            "removal guard: canonical path is exactly {}, and `.git` is a real directory. No source \
             repository was resolved for this clone (owner-dispositioned, unresolved origin chain), \
             so there is none for this removal to endanger",
            expected.display()
        ));
    };
    let csource = std::fs::canonicalize(source).unwrap_or_else(|_| source.to_path_buf());
    if csource == clone {
        return Err(format!(
            "{} IS the source repository — refusing",
            clone.display()
        ));
    }
    if csource.starts_with(clone) {
        return Err(format!(
            "the source repository {} is INSIDE the clone — removing it would destroy the repository \
             the containment proof was asked of",
            csource.display()
        ));
    }
    Ok(format!(
        "removal guard: canonical path is exactly {}, `.git` is a real directory, and the source \
         repository {} is neither it nor inside it",
        expected.display(),
        csource.display()
    ))
}

// ---------------------------------------------------------------------------------------------
// The orchestrator
// ---------------------------------------------------------------------------------------------

pub struct ClonesRequest<'a> {
    /// The already-`verify_root`-checked `.a2a-implement` root.
    pub scan_root: &'a Path,
    /// The S2 classifier's items — OBSERVATIONS, every one re-derived at the boundary.
    pub items: &'a [sr::ReportItem],
    pub protected: &'a rp::ProtectedRoots,
    pub dry_run: bool,
    /// Non-zero means an affirmative container answer is REQUIRED (the host probe cannot see inside a
    /// container VM); zero means the axis is disclosed as uncovered.
    pub runtimes_configured: usize,
    /// `[storage] clone_reap_lookback`.
    pub lookback: u32,
    /// F2: the owner-authored license from `--disposition-list`, when one was supplied. For a run it
    /// names, the content-on-main gate is replaced by the license — and nothing else is.
    pub disposition: Option<&'a DispositionList>,
}

/// Disclose that a LICENSED run was parked before the boundary gates ever ran.
///
/// A run refused at admission — or by the whole-root refusal above it — never reaches `reap_one`'s
/// own disclosure, so without this it is the one licensed park with no explanation anywhere, and an
/// operator is left reading "parked" against a run they authorized in writing. None of these refusals
/// is something the license could waive: it replaces the content-on-main proof and nothing else.
fn disclose_early_park(
    notes: &mut Vec<String>,
    disposition: Option<&DispositionList>,
    it: &sr::ReportItem,
    reason: &rp::ParkReason,
) {
    let (Some(d), Some(id)) = (disposition, it.run_id.as_deref()) else {
        return;
    };
    if !d.licenses(id) {
        return;
    }
    notes.push(format!(
        "run {id} is on the owner disposition list and was PARKED before the boundary gates ran: {}. \
         The license replaces the content-on-main proof ONLY; it grants no authority over any other \
         refusal.",
        reason.summary()
    ));
}

/// A clone that survived the lock-free admission gates.
struct Candidate<'a> {
    item: &'a sr::ReportItem,
    path: PathBuf,
    run_id: String,
    gates: Vec<String>,
    /// R6: this clone's `origin` chain, walked before ANY removal in this invocation.
    origin: OriginResolution,
}

/// Reap `.a2a-implement` standalone clones whose content is verifiably on the source repo's main.
///
/// Two phases, as in S3: the ADMISSION gates take no lock and change no state, so an inadmissible row
/// never causes this command to touch a run's lock namespace at all. Only survivors reach the BOUNDARY
/// gates, which run under that run's HELD operation lock — the lock `implement_resume` and `merge` take
/// on the very directory about to be removed.
pub fn reap_clones<E: rp::ReapEnv>(req: ClonesRequest<'_>, env: &E) -> rp::ReapReport {
    let mut report = rp::ReapReport {
        scan_root: sr::display_path(req.scan_root),
        dry_run: req.dry_run,
        ..Default::default()
    };
    report.free_bytes_before = env.free_bytes(req.scan_root);

    // F2. The license is disclosed BEFORE anything is examined, and disclosed whatever happens next:
    // an operator reading this report has to be able to see that a content-on-main proof was waived
    // for named runs, without inferring it from the rows.
    if let Some(d) = req.disposition {
        report.notes.push(format!(
            "OWNER DISPOSITION LICENSE IN FORCE: {} (sha256 {}), authorized by {:?} — {:?}. For its \
             {} named run(s) the CONTENT-ON-MAIN gate is replaced by this license and nothing else \
             is: the pid-alive, operation-lock, container-axis, consumer-probe, shape, git-state, \
             index-flag and submodule gates all still refuse, and evidence preservation, the fold \
             receipt and the durability barriers are unchanged. Their receipts record verdict \
             `{VERDICT_OWNER_DISPOSITION}` and never claim the content is on main.",
            d.list_file,
            d.list_sha256,
            d.authorized_by,
            d.reason,
            d.run_ids.len()
        ));
        // A run id nobody in this scan answers to is a NOTE, never an error: a list is written by
        // hand against a report, and by the time it is used a run may have been reaped, renamed, or
        // may live on another host. Refusing the whole command for it would be refusing to act on the
        // runs the list DOES name.
        let seen: BTreeSet<&str> = req
            .items
            .iter()
            .filter_map(|i| i.run_id.as_deref())
            .collect();
        let missing: Vec<&str> = d
            .run_ids
            .iter()
            .map(String::as_str)
            .filter(|r| !seen.contains(r))
            .collect();
        if !missing.is_empty() {
            report.notes.push(format!(
                "{} run id(s) on the disposition list were NOT found in this scan (already reaped, \
                 renamed, or on another host) — recorded, not an error: {}",
                missing.len(),
                missing.join(", ")
            ));
        }
    }

    if let Some(reason) = req.protected.refuse(req.scan_root) {
        report.notes.push(format!(
            "REFUSED: the whole scan root is {} — nothing was examined",
            reason.summary()
        ));
        for it in req.items {
            disclose_early_park(&mut report.notes, req.disposition, it, &reason);
            report.items.push(rp::park(it, reason.clone()));
        }
        return report;
    }

    let pin = match bridge_core::fs_custody::PinnedDirectoryV1::open(
        req.scan_root,
        "storage reap --clones",
    ) {
        Ok(p) => p,
        Err(e) => {
            let reason = rp::ParkReason::ScanRootIdentityChanged {
                detail: format!("scan root could not be descriptor-pinned: {e}"),
            };
            report.notes.push(format!("REFUSED: {}", reason.summary()));
            for it in req.items {
                disclose_early_park(&mut report.notes, req.disposition, it, &reason);
                report.items.push(rp::park(it, reason.clone()));
            }
            return report;
        }
    };
    let root = pin.canonical_path().to_path_buf();

    let mut candidates: Vec<Candidate<'_>> = Vec::new();
    for it in req.items {
        match admit(it, &root, req.protected) {
            Err(reason) => {
                disclose_early_park(&mut report.notes, req.disposition, it, &reason);
                report.items.push(rp::park(it, reason))
            }
            Ok(c) => candidates.push(c),
        }
    }

    // R6 — PHASE SEPARATION. Every candidate's `origin` chain is walked HERE, before the first
    // removal, and cached on the candidate. Chained runs are a real shape (`implement --repo <another
    // run's clone>`), so a parent and its child can both be candidates in one invocation; resolving at
    // the boundary meant the parent's removal left the child unable to resolve anything, and no later
    // invocation ever rewrites a dead `origin`, so the child was stranded PERMANENTLY. Resolving first
    // makes deletion order irrelevant: every terminal root is outside the scan root, so it survives
    // every sibling removal, and the boundary re-verifies its identity before trusting the answer.
    //
    // Read-only, no lock held, changes nothing — the verdicts themselves are still derived at the
    // boundary under the run's own operation lock.
    for c in candidates.iter_mut() {
        c.origin = resolve_origin(&c.path, &root);
    }

    // `(clone path, was the removal clean)` for every clone this command actually tried to remove.
    // A PARTIAL or UNKNOWN outcome belongs here too: its descendants are exactly the rows whose
    // recorded state is now least trustworthy.
    let mut attempted: Vec<(PathBuf, bool)> = Vec::new();
    for c in candidates {
        let path = c.path.clone();
        let item = reap_one(c, &pin, &req, env, &mut report);
        match item.outcome {
            rp::ItemOutcome::Deleted => attempted.push((path, true)),
            rp::ItemOutcome::Partial { .. } | rp::ItemOutcome::Unknown { .. } => {
                attempted.push((path, false))
            }
            _ => {}
        }
        report.items.push(item);
    }

    // A clone's nested payload rows (its build target, its evidence sidecar) are not this command's
    // authority and were parked as such — but if their enclosing clone went, they went WITH it. Leaving
    // them recorded as `parked` would report bytes as retained that are provably gone; assuming they
    // all went would be equally false after a partial removal. Each path is restat'ed.
    project_rows_under_reaped_clones(&mut report, &attempted);

    report.free_bytes_after = env.free_bytes(&root);
    report.items.sort_by(|a, b| a.path.cmp(&b.path));
    report
}

/// The lock-free admission gates, in refusal order.
fn admit<'a>(
    it: &'a sr::ReportItem,
    root: &Path,
    protected: &rp::ProtectedRoots,
) -> Result<Candidate<'a>, rp::ParkReason> {
    // FIRST, by the scanner's own declaration. A volume NAME is not addressable by any filesystem
    // operation; the `is_absolute` check behind it is defence in depth, never the discrimination.
    let path = PathBuf::from(&it.path);
    if !it.source.is_filesystem_path()
        || it.class == sr::PayloadClass::ContainerOrImage
        || !path.is_absolute()
    {
        return Err(rp::ParkReason::ContainerVolume);
    }
    // A `[worktrees]` checkout shares its source's object store and is removed with `git worktree
    // remove`; its custody handle is the ADR-0025 sidecar lease, which this command does not hold.
    if it.source == sr::ItemSource::WorktreePath
        || it.checkout_kind == Some(sr::CheckoutKind::LinkedWorktree)
    {
        return Err(rp::ParkReason::WorktreeCustody);
    }
    // D-2 BEFORE classification: a protected root is refused whatever class the scan gave it.
    if let Some(reason) = protected.refuse(&path) {
        return Err(reason);
    }
    // `Unclassified` under the clone root is usually a SHAPE refusal the scan already made (a linked
    // worktree, an ambiguous `.git`, a stray file). Carrying the scan's own note through says WHY,
    // instead of reporting the uninformative "class Unclassified" on the one command whose operator
    // most needs to know which checkouts it refused to touch and on what grounds.
    if it.class == sr::PayloadClass::Unclassified {
        return Err(rp::ParkReason::NotAStandaloneClone {
            detail: it
                .note
                .clone()
                .unwrap_or_else(|| "the scan could not classify this entry".into()),
        });
    }
    if it.class != sr::PayloadClass::SourceCheckout {
        return Err(rp::ParkReason::NotReapableClass {
            class: it.class.label().to_string(),
        });
    }
    if it.checkout_kind != Some(sr::CheckoutKind::StandaloneClone) {
        return Err(rp::ParkReason::NotAStandaloneClone {
            detail: format!(
                "the scan recorded checkout kind {:?}, not a standalone clone",
                it.checkout_kind.map(|k| k.label())
            ),
        });
    }
    let Some(run_id) = it.run_id.clone().filter(|r| !r.is_empty()) else {
        return Err(rp::ParkReason::NoOwningRun);
    };
    // The clone IS the run directory: exactly `<root>/<run id>`, never a descendant and never the root.
    if path != root.join(&run_id) {
        return Err(rp::ParkReason::NotUnderScanRoot {
            root: root.to_string_lossy().into_owned(),
        });
    }
    Ok(Candidate {
        item: it,
        path,
        run_id,
        // Filled in by the resolution phase below, before any removal happens.
        origin: OriginResolution {
            chain: Err(rp::ParkReason::NotOnSourceMain {
                verdict: "unknown".into(),
                detail: "the origin chain was never resolved".into(),
            }),
            immediate: None,
            root_identity: None,
            first_hop: None,
        },
        gates: vec![
            format!(
                "source: the scan declared this row `{}` — an `.a2a-implement` filesystem path, the \
                 only source whose runs have an operation lock to gate a deletion on",
                it.source.label()
            ),
            format!(
                "D-2 protected roots: {} checked, none contains or is contained by this clone",
                protected.paths().len()
            ),
        ],
    })
}

/// One clone, from its run-owner gate to its receipt. Pushes evidence paths and notes into `report`;
/// returns the item record.
fn reap_one<E: rp::ReapEnv>(
    c: Candidate<'_>,
    pin: &bridge_core::fs_custody::PinnedDirectoryV1,
    req: &ClonesRequest<'_>,
    env: &E,
    report: &mut rp::ReapReport,
) -> rp::ReapItem {
    let Candidate {
        item,
        path,
        run_id,
        origin,
        mut gates,
    } = c;
    let root = pin.canonical_path().to_path_buf();
    // F2. The license for THIS run, if the operator supplied a list naming it.
    let licensed = req.disposition.filter(|d| d.licenses(&run_id));

    macro_rules! parked {
        ($reason:expr) => {{
            let reason = $reason;
            // A licensed run that parks anyway is the case an operator is most likely to misread as
            // "the list did not work". Say which gate refused, in the report, every time.
            if licensed.is_some() {
                report.notes.push(format!(
                    "run {run_id} is on the owner disposition list and was PARKED anyway: {}. The \
                     license replaces the content-on-main proof ONLY; every other gate still \
                     refuses, and this one did.",
                    reason.summary()
                ));
            }
            return rp::ReapItem {
                path: item.path.clone(),
                source: item.source,
                class: item.class.label().to_string(),
                run_id: item.run_id.clone(),
                logical_bytes: item.measured.logical_bytes,
                disk_bytes: item.measured.disk_bytes,
                freed_bytes_measured: None,
                outcome: rp::ItemOutcome::Parked { reason },
                gates,
            };
        }};
    }

    // GATE 1 — run-owner liveness, BEFORE the lock (an initial `implement` holds only its ADR-0025 run
    // lease and never the operation lock, so the lock alone does not exclude it). Pre-lock deliberately:
    // it needs no lock to be meaningful, and it avoids writing into a LIVE run's lock namespace.
    let pid = match rp::run_owner_pid(&run_id) {
        Ok(p) => p,
        Err(detail) => parked!(rp::ParkReason::RunIdNotParseable { detail }),
    };
    match env.process_alive(pid) {
        rp::PidLiveness::Alive => {
            env.progress(&format!(
                "run {run_id}: owner pid {pid} is still alive — parked"
            ));
            // The license does NOT reach here: an owner dispositioning a run cannot have meant
            // "delete the working directory out from under it while it is still running".
            parked!(rp::ParkReason::RunOwnerAlive { pid })
        }
        rp::PidLiveness::Unknown(detail) => {
            parked!(rp::ParkReason::RunOwnerLivenessUnknown { detail })
        }
        rp::PidLiveness::Dead => {}
    }
    gates.push(format!(
        "run owner: pid {pid} is not running (the run crashed or completed); the operation lock below \
         excludes only `resume`/`merge`, never an initial `implement`"
    ));

    // GATE 2 — the operation lock, HELD across probe→delete. Here it is genuinely discriminating:
    // `implement_resume` and `merge` take this exact lock on this exact directory.
    let guard = match env.acquire_operation_lock(&root, &run_id) {
        Err(rp::LockFailure::Contended) => {
            parked!(rp::ParkReason::OperationLockHeld {
                run_id: run_id.clone()
            })
        }
        Err(rp::LockFailure::Unavailable(detail)) => {
            parked!(rp::ParkReason::OperationLockUnavailable { detail })
        }
        Ok(g) => g,
    };
    gates.push(format!(
        "operation lock: HELD for run {run_id} across probe and delete (the same lock `resume` and \
         `merge` take on this clone)"
    ));

    // Everything below runs under the held lock; every early return must drop it.
    macro_rules! parked_locked {
        ($reason:expr) => {{
            drop(guard);
            parked!($reason)
        }};
    }

    // GATE 3 — the pinned scan root must still be the root we pinned.
    if let Err(detail) = rp::pinned_root_unchanged(pin) {
        parked_locked!(rp::ParkReason::ScanRootIdentityChanged { detail })
    }
    gates.push(format!(
        "scan root: descriptor-pinned and re-verified ({})",
        rp::root_identity_label(pin)
    ));

    // GATE 4 — path identity: a real directory, not a symlink, still resolving to itself.
    let identity = match rp::dir_dev_ino(&path) {
        Ok(id) => id,
        Err(detail) => {
            if std::fs::symlink_metadata(&path)
                .map(|m| m.file_type().is_symlink())
                .unwrap_or(false)
            {
                parked_locked!(rp::ParkReason::PathIsSymlink)
            }
            parked_locked!(rp::ParkReason::PathNotADirectory { detail })
        }
    };
    match std::fs::canonicalize(&path) {
        Ok(cp) if cp == path => {}
        Ok(cp) => parked_locked!(rp::ParkReason::PathIdentityChanged {
            detail: format!("{} now resolves to {}", path.display(), cp.display()),
        }),
        Err(e) => parked_locked!(rp::ParkReason::PathIdentityChanged {
            detail: format!("{} has no canonical path: {e}", path.display()),
        }),
    }
    gates.push(format!(
        "path identity: real directory, no symlink, dev {} / ino {}",
        identity.0, identity.1
    ));

    // GATE 5 — consumers the scan established, then the container axis.
    for (kind, state) in [
        ("run lease", item.consumers.run_lease),
        ("container mount", item.consumers.container_mount),
    ] {
        if state == sr::HolderState::Held {
            parked_locked!(rp::ParkReason::LiveConsumer {
                kind: kind.to_string(),
                detail: "reported held by the scan; a reaper never overrides a positive holder"
                    .into(),
            })
        }
    }
    if req.runtimes_configured == 0 {
        gates.push(
            "container axis: not covered - no container runtime is configured, so no container answer \
             was sought (disclosed rather than read as `no containers`)"
                .to_string(),
        );
    } else if item.consumers.container_mount == sr::HolderState::Free {
        gates.push(format!(
            "container axis: {} runtime(s) answered, none mounts this clone",
            req.runtimes_configured
        ));
    } else {
        parked_locked!(rp::ParkReason::ContainerAxisUnanswered)
    }

    // GATE 6 — the host consumer probe over the run directory, taken UNDER the held lock.
    env.progress(&format!(
        "run {run_id}: probing {} for open files / cwds (recursive; can take a while)",
        path.display()
    ));
    match env.probe_consumers(&path) {
        rp::ConsumerProbe::Held { detail } => parked_locked!(rp::ParkReason::LiveConsumer {
            kind: "process/open-file/cwd".to_string(),
            detail,
        }),
        rp::ConsumerProbe::Failed { detail } => {
            parked_locked!(rp::ParkReason::ConsumerProbeFailed { detail })
        }
        rp::ConsumerProbe::Free => {}
    }
    gates.push(
        "consumer probe: process / open file / cwd over the clone answered FREE under the held lock"
            .to_string(),
    );

    // GATE 7 — shape, git state, and D-1 containment (or the F2 license that replaces containment),
    // all re-derived from the repository right now.
    let facts = match derive_clone_facts(&path, &run_id, &origin, req.lookback, req.disposition) {
        Ok(f) => f,
        Err(reason) => parked_locked!(reason),
    };
    gates.extend(facts.gates.iter().cloned());

    // GATE 8 — the exact-mechanism removal guard (the `merge::reap_clone` shape).
    match removal_guard(&path, &root, &run_id, facts.source_repo.as_deref()) {
        Ok(evidence) => gates.push(evidence),
        Err(detail) => parked_locked!(rp::ParkReason::RemovalGuardRefused { detail }),
    }

    // Measure only now, so the recorded size is the size of the thing about to go.
    let measured = sr::measure_tree(&path, &[]);
    gates.push(format!(
        "measured: {} logical / {} on disk",
        sr::human_bytes(measured.logical_bytes),
        sr::human_bytes(measured.disk_bytes)
    ));

    let ck = read_checkpoint_facts(&path);
    let mut receipt = FoldReceipt {
        schema: FOLD_RECEIPT_SCHEMA.to_string(),
        run_id: run_id.clone(),
        task_id: ck.task_id.clone(),
        branch: facts.branch.clone().or_else(|| ck.branch.clone()),
        head: Some(facts.head.clone()),
        tree: facts.tree.clone(),
        base: ck.base.clone(),
        base_ref: ck.base_ref.clone(),
        source_repo: facts.source_repo.as_deref().map(sr::display_path),
        origin_chain: facts.origin_chain.clone(),
        clone_path: sr::display_path(&path),
        containment: facts.containment.clone(),
        ref_inventory: facts.ref_inventory.clone(),
        // Plan §5's coordinate. A licensed deletion gets its OWN coordinate: writing `OnMain{...}`
        // here would make the receipt assert exactly the thing nobody proved.
        durability: match licensed {
            Some(d) if facts.containment.is_owner_disposition() => d.durability(),
            _ => format!(
                "OnMain{{ref={}, matched={}}}",
                facts.containment.main_ref.clone().unwrap_or_default(),
                facts.containment.matched_commit.clone().unwrap_or_default()
            ),
        },
        disposition: DISPOSITION_PLANNED.to_string(),
        logical_bytes: measured.logical_bytes,
        disk_bytes: measured.disk_bytes,
        evidence_preserved_at: None,
        evidence_files: Vec::new(),
        failure_detail: None,
        descendant_presence: Vec::new(),
        gates: gates.clone(),
        scan_root: root.to_string_lossy().into_owned(),
        scan_root_identity: rp::root_identity_label(pin),
        at_epoch_secs: env.now_epoch_secs(),
    };

    let base_item = |outcome: rp::ItemOutcome, gates: Vec<String>| rp::ReapItem {
        path: item.path.clone(),
        source: item.source,
        class: item.class.label().to_string(),
        run_id: item.run_id.clone(),
        logical_bytes: measured.logical_bytes,
        disk_bytes: measured.disk_bytes,
        freed_bytes_measured: None,
        outcome,
        gates,
    };

    // GATE 9 — the Evidence STRUCTURAL PREFLIGHT. Non-mutating, and therefore run in a dry run too:
    // the ambiguous shapes it refuses (a symlinked sidecar, a symlinked `.receipts`, a FIFO among the
    // evidence) are exactly the ones an operator needs to see in the plan rather than discover when
    // the real run parks.
    let receipts = receipts_dir(&root);
    let plan = match evidence_preflight(&path, &root) {
        Ok(p) => p,
        Err(reason) => parked_locked!(reason),
    };
    gates.push(format!(
        "evidence preflight (structural, non-mutating): {}",
        plan.summary
    ));

    if req.dry_run {
        // Say what was NOT done. A plan that listed only satisfied gates would imply the copy, the
        // fsync barriers and the receipt write had all succeeded — none of which a dry run performs.
        gates.push(
            "evidence preservation + fold receipt + durability barriers: NOT exercised by a dry run \
             (only the structural preflight above ran). The real run copies the evidence, fsyncs the \
             receipt namespace, and writes the receipt BEFORE any removal — and parks if any of those \
             fails."
                .to_string(),
        );
        drop(guard);
        return base_item(rp::ItemOutcome::Planned, gates);
    }

    // GATE 10 — Evidence preservation BEFORE the deletion. Evidence has its own retention decision
    // (plan §5) and never dies with the parent directory it happens to live in.
    let ev_dst = receipts.join(evidence_dir_name(&run_id));
    if plan.sources.is_empty() {
        gates.push(format!(
            "evidence preserved: nothing to preserve — {} (an ABSENT sidecar, not an unreadable one; \
             an ambiguous one would have parked at the preflight)",
            plan.summary
        ));
    } else {
        match env.copy_evidence(&plan.sources, &ev_dst) {
            Ok(files) => {
                receipt.evidence_preserved_at = Some(ev_dst.to_string_lossy().into_owned());
                receipt.evidence_files = files.clone();
                gates.push(format!(
                    "evidence preserved: {} file(s) copied to {} BEFORE the removal, from {}",
                    files.len(),
                    ev_dst.display(),
                    plan.summary
                ));
            }
            Err(detail) => parked_locked!(rp::ParkReason::EvidencePreservationFailed { detail }),
        }
        // The destination must still be where we meant it to be: a `.receipts` swapped for a symlink
        // between the preflight and the copy would have landed the evidence elsewhere.
        match (
            std::fs::canonicalize(&ev_dst),
            std::fs::canonicalize(&receipts),
        ) {
            (Ok(dst), Ok(rcp)) if dst.parent() == Some(rcp.as_path()) => {}
            (Ok(dst), Ok(rcp)) => parked_locked!(rp::ParkReason::EvidencePreservationFailed {
                detail: format!(
                    "preserved evidence landed at {}, which is not directly under {}",
                    dst.display(),
                    rcp.display()
                ),
            }),
            _ => parked_locked!(rp::ParkReason::EvidencePreservationFailed {
                detail: format!(
                    "the preserved-evidence path {} could not be re-resolved after the copy",
                    ev_dst.display()
                ),
            }),
        }
    }
    receipt.gates = gates.clone();

    // GATE 11 — durability barriers on the namespace ITSELF, before anything is removed. The receipt
    // file is fsync'd by its writer, but a file whose DIRECTORY ENTRY has not reached the disk does
    // not survive the crash the receipt exists to describe.
    for dir in [receipts.as_path(), root.as_path()] {
        if let Err(e) = env.sync_dir(dir) {
            parked_locked!(rp::ParkReason::EvidencePreservationFailed {
                detail: format!(
                    "the durability barrier on {} failed ({e}) — refusing to remove a clone whose \
                     record may not survive a crash",
                    dir.display()
                ),
            })
        }
    }
    gates.push(format!(
        "durability barriers: {} and {} fsync'd before any removal",
        receipts.display(),
        root.display()
    ));
    receipt.gates = gates.clone();

    // GATE 12 — the fold receipt as the crash-durable INTENT, fsync'd BEFORE the removal, in the
    // sibling namespace that outlives the clone.
    if let Err(detail) = rp::pinned_root_unchanged(pin) {
        parked_locked!(rp::ParkReason::ScanRootIdentityChanged { detail })
    }
    let receipt_path = match encode_and_write(&receipt, &receipts, env) {
        Ok(p) => {
            report.intents.push(p.clone());
            p
        }
        Err(detail) => parked_locked!(rp::ParkReason::FoldReceiptUnavailable { detail }),
    };

    // The removal, with the LAST identity checks immediately before the unlink.
    let mut item_gates = gates.clone();
    let outcome = 'removal: {
        if let Err(detail) = rp::pinned_root_unchanged(pin) {
            break 'removal rp::ItemOutcome::Parked {
                reason: rp::ParkReason::ScanRootIdentityChanged { detail },
            };
        }
        match rp::dir_dev_ino(&path) {
            Ok(now) if now == identity => {}
            Ok(now) => {
                break 'removal rp::ItemOutcome::Parked {
                    reason: rp::ParkReason::PathIdentityChanged {
                        detail: format!(
                            "{} changed identity between the gates and the removal (dev/ino {}/{} to \
                             {}/{})",
                            path.display(),
                            identity.0,
                            identity.1,
                            now.0,
                            now.1
                        ),
                    },
                };
            }
            Err(detail) => {
                break 'removal rp::ItemOutcome::Parked {
                    reason: rp::ParkReason::PathNotADirectory { detail },
                };
            }
        }
        item_gates.push(
            "boundary recheck: root and clone identity unchanged immediately before removal"
                .to_string(),
        );
        env.progress(&format!(
            "removing clone {} ({})",
            path.display(),
            sr::human_bytes(measured.disk_bytes)
        ));
        let before = env.free_bytes(&root);
        let removal = env.remove_tree(&path);
        let after = env.free_bytes(&root);
        let gone = std::fs::symlink_metadata(&path).is_err();
        let mut outcome = match (removal, gone) {
            (Ok(()), true) => rp::ItemOutcome::Deleted,
            (Ok(()), false) => rp::ItemOutcome::Partial {
                detail: format!(
                    "the removal reported success but {} is still present",
                    path.display()
                ),
            },
            (Err(e), false) => rp::ItemOutcome::Partial { detail: e },
            (Err(e), true) => rp::ItemOutcome::Unknown {
                detail: format!("removal reported an error ({e}) but the path is gone"),
            },
        };
        if let Err(detail) = rp::pinned_root_unchanged(pin) {
            outcome = rp::ItemOutcome::Unknown {
                detail: format!("the pinned scan root changed during the removal: {detail}"),
            };
        } else {
            item_gates.push("scan root identity re-verified AFTER the removal".to_string());
        }
        let mut out = base_item(outcome, item_gates.clone());
        out.freed_bytes_measured = match (before, after) {
            (Some(b), Some(a)) => Some(a as i64 - b as i64),
            _ => None,
        };
        // Carried out of the block through the item below.
        receipt.disposition = disposition_of(&out.outcome).to_string();
        // A removal that began and did not cleanly finish leaves every OTHER row about this clone
        // stale: some of their bytes may be gone. RESTAT them and record what is actually there —
        // in the receipt, because the report is transient and this is the durable statement.
        if !matches!(out.outcome, rp::ItemOutcome::Deleted) {
            receipt.failure_detail = out.outcome.detail();
            receipt.descendant_presence = descendant_presence(&report.items, &path);
            item_gates.push(format!(
                "descendants: {} row(s) under this clone restat'ed after the failed removal; their \
                 actual presence is recorded on the receipt",
                receipt.descendant_presence.len()
            ));
            out.gates = item_gates.clone();
        }
        receipt.gates = item_gates.clone();
        finish(&mut receipt, &receipts, &receipt_path, env, report, &run_id);
        drop(guard);
        return out;
    };

    // Reached only when a boundary recheck refused between the intent and the unlink: nothing was
    // removed, and the receipt must say so rather than being left claiming an intent to delete.
    receipt.disposition = DISPOSITION_ABORTED.to_string();
    receipt.gates = item_gates.clone();
    finish(&mut receipt, &receipts, &receipt_path, env, report, &run_id);
    drop(guard);
    base_item(outcome, item_gates)
}

fn disposition_of(o: &rp::ItemOutcome) -> &'static str {
    match o {
        rp::ItemOutcome::Deleted => DISPOSITION_DELETED,
        rp::ItemOutcome::Partial { .. } => DISPOSITION_PARTIAL,
        rp::ItemOutcome::Unknown { .. } => DISPOSITION_UNKNOWN,
        rp::ItemOutcome::Planned | rp::ItemOutcome::Parked { .. } => DISPOSITION_ABORTED,
    }
}

fn encode_and_write<E: rp::ReapEnv>(
    receipt: &FoldReceipt,
    receipts: &Path,
    env: &E,
) -> Result<String, String> {
    let json = serde_json::to_string_pretty(receipt).map_err(|e| e.to_string())?;
    env.write_named(receipts, &fold_receipt_name(&receipt.run_id), &json)
}

/// Rewrite the fold receipt with the outcome, BEFORE the operation lock is released — so no racing
/// resume or merge can interleave between the removal and the record of it. A failure here is a command
/// failure, not a note: the clone is already gone, and a reap whose record was lost is not a clean reap.
fn finish<E: rp::ReapEnv>(
    receipt: &mut FoldReceipt,
    receipts: &Path,
    intent_path: &str,
    env: &E,
    report: &mut rp::ReapReport,
    run_id: &str,
) {
    receipt.at_epoch_secs = env.now_epoch_secs();
    match encode_and_write(receipt, receipts, env) {
        Ok(p) => report.receipts.push(p),
        Err(e) => {
            let json = serde_json::to_string(receipt).unwrap_or_else(|_| "<unencodable>".into());
            report.notes.push(format!(
                "fold receipt for run {run_id} NOT updated with its outcome ({e}); the intent record \
                 at {intent_path} still reads `{DISPOSITION_PLANNED}` and the true outcome survives \
                 only in this report: {json}"
            ));
            report.receipt_failures.push(format!("run {run_id}: {e}"));
        }
    }
}

/// Restat every reported row that lives underneath `parent`, so a caller can state what is ACTUALLY
/// there rather than what it intended. Used for the receipt's presence map and for the report's own
/// row projection, so the two can never disagree.
fn descendant_presence(items: &[rp::ReapItem], parent: &Path) -> Vec<DescendantPresence> {
    items
        .iter()
        .filter_map(|it| {
            let p = PathBuf::from(&it.path);
            (p.starts_with(parent) && p != parent).then(|| DescendantPresence {
                present: std::fs::symlink_metadata(&p).is_ok(),
                path: it.path.clone(),
            })
        })
        .collect()
}

/// Project the rows that lived INSIDE a clone this command tried to remove.
///
/// They were parked as "not this command's authority" — true of the gate that would have licensed them
/// independently, and false of where their bytes ended up. A row whose path is GONE is recorded as
/// deleted-with-its-parent; a row still on disk stays retained, with its record saying why. The
/// decision is made by RESTATTING each path, never by assuming the parent's outcome applied uniformly:
/// a partial removal is precisely the case where it did not.
fn project_rows_under_reaped_clones(report: &mut rp::ReapReport, attempted: &[(PathBuf, bool)]) {
    if attempted.is_empty() {
        return;
    }
    for it in report.items.iter_mut() {
        if !matches!(it.outcome, rp::ItemOutcome::Parked { .. }) {
            continue;
        }
        let p = PathBuf::from(&it.path);
        let Some((parent, clean)) = attempted
            .iter()
            .find(|(r, _)| p.starts_with(r.as_path()) && p != *r)
        else {
            continue;
        };
        if std::fs::symlink_metadata(&p).is_ok() {
            // Still there: only reachable after a partial removal, and the row must keep saying so.
            it.gates.push(format!(
                "retained: the removal of its enclosing clone {} did not complete, and this path is \
                 still on disk (restat'ed after the attempt)",
                parent.display()
            ));
            continue;
        }
        it.outcome = rp::ItemOutcome::Deleted;
        it.gates.push(if *clean {
            format!(
                "removed WITH its enclosing clone {} — this row was not independently gated; the \
                 clone's own gates licensed it, and any Evidence it held was copied into the receipt \
                 namespace before the removal",
                parent.display()
            )
        } else {
            format!(
                "removed before the failure that left its enclosing clone {} incomplete — restat'ed \
                 after the attempt and this path is gone",
                parent.display()
            )
        });
    }
}

// ---------------------------------------------------------------------------------------------
// Rendering + usage
// ---------------------------------------------------------------------------------------------

/// The banner deliberately names BOTH licences and commits to neither: with `--disposition-list` in
/// play, "content verifiably on main" would be a claim about rows for which nothing established it.
/// Which one licensed a given deletion is on that row's receipt, where it is auditable.
pub fn render_text(r: &rp::ReapReport) -> String {
    rp::render_report(
        r,
        "--clones",
        "DESTRUCTIVE: quarantine clones licensed for removal were REMOVED — each one either because \
         its content is verifiably on the source repository's main (D-1) or because the owner \
         dispositioned its run as abandoned (`--disposition-list`). WHICH of the two licensed each \
         deletion is recorded on its fold receipt; the receipts and preserved evidence are listed \
         below.",
    )
}

pub const CLONES_USAGE: &str = "\
usage: a2a-bridge storage reap --clones [--dry-run] [--disposition-list <f>] [--config <f>] [--json]

DESTRUCTIVE, and the only reaper that can destroy unique bytes. Deletes `.a2a-implement` standalone
quarantine clones whose content is verifiably on the SOURCE repository's main branch (owner ruling D-1:
pre-squash commits need not survive; content on main suffices), OR whose run the owner has explicitly
dispositioned as abandoned (`--disposition-list`, plan §3 S4). It never touches a linked worktree, a
build target, evidence, an unclassified item, a container volume, or anything at, inside, or containing
a D-2 protected root.

  --clones            REQUIRED. Names the payload class this invocation may remove. There is no default
                      class, and `--clones` may not be combined with `--build-targets`: they are
                      different authorities with different gates and different receipts.
  --dry-run           evaluate every boundary gate for real, delete nothing and write no receipt. THIS
                      IS THE PLAN DOCUMENT: read it before authorizing a deletion.
  --disposition-list <path>
                      an OWNER-AUTHORED license file (see OWNER DISPOSITION below). Only valid with
                      `--clones`: it replaces that authority's content-on-main gate, and
                      `--build-targets` has no such gate for it to replace.
  --config <path>     registry config (default: ./a2a-bridge.toml).
  --json              machine-readable output instead of the table.

EVERY GATE BELOW IS RE-DERIVED AT THE DESTRUCTIVE BOUNDARY under the run's HELD operation lock. A report
is an observation, never a warrant.

  run owner      the pid in `impl-<pid>-<nonce>` is not running (an initial `implement` holds only its
                 ADR-0025 run lease, so the operation lock alone would not exclude it).
  operation lock held across probe->delete. For clones this is genuinely discriminating: `resume` and
                 `merge` take this exact lock on this exact directory.
  pinned root    the scan root is descriptor-pinned; dev/ino re-verified before AND after the removal,
                 and the clone's own identity immediately before the unlink.
  shape          `.git` must be a real DIRECTORY (standalone clone). A linked worktree shares its
                 source's object store and is removed with `git worktree remove`; an ambiguous `.git`
                 proves nothing. Both park.
  git state      `git status --porcelain` must be CLEAN — plus the three things porcelain cannot see:
                 `ls-files -v` must show no `--assume-unchanged` / `--skip-worktree` / sparse entry
                 (they suppress the status line for modified tracked bytes), the clone must not be a
                 sparse checkout, and no submodule may be initialized (a clean submodule emits no
                 status line at all while its object store sits in `.git/modules` and would die with
                 the clone). An ignored entry counts as disposable only when its LAST path component
                 is `target`/`node_modules`/`.venv` AND the on-disk markers prove it (cargo artifacts,
                 a `package.json` sibling, `pyvenv.cfg`) — the same evidence `--build-targets` demands.
                 Unborn HEAD parks.
  content on main `yes(head)` (HEAD is an ancestor of source main) or `yes(tree)` (HEAD's exact tree is
                 on main under a different commit — the squash landing). `no` parks and `unknown` parks.
                 A squash that REWROTE the tree reads `no`: fail-closed, the clone is kept. \"main\" is
                 resolved as a fully-qualified BRANCH (`refs/heads/main`, `master`, else the source's
                 own HEAD branch), so a tag named `main` can never stand in for it, and its OID is
                 re-read afterwards so a verdict is never assembled from a moving history. The source
                 repo comes from the clone's own `origin` and ONLY when that is a local path — a hosted
                 origin parks, because no network is ever contacted — and must agree with the source
                 repo the run's checkpoint records. Window: `[storage] clone_reap_lookback` (default
                 2000 commits of source main).
  origin chain   an `implement` run started with `--repo <another run's clone>` records that SIBLING as
                 its origin, so asking the D-1 question of `origin` asks it of a quarantine directory
                 instead of the repository the work lands in (68 of this host's 81 parked clones were
                 in exactly that shape). When the resolved origin is itself inside the scan root, the
                 chain is FOLLOWED to the first repository outside it — at most 8 in-root edges, no
                 repeated path (a cycle parks), and every hop, the terminal one included, must exist,
                 be a local path, be a REAL git repository, and agree with its own run's checkpoint.
                 (`git -C` walks up, so a hop naming a plain directory would otherwise be answered by
                 whatever repository encloses it; probes are also run with GIT_CEILING_DIRECTORIES so
                 that ascent cannot happen at all.) A chain that cannot be followed parks with verdict
                 `unknown` and the walk recorded. The full chain is on the receipt.
  root corrobor. before any verdict is asked of it, the repository the chain landed on must contain
                 THIS clone's own lineage root commit — and its checkpoint's base commit, when that
                 resolves in the clone. A hop whose run has no checkpoint contributes no cross-check,
                 so this is what stands between a rewritten `origin` and a verdict from a repository
                 the clone never came from. Uncorroborated parks `unknown`.
  ordering       every candidate's chain is resolved BEFORE the first removal, so reaping a parent
                 clone cannot strand the child whose origin names it. Terminal roots live outside the
                 scan root and survive sibling deletions; the boundary re-verifies the root's dev/ino
                 (and the first hop, unless this run just reaped it) before trusting the cached walk.
                 A chain broken by an EARLIER invocation does not self-heal — nothing rewrites a dead
                 `origin` — and the recovery for those is `--disposition-list`.

  refs           EVERY ref is swept (`refs/heads/*`, `refs/tags/*`, `refs/stash`), because containment
                 proves HEAD while the deletion takes the whole object store. Each tip must be HEAD, an
                 ancestor of HEAD, or independently on source main. A rogue agent branch, a stash, or a
                 tag-only commit therefore parks the clone.
  consumers      one recursive process/open-file/cwd probe over the clone must answer FREE.
  container axis with a runtime configured, an affirmative container answer is REQUIRED.

BEFORE any removal: a structural preflight refuses AMBIGUOUS evidence shapes (a symlinked
`.git/a2a-bridge`, a FIFO or symlink inside it, a symlinked or misplaced `.receipts`) while letting a
genuinely ABSENT sidecar through; then the clone's `.git/a2a-bridge/` sidecar plus `.git/A2A_TASK.md`
and `.git/A2A_COMMIT_MSG` are copied to `<root>/.receipts/<run id>-evidence/` (Evidence has its own
retention and never dies with its parent), the namespace is fsync'd, and the fold receipt is fsync'd to
`<root>/.receipts/<run id>-fold.json` carrying `{run id, task id, branch, HEAD, tree, base, origin
chain, containment verdict + which ref/commit matched (or which owner license replaced it),
disposition, timestamp}`. That first write is the crash-durable
INTENT (`disposition: planned_delete`); the same file is rewritten with the outcome before the lock is
released, and a removal that did not cleanly finish also records WHY plus a restat'ed presence map of
everything underneath. Any of these failing PARKS the clone; a failure to record the OUTCOME fails the
command.

A `--dry-run` runs every gate above except the preservation and durability ones — it copies nothing and
writes nothing — and says so on each planned row rather than implying they passed.

OWNER DISPOSITION (`--disposition-list <path>`)

Plan §3 S4 licenses this command over two populations: clones whose content is verifiably on main, and
clones \"whose run the owner has dispositioned as abandoned\". This flag supplies the second. The file
is owner-authored JSON, and exactly this:

  {\"authorized_by\": \"<free text>\", \"reason\": \"<free text>\", \"run_ids\": [\"impl-<pid>-<nonce>\", ...]}

It must be a REGULAR FILE (never followed through a symlink), non-empty, and parsed STRICTLY: an
unknown field, an empty `authorized_by`/`reason`/`run_ids`, or a run id that is not a single ordinary
path component refuses the whole command. A run id in the list that this scan does not find is a NOTE,
never an error — a list is written by hand against an earlier report.

What the license does, exactly: for a run it names, THE CONTENT-ON-MAIN GATE IS REPLACED — the source
resolution, the D-1 verdict, the ref sweep, and the moving-history recheck. NOTHING ELSE IS. The
run-owner pid probe, the operation lock, the pinned root, the path identity, the container axis, the
consumer probe, the standalone-clone shape check, `git status`, the index-flag and sparse checks, the
submodule check, the evidence preflight and copy, the durability barriers, the fold receipt and the
exact-mechanism removal guard all still run and still refuse. A DIRTY dispositioned clone PARKS, and
the refusal names the conflict: an owner can disposition a run, but nobody can disposition bytes that
are on no commit and that no one has read.

The ref SWEEP is part of the content-on-main proof, so the license replaces it — an abandoned run is
abandoned tips and all. What the license does NOT excuse is losing the record of it: every licensed
receipt carries a `ref_inventory` listing every ref that existed at removal time with `{refname, oid,
is_head, is_ancestor_of_head}`, and the gate text names the tips that were outside HEAD's history.
The inventory is RECORDED, never gated: a probe that cannot answer is written down as unknown, because
the license already settled whether the run dies.

Its receipts record `containment: {verdict: \"owner-disposition\", authorized_by, reason, list_file,
list_sha256}` and `durability: OwnerDisposition{...}`. They never say `yes(head)`, `yes(tree)` or
`OnMain{...}`: the license is auditable back to the exact bytes the owner signed, and it never claims
a landing nobody proved.

JSON: `items[].path` is always a filesystem path here; the report's `source` field
(`implement-path` | `worktree-path` | `volume-name`) is what tells destructive code which is which.";

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::collections::BTreeSet;
    use std::process::Command;
    use std::rc::Rc;

    // -----------------------------------------------------------------------------------------
    // The injectable environment
    // -----------------------------------------------------------------------------------------

    #[derive(Clone, Debug, PartialEq, Eq)]
    enum Ev {
        Lock(String),
        Probe(String),
        CopyEvidence(String),
        SyncDir(String),
        Write(String),
        Remove(String),
        Unlock(String),
    }

    #[derive(Default)]
    struct Journal {
        locks_held: Vec<String>,
        events: Vec<Ev>,
        /// `(path, was the operation lock held when the probe ran)`.
        probe_witness: Vec<(String, bool)>,
        remove_witness: Vec<(String, bool)>,
        removed: Vec<String>,
        writes: Vec<(String, String)>,
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
                    Ev::CopyEvidence(_) => "copy-evidence",
                    Ev::SyncDir(_) => "sync-dir",
                    Ev::Write(_) => "write",
                    Ev::Remove(_) => "remove",
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

    type ProbeFn = RefCell<Box<dyn FnMut(&Path) -> rp::ConsumerProbe>>;
    type RemoveFn = RefCell<Box<dyn FnMut(&Path) -> Result<(), String>>>;

    struct FakeEnv {
        j: Rc<RefCell<Journal>>,
        contended: BTreeSet<String>,
        probe: ProbeFn,
        remove: RemoveFn,
        write_error: Option<String>,
        /// Fails only the SECOND write of a receipt — the outcome update, after the clone is gone.
        second_write_error: Option<String>,
        copy_error: Option<String>,
        /// Injected failure of the directory durability barrier (F6).
        sync_error: Option<String>,
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
                probe: RefCell::new(Box::new(|_| rp::ConsumerProbe::Free)),
                remove: RefCell::new(Box::new(|p| {
                    std::fs::remove_dir_all(p).map_err(|e| e.to_string())
                })),
                write_error: None,
                second_write_error: None,
                copy_error: None,
                sync_error: None,
                alive_pids: BTreeSet::new(),
                now: 1_700_000_000,
            }
        }
        fn removed(&self) -> Vec<String> {
            self.j.borrow().removed.clone()
        }
    }

    impl rp::ReapEnv for FakeEnv {
        type Lock = FakeLock;

        fn acquire_operation_lock(
            &self,
            implement_root: &Path,
            run_id: &str,
        ) -> Result<FakeLock, rp::LockFailure> {
            if self.contended.contains(run_id) {
                return Err(rp::LockFailure::Contended);
            }
            // Mirrors `acquire_persistent_lock_in`: taking the lock CREATES the namespace, which is a
            // dry run's one state-visible effect and must be modelled, not wished away.
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

        fn process_alive(&self, pid: u32) -> rp::PidLiveness {
            if self.alive_pids.contains(&pid) {
                rp::PidLiveness::Alive
            } else {
                rp::PidLiveness::Dead
            }
        }

        fn probe_consumers(&self, path: &Path) -> rp::ConsumerProbe {
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
                self.j.borrow_mut().removed.push(path.display().to_string());
                self.j.borrow_mut().free += 4096;
            }
            r
        }

        fn now_epoch_secs(&self) -> u64 {
            self.now
        }

        fn write_intent(&self, dir: &Path, json: &str) -> Result<String, String> {
            self.write_named(dir, "intent.json", json)
        }

        fn write_receipt(&self, dir: &Path, json: &str) -> Result<String, String> {
            self.write_named(dir, "receipt.json", json)
        }

        fn write_named(&self, dir: &Path, file_name: &str, json: &str) -> Result<String, String> {
            if let Some(e) = &self.write_error {
                return Err(e.clone());
            }
            let already = self
                .j
                .borrow()
                .writes
                .iter()
                .any(|(p, _)| p.ends_with(file_name));
            if already {
                if let Some(e) = &self.second_write_error {
                    return Err(e.clone());
                }
            }
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
            let p = dir.join(file_name);
            std::fs::write(&p, json).map_err(|e| e.to_string())?;
            self.j
                .borrow_mut()
                .writes
                .push((p.display().to_string(), json.to_string()));
            self.j
                .borrow_mut()
                .events
                .push(Ev::Write(p.display().to_string()));
            Ok(p.display().to_string())
        }

        fn copy_evidence(&self, sources: &[PathBuf], to: &Path) -> Result<Vec<String>, String> {
            if let Some(e) = &self.copy_error {
                return Err(e.clone());
            }
            self.j.borrow_mut().events.push(Ev::CopyEvidence(
                sources
                    .iter()
                    .map(|s| s.display().to_string())
                    .collect::<Vec<_>>()
                    .join(","),
            ));
            let mut out = Vec::new();
            for from in sources {
                if sr::real_file(from) {
                    std::fs::create_dir_all(to).map_err(|e| e.to_string())?;
                    let name = from.file_name().unwrap_or_default().to_string_lossy();
                    std::fs::copy(from, to.join(name.as_ref())).map_err(|e| e.to_string())?;
                    out.push(name.into_owned());
                    continue;
                }
                if !sr::real_dir(from) {
                    continue;
                }
                std::fs::create_dir_all(to).map_err(|e| e.to_string())?;
                for e in std::fs::read_dir(from).map_err(|e| e.to_string())? {
                    let e = e.map_err(|e| e.to_string())?;
                    if sr::real_file(&e.path()) {
                        std::fs::copy(e.path(), to.join(e.file_name()))
                            .map_err(|e| e.to_string())?;
                        out.push(e.file_name().to_string_lossy().into_owned());
                    }
                }
            }
            Ok(out)
        }

        fn sync_dir(&self, dir: &Path) -> Result<(), String> {
            self.j
                .borrow_mut()
                .events
                .push(Ev::SyncDir(dir.display().to_string()));
            match &self.sync_error {
                Some(e) => Err(e.clone()),
                None => Ok(()),
            }
        }

        fn progress(&self, message: &str) {
            self.j.borrow_mut().progress.push(message.to_string());
        }
    }

    // -----------------------------------------------------------------------------------------
    // LIVE git fixtures. Every containment case is built with a real `git init` source repository
    // and a real `git clone --no-hardlinks` quarantine clone, because the gate under test IS git's
    // answer — a hand-written `.git` directory would test the parser and nothing else.
    // -----------------------------------------------------------------------------------------

    fn git(p: &Path, args: &[&str]) {
        let out = Command::new("git")
            .arg("-C")
            .arg(p)
            .args(args)
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@example.invalid")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@example.invalid")
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?} in {}: {}",
            p.display(),
            String::from_utf8_lossy(&out.stderr)
        );
    }

    fn write(p: &Path, body: &str) {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, body).unwrap();
    }

    struct Fx {
        _td: tempfile::TempDir,
        root: PathBuf,
        implement: PathBuf,
        source: PathBuf,
        /// The clone directory, named `impl-<a dead pid>-<nonce>`.
        clone: PathBuf,
        run_id: String,
    }

    /// A pid that is (almost certainly) not running, so the run-owner gate reads DEAD. `u32::MAX` is
    /// above every platform's pid_max; the fake env's `alive_pids` is authoritative in tests anyway.
    const DEAD_PID: u32 = 4_294_967_294;

    /// Source repo with two commits on `main`, plus a `.a2a-implement` root holding one clone.
    fn fx() -> Fx {
        let td = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(td.path()).unwrap();
        let source = root.join("source");
        std::fs::create_dir_all(&source).unwrap();
        git(&source, &["init", "--initial-branch=main", "-q"]);
        write(&source.join("README.md"), "one\n");
        git(&source, &["add", "-A"]);
        git(&source, &["commit", "-qm", "one"]);
        write(&source.join("src/lib.rs"), "pub fn a() {}\n");
        write(&source.join(".gitignore"), "/target\n");
        git(&source, &["add", "-A"]);
        git(&source, &["commit", "-qm", "two"]);

        let implement = root.join(".a2a-implement");
        std::fs::create_dir_all(&implement).unwrap();
        let run_id = format!("impl-{DEAD_PID}-aa");
        let clone = implement.join(&run_id);
        let out = Command::new("git")
            .args(["clone", "--no-hardlinks", "-q"])
            .arg(&source)
            .arg(&clone)
            .output()
            .unwrap();
        assert!(out.status.success(), "clone: {:?}", out);
        // The checkpoint evidence every implement run carries.
        write(
            &sr::evidence_dir(&clone).join("implement-checkpoint.json"),
            &format!(
                "{{\"schema_version\":1,\"task_id\":\"{run_id}\",\"resume_id\":\"{run_id}\",\
                 \"branch\":\"feat/x\",\"base_commit\":\"deadbeef\",\"base_ref\":\"main\",\
                 \"phase\":\"Approved\"}}"
            ),
        );
        Fx {
            _td: td,
            root,
            implement,
            source,
            clone,
            run_id,
        }
    }

    /// Commit `body` into `file` on a new branch in the clone, returning the new HEAD.
    fn clone_commit(f: &Fx, branch: &str, file: &str, body: &str) -> String {
        git(&f.clone, &["checkout", "-q", "-b", branch]);
        write(&f.clone.join(file), body);
        git(&f.clone, &["add", "-A"]);
        git(&f.clone, &["commit", "-qm", "work"]);
        sr::git_str(&f.clone, &["rev-parse", "HEAD"]).unwrap()
    }

    fn source_commit(f: &Fx, file: &str, body: &str) {
        write(&f.source.join(file), body);
        git(&f.source, &["add", "-A"]);
        git(&f.source, &["commit", "-qm", "landed"]);
    }

    fn scan(implement: &Path) -> Vec<sr::ReportItem> {
        let mut notes = Vec::new();
        sr::scan_implement_root(implement, &mut notes)
    }

    fn run(f: &Fx, items: &[sr::ReportItem], env: &FakeEnv, dry_run: bool) -> rp::ReapReport {
        run_with(f, items, env, dry_run, 0, DEFAULT_CLONE_REAP_LOOKBACK)
    }

    fn run_with(
        f: &Fx,
        items: &[sr::ReportItem],
        env: &FakeEnv,
        dry_run: bool,
        runtimes_configured: usize,
        lookback: u32,
    ) -> rp::ReapReport {
        run_licensed(f, items, env, dry_run, runtimes_configured, lookback, None)
    }

    #[allow(clippy::too_many_arguments)]
    fn run_licensed(
        f: &Fx,
        items: &[sr::ReportItem],
        env: &FakeEnv,
        dry_run: bool,
        runtimes_configured: usize,
        lookback: u32,
        disposition: Option<&DispositionList>,
    ) -> rp::ReapReport {
        reap_clones(
            ClonesRequest {
                scan_root: &f.implement,
                items,
                protected: &rp::ProtectedRoots::default(),
                dry_run,
                runtimes_configured,
                lookback,
                disposition,
            },
            env,
        )
    }

    /// Look an item up by its LEXICAL path (never `display_path`, which canonicalizes and would follow
    /// a symlink the test planted to assert the refusal).
    fn item_for<'a>(r: &'a rp::ReapReport, path: &Path) -> &'a rp::ReapItem {
        let want = path.to_string_lossy().into_owned();
        r.items
            .iter()
            .find(|i| i.path == want)
            .unwrap_or_else(|| panic!("no reap item for {want}; got {:?}", r.items))
    }

    fn parked_reason(r: &rp::ReapReport, path: &Path) -> rp::ParkReason {
        match &item_for(r, path).outcome {
            rp::ItemOutcome::Parked { reason } => reason.clone(),
            other => panic!("expected {} parked, got {other:?}", path.display()),
        }
    }

    fn fold_receipt(f: &Fx) -> serde_json::Value {
        let p = receipts_dir(&f.implement).join(fold_receipt_name(&f.run_id));
        let raw = std::fs::read_to_string(&p)
            .unwrap_or_else(|e| panic!("no fold receipt at {}: {e}", p.display()));
        serde_json::from_str(&raw).unwrap()
    }

    // -----------------------------------------------------------------------------------------
    // Containment — the D-1 gate, on live repositories
    // -----------------------------------------------------------------------------------------

    /// The fast-forward landing: the clone's HEAD is an ancestor of source main. Discriminates a
    /// reaper that refuses everything (a gate that never passes is not a gate) and pins that the
    /// receipt records WHICH evidence licensed it.
    #[test]
    fn a_clone_whose_head_is_on_source_main_is_deleted() {
        let f = fx();
        let items = scan(&f.implement);
        let env = FakeEnv::new();
        let r = run(&f, &items, &env, false);

        assert_eq!(item_for(&r, &f.clone).outcome, rp::ItemOutcome::Deleted);
        assert!(!f.clone.exists(), "the clone survived a `deleted` verdict");
        assert!(
            f.source.join("src/lib.rs").exists(),
            "escaped into the source repo"
        );
        let v = fold_receipt(&f);
        assert_eq!(v["containment"]["verdict"], "yes(head)");
        assert_eq!(v["disposition"], DISPOSITION_DELETED);
    }

    /// The squash landing that keeps the tree: a different commit id carrying byte-identical content.
    /// Discriminates a gate that only accepts ancestry — which would park most of the real backlog,
    /// since the bridge's own workflow squash-merges.
    #[test]
    fn a_squash_landed_clone_is_deleted_on_exact_tree_evidence() {
        let f = fx();
        let head = clone_commit(&f, "feat/x", "src/new.rs", "pub fn b() {}\n");
        // The same content lands on source main under a DIFFERENT commit id.
        source_commit(&f, "src/new.rs", "pub fn b() {}\n");
        assert_ne!(
            head,
            sr::git_str(&f.source, &["rev-parse", "main"]).unwrap(),
            "fixture must model a squash, not a fast-forward"
        );

        let items = scan(&f.implement);
        let env = FakeEnv::new();
        let r = run(&f, &items, &env, false);
        assert_eq!(item_for(&r, &f.clone).outcome, rp::ItemOutcome::Deleted);
        let v = fold_receipt(&f);
        assert_eq!(v["containment"]["verdict"], "yes(tree)");
        assert_eq!(
            v["containment"]["matched_commit"],
            sr::git_str(&f.source, &["rev-parse", "main"]).unwrap(),
            "the receipt does not name WHICH commit matched"
        );
        assert_eq!(v["head"], head, "the pre-squash HEAD was not preserved");
    }

    /// Work that exists only in the clone. Discriminates a reaper that reads "reachable from some ref"
    /// (or from the clone's own frozen `refs/remotes/origin/*`) as landed.
    #[test]
    fn a_branch_only_clone_is_parked_with_its_work_intact() {
        let f = fx();
        clone_commit(&f, "feat/x", "src/only-here.rs", "pub fn unique() {}\n");
        let items = scan(&f.implement);
        let env = FakeEnv::new();
        let r = run(&f, &items, &env, false);

        match parked_reason(&r, &f.clone) {
            rp::ParkReason::NotOnSourceMain { verdict, .. } => assert_eq!(verdict, "no"),
            other => panic!("expected a containment refusal, got {other:?}"),
        }
        assert!(
            f.clone.join("src/only-here.rs").exists(),
            "unique work was destroyed"
        );
        assert!(env.removed().is_empty());
    }

    /// The fail-closed case the plan names explicitly: a squash that REWROTE the tree (a conflict
    /// resolution, a rebase-with-fixups) reads `no`. Discriminates any softening toward
    /// content-equivalence — same file, one different byte, and the clone must be kept.
    #[test]
    fn a_squash_that_rewrote_the_tree_is_parked_fail_closed() {
        let f = fx();
        clone_commit(&f, "feat/x", "src/new.rs", "pub fn b() {}\n");
        source_commit(&f, "src/new.rs", "pub fn b() {} // reworded on landing\n");
        let items = scan(&f.implement);
        let env = FakeEnv::new();
        let r = run(&f, &items, &env, false);
        match parked_reason(&r, &f.clone) {
            rp::ParkReason::NotOnSourceMain { verdict, .. } => assert_eq!(verdict, "no"),
            other => panic!("expected a containment refusal, got {other:?}"),
        }
        assert!(f.clone.exists());
    }

    /// Lookback exhaustion is `unknown`, not `no`. Discriminates a reaper that reads a bounded search's
    /// silence as absence — the S2 fold review's own blocker, here at a boundary where it deletes.
    #[test]
    fn a_landing_beyond_the_lookback_is_parked_as_unknown_not_deleted() {
        let f = fx();
        clone_commit(&f, "feat/x", "src/new.rs", "pub fn b() {}\n");
        source_commit(&f, "src/new.rs", "pub fn b() {}\n"); // the exact tree lands...
        source_commit(&f, "src/later.rs", "pub fn c() {}\n"); // ...then main moves on
        let items = scan(&f.implement);
        let env = FakeEnv::new();
        // A one-commit window cannot reach the matching commit, and main has more history beyond it.
        let r = run_with(&f, &items, &env, false, 0, 1);
        match parked_reason(&r, &f.clone) {
            rp::ParkReason::NotOnSourceMain { verdict, detail } => {
                assert_eq!(verdict, "unknown");
                assert!(detail.contains("lookback"), "unexpected detail: {detail}");
            }
            other => panic!("expected an unknown-verdict refusal, got {other:?}"),
        }
        assert!(f.clone.exists());

        // The SAME clone with the configured default window is deleted — so the park above is the
        // window's doing, not a permanently-closed gate.
        let r2 = run_with(
            &f,
            &items,
            &FakeEnv::new(),
            false,
            0,
            DEFAULT_CLONE_REAP_LOOKBACK,
        );
        assert_eq!(item_for(&r2, &f.clone).outcome, rp::ItemOutcome::Deleted);
    }

    /// F7. Discriminates a "main" resolved by BARE NAME. `git rev-parse main` resolves a TAG named
    /// `main` before any branch, so a source repository whose trunk is called something else — with a
    /// stray `main` tag anywhere in it — hands the D-1 gate the wrong history to search. Here the tag
    /// carries the clone's exact tree and the real trunk does not: the bare-name resolution deletes
    /// unlanded work, the qualified one parks it.
    #[test]
    fn a_tag_named_main_never_stands_in_for_the_source_trunk() {
        let f = fx();
        // The source's trunk is `trunk`; `main` exists only as a tag on a side branch.
        git(&f.source, &["branch", "-m", "main", "trunk"]);
        clone_commit(&f, "feat/x", "new.rs", "pub fn b() {}\n");
        git(&f.source, &["checkout", "-q", "-b", "side"]);
        source_commit(&f, "new.rs", "pub fn b() {}\n"); // the clone's exact tree, NOT on trunk
        git(&f.source, &["tag", "main"]);
        git(&f.source, &["checkout", "-q", "trunk"]);

        let items = scan(&f.implement);
        let env = FakeEnv::new();
        let r = run(&f, &items, &env, false);
        match parked_reason(&r, &f.clone) {
            rp::ParkReason::NotOnSourceMain { verdict, .. } => assert_eq!(verdict, "no"),
            other => panic!("a tag named `main` was read as the trunk: {other:?}"),
        }
        assert!(f.clone.exists(), "unlanded work was deleted");
        // And the resolution names the branch it actually used, fully qualified.
        let (main_ref, _) = sr::on_source_main_with_lookback(
            &f.source,
            &f.clone,
            &sr::git_str(&f.clone, &["rev-parse", "HEAD"]).unwrap(),
            DEFAULT_CLONE_REAP_LOOKBACK,
        );
        assert_eq!(main_ref.as_deref(), Some("refs/heads/trunk"));
    }

    /// Discriminates a reaper that answers containment from the clone's own refs. A hosted origin means
    /// there is no local repository to ask, and this command contacts no network — so it parks rather
    /// than falling back to `refs/remotes/origin/*`, which is a frozen snapshot from clone time.
    #[test]
    fn a_hosted_origin_parks_because_there_is_no_local_source_to_ask() {
        let f = fx();
        git(
            &f.clone,
            &[
                "remote",
                "set-url",
                "origin",
                "https://example.invalid/repo.git",
            ],
        );
        let items = scan(&f.implement);
        let env = FakeEnv::new();
        let r = run(&f, &items, &env, false);
        assert!(matches!(
            parked_reason(&r, &f.clone),
            rp::ParkReason::OriginNotLocal { .. }
        ));
        assert!(f.clone.exists());
    }

    // -----------------------------------------------------------------------------------------
    // F1 — every ref, not just HEAD. Containment proves HEAD; deletion takes the whole object store.
    // -----------------------------------------------------------------------------------------

    /// THE structural gap. `head_guard` (implement.rs) deliberately LEAVES a clone whose agent
    /// switched branch or committed itself — "leaving clone for the operator" — and `restore_branch`
    /// (tweak.rs) puts HEAD back without ever deleting the branch the agent made. So a clone whose HEAD
    /// is perfectly landed can still be the only copy of a rogue agent branch. Discriminates a gate that
    /// asks the D-1 question of HEAD alone and then deletes the entire object store.
    #[test]
    fn a_landed_head_with_an_unlanded_side_branch_is_parked() {
        let f = fx();
        git(&f.clone, &["checkout", "-q", "-b", "agent-went-rogue"]);
        write(
            &f.clone.join("only-on-that-branch.rs"),
            "pub fn unique() {}\n",
        );
        git(&f.clone, &["add", "-A"]);
        git(&f.clone, &["commit", "-qm", "the agent committed itself"]);
        // HEAD goes back to the landed branch — exactly what `restore_branch` does.
        git(&f.clone, &["checkout", "-q", "main"]);

        let items = scan(&f.implement);
        let env = FakeEnv::new();
        let r = run(&f, &items, &env, false);
        match parked_reason(&r, &f.clone) {
            rp::ParkReason::RefsNotContained { detail } => assert!(
                detail.contains("agent-went-rogue"),
                "the refusal does not name the ref: {detail}"
            ),
            other => panic!("a unique side branch was deleted with the clone: {other:?}"),
        }
        assert!(
            f.clone.join(".git").exists(),
            "the object store was destroyed"
        );
    }

    /// `refs/stash` is constructible in a `:rw` clone and is reachable from no branch. Its commits are
    /// the operator's uncommitted work, deliberately parked out of the way — the most surprising thing
    /// to lose. Discriminates a ref sweep that only walks `refs/heads`.
    #[test]
    fn a_landed_clone_with_a_stash_is_parked() {
        let f = fx();
        write(
            &f.clone.join("src/lib.rs"),
            "pub fn a() {} // work in progress\n",
        );
        git(&f.clone, &["stash", "push", "-q", "-m", "wip"]);
        // The working tree is clean again: `git status --porcelain` says nothing at all.
        assert!(
            sr::git_str(&f.clone, &["status", "--porcelain"])
                .unwrap()
                .is_empty(),
            "fixture must model the case porcelain cannot see"
        );

        let items = scan(&f.implement);
        let env = FakeEnv::new();
        let r = run(&f, &items, &env, false);
        match parked_reason(&r, &f.clone) {
            rp::ParkReason::RefsNotContained { detail } => {
                assert!(detail.contains("stash"), "unexpected detail: {detail}")
            }
            other => panic!("a stashed change was deleted with the clone: {other:?}"),
        }
        assert!(f.clone.exists());
    }

    /// The other half, so the gate is not "park whenever a second ref exists": a healthy clone carries
    /// origin's default branch AND the task branch, and must still proceed. Discriminates a ref gate
    /// that parks the entire real population it was built to clean.
    #[test]
    fn a_healthy_two_ref_clone_still_proceeds() {
        let f = fx();
        // The ordinary shape: `main` from the clone, plus the task branch that descends from it.
        clone_commit(&f, "feat/x", "new.rs", "pub fn b() {}\n");
        source_commit(&f, "new.rs", "pub fn b() {}\n");
        let refs = sr::git_str(
            &f.clone,
            &["for-each-ref", "--format=%(refname)", "refs/heads/"],
        )
        .unwrap();
        assert_eq!(refs.lines().count(), 2, "fixture should carry two branches");

        let items = scan(&f.implement);
        let env = FakeEnv::new();
        let r = run(&f, &items, &env, false);
        assert_eq!(item_for(&r, &f.clone).outcome, rp::ItemOutcome::Deleted);
        let gates = &item_for(&r, &f.clone).gates;
        assert!(
            gates.iter().any(|g| g.contains("refs:") && g.contains("2")),
            "the ref gate left no evidence of what it checked: {gates:?}"
        );
    }

    /// A tag reachable from nowhere else is unique custody too, and an annotated tag on a
    /// non-commit object cannot be judged at all — both park.
    #[test]
    fn an_unlanded_tag_is_parked() {
        let f = fx();
        git(&f.clone, &["checkout", "-q", "-b", "tmp"]);
        write(&f.clone.join("tagged.rs"), "pub fn tagged() {}\n");
        git(&f.clone, &["add", "-A"]);
        git(&f.clone, &["commit", "-qm", "tagged work"]);
        git(&f.clone, &["tag", "keepsake"]);
        // Delete the branch: the commit now hangs off the TAG alone.
        git(&f.clone, &["checkout", "-q", "main"]);
        git(&f.clone, &["branch", "-qD", "tmp"]);

        let items = scan(&f.implement);
        let env = FakeEnv::new();
        let r = run(&f, &items, &env, false);
        match parked_reason(&r, &f.clone) {
            rp::ParkReason::RefsNotContained { detail } => {
                assert!(detail.contains("keepsake"), "unexpected detail: {detail}")
            }
            other => panic!("a tag-only commit was deleted with the clone: {other:?}"),
        }
    }

    // -----------------------------------------------------------------------------------------
    // Git state
    // -----------------------------------------------------------------------------------------

    /// Uncommitted bytes are BY DEFINITION not on main, however good the containment verdict is.
    /// Discriminates a reaper that gates on containment alone — the clone's HEAD is landed here, and
    /// deleting it would still destroy the operator's edit.
    #[test]
    fn a_dirty_clone_is_parked_even_though_its_head_is_landed() {
        let f = fx();
        write(
            &f.clone.join("src/lib.rs"),
            "pub fn a() {} // uncommitted\n",
        );
        let items = scan(&f.implement);
        let env = FakeEnv::new();
        let r = run(&f, &items, &env, false);
        match parked_reason(&r, &f.clone) {
            rp::ParkReason::GitStateNotClean { detail } => {
                assert!(detail.contains("src/lib.rs"), "unexpected detail: {detail}")
            }
            other => panic!("expected a git-state refusal, got {other:?}"),
        }
        assert_eq!(
            std::fs::read_to_string(f.clone.join("src/lib.rs")).unwrap(),
            "pub fn a() {} // uncommitted\n"
        );
    }

    /// Untracked files are invisible to the containment proof. Discriminates a status check that only
    /// looks at tracked modifications.
    #[test]
    fn an_untracked_file_parks_the_clone() {
        let f = fx();
        write(&f.clone.join("scratch-notes.md"), "irreplaceable\n");
        let items = scan(&f.implement);
        let env = FakeEnv::new();
        let r = run(&f, &items, &env, false);
        assert!(matches!(
            parked_reason(&r, &f.clone),
            rp::ParkReason::GitStateNotClean { .. }
        ));
        assert!(f.clone.join("scratch-notes.md").exists());
    }

    /// The other side of the same gate: an IGNORED build target does not park. Discriminates a status
    /// rule so strict that it parks every rust clone in the backlog (each carries an ignored `target/`),
    /// which would make the command inert against the population it exists to clean.
    #[test]
    fn an_ignored_build_target_does_not_park_the_clone() {
        let f = fx();
        write(&f.clone.join("target/debug/blob"), "regenerable\n");
        // F3: the NAME is only a candidate filter — cargo's own markers are the evidence.
        write(&f.clone.join("target/CACHEDIR.TAG"), "Signature: 8a477f5\n");
        let items = scan(&f.implement);
        let env = FakeEnv::new();
        let r = run(&f, &items, &env, false);
        assert_eq!(item_for(&r, &f.clone).outcome, rp::ItemOutcome::Deleted);
        assert!(!f.clone.exists());
    }

    /// An unborn HEAD has no commit to ask about. Discriminates a reaper that treats "no HEAD" as
    /// "nothing of value".
    #[test]
    fn an_unborn_head_parks() {
        let f = fx();
        let empty_id = format!("impl-{DEAD_PID}-bb");
        let empty = f.implement.join(&empty_id);
        std::fs::create_dir_all(&empty).unwrap();
        git(&empty, &["init", "--initial-branch=main", "-q"]);
        write(&empty.join("unsaved.txt"), "x\n");
        let items = scan(&f.implement);
        let env = FakeEnv::new();
        let r = run(&f, &items, &env, false);
        assert_eq!(
            parked_reason(&r, &empty),
            rp::ParkReason::UnbornHead { detail: None }
        );
        assert!(empty.join("unsaved.txt").exists());
    }

    /// F3. Discriminates disposability decided BY NAME. An ignored entry is invisible to the
    /// containment proof, so the name may only ever be a candidate filter: the S3 markers must hold on
    /// disk. Covers all three of the review's cases at once — a marker-less `target/` (unique bytes), a
    /// marker-backed NESTED `crates/foo/target/` (must not be friction), and a FILE named `target`
    /// (which git reports without a trailing slash, and which the old rule waved through).
    #[test]
    fn ignored_entries_are_disposable_only_with_their_on_disk_markers() {
        let td = tempfile::tempdir().unwrap();
        let c = std::fs::canonicalize(td.path()).unwrap();

        // A directory named `target` holding someone's data: NOT regenerable.
        write(&c.join("target/unique.bin"), "irreplaceable\n");
        assert!(
            status_disposition("!! target/\n", &c).is_err(),
            "a marker-less `target/` was treated as disposable"
        );
        // With cargo's own markers it is.
        write(&c.join("target/CACHEDIR.TAG"), "Signature: 8a477f5\n");
        std::fs::create_dir_all(c.join("target/debug")).unwrap();
        assert!(status_disposition("!! target/\n", &c).is_ok());

        // NESTED, marker-backed: a workspace member's target must not be friction.
        write(
            &c.join("crates/foo/target/CACHEDIR.TAG"),
            "Signature: 8a477f5\n",
        );
        std::fs::create_dir_all(c.join("crates/foo/target/debug")).unwrap();
        assert!(
            status_disposition("!! crates/foo/target/\n", &c).is_ok(),
            "a marker-backed nested target parked the clone"
        );

        // A FILE named `target` (git prints no trailing slash) is never disposable.
        write(&c.join("nested/target"), "a file, not a build dir\n");
        assert!(
            status_disposition("!! nested/target\n", &c).is_err(),
            "a FILE named `target` was treated as a build directory"
        );

        // The dependency caches go through S3's own provenance functions.
        write(&c.join("node_modules/pkg/i.js"), "x\n");
        assert!(status_disposition("!! node_modules/\n", &c).is_err());
        write(&c.join("package.json"), "{}\n");
        assert!(status_disposition("!! node_modules/\n", &c).is_ok());
        write(&c.join(".venv/lib/thing.py"), "x\n");
        assert!(status_disposition("!! .venv/\n", &c).is_err());
        write(&c.join(".venv/pyvenv.cfg"), "home = /usr\n");
        assert!(status_disposition("!! .venv/\n", &c).is_ok());

        // Name-shaped near-misses and every non-ignored status line still block.
        assert!(status_disposition("", &c).is_ok());
        for line in [
            "!! my-target-notes/",
            "!! src/target-list.txt",
            "!! .venvy/",
            "!! secrets.env",
            "?? untracked.txt",
            " M tracked.rs",
            "M  staged.rs",
            "A  added.rs",
            "UU conflicted.rs",
            "!! \"weird\\303\\251.txt\"",
            "x",
        ] {
            assert!(
                status_disposition(line, &c).is_err(),
                "{line:?} was accepted as clean"
            );
        }
        // A blocking entry alongside disposable ones still blocks, and the refusal quotes it.
        let e = status_disposition("!! target/\n?? notes.md\n", &c).unwrap_err();
        assert!(
            e.contains("notes.md"),
            "the refusal hides the offender: {e}"
        );
    }

    /// F3, end to end on a live clone: ignored bytes with no provenance are unique custody, and the
    /// clone that holds them must survive even though its HEAD is landed and porcelain is otherwise
    /// silent.
    #[test]
    fn a_landed_clone_whose_ignored_target_has_no_cargo_markers_is_parked() {
        let f = fx();
        write(&f.clone.join("target/unique.bin"), "irreplaceable\n");
        let items = scan(&f.implement);
        let env = FakeEnv::new();
        let r = run(&f, &items, &env, false);
        match parked_reason(&r, &f.clone) {
            rp::ParkReason::GitStateNotClean { detail } => {
                assert!(detail.contains("target"), "unexpected detail: {detail}")
            }
            other => panic!("marker-less ignored bytes were deleted: {other:?}"),
        }
        assert!(f.clone.join("target/unique.bin").exists());
    }

    /// F2. Discriminates a git-state gate that trusts porcelain alone. `--assume-unchanged` tells git
    /// to stop consulting the worktree for a path, so a MODIFIED tracked file produces no porcelain
    /// line at all — the tree looks pristine while carrying bytes on no commit.
    #[test]
    fn an_assume_unchanged_modification_is_invisible_to_porcelain_and_parks() {
        let f = fx();
        git(
            &f.clone,
            &["update-index", "--assume-unchanged", "src/lib.rs"],
        );
        write(
            &f.clone.join("src/lib.rs"),
            "pub fn a() {} // invisible edit\n",
        );
        assert!(
            sr::git_str(&f.clone, &["status", "--porcelain"])
                .unwrap()
                .is_empty(),
            "fixture must model the case porcelain cannot see"
        );

        let items = scan(&f.implement);
        let env = FakeEnv::new();
        let r = run(&f, &items, &env, false);
        match parked_reason(&r, &f.clone) {
            rp::ParkReason::IndexFlagsHideState { detail } => {
                assert!(detail.contains("lib.rs"), "unexpected detail: {detail}")
            }
            other => panic!("an invisible modification was deleted: {other:?}"),
        }
        assert_eq!(
            std::fs::read_to_string(f.clone.join("src/lib.rs")).unwrap(),
            "pub fn a() {} // invisible edit\n"
        );
    }

    /// The same blindness wholesale: a sparse checkout simply has no worktree for paths outside the
    /// cone, and `git status` reports nothing about them.
    #[test]
    fn a_sparse_checkout_parks() {
        let f = fx();
        git(&f.clone, &["config", "core.sparseCheckout", "true"]);
        let items = scan(&f.implement);
        let env = FakeEnv::new();
        let r = run(&f, &items, &env, false);
        assert!(matches!(
            parked_reason(&r, &f.clone),
            rp::ParkReason::IndexFlagsHideState { .. }
        ));
        assert!(f.clone.exists());
    }

    /// The pure index-flag rule, including letters this parser has no name for.
    #[test]
    fn index_flags_disposition_accepts_only_plain_cached_entries() {
        assert!(index_flags_disposition("H a.rs\nH b.rs\n").is_ok());
        assert!(index_flags_disposition("").is_ok());
        for listing in [
            "h assume-unchanged.rs\n",
            "S skip-worktree.rs\n",
            "M unmerged.rs\n",
            "s sparse.rs\n",
            "? mystery.rs\n",
            "H fine.rs\nh hidden.rs\n",
        ] {
            assert!(
                index_flags_disposition(listing).is_err(),
                "{listing:?} was accepted"
            );
        }
        let e = index_flags_disposition("H fine.rs\nh hidden.rs\n").unwrap_err();
        assert!(e.contains("hidden.rs"), "the refusal hides the entry: {e}");
    }

    /// F4. Discriminates a clean-looking superproject. An initialized submodule emits NO porcelain
    /// entry when clean, its object store lives in `<clone>/.git/modules/<name>` and dies with the
    /// clone, and the gitlink SHA in the superproject says where those bytes SHOULD be — never that
    /// they are anywhere else.
    #[test]
    fn an_initialized_submodule_parks_even_when_the_superproject_is_clean() {
        let f = fx();
        let sub = f.root.join("sub-upstream");
        std::fs::create_dir_all(&sub).unwrap();
        git(&sub, &["init", "--initial-branch=main", "-q"]);
        write(&sub.join("sub.rs"), "pub fn s() {}\n");
        git(&sub, &["add", "-A"]);
        git(&sub, &["commit", "-qm", "sub"]);
        git(
            &f.clone,
            &[
                "-c",
                "protocol.file.allow=always",
                "submodule",
                "add",
                "-q",
                sub.to_str().unwrap(),
                "vendor/sub",
            ],
        );
        git(&f.clone, &["commit", "-qm", "add submodule"]);
        assert!(
            sr::real_dir(&f.clone.join(".git/modules")),
            "fixture must have an initialized submodule store"
        );

        let items = scan(&f.implement);
        let env = FakeEnv::new();
        let r = run(&f, &items, &env, false);
        match parked_reason(&r, &f.clone) {
            rp::ParkReason::InitializedSubmodule { detail } => assert!(
                detail.contains("modules") || detail.contains("vendor/sub"),
                "unexpected detail: {detail}"
            ),
            other => panic!("an initialized submodule's store was deleted: {other:?}"),
        }
        assert!(f.clone.join(".git/modules").exists());
    }

    /// The pure submodule rule: `-` is the only prefix that means "no object store here", and an
    /// unparseable line is treated as initialized rather than waved through.
    #[test]
    fn submodule_disposition_treats_anything_but_a_leading_dash_as_initialized() {
        assert!(submodule_disposition("").is_ok());
        assert!(submodule_disposition("-abc123 vendor/sub\n").is_ok());
        for line in [
            " abc123 vendor/sub (v1.0)\n",
            "+abc123 vendor/sub\n",
            "Uabc123 vendor/sub\n",
            "garbage\n",
        ] {
            assert!(
                submodule_disposition(line).is_err(),
                "{line:?} was accepted as uninitialized"
            );
        }
    }

    /// F10(a). Discriminates a containment proof aimed by a value the agent can rewrite.
    /// `remote.origin.url` lives in `<clone>/.git/config` — inside the `:rw` mount — so an agent can
    /// repoint it at any repository that happens to carry a matching tree. The checkpoint is written
    /// before the agent runs; a disagreement is a refusal, not a reconciliation.
    #[test]
    fn an_origin_repointed_away_from_the_checkpoints_source_repo_parks() {
        let f = fx();
        // A decoy repository that DOES contain the clone's content.
        let decoy = f.root.join("decoy");
        let out = Command::new("git")
            .args(["clone", "--no-hardlinks", "-q"])
            .arg(&f.source)
            .arg(&decoy)
            .output()
            .unwrap();
        assert!(out.status.success());
        git(
            &f.clone,
            &["remote", "set-url", "origin", decoy.to_str().unwrap()],
        );
        // The checkpoint still records the repository this run was actually cloned from.
        write(
            &sr::evidence_dir(&f.clone).join("implement-checkpoint.json"),
            &format!(
                "{{\"schema_version\":1,\"task_id\":\"{}\",\"source_repo\":{:?},\"branch\":\"main\"}}",
                f.run_id,
                f.source.to_string_lossy()
            ),
        );

        let items = scan(&f.implement);
        let env = FakeEnv::new();
        let r = run(&f, &items, &env, false);
        match parked_reason(&r, &f.clone) {
            rp::ParkReason::OriginDisagreesWithCheckpoint { detail } => {
                assert!(detail.contains("decoy"), "unexpected detail: {detail}")
            }
            other => panic!("a repointed origin aimed the D-1 proof elsewhere: {other:?}"),
        }
        assert!(f.clone.exists());
    }

    // -----------------------------------------------------------------------------------------
    // Shape / class authority
    // -----------------------------------------------------------------------------------------

    /// Discriminates a boundary that trusts the scan's `checkout_kind`. A linked worktree SHARES its
    /// source repository's object store: `rm -rf` on one corrupts the source's worktree administration
    /// and can destroy commits reachable only from it. The row here LIES (it claims a standalone
    /// clone); only re-derivation from `.git` on disk catches it.
    #[test]
    fn a_worktree_shaped_entry_claiming_to_be_a_clone_is_parked_at_the_boundary() {
        let f = fx();
        let wt_id = format!("impl-{DEAD_PID}-cc");
        let wt = f.implement.join(&wt_id);
        // A real linked worktree of the source repo.
        git(
            &f.source,
            &["worktree", "add", "-q", "-b", "wt", wt.to_str().unwrap()],
        );
        assert!(
            sr::real_file(&wt.join(".git")),
            "fixture is not worktree-shaped"
        );
        let items = vec![sr::ReportItem {
            path: sr::display_path(&wt),
            source: sr::ItemSource::ImplementPath,
            class: sr::PayloadClass::SourceCheckout,
            checkout_kind: Some(sr::CheckoutKind::StandaloneClone), // the lie
            run_id: Some(wt_id.clone()),
            measured: sr::Measured::default(),
            consumers: sr::LiveConsumers::default(),
            git: None,
            note: None,
        }];
        let env = FakeEnv::new();
        let r = run(&f, &items, &env, false);
        assert!(
            matches!(
                parked_reason(&r, &wt),
                rp::ParkReason::NotAStandaloneClone { .. }
            ),
            "a linked worktree was accepted as a standalone clone"
        );
        assert!(wt.join(".git").exists());
        assert!(env.removed().is_empty());
    }

    /// The same refusal one step earlier, from the scan's own classification: `classify_checkout`
    /// refuses a worktree shape under the clone root, so the row never even claims to be a clone.
    /// Discriminates a refusal that reports only "class Unclassified" — on the command whose operator
    /// most needs to know WHICH checkouts it declined to touch and on what grounds, the scan's own
    /// shape note must survive into the park reason.
    #[test]
    fn a_worktree_shaped_entry_is_also_refused_by_the_scans_own_classification() {
        let f = fx();
        let wt = f.implement.join(format!("impl-{DEAD_PID}-cc"));
        git(
            &f.source,
            &["worktree", "add", "-q", "-b", "wt", wt.to_str().unwrap()],
        );
        let items = scan(&f.implement);
        let env = FakeEnv::new();
        let r = run(&f, &items, &env, false);
        match parked_reason(&r, &wt) {
            rp::ParkReason::NotAStandaloneClone { detail } => assert!(
                detail.contains("linked-worktree"),
                "the scan's shape note was lost: {detail}"
            ),
            other => panic!("the scan classified a worktree as reapable: {other:?}"),
        }
        assert!(wt.exists());
    }

    /// Discriminates a reaper that infers volume-vs-path from the path string. The row declares itself a
    /// VOLUME while carrying a real clone's absolute path and the reapable class.
    #[test]
    fn a_volume_row_is_refused_by_its_typed_source() {
        let f = fx();
        let items = vec![sr::ReportItem {
            path: sr::display_path(&f.clone),
            source: sr::ItemSource::VolumeName,
            class: sr::PayloadClass::SourceCheckout,
            checkout_kind: Some(sr::CheckoutKind::StandaloneClone),
            run_id: Some(f.run_id.clone()),
            measured: sr::Measured::default(),
            consumers: sr::LiveConsumers::default(),
            git: None,
            note: None,
        }];
        let env = FakeEnv::new();
        let r = run(&f, &items, &env, false);
        assert_eq!(
            r.items[0].outcome,
            rp::ItemOutcome::Parked {
                reason: rp::ParkReason::ContainerVolume
            }
        );
        assert!(f.clone.exists());
    }

    /// F9. The `--help` JSON note promises consumers a `source` field naming what each `path` is; a
    /// reap record that drops it forces the same volume-vs-path inference back onto whoever reads the
    /// receipt. Discriminates a `ReapItem` that carries only the path string, and pins the wire spelling
    /// the documentation promises (kebab-case, matching `ItemSource::label`).
    #[test]
    fn every_reap_item_carries_the_typed_source_on_the_wire() {
        let f = fx();
        let items = vec![
            sr::ReportItem {
                path: "a2a-verify-cache-0123456789abcdef".into(),
                source: sr::ItemSource::VolumeName,
                class: sr::PayloadClass::DependencyCache,
                checkout_kind: None,
                run_id: None,
                measured: sr::Measured::default(),
                consumers: sr::LiveConsumers::default(),
                git: None,
                note: None,
            },
            sr::ReportItem {
                path: sr::display_path(&f.clone),
                source: sr::ItemSource::ImplementPath,
                class: sr::PayloadClass::SourceCheckout,
                checkout_kind: Some(sr::CheckoutKind::StandaloneClone),
                run_id: Some(f.run_id.clone()),
                measured: sr::Measured::default(),
                consumers: sr::LiveConsumers::default(),
                git: None,
                note: None,
            },
        ];
        let env = FakeEnv::new();
        let r = run(&f, &items, &env, true);
        let json: serde_json::Value =
            serde_json::from_str(&serde_json::to_string(&r).unwrap()).unwrap();
        let rows = json["items"].as_array().unwrap();
        let volume = rows
            .iter()
            .find(|i| i["path"] == "a2a-verify-cache-0123456789abcdef")
            .unwrap();
        assert_eq!(
            volume["source"], "volume-name",
            "a parked volume row does not carry its typed source: {volume}"
        );
        let clone = rows
            .iter()
            .find(|i| i["path"] == sr::display_path(&f.clone))
            .unwrap();
        assert_eq!(clone["source"], "implement-path");
        assert_eq!(clone["outcome"], "planned");
    }

    /// Discriminates a reaper that reaps any `SourceCheckout` it is handed. A `[worktrees]` payload's
    /// custody handle is the ADR-0025 sidecar lease, which this command does not hold — and the S3
    /// review carried exactly this as S4's obligation.
    #[test]
    fn a_worktree_sourced_row_is_refused_by_source() {
        let f = fx();
        let items = vec![sr::ReportItem {
            path: sr::display_path(&f.clone),
            source: sr::ItemSource::WorktreePath,
            class: sr::PayloadClass::SourceCheckout,
            checkout_kind: Some(sr::CheckoutKind::LinkedWorktree),
            run_id: Some(f.run_id.clone()),
            measured: sr::Measured::default(),
            consumers: sr::LiveConsumers::default(),
            git: None,
            note: None,
        }];
        let env = FakeEnv::new();
        let r = run(&f, &items, &env, false);
        assert_eq!(
            r.items[0].outcome,
            rp::ItemOutcome::Parked {
                reason: rp::ParkReason::WorktreeCustody
            }
        );
        assert!(f.clone.exists());
    }

    /// Discriminates a reaper that accepts a path under the root but not equal to `<root>/<run id>` —
    /// e.g. a nested checkout — which would delete a subtree the run id does not name and the operation
    /// lock does not guard.
    #[test]
    fn a_row_that_is_not_exactly_the_run_directory_is_refused() {
        let f = fx();
        let nested = f.clone.join("vendor/inner");
        std::fs::create_dir_all(nested.join(".git")).unwrap();
        let items = vec![sr::ReportItem {
            path: sr::display_path(&nested),
            source: sr::ItemSource::ImplementPath,
            class: sr::PayloadClass::SourceCheckout,
            checkout_kind: Some(sr::CheckoutKind::StandaloneClone),
            run_id: Some(f.run_id.clone()),
            measured: sr::Measured::default(),
            consumers: sr::LiveConsumers::default(),
            git: None,
            note: None,
        }];
        let env = FakeEnv::new();
        let r = run(&f, &items, &env, false);
        assert!(matches!(
            parked_reason(&r, &nested),
            rp::ParkReason::NotUnderScanRoot { .. }
        ));
        assert!(nested.exists());
    }

    /// The exact-mechanism guard as a unit, including the case that matters most: the source repository
    /// must never be removable through this path.
    #[test]
    fn removal_guard_refuses_anything_but_the_run_directory() {
        let f = fx();
        let src = std::fs::canonicalize(&f.source).unwrap();
        assert!(removal_guard(&f.clone, &f.implement, &f.run_id, Some(&src)).is_ok());
        // Wrong run id ⇒ wrong path.
        assert!(removal_guard(&f.clone, &f.implement, "impl-1-other", Some(&src)).is_err());
        // The source repository itself.
        assert!(removal_guard(&src, &f.root, "source", Some(&src)).is_err());
        // A directory with no `.git`.
        let plain_id = "impl-2-plain";
        let plain = f.implement.join(plain_id);
        std::fs::create_dir_all(&plain).unwrap();
        assert!(removal_guard(&plain, &f.implement, plain_id, Some(&src)).is_err());
        // A source repository nested INSIDE the clone would be destroyed with it.
        let inner = f.clone.join("inner-source");
        std::fs::create_dir_all(&inner).unwrap();
        assert!(removal_guard(&f.clone, &f.implement, &f.run_id, Some(&inner)).is_err());

        // S4b: with NO source repository (the F2 licensed path with an unresolved origin chain) the
        // PATH rules are unchanged — only the source-relative clause is absent, because there is no
        // source to relate to. Discriminates a `None` that waves the whole guard through.
        assert!(removal_guard(&f.clone, &f.implement, &f.run_id, None).is_ok());
        assert!(removal_guard(&f.clone, &f.implement, "impl-1-other", None).is_err());
        assert!(removal_guard(&plain, &f.implement, plain_id, None).is_err());
    }

    // -----------------------------------------------------------------------------------------
    // Liveness, locks, consumers
    // -----------------------------------------------------------------------------------------

    /// The load-bearing S3 gate, inherited: an initial `implement` run holds only its ADR-0025 run
    /// lease and never the operation lock, so the lock alone would not exclude it. Discriminates a
    /// clone reaper that deletes the working directory out from under a LIVE run.
    #[test]
    fn a_clone_whose_owning_process_is_alive_is_parked() {
        let f = fx();
        let items = scan(&f.implement);
        let mut env = FakeEnv::new();
        env.alive_pids.insert(DEAD_PID);
        let r = run(&f, &items, &env, false);
        assert_eq!(
            parked_reason(&r, &f.clone),
            rp::ParkReason::RunOwnerAlive { pid: DEAD_PID }
        );
        assert!(f.clone.exists());
        assert!(env.removed().is_empty());
    }

    /// Discriminates a reaper that probes and deletes without the run's operation lock held. For a
    /// clone this lock is the one that matters: `resume` and `merge` operate on this exact directory.
    #[test]
    fn the_operation_lock_is_held_across_probe_and_delete() {
        let f = fx();
        let items = scan(&f.implement);
        let env = FakeEnv::new();
        let _ = run(&f, &items, &env, false);
        let j = env.j.borrow();
        assert!(!j.probe_witness.is_empty(), "no consumer probe ran");
        assert!(
            j.probe_witness.iter().all(|(_, held)| *held),
            "a probe ran without the lock: {:?}",
            j.probe_witness
        );
        assert!(!j.remove_witness.is_empty(), "nothing was removed");
        assert!(
            j.remove_witness.iter().all(|(_, held)| *held),
            "a removal ran without the lock: {:?}",
            j.remove_witness
        );
        assert!(j.locks_held.is_empty(), "the lock outlived the reap");
    }

    /// A held operation lock means a resume or merge owns this clone right now.
    #[test]
    fn a_contended_operation_lock_parks_the_clone() {
        let f = fx();
        let items = scan(&f.implement);
        let mut env = FakeEnv::new();
        env.contended.insert(f.run_id.clone());
        let r = run(&f, &items, &env, false);
        assert!(matches!(
            parked_reason(&r, &f.clone),
            rp::ParkReason::OperationLockHeld { .. }
        ));
        assert!(f.clone.exists());
    }

    /// Discriminates a reaper that reads a failed `lsof` as "nothing found".
    #[test]
    fn a_failed_consumer_probe_parks_and_never_reads_as_free() {
        let f = fx();
        let items = scan(&f.implement);
        let env = FakeEnv::new();
        *env.probe.borrow_mut() = Box::new(|_| rp::ConsumerProbe::Failed {
            detail: "lsof not installed".into(),
        });
        let r = run(&f, &items, &env, false);
        assert!(matches!(
            parked_reason(&r, &f.clone),
            rp::ParkReason::ConsumerProbeFailed { .. }
        ));
        assert!(f.clone.exists());
    }

    /// The host probe runs on the host kernel and cannot see inside a container VM. Discriminates a
    /// reaper that reads an unanswered container axis as permission.
    #[test]
    fn an_unanswered_container_axis_parks_when_a_runtime_is_configured() {
        let f = fx();
        let items = scan(&f.implement); // container_mount defaults to Unknown
        let env = FakeEnv::new();
        let r = run_with(&f, &items, &env, false, 1, DEFAULT_CLONE_REAP_LOOKBACK);
        assert_eq!(
            parked_reason(&r, &f.clone),
            rp::ParkReason::ContainerAxisUnanswered
        );
        assert!(f.clone.exists());
    }

    /// D-2, on the class whose loss is unrecoverable.
    #[test]
    fn a_protected_root_inside_the_clone_refuses_it() {
        let f = fx();
        let items = scan(&f.implement);
        let mut notes = Vec::new();
        let protected = rp::ProtectedRoots::resolve(
            &[f.clone.join("src").to_string_lossy().into_owned()],
            &mut notes,
        )
        .unwrap();
        let env = FakeEnv::new();
        let r = reap_clones(
            ClonesRequest {
                scan_root: &f.implement,
                items: &items,
                protected: &protected,
                dry_run: false,
                runtimes_configured: 0,
                lookback: DEFAULT_CLONE_REAP_LOOKBACK,
                disposition: None,
            },
            &env,
        );
        assert!(matches!(
            parked_reason(&r, &f.clone),
            rp::ParkReason::ProtectedRoot { .. }
        ));
        assert!(f.clone.join("src/lib.rs").exists());
    }

    // -----------------------------------------------------------------------------------------
    // Receipts + evidence preservation
    // -----------------------------------------------------------------------------------------

    /// The plan §7 receipt: the durable identity of a run whose clone is gone. Discriminates a receipt
    /// that records only a path — it must carry the run's identity, the pre-squash HEAD and tree, the
    /// containment verdict AND which ref/commit matched, and the disposition.
    #[test]
    fn the_fold_receipt_carries_the_runs_durable_identity() {
        let f = fx();
        let items = scan(&f.implement);
        let env = FakeEnv::new();
        let head = sr::git_str(&f.clone, &["rev-parse", "HEAD"]).unwrap();
        let tree = sr::git_str(&f.clone, &["rev-parse", "HEAD^{tree}"]).unwrap();
        let r = run(&f, &items, &env, false);
        assert_eq!(item_for(&r, &f.clone).outcome, rp::ItemOutcome::Deleted);

        let v = fold_receipt(&f);
        assert_eq!(v["schema"], FOLD_RECEIPT_SCHEMA);
        assert_eq!(v["run_id"], f.run_id);
        assert_eq!(v["task_id"], f.run_id, "the checkpoint's task id was lost");
        assert_eq!(v["branch"], "main");
        assert_eq!(v["head"], head);
        assert_eq!(v["tree"], tree);
        assert_eq!(v["base"], "deadbeef");
        assert_eq!(v["base_ref"], "main");
        assert_eq!(v["disposition"], DISPOSITION_DELETED);
        assert_eq!(v["at_epoch_secs"], env.now);
        assert!(v["durability"].as_str().unwrap().starts_with("OnMain"));
        assert!(v["containment"]["main_ref"].as_str().is_some());
        assert!(
            v["gates"].as_array().unwrap().len() >= 8,
            "the gate evidence was not recorded: {:?}",
            v["gates"]
        );
        // And it lives in the SIBLING namespace, which the deletion did not touch.
        assert!(receipts_dir(&f.implement)
            .join(fold_receipt_name(&f.run_id))
            .exists());
    }

    /// Evidence has its own retention (plan §5) and must never die with the parent it describes.
    /// Discriminates a reaper that deletes the clone with its checkpoint inside it.
    #[test]
    fn the_runs_evidence_is_copied_out_before_the_clone_is_deleted() {
        let f = fx();
        let original =
            std::fs::read_to_string(sr::evidence_dir(&f.clone).join("implement-checkpoint.json"))
                .unwrap();
        let items = scan(&f.implement);
        let env = FakeEnv::new();
        let r = run(&f, &items, &env, false);
        assert_eq!(item_for(&r, &f.clone).outcome, rp::ItemOutcome::Deleted);
        assert!(!f.clone.exists());

        let preserved = receipts_dir(&f.implement)
            .join(evidence_dir_name(&f.run_id))
            .join("implement-checkpoint.json");
        assert!(
            preserved.exists(),
            "the run's evidence died with its parent: {}",
            preserved.display()
        );
        assert_eq!(std::fs::read_to_string(&preserved).unwrap(), original);
        let v = fold_receipt(&f);
        assert!(v["evidence_preserved_at"].as_str().is_some());
        assert_eq!(v["evidence_files"][0], "implement-checkpoint.json");

        // The copy happened BEFORE the removal, not after it (afterwards there is nothing to copy).
        let kinds = env.j.borrow().kinds();
        let copy = kinds.iter().position(|k| *k == "copy-evidence");
        let remove = kinds.iter().position(|k| *k == "remove");
        assert!(
            copy.is_some() && copy < remove,
            "evidence was not preserved before the removal: {kinds:?}"
        );
    }

    /// F5. A symlinked sidecar is AMBIGUOUS, not absent: a `:rw` agent can point `.git/a2a-bridge` at a
    /// user checkout, and the two wrong answers are symmetric — following it copies a protected tree
    /// into the receipt namespace and calls it this run's evidence, while skipping it deletes the clone
    /// while reporting evidence preserved. Discriminates BOTH: the only correct move is to park.
    #[test]
    #[cfg(unix)]
    fn a_symlinked_evidence_sidecar_parks_rather_than_being_skipped_or_followed() {
        let f = fx();
        let decoy = f.root.join("user-checkout");
        std::fs::create_dir_all(&decoy).unwrap();
        std::fs::write(decoy.join("secret.txt"), b"private").unwrap();
        let items = scan(&f.implement); // scan first: the sidecar is real at scan time
        std::fs::remove_dir_all(sr::evidence_dir(&f.clone)).unwrap();
        std::os::unix::fs::symlink(&decoy, sr::evidence_dir(&f.clone)).unwrap();

        let env = FakeEnv::new();
        let r = run(&f, &items, &env, false);
        assert!(
            matches!(
                parked_reason(&r, &f.clone),
                rp::ParkReason::EvidencePreservationFailed { .. }
            ),
            "a symlinked sidecar was not treated as ambiguous"
        );
        assert!(f.clone.exists(), "the clone was deleted anyway");
        assert!(decoy.join("secret.txt").exists(), "followed the symlink");
        assert!(
            !receipts_dir(&f.implement)
                .join(evidence_dir_name(&f.run_id))
                .exists(),
            "copied a symlink target into the receipt namespace as this run's evidence"
        );
    }

    /// F5. The destination side of the same hazard: `.receipts` itself is inside the scan root a `:rw`
    /// container can write. A symlinked `.receipts` (pointing back into the clone, or anywhere else)
    /// would put the receipt somewhere the deletion destroys — or somewhere it does not belong.
    #[test]
    #[cfg(unix)]
    fn a_symlinked_receipts_namespace_parks() {
        let f = fx();
        let items = scan(&f.implement);
        std::os::unix::fs::symlink(&f.clone, receipts_dir(&f.implement)).unwrap();

        let env = FakeEnv::new();
        let r = run(&f, &items, &env, false);
        assert!(
            matches!(
                parked_reason(&r, &f.clone),
                rp::ParkReason::EvidencePreservationFailed { .. }
            ),
            "a symlinked `.receipts` was written through"
        );
        assert!(f.clone.exists());
        assert!(env.removed().is_empty());
    }

    /// F5. A FIFO inside the sidecar is neither a file to copy nor a directory to walk. Copying it would
    /// BLOCK on open; skipping it would delete the clone while claiming its evidence was preserved.
    #[test]
    #[cfg(unix)]
    fn a_non_regular_entry_inside_the_sidecar_parks() {
        let f = fx();
        let fifo = sr::evidence_dir(&f.clone).join("pipe");
        let c = std::ffi::CString::new(fifo.to_string_lossy().as_bytes()).unwrap();
        // SAFETY: `mkfifo` only creates a filesystem node at the given path.
        assert_eq!(
            unsafe { libc::mkfifo(c.as_ptr(), 0o600) },
            0,
            "mkfifo failed"
        );
        let items = scan(&f.implement);

        let env = FakeEnv::new();
        let r = run(&f, &items, &env, false);
        assert!(
            matches!(
                parked_reason(&r, &f.clone),
                rp::ParkReason::EvidencePreservationFailed { .. }
            ),
            "a non-regular sidecar entry was silently skipped"
        );
        assert!(f.clone.exists());
    }

    /// The other side of F5's distinction: a run with NO sidecar at all is genuinely absent, not
    /// ambiguous. It proceeds — and the receipt must say so rather than claiming a preservation that
    /// never happened.
    #[test]
    fn an_absent_sidecar_proceeds_and_the_receipt_does_not_claim_preservation() {
        let f = fx();
        std::fs::remove_dir_all(sr::evidence_dir(&f.clone)).unwrap();
        let items = scan(&f.implement);
        let env = FakeEnv::new();
        let r = run(&f, &items, &env, false);
        assert_eq!(item_for(&r, &f.clone).outcome, rp::ItemOutcome::Deleted);
        let v = fold_receipt(&f);
        assert!(
            v["evidence_preserved_at"].is_null()
                && v["evidence_files"].as_array().unwrap().is_empty(),
            "the receipt claims preservation that never happened: {v}"
        );
        let gates: Vec<String> = v["gates"]
            .as_array()
            .unwrap()
            .iter()
            .map(|g| g.as_str().unwrap().to_string())
            .collect();
        assert!(
            gates.iter().any(|g| g.contains("no evidence")
                || (g.contains("evidence") && g.contains("nothing to preserve"))),
            "the receipt does not distinguish `absent` from `preserved`: {gates:?}"
        );
    }

    /// F5. `.git/A2A_TASK.md` (the out-of-band task the fix loop re-reads) and `.git/A2A_COMMIT_MSG`
    /// (the agent-written hand-off message) are Evidence that lives beside the sidecar, not inside it.
    /// Discriminates a preservation set defined by one directory path.
    #[test]
    fn the_git_scoped_task_and_commit_message_are_preserved_too() {
        let f = fx();
        write(
            &f.clone.join(".git/A2A_TASK.md"),
            "# the task\nbuild the thing\n",
        );
        write(&f.clone.join(".git/A2A_COMMIT_MSG"), "feat: the thing\n");
        let items = scan(&f.implement);
        let env = FakeEnv::new();
        let r = run(&f, &items, &env, false);
        assert_eq!(item_for(&r, &f.clone).outcome, rp::ItemOutcome::Deleted);

        let kept = receipts_dir(&f.implement).join(evidence_dir_name(&f.run_id));
        assert_eq!(
            std::fs::read_to_string(kept.join("A2A_TASK.md")).unwrap(),
            "# the task\nbuild the thing\n"
        );
        assert_eq!(
            std::fs::read_to_string(kept.join("A2A_COMMIT_MSG")).unwrap(),
            "feat: the thing\n"
        );
        assert!(kept.join("implement-checkpoint.json").exists());
    }

    /// F10(b). The dry run is the document an operator reads BEFORE authorizing a deletion, so it must
    /// not imply that gates it never ran have passed. Discriminates a plan that lists every gate as
    /// satisfied while the preservation and durability gates were never exercised — and pins that the
    /// cheap structural preflight still catches the ambiguous cases the real run would park on.
    #[test]
    #[cfg(unix)]
    fn a_dry_run_names_the_gates_it_did_not_exercise_and_still_catches_ambiguity() {
        let f = fx();
        let items = scan(&f.implement);
        let env = FakeEnv::new();
        let r = run(&f, &items, &env, true);
        let text = render_text(&r);
        assert!(
            text.contains("NOT exercised by a dry run"),
            "the plan does not disclose which gates it skipped:\n{text}"
        );
        assert!(
            text.contains("structural preflight"),
            "the plan does not name the preflight it DID run:\n{text}"
        );

        // And the preflight is not decoration: an ambiguous sidecar parks in the PLAN, so the operator
        // learns about it before authorizing anything.
        let f2 = fx();
        let items2 = scan(&f2.implement);
        std::fs::remove_dir_all(sr::evidence_dir(&f2.clone)).unwrap();
        std::os::unix::fs::symlink(&f2.root, sr::evidence_dir(&f2.clone)).unwrap();
        let r2 = run(&f2, &items2, &FakeEnv::new(), true);
        assert!(
            matches!(
                parked_reason(&r2, &f2.clone),
                rp::ParkReason::EvidencePreservationFailed { .. }
            ),
            "the dry run planned a deletion the real run would refuse"
        );
    }

    /// Discriminates a reaper that proceeds when Evidence could not be preserved. The clone is the only
    /// copy; if the evidence cannot be moved to safety, the clone stays.
    #[test]
    fn an_evidence_copy_failure_parks_the_clone() {
        let f = fx();
        let items = scan(&f.implement);
        let mut env = FakeEnv::new();
        env.copy_error = Some("read-only filesystem".into());
        let r = run(&f, &items, &env, false);
        assert!(matches!(
            parked_reason(&r, &f.clone),
            rp::ParkReason::EvidencePreservationFailed { .. }
        ));
        assert!(f.clone.exists());
        assert!(env.removed().is_empty());
    }

    /// F6. Discriminates a durability barrier that is attempted and then ignored. A receipt file whose
    /// DIRECTORY ENTRY never reached the disk does not survive the crash it exists to describe — so a
    /// barrier failure must park BEFORE the removal, exactly like a failed copy or a failed write.
    #[test]
    fn a_failed_directory_barrier_parks_before_any_removal() {
        let f = fx();
        let items = scan(&f.implement);
        let mut env = FakeEnv::new();
        env.sync_error = Some("EIO on fsync".into());
        let r = run(&f, &items, &env, false);
        match parked_reason(&r, &f.clone) {
            rp::ParkReason::EvidencePreservationFailed { detail } => assert!(
                detail.contains("durability barrier"),
                "unexpected detail: {detail}"
            ),
            other => panic!("a failed fsync did not park the clone: {other:?}"),
        }
        assert!(f.clone.exists());
        assert!(
            env.removed().is_empty(),
            "removed despite an unsynced record"
        );
        // And the barrier ran on BOTH the receipt namespace and its parent.
        let synced: Vec<String> = env
            .j
            .borrow()
            .events
            .iter()
            .filter_map(|e| match e {
                Ev::SyncDir(d) => Some(d.clone()),
                _ => None,
            })
            .collect();
        assert!(
            synced.iter().any(|d| d.ends_with(sr::RECEIPTS_DIR)),
            "the receipt namespace was never fsync'd: {synced:?}"
        );
    }

    /// The barriers must also come BEFORE the removal in time, not merely be present.
    #[test]
    fn the_durability_barriers_precede_the_removal() {
        let f = fx();
        let items = scan(&f.implement);
        let env = FakeEnv::new();
        let _ = run(&f, &items, &env, false);
        let kinds = env.j.borrow().kinds();
        let sync = kinds.iter().position(|k| *k == "sync-dir");
        let remove = kinds.iter().position(|k| *k == "remove");
        assert!(
            sync.is_some() && sync < remove,
            "the durability barrier did not precede the removal: {kinds:?}"
        );
    }

    /// Discriminates a reaper that deletes first and records afterwards. Without the receipt there is
    /// no durable identity for the run at all — which is the entire point of D-1's trade.
    #[test]
    fn an_unwritable_fold_receipt_parks_the_clone_before_any_removal() {
        let f = fx();
        let items = scan(&f.implement);
        let mut env = FakeEnv::new();
        env.write_error = Some("read-only filesystem".into());
        let r = run(&f, &items, &env, false);
        assert!(matches!(
            parked_reason(&r, &f.clone),
            rp::ParkReason::FoldReceiptUnavailable { .. }
        ));
        assert!(f.clone.exists());
        assert!(env.removed().is_empty());
    }

    /// The crash-durability ordering: the intent (`planned_delete`) must land BEFORE the removal, and
    /// the outcome update BEFORE the lock is released. Discriminates a receipt written only at the end,
    /// which describes a state a crash may have prevented from existing, and one written after the
    /// unlock, which a racing resume can interleave with.
    #[test]
    fn the_intent_precedes_the_removal_and_the_outcome_precedes_the_unlock() {
        let f = fx();
        let items = scan(&f.implement);
        let env = FakeEnv::new();
        let _ = run(&f, &items, &env, false);
        let j = env.j.borrow();
        let kinds = j.kinds();
        let first_write = kinds.iter().position(|k| *k == "write");
        let remove = kinds.iter().position(|k| *k == "remove");
        let last_write = kinds.iter().rposition(|k| *k == "write");
        let unlock = kinds.iter().position(|k| *k == "unlock");
        assert!(
            first_write.is_some() && first_write < remove,
            "no fold receipt was written before the removal: {kinds:?}"
        );
        assert!(
            last_write > remove && last_write < unlock,
            "the outcome was not recorded between the removal and the unlock: {kinds:?}"
        );
        // The first write is the INTENT, and it says so.
        let (_, first) = &j.writes[0];
        let v: serde_json::Value = serde_json::from_str(first).unwrap();
        assert_eq!(v["disposition"], DISPOSITION_PLANNED);
        assert_eq!(v["run_id"], f.run_id);
    }

    /// Discriminates a lost outcome record reduced to a printed note. The clone is already gone; a zero
    /// exit status would let an automated caller read "reaped and recorded".
    #[test]
    fn a_lost_outcome_update_is_a_command_failure_not_a_note() {
        let f = fx();
        let items = scan(&f.implement);
        let mut env = FakeEnv::new();
        env.second_write_error = Some("read-only filesystem".into());
        let r = run(&f, &items, &env, false);
        assert!(!env.removed().is_empty(), "nothing was removed to record");
        assert!(
            !r.receipt_failures.is_empty(),
            "a lost outcome update did not surface as a command failure"
        );
        // The intent record survives and still reads `planned_delete` — a truthful trace of a reap
        // whose outcome was not recorded, rather than a receipt claiming success.
        let v = fold_receipt(&f);
        assert_eq!(v["disposition"], DISPOSITION_PLANNED);
        assert!(r.notes.iter().any(|n| n.contains("NOT updated")));
    }

    // -----------------------------------------------------------------------------------------
    // Dry run, truthfulness, reporting
    // -----------------------------------------------------------------------------------------

    /// The dry run IS the plan document. Discriminates one that is only a print flag (it would delete,
    /// copy evidence, or write a receipt) and one that hides its gate evidence in `--json`.
    #[test]
    fn a_dry_run_deletes_nothing_writes_no_receipt_and_shows_its_gates() {
        let f = fx();
        let items = scan(&f.implement);
        let env = FakeEnv::new();
        let r = run(&f, &items, &env, true);

        assert_eq!(item_for(&r, &f.clone).outcome, rp::ItemOutcome::Planned);
        assert!(
            f.clone.join("src/lib.rs").exists(),
            "a dry run deleted the clone"
        );
        assert!(env.removed().is_empty());
        assert!(r.receipts.is_empty() && r.intents.is_empty());
        assert!(
            !receipts_dir(&f.implement).exists(),
            "a dry run created the receipt namespace"
        );
        let text = render_text(&r);
        for expected in [
            "content on main (D-1)",
            "git state",
            "operation lock",
            "consumer probe",
            "run owner",
            "shape",
            "removal guard",
        ] {
            assert!(
                text.contains(expected),
                "no `{expected}` gate line in:\n{text}"
            );
        }
        assert!(
            text.contains(".operation-locks"),
            "the banner does not name its one state-visible effect:\n{text}"
        );
    }

    /// F8. A half-completed removal is the case where every OTHER row in the report becomes a lie: the
    /// clone reads `PARTIAL`, while its children still read `parked` (retained) even though some of
    /// their bytes are gone. Discriminates a projection that only runs for a fully successful removal —
    /// each descendant must be RESTATTED and reported as it actually is, and the presence map must be
    /// on the receipt, because the report is transient and the receipt is the durable record.
    #[test]
    fn a_partial_removal_projects_each_descendants_actual_presence() {
        let f = fx();
        write(&f.clone.join("target/debug/blob"), "regenerable\n");
        write(&f.clone.join("target/CACHEDIR.TAG"), "Signature: 8a477f5\n");
        let items = scan(&f.implement);
        assert!(
            items
                .iter()
                .any(|i| i.class == sr::PayloadClass::BuildTarget),
            "fixture needs a nested payload row"
        );
        let env = FakeEnv::new();
        // Remove the child, then fail: the classic half-done removal.
        *env.remove.borrow_mut() = Box::new(|p| {
            std::fs::remove_dir_all(p.join("target")).map_err(|e| e.to_string())?;
            Err("permission denied on .git".into())
        });
        let r = run(&f, &items, &env, false);

        assert!(
            matches!(
                item_for(&r, &f.clone).outcome,
                rp::ItemOutcome::Partial { .. }
            ),
            "the parent should read PARTIAL"
        );
        let child = item_for(&r, &f.clone.join("target"));
        assert!(
            matches!(child.outcome, rp::ItemOutcome::Deleted),
            "a child whose bytes are GONE still reads {:?}",
            child.outcome
        );
        assert!(
            child.gates.iter().any(|g| g.contains("before the failure")),
            "the child's record does not say how it went: {:?}",
            child.gates
        );
        // A row that really did survive stays retained.
        let evidence = item_for(&r, &sr::evidence_dir(&f.clone));
        assert!(
            matches!(evidence.outcome, rp::ItemOutcome::Parked { .. }),
            "a surviving child was reported as deleted"
        );

        // And the durable record carries the failure detail plus the per-path presence map.
        let v = fold_receipt(&f);
        assert_eq!(v["disposition"], DISPOSITION_PARTIAL);
        assert!(
            v["failure_detail"]
                .as_str()
                .is_some_and(|d| d.contains("permission denied")),
            "the receipt does not record why the removal failed: {v}"
        );
        let presence = v["descendant_presence"].as_array().unwrap();
        let gone = presence
            .iter()
            .find(|p| p["path"].as_str().unwrap().ends_with("/target"))
            .expect("the removed child is missing from the presence map");
        assert_eq!(gone["present"], false);
        assert!(
            presence
                .iter()
                .any(|p| p["path"].as_str().unwrap().contains("a2a-bridge")
                    && p["present"] == true),
            "the surviving child is missing from the presence map: {presence:?}"
        );
    }

    /// The other truthfulness case: the removal errored but the root is GONE, so what it managed to
    /// remove cannot be attested. The outcome is `UNKNOWN` and the presence map is the only honest
    /// statement available about the descendants.
    #[test]
    fn an_unknown_removal_still_records_the_presence_map() {
        let f = fx();
        write(&f.clone.join("target/debug/blob"), "regenerable\n");
        write(&f.clone.join("target/CACHEDIR.TAG"), "Signature: 8a477f5\n");
        let items = scan(&f.implement);
        let env = FakeEnv::new();
        *env.remove.borrow_mut() = Box::new(|p| {
            let _ = std::fs::remove_dir_all(p);
            Err("interrupted".into())
        });
        let r = run(&f, &items, &env, false);
        assert!(matches!(
            item_for(&r, &f.clone).outcome,
            rp::ItemOutcome::Unknown { .. }
        ));
        let v = fold_receipt(&f);
        assert_eq!(v["disposition"], DISPOSITION_UNKNOWN);
        let presence = v["descendant_presence"].as_array().unwrap();
        assert!(
            !presence.is_empty(),
            "no presence map on an UNKNOWN outcome"
        );
        assert!(
            presence.iter().all(|p| p["present"] == false),
            "the whole tree is gone; the map says otherwise: {presence:?}"
        );
    }

    /// Discriminates a reaper that reports a removal it did not complete as success.
    #[test]
    fn a_removal_that_left_the_clone_present_is_recorded_partial() {
        let f = fx();
        let items = scan(&f.implement);
        let env = FakeEnv::new();
        *env.remove.borrow_mut() = Box::new(|p| {
            let _ = std::fs::remove_dir_all(p.join("src"));
            Err("permission denied on .git".into())
        });
        let r = run(&f, &items, &env, false);
        match &item_for(&r, &f.clone).outcome {
            rp::ItemOutcome::Partial { detail } => assert!(detail.contains("permission denied")),
            other => panic!("expected Partial, got {other:?}"),
        }
        assert_eq!(fold_receipt(&f)["disposition"], DISPOSITION_PARTIAL);
    }

    /// Discriminates a report that leaves a deleted clone's children recorded as `parked` — bytes
    /// reported as retained that are provably gone, on the very command whose output is the operator's
    /// record of what happened.
    #[test]
    fn rows_inside_a_deleted_clone_are_recorded_as_deleted_with_it() {
        let f = fx();
        write(&f.clone.join("target/debug/blob"), "regenerable\n");
        write(&f.clone.join("target/CACHEDIR.TAG"), "Signature: 8a477f5\n");
        let items = scan(&f.implement);
        assert!(
            items.iter().any(|i| i.class == sr::PayloadClass::Evidence),
            "fixture should offer a nested evidence row"
        );
        let env = FakeEnv::new();
        let r = run(&f, &items, &env, false);
        for it in &r.items {
            if PathBuf::from(&it.path).starts_with(&f.clone) {
                assert!(
                    matches!(it.outcome, rp::ItemOutcome::Deleted),
                    "{} is inside a deleted clone but reads {:?}",
                    it.path,
                    it.outcome
                );
            }
        }
        assert!(!f.clone.exists());
    }

    /// Discriminates a reaper that reports the payload's own size as the space reclaimed.
    #[test]
    fn freed_space_is_measured_from_the_volume_beside_the_clone_size() {
        let f = fx();
        let items = scan(&f.implement);
        let env = FakeEnv::new();
        let r = run(&f, &items, &env, false);
        let it = item_for(&r, &f.clone);
        assert!(it.logical_bytes.unwrap() > 0);
        assert!(it.disk_bytes.unwrap() > 0);
        assert_eq!(it.freed_bytes_measured, Some(4096));
        assert_eq!(r.free_bytes_before, Some(1_000_000));
        assert_eq!(r.free_bytes_after, Some(1_000_000 + 4096));
    }

    /// The checkpoint reader must survive schema drift: a legacy or half-written checkpoint degrades to
    /// "unknown", never to a fabricated identity, and never blocks the reap.
    #[test]
    fn checkpoint_facts_survive_schema_drift() {
        let full = checkpoint_facts(
            "{\"task_id\":\"impl-7-x\",\"base_commit\":\"abc\",\"base_ref\":\"main\",\
             \"branch\":\"feat/y\",\"phase\":\"Approved\",\"unknown_future_field\":42}",
        );
        assert_eq!(full.task_id.as_deref(), Some("impl-7-x"));
        assert_eq!(full.base.as_deref(), Some("abc"));
        assert_eq!(full.branch.as_deref(), Some("feat/y"));
        // `resume_id` is the pre-rename spelling.
        assert_eq!(
            checkpoint_facts("{\"resume_id\":\"impl-8-y\"}")
                .task_id
                .as_deref(),
            Some("impl-8-y")
        );
        // Garbage, empty strings, and wrong types all read as unknown rather than as values.
        assert_eq!(checkpoint_facts("not json").task_id, None);
        assert_eq!(checkpoint_facts("{\"task_id\":\"\"}").task_id, None);
        assert_eq!(checkpoint_facts("{\"task_id\":7}").task_id, None);
    }

    // =========================================================================================
    // S4b — fixtures shared by F1 (chained origins) and F2 (the owner disposition license)
    // =========================================================================================

    /// `git clone --no-hardlinks <from> <root>/impl-<dead pid>-<suffix>` — the exact shape the
    /// implement loop produces, so a chained run is built the way production builds one.
    fn clone_into_root(f: &Fx, from: &Path, suffix: &str) -> (PathBuf, String) {
        let run_id = format!("impl-{DEAD_PID}-{suffix}");
        let dst = f.implement.join(&run_id);
        let out = Command::new("git")
            .args(["clone", "--no-hardlinks", "-q"])
            .arg(from)
            .arg(&dst)
            .output()
            .unwrap();
        assert!(out.status.success(), "clone into root: {out:?}");
        (dst, run_id)
    }

    /// The checkpoint the bridge writes BEFORE the agent runs — including `source_repo`, the one
    /// statement about provenance the agent did not author.
    fn write_checkpoint(clone: &Path, run_id: &str, source: &Path) {
        write(
            &sr::evidence_dir(clone).join("implement-checkpoint.json"),
            &format!(
                "{{\"schema_version\":1,\"task_id\":\"{run_id}\",\"branch\":\"feat/x\",\
                 \"base_ref\":\"main\",\"source_repo\":{:?}}}",
                source.to_string_lossy()
            ),
        );
    }

    /// Only the rows for `path`. A chained fixture has several clones under one root, and a test about
    /// ONE of them must not have a sibling reaped out from under it mid-run.
    fn items_for(items: &[sr::ReportItem], path: &Path) -> Vec<sr::ReportItem> {
        let want = path.to_string_lossy().into_owned();
        items.iter().filter(|i| i.path == want).cloned().collect()
    }

    fn fold_receipt_for(f: &Fx, run_id: &str) -> serde_json::Value {
        let p = receipts_dir(&f.implement).join(fold_receipt_name(run_id));
        let raw = std::fs::read_to_string(&p)
            .unwrap_or_else(|e| panic!("no fold receipt at {}: {e}", p.display()));
        serde_json::from_str(&raw).unwrap()
    }

    /// An owner-authored license file, written and loaded exactly the way the CLI loads it (so the
    /// sha256 on every receipt below is a real digest of real bytes, not a test fixture's invention).
    fn license(f: &Fx, run_ids: &[&str]) -> DispositionList {
        let p = f.root.join("disposition.json");
        let body = serde_json::json!({
            "authorized_by": "owner (forensic pass 2026-08-08)",
            "reason": "chained implements: work superseded, runs abandoned",
            "run_ids": run_ids,
        });
        std::fs::write(&p, serde_json::to_string_pretty(&body).unwrap()).unwrap();
        load_disposition_list(&p).unwrap()
    }

    // -----------------------------------------------------------------------------------------
    // F1 — chained-origin root resolution
    // -----------------------------------------------------------------------------------------

    /// THE forensic defect. An `implement` run started with `--repo <another run's clone>` records
    /// that SIBLING as its `origin`, so S4 asked the D-1 question of a quarantine directory rather
    /// than of the repository the work lands in — 68 of this host's 81 parked clones were in that
    /// shape, every verdict about the wrong repo.
    ///
    /// The fixture makes the two answers DIFFER on purpose: the work's exact tree is on the SOURCE's
    /// main and is unknown to the sibling. Asked of the sibling the verdict is `no` (assert it, so the
    /// discrimination is proved rather than assumed) and the clone would park forever; asked of the
    /// true root it is `yes(tree)` and the clone is reaped. Discriminates any resolution that stops at
    /// the first `origin`.
    #[test]
    fn a_chained_clone_is_judged_against_the_true_source_not_the_sibling_it_came_from() {
        let f = fx();
        let (b, b_id) = clone_into_root(&f, &f.source, "bb");
        write_checkpoint(&b, &b_id, &f.source);
        let (a, a_id) = clone_into_root(&f, &b, "cc");
        write_checkpoint(&a, &a_id, &b);

        // The sibling then moves on with work of its own that never lands, so it PARKS on its own
        // evidence — the point of this test is which repository answered for the CHAINED clone, and a
        // sibling that got reaped alongside it would leave that unobservable.
        write(&b.join("src/b_only.rs"), "pub fn b_only() {}\n");
        git(&b, &["add", "-A"]);
        git(&b, &["commit", "-qm", "work only in the sibling"]);

        git(&a, &["checkout", "-q", "-b", "feat/chained"]);
        write(&a.join("src/chained.rs"), "pub fn chained() {}\n");
        git(&a, &["add", "-A"]);
        git(&a, &["commit", "-qm", "chained work"]);
        let head = sr::git_str(&a, &["rev-parse", "HEAD"]).unwrap();
        // The same content lands on the SOURCE's main. The sibling never hears about it.
        source_commit(&f, "src/chained.rs", "pub fn chained() {}\n");

        let (_, against_sibling) =
            sr::on_source_main_with_lookback(&b, &a, &head, DEFAULT_CLONE_REAP_LOOKBACK);
        assert_eq!(
            against_sibling.label(),
            "no",
            "the fixture must make the sibling's answer differ from the true source's"
        );

        // The PRODUCTION shape: every scanned row, siblings included. (An earlier version of this
        // test passed only the chained clone's row, which hid the fact that reaping a sibling first
        // used to strand it — see `a_chained_pair_is_reaped_in_one_invocation_whatever_the_order`.)
        let items = scan(&f.implement);
        assert!(
            items.iter().any(|i| i.path == sr::display_path(&b)),
            "the fixture must offer the sibling row too"
        );
        let env = FakeEnv::new();
        let r = run(&f, &items, &env, false);
        assert_eq!(
            item_for(&r, &a).outcome,
            rp::ItemOutcome::Deleted,
            "the chained clone was judged against the wrong repository: {:?}",
            item_for(&r, &a).outcome
        );

        let v = fold_receipt_for(&f, &a_id);
        assert_eq!(v["containment"]["verdict"], "yes(tree)");
        assert_eq!(
            v["source_repo"],
            sr::display_path(&f.source),
            "the receipt does not name the TRUE source"
        );
        let chain = v["origin_chain"]
            .as_array()
            .expect("no origin chain on the receipt");
        assert_eq!(chain.len(), 2, "the full chain was not recorded: {chain:?}");
        assert!(
            chain[0].as_str().unwrap().contains(&sr::display_path(&b)),
            "hop 1 does not name the sibling: {chain:?}"
        );
        assert!(
            chain[1]
                .as_str()
                .unwrap()
                .contains(&sr::display_path(&f.source)),
            "hop 2 does not name the source: {chain:?}"
        );
        // The sibling is untouched — it carries work of its own that never landed, so the SAME
        // invocation that reaped the chained clone kept its parent, on that parent's own evidence.
        assert!(b.join(".git").exists(), "the chain walk deleted a hop");
        assert!(matches!(
            parked_reason(&r, &b),
            rp::ParkReason::NotOnSourceMain { .. }
        ));
        // R4: the terminal root was not merely reached, it was corroborated against the clone's own
        // history before any question was asked of it.
        assert!(
            item_for(&r, &a)
                .gates
                .iter()
                .any(|g| g.contains("terminal root corroborated") && g.contains("lineage root")),
            "the chain was accepted without corroborating the root: {:?}",
            item_for(&r, &a).gates
        );
    }

    /// The ordinary directly-cloned run must still resolve in ONE hop and record it. Discriminates a
    /// resolver that only works for chains, or one that silently walks past the real source.
    #[test]
    fn a_directly_cloned_run_records_a_single_hop_chain() {
        let f = fx();
        let items = scan(&f.implement);
        let env = FakeEnv::new();
        let r = run(&f, &items, &env, false);
        assert_eq!(item_for(&r, &f.clone).outcome, rp::ItemOutcome::Deleted);
        let v = fold_receipt(&f);
        let chain = v["origin_chain"].as_array().unwrap();
        assert_eq!(chain.len(), 1, "unexpected chain: {chain:?}");
        assert_eq!(v["source_repo"], sr::display_path(&f.source));
    }

    /// A chain that runs into something that is not a repository, and a chain that runs into a path
    /// that is not there at all. Both are the SAME failure — the true source cannot be identified —
    /// and both must park with verdict `unknown` (not `no`, which would assert the content is
    /// demonstrably absent from a repository nobody found) with the walk recorded for the operator.
    #[test]
    fn a_broken_origin_chain_parks_unknown_with_the_walk_recorded() {
        let f = fx();
        let (b, _) = clone_into_root(&f, &f.source, "bb");
        let (a, _) = clone_into_root(&f, &b, "cc");

        // (1) the next hop EXISTS but is not a repository — a half-removed quarantine directory.
        let stump = f.implement.join(format!("impl-{DEAD_PID}-dd"));
        std::fs::create_dir_all(stump.join("src")).unwrap();
        git(
            &b,
            &["remote", "set-url", "origin", stump.to_str().unwrap()],
        );
        let items = items_for(&scan(&f.implement), &a);
        let r = run(&f, &items, &FakeEnv::new(), false);
        match parked_reason(&r, &a) {
            rp::ParkReason::NotOnSourceMain { verdict, detail } => {
                assert_eq!(
                    verdict, "unknown",
                    "a broken chain read as a definite answer"
                );
                assert!(
                    detail.contains("NOT a git repository"),
                    "the refusal does not say what broke: {detail}"
                );
                assert!(
                    detail.contains(&sr::display_path(&b)),
                    "the chain walked so far is not recorded: {detail}"
                );
            }
            other => panic!("expected an unknown-verdict park, got {other:?}"),
        }
        assert!(
            a.join(".git").exists(),
            "a clone with no identified source was deleted"
        );

        // (2) the next hop is simply NOT THERE.
        let gone = f.implement.join(format!("impl-{DEAD_PID}-ee"));
        git(&b, &["remote", "set-url", "origin", gone.to_str().unwrap()]);
        let r = run(&f, &items, &FakeEnv::new(), false);
        match parked_reason(&r, &a) {
            rp::ParkReason::NotOnSourceMain { verdict, detail } => {
                assert_eq!(verdict, "unknown");
                assert!(
                    detail.contains("not a local directory"),
                    "unexpected detail: {detail}"
                );
                assert!(
                    detail.contains("origin chain walked so far"),
                    "no walk: {detail}"
                );
            }
            other => panic!("expected an unknown-verdict park, got {other:?}"),
        }
        assert!(a.exists());
    }

    /// `A -> B -> A`. Without cycle detection the resolver loops until the hop limit; with neither it
    /// never returns. Discriminates a walk that only bounds its length.
    #[test]
    fn an_origin_cycle_parks() {
        let f = fx();
        let (b, _) = clone_into_root(&f, &f.source, "bb");
        let (a, _) = clone_into_root(&f, &b, "cc");
        git(&b, &["remote", "set-url", "origin", a.to_str().unwrap()]);

        let items = items_for(&scan(&f.implement), &a);
        let r = run(&f, &items, &FakeEnv::new(), false);
        match parked_reason(&r, &a) {
            rp::ParkReason::NotOnSourceMain { verdict, detail } => {
                assert_eq!(verdict, "unknown");
                assert!(detail.contains("CYCLE"), "unexpected detail: {detail}");
            }
            other => panic!("expected an unknown-verdict park, got {other:?}"),
        }
        assert!(a.exists());
    }

    /// The bound itself. Nine in-root hops is not a workflow, it is a config someone wrote; the walk
    /// stops and parks rather than continuing to trust the next `remote.origin.url`.
    #[test]
    fn an_origin_chain_past_the_hop_limit_parks() {
        let f = fx();
        // h1..h9 are bare-bones repositories inside the scan root, each pointing at the next; h9
        // points at the real source, which is one hop too far to ever be reached.
        let mut names: Vec<PathBuf> = Vec::new();
        for i in 1..=MAX_ORIGIN_HOPS + 1 {
            let p = f.implement.join(format!("impl-{DEAD_PID}-h{i}"));
            std::fs::create_dir_all(&p).unwrap();
            git(&p, &["init", "--initial-branch=main", "-q"]);
            names.push(p);
        }
        for i in 0..names.len() {
            let next = names
                .get(i + 1)
                .cloned()
                .unwrap_or_else(|| f.source.clone());
            git(
                &names[i],
                &["remote", "add", "origin", next.to_str().unwrap()],
            );
        }
        let (a, _) = clone_into_root(&f, &f.source, "cc");
        git(
            &a,
            &["remote", "set-url", "origin", names[0].to_str().unwrap()],
        );

        let items = items_for(&scan(&f.implement), &a);
        let r = run(&f, &items, &FakeEnv::new(), false);
        match parked_reason(&r, &a) {
            rp::ParkReason::NotOnSourceMain { verdict, detail } => {
                assert_eq!(verdict, "unknown");
                // The message must describe what actually happened: the walk stayed INSIDE the root
                // for the whole bound. (An earlier wording claimed that unconditionally, including
                // when the last edge did leave the root — a false statement on a refusal.)
                assert!(
                    detail.contains(&format!(
                        "has followed {} edges without leaving the scan root",
                        MAX_ORIGIN_HOPS + 1
                    )) && detail.contains(&format!("limit {MAX_ORIGIN_HOPS}")),
                    "unexpected detail: {detail}"
                );
            }
            other => panic!("expected an unknown-verdict park, got {other:?}"),
        }
        assert!(a.exists());
    }

    /// The per-hop checkpoint cross-check. S4 bound the clone's own `origin` to the source its
    /// checkpoint recorded; a chain is only as trustworthy as its weakest hop, and every hop's
    /// `origin` lives in a directory a `:rw` agent could write. Discriminates a walk that applies the
    /// cross-check to the first edge alone.
    #[test]
    fn a_chain_hop_that_disagrees_with_its_own_checkpoint_parks() {
        let f = fx();
        let (b, b_id) = clone_into_root(&f, &f.source, "bb");
        // B's checkpoint says it was cloned from the source; its `origin` now says otherwise.
        write_checkpoint(&b, &b_id, &f.source);
        let elsewhere = f.root.join("elsewhere");
        std::fs::create_dir_all(&elsewhere).unwrap();
        git(&elsewhere, &["init", "--initial-branch=main", "-q"]);
        write(&elsewhere.join("README.md"), "one\n");
        git(&elsewhere, &["add", "-A"]);
        git(&elsewhere, &["commit", "-qm", "one"]);
        git(
            &b,
            &["remote", "set-url", "origin", elsewhere.to_str().unwrap()],
        );

        let (a, a_id) = clone_into_root(&f, &b, "cc");
        write_checkpoint(&a, &a_id, &b);

        let items = items_for(&scan(&f.implement), &a);
        let r = run(&f, &items, &FakeEnv::new(), false);
        match parked_reason(&r, &a) {
            rp::ParkReason::OriginDisagreesWithCheckpoint { detail } => {
                assert!(detail.contains("hop 2"), "the hop is not named: {detail}");
                assert!(
                    detail.contains(&sr::display_path(&b)),
                    "the disagreeing run is not named: {detail}"
                );
            }
            other => panic!("expected a checkpoint disagreement, got {other:?}"),
        }
        assert!(a.exists());
    }

    // -----------------------------------------------------------------------------------------
    // F2 — the owner disposition license: the document
    // -----------------------------------------------------------------------------------------

    /// A license is a DELETION AUTHORIZATION, so every way of misreading it is refused rather than
    /// interpreted. Discriminates a lenient decode — most importantly `deny_unknown_fields`, without
    /// which `{"runs": [...]}` parses as a license naming nobody, or a future field with real meaning
    /// is silently dropped by an old binary.
    #[test]
    fn a_disposition_document_is_parsed_strictly() {
        let ok = parse_disposition_document(
            "{\"authorized_by\":\"owner\",\"reason\":\"abandoned\",\"run_ids\":[\"impl-1-a\"]}",
        )
        .unwrap();
        assert_eq!(ok.run_ids, vec!["impl-1-a".to_string()]);

        for (bad, why) in [
            (
                "{\"authorized_by\":\"o\",\"reason\":\"r\",\"run_ids\":[\"impl-1-a\"],\"note\":\"x\"}",
                "an unknown field",
            ),
            (
                "{\"authorized_by\":\"o\",\"reason\":\"r\",\"runs\":[\"impl-1-a\"]}",
                "a misspelled run_ids key",
            ),
            (
                "{\"authorized_by\":\"\",\"reason\":\"r\",\"run_ids\":[\"impl-1-a\"]}",
                "an unattributed license",
            ),
            (
                "{\"authorized_by\":\"o\",\"reason\":\" \",\"run_ids\":[\"impl-1-a\"]}",
                "a license with no reason",
            ),
            (
                "{\"authorized_by\":\"o\",\"reason\":\"r\",\"run_ids\":[]}",
                "a license naming no run",
            ),
            (
                "{\"authorized_by\":\"o\",\"reason\":\"r\",\"run_ids\":[\"../impl-1-a\"]}",
                "a run id that is not one path component",
            ),
            (
                "{\"authorized_by\":\"o\",\"reason\":\"r\",\"run_ids\":[\"a/b\"]}",
                "a run id with a separator",
            ),
            ("", "an empty document"),
            ("[\"impl-1-a\"]", "a bare array"),
        ] {
            assert!(
                parse_disposition_document(bad).is_err(),
                "{why} was accepted: {bad}"
            );
        }
    }

    /// The file itself, no-follow. A symlinked or non-regular license would put the bytes HASHED onto
    /// the receipt and the path RECORDED on it out of correspondence — which is the whole of what
    /// makes the license auditable. Discriminates a plain `read_to_string`.
    #[test]
    fn a_license_file_must_be_a_regular_non_empty_file() {
        let td = tempfile::tempdir().unwrap();
        let dir = td.path();
        let real = dir.join("real.json");
        std::fs::write(
            &real,
            "{\"authorized_by\":\"o\",\"reason\":\"r\",\"run_ids\":[\"impl-1-a\"]}",
        )
        .unwrap();
        let loaded = load_disposition_list(&real).unwrap();
        assert_eq!(loaded.list_sha256.len(), 64);
        assert!(loaded.licenses("impl-1-a"));
        assert!(
            !loaded.licenses("impl-1-b"),
            "a license matched a run it does not name"
        );

        let link = dir.join("link.json");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let e = load_disposition_list(&link).unwrap_err();
        assert!(e.contains("SYMLINK"), "unexpected refusal: {e}");

        let empty = dir.join("empty.json");
        std::fs::write(&empty, "").unwrap();
        assert!(load_disposition_list(&empty).is_err());

        assert!(load_disposition_list(&dir.join("nope.json")).is_err());
        assert!(
            load_disposition_list(dir).is_err(),
            "a directory was accepted as a license"
        );

        // The digest is of the file's bytes: an edited list cannot describe an earlier deletion.
        std::fs::write(
            &real,
            "{\"authorized_by\":\"o\",\"reason\":\"r2\",\"run_ids\":[\"impl-1-a\"]}",
        )
        .unwrap();
        assert_ne!(
            load_disposition_list(&real).unwrap().list_sha256,
            loaded.list_sha256
        );
    }

    // -----------------------------------------------------------------------------------------
    // F2 — the owner disposition license: what it replaces, and what it does NOT
    // -----------------------------------------------------------------------------------------

    /// The feature. Work that exists ONLY in the clone reads `no` and parks forever under D-1 alone
    /// (see `a_branch_only_clone_is_parked_with_its_work_intact`); with the owner's written
    /// disposition it is reaped, and the receipt says exactly that — an owner license, naming who,
    /// why, from which file, at which digest, and NEVER a containment claim.
    #[test]
    fn a_dispositioned_clone_with_verdict_no_is_deleted_under_the_license() {
        let f = fx();
        clone_commit(&f, "feat/x", "src/only-here.rs", "pub fn unique() {}\n");
        let lic = license(&f, &[f.run_id.as_str()]);
        let items = scan(&f.implement);
        let env = FakeEnv::new();
        let r = run_licensed(
            &f,
            &items,
            &env,
            false,
            0,
            DEFAULT_CLONE_REAP_LOOKBACK,
            Some(&lic),
        );
        assert_eq!(item_for(&r, &f.clone).outcome, rp::ItemOutcome::Deleted);
        assert!(!f.clone.exists());

        let v = fold_receipt(&f);
        assert_eq!(v["containment"]["verdict"], VERDICT_OWNER_DISPOSITION);
        assert_eq!(v["containment"]["authorized_by"], lic.authorized_by);
        assert_eq!(v["containment"]["reason"], lic.reason);
        assert_eq!(v["containment"]["list_file"], lic.list_file);
        assert_eq!(v["containment"]["list_sha256"], lic.list_sha256);
        assert_eq!(v["disposition"], DISPOSITION_DELETED);
        // The wording discipline, asserted rather than trusted: nothing on this receipt may read as a
        // content-on-main claim.
        assert!(
            v["durability"]
                .as_str()
                .unwrap()
                .starts_with("OwnerDisposition{"),
            "durability claims a landing: {}",
            v["durability"]
        );
        assert!(v["containment"]["main_ref"].is_null());
        assert!(v["containment"]["matched_commit"].is_null());
        let text = serde_json::to_string(&v).unwrap();
        for forbidden in ["yes(head)", "yes(tree)", "OnMain{"] {
            assert!(
                !text.contains(forbidden),
                "the licensed receipt claims {forbidden}: {text}"
            );
        }
        // The disclaimer is part of the record, not decoration: a reader who only ever sees this
        // field must be told, in it, that no containment proof exists (R7).
        assert!(
            v["containment"]["detail"]
                .as_str()
                .unwrap()
                .contains("This is a LICENSE, NOT a containment proof"),
            "the receipt omits the disclaimer: {}",
            v["containment"]["detail"]
        );
        // The run's own work is preserved as EVIDENCE, exactly as an unlicensed reap preserves it.
        assert!(
            !r.receipts.is_empty(),
            "no receipt was recorded for a licensed deletion"
        );
    }

    /// THE fail-closed interaction. `git status` says these bytes are on no commit, and an owner
    /// cannot have dispositioned bytes nobody has read: the license replaces the content-on-main
    /// proof, never the dirty-state gate. Discriminates a license implemented as a general bypass —
    /// the shape this slice deliberately built first and watched fail here.
    #[test]
    fn a_dispositioned_dirty_clone_parks_with_the_conflict_named() {
        let f = fx();
        write(
            &f.clone.join("src/uncommitted.rs"),
            "pub fn nobody_read_me() {}\n",
        );
        let lic = license(&f, &[f.run_id.as_str()]);
        let items = scan(&f.implement);
        let env = FakeEnv::new();
        let r = run_licensed(
            &f,
            &items,
            &env,
            false,
            0,
            DEFAULT_CLONE_REAP_LOOKBACK,
            Some(&lic),
        );
        match parked_reason(&r, &f.clone) {
            rp::ParkReason::GitStateNotClean { detail } => {
                assert!(
                    detail.contains("uncommitted.rs"),
                    "the refusal does not name the state: {detail}"
                );
                assert!(
                    detail.contains("owner disposition list"),
                    "the refusal does not name the conflict with the license: {detail}"
                );
            }
            other => panic!("a DIRTY dispositioned clone was not parked: {other:?}"),
        }
        assert!(
            f.clone.join("src/uncommitted.rs").exists(),
            "bytes on no commit were destroyed under a license that never covered them"
        );
        assert!(env.removed().is_empty());
        assert!(
            r.notes.iter().any(|n| n.contains("PARKED anyway")),
            "the report does not disclose that a licensed run parked: {:?}",
            r.notes
        );
    }

    /// The other gate the license must not touch: a LIVE owning process. `implement` holds only its
    /// ADR-0025 run lease, so nothing else excludes it — and an owner dispositioning a run cannot
    /// have meant "delete the working directory out from under it while it runs".
    #[test]
    fn a_dispositioned_clone_whose_owner_is_alive_parks() {
        let f = fx();
        clone_commit(&f, "feat/x", "src/only-here.rs", "pub fn unique() {}\n");
        let lic = license(&f, &[f.run_id.as_str()]);
        let items = scan(&f.implement);
        let mut env = FakeEnv::new();
        env.alive_pids.insert(DEAD_PID);
        let r = run_licensed(
            &f,
            &items,
            &env,
            false,
            0,
            DEFAULT_CLONE_REAP_LOOKBACK,
            Some(&lic),
        );
        assert_eq!(
            parked_reason(&r, &f.clone),
            rp::ParkReason::RunOwnerAlive { pid: DEAD_PID }
        );
        assert!(f.clone.exists());
        assert!(env.removed().is_empty());
        assert!(r.notes.iter().any(|n| n.contains("PARKED anyway")));
    }

    /// The remaining gates in the "everything else still runs" list, one invocation each, so a
    /// bypass that skipped any ONE of them cannot hide behind the others.
    #[test]
    fn a_dispositioned_clone_still_answers_to_every_other_gate() {
        // (1) the operation lock — `resume` and `merge` take this exact lock on this exact directory.
        let f = fx();
        let lic = license(&f, &[f.run_id.as_str()]);
        let items = scan(&f.implement);
        let mut env = FakeEnv::new();
        env.contended.insert(f.run_id.clone());
        let r = run_licensed(
            &f,
            &items,
            &env,
            false,
            0,
            DEFAULT_CLONE_REAP_LOOKBACK,
            Some(&lic),
        );
        assert!(matches!(
            parked_reason(&r, &f.clone),
            rp::ParkReason::OperationLockHeld { .. }
        ));
        assert!(f.clone.exists());

        // (2) the host consumer probe.
        let f = fx();
        let lic = license(&f, &[f.run_id.as_str()]);
        let items = scan(&f.implement);
        let env = FakeEnv::new();
        *env.probe.borrow_mut() = Box::new(|_| rp::ConsumerProbe::Held {
            detail: "pid 4242 has an open file here".into(),
        });
        let r = run_licensed(
            &f,
            &items,
            &env,
            false,
            0,
            DEFAULT_CLONE_REAP_LOOKBACK,
            Some(&lic),
        );
        assert!(matches!(
            parked_reason(&r, &f.clone),
            rp::ParkReason::LiveConsumer { .. }
        ));
        assert!(f.clone.exists());

        // (3) the container axis: a runtime is configured and did not answer for this payload.
        let f = fx();
        let lic = license(&f, &[f.run_id.as_str()]);
        let items = scan(&f.implement);
        let env = FakeEnv::new();
        let r = run_licensed(
            &f,
            &items,
            &env,
            false,
            1,
            DEFAULT_CLONE_REAP_LOOKBACK,
            Some(&lic),
        );
        assert!(matches!(
            parked_reason(&r, &f.clone),
            rp::ParkReason::ContainerAxisUnanswered
        ));
        assert!(f.clone.exists());

        // (4) the shape gate: an ambiguous `.git` proves nothing, licensed or not.
        let f = fx();
        let lic = license(&f, &[f.run_id.as_str()]);
        let items = scan(&f.implement);
        std::fs::remove_dir_all(f.clone.join(".git")).unwrap();
        std::fs::write(f.clone.join(".git"), "gitdir: /elsewhere\n").unwrap();
        let env = FakeEnv::new();
        let r = run_licensed(
            &f,
            &items,
            &env,
            false,
            0,
            DEFAULT_CLONE_REAP_LOOKBACK,
            Some(&lic),
        );
        assert!(matches!(
            parked_reason(&r, &f.clone),
            rp::ParkReason::NotAStandaloneClone { .. }
        ));
        assert!(f.clone.exists());

        // (5) evidence preservation: an AMBIGUOUS sidecar still refuses.
        let f = fx();
        let lic = license(&f, &[f.run_id.as_str()]);
        let items = scan(&f.implement);
        let sidecar = sr::evidence_dir(&f.clone);
        std::fs::remove_dir_all(&sidecar).unwrap();
        std::os::unix::fs::symlink(&f.source, &sidecar).unwrap();
        let env = FakeEnv::new();
        let r = run_licensed(
            &f,
            &items,
            &env,
            false,
            0,
            DEFAULT_CLONE_REAP_LOOKBACK,
            Some(&lic),
        );
        assert!(matches!(
            parked_reason(&r, &f.clone),
            rp::ParkReason::EvidencePreservationFailed { .. }
        ));
        assert!(f.clone.exists());
    }

    /// The ref sweep IS part of the content-on-main proof (it asks the same D-1 question of every
    /// other tip), so the license replaces it too — otherwise the abandoned runs the license exists to
    /// clear, which are precisely the ones carrying rogue branches and stashes, would still park.
    /// Discriminates a license wired only into the HEAD verdict. The unlicensed twin of this fixture
    /// is `a_landed_head_with_an_unlanded_side_branch_is_parked`.
    #[test]
    fn a_dispositioned_clone_with_a_rogue_branch_is_deleted() {
        let f = fx();
        git(&f.clone, &["checkout", "-q", "-b", "agent-went-rogue"]);
        write(
            &f.clone.join("only-on-that-branch.rs"),
            "pub fn unique() {}\n",
        );
        git(&f.clone, &["add", "-A"]);
        git(&f.clone, &["commit", "-qm", "the agent committed itself"]);
        git(&f.clone, &["checkout", "-q", "main"]);

        let lic = license(&f, &[f.run_id.as_str()]);
        let items = scan(&f.implement);
        let env = FakeEnv::new();
        let r = run_licensed(
            &f,
            &items,
            &env,
            false,
            0,
            DEFAULT_CLONE_REAP_LOOKBACK,
            Some(&lic),
        );
        assert_eq!(item_for(&r, &f.clone).outcome, rp::ItemOutcome::Deleted);
        let gates = &item_for(&r, &f.clone).gates;
        assert!(
            gates
                .iter()
                .any(|g| g.contains("RECORDED, not gated") && g.contains("replaces the ref sweep")),
            "the gates do not disclose that the ref sweep was replaced: {gates:?}"
        );
        // ...and the loss is enumerable rather than merely permitted (R2).
        assert!(
            gates
                .iter()
                .any(|g| g.contains("agent-went-rogue") && g.contains("went with the clone")),
            "the gates do not name what the licensed deletion took: {gates:?}"
        );
    }

    /// A licensed run refused BEFORE the boundary — by D-2 at the scan root, or by an admission gate
    /// such as the linked-worktree custody rule — never reaches `reap_one`'s own disclosure. Without
    /// this it is the one licensed park with no explanation anywhere, and an operator is left reading
    /// "parked" against a run they authorized in writing. Discriminates a disclosure wired only into
    /// the boundary phase. Neither refusal is one the license could waive.
    #[test]
    fn a_licensed_run_parked_before_the_boundary_is_still_disclosed() {
        // (1) D-2 at the scan root, which refuses the whole invocation before admission runs.
        let f = fx();
        let lic = license(&f, &[f.run_id.as_str()]);
        let items = scan(&f.implement);
        let mut notes = Vec::new();
        let protected =
            rp::ProtectedRoots::resolve(&[f.clone.to_string_lossy().into_owned()], &mut notes)
                .unwrap();
        let r = reap_clones(
            ClonesRequest {
                scan_root: &f.implement,
                items: &items,
                protected: &protected,
                dry_run: false,
                runtimes_configured: 0,
                lookback: DEFAULT_CLONE_REAP_LOOKBACK,
                disposition: Some(&lic),
            },
            &FakeEnv::new(),
        );
        assert!(matches!(
            parked_reason(&r, &f.clone),
            rp::ParkReason::ProtectedRoot { .. }
        ));
        assert!(f.clone.join("src/lib.rs").exists());
        assert!(
            r.notes.iter().any(
                |n| n.contains("PARKED before the boundary gates ran") && n.contains(&f.run_id)
            ),
            "the D-2 park of a licensed run was not disclosed: {:?}",
            r.notes
        );

        // (2) an ADMISSION gate: the scan recorded a linked worktree, whose custody handle is the
        //     ADR-0025 sidecar lease and not this command's operation lock.
        let f = fx();
        let lic = license(&f, &[f.run_id.as_str()]);
        let items = vec![sr::ReportItem {
            path: sr::display_path(&f.clone),
            source: sr::ItemSource::ImplementPath,
            class: sr::PayloadClass::SourceCheckout,
            checkout_kind: Some(sr::CheckoutKind::LinkedWorktree),
            run_id: Some(f.run_id.clone()),
            measured: sr::Measured::default(),
            consumers: sr::LiveConsumers::default(),
            git: None,
            note: None,
        }];
        let env = FakeEnv::new();
        let r = run_licensed(
            &f,
            &items,
            &env,
            false,
            0,
            DEFAULT_CLONE_REAP_LOOKBACK,
            Some(&lic),
        );
        assert!(matches!(
            parked_reason(&r, &f.clone),
            rp::ParkReason::WorktreeCustody
        ));
        assert!(f.clone.exists());
        assert!(env.removed().is_empty());
        assert!(
            r.notes
                .iter()
                .any(|n| n.contains("PARKED before the boundary gates ran")),
            "the admission park of a licensed run was not disclosed: {:?}",
            r.notes
        );
    }

    /// A license is an ENUMERATION of runs, never a mode. Discriminates a `--disposition-list` that
    /// switches the gate off for the whole invocation.
    #[test]
    fn a_license_that_does_not_name_this_run_changes_nothing() {
        let f = fx();
        clone_commit(&f, "feat/x", "src/only-here.rs", "pub fn unique() {}\n");
        let lic = license(&f, &["impl-4242-someone-else"]);
        let items = scan(&f.implement);
        let env = FakeEnv::new();
        let r = run_licensed(
            &f,
            &items,
            &env,
            false,
            0,
            DEFAULT_CLONE_REAP_LOOKBACK,
            Some(&lic),
        );
        match parked_reason(&r, &f.clone) {
            rp::ParkReason::NotOnSourceMain { verdict, .. } => assert_eq!(verdict, "no"),
            other => panic!("an unnamed run was licensed: {other:?}"),
        }
        assert!(f.clone.join("src/only-here.rs").exists());
    }

    /// A list is written by hand against an earlier report, so by the time it is used a run may be
    /// gone. That is a NOTE — refusing the whole command would refuse to act on the runs it does
    /// name. Discriminates both a hard error and a silent drop.
    #[test]
    fn a_licensed_run_id_missing_from_the_scan_is_noted_not_refused() {
        let f = fx();
        clone_commit(&f, "feat/x", "src/only-here.rs", "pub fn unique() {}\n");
        let lic = license(&f, &[f.run_id.as_str(), "impl-777-vanished"]);
        let items = scan(&f.implement);
        let env = FakeEnv::new();
        let r = run_licensed(
            &f,
            &items,
            &env,
            false,
            0,
            DEFAULT_CLONE_REAP_LOOKBACK,
            Some(&lic),
        );
        assert!(
            r.notes
                .iter()
                .any(|n| n.contains("impl-777-vanished") && n.contains("not an error")),
            "the absent run id was not disclosed: {:?}",
            r.notes
        );
        // ...and the run that IS present was still acted on.
        assert_eq!(item_for(&r, &f.clone).outcome, rp::ItemOutcome::Deleted);
        assert!(
            r.notes
                .iter()
                .any(|n| n.contains("OWNER DISPOSITION LICENSE IN FORCE")),
            "the license itself is not disclosed on the report: {:?}",
            r.notes
        );
    }

    /// F1 x F2. A chained clone whose parent was reaped first has a BROKEN chain by construction —
    /// and that is exactly the population an owner disposition exists to clear. The license must not
    /// rest on a source repository, and the receipt must say honestly that none was found rather than
    /// naming one.
    #[test]
    fn a_dispositioned_clone_with_an_unresolvable_origin_is_still_deleted() {
        let f = fx();
        let (b, _) = clone_into_root(&f, &f.source, "bb");
        let (a, a_id) = clone_into_root(&f, &b, "cc");
        // The parent went first.
        std::fs::remove_dir_all(&b).unwrap();

        let lic = license(&f, &[a_id.as_str()]);
        let items = items_for(&scan(&f.implement), &a);
        let env = FakeEnv::new();
        let r = run_licensed(
            &f,
            &items,
            &env,
            false,
            0,
            DEFAULT_CLONE_REAP_LOOKBACK,
            Some(&lic),
        );
        assert_eq!(item_for(&r, &a).outcome, rp::ItemOutcome::Deleted);
        let v = fold_receipt_for(&f, &a_id);
        assert_eq!(v["containment"]["verdict"], VERDICT_OWNER_DISPOSITION);
        assert!(
            v["source_repo"].is_null(),
            "the receipt named a source repository that was never resolved: {}",
            v["source_repo"]
        );
        assert!(
            v["origin_chain"][0]
                .as_str()
                .unwrap()
                .contains("UNRESOLVED"),
            "the receipt does not record that the chain could not be walked: {}",
            v["origin_chain"]
        );
        // R7: the `None` arm of the removal guard, exercised through a REAL licensed removal rather
        // than as a unit — and saying which of its two shapes it proved.
        assert!(
            item_for(&r, &a)
                .gates
                .iter()
                .any(|g| g.contains("No source repository was resolved")
                    && g.contains("none for this removal to endanger")),
            "the guard did not disclose that it ran without a source: {:?}",
            item_for(&r, &a).gates
        );

        // The SAME clone without the license parks: the license is what changed the outcome.
        let f2 = fx();
        let (b2, _) = clone_into_root(&f2, &f2.source, "bb");
        let (a2, _) = clone_into_root(&f2, &b2, "cc");
        std::fs::remove_dir_all(&b2).unwrap();
        let items2 = items_for(&scan(&f2.implement), &a2);
        let r2 = run(&f2, &items2, &FakeEnv::new(), false);
        assert!(matches!(
            parked_reason(&r2, &a2),
            rp::ParkReason::OriginNotLocal { .. }
        ));
        assert!(a2.exists());
    }

    // =========================================================================================
    // S4b review repairs (R1–R7)
    // =========================================================================================

    /// R1. `git -C <dir>` ASCENDS: pointed at a plain directory it answers from whatever repository
    /// encloses it. So an `origin` naming a non-repository INSIDE the source's own tree made the
    /// enclosing repository answer the D-1 question — and the clone was deleted on a history nobody
    /// asked about, while the gate string claimed "a real git repository". The terminal root must
    /// pass the same repository check the in-root hops already did.
    ///
    /// Discriminates exactly that: the plain directory sits inside the source, whose main DOES contain
    /// the clone's HEAD, so an ascending probe answers `yes(head)` and deletes.
    #[test]
    fn an_origin_naming_a_plain_directory_inside_a_repo_parks() {
        let f = fx();
        let plain = f.source.join("vendor/plain");
        std::fs::create_dir_all(&plain).unwrap();
        git(
            &f.clone,
            &["remote", "set-url", "origin", plain.to_str().unwrap()],
        );

        let items = scan(&f.implement);
        let r = run(&f, &items, &FakeEnv::new(), false);
        match parked_reason(&r, &f.clone) {
            rp::ParkReason::OriginNotLocal { detail } => assert!(
                detail.contains("exists but is NOT a git repository"),
                "unexpected detail: {detail}"
            ),
            other => panic!("an enclosing repository answered for a plain directory: {other:?}"),
        }
        assert!(
            f.clone.exists(),
            "the clone was deleted on an enclosing repository's history"
        );
    }

    /// R1, the other half. [`looks_like_git_repo`] is a FILESYSTEM check: a directory holding an empty
    /// `.git/` passes it, and git then rejects that `.git` as invalid and CARRIES ON UP — so the
    /// enclosing repository answers after all, and the shape check alone does not close the hole.
    /// `GIT_CEILING_DIRECTORIES` does, and only when it names the PARENT (an entry equal to the start
    /// directory matches no ancestor and is a silent no-op).
    ///
    /// Discriminates exactly the ascent: the decoy sits inside the source repository, whose main
    /// contains the clone's HEAD, so an ascending probe answers `yes(head)` and deletes.
    #[test]
    fn an_origin_whose_git_is_invalid_never_falls_through_to_the_enclosing_repo() {
        let f = fx();
        let decoy = f.source.join("vendor/decoy");
        std::fs::create_dir_all(decoy.join(".git")).unwrap();
        assert!(
            looks_like_git_repo(&decoy),
            "the fixture must model a directory the SHAPE check accepts"
        );
        git(
            &f.clone,
            &["remote", "set-url", "origin", decoy.to_str().unwrap()],
        );

        let items = scan(&f.implement);
        let r = run(&f, &items, &FakeEnv::new(), false);
        match parked_reason(&r, &f.clone) {
            rp::ParkReason::NotOnSourceMain { verdict, .. } => assert_eq!(verdict, "unknown"),
            other => {
                panic!("an enclosing repository answered through an invalid `.git`: {other:?}")
            }
        }
        assert!(
            f.clone.exists(),
            "the clone was deleted on an enclosing repository's history"
        );
    }

    /// R2. The ref sweep is part of the content-on-main proof, so an owner disposition replaces it —
    /// the run is abandoned, tips and all. But the loss must be INFORMED: what the deletion took has
    /// to be enumerable from the receipt afterwards. An INVENTORY (never a gate) records every tip.
    /// Discriminates a licensed path that simply skips the refs and leaves no trace of them.
    #[test]
    fn a_licensed_deletion_records_an_inventory_of_every_ref_it_took() {
        let f = fx();
        git(&f.clone, &["checkout", "-q", "-b", "agent-went-rogue"]);
        write(
            &f.clone.join("only-on-that-branch.rs"),
            "pub fn unique() {}\n",
        );
        git(&f.clone, &["add", "-A"]);
        git(&f.clone, &["commit", "-qm", "the agent committed itself"]);
        let rogue = sr::git_str(&f.clone, &["rev-parse", "HEAD"]).unwrap();
        git(&f.clone, &["checkout", "-q", "main"]);
        let head = sr::git_str(&f.clone, &["rev-parse", "HEAD"]).unwrap();

        let lic = license(&f, &[f.run_id.as_str()]);
        let items = scan(&f.implement);
        let env = FakeEnv::new();
        let r = run_licensed(
            &f,
            &items,
            &env,
            false,
            0,
            DEFAULT_CLONE_REAP_LOOKBACK,
            Some(&lic),
        );
        assert_eq!(item_for(&r, &f.clone).outcome, rp::ItemOutcome::Deleted);

        let v = fold_receipt(&f);
        let inv = v["ref_inventory"]
            .as_array()
            .expect("no ref inventory on a licensed receipt");
        let rogue_row = inv
            .iter()
            .find(|r| r["refname"] == "refs/heads/agent-went-rogue")
            .unwrap_or_else(|| panic!("the inventory does not name the rogue branch: {inv:?}"));
        assert_eq!(
            rogue_row["oid"], rogue,
            "the inventory does not carry the oid"
        );
        assert_eq!(rogue_row["is_head"], false);
        assert_eq!(
            rogue_row["is_ancestor_of_head"], false,
            "a tip outside HEAD's history was recorded as inside it"
        );
        let head_row = inv
            .iter()
            .find(|r| r["refname"] == "refs/heads/main")
            .expect("the inventory omits the checked-out branch");
        assert_eq!(head_row["oid"], head);
        assert_eq!(head_row["is_head"], true);
        // ...and the operator sees it without opening the JSON.
        let gates = &item_for(&r, &f.clone).gates;
        assert!(
            gates
                .iter()
                .any(|g| g.contains("ref inventory") && g.contains("agent-went-rogue")),
            "the gate text does not name what the deletion took: {gates:?}"
        );
    }

    /// R4. A chain hop with no checkpoint is followed on its `remote.origin.url` alone, and that URL
    /// lives in a directory a `:rw` container could write. So the TERMINAL root is corroborated
    /// independently: it must contain something the clone itself claims about its own provenance —
    /// its checkpoint's base commit, or failing that its lineage ROOT commit, which a clone shares
    /// with the repository it was cloned from no matter how much work was added on top.
    ///
    /// Discriminates a resolver that accepts whatever the chain points at: here the foreign repo
    /// carries the clone's EXACT TREE (so containment answers `yes(tree)` and deletes) while sharing
    /// no history with it at all.
    #[test]
    fn a_terminal_root_that_shares_no_history_with_the_clone_parks() {
        let f = fx();
        let (b, _) = clone_into_root(&f, &f.source, "bb");
        let (a, _) = clone_into_root(&f, &b, "cc");

        // A foreign repository with byte-identical content and an unrelated history.
        let foreign = f.root.join("foreign");
        std::fs::create_dir_all(&foreign).unwrap();
        git(&foreign, &["init", "--initial-branch=main", "-q"]);
        write(&foreign.join("README.md"), "one\n");
        write(&foreign.join("src/lib.rs"), "pub fn a() {}\n");
        write(&foreign.join(".gitignore"), "/target\n");
        git(&foreign, &["add", "-A"]);
        git(
            &foreign,
            &["commit", "-qm", "same content, unrelated history"],
        );
        assert_eq!(
            sr::git_str(&a, &["rev-parse", "HEAD^{tree}"]).unwrap(),
            sr::git_str(&foreign, &["rev-parse", "HEAD^{tree}"]).unwrap(),
            "the fixture must model a foreign repo whose TREE matches, or it proves nothing"
        );
        git(
            &b,
            &["remote", "set-url", "origin", foreign.to_str().unwrap()],
        );

        let items = items_for(&scan(&f.implement), &a);
        let r = run(&f, &items, &FakeEnv::new(), false);
        match parked_reason(&r, &a) {
            rp::ParkReason::NotOnSourceMain { verdict, detail } => {
                assert_eq!(verdict, "unknown");
                assert!(
                    detail.contains("corroborat"),
                    "the refusal does not say the root was uncorroborated: {detail}"
                );
            }
            other => panic!("an uncorroborated terminal root answered the D-1 question: {other:?}"),
        }
        assert!(
            a.exists(),
            "a clone was deleted against a repository it never came from"
        );
    }

    /// R5. `source_repo: None` drops the removal guard's "the source is neither the clone nor inside
    /// it" clause. That is only honest when there IS no source directory — the parent-already-reaped
    /// case. When the clone's own `origin` names a directory that EXISTS, the guard must still get it,
    /// license or no license. Discriminates a licensed path that answers `None` for every resolver
    /// failure: here `origin` names a repository INSIDE the clone, which the guard exists to refuse.
    #[test]
    fn a_licensed_clone_whose_existing_origin_is_inside_itself_is_refused_by_the_guard() {
        let f = fx();
        // Under `.git/`, so `git status` says nothing about it and the clone reaches the removal
        // guard on a CLEAN tree — the gate under test here is the guard's source clause, not the
        // dirty-state gate.
        let inner = f.clone.join(".git/vendored-source");
        std::fs::create_dir_all(&inner).unwrap();
        git(&inner, &["init", "--initial-branch=main", "-q"]);
        git(
            &f.clone,
            &["remote", "set-url", "origin", inner.to_str().unwrap()],
        );

        let lic = license(&f, &[f.run_id.as_str()]);
        let items = items_for(&scan(&f.implement), &f.clone);
        let env = FakeEnv::new();
        let r = run_licensed(
            &f,
            &items,
            &env,
            false,
            0,
            DEFAULT_CLONE_REAP_LOOKBACK,
            Some(&lic),
        );
        match parked_reason(&r, &f.clone) {
            rp::ParkReason::RemovalGuardRefused { detail } => assert!(
                detail.contains("INSIDE the clone"),
                "unexpected detail: {detail}"
            ),
            other => panic!("the guard's source clause was dropped under a license: {other:?}"),
        }
        assert!(f.clone.exists());
        assert!(env.removed().is_empty());
    }

    /// R6. THE stranding defect. Rows are processed in scan order, so a parent clone can be removed
    /// before the child whose `origin` names it — and the child then resolves nothing, PERMANENTLY:
    /// no later invocation rewrites a dead `origin`. Resolving every candidate's chain during
    /// ADMISSION, before any deletion, is what makes deletion order irrelevant; the terminal root is
    /// outside the scan root by construction, so it survives every sibling removal.
    ///
    /// Discriminates order-dependent resolution by running the SAME production shape both ways.
    #[test]
    fn a_chained_pair_is_reaped_in_one_invocation_whatever_the_order() {
        for reversed in [false, true] {
            let f = fx();
            // Both clones' HEADs are already on the source's main, so both are deletable; A's only
            // route to the source runs THROUGH B.
            let (b, b_id) = clone_into_root(&f, &f.source, "bb");
            let (a, a_id) = clone_into_root(&f, &b, "cc");
            write_checkpoint(&b, &b_id, &f.source);
            write_checkpoint(&a, &a_id, &b);

            let mut items = scan(&f.implement);
            if reversed {
                items.reverse();
            }
            let env = FakeEnv::new();
            let r = run(&f, &items, &env, false);
            assert_eq!(
                item_for(&r, &a).outcome,
                rp::ItemOutcome::Deleted,
                "reversed={reversed}: the chained child was stranded by its parent's removal: {:?}",
                item_for(&r, &a).outcome
            );
            assert_eq!(
                item_for(&r, &b).outcome,
                rp::ItemOutcome::Deleted,
                "reversed={reversed}: the parent was not reaped"
            );
            assert_eq!(
                fold_receipt_for(&f, &a_id)["source_repo"],
                sr::display_path(&f.source),
                "reversed={reversed}: the child's receipt does not name the true source"
            );
            assert!(f.source.join("src/lib.rs").exists());
        }
    }

    /// R7. The Q1 boundary: a license is per-run, and the runs it does NOT name must be judged
    /// exactly as they would be without it — in the SAME invocation, where a mode-shaped
    /// implementation would leak across.
    #[test]
    fn a_listed_and_an_unlisted_clone_are_judged_differently_in_one_invocation() {
        let f = fx();
        let (listed, listed_id) = clone_into_root(&f, &f.source, "bb");
        let (unlisted, _) = clone_into_root(&f, &f.source, "cc");
        for (c, name) in [(&listed, "listed.rs"), (&unlisted, "unlisted.rs")] {
            git(c, &["checkout", "-q", "-b", "feat/x"]);
            write(&c.join(name), "pub fn unique() {}\n");
            git(c, &["add", "-A"]);
            git(c, &["commit", "-qm", "never landed"]);
        }

        let lic = license(&f, &[listed_id.as_str()]);
        let items = scan(&f.implement);
        let env = FakeEnv::new();
        let r = run_licensed(
            &f,
            &items,
            &env,
            false,
            0,
            DEFAULT_CLONE_REAP_LOOKBACK,
            Some(&lic),
        );
        assert_eq!(item_for(&r, &listed).outcome, rp::ItemOutcome::Deleted);
        match parked_reason(&r, &unlisted) {
            rp::ParkReason::NotOnSourceMain { verdict, .. } => assert_eq!(verdict, "no"),
            other => panic!("the license leaked onto a run it does not name: {other:?}"),
        }
        assert!(
            unlisted.join("unlisted.rs").exists(),
            "unlisted work was destroyed"
        );
        assert_eq!(
            fold_receipt_for(&f, &listed_id)["containment"]["verdict"],
            VERDICT_OWNER_DISPOSITION
        );
    }

    /// R7. `starts_with(scan_root)` decides whether a hop is a sibling quarantine clone or the true
    /// source, so it must be applied to the RESOLVED path. Discriminates a lexical test: a `../`
    /// relative URL and a symlink pointing into the root both name in-root siblings without looking
    /// like it.
    #[test]
    fn the_scan_root_boundary_is_decided_on_the_resolved_path() {
        // (1) a relative `../<sibling>` URL.
        let f = fx();
        let (b, b_id) = clone_into_root(&f, &f.source, "bb");
        write_checkpoint(&b, &b_id, &f.source);
        let (a, a_id) = clone_into_root(&f, &b, "cc");
        write_checkpoint(&a, &a_id, &b);
        git(&a, &["remote", "set-url", "origin", &format!("../{b_id}")]);
        let items = items_for(&scan(&f.implement), &a);
        let r = run(&f, &items, &FakeEnv::new(), false);
        assert_eq!(
            item_for(&r, &a).outcome,
            rp::ItemOutcome::Deleted,
            "a relative in-root origin was not followed: {:?}",
            item_for(&r, &a).outcome
        );
        let chain = fold_receipt_for(&f, &a_id)["origin_chain"]
            .as_array()
            .unwrap()
            .len();
        assert_eq!(
            chain, 2,
            "the relative URL did not resolve as an in-root hop"
        );

        // (2) a path whose PARENT is a symlink outside the root and whose leaf is a real directory
        //     inside it. Lexically it is not under the scan root; canonically it is a sibling clone,
        //     and reading it as the true source would ask the D-1 question of a quarantine directory
        //     again — the very defect F1 exists to fix, re-entered through a link.
        //
        //     (A symlink at the LEAF is refused outright by `real_dir`'s no-follow check, asserted
        //     below, so this is the only shape that reaches the boundary test at all.)
        let f = fx();
        let (b, b_id) = clone_into_root(&f, &f.source, "bb");
        write_checkpoint(&b, &b_id, &f.source);
        let (a, a_id) = clone_into_root(&f, &b, "cc");
        write_checkpoint(&a, &a_id, &b);
        let alias = f.root.join("alias");
        std::os::unix::fs::symlink(&f.implement, &alias).unwrap();
        git(
            &a,
            &[
                "remote",
                "set-url",
                "origin",
                alias.join(&b_id).to_str().unwrap(),
            ],
        );
        let items = items_for(&scan(&f.implement), &a);
        let r = run(&f, &items, &FakeEnv::new(), false);
        assert_eq!(
            item_for(&r, &a).outcome,
            rp::ItemOutcome::Deleted,
            "a symlinked in-root origin was not resolved: {:?}",
            item_for(&r, &a).outcome
        );
        let v = fold_receipt_for(&f, &a_id);
        assert_eq!(
            v["origin_chain"].as_array().unwrap().len(),
            2,
            "the symlinked path was taken for the true source: {}",
            v["origin_chain"]
        );
        assert_eq!(v["source_repo"], sr::display_path(&f.source));

        // (3) and a symlink at the LEAF is never followed at all — no-follow, not resolved.
        let f = fx();
        let leaf_link = f.root.join("leaf-link");
        std::os::unix::fs::symlink(&f.source, &leaf_link).unwrap();
        git(
            &f.clone,
            &["remote", "set-url", "origin", leaf_link.to_str().unwrap()],
        );
        let items = items_for(&scan(&f.implement), &f.clone);
        let r = run(&f, &items, &FakeEnv::new(), false);
        assert!(
            matches!(
                parked_reason(&r, &f.clone),
                rp::ParkReason::OriginNotLocal { .. }
            ),
            "a symlinked origin leaf was followed"
        );
        assert!(f.clone.exists());
    }
}
