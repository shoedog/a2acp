//! Clone adjudication and deletion. Only exact landing trailers on source main license automatic
//! merged cleanup. Owner remove decisions waive Git custody, while every live-use, operation-lock,
//! protected-root, path-identity and container safeguard remains mandatory. The configured archive
//! owns evidence retention; fold receipts record deletion intent and outcome independently.

use crate::storage_reap as rp;
use crate::storage_report as sr;
use bridge_core::fs_custody;
use std::collections::{BTreeMap, BTreeSet};
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
#[cfg(test)]
pub const DISPOSABLE_IGNORED: [&str; 3] = ["target", "node_modules", ".venv"];

/// How many offending `git status` entries a refusal quotes before it stops. Bounded so a clone with a
/// hundred thousand untracked files cannot turn one park reason into a memory-sized string.
#[cfg(test)]
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
#[cfg(test)]
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
#[cfg(test)]
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
    #[serde(default)]
    pub run_ids: Vec<String>,
    #[serde(default)]
    pub keep_run_ids: Vec<String>,
}

/// A validated license plus the identity of the file it came from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DispositionList {
    pub authorized_by: String,
    pub reason: String,
    pub run_ids: BTreeSet<String>,
    pub keep_run_ids: BTreeSet<String>,
    /// The canonical path of the list file.
    pub list_file: String,
    /// sha256 of the file's exact bytes.
    pub list_sha256: String,
}

impl DispositionList {
    /// Does this license name `run_id`? Exact match on the run directory name — never a prefix, never
    /// a pattern. A license is an enumeration of runs, not a rule that could match one nobody meant.
    pub fn licenses(&self, run_id: &str) -> bool {
        self.run_ids.contains(run_id) && !self.keep_run_ids.contains(run_id)
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
                 content-on-main gate did not pass for it. Every required live-use and path safeguard passed.",
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
    if doc.run_ids.is_empty() && doc.keep_run_ids.is_empty() {
        return Err(
            "`run_ids` is empty — a list that authorizes no run is refused rather than run as a \
             no-op (it is far more likely truncated than intentional)"
                .into(),
        );
    }
    if doc.run_ids.iter().any(|id| doc.keep_run_ids.contains(id)) {
        return Err("contradictory keep and remove decisions for the same run".into());
    }
    for id in doc.run_ids.iter().chain(&doc.keep_run_ids) {
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
        keep_run_ids: doc.keep_run_ids.into_iter().collect(),
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
/// removing the parent first would strand the child. Resolution precedes dependency ordering, and
/// children must finish deletion before their sources can be removed: immediate-source custody is
/// still checked at the boundary, separately from terminal-root landing provenance.
#[derive(Clone, Debug)]
pub struct OriginResolution {
    /// The walk's outcome, cached from admission.
    pub chain: Result<OriginChain, rp::ParkReason>,
    /// R5. The clone's OWN `origin` as an existing local directory, whatever became of the rest of
    /// the walk. This is what [`removal_guard`] needs for its "the source is neither the clone nor
    /// inside it" clause, and it is `None` only when that directory is genuinely absent — the
    /// parent-already-reaped case, where there is no source for a removal to endanger.
    pub immediate: Option<PathBuf>,
    /// Admission identity of the immediate source, distinct from the terminal root.
    pub immediate_identity: Option<(u64, u64)>,
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
        .and_then(|c| fs_custody::directory_dev_ino(&c.root).ok());
    OriginResolution {
        immediate_identity: immediate
            .as_deref()
            .and_then(|p| fs_custody::directory_dev_ino(p).ok()),
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

/// Read the configuration recorded by this run, never the reaper invocation's config.
fn recorded_config(
    clone: &Path,
    run_id: &str,
) -> Result<
    (
        bridge_controller::implement_resume::ImplementCheckpoint,
        crate::config::RegistryConfig,
    ),
    rp::ParkReason,
> {
    let unknown = |detail| rp::ParkReason::GitStateUnknown { detail };
    let ck = bridge_controller::implement_resume::load_checkpoint(clone).map_err(&unknown)?;
    if ck.schema_version != bridge_controller::implement_resume::SCHEMA_VERSION
        || !ck.source_repo.is_absolute()
        || !ck.clone_path.is_absolute()
        || ck.task_id != run_id
        || ck.resume_id != run_id
        || ck.clone_path.canonicalize().ok().as_deref() != Some(clone)
        || !ck.config_path.is_absolute()
    {
        return Err(unknown(
            "checkpoint identity or absolute recorded configuration path invalid".into(),
        ));
    }
    let raw = std::fs::read_to_string(&ck.config_path).map_err(|e| {
        unknown(format!(
            "recorded configuration {} unavailable: {e}",
            ck.config_path.display()
        ))
    })?;
    let cfg = crate::config::RegistryConfig::parse(&raw)
        .map_err(|e| unknown(format!("recorded configuration invalid: {e}")))?;
    cfg.custody.validate().map_err(&unknown)?;
    Ok((ck, cfg))
}

/// Derive adjudication and custody from one run under the caller's held operation lock.
#[allow(dead_code)] // retained for callers deriving a single clone outside the reaper
pub fn derive_clone_facts(
    clone: &Path,
    run_id: &str,
    resolution: &OriginResolution,
    lookback: u32,
    disposition: Option<&DispositionList>,
) -> Result<CloneFacts, rp::ParkReason> {
    let (ck, cfg) = recorded_config(clone, run_id)?;
    let _policy = recorded_policy(clone, &cfg)?;
    derive_with_config(clone, run_id, resolution, lookback, disposition, &ck, &cfg)
}

fn recorded_policy(
    clone: &Path,
    cfg: &crate::config::RegistryConfig,
) -> Result<bridge_controller::clone_config::ClonePolicyScope, rp::ParkReason> {
    let policy = cfg
        .implement
        .as_ref()
        .map(|i| i.to_config())
        .transpose()
        .map_err(|e| rp::ParkReason::GitStateUnknown {
            detail: format!("recorded implement policy invalid: {e}"),
        })?
        .unwrap_or_default()
        .clone_config;
    bridge_controller::clone_config::ClonePolicyScope::install(clone, policy)
        .map_err(|detail| rp::ParkReason::GitStateUnknown { detail })
}

#[allow(clippy::too_many_arguments)]
fn derive_with_config(
    clone: &Path,
    run_id: &str,
    resolution: &OriginResolution,
    lookback: u32,
    disposition: Option<&DispositionList>,
    ck: &bridge_controller::implement_resume::ImplementCheckpoint,
    cfg: &crate::config::RegistryConfig,
) -> Result<CloneFacts, rp::ParkReason> {
    let before = sr::shape_fingerprint(clone);
    if !sr::real_dir(&clone.join(".git")) {
        return Err(rp::ParkReason::NotAStandaloneClone {
            detail: "real standalone .git directory required".into(),
        });
    }
    let licensed = disposition.filter(|d| d.licenses(run_id));
    if disposition.is_some_and(|d| d.keep_run_ids.contains(run_id)) {
        return Err(rp::ParkReason::NotOnSourceMain {
            verdict: "explicit-keep".into(),
            detail: "owner chose keep; clone and remote refs preserved".into(),
        });
    }
    let mut gates = vec![format!(
        "recorded configuration: {}",
        ck.config_path.display()
    )];
    // On discard, Git facts are only a best-effort inventory. Never execute unadmitted clone config.
    let admitted = bridge_controller::clone_config::check_clone_with_policy(clone);
    if licensed.is_none() {
        admitted
            .as_ref()
            .map_err(|e| rp::ParkReason::GitStateUnknown {
                detail: format!("clone configuration not admitted: {e}"),
            })?;
    }
    let actual_head = if admitted.is_ok() {
        sr::git_str(clone, &["rev-parse", "HEAD"]).ok()
    } else {
        None
    };
    if licensed.is_none() && actual_head.is_none() {
        return Err(rp::ParkReason::UnbornHead {
            detail: Some("HEAD unavailable".into()),
        });
    }
    let head = actual_head
        .or_else(|| ck.current_commit.clone())
        .unwrap_or_default();
    let tree = if admitted.is_ok() {
        sr::git_str(clone, &["rev-parse", &format!("{head}^{{tree}}")]).ok()
    } else {
        None
    };
    let immediate = match ck.source_repo.canonicalize() {
        Ok(source) => Some(source),
        Err(e) if licensed.is_some() && e.kind() == std::io::ErrorKind::NotFound => {
            gates.push("recorded source gone: remote cleanup unavailable".into());
            None
        }
        Err(e) => {
            return Err(rp::ParkReason::GitStateUnknown {
                detail: format!("recorded source unavailable: {e}"),
            })
        }
    };
    if let Some(source) = &immediate {
        // Source containment remains mandatory even on the authorized discard path.
        if source == clone || source.starts_with(clone) {
            return Err(rp::ParkReason::RemovalGuardRefused {
                detail: "recorded source is the clone or inside it".into(),
            });
        }
        if resolution.first_hop.as_ref().is_some_and(|p| p != source)
            || resolution.immediate_identity.is_some_and(|expected| {
                fs_custody::directory_dev_ino(source).ok() != Some(expected)
            })
        {
            return Err(rp::ParkReason::SourceRootIdentityChanged {
                detail: "recorded immediate source identity changed".into(),
            });
        }
    }
    let containment;
    let main_oid;
    let source;
    if let Some(d) = licensed {
        source = resolution
            .chain
            .as_ref()
            .ok()
            .map(|c| c.root.clone())
            .or(immediate);
        containment = d.evidence(run_id);
        main_oid = None;
        gates.push("owner remove: merged classification and Git custody checks waived; live-use, locks, root, path and container safeguards retained".into());
    } else {
        let chain = resolution.chain.as_ref().map_err(Clone::clone)?;
        let terminal = &chain.root;
        if resolution.first_hop.as_ref() != immediate.as_ref()
            || immediate_origin(clone).as_ref() != immediate.as_ref()
        {
            return Err(rp::ParkReason::SourceRootIdentityChanged {
                detail: "origin disagrees with recorded immediate source".into(),
            });
        }
        gates.push(corroborate_terminal_root(clone, &head, terminal, chain)?);
        if resolution.root_identity.is_none()
            || fs_custody::directory_dev_ino(terminal).ok() != resolution.root_identity
        {
            return Err(rp::ParkReason::SourceRootIdentityChanged {
                detail: "terminal source identity changed".into(),
            });
        }
        // Landing provenance belongs to the terminal repository. Transport and archive
        // custody below still use the checkpoint's immediate source.
        let mut landing_ck = ck.clone();
        landing_ck.source_repo = terminal.clone();
        source = Some(terminal.clone());
        let evidence =
            bridge_controller::merge::classify_landing(clone, &landing_ck, &head, lookback);
        let blockers = bridge_controller::clone_deletion::failures_for_phase(
            clone,
            ck,
            &cfg.custody,
            &cfg.evidence,
            &format!("{:?}", ck.phase).to_ascii_lowercase(),
        );
        let detail = format!("run {run_id}; path {}; source {}; confidence {}; matched {:?}; {}; deletion blockers: {}", clone.display(), terminal.display(), evidence.confidence, evidence.matched_commit, evidence.reason, if blockers.is_empty() { "none".into() } else { blockers.join(" | ") });
        if evidence.confirmed() && !blockers.is_empty() {
            let first = &blockers[0];
            return Err(if first.starts_with("check 2:") {
                rp::ParkReason::GitStateNotClean { detail }
            } else if first.starts_with("check 4:") {
                rp::ParkReason::RefsNotContained { detail }
            } else if first.contains("gitlink") || first.contains("modules") {
                rp::ParkReason::InitializedSubmodule { detail }
            } else if first.contains("index") {
                rp::ParkReason::IndexFlagsHideState { detail }
            } else {
                rp::ParkReason::GitStateUnknown { detail }
            });
        }
        if !evidence.confirmed() {
            return Err(rp::ParkReason::NotOnSourceMain {
                verdict: evidence.confidence.into(),
                detail,
            });
        }
        gates.push(detail.clone());
        containment = ContainmentEvidence::proved(
            evidence.confidence.into(),
            Some("main".into()),
            evidence.matched_commit,
            detail,
        );
        main_oid = evidence.main_oid;
    }
    let (inventory, note) = if admitted.is_ok() {
        ref_inventory(clone, &head)
    } else {
        (
            vec![],
            Some("unadmitted configuration; Git inventory skipped".into()),
        )
    };
    gates.push(ref_inventory_summary(&inventory, note.as_deref()));
    if sr::shape_fingerprint(clone) != before {
        return Err(rp::ParkReason::GitIdentityChanged {
            detail: "clone .git identity changed during probes".into(),
        });
    }
    Ok(CloneFacts {
        branch: Some(ck.branch.clone()),
        head,
        tree,
        source_repo: source,
        origin_chain: resolution
            .chain
            .as_ref()
            .map(|c| c.hops.clone())
            .unwrap_or_default(),
        main_ref_full: main_oid.as_ref().map(|_| "refs/heads/main".into()),
        main_oid,
        containment,
        ref_inventory: inventory,
        gates,
    })
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
#[cfg(test)]
pub use bridge_controller::clone_deletion::index_flags_disposition;

/// PURE. `git submodule status` marks each submodule: `-` = NOT initialized (no object store here),
/// anything else (` `, `+`, `U`) = initialized. Fail-closed: an unparseable line is treated as
/// initialized, because "I could not tell" is not "there is nothing there".
#[cfg(test)]
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

// ---------------------------------------------------------------------------------------------
// Evidence preservation: the structural preflight
// ---------------------------------------------------------------------------------------------

/// The Evidence a run carries OUTSIDE its worktree, and therefore outside every git gate above:
/// the sidecar directory plus the two `.git`-scoped files the implement loop writes.
/// `.git/A2A_TASK.md` is the out-of-band task the fix loop re-reads (tweak.rs) and
/// `.git/A2A_COMMIT_MSG` is the agent-written hand-off message (implement.rs).
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
    /// Invocation scan count, disclosed as supplemental evidence. Container absence must
    /// independently be established from each run's recorded runtimes.
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
/// is something an owner discard can waive: they establish operational and path authority.
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
         Owner remove waives Git custody; this admission/live-use/path refusal remains mandatory.",
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
            "OWNER DISPOSITION LICENSE IN FORCE: explicit REMOVE decisions: {} (sha256 {}), authorized by {:?} — {:?}. {} named run(s) waive merged classification and Git-state custody. Live-use, operation lock, protected roots, path identity and container safeguards remain mandatory. Receipts record `{VERDICT_OWNER_DISPOSITION}`.",
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

    // Resolve before any removal; custody still needs the immediate source to survive until its
    // children have been deleted. Include refused candidates in the dependency graph so a parked
    // child (including one refused at admission) never loses its source in this invocation.
    for c in candidates.iter_mut() {
        c.origin = resolve_origin(&c.path, &root);
    }
    let mut dependents: BTreeMap<PathBuf, BTreeSet<PathBuf>> = BTreeMap::new();
    for it in req.items {
        let child = PathBuf::from(&it.path);
        if !it.source.is_filesystem_path() || child.parent() != Some(root.as_path()) {
            continue;
        }
        let recorded_source = bridge_controller::implement_resume::load_checkpoint(&child)
            .ok()
            .map(|ck| ck.source_repo);
        for source in recorded_source.into_iter().chain(immediate_origin(&child)) {
            let source = source.canonicalize().unwrap_or(source);
            dependents.entry(source).or_default().insert(child.clone());
        }
    }
    let mut completed = BTreeSet::new();

    // `(clone path, was the removal clean)` for every clone this command actually tried to remove.
    // A PARTIAL or UNKNOWN outcome belongs here too: its descendants are exactly the rows whose
    // recorded state is now least trustworthy.
    let mut attempted: Vec<(PathBuf, bool)> = Vec::new();
    while !candidates.is_empty() {
        // Leaves first, independent of scan order. Cycles have no leaf and are retained below.
        let next = candidates
            .iter()
            .position(|c| {
                dependents.get(&c.path).is_none_or(|children| {
                    !candidates
                        .iter()
                        .any(|other| children.contains(&other.path))
                })
            })
            .unwrap_or(0);
        let c = candidates.remove(next);
        let path = c.path.clone();
        let blocker = dependents
            .get(&path)
            .and_then(|children| children.iter().find(|child| !completed.contains(*child)));
        let item = if let Some(child) = blocker {
            let reason = rp::ParkReason::LiveConsumer {
                kind: "dependent clone".into(),
                detail: format!("{} still depends on this source clone", child.display()),
            };
            disclose_early_park(&mut report.notes, req.disposition, c.item, &reason);
            rp::park(c.item, reason)
        } else {
            reap_one(c, &pin, &req, env, &mut report)
        };
        match item.outcome {
            rp::ItemOutcome::Deleted => {
                completed.insert(path.clone());
                attempted.push((path, true));
            }
            rp::ItemOutcome::Planned if req.dry_run => {
                completed.insert(path);
            }
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
            immediate_identity: None,
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

/// Query the run's recorded runtimes, including stopped containers and every resume generation.
/// The invocation scan is supplemental and can never supply this proof.
fn recorded_containers_absent(
    clone: &Path,
    cfg: &crate::config::RegistryConfig,
) -> Result<String, rp::ParkReason> {
    let runtimes: BTreeSet<String> = cfg
        .agents
        .iter()
        .filter_map(|a| a.sandbox.as_ref())
        .map(|s| {
            s.runtime
                .clone()
                .unwrap_or_else(|| bridge_core::domain::DEFAULT_RUNTIME.into())
        })
        .chain(cfg.verify.as_ref().map(|v| {
            v.runtime
                .clone()
                .unwrap_or_else(|| bridge_core::domain::DEFAULT_RUNTIME.into())
        }))
        .collect();
    if runtimes.is_empty() {
        return Err(rp::ParkReason::ContainerAxisUnanswered);
    }
    let path = clone
        .to_str()
        .ok_or(rp::ParkReason::ContainerAxisUnanswered)?;
    // Use the backend's own display-label encoding. Task IDs are not process instance IDs and
    // cannot substitute for a missing container-runs record.
    let labels = bridge_core::run_identity::RunHandle {
        instance_id: String::new(),
        host: String::new(),
        lease: String::new(),
        start: String::new(),
    }
    .labels("rw", "warm", "", "", Some(path), Some(path));
    let repo = labels.repo.ok_or(rp::ParkReason::ContainerAxisUnanswered)?;
    let cwd = labels.cwd.ok_or(rp::ParkReason::ContainerAxisUnanswered)?;
    let path_is_representable = repo == path && cwd == path;
    let mut filters = vec![
        format!("label=a2a.cwd={cwd}"),
        format!("label=a2a.repo={repo}"),
    ];
    filters.extend(
        bridge_controller::implement_resume::container_run_ids(clone)
            .map_err(|detail| rp::ParkReason::GitStateUnknown { detail })?
            .into_iter()
            .map(|id| format!("label=a2a.run={id}")),
    );
    let count = runtimes.len();
    // reap_clones is synchronous and may itself be called on an async runtime. Join a
    // dedicated bounded query runtime rather than nesting block_on on the caller's thread.
    let result = std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|_| rp::ParkReason::ContainerAxisUnanswered)?;
        runtime.block_on(async {
            let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
            for runtime in runtimes {
                for filter in &filters {
                    let out = tokio::time::timeout_at(
                        deadline,
                        tokio::process::Command::new(&runtime)
                            .kill_on_drop(true)
                            .args([
                                "ps",
                                "-aq",
                                "--filter",
                                "label=a2a.managed=1",
                                "--filter",
                                filter,
                            ])
                            .output(),
                    )
                    .await;
                    match out {
                        Ok(Ok(out))
                            if out.status.success()
                                && out.stdout.iter().all(u8::is_ascii_whitespace) => {}
                        Ok(Ok(out)) if out.status.success() => {
                            return Err(rp::ParkReason::LiveConsumer {
                                kind: "recorded runtime container".into(),
                                detail: format!("{runtime}: container still exists for {filter}"),
                            })
                        }
                        _ => return Err(rp::ParkReason::ContainerAxisUnanswered),
                    }
                }
            }
            Ok(())
        })
    })
    .join()
    .map_err(|_| rp::ParkReason::ContainerAxisUnanswered)?;
    result?;
    // These are display labels, not mount identities. A lossy path match can establish a holder,
    // but an empty result cannot establish absence (including for an owner-authorized discard).
    if !path_is_representable {
        return Err(rp::ParkReason::ContainerAxisUnanswered);
    }
    Ok(format!("container axis: {count} recorded runtime(s) affirm no container exists for this clone or run"))
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
                     owner remove waives Git custody only; every live-use and path safeguard still \
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
    if let Err(detail) = fs_custody::pinned_root_unchanged(pin) {
        parked_locked!(rp::ParkReason::ScanRootIdentityChanged { detail })
    }
    gates.push(format!(
        "scan root: descriptor-pinned and re-verified ({})",
        fs_custody::root_identity_label(pin)
    ));

    // GATE 4 — path identity: a real directory, not a symlink, still resolving to itself.
    let identity = match fs_custody::verify_payload_directory_identity(&path) {
        Ok(id) => id,
        Err(refusal) => parked_locked!(rp::payload_identity_park_reason(refusal)),
    };
    let identity_label = match (identity.dev, identity.ino) {
        (Some(dev), Some(ino)) => format!("dev {dev} / ino {ino}"),
        _ => "dev/ino unavailable".to_string(),
    };
    gates.push(format!(
        "path identity: real directory, no symlink, {identity_label}"
    ));

    if !sr::real_dir(&path.join(".git")) {
        parked_locked!(rp::ParkReason::NotAStandaloneClone {
            detail: "real standalone .git directory required".into()
        });
    }
    let (recorded, cfg) = match recorded_config(&path, &run_id) {
        Ok(value) => value,
        Err(reason) => parked_locked!(reason),
    };

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
    if item.consumers.container_mount == sr::HolderState::Unknown {
        parked_locked!(rp::ParkReason::ContainerAxisUnanswered);
    }
    match recorded_containers_absent(&path, &cfg) {
        Ok(detail) => gates.push(detail),
        Err(reason) => parked_locked!(reason),
    }
    gates.push(format!(
        "invocation scan: {} runtime(s); positive holders retained as additional evidence",
        req.runtimes_configured
    ));

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

    // GATE 7 — recorded policy and shared Git custody.
    let _policy = match recorded_policy(&path, &cfg) {
        Ok(policy) => policy,
        Err(reason) => parked_locked!(reason),
    };
    let facts = match derive_with_config(
        &path,
        &run_id,
        &origin,
        req.lookback,
        req.disposition,
        &recorded,
        &cfg,
    ) {
        Ok(f) => f,
        Err(reason) => parked_locked!(reason),
    };
    gates.extend(facts.gates.iter().cloned());
    gates.push(format!("origin at admission: {:?}", origin.first_hop));

    // GATE 8 — the exact-mechanism removal guard (the `merge::reap_clone` shape).
    match removal_guard(&path, &root, &run_id, facts.source_repo.as_deref()) {
        Ok(evidence) => gates.push(evidence),
        Err(detail) => parked_locked!(rp::ParkReason::RemovalGuardRefused { detail }),
    }

    if let Some(source) = origin.immediate.as_deref() {
        if let Err(detail) = removal_guard(&path, &root, &run_id, Some(source)) {
            parked_locked!(rp::ParkReason::RemovalGuardRefused { detail });
        }
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
        scan_root_identity: fs_custody::root_identity_label(pin),
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

    // The configured archive is the evidence copy. Keep receipts, not a second payload archive.
    let receipts = receipts_dir(&root);
    if std::fs::symlink_metadata(&receipts).is_ok_and(|m| !m.is_dir() || m.file_type().is_symlink())
    {
        parked_locked!(rp::ParkReason::EvidencePreservationFailed {
            detail: "receipt namespace is not a real directory".into()
        });
    }
    // Prepare transport while the clone exists. No remote deletion occurs before exact local success.
    let cleanup = if licensed.is_some() || cfg.custody.delete_remote_on_merge {
        Some(if recorded.source_repo.canonicalize().is_err() {
            Err("recorded source unavailable; remote cleanup unavailable".into())
        } else {
            bridge_controller::custody_push::RemoteCleanup::prepare(&path, &cfg.custody, &recorded)
        })
    } else {
        None
    };
    if req.dry_run {
        gates.push("configured archive checked by shared gate; receipt publication and removal not exercised by dry run".into());
        drop(guard);
        return base_item(rp::ItemOutcome::Planned, gates);
    }
    if licensed.is_some() {
        let phase = format!("{:?}", recorded.phase).to_ascii_lowercase();
        match bridge_controller::evidence::write_run_archive(
            &path,
            &recorded,
            &phase,
            &cfg.evidence,
        ) {
            Ok(archive) => receipt.evidence_preserved_at = Some(archive.display().to_string()),
            Err(e) => gates.push(format!(
                "owner discard: archive unavailable ({e}); evidence custody waived"
            )),
        }
    }
    let archive_location = receipt.evidence_preserved_at.clone().or_else(|| {
        let root = cfg.evidence.path.clone().or_else(|| {
            bridge_core::platform_state::platform_state_dir()
                .ok()
                .map(|p| p.join("evidence"))
        })?;
        Some(
            root.join(bridge_controller::evidence::repo_key(&recorded.source_repo).ok()?)
                .join(format!("{run_id}.tar.gz"))
                .display()
                .to_string(),
        )
    });
    gates.push(format!(
        "configured archive: selected kinds [{}]; location {}",
        cfg.evidence.include.join(", "),
        archive_location.as_deref().unwrap_or("unavailable")
    ));
    // Preserve the legacy adapter port for embedders without invoking it: the configured
    // archive is the only payload copy required by this command.
    let _legacy_copy_port = E::copy_evidence;
    if let Err(e) = std::fs::create_dir_all(&receipts) {
        parked_locked!(rp::ParkReason::EvidencePreservationFailed {
            detail: e.to_string()
        });
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
    if let Err(detail) = fs_custody::pinned_root_unchanged(pin) {
        parked_locked!(rp::ParkReason::ScanRootIdentityChanged { detail })
    }
    let receipt_path = match encode_and_write(&receipt, &receipts, env) {
        Ok(p) => {
            report.intents.push(p.clone());
            p
        }
        Err(detail) => parked_locked!(rp::ParkReason::FoldReceiptUnavailable { detail }),
    };

    // The removal, with the LAST identity checks immediately before the unlink — the same
    // verify-then-act boundary the build-target reaper runs, from its one implementation. Only
    // the gate vocabulary below ("clone" rather than "payload") is this command's.
    let mut item_gates = gates.clone();
    let mut freed_bytes_measured = None;
    if let (Some(source), Some(main_ref), Some(expected)) =
        (&facts.source_repo, &facts.main_ref_full, &facts.main_oid)
    {
        if sr::ref_oid(source, main_ref).ok().as_ref() != Some(expected) {
            receipt.disposition = DISPOSITION_ABORTED.into();
            finish(&mut receipt, &receipts, &receipt_path, env, report, &run_id);
            parked_locked!(rp::ParkReason::SourceMainMoved {
                detail: "source main changed before removal".into()
            });
        }
    }
    let verified = fs_custody::verify_then_remove(pin, &path, &identity, || {
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
        freed_bytes_measured = match (before, after) {
            (Some(b), Some(a)) => Some(a as i64 - b as i64),
            _ => None,
        };
        removal
    });
    let outcome = 'removal: {
        let (observation, root_changed_during) = match verified {
            fs_custody::VerifiedRemovalV1::Refused(refusal) => {
                break 'removal rp::ItemOutcome::Parked {
                    reason: rp::removal_boundary_park_reason(refusal),
                };
            }
            fs_custody::VerifiedRemovalV1::Acted {
                observation,
                root_changed_during,
            } => (observation, root_changed_during),
        };
        let mut outcome = rp::removal_item_outcome(observation);
        if let Some(detail) = root_changed_during {
            outcome = rp::ItemOutcome::Unknown {
                detail: format!("the pinned scan root changed during the removal: {detail}"),
            };
        } else {
            item_gates.push("scan root identity re-verified AFTER the removal".to_string());
        }
        let mut out = base_item(outcome, item_gates.clone());
        out.freed_bytes_measured = freed_bytes_measured;
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
        if matches!(out.outcome, rp::ItemOutcome::Deleted) {
            if let Some(cleanup) = cleanup {
                if let Err(e) = cleanup.and_then(|c| c.delete()) {
                    let detail = format!("run {run_id}: incomplete remote cleanup after local deletion: {e}; remaining/unverified refs: refs/heads/{}{}, refs/heads/{}{}-dirty", cfg.custody.ref_prefix, run_id, cfg.custody.ref_prefix, run_id);
                    report.notes.push(detail.clone());
                    item_gates.push(detail.clone());
                    receipt.failure_detail = Some(detail);
                    out.gates = item_gates.clone();
                }
            }
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
    let archive_notes: Vec<_> = report
        .items
        .iter()
        .map(|it| {
            (
                it.path.clone(),
                it.gates
                    .iter()
                    .find(|g| g.starts_with("configured archive:"))
                    .cloned(),
            )
        })
        .collect();
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
        let detail = if *clean {
            format!(
                "removed WITH its enclosing clone {} — this row was not independently gated; the \
                 clone's own gates licensed it; {}",
                parent.display(),
                archive_notes.iter().find(|(p, _)| Path::new(p) == parent)
                    .and_then(|(_, note)| note.as_deref()).unwrap_or("configured archive: location unavailable; only configured evidence kinds are selected")
            )
        } else {
            format!(
                "removed before the failure that left its enclosing clone {} incomplete — restat'ed \
                 after the attempt and this path is gone",
                parent.display()
            )
        };
        // Real-run tables omit gates, so also render the descendant's actual disposition.
        report.notes.push(format!("{}: {detail}", it.path));
        it.gates.push(detail);
    }
}

// ---------------------------------------------------------------------------------------------
// Rendering + usage
// ---------------------------------------------------------------------------------------------

/// The banner deliberately names BOTH licences and commits to neither: with `--disposition-list` in
/// play, "content verifiably on main" would be a claim about rows for which nothing established it.
/// Which one licensed a given deletion is on that row's receipt, where it is auditable.
pub fn render_text(r: &rp::ReapReport) -> String {
    rp::render_report(r, "--clones", "DESTRUCTIVE: clones with confirmed A2A-Run/A2A-Original provenance on source main or explicit owner remove decisions were removed. Patch/content equivalence awaits adjudication. Shared custody and every live-use/path safeguard apply as recorded in each fold receipt.")
}

pub const CLONES_USAGE: &str = "\
usage: a2a-bridge storage reap --clones [--dry-run] [--disposition-list <f>] [--config <f>] [--json]

Automatic merged reap requires an exact A2A-Run/A2A-Original trailer pair on source main,
plus the shared custody/archive/ref/shape gate, using each run's recorded configuration.
Patch equivalence (high confidence) and content equivalence (low confidence, all changed
paths in one main commit within 14 days) are reports for adjudication, never provenance.
Missing metadata, invalid configuration and unknown probes preserve the clone.

Owner decisions use a strict JSON document:
{\"authorized_by\":\"owner\",\"reason\":\"reviewed report\",\"run_ids\":[\"impl-...\"],\"keep_run_ids\":[]}
run_ids explicitly REMOVE named clones and their recorded owned remote refs. keep_run_ids
explicitly KEEP named clones and refs. Contradictory decisions refuse; unspecified runs
need confirmed provenance for automatic removal. Remove waives merged classification and
Git-state custody (remote, cleanliness, archive, refs and shape allowlists). Every live-use,
operation-lock, protected-root, path-identity and container safeguard remains mandatory.

The configured archive replaces duplicate evidence copies. Discards retain the actual
terminal phase. Durable fold receipts precede removal and record its outcome. Only exact
successful local removal permits leased remote cleanup; changed foreign refs remain.
A dry run probes gates without receipts, archives, removal or remote deletion; the held
.operation-locks namespace may be created. Configured runtimes must answer the container
axis; an unconfigured axis is disclosed. Only exact standalone clone directories under
the pinned scan root qualify; linked worktrees and protected roots are retained.

Operator details: clone_reap_lookback bounds alternative equivalence probes; an exhausted
window reports unknown. Exact trailer provenance searches all reachable main history.
The shared clean check covers git status --porcelain. Fold receipts use <run>-fold.json
and retain ref_inventory and owner list_sha256. OWNER DISPOSITION is an explicit discard:
THE CONTENT-ON-MAIN GATE IS REPLACED along with Git custody by the owner's remove decision.
The previous rule \"A DIRTY dispositioned clone PARKS\" no longer applies to an explicit
remove; live-use and path safeguards still apply to every clone.
Historical yes(head)/yes(tree) content on main signals are adjudication evidence only.
The origin chain is walked BEFORE the first removal, bounded at 8 in-root edges; terminal
root corroboration checks the lineage root when a chain resolves. GIT_CEILING_DIRECTORIES
prevents Git discovery from escaping the selected directory.
";

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
        fx_with_prefix("clone-reap-")
    }

    fn fx_with_prefix(prefix: &str) -> Fx {
        let td = tempfile::Builder::new().prefix(prefix).tempdir().unwrap();
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
        let base = sr::git_str(&clone, &["rev-parse", "HEAD^"]).unwrap();
        let head = sr::git_str(&clone, &["rev-parse", "HEAD"]).unwrap();
        let branch = format!("implement/{run_id}");
        git(&clone, &["branch", "-m", &branch]);
        git(
            &source,
            &[
                "commit",
                "--allow-empty",
                "-qm",
                &format!("landed\n\nA2A-Run: {run_id}\nA2A-Original: {head}"),
            ],
        );
        let remote = root.join("backup.git");
        git(&source, &["init", "--bare", "-q", remote.to_str().unwrap()]);
        git(
            &source,
            &["remote", "add", "origin", remote.to_str().unwrap()],
        );
        let runtime = root.join("recorded-runtime");
        write(&runtime, "#!/bin/sh\nexit 0\n");
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&runtime, std::fs::Permissions::from_mode(0o755)).unwrap();
        let config_path = root.join("recorded.toml");
        write(&config_path, &format!("default = \"test\"\n[server]\naddr = \"127.0.0.1:0\"\n[custody]\ndelete_remote_on_merge = true\n[evidence]\npath = {:?}\n[verify]\nruntime = {:?}\nimage = \"test\"\ncache = \"test\"\negress = \"open\"\n", root.join("archives").to_str().unwrap(), runtime.to_str().unwrap()));
        let mut ck = bridge_controller::implement_resume::ImplementCheckpoint {
            schema_version: 1,
            resume_id: run_id.clone(),
            task_id: run_id.clone(),
            task_brief: "test".into(),
            source_repo: source.clone(),
            clone_path: clone.clone(),
            config_path,
            branch,
            base_ref: Some("main".into()),
            base_commit: base,
            current_commit: Some(head),
            original_message: None,
            edit_workflow: "edit".into(),
            fix_workflow: "fix".into(),
            loop_max_attempts: 3,
            review_invocation: 0,
            final_review: None,
            attempt_next: 1,
            forced_depth: None,
            resolved_lang: None,
            activity_tally: None,
            terminal_evidence_counts: None,
            phase: bridge_controller::implement_resume::ImplementPhase::Approved,
            created_at_ms: 0,
            updated_at_ms: 0,
        };
        bridge_controller::implement_resume::save_checkpoint(&clone, &ck).unwrap();
        write(&clone.join(".git/A2A_TASK.md"), "task spec");
        let cfg = crate::config::RegistryConfig::parse(
            &std::fs::read_to_string(&ck.config_path).unwrap(),
        )
        .unwrap();
        let sha = ck.current_commit.clone().unwrap();
        bridge_controller::custody_push::push_before_review_or_merge(
            &clone,
            &cfg.custody,
            &mut ck,
            &sha,
        );
        bridge_controller::evidence::write_run_archive(&clone, &ck, "approved", &cfg.evidence)
            .unwrap();
        Fx {
            _td: td,
            root,
            implement,
            source,
            clone,
            run_id,
        }
    }

    fn clone_commit(f: &Fx, branch: &str, file: &str, body: &str) -> String {
        git(&f.clone, &["checkout", "-q", "-b", branch]);
        write(&f.clone.join(file), body);
        git(&f.clone, &["add", "-A"]);
        git(&f.clone, &["commit", "-qm", "work"]);
        sr::git_str(&f.clone, &["rev-parse", "HEAD"]).unwrap()
    }

    fn scan(implement: &Path) -> Vec<sr::ReportItem> {
        let mut notes = Vec::new();
        let mut items = sr::scan_implement_root(implement, &mut notes);
        // The fixture runtime reports an empty, successful container scan. Tests of an unanswered
        // invocation scan override this explicitly; the filesystem scanner alone does not probe it.
        for item in &mut items {
            item.consumers.container_mount = sr::HolderState::Free;
        }
        items
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
        assert_eq!(v["containment"]["verdict"], "confirmed-provenance");
        assert_eq!(v["disposition"], DISPOSITION_DELETED);
    }

    // -----------------------------------------------------------------------------------------
    // F1 — every ref, not just HEAD. Containment proves HEAD; deletion takes the whole object store.
    // -----------------------------------------------------------------------------------------

    // -----------------------------------------------------------------------------------------
    // Git state
    // -----------------------------------------------------------------------------------------

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
                consumers: sr::LiveConsumers {
                    container_mount: sr::HolderState::Free,
                    ..Default::default()
                },
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
        let items = scan(&f.implement); // invocation scan cannot answer the recorded runtime
        std::fs::remove_file(f.root.join("recorded-runtime")).unwrap();
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
        if std::env::var_os("A2A_DEFAULT_ARCHIVE_REPORT_TEST").is_none() {
            let state = tempfile::tempdir().unwrap();
            let out = Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "storage_reap_clones::tests::rows_inside_a_deleted_clone_are_recorded_as_deleted_with_it"])
                .env("A2A_DEFAULT_ARCHIVE_REPORT_TEST", "1")
                .env("A2A_BRIDGE_STATE_DIR", state.path())
                .output().unwrap();
            assert!(
                out.status.success(),
                "{}{}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            );
            return;
        }
        let f = fx();
        let (ck, _) = recorded_config(&f.clone, &f.run_id).unwrap();
        let raw = std::fs::read_to_string(&ck.config_path).unwrap().replace(
            &format!("path = {:?}\n", f.root.join("archives").to_str().unwrap()),
            "",
        );
        write(&ck.config_path, &raw);
        let (_, cfg) = recorded_config(&f.clone, &f.run_id).unwrap();
        assert!(cfg.evidence.path.is_none());
        bridge_controller::evidence::write_run_archive(&f.clone, &ck, "approved", &cfg.evidence)
            .unwrap();
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
        let rendered = render_text(&r);
        assert!(
            rendered.contains("selected kinds [spec, final_review_summary]"),
            "{rendered}"
        );
        assert!(
            rendered.contains(
                &bridge_core::platform_state::platform_state_dir()
                    .unwrap()
                    .join("evidence")
                    .display()
                    .to_string()
            ),
            "{rendered}"
        );
        assert!(!rendered.contains("copied into the receipt"), "{rendered}");
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

    fn two_hop(f: &Fx) -> PathBuf {
        let parent = f.implement.join(format!("impl-{DEAD_PID}-parent"));
        git(
            &f.source,
            &[
                "clone",
                "--no-hardlinks",
                "-q",
                f.source.to_str().unwrap(),
                parent.to_str().unwrap(),
            ],
        );
        let (mut ck, cfg) = recorded_config(&f.clone, &f.run_id).unwrap();
        let raw = std::fs::read_to_string(&ck.config_path).unwrap();
        write(
            &ck.config_path,
            &format!("{raw}\n[implement]\nclone_config_extra_keys = ['remote.origin.pushurl']\n"),
        );
        git(
            &f.clone,
            &["remote", "set-url", "origin", parent.to_str().unwrap()],
        );
        // Use a separate push URL to preserve the origin chain for resolution.
        git(
            &parent,
            &[
                "remote",
                "set-url",
                "--push",
                "origin",
                f.root.join("backup.git").to_str().unwrap(),
            ],
        );
        // The immediate source must not carry the child's landing trailer. Only the terminal
        // root knows that landing; the parent has its own distinct implementation and trailer pair.
        git(
            &parent,
            &["commit", "--amend", "--allow-empty", "-qm", "parent work"],
        );
        let parent_head = sr::git_str(&parent, &["rev-parse", "HEAD"]).unwrap();
        let mut parent_ck = ck.clone();
        parent_ck.source_repo = f.source.clone();
        parent_ck.clone_path = parent.clone();
        parent_ck.task_id = parent.file_name().unwrap().to_str().unwrap().into();
        parent_ck.resume_id = parent_ck.task_id.clone();
        parent_ck.branch = format!("implement/{}", parent_ck.task_id);
        parent_ck.current_commit = Some(parent_head.clone());
        git(&parent, &["branch", "-m", &parent_ck.branch]);
        bridge_controller::implement_resume::save_checkpoint(&parent, &parent_ck).unwrap();
        write(&parent.join(".git/A2A_TASK.md"), "parent task spec");
        bridge_controller::custody_push::push_before_review_or_merge(
            &parent,
            &cfg.custody,
            &mut parent_ck,
            &parent_head,
        );
        bridge_controller::evidence::write_run_archive(
            &parent,
            &parent_ck,
            "approved",
            &cfg.evidence,
        )
        .unwrap();
        git(
            &f.source,
            &[
                "commit",
                "--allow-empty",
                "-qm",
                &format!(
                    "parent landed\n\nA2A-Run: {}\nA2A-Original: {parent_head}",
                    parent_ck.task_id
                ),
            ],
        );
        ck.source_repo = parent.clone();
        bridge_controller::implement_resume::save_checkpoint(&f.clone, &ck).unwrap();
        bridge_controller::evidence::write_run_archive(&f.clone, &ck, "approved", &cfg.evidence)
            .unwrap();
        parent
    }

    #[test]
    fn two_hop_origin_uses_terminal_provenance_and_immediate_custody() {
        let f = fx();
        let parent = two_hop(&f);
        let chain = resolve_origin_root(&f.clone, &f.implement).unwrap();
        assert_eq!(chain.hops.len(), 2);
        assert_eq!(chain.root, f.source);
        let (ck, cfg) = recorded_config(&f.clone, &f.run_id).unwrap();
        assert_eq!(ck.source_repo, parent);
        assert!(bridge_controller::clone_deletion::failures_for_phase(
            &f.clone,
            &ck,
            &cfg.custody,
            &cfg.evidence,
            "approved"
        )
        .is_empty());
        assert!(
            !landing(&f).confirmed(),
            "immediate source must not prove child landing"
        );
        let mut items = scan(&f.implement);
        items.retain(|item| item.path != parent.to_str().unwrap());
        let env = FakeEnv::new();
        let report = run(&f, &items, &env, false);
        assert_eq!(
            item_for(&report, &f.clone).outcome,
            rp::ItemOutcome::Deleted,
            "{report:?}"
        );
        let receipt = fold_receipt(&f);
        assert_eq!(receipt["source_repo"], f.source.display().to_string());
        assert_eq!(receipt["origin_chain"].as_array().unwrap().len(), 2);
        assert_remote(&f, false);
        assert!(parent.exists());
    }

    #[test]
    fn a_chained_pair_is_reaped_in_one_invocation_whatever_the_order() {
        for parent_first in [false, true] {
            let f = fx();
            let parent = two_hop(&f);
            assert!(!landing(&f).confirmed());
            let child_landing = sr::git_str(&f.source, &["rev-parse", "HEAD^"]).unwrap();
            let parent_landing = sr::git_str(&f.source, &["rev-parse", "HEAD"]).unwrap();
            assert_ne!(child_landing, parent_landing);
            let mut items = scan(&f.implement);
            items.sort_by_key(|item| (item.path == parent.to_str().unwrap()) != parent_first);
            assert_eq!(items[0].path == parent.to_str().unwrap(), parent_first);
            let env = FakeEnv::new();
            let report = run(&f, &items, &env, false);
            for (path, landing) in [(&f.clone, child_landing), (&parent, parent_landing)] {
                assert_eq!(
                    item_for(&report, path).outcome,
                    rp::ItemOutcome::Deleted,
                    "{report:?}"
                );
                assert!(!path.exists());
                let receipt: serde_json::Value = serde_json::from_slice(
                    &std::fs::read(receipts_dir(&f.implement).join(fold_receipt_name(
                        path.file_name().unwrap().to_str().unwrap(),
                    )))
                    .unwrap(),
                )
                .unwrap();
                assert_eq!(receipt["containment"]["matched_commit"], landing);
                assert_eq!(receipt["source_repo"], f.source.to_str().unwrap());
            }
            assert_eq!(
                env.removed(),
                vec![f.clone.display().to_string(), parent.display().to_string()]
            );
            assert!(f.source.exists());
        }
    }

    #[test]
    fn a_parked_child_keeps_its_source_clone() {
        for parent_first in [false, true] {
            for explicit in [false, true] {
                for admission_refusal in [false, true] {
                    let f = fx();
                    let parent = two_hop(&f);
                    let mut items = scan(&f.implement);
                    items.sort_by_key(|item| {
                        (item.path == parent.to_str().unwrap()) != parent_first
                    });
                    let child = items
                        .iter_mut()
                        .find(|item| item.path == f.clone.to_str().unwrap())
                        .unwrap();
                    if admission_refusal {
                        child.checkout_kind = Some(sr::CheckoutKind::LinkedWorktree);
                    } else {
                        child.consumers.container_mount = sr::HolderState::Unknown;
                    }
                    let lic = license(
                        &f,
                        &[&f.run_id, parent.file_name().unwrap().to_str().unwrap()],
                    );
                    let env = FakeEnv::new();
                    let report =
                        run_licensed(&f, &items, &env, false, 1, 2000, explicit.then_some(&lic));
                    assert!(matches!(
                        item_for(&report, &f.clone).outcome,
                        rp::ItemOutcome::Parked { .. }
                    ));
                    assert!(
                        matches!(parked_reason(&report, &parent), rp::ParkReason::LiveConsumer { kind, .. } if kind == "dependent clone")
                    );
                    assert!(env.removed().is_empty());
                    assert!(f.clone.exists() && parent.exists());
                }
            }
        }
    }

    #[test]
    fn chained_origins_refuse_checkpoint_disagreement_and_cycles() {
        for cycle in [false, true] {
            let f = fx();
            let parent = two_hop(&f);
            if cycle {
                git(
                    &parent,
                    &["remote", "set-url", "origin", f.clone.to_str().unwrap()],
                );
                let mut ck = bridge_controller::implement_resume::load_checkpoint(&parent).unwrap();
                ck.source_repo = f.clone.clone();
                bridge_controller::implement_resume::save_checkpoint(&parent, &ck).unwrap();
            } else {
                git(
                    &parent,
                    &[
                        "remote",
                        "set-url",
                        "origin",
                        f.root.join("backup.git").to_str().unwrap(),
                    ],
                );
            }
            assert!(resolve_origin_root(&f.clone, &f.implement).is_err());
            let report = run(&f, &scan(&f.implement), &FakeEnv::new(), false);
            assert!(matches!(
                item_for(&report, &f.clone).outcome,
                rp::ItemOutcome::Parked { .. }
            ));
            assert_remote(&f, true);
        }
    }

    #[test]
    fn owner_remove_deletes_clone_when_recorded_parent_is_gone() {
        let f = fx();
        let parent = two_hop(&f);
        std::fs::remove_dir_all(parent).unwrap();
        let lic = license(&f, &[&f.run_id]);
        let report = run_licensed(
            &f,
            &scan(&f.implement),
            &FakeEnv::new(),
            false,
            0,
            2000,
            Some(&lic),
        );
        assert_eq!(
            item_for(&report, &f.clone).outcome,
            rp::ItemOutcome::Deleted,
            "{report:?}"
        );
        assert!(!f.clone.exists());
        assert!(fold_receipt(&f)["failure_detail"]
            .as_str()
            .unwrap()
            .contains("remote cleanup unavailable"));
        assert_remote(&f, true);
    }

    #[test]
    fn encoded_path_holders_without_process_records_keep_clones() {
        let long_prefix = "long-path-".repeat(20);
        for prefix in ["ordinary-", "nonascii-é-", long_prefix.as_str()] {
            for holder_label in [None, Some("cwd"), Some("repo")] {
                for explicit in [false, true] {
                    let f = fx_with_prefix(prefix);
                    assert!(!f.clone.join(".git/a2a-bridge/container-runs.json").exists());
                    let path = f.clone.to_str().unwrap();
                    // Independent expectation of the backend's printable ASCII, 200-char label.
                    let encoded: String = path
                        .chars()
                        .filter(|c| c.is_ascii_graphic() || *c == ' ')
                        .take(200)
                        .collect();
                    let queries = f.root.join("runtime-queries");
                    let response = holder_label
                        .map(|key| {
                            format!(
                        "if [ \"$arg\" = 'label=a2a.{key}={encoded}' ]; then printf 'holder\\n'; fi"
                    )
                        })
                        .unwrap_or_default();
                    write(&f.root.join("recorded-runtime"), &format!(
                        "#!/bin/sh\nfor arg in \"$@\"; do\nprintf '%s\\n' \"$arg\" >> '{}'\n{response}\ndone\n", queries.display()
                    ));
                    let lic = license(&f, &[&f.run_id]);
                    let env = FakeEnv::new();
                    let report = run_licensed(
                        &f,
                        &scan(&f.implement),
                        &env,
                        false,
                        1,
                        2000,
                        explicit.then_some(&lic),
                    );
                    let queried = std::fs::read_to_string(queries).unwrap();
                    assert!(queried.contains(&format!("label=a2a.cwd={encoded}")));
                    assert!(!queried.contains(&format!("label=a2a.run={}", f.run_id)));
                    if holder_label.is_some() {
                        assert!(
                            matches!(
                                parked_reason(&report, &f.clone),
                                rp::ParkReason::LiveConsumer { .. }
                            ),
                            "{report:?}"
                        );
                    } else if encoded != path {
                        assert_eq!(
                            parked_reason(&report, &f.clone),
                            rp::ParkReason::ContainerAxisUnanswered
                        );
                    } else {
                        assert_eq!(
                            item_for(&report, &f.clone).outcome,
                            rp::ItemOutcome::Deleted,
                            "{report:?}"
                        );
                        assert!(!f.clone.exists());
                        continue;
                    }
                    assert!(env.removed().is_empty());
                    assert!(f.clone.exists());
                    assert_remote(&f, true);
                }
            }
        }
    }

    #[test]
    fn unknown_container_scan_keeps_clone_even_when_recorded_runtime_answers_empty() {
        for explicit in [false, true] {
            let f = fx();
            let mut items = scan(&f.implement);
            for item in &mut items {
                item.consumers.container_mount = sr::HolderState::Unknown;
            }
            let lic = license(&f, &[&f.run_id]);
            let env = FakeEnv::new();
            let report = run_licensed(&f, &items, &env, false, 1, 2000, explicit.then_some(&lic));
            assert_eq!(
                parked_reason(&report, &f.clone),
                rp::ParkReason::ContainerAxisUnanswered
            );
            assert!(env.removed().is_empty());
            assert!(f.clone.exists());
        }
    }

    #[test]
    fn recorded_runtime_holder_or_unavailable_query_blocks_even_owner_remove() {
        for explicit in [false, true] {
            for response in ["printf 'holder\\n'", "exit 1"] {
                let f = fx();
                let invocation = f.root.join("invocation-runtime");
                write(&invocation, "#!/bin/sh\nexit 0\n");
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&invocation, std::fs::Permissions::from_mode(0o755))
                    .unwrap();
                assert!(Command::new(&invocation)
                    .args(["ps", "-aq"])
                    .output()
                    .unwrap()
                    .stdout
                    .is_empty());
                write(
                    &f.root.join("recorded-runtime"),
                    &format!("#!/bin/sh\n{response}\n"),
                );
                let mut items = scan(&f.implement);
                for item in &mut items {
                    item.consumers.container_mount = sr::HolderState::Free;
                }
                let lic = license(&f, &[&f.run_id]);
                let env = FakeEnv::new();
                let report =
                    run_licensed(&f, &items, &env, false, 1, 2000, explicit.then_some(&lic));
                assert!(
                    matches!(
                        parked_reason(&report, &f.clone),
                        rp::ParkReason::LiveConsumer { .. }
                            | rp::ParkReason::ContainerAxisUnanswered
                    ),
                    "{report:?}"
                );
                assert!(env.removed().is_empty());
                assert!(f.clone.exists());
                assert_remote(&f, true);
            }
        }
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

    // =========================================================================================
    // S4b review repairs (R1–R7)
    // =========================================================================================

    fn landing(f: &Fx) -> bridge_controller::merge::LandingEvidence {
        let (ck, _) = recorded_config(&f.clone, &f.run_id).unwrap();
        let head = sr::git_str(&f.clone, &["rev-parse", "HEAD"]).unwrap();
        bridge_controller::merge::classify_landing(
            &f.clone,
            &ck,
            &head,
            DEFAULT_CLONE_REAP_LOOKBACK,
        )
    }

    #[test]
    fn only_exact_main_trailers_confirm_provenance() {
        for case in [
            "exact",
            "wrong-run",
            "wrong-original",
            "duplicate",
            "other-branch",
        ] {
            let f = fx();
            let head = sr::git_str(&f.clone, &["rev-parse", "HEAD"]).unwrap();
            let run = if case == "wrong-run" {
                "impl-foreign"
            } else {
                &f.run_id
            };
            let original = if case == "wrong-original" {
                "wrong"
            } else {
                &head
            };
            let duplicate = if case == "duplicate" {
                "\nA2A-Run: impl-foreign"
            } else {
                ""
            };
            git(
                &f.source,
                &[
                    "commit",
                    "--amend",
                    "--allow-empty",
                    "-qm",
                    &format!("landed\n\nA2A-Run: {run}\nA2A-Original: {original}{duplicate}"),
                ],
            );
            let matched = sr::git_str(&f.source, &["rev-parse", "main"]).unwrap();
            if case == "other-branch" {
                git(&f.source, &["branch", "side", &matched]);
                git(&f.source, &["update-ref", "refs/heads/main", &head]);
            }
            let evidence = landing(&f);
            assert_eq!(
                evidence.confirmed(),
                case == "exact",
                "{case}: {evidence:?}"
            );
            if case == "exact" {
                assert_eq!(evidence.matched_commit.as_deref(), Some(matched.as_str()));
            }
        }
    }

    #[test]
    fn patch_and_content_equivalence_are_reported_and_parked() {
        for content_only in [false, true] {
            let f = fx();
            if content_only {
                write(&f.source.join("unrelated"), "additional integration work");
                git(&f.source, &["add", "unrelated"]);
            }
            let (ck, _) = recorded_config(&f.clone, &f.run_id).unwrap();
            git(
                &f.source,
                &["update-ref", "refs/heads/main", &ck.base_commit],
            );
            git(&f.source, &["add", "-A"]);
            git(&f.source, &["commit", "-qm", "ordinary landing"]);
            let evidence = landing(&f);
            assert_eq!(
                evidence.confidence,
                if content_only {
                    "low-content-equivalence"
                } else {
                    "high-patch-equivalence"
                }
            );
            let env = FakeEnv::new();
            let report = run(&f, &scan(&f.implement), &env, false);
            let reason = parked_reason(&report, &f.clone).summary();
            for field in [
                &f.run_id,
                "confidence",
                "matched",
                "source",
                "deletion blockers",
            ] {
                assert!(reason.contains(field), "{reason}");
            }
            assert!(f.clone.exists());
            assert!(env.removed().is_empty());
        }
    }

    #[test]
    fn content_equivalence_requires_every_path_deletion_and_time_window() {
        for case in ["match", "missing", "wrong-deletion", "partial", "late"] {
            let f = fx();
            let (mut ck, _) = recorded_config(&f.clone, &f.run_id).unwrap();
            ck.base_commit = sr::git_str(&f.clone, &["rev-parse", "HEAD"]).unwrap();
            git(&f.clone, &["rm", "README.md"]);
            write(&f.clone.join("new"), "new bytes");
            write(&f.clone.join("src/lib.rs"), "changed bytes");
            git(&f.clone, &["add", "-A"]);
            git(&f.clone, &["commit", "-qm", "run delta"]);
            bridge_controller::implement_resume::save_checkpoint(&f.clone, &ck).unwrap();
            if case != "wrong-deletion" {
                git(&f.source, &["rm", "README.md"]);
            }
            if case != "missing" {
                write(&f.source.join("new"), "new bytes");
            }
            if case != "partial" {
                write(&f.source.join("src/lib.rs"), "changed bytes");
            }
            write(&f.source.join("extra"), "makes patch different");
            git(&f.source, &["add", "-A"]);
            let clone_time: i64 = sr::git_str(&f.clone, &["show", "-s", "--format=%ct", "HEAD"])
                .unwrap()
                .parse()
                .unwrap();
            let time = clone_time + if case == "late" { 14 * 86400 + 1 } else { 1 };
            let out = Command::new("git")
                .arg("-C")
                .arg(&f.source)
                .args(["commit", "-qm", "integration"])
                .env("GIT_AUTHOR_NAME", "t")
                .env("GIT_AUTHOR_EMAIL", "t@t")
                .env("GIT_COMMITTER_NAME", "t")
                .env("GIT_COMMITTER_EMAIL", "t@t")
                .env("GIT_COMMITTER_DATE", format!("{time} +0000"))
                .output()
                .unwrap();
            assert!(out.status.success(), "{:?}", out);
            let evidence = landing(&f);
            assert_eq!(
                evidence.confidence,
                if case == "match" {
                    "low-content-equivalence"
                } else {
                    "absent"
                },
                "{case}: {evidence:?}"
            );
        }
    }

    #[test]
    fn metadata_and_probe_failures_preserve_clone_and_remote() {
        for case in [
            "missing-checkpoint",
            "invalid-schema",
            "missing-config",
            "invalid-config",
            "missing-main",
        ] {
            let f = fx();
            let (ck, _) = recorded_config(&f.clone, &f.run_id).unwrap();
            match case {
                "missing-checkpoint" => std::fs::remove_file(
                    bridge_controller::implement_resume::checkpoint_path(&f.clone),
                )
                .unwrap(),
                "invalid-schema" => {
                    let path = bridge_controller::implement_resume::checkpoint_path(&f.clone);
                    let mut value: serde_json::Value =
                        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
                    value["schema_version"] = serde_json::json!(999);
                    std::fs::write(path, serde_json::to_vec(&value).unwrap()).unwrap();
                }
                "missing-config" => std::fs::remove_file(&ck.config_path).unwrap(),
                "invalid-config" => write(&ck.config_path, "invalid toml"),
                "missing-main" => git(&f.source, &["update-ref", "-d", "refs/heads/main"]),
                _ => unreachable!(),
            }
            let env = FakeEnv::new();
            let report = run(&f, &scan(&f.implement), &env, false);
            assert!(matches!(
                item_for(&report, &f.clone).outcome,
                rp::ItemOutcome::Parked { .. }
            ));
            assert!(f.clone.exists());
            assert_remote(&f, true);
            if case == "missing-main" {
                assert_eq!(landing(&f).confidence, "unknown");
            }
        }
    }

    fn assert_remote(f: &Fx, present: bool) {
        let remote = f.root.join("backup.git");
        let reference = format!("refs/heads/wip/{}", f.run_id);
        let out = sr::git_ro(&remote, &["show-ref", "--verify", &reference]).unwrap();
        assert_eq!(out.status.success(), present, "{reference}");
    }

    #[test]
    fn shared_gate_agrees_with_reap_for_all_custody_blockers() {
        for case in [
            "remote-mismatch",
            "disabled-push",
            "dirty",
            "bad-archive",
            "side-ref",
            "index",
            "nested",
            "worktree",
            "ignored",
        ] {
            let f = fx();
            let (ck, mut cfg) = recorded_config(&f.clone, &f.run_id).unwrap();
            match case {
                "remote-mismatch" => git(
                    &f.root.join("backup.git"),
                    &[
                        "update-ref",
                        &format!("refs/heads/wip/{}", f.run_id),
                        &ck.base_commit,
                    ],
                ),
                "disabled-push" => {
                    cfg.custody.push = false;
                    let raw = std::fs::read_to_string(&ck.config_path).unwrap().replace(
                        "delete_remote_on_merge = true",
                        "delete_remote_on_merge = true\npush = false",
                    );
                    write(&ck.config_path, &raw);
                }
                "dirty" => write(&f.clone.join("src/lib.rs"), "dirty"),
                "bad-archive" => {
                    let archive = cfg
                        .evidence
                        .path
                        .as_ref()
                        .unwrap()
                        .join(bridge_controller::evidence::repo_key(&f.source).unwrap())
                        .join(format!("{}.tar.gz", f.run_id));
                    write(&archive, "invalid");
                }
                "side-ref" => git(
                    &f.clone,
                    &[
                        "update-ref",
                        "refs/stash",
                        ck.current_commit.as_deref().unwrap(),
                    ],
                ),
                "index" => git(
                    &f.clone,
                    &["update-index", "--assume-unchanged", "src/lib.rs"],
                ),
                "nested" => {
                    std::fs::create_dir_all(f.clone.join("target/nested/.git")).unwrap();
                }
                "worktree" => {
                    git(
                        &f.clone,
                        &[
                            "worktree",
                            "add",
                            "-q",
                            "-b",
                            "linked",
                            f.root.join("linked").to_str().unwrap(),
                        ],
                    );
                }
                "ignored" => write(&f.clone.join("target/ordinary-output"), "cache"),
                _ => unreachable!(),
            }
            let failures = bridge_controller::clone_deletion::failures_for_phase(
                &f.clone,
                &ck,
                &cfg.custody,
                &cfg.evidence,
                "approved",
            );
            assert_eq!(
                failures.is_empty(),
                case == "ignored",
                "{case}: {failures:?}"
            );
            let env = FakeEnv::new();
            let report = run(&f, &scan(&f.implement), &env, false);
            assert_eq!(
                matches!(
                    item_for(&report, &f.clone).outcome,
                    rp::ItemOutcome::Deleted
                ),
                case == "ignored",
                "{case}: {report:?}"
            );
            assert_remote(&f, case != "ignored");
        }
    }

    #[test]
    fn explicit_remove_discards_dirty_parked_work_and_keeps_actual_archive_phase() {
        let f = fx();
        git(&f.source, &["update-ref", "-d", "refs/heads/main"]);
        write(&f.clone.join("unsaved"), "owner-authorized discard");
        git(
            &f.clone,
            &["update-index", "--assume-unchanged", "src/lib.rs"],
        );
        git(&f.clone, &["update-ref", "refs/stash", "HEAD"]);
        std::fs::create_dir_all(f.clone.join("target/nested/.git")).unwrap();
        let (_, cfg) = recorded_config(&f.clone, &f.run_id).unwrap();
        let lic = license(&f, &[&f.run_id]);
        let env = FakeEnv::new();
        let report = run_licensed(&f, &scan(&f.implement), &env, false, 0, 2000, Some(&lic));
        assert_eq!(
            item_for(&report, &f.clone).outcome,
            rp::ItemOutcome::Deleted,
            "{report:?}"
        );
        assert_remote(&f, false);
        let archive = cfg
            .evidence
            .path
            .unwrap()
            .join(bridge_controller::evidence::repo_key(&f.source).unwrap())
            .join(format!("{}.tar.gz", f.run_id));
        let mut tar = tar::Archive::new(flate2::read::GzDecoder::new(
            std::fs::File::open(archive).unwrap(),
        ));
        let mut phase = None;
        for entry in tar.entries().unwrap() {
            let mut entry = entry.unwrap();
            if entry.path().unwrap().as_ref() == Path::new("manifest.json") {
                let manifest: bridge_controller::evidence::Manifest =
                    serde_json::from_reader(&mut entry).unwrap();
                phase = Some(manifest.terminal_phase);
            }
        }
        assert_eq!(phase.as_deref(), Some("approved"));
        assert_eq!(
            fold_receipt(&f)["containment"]["verdict"],
            VERDICT_OWNER_DISPOSITION
        );
    }

    #[test]
    fn keep_unspecified_and_contradictory_decisions_preserve_work() {
        let f = fx();
        let mut lic = license(&f, &["impl-other"]);
        lic.keep_run_ids.insert(f.run_id.clone());
        let env = FakeEnv::new();
        let report = run_licensed(&f, &scan(&f.implement), &env, false, 0, 2000, Some(&lic));
        assert!(parked_reason(&report, &f.clone).summary().contains("keep"));
        assert_remote(&f, true);
        assert!(parse_disposition_document(&format!(r#"{{"authorized_by":"owner","reason":"reviewed","run_ids":["{}"],"keep_run_ids":["{}"]}}"#, f.run_id, f.run_id)).is_err());
        // A programmatic contradictory document is refused too, before deletion.
        lic.run_ids.insert(f.run_id.clone());
        let report = run_licensed(&f, &scan(&f.implement), &env, false, 0, 2000, Some(&lic));
        assert!(matches!(
            item_for(&report, &f.clone).outcome,
            rp::ItemOutcome::Parked { .. }
        ));
        assert_remote(&f, true);
    }

    #[test]
    fn failed_local_removal_never_deletes_remote_refs() {
        let f = fx();
        let env = FakeEnv::new();
        *env.remove.borrow_mut() = Box::new(|_| Err("injected local removal failure".into()));
        let report = run(&f, &scan(&f.implement), &env, false);
        assert!(!matches!(
            item_for(&report, &f.clone).outcome,
            rp::ItemOutcome::Deleted
        ));
        assert_remote(&f, true);
    }

    #[test]
    fn changed_remote_ref_after_preparation_is_retained_and_reported() {
        let f = fx();
        let (ck, _) = recorded_config(&f.clone, &f.run_id).unwrap();
        let remote = f.root.join("backup.git");
        let reference = format!("refs/heads/wip/{}", f.run_id);
        let env = FakeEnv::new();
        *env.remove.borrow_mut() = Box::new(move |path| {
            git(&remote, &["update-ref", &reference, &ck.base_commit]);
            std::fs::remove_dir_all(path).map_err(|e| e.to_string())
        });
        let report = run(&f, &scan(&f.implement), &env, false);
        assert_eq!(
            item_for(&report, &f.clone).outcome,
            rp::ItemOutcome::Deleted
        );
        assert!(report
            .notes
            .iter()
            .any(|n| n.contains("incomplete remote cleanup")));
        assert_remote(&f, true);
    }
    #[test]
    fn rename_equivalence_requires_the_old_path_to_be_deleted() {
        let f = fx();
        let (mut ck, _) = recorded_config(&f.clone, &f.run_id).unwrap();
        ck.base_commit = sr::git_str(&f.clone, &["rev-parse", "HEAD"]).unwrap();
        git(&f.clone, &["mv", "README.md", "renamed.md"]);
        git(&f.clone, &["commit", "-qm", "rename"]);
        bridge_controller::implement_resume::save_checkpoint(&f.clone, &ck).unwrap();
        write(&f.source.join("renamed.md"), "one\n");
        write(&f.source.join("extra"), "unrelated");
        git(&f.source, &["add", "-A"]);
        git(&f.source, &["commit", "-qm", "copy plus integration"]);
        assert_eq!(landing(&f).confidence, "absent");
    }

    #[test]
    fn automatic_and_owner_cleanup_remove_recorded_normal_and_dirty_refs() {
        for explicit in [false, true] {
            let f = fx();
            let (ck, _) = recorded_config(&f.clone, &f.run_id).unwrap();
            let sha = ck.current_commit.as_ref().unwrap();
            let dirty = format!("refs/heads/wip/{}-dirty", f.run_id);
            git(&f.root.join("backup.git"), &["update-ref", &dirty, sha]);
            let path = bridge_controller::implement_resume::checkpoint_path(&f.clone);
            let mut checkpoint: serde_json::Value =
                serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
            checkpoint["custody"]["dirty"] = serde_json::json!({"ref":dirty,"last_pushed_sha":sha});
            std::fs::write(path, serde_json::to_vec(&checkpoint).unwrap()).unwrap();
            if explicit {
                let raw = std::fs::read_to_string(&ck.config_path).unwrap().replace(
                    "delete_remote_on_merge = true",
                    "delete_remote_on_merge = false",
                );
                write(&ck.config_path, &raw);
                git(&f.source, &["update-ref", "-d", "refs/heads/main"]);
            }
            let lic = license(&f, &[&f.run_id]);
            let env = FakeEnv::new();
            let report = run_licensed(
                &f,
                &scan(&f.implement),
                &env,
                false,
                0,
                2000,
                explicit.then_some(&lic),
            );
            assert_eq!(
                item_for(&report, &f.clone).outcome,
                rp::ItemOutcome::Deleted,
                "{report:?}"
            );
            assert_remote(&f, false);
            assert!(!sr::git_ro(
                &f.root.join("backup.git"),
                &["show-ref", "--verify", &dirty]
            )
            .unwrap()
            .status
            .success());
        }
    }

    #[test]
    fn owner_discard_retains_every_operational_safeguard() {
        for case in [
            "pid",
            "lock",
            "consumer",
            "container",
            "protected",
            "path-swap",
        ] {
            let f = fx();
            write(&f.clone.join("dirty"), "discard authorized");
            let lic = license(&f, &[&f.run_id]);
            let mut env = FakeEnv::new();
            let mut items = scan(&f.implement);
            let mut protected = rp::ProtectedRoots::default();
            match case {
                "pid" => {
                    env.alive_pids.insert(DEAD_PID);
                }
                "lock" => {
                    env.contended.insert(f.run_id.clone());
                }
                "consumer" => {
                    *env.probe.borrow_mut() = Box::new(|_| rp::ConsumerProbe::Held {
                        detail: "cwd held".into(),
                    });
                }
                "container" => {
                    for item in &mut items {
                        if item.path == f.clone.display().to_string() {
                            item.consumers.container_mount = sr::HolderState::Held;
                        }
                    }
                }
                "protected" => {
                    protected =
                        rp::ProtectedRoots::resolve(&[f.clone.display().to_string()], &mut vec![])
                            .unwrap();
                }
                "path-swap" => {
                    let saved = f.root.join("saved");
                    *env.probe.borrow_mut() = Box::new(move |path| {
                        std::fs::rename(path, &saved).unwrap();
                        std::fs::create_dir_all(path.join(".git")).unwrap();
                        rp::ConsumerProbe::Free
                    });
                }
                _ => unreachable!(),
            }
            let report = reap_clones(
                ClonesRequest {
                    scan_root: &f.implement,
                    items: &items,
                    protected: &protected,
                    dry_run: false,
                    runtimes_configured: 0,
                    lookback: 2000,
                    disposition: Some(&lic),
                },
                &env,
            );
            assert!(
                matches!(
                    item_for(&report, &f.clone).outcome,
                    rp::ItemOutcome::Parked { .. }
                ),
                "{case}: {report:?}"
            );
            assert!(env.removed().is_empty());
            assert_remote(&f, true);
        }
    }

    #[test]
    fn dry_run_does_not_publish_remove_or_clean_remote_refs() {
        let f = fx();
        let env = FakeEnv::new();
        let report = run(&f, &scan(&f.implement), &env, true);
        assert_eq!(
            item_for(&report, &f.clone).outcome,
            rp::ItemOutcome::Planned
        );
        assert!(f.clone.exists());
        assert_remote(&f, true);
        assert!(report.receipts.is_empty() && report.intents.is_empty());
        assert!(!receipts_dir(&f.implement).exists());
        assert!(env.removed().is_empty());
    }
    #[test]
    fn configured_archive_requires_no_duplicate_copy() {
        let f = fx();
        let mut env = FakeEnv::new();
        env.copy_error = Some("legacy copy port must not be invoked".into());
        let report = run(&f, &scan(&f.implement), &env, false);
        assert_eq!(
            item_for(&report, &f.clone).outcome,
            rp::ItemOutcome::Deleted
        );
        assert!(!env.j.borrow().kinds().contains(&"copy-evidence"));
    }
}
