//! `a2a-bridge storage report` — the READ-ONLY audit instrument for bridge-owned storage (R2f1b
//! pre-slice-2 custody plan §3 S2). It walks bridge-owned roots only, classifies every item by payload
//! class, measures it, and reports who is still consuming it. It deletes nothing, pushes nothing, and
//! fetches nothing.
//!
//! **The one state-changing exception, declared rather than hidden** ([`LOCK_PROBE_DISCLOSURE`]):
//! reading lock/lease liveness means TAKING an advisory `flock` and immediately releasing it, because
//! there is no query-only flock API. A merge or resume that races that microsecond window sees a clean
//! "already held" refusal and can retry. Nothing else here touches state.
//!
//! Read-only discipline, enforced rather than assumed:
//! - every git probe runs `git --no-optional-locks`, so `status` cannot refresh-and-rewrite
//!   `.git/index` (a fail-first control confirms this: without the flag the byte-identical assertion
//!   fails, and the only differing entry is `.git/index`);
//! - liveness reuses ADR-0025's [`bridge_core::liveness::FsLeaseProbe`], which OPENS an existing lock
//!   path (never `create`, never `truncate`) — unlike `acquire_persistent_lock_in`, which would
//!   `create_dir_all` the namespace;
//! - **no existence or type check ever follows a symlink.** Every one goes through [`real_meta`]. A
//!   `:rw` container can plant `ln -s <user checkout> .git/a2a-bridge`, and following it would measure
//!   a D-2 protected root as bridge-owned Evidence.
//!
//! Nothing this module reports is deletion authority. S3/S4 own destructive authority and must re-probe
//! at the destructive boundary; an audit taken at time T says nothing about time T+1.
//!
//! That is not merely a scheduling caveat. Under parallel load a just-released flock was measured still
//! reading `Held` for one or more subsequent probes on a path no other process touched (observed
//! sequence: `[Held, Held, Free, Free, Free]`). Lock readings are therefore point-in-time and can lag
//! reality in BOTH directions, which is why a single `Free` here can never stand in for the recheck S3
//! must perform while it holds the operation lock itself.
//!
//! Layout follows the `containers` module (the other operator-surface CLI): the pure / FS-only cores +
//! their unit tests live here, and the container-runtime shell-out + config load live in `main.rs`'s
//! `storage_cmd`.

use bridge_core::liveness::{FsLeaseProbe, LeaseProbe};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// What a measured item IS, in the custody plan's vocabulary (§5). Cleanup rules attach to the class,
/// never to a run directory as a prefix — so the reporter must name the class of every byte it counts.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, serde::Serialize)]
pub enum PayloadClass {
    /// A standalone `.a2a-implement/<id>` quarantine clone, or a linked worktree under
    /// `[worktrees].root`. [`ReportItem::checkout_kind`] tells them apart — S4 reaps only the former.
    SourceCheckout,
    /// A cargo target directory inside a bridge-owned scope (also the `*-target` cache volumes).
    BuildTarget,
    /// A per-repo dependency cache: `node_modules`, `.venv`, and the `*cache*` volumes.
    DependencyCache,
    /// Checkpoints, receipts, review slices, logs — `.git/a2a-bridge/*` and worktree sidecars.
    Evidence,
    /// A bridge-labeled container volume (best-effort; sizes are `unknown`).
    ContainerOrImage,
    /// Inside a bridge-owned root but matching no known class. NOT a plan class: reported so a reaper
    /// has a name for "I do not know what this is", which it must refuse rather than silently sweep.
    Unclassified,
}

impl PayloadClass {
    pub fn label(self) -> &'static str {
        match self {
            Self::SourceCheckout => "SourceCheckout",
            Self::BuildTarget => "BuildTarget",
            Self::DependencyCache => "DependencyCache",
            Self::Evidence => "Evidence",
            Self::ContainerOrImage => "ContainerOrImage",
            Self::Unclassified => "Unclassified",
        }
    }
    /// Every class, in report order (totals iterate this so an absent class still prints a zero row).
    pub const ALL: [PayloadClass; 6] = [
        Self::SourceCheckout,
        Self::BuildTarget,
        Self::DependencyCache,
        Self::Evidence,
        Self::ContainerOrImage,
        Self::Unclassified,
    ];
}

/// WHAT a [`ReportItem`]'s `path` field actually NAMES. Recorded by the scanner that produced the row
/// rather than inferred by whoever consumes it: S3 had to infer "this row is a container volume, not a
/// filesystem path" from `!path.is_absolute()`, which made a destructive command's refusal rest on the
/// accident that volume names happen not to start with `/`. The scanner always knew; now it says so.
///
/// Consumers must branch on this field. The shape checks stay as defence in depth, never as the primary
/// discrimination.
// kebab-case on the wire so the JSON spelling IS `label()`'s spelling and the one the `--help` schema
// note documents. One name for one concept, in code, in the table, and in the receipt.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ItemSource {
    /// A canonical filesystem path under the `.a2a-implement` root: a run's quarantine clone or one of
    /// its nested payloads. The only source whose runs have a per-run operation lock to hold across a
    /// destructive boundary.
    ImplementPath,
    /// A canonical filesystem path under `[worktrees].root`. Its custody handle is the ADR-0025 sidecar
    /// lease, NOT the operation lock, so neither reaper's boundary applies to one — a linked worktree is
    /// also removed with `git worktree remove`, never `rm -rf`.
    WorktreePath,
    /// A container VOLUME NAME. Not a filesystem path at all: no `stat`, `canonicalize` or removal can
    /// address it, and ADR-0021/0025 owns its lifecycle.
    VolumeName,
}

impl ItemSource {
    pub fn label(self) -> &'static str {
        match self {
            Self::ImplementPath => "implement-path",
            Self::WorktreePath => "worktree-path",
            Self::VolumeName => "volume-name",
        }
    }
    /// Is `path` a filesystem path this process may `stat` and (with authority) remove?
    pub fn is_filesystem_path(self) -> bool {
        matches!(self, Self::ImplementPath | Self::WorktreePath)
    }
}

/// Which kind of checkout a `SourceCheckout` is. S4 reaps standalone clones (each carrying its own
/// duplicated `.git` object store — the plan's 13.75 GiB class); a linked worktree shares its source
/// repo's object store and is removed with `git worktree remove`, never `rm -rf`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, serde::Serialize)]
pub enum CheckoutKind {
    StandaloneClone,
    LinkedWorktree,
}

impl CheckoutKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::StandaloneClone => "clone",
            Self::LinkedWorktree => "worktree",
        }
    }
}

/// Liveness of one consumer handle. `Unknown` means NOT PROBED (absent handle, unreadable, or a kind
/// this build does not probe at all) — it is never evidence of freedom.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize)]
pub enum HolderState {
    /// A live consumer holds it.
    Held,
    /// The handle exists and nobody holds it.
    Free,
    #[default]
    Unknown,
}

impl HolderState {
    pub fn label(self) -> &'static str {
        match self {
            Self::Held => "held",
            Self::Free => "free",
            Self::Unknown => "?",
        }
    }
    pub fn probed(self) -> bool {
        self != Self::Unknown
    }
}

/// The consumer kinds, in report order, with where each one's evidence comes from.
pub const CONSUMER_KINDS: [(&str, &str); 4] = [
    (
        "run_lease",
        "worktree custody sidecars + managed-container a2a.lease labels",
    ),
    (
        "operation_lock",
        ".a2a-implement/.operation-locks/<id>.lock",
    ),
    ("container_mount", "managed-container a2a.repo mounts"),
    ("process", PROCESS_PROBE_DISCLOSURE),
];

/// Disclosed limitation: there is no process / open-file / cwd probe in S2. Building one is S3's job,
/// at the destructive boundary where it actually gates a deletion.
pub const PROCESS_PROBE_DISCLOSURE: &str = "not probed; lands with S3 at the destructive boundary";

/// Disclosed side effect: reading an flock requires taking it.
pub const LOCK_PROBE_DISCLOSURE: &str =
    "lock/lease liveness is read by TAKING an advisory flock and immediately releasing it (there is no \
     query-only flock API); a merge or resume racing that window sees a clean \"already held\" refusal \
     and can retry";

/// Disclosed limitation: volume ownership is a name-prefix guess, not a label.
pub const VOLUME_OWNERSHIP_DISCLOSURE: &str =
    "container volumes are matched by the `a2a-` name prefix, not by a label: a bridge volume named \
     otherwise is missed, and a foreign `a2a-*` volume would be claimed (label-at-creation lands with S3)";

/// Who is still consuming an item. Each field is probed independently; see [`CONSUMER_KINDS`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize)]
pub struct LiveConsumers {
    /// The ADR-0025 per-run flock lease (from a worktree sidecar or a managed container's `a2a.lease`).
    pub run_lease: HolderState,
    /// `.a2a-implement/.operation-locks/<id>.lock` — held while a resume or merge owns the run.
    pub operation_lock: HolderState,
    /// A managed container currently mounts this path (filled in by the runtime pass in `main.rs`).
    pub container_mount: HolderState,
    /// Always `Unknown` in S2 — see [`PROCESS_PROBE_DISCLOSURE`].
    pub process: HolderState,
}

impl LiveConsumers {
    pub fn states(&self) -> [HolderState; 4] {
        [
            self.run_lease,
            self.operation_lock,
            self.container_mount,
            self.process,
        ]
    }
    /// No consumer among the kinds ACTUALLY PROBED for this item: at least one kind was probed, and no
    /// probed kind reports `Held`. Unknown kinds are excluded from the judgement and disclosed
    /// separately, never silently counted as free.
    ///
    /// This is an OBSERVATION, not deletion authorization. S3/S4 own destructive authority and must
    /// re-probe at the destructive boundary — an audit taken at time T says nothing about time T+1.
    pub fn no_live_consumer_among_probed(&self) -> bool {
        let s = self.states();
        s.contains(&HolderState::Free) && !s.contains(&HolderState::Held)
    }
}

/// Git facts for a `SourceCheckout`. Every field is optional because a failed probe must read as
/// "unknown", never as a convenient default (a failed `git status` must not be misread as "clean").
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize)]
pub struct GitFacts {
    pub head: Option<String>,
    pub branch: Option<String>,
    /// A branch that exists but has no commit yet — a fact, not a probe failure.
    pub unborn: bool,
    /// `git status --porcelain` non-empty.
    pub dirty: Option<bool>,
    pub origin_url: Option<String>,
    /// `origin` resolves to a local directory. Implement clones are made with
    /// `git clone --no-hardlinks <local source path>`, so this is the normal case for them.
    pub origin_is_local_path: bool,

    /// INFORMATIONAL ONLY. Any refs of the SOURCE REPOSITORY that contain HEAD — including topic
    /// branches, remote-tracking refs, and anything else a human happened to leave lying around.
    /// This is NOT the reap gate: a commit sitting on `refs/heads/some-experiment` is not "landed".
    pub containing_refs: Vec<String>,
    /// **The D-1 gate**: is this clone's content verifiably on the SOURCE repository's main branch?
    /// `None` when the question does not apply (origin is not a local path, or HEAD is unborn).
    pub on_source_main: Option<OnSourceMain>,
    /// Which ref was treated as "main", FULLY QUALIFIED (`refs/heads/main`, `refs/heads/master`, or the
    /// source's own HEAD branch). Qualified because a bare `main` would let a TAG of that name stand in
    /// for the branch — see [`resolve_source_main`].
    pub source_main_ref: Option<String>,

    /// This checkout's own `refs/remotes/origin/*` that contain HEAD. Used only when `origin` is a
    /// real remote URL.
    pub origin_refs: Vec<String>,
    /// Reachability from local remote-tracking refs, AS OF THE LAST FETCH. No network is contacted, so
    /// this can OVERSTATE the real remote: a branch deleted or force-pushed upstream still leaves a
    /// stale `refs/remotes/origin/*` here that contains HEAD.
    pub on_origin_as_of_last_fetch: Option<bool>,

    pub probe_error: Option<String>,
}

/// The three-valued answer to D-1's "is this content on main?", plus the mandatory fourth value for a
/// probe that could not answer.
///
/// A failed probe (permission, corruption, a missing object, a git that would not run) is INADMISSIBLE
/// evidence and becomes [`OnSourceMain::Unknown`] — never `No`. Collapsing an unreadable repository to
/// "not landed" would hand S4 a deletion warrant built on a broken probe.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(tag = "verdict", rename_all = "snake_case")]
pub enum OnSourceMain {
    /// HEAD is an ancestor of the source repository's main branch.
    YesHead,
    /// HEAD's exact tree id equals the tree of a commit on source main — the squash-landing case,
    /// where the commit id differs but the content is provably identical.
    YesTree { commit: String },
    /// The source's main branch demonstrably does not carry this content.
    No,
    /// A probe failed. Nothing may be concluded; a reaper must refuse.
    Unknown { reason: String },
}

impl OnSourceMain {
    pub fn label(&self) -> String {
        match self {
            Self::YesHead => "yes(head)".into(),
            Self::YesTree { .. } => "yes(tree)".into(),
            Self::No => "no".into(),
            Self::Unknown { .. } => "unknown".into(),
        }
    }
    /// Only a positive verdict may ever support a reap. `No` means "keep and report"; `Unknown` means
    /// "keep and report"; neither is authority to delete.
    pub fn is_landed(&self) -> bool {
        matches!(self, Self::YesHead | Self::YesTree { .. })
    }
}

/// What a checkout's `.git` entry actually IS on disk, which is what decides whether it is a standalone
/// clone or a linked worktree. Never inferred from which directory the scanner happened to find it in:
/// a `git worktree` checkout can be created under `.a2a-implement`, and a clone under the worktree root,
/// and a reaper that trusted the location would apply the wrong removal mechanism.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub enum GitShape {
    /// `.git` is a real directory ⇒ standalone clone (its own object store).
    CloneDir,
    /// `.git` is a real file naming a resolvable common dir ⇒ linked worktree (shared object store).
    WorktreeFile { common_dir: String },
    /// No `.git` at all.
    Absent,
    /// Present but uninterpretable (symlinked, unreadable, a `gitdir:` that does not resolve).
    Ambiguous { reason: String },
}

/// Measured size. `logical_bytes` is the sum of FILE lengths (directory inodes are filesystem
/// bookkeeping, not payload); `disk_bytes` is allocated 512-byte blocks over every entry including
/// directories — what a reap actually returns to the filesystem. `None` = could not measure.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize)]
pub struct Measured {
    pub logical_bytes: Option<u64>,
    pub disk_bytes: Option<u64>,
    pub files: u64,
    /// Entries the walk could not stat (permission, race). Non-zero ⇒ the size is a lower bound.
    pub errors: u64,
}

/// One reported item.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct ReportItem {
    /// Canonical path, or the volume name for `ContainerOrImage`. WHICH of those it is, is
    /// [`ReportItem::source`] — never inferred from this string's shape.
    pub path: String,
    /// What `path` names (filesystem path vs volume name; implement root vs worktree root). Destructive
    /// consumers branch on this rather than guessing.
    pub source: ItemSource,
    pub class: PayloadClass,
    /// Set iff `class == SourceCheckout`.
    pub checkout_kind: Option<CheckoutKind>,
    /// The owning run id (the `.a2a-implement/<id>` / worktree directory name), when there is one.
    pub run_id: Option<String>,
    pub measured: Measured,
    pub consumers: LiveConsumers,
    pub git: Option<GitFacts>,
    pub note: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize)]
pub struct ClassTotal {
    /// The class label; `SourceCheckout` is split into `SourceCheckout (clone)` and
    /// `SourceCheckout (worktree)` because S4's authority differs between them.
    pub class: String,
    pub items: u64,
    pub logical_bytes: u64,
    pub disk_bytes: u64,
    /// Items whose size could not be measured (their bytes are NOT in the sums above).
    pub unmeasured: u64,
}

/// How many items each consumer kind was actually probed for.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize)]
pub struct ProbeCoverage {
    pub kind: String,
    pub probed: u64,
    pub total: u64,
    pub source: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize)]
pub struct DataVolume {
    pub path: String,
    pub free_bytes: Option<u64>,
    pub total_bytes: Option<u64>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize)]
pub struct StorageReport {
    pub roots: Vec<String>,
    pub items: Vec<ReportItem>,
    pub totals: Vec<ClassTotal>,
    pub probe_coverage: Vec<ProbeCoverage>,
    pub data_volume: DataVolume,
    pub notes: Vec<String>,
}

// ---------------------------------------------------------------------------------------------
// Symlink-safe filesystem predicates
// ---------------------------------------------------------------------------------------------

/// `symlink_metadata`, with symlinks rejected outright. EVERY existence/type check in this module goes
/// through here: `Path::exists`/`is_dir` follow symlinks, and a planted link inside a bridge root would
/// otherwise let the report classify, measure, and hand a reaper a D-2 protected path.
pub fn real_meta(path: &Path) -> Option<std::fs::Metadata> {
    let md = std::fs::symlink_metadata(path).ok()?;
    (!md.file_type().is_symlink()).then_some(md)
}

pub fn real_dir(path: &Path) -> bool {
    real_meta(path).map(|m| m.is_dir()).unwrap_or(false)
}

pub fn real_file(path: &Path) -> bool {
    real_meta(path).map(|m| m.is_file()).unwrap_or(false)
}

fn is_symlink(path: &Path) -> bool {
    std::fs::symlink_metadata(path)
        .map(|m| m.file_type().is_symlink())
        .unwrap_or(false)
}

// ---------------------------------------------------------------------------------------------
// Measurement
// ---------------------------------------------------------------------------------------------

/// Recursively measure `dir`, skipping paths in `exclude`. Does NOT split out nested payloads — use
/// this for a leaf payload (a target dir, an evidence dir), where everything inside belongs to it.
pub fn measure_tree(dir: &Path, exclude: &[PathBuf]) -> Measured {
    measure_inner(dir, exclude, false).0
}

/// Measure `root` while STOPPING at every nested directory that classifies as its own payload, at ANY
/// depth. Those are returned for the caller to report separately, so class totals never double-count.
///
/// Depth matters: a cargo workspace can carry `crates/*/target`, and build targets are the plan's
/// largest measured class (§1: 86.5% of recovered bytes). Classifying only immediate children would
/// silently fold those into `SourceCheckout` and understate the very thing this instrument exists to
/// measure.
pub fn measure_and_split(
    root: &Path,
    exclude: &[PathBuf],
) -> (Measured, Vec<(PathBuf, PayloadClass)>) {
    measure_inner(root, exclude, true)
}

/// READ-ONLY + symlink-safe: `symlink_metadata` throughout, so a symlink contributes its own link size
/// and is never traversed. Unreadable entries increment `errors` instead of aborting — a partial
/// measurement is reported as a lower bound, never as zero.
fn measure_inner(
    root: &Path,
    exclude: &[PathBuf],
    split: bool,
) -> (Measured, Vec<(PathBuf, PayloadClass)>) {
    let mut m = Measured::default();
    let mut nested: Vec<(PathBuf, PayloadClass)> = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    let mut any = false;
    while let Some(p) = stack.pop() {
        let Ok(md) = std::fs::symlink_metadata(&p) else {
            m.errors += 1;
            continue;
        };
        any = true;
        if md.file_type().is_symlink() {
            // Counted as its own (tiny) entry, never followed.
            m.files += 1;
            m.logical_bytes = Some(m.logical_bytes.unwrap_or(0) + md.len());
            m.disk_bytes = Some(m.disk_bytes.unwrap_or(0) + allocated_bytes(&md));
            continue;
        }
        if md.is_dir() {
            if split && p != root {
                if let Some(class) = classify_nested(&p) {
                    nested.push((p, class));
                    continue;
                }
            }
            // A directory's own inode length is filesystem bookkeeping, not payload: counting it would
            // make `logical_bytes` depend on the host filesystem's directory representation. Its
            // ALLOCATED blocks are still counted, because reaping the tree does return them.
            m.logical_bytes = Some(m.logical_bytes.unwrap_or(0));
            m.disk_bytes = Some(m.disk_bytes.unwrap_or(0) + allocated_bytes(&md));
            let Ok(rd) = std::fs::read_dir(&p) else {
                m.errors += 1;
                continue;
            };
            for e in rd {
                match e {
                    Ok(e) => {
                        let child = e.path();
                        if !exclude.contains(&child) {
                            stack.push(child);
                        }
                    }
                    Err(_) => m.errors += 1,
                }
            }
        } else {
            m.files += 1;
            m.logical_bytes = Some(m.logical_bytes.unwrap_or(0) + md.len());
            m.disk_bytes = Some(m.disk_bytes.unwrap_or(0) + allocated_bytes(&md));
        }
    }
    if !any {
        // Nothing was stat-able at all: report unknown rather than a confident zero.
        m.logical_bytes = None;
        m.disk_bytes = None;
    }
    nested.sort();
    (m, nested)
}

#[cfg(unix)]
fn allocated_bytes(md: &std::fs::Metadata) -> u64 {
    use std::os::unix::fs::MetadataExt as _;
    md.blocks().saturating_mul(512)
}

#[cfg(not(unix))]
fn allocated_bytes(md: &std::fs::Metadata) -> u64 {
    md.len()
}

/// Free / total bytes of the filesystem holding `path` (the D-4 admission floor's `df`-equivalent).
/// `None` when the syscall fails. Read-only.
#[cfg(unix)]
pub fn filesystem_space(path: &Path) -> (Option<u64>, Option<u64>) {
    let Ok(c) = std::ffi::CString::new(path.as_os_str().as_encoded_bytes()) else {
        return (None, None);
    };
    // SAFETY: `stat` is fully initialized by `statvfs` on success; we only read it when rc == 0.
    let mut stat: libc::statvfs = unsafe { std::mem::zeroed() };
    let rc = unsafe { libc::statvfs(c.as_ptr(), &mut stat) };
    if rc != 0 {
        return (None, None);
    }
    let frsize = if stat.f_frsize == 0 {
        stat.f_bsize as u64
    } else {
        stat.f_frsize as u64
    };
    (
        Some((stat.f_bavail as u64).saturating_mul(frsize)),
        Some((stat.f_blocks as u64).saturating_mul(frsize)),
    )
}

#[cfg(not(unix))]
pub fn filesystem_space(_path: &Path) -> (Option<u64>, Option<u64>) {
    (None, None)
}

// ---------------------------------------------------------------------------------------------
// Liveness probes
// ---------------------------------------------------------------------------------------------

/// Probe an flock path without creating it and without keeping it. Reuses ADR-0025's [`FsLeaseProbe`],
/// which opens an EXISTING path and releases anything it manages to acquire. A missing path reads
/// `Unknown`, not `Free`: absence proves nothing about a consumer that never wrote a lease.
///
/// See [`LOCK_PROBE_DISCLOSURE`] — this is the module's one state-changing operation.
pub fn probe_lock_path(path: &Path) -> HolderState {
    if !real_file(path) {
        return HolderState::Unknown;
    }
    match FsLeaseProbe.try_state(&path.to_string_lossy()) {
        Some(true) => HolderState::Free,
        Some(false) => HolderState::Held,
        None => HolderState::Unknown,
    }
}

/// `<implement_root>/.operation-locks/<id>.lock` — the resume/merge mutex from `implement_resume`.
/// Never creates the namespace (`acquire_operation_lock` would).
pub fn operation_lock_path(implement_root: &Path, id: &str) -> PathBuf {
    implement_root
        .join(OPERATION_LOCK_DIR)
        .join(format!("{id}.lock"))
}

pub const OPERATION_LOCK_DIR: &str = ".operation-locks";

/// `<implement root>/.receipts` — the fold-receipt namespace (plan §7). A SIBLING of the clones, not a
/// child of one: the receipt exists precisely to outlive the clone it describes, so it cannot live
/// inside it. Preserved run evidence is copied here too (plan §5: Evidence gets its own retention and
/// never dies with its parent).
pub const RECEIPTS_DIR: &str = ".receipts";

// ---------------------------------------------------------------------------------------------
// Root verification + container-mount resolution (pure seams, unit-tested)
// ---------------------------------------------------------------------------------------------

/// Verify a configured scan root before ANY enumeration: it must be a real directory (not a symlink)
/// AND resolve under its own parent. A symlinked `.a2a-implement` or `[worktrees].root` would otherwise
/// let a link redirect the whole scan onto a D-2 protected tree, and every path enumerated through it
/// would be reported as bridge-owned.
///
/// This rejects a symlink that is ALREADY in place. It does NOT make the check race-free against a
/// hostile parent directory — see the residual-race note in the body.
pub fn verify_root(path: &Path) -> Result<PathBuf, String> {
    let md = std::fs::symlink_metadata(path)
        .map_err(|e| format!("scan root {} is unreadable: {e}", path.display()))?;
    if md.file_type().is_symlink() {
        return Err(format!(
            "refusing a symlinked scan root (never enumerated through): {}",
            path.display()
        ));
    }
    if !md.is_dir() {
        return Err(format!("scan root is not a directory: {}", path.display()));
    }
    // Canonical containment: the resolved root must sit directly under the resolved parent.
    //
    // RESIDUAL RACE, NOT CLOSED: these are two separate syscalls on a path, not operations on a pinned
    // descriptor. An attacker who controls the PARENT directory can still swap the leaf between the
    // `symlink_metadata` above and the `canonicalize` below (or between this check and the walk), and
    // this function would not notice. Closing it needs `openat`/`O_NOFOLLOW` descriptor pinning with
    // every subsequent operation performed relative to the held descriptor — deferred to S3, where it
    // gates an actual deletion. Here it narrows an audit's blast radius; it does not eliminate it.
    let canon = std::fs::canonicalize(path)
        .map_err(|e| format!("scan root {} has no canonical path: {e}", path.display()))?;
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        let parent_canon = std::fs::canonicalize(parent).map_err(|e| {
            format!(
                "scan root parent {} has no canonical path: {e}",
                parent.display()
            )
        })?;
        if canon.parent() != Some(parent_canon.as_path()) {
            return Err(format!(
                "scan root {} does not resolve under its own parent (resolved to {}) — refusing",
                path.display(),
                canon.display()
            ));
        }
    }
    Ok(canon)
}

/// Container-mount evidence gathered from the runtime pass, in a form the pure resolver can judge.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MountEvidence {
    /// Canonical repo path (from a container's `a2a.repo` label) → that container's lease liveness.
    pub by_repo: BTreeMap<String, HolderState>,
    /// How many container runtimes this config declares.
    pub runtimes_configured: usize,
    /// How many of them actually answered `ps`.
    pub runtimes_answered: usize,
}

/// Decide an item's `container_mount` (and any lease evidence a matching container carries).
///
/// Returns `(mount_state, lease_from_container)`.
pub fn resolve_mount(item_path: &str, ev: &MountEvidence) -> (HolderState, Option<HolderState>) {
    // Every container covering this path, merged conservatively (a holder anywhere wins).
    let matched = ev
        .by_repo
        .iter()
        .filter(|(repo, _)| {
            item_path == repo.as_str() || item_path.starts_with(&format!("{repo}/"))
        })
        .map(|(_, l)| *l)
        .reduce(merge_holder);
    if let Some(lease) = matched {
        return (HolderState::Held, Some(lease));
    }
    // A non-match may only be reported as `Free` when the question was actually answerable:
    //   1. every CONFIGURED runtime answered — a silent runtime could be holding it; and
    //   2. the label could represent this path at all — an unrepresentable path can never match.
    let all_answered = ev.runtimes_configured > 0 && ev.runtimes_answered == ev.runtimes_configured;
    if all_answered && label_represents_path(item_path) {
        (HolderState::Free, None)
    } else {
        (HolderState::Unknown, None)
    }
}

/// Does `path` survive the container-label sanitizer intact? `a2a.repo` is written through
/// `run_identity`'s display-label hygiene (printable ASCII + space + `/`, capped at 200 chars — see
/// `crates/bridge-core/src/run_identity.rs`), so a unicode or very long path CANNOT be represented in
/// the label. For such an item a non-match is not evidence of absence.
pub fn label_represents_path(path: &str) -> bool {
    let sanitized: String = path
        .chars()
        .filter(|c| c.is_ascii_graphic() || *c == ' ' || *c == '/')
        .take(200)
        .collect();
    sanitized == path
}

/// Merge duplicate observations of the same repo across runtimes: a live holder anywhere wins, and an
/// unknown beats a free (never let one runtime's "nobody" mask another's "cannot tell").
pub fn merge_holder(a: HolderState, b: HolderState) -> HolderState {
    use HolderState::*;
    match (a, b) {
        (Held, _) | (_, Held) => Held,
        (Unknown, _) | (_, Unknown) => Unknown,
        _ => Free,
    }
}

/// What `.git` actually IS on disk. See [`GitShape`]. Never infers from location.
pub fn git_shape(dir: &Path) -> GitShape {
    let git_path = dir.join(".git");
    let Ok(md) = std::fs::symlink_metadata(&git_path) else {
        return GitShape::Absent;
    };
    if md.file_type().is_symlink() {
        return GitShape::Ambiguous {
            reason: format!("`.git` is a symlink: {}", git_path.display()),
        };
    }
    if md.is_dir() {
        return GitShape::CloneDir;
    }
    if !md.is_file() {
        return GitShape::Ambiguous {
            reason: "`.git` is neither a regular file nor a directory".into(),
        };
    }
    // A linked worktree's `.git` is a one-line `gitdir: <common dir>` pointer. It only counts as a
    // worktree if that pointer actually resolves — otherwise the shape is uninterpretable, not a clone.
    let Ok(text) = std::fs::read_to_string(&git_path) else {
        return GitShape::Ambiguous {
            reason: "`.git` file is unreadable".into(),
        };
    };
    let Some(rest) = text.trim().strip_prefix("gitdir:") else {
        return GitShape::Ambiguous {
            reason: "`.git` file carries no `gitdir:` pointer".into(),
        };
    };
    let target = rest.trim();
    let p = Path::new(target);
    let resolved = if p.is_absolute() {
        p.to_path_buf()
    } else {
        dir.join(p)
    };
    if real_dir(&resolved) {
        GitShape::WorktreeFile {
            common_dir: display_path(&resolved),
        }
    } else {
        GitShape::Ambiguous {
            reason: format!("`gitdir:` does not resolve to a directory: {target}"),
        }
    }
}

/// Classify a checkout for a root that expects `expected`. A shape that CONTRADICTS its location is
/// refused rather than reported: S4 removes a standalone clone with `rm -rf` and a linked worktree with
/// `git worktree remove`, so applying the wrong mechanism to a misplaced checkout would corrupt the
/// source repository's worktree administration.
///
/// Returns `(class, checkout_kind, note)`.
pub fn classify_checkout(
    shape: &GitShape,
    expected: CheckoutKind,
) -> (PayloadClass, Option<CheckoutKind>, Option<String>) {
    let matches = matches!(
        (shape, expected),
        (GitShape::CloneDir, CheckoutKind::StandaloneClone)
            | (GitShape::WorktreeFile { .. }, CheckoutKind::LinkedWorktree)
    );
    if matches {
        return (PayloadClass::SourceCheckout, Some(expected), None);
    }
    let note = match shape {
        GitShape::Absent => "no real `.git` — not a recognizable checkout".to_string(),
        GitShape::Ambiguous { reason } => format!("ambiguous `.git` shape ({reason})"),
        GitShape::CloneDir => "standalone-clone shape found where a linked worktree was expected \
             — refusing to classify (removal mechanisms differ)"
            .to_string(),
        GitShape::WorktreeFile { .. } => {
            "linked-worktree shape found where a standalone clone was \
             expected — refusing to classify (removal mechanisms differ)"
                .to_string()
        }
    };
    (PayloadClass::Unclassified, None, Some(note))
}

/// A checkout's `.git` identity: its shape (including the resolved `gitdir:` target for a worktree)
/// AND the filesystem identity of the entry itself. Shape alone is not enough — one `.git` directory
/// can be swapped for a DIFFERENT `.git` directory without changing shape, and the audit would then
/// report another repository's facts under this path.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShapeFingerprint {
    pub shape: GitShape,
    /// `st_dev`/`st_ino` of the `.git` entry; `None` where unavailable.
    pub dev_ino: Option<(u64, u64)>,
}

pub fn shape_fingerprint(dir: &Path) -> ShapeFingerprint {
    let git_path = dir.join(".git");
    ShapeFingerprint {
        shape: git_shape(dir),
        dev_ino: entry_identity(&git_path),
    }
}

#[cfg(unix)]
fn entry_identity(path: &Path) -> Option<(u64, u64)> {
    use std::os::unix::fs::MetadataExt as _;
    let md = std::fs::symlink_metadata(path).ok()?;
    Some((md.dev(), md.ino()))
}

#[cfg(not(unix))]
fn entry_identity(_path: &Path) -> Option<(u64, u64)> {
    None
}

/// Probe git only after RE-CONFIRMING the exact `.git` identity classification saw, and confirming it
/// AGAIN afterwards. This closes the check-then-use window in which `.git` could be swapped between the
/// classification stat and the git spawn — including a same-shape swap.
pub fn git_facts_rechecked(dir: &Path, expected: &ShapeFingerprint) -> Result<GitFacts, String> {
    let changed = |when: &str, seen: &ShapeFingerprint| {
        Err(format!(
            "`.git` changed {when} probing at {} (expected {:?}/{:?}, saw {:?}/{:?}) — refusing to \
             attribute another repository's facts to this path",
            dir.display(),
            expected.shape,
            expected.dev_ino,
            seen.shape,
            seen.dev_ino
        ))
    };
    // FULL identity, not just the variant: a `gitdir:` repointed at another worktree, or one `.git`
    // directory swapped for another, keeps the variant identical while changing which repository
    // answers every subsequent question.
    let before = shape_fingerprint(dir);
    if before != *expected {
        return changed("before", &before);
    }
    let facts = git_facts(dir);
    // Re-confirm AFTER: the swap could have landed while git was running.
    let after = shape_fingerprint(dir);
    if after != *expected {
        return changed("during", &after);
    }
    Ok(facts)
}

// ---------------------------------------------------------------------------------------------
// Git facts (read-only, no network)
// ---------------------------------------------------------------------------------------------

/// Run git in `dir` with `--no-optional-locks`, which is what makes this reporter read-only: without it
/// `git status` may refresh the index's stat cache and REWRITE `.git/index`, so the "read-only" report
/// would mutate every repository it looked at.
/// A repository under audit is UNTRUSTED INPUT — a `:rw` container wrote it. Every hardening flag here
/// closes a way for it to make the auditor act on its behalf:
/// - `--no-optional-locks`: `status` cannot refresh-and-rewrite `.git/index` (the read-only contract);
/// - `-c core.fsmonitor=false`: `status` would otherwise EXECUTE a repository-configured program;
/// - `-c core.hooksPath=/dev/null`: same, for any other hook a probe might trigger;
/// - `GIT_NO_LAZY_FETCH=1`: a promisor/partial clone cannot make `cat-file`/`rev-list` reach the network;
/// - `GIT_TERMINAL_PROMPT=0`: no probe may block waiting for credentials;
/// - `GIT_CEILING_DIRECTORIES=<parent of dir>`: git's repository DISCOVERY WALKS UP. Without a
///   ceiling, a probe aimed at a directory that is not a repository — or whose `.git` exists but is
///   not a valid one — is answered by whatever repository happens to ENCLOSE it. So
///   `git_ro(<some path>, ["rev-parse", …])` could return a commit that has nothing to do with
///   `<some path>`, and a caller asking "what does THIS repository contain?" is silently answered by
///   its parent. Every caller here means the question to be about `dir` itself.
///
///   The entry must be the PARENT, not `dir`: git computes the ceiling as the longest ceiling entry
///   that is a proper ANCESTOR of the start directory, so an entry equal to `dir` matches nothing and
///   does nothing at all (measured — an earlier form of this hardening was a silent no-op, and the
///   mutation round that should have caught it stayed green until a test forced the distinction).
///   With the parent set, `dir` itself is still examined and the ascent above it stops.
pub fn git_ro(dir: &Path, argv: &[&str]) -> std::io::Result<std::process::Output> {
    let canonical = std::fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf());
    let mut cmd = std::process::Command::new("git");
    cmd.arg("--no-optional-locks")
        .args(["-c", "core.fsmonitor=false"])
        .args(["-c", "core.hooksPath=/dev/null"])
        .arg("-C")
        .arg(dir)
        .args(argv)
        .env("GIT_NO_LAZY_FETCH", "1")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_OPTIONAL_LOCKS", "0");
    // No parent means `dir` is the filesystem root: there is nothing above it to ascend into.
    if let Some(parent) = canonical.parent() {
        cmd.env("GIT_CEILING_DIRECTORIES", parent);
    }
    cmd.output()
}

pub fn git_str(dir: &Path, argv: &[&str]) -> Result<String, String> {
    let out = git_ro(dir, argv).map_err(|e| format!("git {}: {e}", argv.join(" ")))?;
    if !out.status.success() {
        return Err(format!(
            "git {}: {}",
            argv.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// **S4's "content verifiably on main" query, in read-only form**: which refs of `repo` currently
/// contain `sha`. This is the check the clone reaper (plan §3 S4, enabled by D-1) must pass before
/// deleting a quarantine clone — asked of the SOURCE repository's LIVE refs, never of the clone's own
/// frozen `refs/remotes/origin/*` snapshot.
///
/// `Ok(vec![])` means the source genuinely does not contain the commit (including the common case where
/// the object is absent entirely); `Err` means the probe itself failed and nothing may be concluded.
/// No network: local refs only.
pub fn refs_containing(repo: &Path, sha: &str) -> Result<Vec<String>, String> {
    // An absent object is a definite "not contained", not a probe failure — and asking `for-each-ref`
    // about an unknown sha errors out, which would otherwise read as "unknown".
    let spec = format!("{sha}^{{commit}}");
    match git_ro(repo, &["cat-file", "-e", &spec]) {
        Ok(out) if !out.status.success() => return Ok(Vec::new()),
        Err(e) => return Err(format!("git cat-file: {e}")),
        Ok(_) => {}
    }
    let out = git_str(
        repo,
        &[
            "for-each-ref",
            "--format=%(refname:short)",
            "--contains",
            sha,
            "refs/heads/",
            "refs/remotes/",
        ],
    )?;
    Ok(out
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .collect())
}

/// A `remote.origin.url` that names a local directory, resolved against `dir` when relative. Implement
/// clones are created with `git clone --no-hardlinks <local path>`, so this is their normal shape.
///
/// `None` for a hosted remote (ssh/https/scp-style) — which S4 treats as a REFUSAL, not a fallback: the
/// containment query has to be asked of a local source repository's live refs, and there is none.
pub fn local_source_path(dir: &Path, url: &str) -> Option<PathBuf> {
    let raw = url.strip_prefix("file://").unwrap_or(url);
    if raw.is_empty() || raw.contains("://") || raw.contains('@') {
        return None; // ssh/https/git remote
    }
    let p = Path::new(raw);
    let candidate = if p.is_absolute() {
        p.to_path_buf()
    } else {
        dir.join(p)
    };
    real_dir(&candidate).then(|| std::fs::canonicalize(&candidate).unwrap_or(candidate))
}

/// HEAD, branch, dirty/clean, and containment — asked of the source repo for a local-origin clone
/// (`on_source`), or of local remote-tracking refs otherwise (`on_origin_as_of_last_fetch`).
/// No network is ever contacted.
pub fn git_facts(dir: &Path) -> GitFacts {
    // Branch FIRST: `symbolic-ref` resolves on an unborn HEAD, which `rev-parse HEAD` cannot.
    let mut f = GitFacts {
        branch: git_str(dir, &["symbolic-ref", "--quiet", "--short", "HEAD"]).ok(),
        ..Default::default()
    };
    match git_str(dir, &["rev-parse", "HEAD"]) {
        Ok(h) => f.head = Some(h),
        Err(e) => {
            if f.branch.is_some() {
                f.unborn = true; // a branch with no commit yet — a fact, not a probe failure
            } else {
                f.probe_error = Some(e);
                return f;
            }
        }
    }
    match git_ro(dir, &["status", "--porcelain"]) {
        Ok(out) if out.status.success() => {
            f.dirty = Some(!String::from_utf8_lossy(&out.stdout).trim().is_empty());
        }
        Ok(out) => push_err(
            &mut f,
            format!(
                "git status: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            ),
        ),
        Err(e) => push_err(&mut f, format!("git status: {e}")),
    }

    f.origin_url = git_str(dir, &["config", "--get", "remote.origin.url"]).ok();
    let source = f
        .origin_url
        .as_deref()
        .and_then(|u| local_source_path(dir, u));
    f.origin_is_local_path = source.is_some();

    let Some(head) = f.head.clone() else {
        return f; // unborn: nothing to ask about
    };
    match source {
        // A local `origin` means `refs/remotes/origin/*` here is a FROZEN snapshot taken at clone time.
        // Ask the source repository what it actually contains today.
        Some(src) => {
            // Informational only: every ref that happens to contain HEAD, including topic branches.
            if let Ok(refs) = refs_containing(&src, &head) {
                f.containing_refs = refs;
            }
            let (main_ref, verdict) = on_source_main(&src, dir, &head);
            f.source_main_ref = main_ref;
            f.on_source_main = Some(verdict);
        }
        None => match refs_containing_locally(dir, &head, "refs/remotes/origin/") {
            Ok(refs) => {
                f.on_origin_as_of_last_fetch = Some(!refs.is_empty());
                f.origin_refs = refs;
            }
            Err(e) => push_err(&mut f, format!("origin reachability: {e}")),
        },
    }
    f
}

/// How far back along source main the exact-tree search looks. Bounded so an audit over 112 clones
/// cannot turn into a full-history walk per clone.
pub const SOURCE_MAIN_LOOKBACK: u32 = 500;

/// Is an object present in `repo`? `Ok(false)` = definitively absent (the ordinary unpushed case);
/// `Err` = the probe itself failed and nothing may be concluded.
/// `rev-parse --verify --quiet`, NOT `cat-file -e <sha>^{commit}`: peeling a missing object exits 128
/// with "Not a valid object name", which is indistinguishable from a genuinely broken repository. The
/// `--quiet` form exits 1 for "cannot resolve" and reserves 128 for real failures, which is exactly the
/// distinction this function has to make.
pub fn object_present(repo: &Path, sha: &str) -> Result<bool, String> {
    let spec = format!("{sha}^{{commit}}");
    let out = git_ro(repo, &["rev-parse", "--verify", "--quiet", &spec])
        .map_err(|e| format!("rev-parse could not run: {e}"))?;
    match out.status.code() {
        Some(0) => Ok(true),
        Some(1) => Ok(false),
        _ => Err(format!(
            "rev-parse failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )),
    }
}

/// Resolve which BRANCH of `src` is "main", as a FULLY QUALIFIED ref: `refs/heads/main`, else
/// `refs/heads/master`, else the source's own HEAD branch.
///
/// Qualified deliberately. `git rev-parse main` follows the gitrevisions precedence order, which tries
/// `refs/tags/main` BEFORE `refs/heads/main` — so a stray tag named `main` (or `master`) silently
/// becomes the history the D-1 gate searches, and a commit reachable only from that tag reads as
/// landed. `refs/heads/<name>^{commit}` can only ever name a branch, and peeling to `^{commit}` also
/// refuses a branch name that somehow resolves to a non-commit.
///
/// The returned ref is what the caller must record and re-check: it is the identity of the history the
/// verdict was computed against.
pub fn resolve_source_main(src: &Path) -> Result<String, String> {
    for cand in ["refs/heads/main", "refs/heads/master"] {
        let spec = format!("{cand}^{{commit}}");
        let out = git_ro(src, &["rev-parse", "--verify", "--quiet", &spec])
            .map_err(|e| format!("rev-parse could not run: {e}"))?;
        if out.status.success() {
            return Ok(cand.to_string());
        }
    }
    // `symbolic-ref HEAD` (NOT `--short`) yields the fully-qualified `refs/heads/<name>`.
    let head_ref = git_str(src, &["symbolic-ref", "--quiet", "HEAD"])
        .map_err(|e| format!("no `refs/heads/main`/`master`, and HEAD is unresolvable: {e}"))?;
    if !head_ref.starts_with("refs/heads/") {
        return Err(format!(
            "source HEAD resolves to {head_ref:?}, which is not a branch under `refs/heads/`"
        ));
    }
    let spec = format!("{head_ref}^{{commit}}");
    let out = git_ro(src, &["rev-parse", "--verify", "--quiet", &spec])
        .map_err(|e| format!("rev-parse could not run: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "source HEAD branch {head_ref} has no commit (unborn) — there is no main history to search"
        ));
    }
    Ok(head_ref)
}

/// The current OID of a fully-qualified ref, for the boundary's before/after consistency check: a
/// verdict computed while source main moved under the probes is a verdict about no single history.
pub fn ref_oid(repo: &Path, full_ref: &str) -> Result<String, String> {
    git_str(
        repo,
        &["rev-parse", "--verify", &format!("{full_ref}^{{commit}}")],
    )
}

/// **The D-1 gate**: is `head` verifiably on `src`'s main branch?
///
/// Two admissible positives, in order:
/// 1. `YesHead` — `head` is an ancestor of source main (fast-forward / merge landing).
/// 2. `YesTree` — `head`'s exact tree id equals the tree of a commit within the last
///    [`SOURCE_MAIN_LOOKBACK`] commits of source main. This is the squash landing: a different commit
///    id carrying byte-identical content.
///
/// Everything else is `No` — INCLUDING a squash that rewrote the tree (a conflict resolution, a
/// rebase-with-fixups). That is deliberately fail-closed: `No` keeps the clone, and a retained clone
/// costs disk while a wrongly-reaped one costs work. It is also why `No` must never be produced by a
/// failed probe: see [`OnSourceMain::Unknown`].
///
/// Returns `(main_ref, verdict)`.
pub fn on_source_main(src: &Path, clone: &Path, head: &str) -> (Option<String>, OnSourceMain) {
    on_source_main_with_lookback(src, clone, head, SOURCE_MAIN_LOOKBACK)
}

/// [`on_source_main`] with an explicit window, so the lookback-exhaustion boundary is testable without
/// building a 500-commit fixture.
pub fn on_source_main_with_lookback(
    src: &Path,
    clone: &Path,
    head: &str,
    lookback: u32,
) -> (Option<String>, OnSourceMain) {
    let main_ref = match resolve_source_main(src) {
        Ok(r) => r,
        Err(reason) => {
            return (
                None,
                OnSourceMain::Unknown {
                    reason: format!("source main unresolvable: {reason}"),
                },
            )
        }
    };
    let unknown = |reason: String| (Some(main_ref.clone()), OnSourceMain::Unknown { reason });

    // (1) head-reachability.
    match git_ro(src, &["merge-base", "--is-ancestor", head, &main_ref]) {
        Err(e) => return unknown(format!("merge-base could not run: {e}")),
        Ok(out) => match out.status.code() {
            Some(0) => return (Some(main_ref), OnSourceMain::YesHead),
            Some(1) => {} // not an ancestor — fall through to the tree check
            _ => {
                // Non-zero beyond 0/1 usually means the object is simply not in this repository (the
                // ordinary unpushed case). Distinguish that from a genuinely broken probe rather than
                // letting either one silently become a verdict.
                match object_present(src, head) {
                    Ok(false) => {}
                    Ok(true) => {
                        return unknown(format!(
                            "merge-base failed on a present object: {}",
                            String::from_utf8_lossy(&out.stderr).trim()
                        ))
                    }
                    Err(e) => return unknown(e),
                }
            }
        },
    }

    // (2) exact-tree equivalence against source main's recent history.
    let tree = match git_str(clone, &["rev-parse", &format!("{head}^{{tree}}")]) {
        Ok(t) => t,
        Err(e) => return unknown(format!("HEAD tree unresolvable in the clone: {e}")),
    };
    // Ask for ONE more row than the window. That extra row is a sentinel: if it exists, main's history
    // is longer than we searched, so "no match" means the search was INCOMPLETE — not that the content
    // was never landed. Only an exhausted HISTORY supports a definite `no`.
    let requested = lookback.saturating_add(1);
    let log = match git_str(
        src,
        &[
            "log",
            &format!("--max-count={requested}"),
            "--format=%H %T",
            &main_ref,
        ],
    ) {
        Ok(l) => l,
        Err(e) => return unknown(format!("source main history unreadable: {e}")),
    };
    let mut rows: u32 = 0;
    for line in log.lines() {
        rows = rows.saturating_add(1);
        if rows > lookback {
            break; // the sentinel row is evidence of more history, not a candidate
        }
        if let Some((commit, main_tree)) = line.split_once(' ') {
            if main_tree.trim() == tree {
                return (
                    Some(main_ref),
                    OnSourceMain::YesTree {
                        commit: commit.trim().to_string(),
                    },
                );
            }
        }
    }
    if rows > lookback {
        return unknown(format!(
            "exact-tree search reached the {lookback}-commit lookback with more history beyond it — \
             the search was INCOMPLETE (lookback exhausted), so absence of a match proves nothing"
        ));
    }
    (Some(main_ref), OnSourceMain::No)
}

/// One container runtime's `ps` result, after parsing.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RuntimePsOutcome {
    /// The runtime produced a COMPLETE, fully-parsed answer. A zero exit is not enough: output we
    /// could not parse means we do not know what that runtime is running.
    pub answered: bool,
    /// `(canonical repo path, lease liveness)` per parsed record.
    pub records: Vec<(String, HolderState)>,
    pub malformed_lines: usize,
}

/// Parse one runtime's `ps` output into an outcome. `stdout` is `None` when the runtime did not answer
/// at all (spawn failure or non-zero exit).
pub fn ps_outcome<F>(
    runtime: &str,
    stdout: Option<&str>,
    mut parse: F,
    notes: &mut Vec<String>,
) -> RuntimePsOutcome
where
    F: FnMut(&str) -> Option<(String, HolderState)>,
{
    let Some(stdout) = stdout else {
        notes.push(format!(
            "container runtime `{runtime}` did not answer `ps` — container-mount consumers are unknown"
        ));
        return RuntimePsOutcome::default();
    };
    let mut out = RuntimePsOutcome {
        answered: true,
        ..Default::default()
    };
    for line in stdout.lines() {
        if line.trim().is_empty() {
            continue;
        }
        match parse(line) {
            Some(rec) => out.records.push(rec),
            None => out.malformed_lines += 1,
        }
    }
    // A zero exit is not an answer. Output we could not parse means we do not know what this runtime is
    // running, so it must not license a `free` reading for anything.
    if out.malformed_lines > 0 {
        out.answered = false;
        notes.push(format!(
            "container runtime `{runtime}` returned {} unparseable `ps` line(s) — its answer is treated \
             as INCOMPLETE and container-mount consumers stay unknown",
            out.malformed_lines
        ));
    }
    out
}

fn refs_containing_locally(dir: &Path, sha: &str, glob: &str) -> Result<Vec<String>, String> {
    let out = git_str(
        dir,
        &[
            "for-each-ref",
            "--format=%(refname:short)",
            "--contains",
            sha,
            glob,
        ],
    )?;
    Ok(out
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .collect())
}

fn push_err(f: &mut GitFacts, msg: String) {
    f.probe_error = Some(match f.probe_error.take() {
        Some(prev) => format!("{prev}; {msg}"),
        None => msg,
    });
}

// ---------------------------------------------------------------------------------------------
// Classification
// ---------------------------------------------------------------------------------------------

/// PURE-ish (FS reads only). Class of a nested directory inside a bridge-owned checkout, by name AND
/// on-disk evidence. `None` = not a separately-reported payload (it stays counted inside its parent).
pub fn classify_nested(dir: &Path) -> Option<PayloadClass> {
    let name = dir.file_name()?.to_str()?;
    match name {
        "target" if is_cargo_target(dir) => Some(PayloadClass::BuildTarget),
        "node_modules" | ".venv" => Some(PayloadClass::DependencyCache),
        _ => None,
    }
}

/// Is this `target/` really a cargo build directory? Requires the directory NAME (checked by the
/// caller) plus real cargo evidence:
///
/// - `.rustc_info.json` is cargo's own artifact — proof on its own;
/// - `CACHEDIR.TAG` is a GENERIC cache marker that any tool may write, so per plan §5 ("evidence, not
///   proof") it only counts alongside a cargo profile directory.
///
/// A `target/` holding a user's own data is therefore left unclassified rather than promoted into a
/// reapable class.
pub fn is_cargo_target(dir: &Path) -> bool {
    if real_file(&dir.join(".rustc_info.json")) {
        return true;
    }
    real_file(&dir.join("CACHEDIR.TAG"))
        && (real_dir(&dir.join("debug")) || real_dir(&dir.join("release")))
}

/// PURE. Class of a bridge-labeled container volume, by name. `None` = not bridge-owned.
///
/// Names are `<base>-<16 hex>` (see `verify::cache_volume_name`), and the base itself may carry a
/// language segment — `a2a-impl-lsp-cache-go`, `-py`, `-ts` are all shipped in
/// `examples/a2a-bridge.containerized.toml`. So the hash is stripped first and then a `cache`/`target`
/// segment is looked for ANYWHERE in the remainder, not just at the end.
///
/// Ownership is a name-prefix guess — see [`VOLUME_OWNERSHIP_DISCLOSURE`].
pub fn classify_volume(name: &str) -> Option<PayloadClass> {
    if !name.starts_with("a2a-") {
        return None;
    }
    let base = match name.rsplit_once('-') {
        Some((head, tail)) if is_volume_hash(tail) => head,
        _ => name,
    };
    let mut segments = base.split('-');
    if segments.any(|s| s == "cache") {
        Some(PayloadClass::DependencyCache)
    } else if base.split('-').any(|s| s == "target") {
        Some(PayloadClass::BuildTarget)
    } else {
        Some(PayloadClass::ContainerOrImage)
    }
}

/// The `{:016x}` suffix `verify::cache_volume_name` appends.
fn is_volume_hash(s: &str) -> bool {
    s.len() == 16
        && s.chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_uppercase())
}

/// The evidence sidecar inside a quarantine clone (`<clone>/.git/a2a-bridge`): the implement
/// checkpoint, review slices, and (per plan §7) the fold receipts. Never treated as cache.
pub fn evidence_dir(checkout: &Path) -> PathBuf {
    checkout.join(".git").join("a2a-bridge")
}

// ---------------------------------------------------------------------------------------------
// Scanning
// ---------------------------------------------------------------------------------------------

/// Collect the nested payload items discovered under a checkout.
fn nested_items(
    nested: Vec<(PathBuf, PayloadClass)>,
    run_id: &str,
    consumers: LiveConsumers,
    source: ItemSource,
) -> Vec<ReportItem> {
    nested
        .into_iter()
        .map(|(path, class)| ReportItem {
            path: display_path(&path),
            source,
            class,
            checkout_kind: None,
            run_id: Some(run_id.to_string()),
            measured: measure_tree(&path, &[]),
            consumers,
            git: None,
            note: None,
        })
        .collect()
}

/// Scan one `.a2a-implement` root. FS-only: no container runtime, no network, nothing written beyond
/// the declared flock probe.
///
/// Emits, per `<root>/<id>`: the `SourceCheckout` itself (bytes EXCLUDING its separately-reported
/// children, so class totals never double-count), its `Evidence` sidecar, and every nested
/// `BuildTarget`/`DependencyCache` at any depth. Symlinks are refused with a note at every level —
/// following one would let a link inside a bridge root pull a protected checkout into a reaper's view.
pub fn scan_implement_root(root: &Path, notes: &mut Vec<String>) -> Vec<ReportItem> {
    let mut items = Vec::new();
    let Ok(rd) = std::fs::read_dir(root) else {
        notes.push(format!("implement root unreadable: {}", root.display()));
        return items;
    };
    let mut entries: Vec<PathBuf> = rd.flatten().map(|e| e.path()).collect();
    entries.sort();
    for entry in entries {
        let Some(name) = entry
            .file_name()
            .and_then(|n| n.to_str())
            .map(str::to_string)
        else {
            // Disclosed limitation: names are handled as UTF-8. The skip is REPORTED so a reaper never
            // believes the enumeration was complete.
            notes.push(format!(
                "entry with a non-UTF-8 name skipped, NOT classified: {}",
                entry.to_string_lossy()
            ));
            continue;
        };
        let Ok(md) = std::fs::symlink_metadata(&entry) else {
            notes.push(format!("unreadable entry skipped: {}", entry.display()));
            continue;
        };
        if md.file_type().is_symlink() {
            notes.push(format!(
                "symlink skipped (never followed out of a bridge root): {}",
                entry.display()
            ));
            continue;
        }
        if name == OPERATION_LOCK_DIR {
            continue; // the lock namespace itself is a probe target, not a payload
        }
        // The fold-receipt namespace (plan §7) is a SIBLING of the clones so it outlives them. It is
        // Evidence, with its own retention: reported so its bytes are counted and named, never
        // classified as a checkout or swept as cache.
        if name == RECEIPTS_DIR {
            items.push(ReportItem {
                path: display_path(&entry),
                source: ItemSource::ImplementPath,
                class: PayloadClass::Evidence,
                checkout_kind: None,
                run_id: None,
                measured: measure_tree(&entry, &[]),
                consumers: LiveConsumers::default(),
                git: None,
                note: Some(
                    "clone fold receipts + preserved run evidence (plan §7) — Evidence class, never \
                     auto-deleted with the run it describes"
                        .into(),
                ),
            });
            continue;
        }
        let op_lock = probe_lock_path(&operation_lock_path(root, &name));
        if !md.is_dir() {
            items.push(ReportItem {
                path: display_path(&entry),
                source: ItemSource::ImplementPath,
                class: PayloadClass::Unclassified,
                checkout_kind: None,
                run_id: None,
                measured: measure_tree(&entry, &[]),
                consumers: LiveConsumers::default(),
                git: None,
                note: Some("stray file under the implement root".into()),
            });
            continue;
        }

        // Classify by what `.git` IS, not by which root we happened to be walking. This root holds
        // standalone clones; a linked-worktree or ambiguous shape here is refused, not relabelled.
        let git_path = entry.join(".git");
        if is_symlink(&git_path) {
            notes.push(format!(
                "symlinked .git refused (never followed): {}",
                git_path.display()
            ));
        }
        let shape = shape_fingerprint(&entry);
        let (class, checkout_kind, mut note) =
            classify_checkout(&shape.shape, CheckoutKind::StandaloneClone);
        let is_checkout = class == PayloadClass::SourceCheckout;

        // Same guard for the evidence sidecar.
        let ev = evidence_dir(&entry);
        if is_checkout && is_symlink(&ev) {
            notes.push(format!(
                "symlinked evidence dir refused (never followed): {}",
                ev.display()
            ));
        }
        let has_evidence = is_checkout && real_dir(&ev);

        let consumers = LiveConsumers {
            operation_lock: op_lock,
            ..Default::default()
        };
        let excluded: Vec<PathBuf> = has_evidence.then(|| ev.clone()).into_iter().collect();
        let (measured, nested) = measure_and_split(&entry, &excluded);

        // Re-confirm the shape immediately before spawning git (check-then-use).
        let git = if is_checkout {
            match git_facts_rechecked(&entry, &shape) {
                Ok(f) => Some(f),
                Err(e) => {
                    notes.push(e.clone());
                    note = Some(e);
                    None
                }
            }
        } else {
            None
        };

        items.push(ReportItem {
            path: display_path(&entry),
            source: ItemSource::ImplementPath,
            class,
            checkout_kind,
            run_id: Some(name.clone()),
            measured,
            consumers,
            git,
            note,
        });
        if has_evidence {
            items.push(ReportItem {
                path: display_path(&ev),
                source: ItemSource::ImplementPath,
                class: PayloadClass::Evidence,
                checkout_kind: None,
                run_id: Some(name.clone()),
                measured: measure_tree(&ev, &[]),
                consumers,
                git: None,
                note: None,
            });
        }
        items.extend(nested_items(
            nested,
            &name,
            consumers,
            ItemSource::ImplementPath,
        ));
    }
    items
}

/// The two record suffixes a worktree root can carry, and what each says about its sibling.
///
/// Added in R2f1b slice 2b2 because V3 publishes `<name>.custody.v1.json` INSTEAD OF
/// `<name>.meta.json`: a report that knew only the legacy suffix would show every V3 record and
/// every V3 checkout's custody evidence as `Unclassified` — bridge-owned bytes presented as
/// garbage, which is exactly what risk R-4 named.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WorktreeRecordKindV1 {
    /// `<name>.meta.json` — the V2 sidecar. Its `lease` field is a run-lease handle.
    LegacySidecar,
    /// `<name>.custody.v1.json` — the V3 custody record. Its holder state is derived from the
    /// custody STATE, not from a lease: V3 protection is the record itself (§5.2), and a live
    /// custody state is held by definition while a terminal one is awaiting R2f2 disposition.
    CustodyRecord,
    /// `<name>.custody.v1.json.staging-<hex>` — a quarantined staged publication (2b2's residue
    /// policy). Recovery-owned, never a checkout.
    CustodyStagingResidue,
}

impl WorktreeRecordKindV1 {
    fn classify(name: &str) -> Option<Self> {
        if name.ends_with(".meta.json") {
            Some(Self::LegacySidecar)
        } else if bridge_worktree::custody::is_custody_record_name(name) {
            Some(Self::CustodyRecord)
        } else if bridge_worktree::custody_writer::is_staged_custody_residue(name) {
            Some(Self::CustodyStagingResidue)
        } else {
            None
        }
    }

    fn note(self) -> &'static str {
        match self {
            Self::LegacySidecar => "worktree custody sidecar",
            Self::CustodyRecord => "R2f1b worktree custody record",
            Self::CustodyStagingResidue => {
                "R2f1b staged custody publication (quarantined; recovery-owned)"
            }
        }
    }
}

/// Record one holder answer for a path, combining it with any answer already there through the
/// module's existing [`merge_holder`] lattice (`Held` dominates; `Unknown` beats `Free`).
///
/// A checkout can be named by TWO records at once — 2b1's deletion gate manufactures exactly that
/// state, retaining the legacy sidecar beside a custody record — and the two are probed from
/// different evidence: the sidecar from its run lease, the custody record from its custody state.
/// Plain insertion made the answer depend on FILENAME SORT ORDER, and `.custody.v1.json` sorts
/// before `.meta.json`, so a checkout held by a live custody record was reported free the moment a
/// stale sidecar sat beside it. Reusing `merge_holder` rather than inventing a second rule also
/// keeps "one runtime's 'nobody' must not mask another's 'cannot tell'" true here (repair R8a).
fn record_holder(map: &mut BTreeMap<String, HolderState>, key: String, state: HolderState) {
    let merged = match map.get(&key).copied() {
        Some(existing) => merge_holder(existing, state),
        None => state,
    };
    map.insert(key, merged);
}

/// Holder state for a V3 custody record, derived from its own custody state.
///
/// `Held` for every state whose checkout is live or protective, because the record IS the
/// protection and no lease can be consulted for it. `Free` is never produced: no custody state
/// this slice can publish releases the checkout, and answering "free" for one would invite a
/// reaper to treat protected work as reclaimable.
fn custody_record_holder_state(path: &Path) -> HolderState {
    let Ok(bytes) = std::fs::read(path) else {
        return HolderState::Unknown;
    };
    match bridge_worktree::custody::WorktreeCustodyRecordV1::decode_canonical(&bytes) {
        Ok(record) => match record.sweep_disposition() {
            bridge_worktree::custody::CustodySweepDispositionV1::Recover
            | bridge_worktree::custody::CustodySweepDispositionV1::Preserved => HolderState::Held,
            // Marker-only and refused records name no protected checkout of their own, and an
            // unknown one is exactly that.
            _ => HolderState::Unknown,
        },
        Err(_) => HolderState::Unknown,
    }
}

/// Scan one `[worktrees].root`. Each `<name>.meta.json` sidecar is Evidence and binds its sibling
/// worktree's run lease; each sibling directory is a `SourceCheckout` of kind `LinkedWorktree`.
///
/// R2f1b slice 2b2 adds the second suffix: `<name>.custody.v1.json` is Evidence too, associates
/// with the same sibling, and takes its holder state from the custody state. V2 output is
/// unchanged — a root with no V3 records produces byte-identical items.
pub fn scan_worktree_root(root: &Path, notes: &mut Vec<String>) -> Vec<ReportItem> {
    let mut items = Vec::new();
    let Ok(rd) = std::fs::read_dir(root) else {
        notes.push(format!("worktree root unreadable: {}", root.display()));
        return items;
    };
    let mut entries: Vec<PathBuf> = rd.flatten().map(|e| e.path()).collect();
    entries.sort();
    // Sidecar first: its `lease` field is the run-lease handle for the sibling worktree. The state is
    // recorded against BOTH the worktree it names and the sidecar file itself — a sidecar is the custody
    // record of a live run, and reporting it with no consumer would present live evidence as garbage.
    let mut leases: BTreeMap<String, HolderState> = BTreeMap::new();
    let mut sidecar_leases: BTreeMap<String, HolderState> = BTreeMap::new();
    for entry in &entries {
        let s = entry.to_string_lossy();
        if is_symlink(entry) {
            continue;
        }
        match WorktreeRecordKindV1::classify(&s) {
            Some(WorktreeRecordKindV1::LegacySidecar) => {
                if let Some(sidecar) = bridge_worktree::provider_path::read_sidecar(&s) {
                    let state = probe_lock_path(Path::new(&sidecar.lease));
                    record_holder(&mut leases, sidecar.worktree_path.clone(), state);
                    record_holder(&mut sidecar_leases, display_path(entry), state);
                }
            }
            Some(WorktreeRecordKindV1::CustodyRecord) => {
                // Sibling association by NAME, the same rule the sweep uses: strip the suffix.
                let state = custody_record_holder_state(entry);
                if let Some(sibling) =
                    s.strip_suffix(bridge_worktree::custody::CUSTODY_RECORD_SUFFIX)
                {
                    record_holder(&mut leases, display_path(Path::new(sibling)), state);
                }
                record_holder(&mut sidecar_leases, display_path(entry), state);
            }
            Some(WorktreeRecordKindV1::CustodyStagingResidue) | None => {}
        }
    }
    for entry in entries {
        let Some(name) = entry
            .file_name()
            .and_then(|n| n.to_str())
            .map(str::to_string)
        else {
            notes.push(format!(
                "entry with a non-UTF-8 name skipped, NOT classified: {}",
                entry.to_string_lossy()
            ));
            continue;
        };
        let Ok(md) = std::fs::symlink_metadata(&entry) else {
            notes.push(format!("unreadable entry skipped: {}", entry.display()));
            continue;
        };
        if md.file_type().is_symlink() {
            notes.push(format!(
                "symlink skipped (never followed out of a bridge root): {}",
                entry.display()
            ));
            continue;
        }
        if !md.is_dir() {
            let record = WorktreeRecordKindV1::classify(&name);
            let canonical = display_path(&entry);
            let consumers = LiveConsumers {
                run_lease: sidecar_leases
                    .get(&canonical)
                    .copied()
                    .unwrap_or(HolderState::Unknown),
                ..Default::default()
            };
            items.push(ReportItem {
                path: canonical,
                source: ItemSource::WorktreePath,
                class: if record.is_some() {
                    PayloadClass::Evidence
                } else {
                    PayloadClass::Unclassified
                },
                checkout_kind: None,
                run_id: None,
                measured: measure_tree(&entry, &[]),
                consumers,
                git: None,
                note: record.map(|record| record.note().to_string()),
            });
            continue;
        }

        // The R2f1b custody LOCK directory (`<root>/.custody-locks`). It is bridge-owned
        // coordination state, not a checkout: without this arm it lands in the directory branch
        // below, fails the linked-worktree shape check, and surfaces as an Unclassified item on
        // every report (2b1 dual review, opus S-10 — report noise, never a deletion hazard).
        if name == bridge_worktree::custody_lock::CUSTODY_LOCK_DIR_NAME {
            items.push(ReportItem {
                path: display_path(&entry),
                source: ItemSource::WorktreePath,
                class: PayloadClass::Evidence,
                checkout_kind: None,
                run_id: None,
                measured: measure_tree(&entry, &[]),
                consumers: LiveConsumers::default(),
                git: None,
                note: Some("R2f1b custody lock cells (coordination state, not a checkout)".into()),
            });
            continue;
        }

        let git_path = entry.join(".git");
        if is_symlink(&git_path) {
            notes.push(format!(
                "symlinked .git refused (never followed): {}",
                git_path.display()
            ));
        }
        // This root holds LINKED WORKTREES. A standalone-clone shape here is refused, not relabelled:
        // `rm -rf` and `git worktree remove` are not interchangeable.
        let shape = shape_fingerprint(&entry);
        let (class, checkout_kind, mut note) =
            classify_checkout(&shape.shape, CheckoutKind::LinkedWorktree);
        let is_checkout = class == PayloadClass::SourceCheckout;
        let canonical = display_path(&entry);
        let consumers = LiveConsumers {
            run_lease: leases
                .get(&canonical)
                .copied()
                .unwrap_or(HolderState::Unknown),
            ..Default::default()
        };
        let (measured, nested) = measure_and_split(&entry, &[]);
        let git = if is_checkout {
            match git_facts_rechecked(&entry, &shape) {
                Ok(f) => Some(f),
                Err(e) => {
                    notes.push(e.clone());
                    note = Some(e);
                    None
                }
            }
        } else {
            None
        };
        items.push(ReportItem {
            path: canonical,
            source: ItemSource::WorktreePath,
            class,
            checkout_kind,
            run_id: Some(name.clone()),
            measured,
            consumers,
            git,
            note,
        });
        items.extend(nested_items(
            nested,
            &name,
            consumers,
            ItemSource::WorktreePath,
        ));
    }
    items
}

/// Canonical display path; falls back to the lexical path when the target vanished mid-scan.
pub fn display_path(p: &Path) -> String {
    std::fs::canonicalize(p)
        .unwrap_or_else(|_| p.to_path_buf())
        .to_string_lossy()
        .into_owned()
}

// ---------------------------------------------------------------------------------------------
// Aggregation
// ---------------------------------------------------------------------------------------------

/// PURE. Per-class totals. `SourceCheckout` is split by [`CheckoutKind`] because S4's authority differs:
/// a standalone clone is `rm -rf`-able under D-1, a linked worktree is not. Unmeasured items are
/// counted separately rather than folded in as zero, so a total is never quietly understated.
pub fn totals(items: &[ReportItem]) -> Vec<ClassTotal> {
    let mut out = Vec::new();
    for class in PayloadClass::ALL {
        if class == PayloadClass::SourceCheckout {
            for kind in [CheckoutKind::StandaloneClone, CheckoutKind::LinkedWorktree] {
                out.push(tally(
                    format!("SourceCheckout ({})", kind.label()),
                    items
                        .iter()
                        .filter(|i| i.class == class && i.checkout_kind == Some(kind)),
                ));
            }
            continue;
        }
        out.push(tally(
            class.label().to_string(),
            items.iter().filter(|i| i.class == class),
        ));
    }
    out
}

fn tally<'a>(class: String, items: impl Iterator<Item = &'a ReportItem>) -> ClassTotal {
    let mut t = ClassTotal {
        class,
        ..Default::default()
    };
    for it in items {
        t.items += 1;
        match (it.measured.logical_bytes, it.measured.disk_bytes) {
            (Some(l), Some(d)) => {
                t.logical_bytes = t.logical_bytes.saturating_add(l);
                t.disk_bytes = t.disk_bytes.saturating_add(d);
            }
            _ => t.unmeasured += 1,
        }
    }
    t
}

/// PURE. How many items each consumer kind was actually probed for, so an unprobed kind can never be
/// misread as evidence of freedom.
pub fn probe_coverage(items: &[ReportItem]) -> Vec<ProbeCoverage> {
    let total = items.len() as u64;
    CONSUMER_KINDS
        .iter()
        .enumerate()
        .map(|(i, (kind, source))| ProbeCoverage {
            kind: (*kind).to_string(),
            probed: items
                .iter()
                .filter(|it| it.consumers.states()[i].probed())
                .count() as u64,
            total,
            source: (*source).to_string(),
        })
        .collect()
}

/// True iff any lock was actually flocked during this run (so the disclosure is only printed when it
/// applies). A probe of an absent path never takes a lock.
pub fn any_lock_probed(items: &[ReportItem]) -> bool {
    items
        .iter()
        .any(|i| i.consumers.run_lease.probed() || i.consumers.operation_lock.probed())
}

// ---------------------------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------------------------

/// PURE. Compact human bytes (`1.5 GiB`); `unknown` for an unmeasured item.
pub fn human_bytes(b: Option<u64>) -> String {
    let Some(b) = b else {
        return "unknown".into();
    };
    const UNITS: [&str; 6] = ["B", "KiB", "MiB", "GiB", "TiB", "PiB"];
    let mut v = b as f64;
    let mut i = 0;
    while v >= 1024.0 && i + 1 < UNITS.len() {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 {
        format!("{b} B")
    } else {
        format!("{v:.1} {}", UNITS[i])
    }
}

/// One item row, or — given the column titles — the header. Sharing one format string is what keeps
/// the header aligned with the rows.
/// Column values in header order: class, logical, on-disk, lease, oplock, mount, proc, path.
fn item_row(c: [&str; 8]) -> String {
    format!(
        "{:<16} {:>11} {:>11}  {:<6} {:<7} {:<6} {:<5} {}",
        c[0], c[1], c[2], c[3], c[4], c[5], c[6], c[7]
    )
}

pub fn item_header() -> String {
    item_row([
        "CLASS", "LOGICAL", "ON-DISK", "LEASE", "OPLOCK", "MOUNT", "PROC", "PATH",
    ])
}

/// PURE. One item row (column order matches [`item_header`]).
pub fn format_item(it: &ReportItem) -> String {
    let class = match (it.class, it.checkout_kind) {
        (PayloadClass::SourceCheckout, Some(k)) => format!("Checkout/{}", k.label()),
        (c, _) => c.label().to_string(),
    };
    item_row([
        &class,
        &human_bytes(it.measured.logical_bytes),
        &human_bytes(it.measured.disk_bytes),
        it.consumers.run_lease.label(),
        it.consumers.operation_lock.label(),
        it.consumers.container_mount.label(),
        it.consumers.process.label(),
        &it.path,
    ])
}

fn git_row(head: &str, kind: &str, branch: &str, state: &str, on: &str, path: &str) -> String {
    format!("{head:<8} {kind:<9} {branch:<28} {state:<6} {on:<22} {path}")
}

pub fn git_header() -> String {
    git_row("HEAD", "KIND", "BRANCH", "STATE", "CONTAINMENT", "PATH")
}

/// PURE. One git row for a `SourceCheckout`. The containment column names WHICH question was answered:
/// `on-source` (the live source repo — S4's gate) or `on-origin` (local remote-tracking refs, as of the
/// last fetch, which may overstate a deleted or force-pushed upstream).
pub fn format_git(it: &ReportItem) -> String {
    let g = it.git.clone().unwrap_or_default();
    let head = if g.unborn {
        "(unborn)".to_string()
    } else {
        g.head
            .as_deref()
            .map(|h| h.chars().take(7).collect::<String>())
            .unwrap_or_else(|| "?".into())
    };
    let branch = g
        .branch
        .clone()
        .unwrap_or_else(|| if g.unborn { "?" } else { "(detached)" }.to_string());
    let state = match g.dirty {
        Some(true) => "dirty",
        Some(false) => "clean",
        None => "?",
    };
    let containment = match (&g.on_source_main, g.on_origin_as_of_last_fetch) {
        (Some(v), _) => format!("on-main {}", v.label()),
        (None, Some(v)) => format!("on-origin {} (as of fetch)", yes_no(v)),
        (None, None) => "-".to_string(),
    };
    git_row(
        &head,
        it.checkout_kind.map(|k| k.label()).unwrap_or("-"),
        &branch,
        state,
        &containment,
        &it.path,
    )
}

fn yes_no(v: bool) -> &'static str {
    if v {
        "yes"
    } else {
        "no"
    }
}

/// PURE. The whole human-readable report.
pub fn render_text(r: &StorageReport) -> String {
    let mut out = String::new();
    out.push_str(
        "storage report — READ-ONLY: nothing deleted, pushed, fetched, or modified, except that\n\
         reading a lock takes and immediately releases an advisory flock (see NOTES).\n\nROOTS\n",
    );
    if r.roots.is_empty() {
        out.push_str("  (none resolved)\n");
    }
    for root in &r.roots {
        out.push_str(&format!("  {root}\n"));
    }

    out.push_str(&format!("\nITEMS\n{}\n", item_header()));
    if r.items.is_empty() {
        out.push_str("(no items under the resolved bridge-owned roots)\n");
    }
    for it in &r.items {
        out.push_str(&format_item(it));
        out.push('\n');
        if let Some(note) = &it.note {
            out.push_str(&format!("    note: {note}\n"));
        }
    }

    let checkouts: Vec<&ReportItem> = r
        .items
        .iter()
        .filter(|i| i.class == PayloadClass::SourceCheckout)
        .collect();
    if !checkouts.is_empty() {
        out.push_str(
            "\nSOURCE CHECKOUTS\n\
             on-main = is this content verifiably on the SOURCE repository's main branch? This is the\n\
             D-1 gate. `yes(head)` = HEAD is an ancestor of main; `yes(tree)` = HEAD's exact tree is on\n\
             main under a different commit (a squash landing); `no` = demonstrably not, INCLUDING a\n\
             squash that rewrote the tree (fail-closed: the clone is kept); `unknown` = a probe failed\n\
             and nothing may be concluded. Reachability from some other ref is NOT this gate.\n\
             on-origin = local remote-tracking refs contain HEAD as of the last fetch; no network was\n\
             contacted, so it may OVERSTATE a branch deleted or force-pushed upstream.\n",
        );
        out.push_str(&git_header());
        out.push('\n');
        let mut landed = 0usize;
        for it in &checkouts {
            out.push_str(&format_git(it));
            out.push('\n');
            if let Some(g) = it.git.as_ref() {
                if g.on_source_main.as_ref().is_some_and(|v| v.is_landed()) {
                    landed += 1;
                }
                if let Some(err) = g.probe_error.clone() {
                    out.push_str(&format!("    probe: {err}\n"));
                }
                if let Some(refs) = Some(&g.containing_refs).filter(|r| !r.is_empty()) {
                    out.push_str(&format!(
                        "    also on (informational, NOT the gate): {}\n",
                        refs.join(", ")
                    ));
                }
            }
        }
        out.push_str(&format!(
            "{landed} of {} checkouts have content verifiably on source main.\n",
            checkouts.len()
        ));
    }

    out.push_str("\nTOTALS BY CLASS\nCLASS                     ITEMS      LOGICAL      ON-DISK  UNMEASURED\n");
    let (mut ai, mut al, mut ad, mut au) = (0u64, 0u64, 0u64, 0u64);
    for t in &r.totals {
        ai += t.items;
        al = al.saturating_add(t.logical_bytes);
        ad = ad.saturating_add(t.disk_bytes);
        au += t.unmeasured;
        out.push_str(&format!(
            "{:<25} {:>5} {:>12} {:>12} {:>11}\n",
            t.class,
            t.items,
            human_bytes(Some(t.logical_bytes)),
            human_bytes(Some(t.disk_bytes)),
            t.unmeasured
        ));
    }
    out.push_str(&format!(
        "{:<25} {:>5} {:>12} {:>12} {:>11}\n",
        "TOTAL",
        ai,
        human_bytes(Some(al)),
        human_bytes(Some(ad)),
        au
    ));

    // Which kinds were actually probed. Without this the `?` column reads as "probably fine".
    out.push_str("\nCONSUMER PROBES  (Unknown/`?` = NOT probed — never read as free)\nKIND              PROBED  EVIDENCE SOURCE\n");
    for p in &r.probe_coverage {
        out.push_str(&format!(
            "{:<16} {:>3}/{:<3}  {}\n",
            p.kind, p.probed, p.total, p.source
        ));
    }

    let free = r
        .items
        .iter()
        .filter(|i| i.consumers.no_live_consumer_among_probed())
        .count();
    out.push_str(&format!(
        "\nNO LIVE CONSUMER  {free} of {} items — no PROBED kind reports held, and at least one kind\n\
         was probed. This is an OBSERVATION, not deletion authority: S3/S4 own that and must re-probe\n\
         at the destructive boundary.\n",
        r.items.len()
    ));

    out.push_str(&format!(
        "\nDATA VOLUME  {}  free {} of {}\n",
        r.data_volume.path,
        human_bytes(r.data_volume.free_bytes),
        human_bytes(r.data_volume.total_bytes),
    ));

    let mut notes = r.notes.clone();
    if any_lock_probed(&r.items) {
        notes.push(LOCK_PROBE_DISCLOSURE.to_string());
    }
    notes.push(format!(
        "process/open-file consumers: {PROCESS_PROBE_DISCLOSURE}"
    ));
    if r.items
        .iter()
        .any(|i| i.class == PayloadClass::ContainerOrImage)
        || r.probe_coverage.iter().any(|p| p.total > 0)
    {
        notes.push(VOLUME_OWNERSHIP_DISCLOSURE.to_string());
    }
    if !notes.is_empty() {
        out.push_str("\nNOTES\n");
        for n in &notes {
            out.push_str(&format!("  - {n}\n"));
        }
    }
    out
}

pub const STORAGE_USAGE: &str = "\
usage: a2a-bridge storage report [--config <f>] [--json]
       a2a-bridge storage reap --build-targets [--dry-run] [--config <f>] [--json]
       a2a-bridge storage reap --clones [--dry-run] [--config <f>] [--json]

`report` audits bridge-owned storage. READ-ONLY, and not a reaper: it deletes nothing, pushes nothing,
and fetches nothing — it is the instrument the storage reapers are gated on. Its findings are
OBSERVATIONS, never deletion authority: the reapers own that and re-probe at the destructive boundary.

`reap` is DESTRUCTIVE (see `a2a-bridge storage reap --help`): it removes completed runs' build targets
and per-run dependency caches, and nothing else, after re-checking every gate at the boundary.

  report              walk this config's bridge-owned roots (`<allowed_cwd_root>/.a2a-implement` and,
                      when `[worktrees]` is enabled, its root), classify every item
                      (SourceCheckout | BuildTarget | DependencyCache | Evidence | ContainerOrImage |
                      Unclassified), measure logical + on-disk bytes, and report live consumers
                      (run lease, operation lock, container mount), git HEAD/branch/dirty, and
                      containment: `on-source` for a clone (does the LIVE source repo contain HEAD —
                      S4's gate) or `on-origin` for a worktree (local remote-tracking refs as of the
                      last fetch; NO network, so it may overstate a deleted upstream branch). Also
                      per-class totals, probe coverage, and data-volume free space.
  --config <path>     registry config (default: ./a2a-bridge.toml); its `allowed_cwd_root` resolves the
                      implement root exactly as `a2a-bridge implement` does.
  --json              machine-readable output instead of the table.

JSON SCHEMA NOTE — every item carries a `source` field naming WHAT its `path` is, so a consumer never
has to infer it from the string's shape:
  \"implement-path\"  a canonical filesystem path under `.a2a-implement` (a run clone or its payloads);
                    the only source whose runs have a per-run operation lock to gate a deletion on.
  \"worktree-path\"   a canonical filesystem path under `[worktrees].root`. Custody is the ADR-0025
                    sidecar lease, and removal is `git worktree remove` — neither reaper touches one.
  \"volume-name\"     a container VOLUME NAME, not a path: nothing can `stat` or remove it, and
                    ADR-0021/0025 owns its lifecycle.

ONE DECLARED SIDE EFFECT: reading lock/lease liveness takes an advisory flock and immediately releases
it (there is no query-only flock API). A merge or resume racing that window sees a clean \"already
held\" refusal and can retry. Nothing else touches state.

Symlinks are never followed out of a bridge-owned root, and nothing outside the resolved roots is ever
listed. Container volumes are best-effort: an unreachable runtime is reported as a note, not a failure.";

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    fn git(p: &Path, args: &[&str]) {
        assert!(
            Command::new("git")
                .arg("-C")
                .arg(p)
                .args(args)
                .output()
                .unwrap()
                .status
                .success(),
            "git {args:?} in {}",
            p.display()
        );
    }

    fn write(p: &Path, bytes: usize) {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, vec![b'x'; bytes]).unwrap();
    }

    /// Mark a directory as a real cargo target (name evidence + a cargo marker).
    fn cargo_target(dir: &Path) {
        write(&dir.join("CACHEDIR.TAG"), 43);
        std::fs::create_dir_all(dir.join("debug")).unwrap();
    }

    /// A crashed run's leftover lock: the file persists, nobody holds the flock. Written directly
    /// rather than acquire-then-drop, because a just-released flock was measured still reading `Held`
    /// for subsequent probes under parallel load — see
    /// `run_lease_probe_reports_held_for_a_live_holder_and_unknown_for_an_absent_handle`. This shape is
    /// the crash case exactly (file left behind, lock free) and is deterministic.
    fn crashed_lock(lock_dir: &Path, id: &str) -> PathBuf {
        std::fs::create_dir_all(lock_dir).unwrap();
        let p = lock_dir.join(format!("{id}.lock"));
        std::fs::write(&p, b"").unwrap();
        p
    }

    /// A content fingerprint of an entire tree: every path (relative), its file type, and its exact
    /// bytes. Two runs producing the same fingerprint means the scan wrote NOTHING — created no file,
    /// removed none, and changed no byte (including `.git/index`, which `git status` may rewrite).
    fn tree_fingerprint(root: &Path) -> Vec<(String, String)> {
        let mut out = Vec::new();
        let mut stack = vec![root.to_path_buf()];
        while let Some(p) = stack.pop() {
            let md = std::fs::symlink_metadata(&p).unwrap();
            let rel = p.strip_prefix(root).unwrap().to_string_lossy().into_owned();
            if md.file_type().is_symlink() {
                out.push((
                    rel,
                    format!("link:{}", std::fs::read_link(&p).unwrap().display()),
                ));
            } else if md.is_dir() {
                out.push((rel, "dir".to_string()));
                for e in std::fs::read_dir(&p).unwrap() {
                    stack.push(e.unwrap().path());
                }
            } else {
                let bytes = std::fs::read(&p).unwrap();
                out.push((rel, format!("file:{}:{:x}", bytes.len(), fnv(&bytes))));
            }
        }
        out.sort();
        out
    }

    fn fnv(bytes: &[u8]) -> u64 {
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        for b in bytes {
            h ^= *b as u64;
            h = h.wrapping_mul(0x1000_0000_01b3);
        }
        h
    }

    /// A fixture with: a fake clone (`.git` dir + checkpoint evidence + cargo target + node_modules),
    /// a second bare clone, an unclassified stray, and a user checkout OUTSIDE the bridge root with a
    /// symlink pointing at it from inside.
    struct Fixture {
        _td: tempfile::TempDir,
        root: PathBuf,
        implement: PathBuf,
        user_repo: PathBuf,
    }

    fn fixture() -> Fixture {
        let td = tempfile::tempdir().unwrap();
        let root = td.path().to_path_buf();
        let implement = root.join(".a2a-implement");

        let clone = implement.join("impl-1-aa");
        std::fs::create_dir_all(clone.join(".git")).unwrap();
        write(&clone.join("src/lib.rs"), 100);
        write(&clone.join(".git/HEAD"), 41);
        write(
            &clone.join(".git/a2a-bridge/implement-checkpoint.json"),
            512,
        );
        cargo_target(&clone.join("target"));
        write(&clone.join("target/debug/blob"), 4096);
        write(&clone.join("node_modules/pkg/index.js"), 200);

        let bare = implement.join("impl-2-bb");
        std::fs::create_dir_all(bare.join(".git")).unwrap();
        write(&bare.join("README.md"), 10);

        write(&implement.join("stray-dir/notes.txt"), 7);

        let user_repo = root.join("user-repo");
        write(&user_repo.join("secret.txt"), 64);
        #[cfg(unix)]
        std::os::unix::fs::symlink(&user_repo, implement.join("escape-link")).unwrap();

        Fixture {
            _td: td,
            root,
            implement,
            user_repo,
        }
    }

    fn report_of(implement: &Path) -> (Vec<ReportItem>, String) {
        let mut notes = Vec::new();
        let items = scan_implement_root(implement, &mut notes);
        let totals = totals(&items);
        let probe_coverage = probe_coverage(&items);
        let text = render_text(&StorageReport {
            roots: vec![display_path(implement)],
            items: items.clone(),
            totals,
            probe_coverage,
            data_volume: DataVolume::default(),
            notes,
        });
        (items, text)
    }

    #[test]
    fn classifies_clone_evidence_target_and_cache_under_the_implement_root() {
        let f = fixture();
        let mut notes = Vec::new();
        let items = scan_implement_root(&f.implement, &mut notes);
        let by_class = |c: PayloadClass| -> Vec<&ReportItem> {
            items.iter().filter(|i| i.class == c).collect()
        };

        let checkouts = by_class(PayloadClass::SourceCheckout);
        assert_eq!(checkouts.len(), 2, "two `.git`-bearing clones: {items:#?}");
        assert!(checkouts
            .iter()
            .all(|i| i.checkout_kind == Some(CheckoutKind::StandaloneClone)));

        let evidence = by_class(PayloadClass::Evidence);
        assert_eq!(evidence.len(), 1, "only impl-1-aa has .git/a2a-bridge");
        assert!(evidence[0].path.ends_with(".git/a2a-bridge"));
        assert_eq!(evidence[0].run_id.as_deref(), Some("impl-1-aa"));

        let targets = by_class(PayloadClass::BuildTarget);
        assert_eq!(targets.len(), 1);
        assert!(targets[0].path.ends_with("impl-1-aa/target"));

        let caches = by_class(PayloadClass::DependencyCache);
        assert_eq!(caches.len(), 1);
        assert!(caches[0].path.ends_with("impl-1-aa/node_modules"));

        let unclassified = by_class(PayloadClass::Unclassified);
        assert_eq!(unclassified.len(), 1);
        assert!(unclassified[0].path.ends_with("stray-dir"));
    }

    #[test]
    fn a_target_without_cargo_evidence_is_not_promoted_to_buildtarget() {
        let td = tempfile::tempdir().unwrap();
        let implement = td.path().join(".a2a-implement");
        let clone = implement.join("impl-3-cc");
        std::fs::create_dir_all(clone.join(".git")).unwrap();
        // A `target/` full of a user's own data, with none of cargo's markers.
        write(&clone.join("target/handwritten.txt"), 99);
        // And a `target/` with ONLY the generic CACHEDIR.TAG: evidence, not proof (plan §5).
        let other = implement.join("impl-4-dd");
        std::fs::create_dir_all(other.join(".git")).unwrap();
        write(&other.join("target/CACHEDIR.TAG"), 43);

        let mut notes = Vec::new();
        let items = scan_implement_root(&implement, &mut notes);
        assert!(
            !items.iter().any(|i| i.class == PayloadClass::BuildTarget),
            "neither target/ may be classified reapable: {items:#?}"
        );
        let checkout = items
            .iter()
            .find(|i| i.path.ends_with("impl-3-cc"))
            .unwrap();
        assert!(checkout.measured.logical_bytes.unwrap() >= 99);
    }

    #[test]
    fn nested_targets_below_the_immediate_children_are_classified() {
        // A cargo workspace layout: `crates/*/target`, not just `<root>/target`. Build targets are the
        // plan's largest class, so missing these would understate the audit.
        let td = tempfile::tempdir().unwrap();
        let implement = td.path().join(".a2a-implement");
        let clone = implement.join("impl-ws-1");
        std::fs::create_dir_all(clone.join(".git")).unwrap();
        write(&clone.join("Cargo.toml"), 20);
        cargo_target(&clone.join("crates/a/target"));
        write(&clone.join("crates/a/target/debug/x.o"), 1000);
        cargo_target(&clone.join("crates/b/target"));
        write(&clone.join("crates/b/target/debug/y.o"), 2000);
        write(&clone.join("crates/a/src/lib.rs"), 5);

        let mut notes = Vec::new();
        let items = scan_implement_root(&implement, &mut notes);
        let targets: Vec<&ReportItem> = items
            .iter()
            .filter(|i| i.class == PayloadClass::BuildTarget)
            .collect();
        assert_eq!(targets.len(), 2, "both nested targets: {items:#?}");
        assert_eq!(targets[0].measured.logical_bytes, Some(43 + 1000));
        assert_eq!(targets[1].measured.logical_bytes, Some(43 + 2000));
        let checkout = items
            .iter()
            .find(|i| i.class == PayloadClass::SourceCheckout)
            .unwrap();
        assert_eq!(
            checkout.measured.logical_bytes,
            Some(25),
            "and their bytes are excluded from the checkout: {checkout:#?}"
        );
    }

    #[test]
    fn checkout_bytes_exclude_separately_reported_children() {
        let f = fixture();
        let mut notes = Vec::new();
        let items = scan_implement_root(&f.implement, &mut notes);
        let get = |pred: &dyn Fn(&ReportItem) -> bool| -> &ReportItem {
            items.iter().find(|i| pred(i)).unwrap()
        };

        let checkout =
            get(&|i| i.class == PayloadClass::SourceCheckout && i.path.ends_with("impl-1-aa"));
        let evidence = get(&|i| i.class == PayloadClass::Evidence);
        let target = get(&|i| i.class == PayloadClass::BuildTarget);
        let cache = get(&|i| i.class == PayloadClass::DependencyCache);

        assert_eq!(evidence.measured.logical_bytes, Some(512));
        assert_eq!(target.measured.logical_bytes, Some(43 + 4096));
        assert_eq!(cache.measured.logical_bytes, Some(200));
        // src/lib.rs (100) + .git/HEAD (41). The evidence/target/cache bytes are NOT here.
        assert_eq!(checkout.measured.logical_bytes, Some(141));
        assert_eq!(checkout.measured.files, 2);

        // Totals therefore sum each byte exactly once.
        let t = totals(&items);
        let sum: u64 = t.iter().map(|c| c.logical_bytes).sum();
        assert_eq!(sum, 141 + 512 + 43 + 4096 + 200 + 10 + 7);
        let clones = t
            .iter()
            .find(|c| c.class == "SourceCheckout (clone)")
            .unwrap();
        assert_eq!(clones.items, 2);
        assert_eq!(clones.logical_bytes, 141 + 10);
        assert!(t.iter().all(|c| c.unmeasured == 0));
    }

    #[test]
    fn totals_split_standalone_clones_from_linked_worktrees() {
        // S4 may `rm -rf` a standalone clone under D-1; a linked worktree shares its source's object
        // store and must go through `git worktree remove`. The totals must not merge them.
        let t = totals(&[
            ReportItem {
                path: "/c".into(),
                source: ItemSource::ImplementPath,
                class: PayloadClass::SourceCheckout,
                checkout_kind: Some(CheckoutKind::StandaloneClone),
                run_id: None,
                measured: Measured {
                    logical_bytes: Some(100),
                    disk_bytes: Some(200),
                    files: 1,
                    errors: 0,
                },
                consumers: LiveConsumers::default(),
                git: None,
                note: None,
            },
            ReportItem {
                path: "/w".into(),
                source: ItemSource::WorktreePath,
                class: PayloadClass::SourceCheckout,
                checkout_kind: Some(CheckoutKind::LinkedWorktree),
                run_id: None,
                measured: Measured {
                    logical_bytes: Some(7),
                    disk_bytes: Some(8),
                    files: 1,
                    errors: 0,
                },
                consumers: LiveConsumers::default(),
                git: None,
                note: None,
            },
        ]);
        let clone = t
            .iter()
            .find(|c| c.class == "SourceCheckout (clone)")
            .unwrap();
        let wt = t
            .iter()
            .find(|c| c.class == "SourceCheckout (worktree)")
            .unwrap();
        assert_eq!((clone.items, clone.logical_bytes), (1, 100));
        assert_eq!((wt.items, wt.logical_bytes), (1, 7));
    }

    #[test]
    fn on_disk_bytes_are_reported_beside_logical_bytes() {
        let f = fixture();
        let mut notes = Vec::new();
        for it in scan_implement_root(&f.implement, &mut notes) {
            assert!(
                it.measured.disk_bytes.is_some(),
                "every measurable item reports allocated bytes too: {it:#?}"
            );
        }
    }

    #[test]
    fn a_user_path_outside_the_bridge_roots_is_never_listed() {
        let f = fixture();
        let mut notes = Vec::new();
        let items = scan_implement_root(&f.implement, &mut notes);
        let user = display_path(&f.user_repo);
        assert!(
            !items.iter().any(|i| i.path.starts_with(&user)),
            "a protected/user path must never appear: {:#?}",
            items.iter().map(|i| &i.path).collect::<Vec<_>>()
        );
        assert!(!items.iter().any(|i| i.path.contains("escape-link")));
        #[cfg(unix)]
        assert!(
            notes.iter().any(|n| n.contains("symlink skipped")),
            "the refusal must be reported, not silent: {notes:?}"
        );
        assert!(f.root.join("user-repo/secret.txt").exists());
    }

    #[test]
    fn operation_lock_state_is_probed_without_creating_or_taking_it() {
        let f = fixture();
        let lock_dir = f.implement.join(OPERATION_LOCK_DIR);

        let mut notes = Vec::new();
        let items = scan_implement_root(&f.implement, &mut notes);
        let one = items
            .iter()
            .find(|i| {
                i.run_id.as_deref() == Some("impl-1-aa") && i.class == PayloadClass::SourceCheckout
            })
            .unwrap();
        assert_eq!(one.consumers.operation_lock, HolderState::Unknown);
        assert!(!lock_dir.exists(), "probing must not create {lock_dir:?}");

        let held =
            bridge_core::liveness::acquire_persistent_lock_in(&lock_dir, "impl-1-aa").unwrap();
        crashed_lock(&lock_dir, "impl-2-bb"); // left behind by a crashed run: free
        let items = scan_implement_root(&f.implement, &mut notes);
        let state = |id: &str| {
            items
                .iter()
                .find(|i| {
                    i.run_id.as_deref() == Some(id) && i.class == PayloadClass::SourceCheckout
                })
                .unwrap()
                .consumers
                .operation_lock
        };
        assert_eq!(state("impl-1-aa"), HolderState::Held);
        assert_eq!(state("impl-2-bb"), HolderState::Free);
        assert!(
            bridge_core::liveness::acquire_persistent_lock_in(&lock_dir, "impl-1-aa").is_err(),
            "the probe must not have stolen or dropped the live holder's lock"
        );
        drop(held);
    }

    /// This test owns the mapping `FsLeaseProbe` → [`HolderState`], not the primitive's own crash
    /// semantics (`bridge_core::liveness` already tests those beside the implementation).
    ///
    /// The released-lock ⇒ `Free` leg is deliberately NOT asserted here: under parallel test load a
    /// just-released flock was observed still reading `Held` for one or more subsequent probes on a
    /// path no other test touches (measured: `[Held, Held, Free, Free, Free]`). That is a property of
    /// flock release visibility, not of this mapping — and it is exactly why the report is an
    /// observation rather than authorization (see the module docs). The `Free` reading IS covered
    /// deterministically through the real scan path by
    /// `operation_lock_state_is_probed_without_creating_or_taking_it`.
    #[test]
    fn run_lease_probe_reports_held_for_a_live_holder_and_unknown_for_an_absent_handle() {
        let dir = tempfile::tempdir().unwrap();
        let guard = bridge_core::liveness::acquire_lease_in(dir.path(), "live").unwrap();
        assert_eq!(
            probe_lock_path(guard.path()),
            HolderState::Held,
            "a live holder must read Held"
        );

        assert_eq!(
            probe_lock_path(&dir.path().join("never-existed.lock")),
            HolderState::Unknown,
            "an absent handle proves nothing — never Free"
        );

        // A directory at the lock path is not a lock handle either.
        assert_eq!(
            probe_lock_path(dir.path()),
            HolderState::Unknown,
            "a non-file path is not a probe-able handle"
        );
        drop(guard);
    }

    /// Build an upstream repo + a `--no-hardlinks` clone of it under an implement root, exactly as
    /// `implement::clone_argv` does.
    fn upstream_and_clone(td: &Path, id: &str) -> (PathBuf, PathBuf, PathBuf) {
        let implement = td.join(".a2a-implement");
        let upstream = td.join("upstream");
        std::fs::create_dir_all(&upstream).unwrap();
        git(&upstream, &["init", "-q", "-b", "main"]);
        git(&upstream, &["config", "user.email", "t@t"]);
        git(&upstream, &["config", "user.name", "t"]);
        write(&upstream.join("a.txt"), 5);
        git(&upstream, &["add", "."]);
        git(&upstream, &["commit", "-qm", "base"]);

        let clone = implement.join(id);
        std::fs::create_dir_all(&implement).unwrap();
        assert!(Command::new("git")
            .args(["clone", "-q", "--no-hardlinks"])
            .arg(&upstream)
            .arg(&clone)
            .output()
            .unwrap()
            .status
            .success());
        git(&clone, &["config", "user.email", "t@t"]);
        git(&clone, &["config", "user.name", "t"]);
        (implement, upstream, clone)
    }

    /// Scan and return the single SourceCheckout's git facts.
    fn facts_of(implement: &Path) -> GitFacts {
        let mut notes = Vec::new();
        scan_implement_root(implement, &mut notes)
            .into_iter()
            .find(|i| i.class == PayloadClass::SourceCheckout)
            .expect("one checkout")
            .git
            .expect("git facts")
    }

    /// Commit some work on a topic branch in the clone; returns (head, tree).
    fn commit_work(clone: &Path, branch: &str) -> (String, String) {
        git(clone, &["checkout", "-q", "-b", branch]);
        write(&clone.join("b.txt"), 3);
        git(clone, &["add", "."]);
        git(clone, &["commit", "-qm", "wip"]);
        (
            git_str(clone, &["rev-parse", "HEAD"]).unwrap(),
            git_str(clone, &["rev-parse", "HEAD^{tree}"]).unwrap(),
        )
    }

    #[test]
    fn w1_local_origin_clone_is_judged_against_the_live_source_repo() {
        let td = tempfile::tempdir().unwrap();
        let (implement, _upstream, clone) = upstream_and_clone(td.path(), "impl-src-1");

        // A fresh clone's HEAD IS the source's main tip ⇒ landed by head-reachability.
        let g = facts_of(&implement);
        assert_eq!(g.probe_error, None, "{g:#?}");
        assert!(g.origin_is_local_path, "{g:#?}");
        assert_eq!(g.on_source_main, Some(OnSourceMain::YesHead), "{g:#?}");
        // FULLY QUALIFIED: a bare `main` would let a tag of that name stand in for the branch.
        assert_eq!(
            g.source_main_ref.as_deref(),
            Some("refs/heads/main"),
            "{g:#?}"
        );
        assert_eq!(
            g.on_origin_as_of_last_fetch, None,
            "a frozen origin/* snapshot must not be presented as reachability"
        );

        // Unpushed work: the object is not in the source at all ⇒ a definite `no`, not `unknown`.
        commit_work(&clone, "implement/impl-src-1");
        write(&clone.join("c.txt"), 3);
        let g = facts_of(&implement);
        assert_eq!(g.dirty, Some(true), "an untracked file counts as dirty");
        assert_eq!(g.on_source_main, Some(OnSourceMain::No), "{g:#?}");
    }

    /// W1. THE BLOCKER: a commit reachable only from some topic branch in the source is NOT landed.
    /// D-1 authorizes reaping content that is on MAIN, not content someone parked on a side ref.
    #[test]
    fn w1_branch_only_landing_is_not_on_main() {
        let td = tempfile::tempdir().unwrap();
        let (implement, upstream, clone) = upstream_and_clone(td.path(), "impl-branch-1");
        let (head, _tree) = commit_work(&clone, "implement/impl-branch-1");

        // Land it in the SOURCE, but only on `refs/heads/landed` — main is untouched.
        assert!(Command::new("git")
            .arg("-C")
            .arg(&upstream)
            .args(["fetch", "-q"])
            .arg(&clone)
            .arg("implement/impl-branch-1:landed")
            .output()
            .unwrap()
            .status
            .success());

        let g = facts_of(&implement);
        assert_eq!(
            g.on_source_main,
            Some(OnSourceMain::No),
            "reachable only from a topic branch is NOT the D-1 gate: {g:#?}"
        );
        // Any-ref reachability is still reported, clearly as information rather than as the gate.
        assert!(
            g.containing_refs.iter().any(|r| r == "landed"),
            "informational refs still list it: {g:#?}"
        );
        assert_eq!(g.head.as_deref(), Some(head.as_str()));
    }

    /// W1. The repo's real landing shape: a squash merge whose commit id differs but whose tree is
    /// byte-identical. Content IS on main, so the gate must say so.
    #[test]
    fn w1_squash_landing_with_the_exact_tree_reads_yes_tree() {
        let td = tempfile::tempdir().unwrap();
        let (implement, upstream, clone) = upstream_and_clone(td.path(), "impl-squash-1");
        let (_head, tree) = commit_work(&clone, "implement/impl-squash-1");

        // Simulate the squash landing: a NEW commit on source main carrying the clone's exact tree.
        // The objects must exist in the source first.
        assert!(Command::new("git")
            .arg("-C")
            .arg(&upstream)
            .args(["fetch", "-q"])
            .arg(&clone)
            .arg("implement/impl-squash-1:refs/tmp/incoming")
            .output()
            .unwrap()
            .status
            .success());
        let main_tip = git_str(&upstream, &["rev-parse", "main"]).unwrap();
        let squashed = git_str(
            &upstream,
            &["commit-tree", &tree, "-p", &main_tip, "-m", "squash: land"],
        )
        .unwrap();
        git(&upstream, &["update-ref", "refs/heads/main", &squashed]);
        git(&upstream, &["update-ref", "-d", "refs/tmp/incoming"]);

        let g = facts_of(&implement);
        match &g.on_source_main {
            Some(OnSourceMain::YesTree { commit }) => assert_eq!(commit, &squashed),
            other => panic!("exact-tree squash landing must read yes(tree), got {other:?}\n{g:#?}"),
        }
    }

    /// W1. A plain fast-forward landing onto main is head-reachable.
    #[test]
    fn w1_fast_forward_landing_reads_yes_head() {
        let td = tempfile::tempdir().unwrap();
        let (implement, upstream, clone) = upstream_and_clone(td.path(), "impl-ff-1");
        let (head, _tree) = commit_work(&clone, "implement/impl-ff-1");
        // Fetch to a temp ref first: git refuses to fetch directly into a checked-out branch.
        assert!(Command::new("git")
            .arg("-C")
            .arg(&upstream)
            .args(["fetch", "-q"])
            .arg(&clone)
            .arg("implement/impl-ff-1:refs/tmp/ff")
            .output()
            .unwrap()
            .status
            .success());
        git(&upstream, &["update-ref", "refs/heads/main", &head]);

        let g = facts_of(&implement);
        assert_eq!(g.on_source_main, Some(OnSourceMain::YesHead), "{g:#?}");
        assert_eq!(g.head.as_deref(), Some(head.as_str()));
    }

    /// W1. A source whose main carries entirely unrelated content is a definite `no`.
    #[test]
    fn w1_unrelated_source_main_reads_no() {
        let td = tempfile::tempdir().unwrap();
        let (implement, upstream, clone) = upstream_and_clone(td.path(), "impl-unrel-1");
        commit_work(&clone, "implement/impl-unrel-1");
        // Move source main on with different content.
        write(&upstream.join("elsewhere.txt"), 12);
        git(&upstream, &["add", "."]);
        git(&upstream, &["commit", "-qm", "unrelated"]);

        let g = facts_of(&implement);
        assert_eq!(g.on_source_main, Some(OnSourceMain::No), "{g:#?}");
    }

    /// W1. An inadmissible probe must read `unknown`. Collapsing a broken probe to `no` is safe;
    /// collapsing it to `yes` would authorize deletion — but `no` is also wrong, because a reaper that
    /// sees `no` reports "unlanded work" when in truth it learned nothing.
    #[test]
    fn w1_failed_probe_reads_unknown_never_no() {
        let td = tempfile::tempdir().unwrap();
        let (implement, upstream, _clone) = upstream_and_clone(td.path(), "impl-broken-1");
        // The origin path still exists and is a directory, so it is still treated as a local source —
        // but it is no longer a git repository, so every probe against it fails.
        std::fs::remove_dir_all(upstream.join(".git")).unwrap();

        let g = facts_of(&implement);
        match &g.on_source_main {
            Some(OnSourceMain::Unknown { reason }) => {
                assert!(!reason.is_empty(), "the reason must be recorded")
            }
            other => panic!("a failed probe must never read as a verdict, got {other:?}\n{g:#?}"),
        }
        assert!(
            !g.on_source_main.as_ref().unwrap().is_landed(),
            "and unknown is certainly not landed"
        );
    }

    /// W2. A symlinked scan root must be refused outright — never enumerated through.
    #[test]
    fn w2_symlinked_root_is_refused_before_enumeration() {
        let td = tempfile::tempdir().unwrap();
        let real = td.path().join("real-root");
        std::fs::create_dir_all(real.join("child")).unwrap();
        assert!(verify_root(&real).is_ok(), "a real directory root is fine");

        let link = td.path().join("linked-root");
        #[cfg(unix)]
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let err = verify_root(&link).unwrap_err();
        assert!(
            err.contains("symlink"),
            "a symlinked root must be refused by name: {err}"
        );

        assert!(verify_root(&td.path().join("missing")).is_err());

        // A file is not a root, and the accepted root comes back canonical.
        write(&td.path().join("a-file"), 4);
        assert!(verify_root(&td.path().join("a-file")).is_err());
        let ok = verify_root(&real).unwrap();
        assert_eq!(ok, std::fs::canonicalize(&real).unwrap());
    }

    /// W2/W4. `.git` is re-examined immediately before any git process is spawned, so a swap between
    /// classification and probing cannot get a protected repository probed.
    #[test]
    fn w2_symlinked_git_is_ambiguous_and_never_probed() {
        let td = tempfile::tempdir().unwrap();
        let victim = td.path().join("victim");
        std::fs::create_dir_all(&victim).unwrap();
        git(&victim, &["init", "-q", "-b", "main"]);

        let swapped = td.path().join("swapped");
        std::fs::create_dir_all(&swapped).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(victim.join(".git"), swapped.join(".git")).unwrap();

        match git_shape(&swapped) {
            GitShape::Ambiguous { reason } => assert!(reason.contains("symlink"), "{reason}"),
            other => panic!("a symlinked .git must be Ambiguous, got {other:?}"),
        }
    }

    /// W4. A real linked worktree placed under `.a2a-implement` must NOT be reported as a standalone
    /// clone: S4 would `rm -rf` it and corrupt the source repo's worktree administration.
    #[test]
    fn w4_worktree_shape_under_the_implement_root_is_unclassified() {
        let td = tempfile::tempdir().unwrap();
        let source = td.path().join("src");
        std::fs::create_dir_all(&source).unwrap();
        git(&source, &["init", "-q", "-b", "main"]);
        git(&source, &["config", "user.email", "t@t"]);
        git(&source, &["config", "user.name", "t"]);
        write(&source.join("a.txt"), 5);
        git(&source, &["add", "."]);
        git(&source, &["commit", "-qm", "base"]);

        let implement = td.path().join(".a2a-implement");
        std::fs::create_dir_all(&implement).unwrap();
        let wt = implement.join("impl-wt-1");
        git(
            &source,
            &["worktree", "add", "-q", &wt.to_string_lossy(), "-b", "wt"],
        );
        assert!(
            real_file(&wt.join(".git")),
            "a linked worktree's .git is a FILE"
        );

        let mut notes = Vec::new();
        let items = scan_implement_root(&implement, &mut notes);
        let it = items
            .iter()
            .find(|i| i.path.ends_with("impl-wt-1"))
            .unwrap();
        assert_eq!(
            it.class,
            PayloadClass::Unclassified,
            "worktree shape under the clone root contradicts its location: {it:#?}"
        );
        assert!(
            it.note.as_deref().unwrap_or_default().contains("worktree"),
            "the contradiction must be named: {it:#?}"
        );
    }

    /// W4. And the mirror: a standalone clone under the worktree root.
    #[test]
    fn w4_clone_shape_under_the_worktree_root_is_unclassified() {
        let td = tempfile::tempdir().unwrap();
        let (_implement, upstream, _clone) = upstream_and_clone(td.path(), "impl-x");
        let wt_root = td.path().join("wt-root");
        std::fs::create_dir_all(&wt_root).unwrap();
        let rogue = wt_root.join("ownr-run1-abc");
        assert!(Command::new("git")
            .args(["clone", "-q", "--no-hardlinks"])
            .arg(&upstream)
            .arg(&rogue)
            .output()
            .unwrap()
            .status
            .success());
        assert!(
            real_dir(&rogue.join(".git")),
            "a clone's .git is a DIRECTORY"
        );

        let mut notes = Vec::new();
        let items = scan_worktree_root(&wt_root, &mut notes);
        let it = items
            .iter()
            .find(|i| i.path.ends_with("ownr-run1-abc"))
            .unwrap();
        assert_eq!(
            it.class,
            PayloadClass::Unclassified,
            "clone shape under the worktree root contradicts its location: {it:#?}"
        );
        assert!(
            it.note.as_deref().unwrap_or_default().contains("clone"),
            "the contradiction must be named: {it:#?}"
        );
    }

    /// W3. `a2a.repo` is written through a sanitizer that strips non-ASCII and caps at 200 chars, so a
    /// unicode or very long path cannot be represented. A non-match there is not evidence of absence.
    #[test]
    fn w3_lossy_label_path_can_never_read_free() {
        assert!(label_represents_path("/Users/w/code/proj"));
        assert!(!label_represents_path("/Users/w/code/проект"));
        assert!(!label_represents_path(&format!("/{}", "a".repeat(250))));

        let ev = MountEvidence {
            by_repo: BTreeMap::new(),
            runtimes_configured: 1,
            runtimes_answered: 1,
        };
        let (ascii, _) = resolve_mount("/Users/w/code/proj", &ev);
        assert_eq!(
            ascii,
            HolderState::Free,
            "a representable path may read Free"
        );

        let (unicode, _) = resolve_mount("/Users/w/code/проект", &ev);
        assert_eq!(
            unicode,
            HolderState::Unknown,
            "an unrepresentable path must not be declared unmounted"
        );
        let (long, _) = resolve_mount(&format!("/{}", "a".repeat(250)), &ev);
        assert_eq!(long, HolderState::Unknown);
    }

    /// W3. `Free` requires that EVERY configured runtime answered. One runtime answering says nothing
    /// about containers held by the one that did not.
    #[test]
    fn w3_partial_runtime_answers_can_never_read_free() {
        let partial = MountEvidence {
            by_repo: BTreeMap::new(),
            runtimes_configured: 2,
            runtimes_answered: 1,
        };
        let (m, _) = resolve_mount("/repo", &partial);
        assert_eq!(
            m,
            HolderState::Unknown,
            "a silent runtime could be holding it"
        );

        let complete = MountEvidence {
            by_repo: BTreeMap::new(),
            runtimes_configured: 2,
            runtimes_answered: 2,
        };
        assert_eq!(resolve_mount("/repo", &complete).0, HolderState::Free);

        let none = MountEvidence {
            runtimes_configured: 1,
            ..Default::default()
        };
        assert_eq!(resolve_mount("/repo", &none).0, HolderState::Unknown);
    }

    /// W3. Duplicate observations of one repo merge conservatively: any holder wins, unknown beats free.
    #[test]
    fn w3_duplicate_records_merge_held_over_unknown_over_free() {
        use HolderState::*;
        assert_eq!(merge_holder(Free, Held), Held);
        assert_eq!(merge_holder(Held, Unknown), Held);
        assert_eq!(merge_holder(Unknown, Free), Unknown);
        assert_eq!(merge_holder(Free, Free), Free);

        let ev = MountEvidence {
            by_repo: BTreeMap::from([("/repo".to_string(), Held)]),
            runtimes_configured: 1,
            runtimes_answered: 1,
        };
        let (mount, lease) = resolve_mount("/repo/sub", &ev);
        assert_eq!(mount, Held, "a mount covers paths beneath it");
        assert_eq!(lease, Some(Held));
    }

    /// W5. A git probe must not run repository-configured hooks. `core.fsmonitor` is the sharp one:
    /// `git status` would execute it, letting an attacker-controlled clone run code during an audit.
    #[test]
    fn w5_git_probes_do_not_run_repository_configured_hooks() {
        let td = tempfile::tempdir().unwrap();
        let implement = td.path().join(".a2a-implement");
        let clone = implement.join("impl-hook-1");
        std::fs::create_dir_all(&clone).unwrap();
        git(&clone, &["init", "-q", "-b", "main"]);
        git(&clone, &["config", "user.email", "t@t"]);
        git(&clone, &["config", "user.name", "t"]);
        write(&clone.join("a.txt"), 5);
        git(&clone, &["add", "."]);
        git(&clone, &["commit", "-qm", "base"]);

        let witness = td.path().join("fsmonitor-ran");
        let hook = td.path().join("fsmonitor-hook.sh");
        std::fs::write(
            &hook,
            format!("#!/bin/sh\ntouch '{}'\nexit 1\n", witness.display()),
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        git(
            &clone,
            &["config", "core.fsmonitor", &hook.to_string_lossy()],
        );

        let mut notes = Vec::new();
        let _ = scan_implement_root(&implement, &mut notes);
        assert!(
            !witness.exists(),
            "the audit must not execute a repository-configured hook"
        );
    }

    // ---- owner-round (C) repairs ---------------------------------------------------------------

    /// C1. An exhausted LOOKBACK is not an exhausted HISTORY. If the window filled up and no matching
    /// tree was found, the search was incomplete and the verdict must be `unknown`, not `no` — `no`
    /// would tell S4 "this content was never landed" on the strength of a truncated search.
    #[test]
    fn c1_lookback_exhaustion_is_unknown_not_no() {
        let td = tempfile::tempdir().unwrap();
        let (_implement, upstream, clone) = upstream_and_clone(td.path(), "impl-look-1");
        let (head, _tree) = commit_work(&clone, "implement/impl-look-1");

        // Source main advances well past the window with unrelated content.
        for i in 0..6 {
            write(&upstream.join(format!("f{i}.txt")), 4 + i);
            git(&upstream, &["add", "."]);
            git(&upstream, &["commit", "-qm", &format!("c{i}")]);
        }

        // Window smaller than history, no match inside it ⇒ incomplete ⇒ unknown.
        let (_r, verdict) = on_source_main_with_lookback(&upstream, &clone, &head, 3);
        match &verdict {
            OnSourceMain::Unknown { reason } => assert!(
                reason.contains("lookback"),
                "the reason must name the truncated search: {reason}"
            ),
            other => panic!("a truncated search must not read as a verdict, got {other:?}"),
        }
        assert!(!verdict.is_landed());

        // Window larger than history: the search really was exhaustive, so `no` is now correct.
        let (_r, verdict) = on_source_main_with_lookback(&upstream, &clone, &head, 500);
        assert_eq!(
            verdict,
            OnSourceMain::No,
            "an exhausted HISTORY does support a definite no"
        );
    }

    /// C1. A match inside the window still wins, and the sentinel row must not be mistaken for a match.
    #[test]
    fn c1_match_inside_an_exhausted_window_still_reads_yes_tree() {
        let td = tempfile::tempdir().unwrap();
        let (_implement, upstream, clone) = upstream_and_clone(td.path(), "impl-look-2");
        let (_head, tree) = commit_work(&clone, "implement/impl-look-2");
        assert!(Command::new("git")
            .arg("-C")
            .arg(&upstream)
            .args(["fetch", "-q"])
            .arg(&clone)
            .arg("implement/impl-look-2:refs/tmp/in")
            .output()
            .unwrap()
            .status
            .success());
        let tip = git_str(&upstream, &["rev-parse", "main"]).unwrap();
        let squashed = git_str(
            &upstream,
            &["commit-tree", &tree, "-p", &tip, "-m", "squash"],
        )
        .unwrap();
        git(&upstream, &["update-ref", "refs/heads/main", &squashed]);

        let head = git_str(&clone, &["rev-parse", "HEAD"]).unwrap();
        let (_r, verdict) = on_source_main_with_lookback(&upstream, &clone, &head, 1);
        match verdict {
            OnSourceMain::YesTree { commit } => assert_eq!(commit, squashed),
            other => panic!("the tip match must be found even with a window of 1, got {other:?}"),
        }
    }

    /// C2. A SAME-SHAPE swap must be refused: repointing `gitdir:` at another valid worktree, or
    /// swapping one `.git` directory for another, would otherwise attribute a different repository's
    /// HEAD, branch and containment to this path.
    #[test]
    fn c2_same_shape_git_replacement_is_refused() {
        let td = tempfile::tempdir().unwrap();
        let wt = td.path().join("wt");
        std::fs::create_dir_all(&wt).unwrap();
        let common_a = td.path().join("repo-a/.git/worktrees/w");
        let common_b = td.path().join("repo-b/.git/worktrees/w");
        std::fs::create_dir_all(&common_a).unwrap();
        std::fs::create_dir_all(&common_b).unwrap();

        std::fs::write(wt.join(".git"), format!("gitdir: {}\n", common_a.display())).unwrap();
        let captured = shape_fingerprint(&wt);
        assert!(matches!(captured.shape, GitShape::WorktreeFile { .. }));

        // Same VARIANT, different target repository.
        std::fs::write(wt.join(".git"), format!("gitdir: {}\n", common_b.display())).unwrap();
        let err = git_facts_rechecked(&wt, &captured).unwrap_err();
        assert!(
            err.contains("changed"),
            "a repointed gitdir must be refused: {err}"
        );
    }

    /// C2. And the directory case, where the shape carries no distinguishing data at all — only
    /// filesystem identity separates one `.git` directory from another.
    #[test]
    fn c2_swapped_git_directory_is_refused_by_filesystem_identity() {
        let td = tempfile::tempdir().unwrap();
        let clone = td.path().join("clone");
        std::fs::create_dir_all(&clone).unwrap();
        git(&clone, &["init", "-q", "-b", "main"]);
        let captured = shape_fingerprint(&clone);
        assert_eq!(captured.shape, GitShape::CloneDir);

        // A DIFFERENT repository's `.git`, same shape.
        let other = td.path().join("other");
        std::fs::create_dir_all(&other).unwrap();
        git(&other, &["init", "-q", "-b", "main"]);
        std::fs::remove_dir_all(clone.join(".git")).unwrap();
        std::fs::rename(other.join(".git"), clone.join(".git")).unwrap();

        let err = git_facts_rechecked(&clone, &captured).unwrap_err();
        assert!(
            err.contains("changed"),
            "a swapped .git directory must be refused: {err}"
        );

        // The unswapped case still probes normally.
        let fresh = shape_fingerprint(&clone);
        assert!(git_facts_rechecked(&clone, &fresh).is_ok());
    }

    /// C3. A runtime that exits zero but emits output we cannot parse has NOT answered. Counting it as
    /// answered yields an empty record set and therefore a false `Free` for every item.
    #[test]
    fn c3_malformed_ps_output_does_not_count_as_an_answer() {
        let healthy = "r1\trw\twarm\timpl\town9\th1\t/l/r.lock\t1700\t/repo\tname-1";
        let parse = |line: &str| {
            crate::containers::parse_record(line).map(|r| (r.repo.clone(), HolderState::Held))
        };

        let mut notes = Vec::new();
        let ok = ps_outcome("docker", Some(healthy), parse, &mut notes);
        assert!(ok.answered, "a fully-parsed answer counts");
        assert_eq!(ok.records.len(), 1);
        assert_eq!(ok.malformed_lines, 0);
        assert!(notes.is_empty());

        // Exit 0, but the output is not the format we asked for (a version skew, a wrapper, a truncated
        // stream). We do not know what this runtime is running.
        let mut notes = Vec::new();
        let bad = ps_outcome(
            "docker",
            Some("garbage\tnot\tten\tfields"),
            parse,
            &mut notes,
        );
        assert!(
            !bad.answered,
            "unparseable output is not an answer: {bad:?}"
        );
        assert_eq!(bad.malformed_lines, 1);
        assert!(
            notes.iter().any(|n| n.contains("docker")),
            "the runtime must be named: {notes:?}"
        );

        // A runtime that did not answer at all is likewise not an answer.
        let mut notes = Vec::new();
        let none = ps_outcome("podman", None, parse, &mut notes);
        assert!(!none.answered);
        assert!(notes.iter().any(|n| n.contains("podman")));
    }

    /// C3. End-to-end through the resolver: a malformed runtime leaves items Unknown, never Free.
    #[test]
    fn c3_incomplete_runtime_answer_leaves_items_unknown() {
        let parse = |line: &str| {
            crate::containers::parse_record(line).map(|r| (r.repo.clone(), HolderState::Held))
        };
        let mut notes = Vec::new();
        let outcome = ps_outcome("docker", Some("garbage"), parse, &mut notes);

        let ev = MountEvidence {
            by_repo: outcome.records.iter().cloned().collect(),
            runtimes_configured: 1,
            runtimes_answered: usize::from(outcome.answered),
        };
        assert_eq!(
            resolve_mount("/some/repo", &ev).0,
            HolderState::Unknown,
            "a runtime we could not parse cannot license a `free` reading"
        );

        // Positive control: the same path through a healthy answer does read Free.
        let healthy = "r1\trw\twarm\timpl\town9\th1\t/l/r.lock\t1700\t/other\tname-1";
        let mut notes = Vec::new();
        let outcome = ps_outcome("docker", Some(healthy), parse, &mut notes);
        let ev = MountEvidence {
            by_repo: outcome.records.iter().cloned().collect(),
            runtimes_configured: 1,
            runtimes_answered: usize::from(outcome.answered),
        };
        assert_eq!(resolve_mount("/some/repo", &ev).0, HolderState::Free);
        assert_eq!(resolve_mount("/other", &ev).0, HolderState::Held);
    }

    /// W8 (disclosed): a directory entry whose name is not valid UTF-8 is skipped, but the skip is
    /// reported rather than silent — a reaper must not believe the enumeration was complete.
    #[test]
    fn non_utf8_entry_names_are_skipped_with_a_note() {
        let td = tempfile::tempdir().unwrap();
        let implement = td.path().join(".a2a-implement");
        std::fs::create_dir_all(&implement).unwrap();
        #[cfg(unix)]
        {
            use std::ffi::OsStr;
            use std::os::unix::ffi::OsStrExt as _;
            let bad = implement.join(OsStr::from_bytes(b"impl-\xff\xfe-bad"));
            // APFS (and any filesystem enforcing UTF-8 names) REFUSES to create this. Where the name
            // cannot exist, the code path cannot be exercised — say so rather than assert vacuously.
            if std::fs::create_dir_all(&bad).is_err() {
                eprintln!(
                    "SKIPPED: this filesystem rejects non-UTF-8 names, so the skip-and-disclose path \
                     is UNVERIFIED here (it is exercised on filesystems that permit them)"
                );
                return;
            }
            let mut notes = Vec::new();
            let items = scan_implement_root(&implement, &mut notes);
            assert!(items.is_empty(), "the entry is not classified: {items:#?}");
            assert!(
                notes.iter().any(|n| n.contains("non-UTF-8")),
                "but the gap is disclosed: {notes:?}"
            );
        }
    }

    #[test]
    fn refs_containing_reports_absent_objects_as_not_contained_not_as_a_failure() {
        let td = tempfile::tempdir().unwrap();
        let repo = td.path().join("r");
        std::fs::create_dir_all(&repo).unwrap();
        git(&repo, &["init", "-q", "-b", "main"]);
        git(&repo, &["config", "user.email", "t@t"]);
        git(&repo, &["config", "user.name", "t"]);
        write(&repo.join("a"), 1);
        git(&repo, &["add", "."]);
        git(&repo, &["commit", "-qm", "base"]);

        let head = git_str(&repo, &["rev-parse", "HEAD"]).unwrap();
        assert_eq!(refs_containing(&repo, &head).unwrap(), vec!["main"]);
        assert_eq!(
            refs_containing(&repo, "0000000000000000000000000000000000000000").unwrap(),
            Vec::<String>::new(),
            "an absent object is a definite 'not contained', not a probe error"
        );
    }

    #[test]
    fn an_unborn_head_renders_as_unborn_not_detached() {
        let td = tempfile::tempdir().unwrap();
        let implement = td.path().join(".a2a-implement");
        let clone = implement.join("impl-unborn");
        std::fs::create_dir_all(&clone).unwrap();
        git(&clone, &["init", "-q", "-b", "main"]);

        let mut notes = Vec::new();
        let items = scan_implement_root(&implement, &mut notes);
        let it = items
            .iter()
            .find(|i| i.class == PayloadClass::SourceCheckout)
            .unwrap();
        let g = it.git.clone().unwrap();
        assert!(g.unborn, "{g:#?}");
        assert_eq!(g.head, None);
        assert_eq!(g.branch.as_deref(), Some("main"));
        assert_eq!(
            g.probe_error, None,
            "an unborn HEAD is a fact, not an error"
        );
        let row = format_git(it);
        assert!(row.contains("(unborn)"), "{row}");
        assert!(!row.contains("(detached)"), "{row}");
    }

    #[test]
    fn w3_symlinked_evidence_dir_is_never_followed() {
        let td = tempfile::tempdir().unwrap();
        let implement = td.path().join(".a2a-implement");
        let clone = implement.join("impl-evil-1");
        std::fs::create_dir_all(clone.join(".git")).unwrap();
        write(&clone.join("src/lib.rs"), 10);
        let protected = td.path().join("user-checkout");
        write(&protected.join("secret.txt"), 4096);
        #[cfg(unix)]
        std::os::unix::fs::symlink(&protected, clone.join(".git/a2a-bridge")).unwrap();

        let mut notes = Vec::new();
        let items = scan_implement_root(&implement, &mut notes);
        let user = display_path(&protected);
        assert!(
            !items.iter().any(|i| i.path.starts_with(&user)),
            "a symlinked evidence dir must not pull a protected root in: {:#?}",
            items.iter().map(|i| &i.path).collect::<Vec<_>>()
        );
        assert!(
            !items.iter().any(|i| i.class == PayloadClass::Evidence),
            "and it must not be reported as Evidence at all: {items:#?}"
        );
        #[cfg(unix)]
        assert!(
            notes
                .iter()
                .any(|n| n.contains("symlinked evidence dir refused")),
            "the refusal must be disclosed: {notes:?}"
        );
        // The clone's own bytes must also exclude the linked tree.
        let checkout = items
            .iter()
            .find(|i| i.class == PayloadClass::SourceCheckout)
            .unwrap();
        assert!(
            checkout.measured.logical_bytes.unwrap() < 4096,
            "the protected tree's bytes must not be counted: {checkout:#?}"
        );
    }

    #[test]
    fn w3_symlinked_git_dir_does_not_promote_a_stray_to_a_checkout() {
        let td = tempfile::tempdir().unwrap();
        let implement = td.path().join(".a2a-implement");
        let stray = implement.join("impl-evil-2");
        std::fs::create_dir_all(&stray).unwrap();
        let protected = td.path().join("user-checkout");
        std::fs::create_dir_all(protected.join(".git")).unwrap();
        write(&protected.join(".git/HEAD"), 41);
        #[cfg(unix)]
        std::os::unix::fs::symlink(protected.join(".git"), stray.join(".git")).unwrap();

        let mut notes = Vec::new();
        let items = scan_implement_root(&implement, &mut notes);
        let it = items
            .iter()
            .find(|i| i.path.ends_with("impl-evil-2"))
            .unwrap();
        assert_eq!(
            it.class,
            PayloadClass::Unclassified,
            "a symlinked .git is not proof of a bridge-owned clone: {it:#?}"
        );
        assert_eq!(it.checkout_kind, None);
        assert!(it.git.is_none(), "and no git probe may run against it");
        #[cfg(unix)]
        assert!(
            notes.iter().any(|n| n.contains("symlinked .git refused")),
            "{notes:?}"
        );
    }

    #[test]
    fn the_report_run_leaves_the_fixture_tree_byte_identical() {
        // The read-only contract, asserted rather than assumed. Real git repos are included because
        // `git status` WILL rewrite `.git/index` unless it is run with `--no-optional-locks`.
        //
        // This test discriminates: dropping `--no-optional-locks` from `git_ro` makes it fail, and the
        // ONLY differing entry is `.git/index` (same length, different bytes) — i.e. without the flag a
        // "read-only" report silently mutates every repository it inspects.
        //
        // The fixture is a `--no-hardlinks` clone of a local upstream, so the SOURCE repo is walked by
        // the on-source containment query too and is covered by the same assertion.
        let td = tempfile::tempdir().unwrap();
        let (implement, _upstream, clone) = upstream_and_clone(td.path(), "impl-7-rr");
        write(&clone.join("src/main.rs"), 128);
        cargo_target(&clone.join("target"));
        write(&clone.join(".git/a2a-bridge/implement-checkpoint.json"), 64);
        git(&clone, &["add", "src"]);
        git(&clone, &["commit", "-qm", "work"]);
        let user_repo = td.path().join("user-repo");
        write(&user_repo.join("keep.txt"), 11);

        // Make the index's cached stat data STALE while the content stays IDENTICAL: this is exactly
        // the state in which `git status` refreshes the index and writes `.git/index` back out. (A file
        // whose CONTENT changed would just stay "modified" and never trigger the rewrite.) The delay
        // takes the entry out of git's "racily clean" window — inside it git refuses to update the
        // cached stat, so a same-second rewrite would not exercise this at all.
        std::thread::sleep(std::time::Duration::from_millis(1100));
        std::fs::write(clone.join("src/main.rs"), vec![b'x'; 128]).unwrap();
        assert!(clone.join(".git/index").exists());

        let before = tree_fingerprint(td.path());
        let (_items, _text) = report_of(&implement);
        let after = tree_fingerprint(td.path());

        assert_eq!(
            before, after,
            "the report must not create, remove, or modify a single byte"
        );
    }

    #[test]
    fn worktree_root_reports_checkouts_sidecars_and_their_lease() {
        let td = tempfile::tempdir().unwrap();
        let root = td.path().join("wt-root");
        let wt = root.join("ownr-run7-abc");
        std::fs::create_dir_all(&wt).unwrap();
        // A linked worktree's `.git` is a FILE naming a resolvable common dir — the shape that decides
        // its kind, not the directory it happens to sit in.
        let common = td.path().join("source-repo/.git/worktrees/ownr-run7-abc");
        std::fs::create_dir_all(&common).unwrap();
        std::fs::write(wt.join(".git"), format!("gitdir: {}\n", common.display())).unwrap();
        write(&wt.join("file.txt"), 33);
        cargo_target(&wt.join("target"));

        let lease_dir = td.path().join("leases");
        let lease = bridge_core::liveness::acquire_lease_in(&lease_dir, "run7").unwrap();
        let sidecar = bridge_worktree::provider_path::WorktreeSidecar {
            canonical_source: "/repo".into(),
            common_dir: "/repo/.git".into(),
            worktree_path: display_path(&wt),
            owner: "ownr".into(),
            run_id: "run7".into(),
            host: "h1".into(),
            lease: lease.path().to_string_lossy().into_owned(),
        };
        bridge_worktree::provider_path::write_sidecar(&sidecar).unwrap();

        let mut notes = Vec::new();
        let items = scan_worktree_root(&root, &mut notes);
        let checkout = items
            .iter()
            .find(|i| i.class == PayloadClass::SourceCheckout)
            .unwrap();
        assert_eq!(
            checkout.checkout_kind,
            Some(CheckoutKind::LinkedWorktree),
            "a worktree is not a standalone clone — S4's authority differs"
        );
        assert_eq!(checkout.consumers.run_lease, HolderState::Held);
        // file.txt plus the `.git` pointer file itself; the cargo target is reported separately.
        let gitfile = std::fs::metadata(wt.join(".git")).unwrap().len();
        assert_eq!(
            checkout.measured.logical_bytes,
            Some(33 + gitfile),
            "target excluded"
        );
        assert!(items.iter().any(|i| i.class == PayloadClass::BuildTarget));
        let ev = items
            .iter()
            .find(|i| i.class == PayloadClass::Evidence)
            .expect("the sidecar is Evidence");
        assert!(ev.path.ends_with(".meta.json"));
        // W6: the sidecar IS the custody record for a live run. Reporting it with no consumer would
        // invite a reaper to treat live custody evidence as free-standing garbage.
        assert_eq!(
            ev.consumers.run_lease,
            HolderState::Held,
            "the sidecar carries the same lease it names: {ev:#?}"
        );

        drop(lease);
        let items = scan_worktree_root(&root, &mut notes);
        let checkout = items
            .iter()
            .find(|i| i.class == PayloadClass::SourceCheckout)
            .unwrap();
        assert_eq!(
            checkout.consumers.run_lease,
            HolderState::Unknown,
            "a clean lease drop removes the file — absent is Unknown, not Free"
        );
        let ev = items
            .iter()
            .find(|i| i.class == PayloadClass::Evidence)
            .unwrap();
        assert_eq!(
            ev.consumers.run_lease,
            HolderState::Unknown,
            "and the sidecar tracks it: {ev:#?}"
        );
    }

    #[test]
    fn volume_names_classify_by_segment_and_only_when_bridge_owned() {
        assert_eq!(
            classify_volume("a2a-verify-cache-0e8fba837f0f5749"),
            Some(PayloadClass::DependencyCache)
        );
        assert_eq!(
            classify_volume("a2a-impl-lsp-cache-04b977f7874de7c4"),
            Some(PayloadClass::DependencyCache)
        );
        assert_eq!(
            classify_volume("a2a-impl-lsp-target-5a5d88b3dfc8d5d2"),
            Some(PayloadClass::BuildTarget)
        );
        assert_eq!(
            classify_volume("a2a-kiro-data"),
            Some(PayloadClass::ContainerOrImage)
        );
        assert_eq!(
            classify_volume("a2a-r2f1b-completion-cargo-rx6zsu"),
            Some(PayloadClass::ContainerOrImage),
            "`cargo` is not `cache`, and `rx6zsu` is not a 16-hex hash"
        );
        assert_eq!(classify_volume("P-object-store"), None, "not bridge-owned");
        assert_eq!(classify_volume("postgres-data"), None);
    }

    /// W4. The shipped per-language warm caches carry a language segment before the hash — see
    /// `examples/a2a-bridge.containerized.toml` (`warm_cache = "a2a-impl-lsp-cache-{go,py,ts}"`).
    #[test]
    fn w4_language_suffixed_cache_volumes_classify_as_dependency_cache() {
        for name in [
            "a2a-impl-lsp-cache-go-0123456789abcdef",
            "a2a-impl-lsp-cache-py-0123456789abcdef",
            "a2a-impl-lsp-cache-ts-0123456789abcdef",
        ] {
            assert_eq!(
                classify_volume(name),
                Some(PayloadClass::DependencyCache),
                "{name} is a per-language dependency cache"
            );
        }
    }

    #[test]
    fn totals_count_unmeasured_items_apart_from_the_sums() {
        let items = vec![
            ReportItem {
                path: "vol-a".into(),
                source: ItemSource::VolumeName,
                class: PayloadClass::ContainerOrImage,
                checkout_kind: None,
                run_id: None,
                measured: Measured::default(), // unknown size
                consumers: LiveConsumers::default(),
                git: None,
                note: None,
            },
            ReportItem {
                path: "/x".into(),
                source: ItemSource::ImplementPath,
                class: PayloadClass::BuildTarget,
                checkout_kind: None,
                run_id: None,
                measured: Measured {
                    logical_bytes: Some(1024),
                    disk_bytes: Some(2048),
                    files: 1,
                    errors: 0,
                },
                consumers: LiveConsumers::default(),
                git: None,
                note: None,
            },
        ];
        let t = totals(&items);
        let vol = t.iter().find(|c| c.class == "ContainerOrImage").unwrap();
        assert_eq!((vol.items, vol.logical_bytes, vol.unmeasured), (1, 0, 1));
        let bt = t.iter().find(|c| c.class == "BuildTarget").unwrap();
        assert_eq!(
            (bt.items, bt.logical_bytes, bt.disk_bytes, bt.unmeasured),
            (1, 1024, 2048, 0)
        );
        assert_eq!(
            t.len(),
            PayloadClass::ALL.len() + 1,
            "every class gets a row, and SourceCheckout gets two"
        );
    }

    /// W1. `no_live_consumer_among_probed` judges only the kinds actually probed, and treats `Unknown`
    /// as "not evidence" rather than as "free". The predicate it replaced required all kinds to read
    /// `Free` at once, which no constructor could produce.
    #[test]
    fn no_live_consumer_judges_only_probed_kinds_and_never_infers_from_unknown() {
        let all_unknown = LiveConsumers::default();
        assert!(
            !all_unknown.no_live_consumer_among_probed(),
            "nothing probed ⇒ no claim"
        );

        let one_free = LiveConsumers {
            operation_lock: HolderState::Free,
            ..Default::default()
        };
        assert!(
            one_free.no_live_consumer_among_probed(),
            "one probed-and-free kind is a real observation even with the rest Unknown"
        );

        let mixed_held = LiveConsumers {
            operation_lock: HolderState::Free,
            container_mount: HolderState::Held,
            ..Default::default()
        };
        assert!(
            !mixed_held.no_live_consumer_among_probed(),
            "any Held kind vetoes it"
        );

        let all_free = LiveConsumers {
            run_lease: HolderState::Free,
            operation_lock: HolderState::Free,
            container_mount: HolderState::Free,
            process: HolderState::Free,
        };
        assert!(all_free.no_live_consumer_among_probed());
    }

    /// W1. The headline must be reachable on a real fixture: a released operation lock, no containers.
    #[test]
    fn w1_headline_counts_items_with_no_live_consumer_among_probed_kinds() {
        let f = fixture();
        crashed_lock(&f.implement.join(OPERATION_LOCK_DIR), "impl-1-aa");

        let (items, text) = report_of(&f.implement);
        let counted = items
            .iter()
            .filter(|i| i.consumers.no_live_consumer_among_probed())
            .count();
        assert!(
            counted > 0,
            "a free operation lock with no live holder must be counted: {items:#?}"
        );
        assert!(!text.contains("NO LIVE CONSUMER  0 of"), "{text}");
        assert!(
            text.contains("not deletion authority"),
            "and it must not read as reap authorization: {text}"
        );
    }

    /// W1. The report must disclose WHICH consumer kinds were actually probed.
    #[test]
    fn w1_report_discloses_probed_versus_unknown_consumer_kinds() {
        let f = fixture();
        let (items, text) = report_of(&f.implement);
        assert!(text.contains("CONSUMER PROBES"), "{text}");
        assert!(text.contains("process"), "{text}");
        assert!(text.contains(PROCESS_PROBE_DISCLOSURE), "{text}");

        let cov = probe_coverage(&items);
        let by = |k: &str| cov.iter().find(|c| c.kind == k).unwrap().clone();
        assert_eq!(by("process").probed, 0, "process is never probed in S2");
        assert_eq!(by("container_mount").probed, 0, "no runtime pass ran here");
        assert!(cov.iter().all(|c| c.total == items.len() as u64));
        assert!(items
            .iter()
            .all(|i| i.consumers.process == HolderState::Unknown));
    }

    /// W5. The flock side effect must be declared in the help text AND surfaced at runtime whenever a
    /// lock was actually taken.
    #[test]
    fn w5_flock_probe_side_effect_is_declared() {
        assert!(STORAGE_USAGE.contains("advisory flock"), "{STORAGE_USAGE}");
        assert!(LOCK_PROBE_DISCLOSURE.contains("advisory flock"));

        let f = fixture();
        let (_items, quiet) = report_of(&f.implement);
        assert!(
            !quiet.contains(LOCK_PROBE_DISCLOSURE),
            "no lock existed, so none was taken — do not claim otherwise: {quiet}"
        );

        crashed_lock(&f.implement.join(OPERATION_LOCK_DIR), "impl-1-aa");
        let (_items, text) = report_of(&f.implement);
        assert!(
            text.contains("advisory flock"),
            "a run that probed a lock must disclose it: {text}"
        );
    }

    #[test]
    fn human_bytes_buckets_and_marks_unknown() {
        assert_eq!(human_bytes(None), "unknown");
        assert_eq!(human_bytes(Some(0)), "0 B");
        assert_eq!(human_bytes(Some(512)), "512 B");
        assert_eq!(human_bytes(Some(1536)), "1.5 KiB");
        assert_eq!(human_bytes(Some(3 * 1024 * 1024 * 1024)), "3.0 GiB");
    }

    #[test]
    fn table_headers_align_with_their_rows() {
        // The header and the rows share one format string; this pins that they stay in step.
        let it = ReportItem {
            path: "/p".into(),
            source: ItemSource::ImplementPath,
            class: PayloadClass::BuildTarget,
            checkout_kind: None,
            run_id: None,
            measured: Measured {
                logical_bytes: Some(1),
                disk_bytes: Some(1),
                files: 1,
                errors: 0,
            },
            consumers: LiveConsumers::default(),
            git: None,
            note: None,
        };
        let header = item_header();
        let row = format_item(&it);
        assert_eq!(
            header.find("PATH"),
            row.find("/p"),
            "PATH column must start where the path does:\n{header}\n{row}"
        );
        assert_eq!(
            header.find("MOUNT").unwrap(),
            row.match_indices('?').nth(2).unwrap().0,
            "the third `?` is the MOUNT column:\n{header}\n{row}"
        );
        let gh = git_header();
        let grow = format_git(&ReportItem {
            checkout_kind: Some(CheckoutKind::StandaloneClone),
            class: PayloadClass::SourceCheckout,
            git: Some(GitFacts::default()),
            ..it
        });
        assert_eq!(
            gh.find("PATH"),
            grow.find("/p"),
            "git PATH column:\n{gh}\n{grow}"
        );
    }

    #[test]
    fn rendered_report_states_read_only_and_names_the_containment_question() {
        let f = fixture();
        let mut notes = Vec::new();
        let items = scan_implement_root(&f.implement, &mut notes);
        let totals = totals(&items);
        let probe_coverage = probe_coverage(&items);
        let text = render_text(&StorageReport {
            roots: vec![display_path(&f.implement)],
            items,
            totals,
            probe_coverage,
            data_volume: DataVolume {
                path: display_path(&f.root),
                free_bytes: Some(1024),
                total_bytes: Some(4096),
            },
            notes,
        });
        assert!(text.contains("READ-ONLY"), "{text}");
        assert!(text.contains("as of the last fetch"), "{text}");
        assert!(text.contains("may OVERSTATE"), "{text}");
        assert!(text.contains("Checkout/clone"), "{text}");
        assert!(text.contains("BuildTarget"));
        assert!(text.contains("TOTALS BY CLASS"));
        assert!(text.contains("SourceCheckout (clone)"));
        assert!(text.contains("SourceCheckout (worktree)"));
        assert!(text.contains("DATA VOLUME"));
        assert!(text.contains("free 1.0 KiB of 4.0 KiB"), "{text}");
        assert!(text.contains(VOLUME_OWNERSHIP_DISCLOSURE), "{text}");
    }

    #[test]
    fn filesystem_space_reports_a_real_volume() {
        let td = tempfile::tempdir().unwrap();
        let (free, total) = filesystem_space(td.path());
        assert!(free.is_some() && total.is_some(), "statvfs on a temp dir");
        assert!(total.unwrap() >= free.unwrap());
        assert_eq!(
            filesystem_space(Path::new("/definitely/not/a/path/xyzzy")),
            (None, None),
            "a failed probe is unknown, not zero"
        );
    }

    #[test]
    fn usage_text_documents_the_read_only_contract_and_its_one_exception() {
        assert!(STORAGE_USAGE.contains("READ-ONLY"));
        assert!(STORAGE_USAGE.contains("--json"));
        assert!(STORAGE_USAGE.contains("--config"));
        assert!(STORAGE_USAGE.contains("NO network"));
        assert!(STORAGE_USAGE.contains("advisory flock"));
        assert!(STORAGE_USAGE.contains("not a reaper"));
        assert!(STORAGE_USAGE.contains("on-source"));
    }

    // ---- R2f1b slice 2b2: the second record suffix and the custody lock directory ----

    fn v3_custody_record(worktree: &Path, state: bridge_worktree::custody::WorktreeCustodyStateV1) {
        use bridge_core::execution_policy::{
            PolicyNodeRefV1, Sha256HexV1, WorktreeCustodyIdV1, WorktreeObjectIdentityV1,
        };
        use bridge_core::fs_custody::DirectoryIdentityV1;
        use bridge_core::ids::{AttemptId, AttemptIdentity, ExecutionId};
        let canonical = display_path(worktree);
        let meta = std::fs::symlink_metadata(worktree).unwrap();
        use std::os::unix::fs::MetadataExt as _;
        let mut record = bridge_worktree::custody::WorktreeCustodyRecordV1 {
            schema_version: bridge_worktree::custody::WORKTREE_CUSTODY_RECORD_SCHEMA_V1,
            custody_id: WorktreeCustodyIdV1::parse(format!("custody-{}", "3".repeat(64))).unwrap(),
            checkout_fingerprint: Sha256HexV1::parse("6".repeat(64)).unwrap(),
            current_attempt: AttemptIdentity {
                execution_id: ExecutionId::parse(format!("exec-{}", "1".repeat(32))).unwrap(),
                attempt_id: AttemptId::parse(format!("attempt-{}", "2".repeat(32))).unwrap(),
                ordinal: 0,
                parent_attempt_id: None,
            },
            worktree: WorktreeObjectIdentityV1 {
                canonical_path: canonical.clone(),
                directory_identity: DirectoryIdentityV1 {
                    canonical_path: canonical.clone(),
                    dev: Some(meta.dev()),
                    ino: Some(meta.ino()),
                },
            },
            state,
            claim: None,
        };
        // The state's own settled rule decides whether a claim is REQUIRED (2a data); a record
        // that violates it would not encode at all.
        if record.state.claim_presence() == bridge_worktree::custody::ClaimPresenceV1::Required {
            record.claim = Some(bridge_worktree::custody::PreservedWorktreeClaimV1 {
                schema_version: bridge_worktree::custody::WORKTREE_CUSTODY_RECORD_SCHEMA_V1,
                custody_id: record.custody_id.clone(),
                execution_id: record.current_attempt.execution_id.clone(),
                origin_attempt_id: record.current_attempt.attempt_id.clone(),
                current_attempt: record.current_attempt.clone(),
                node: PolicyNodeRefV1::from_node_id(0, "node"),
                checkout_fingerprint: record.checkout_fingerprint.clone(),
                source: record.worktree.clone(),
                root: record.worktree.clone(),
                worktree: record.worktree.clone(),
                common_dir: record.worktree.clone(),
                reason: bridge_worktree::custody::PreservationReasonV1::NodeFailure,
                created_wall_ms: 1_700_000_000_000,
                recovery_locator: bridge_worktree::custody::RecoveryLocatorV1::RegisteredWorktree {},
            });
        }
        std::fs::write(
            bridge_worktree::custody::custody_record_path(&canonical),
            record.encode_canonical().unwrap(),
        )
        .unwrap();
    }

    /// R-4's closure. A V3 record is Evidence, associates with its sibling checkout, and its
    /// holder state comes from the CUSTODY STATE (there is no lease to probe for one).
    ///
    /// Discriminates a report that knows only `.meta.json`: the record would show as
    /// `Unclassified` — bridge-owned bytes presented as garbage — and the checkout's `run_lease`
    /// would read `Unknown`, i.e. "nobody holds this", for a live protected checkout.
    #[test]
    fn worktree_root_reports_a_v3_custody_record_as_evidence_binding_its_sibling() {
        let td = tempfile::tempdir().unwrap();
        let root = td.path().join("wt-root");
        let wt = root.join("ownr-run7-abc");
        std::fs::create_dir_all(&wt).unwrap();
        let common = td.path().join("source-repo/.git/worktrees/ownr-run7-abc");
        std::fs::create_dir_all(&common).unwrap();
        std::fs::write(wt.join(".git"), format!("gitdir: {}\n", common.display())).unwrap();
        v3_custody_record(
            &wt,
            bridge_worktree::custody::WorktreeCustodyStateV1::LiveProtected {},
        );

        let mut notes = Vec::new();
        let items = scan_worktree_root(&root, &mut notes);

        let record = items
            .iter()
            .find(|i| i.path.ends_with(".custody.v1.json"))
            .expect("the V3 record must be reported");
        assert_eq!(record.class, PayloadClass::Evidence);
        assert_eq!(record.consumers.run_lease, HolderState::Held);
        assert_eq!(
            record.note.as_deref(),
            Some("R2f1b worktree custody record")
        );
        let checkout = items
            .iter()
            .find(|i| i.class == PayloadClass::SourceCheckout)
            .expect("the sibling checkout is still reported");
        assert_eq!(
            checkout.consumers.run_lease,
            HolderState::Held,
            "a live custody record holds its sibling checkout"
        );
        assert!(
            !items.iter().any(|i| i.class == PayloadClass::Unclassified),
            "nothing in a V3 root may surface as unclassified: {items:#?}"
        );
    }

    /// A preserved record still HOLDS: it awaits R2f2 disposition, so nothing may treat its
    /// checkout as reclaimable. Discriminates a holder mapping that reads any terminal custody
    /// state as free.
    #[test]
    fn a_preserved_custody_record_still_holds_its_checkout() {
        let td = tempfile::tempdir().unwrap();
        let root = td.path().join("wt-root");
        let wt = root.join("ownr-run7-abc");
        std::fs::create_dir_all(&wt).unwrap();
        v3_custody_record(
            &wt,
            bridge_worktree::custody::WorktreeCustodyStateV1::PreservationPrepared {},
        );

        let mut notes = Vec::new();
        let items = scan_worktree_root(&root, &mut notes);

        let record = items
            .iter()
            .find(|i| i.path.ends_with(".custody.v1.json"))
            .unwrap();
        assert_eq!(record.consumers.run_lease, HolderState::Held);
        assert_ne!(record.consumers.run_lease, HolderState::Free);
    }

    /// 2b1 dual-review ledger item (opus S-10): `<root>/.custody-locks` is coordination state, not
    /// a checkout. Discriminates the pre-2b2 behaviour where it fell through to the directory
    /// branch, failed the linked-worktree shape check, and surfaced Unclassified on every report.
    #[test]
    fn the_custody_lock_directory_is_classified_and_never_unclassified() {
        let td = tempfile::tempdir().unwrap();
        let root = td.path().join("wt-root");
        std::fs::create_dir_all(&root).unwrap();
        let cell = bridge_worktree::custody_lock::try_acquire_custody_lock_in(
            &root,
            &bridge_core::execution_policy::WorktreeCustodyIdV1::parse(format!(
                "custody-{}",
                "b".repeat(64)
            ))
            .unwrap(),
        )
        .unwrap();

        let mut notes = Vec::new();
        let items = scan_worktree_root(&root, &mut notes);

        let locks = items
            .iter()
            .find(|i| i.path.ends_with(".custody-locks"))
            .expect("the lock directory must be reported");
        assert_eq!(locks.class, PayloadClass::Evidence);
        assert_eq!(locks.checkout_kind, None);
        assert!(
            !items.iter().any(|i| i.class == PayloadClass::Unclassified),
            "{items:#?}"
        );
        drop(cell);
    }

    /// V2 output is unchanged: a root with no V3 artifacts produces the same items it always did.
    /// The positive control for every assertion above — without it, the tests could pass against a
    /// scanner that reclassified the legacy sidecar too.
    #[test]
    fn a_v2_only_worktree_root_reports_exactly_what_it_did_before() {
        let td = tempfile::tempdir().unwrap();
        let root = td.path().join("wt-root");
        let wt = root.join("ownr-run7-abc");
        std::fs::create_dir_all(&wt).unwrap();
        let lease_dir = td.path().join("leases");
        let lease = bridge_core::liveness::acquire_lease_in(&lease_dir, "run7").unwrap();
        let sidecar = bridge_worktree::provider_path::WorktreeSidecar {
            canonical_source: "/repo".into(),
            common_dir: "/repo/.git".into(),
            worktree_path: display_path(&wt),
            owner: "ownr".into(),
            run_id: "run7".into(),
            host: "h1".into(),
            lease: lease.path().to_string_lossy().into_owned(),
        };
        bridge_worktree::provider_path::write_sidecar(&sidecar).unwrap();

        let mut notes = Vec::new();
        let items = scan_worktree_root(&root, &mut notes);

        let ev = items
            .iter()
            .find(|i| i.class == PayloadClass::Evidence)
            .expect("the legacy sidecar is Evidence, exactly as before");
        assert!(ev.path.ends_with(".meta.json"));
        assert_eq!(ev.note.as_deref(), Some("worktree custody sidecar"));
        assert_eq!(ev.consumers.run_lease, HolderState::Held);
        assert!(!items.iter().any(|i| i.path.ends_with(".custody.v1.json")));
    }

    /// R8a's red test. A checkout named by BOTH records — the state 2b1's deletion gate produces
    /// on every refusal — must report `Held` when either record holds it.
    ///
    /// Discriminates the shipped plain-insert: `.custody.v1.json` sorts before `.meta.json`, so a
    /// live custody record's `Held` was overwritten by a free legacy sidecar's `Free`, and a
    /// protected checkout was reported as having no live consumer. The lease here is deliberately
    /// FREE (no holder), so only the merge rule can produce `Held`.
    #[test]
    fn a_live_custody_record_holds_its_checkout_even_beside_a_free_legacy_sidecar() {
        let td = tempfile::tempdir().unwrap();
        let root = td.path().join("wt-root");
        let wt = root.join("ownr-run7-abc");
        std::fs::create_dir_all(&wt).unwrap();
        v3_custody_record(
            &wt,
            bridge_worktree::custody::WorktreeCustodyStateV1::LiveProtected {},
        );
        // A legacy sidecar naming a lease nobody holds — `probe_lock_path` answers Free/Unknown.
        let sidecar = bridge_worktree::provider_path::WorktreeSidecar {
            canonical_source: "/repo".into(),
            common_dir: "/repo/.git".into(),
            worktree_path: display_path(&wt),
            owner: "ownr".into(),
            run_id: "run7".into(),
            host: "h1".into(),
            lease: td
                .path()
                .join("leases/never-held.lock")
                .display()
                .to_string(),
        };
        std::fs::create_dir_all(td.path().join("leases")).unwrap();
        std::fs::write(&sidecar.lease, b"").unwrap();
        bridge_worktree::provider_path::write_sidecar(&sidecar).unwrap();
        assert!(
            wt.with_file_name("ownr-run7-abc.custody.v1.json")
                < wt.with_file_name("ownr-run7-abc.meta.json"),
            "the custody record must sort FIRST, so a plain insert really is overwritten"
        );

        let mut notes = Vec::new();
        let items = scan_worktree_root(&root, &mut notes);

        let checkout = items
            .iter()
            .find(|i| i.class == PayloadClass::SourceCheckout || i.path == display_path(&wt))
            .expect("the checkout is reported");
        assert_eq!(
            checkout.consumers.run_lease,
            HolderState::Held,
            "a live custody record must hold the checkout regardless of a free sidecar beside it"
        );
    }
}
