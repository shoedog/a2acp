//! `a2a-bridge merge <id>` — land an Approved run's commit into its source_repo, re-authored to the
//! operator, via `git commit-tree` + `git push --force-with-lease`. Exact-base Mode A is the default;
//! ADR-0040 adds an explicit current-target integration mode for reviewed parallel siblings.
//! Pure core here; impure git ops + the CLI orchestrator follow. See ADR-0027 and ADR-0040.

use crate::implement::{
    commit_message, current_branch, head_sha, is_worktree_dirty, pin_prefix_argv, run_git,
};
use crate::implement_resume::{
    acquire_operation_lock, load_checkpoint, ImplementCheckpoint, ImplementPhase, SCHEMA_VERSION,
};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

/// Canonicalize bridge provenance, even when the supplied message contains forged/duplicate trailers.
fn provenance_message(message: &str, run: &str, original: &str) -> String {
    let body = message
        .lines()
        .filter(|line| {
            let key = line.trim_start().split_once(':').map(|(key, _)| key.trim());
            !key.is_some_and(|key| {
                key.eq_ignore_ascii_case("A2A-Run") || key.eq_ignore_ascii_case("A2A-Original")
            })
        })
        .collect::<Vec<_>>()
        .join("\n");
    let body = if body.trim().is_empty() {
        format!("Implement run {run}")
    } else {
        body.trim_end().to_owned()
    };
    format!("{body}\n\nA2A-Run: {run}\nA2A-Original: {original}\n")
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LandingEvidence {
    /// Only `confirmed-provenance` authorizes automatic merged reap.
    pub confidence: &'static str,
    pub matched_commit: Option<String>,
    /// Source main snapshot checked by the probes, for the final deletion boundary recheck.
    pub main_oid: Option<String>,
    pub reason: String,
}
impl LandingEvidence {
    pub fn confirmed(&self) -> bool {
        self.confidence == "confirmed-provenance"
    }
}

fn patch_id(repo: &Path, args: &[&str], in_clone: bool) -> Result<Option<String>, String> {
    use std::io::Write;
    use std::process::{Command, Stdio};
    let diff = if in_clone {
        crate::implement::run_clone_git(repo, args)
    } else {
        run_git(Some(repo), args)
    }
    .map_err(|_| "patch probe failed")?;
    if !diff.status.success() {
        return Err("patch probe failed".into());
    }
    let mut child = Command::new("git")
        .args(["patch-id", "--stable"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| "patch-id spawn failed")?;
    // Read stdout concurrently: patch-id produces one small line for a single diff.
    child
        .stdin
        .take()
        .ok_or("patch-id stdin missing")?
        .write_all(&diff.stdout)
        .map_err(|_| "patch-id input failed")?;
    let out = child.wait_with_output().map_err(|_| "patch-id failed")?;
    if !out.status.success() {
        return Err("patch-id failed".into());
    }
    Ok(String::from_utf8(out.stdout)
        .map_err(|_| "invalid patch-id")?
        .split_whitespace()
        .next()
        .map(str::to_owned))
}

/// Inspect only source `main`. Equivalence is useful adjudication evidence, never run provenance.
/// Every probe is pinned to one main OID and the live ref is rechecked before returning.
pub fn classify_landing(
    clone: &Path,
    ck: &ImplementCheckpoint,
    head: &str,
    lookback: u32,
) -> LandingEvidence {
    let probe = || -> Result<LandingEvidence, String> {
        let source = &ck.source_repo;
        let query = |args: &[&str]| -> Result<String, String> {
            let out = run_git(Some(source), args).map_err(|_| "source git probe failed")?;
            if !out.status.success() {
                return Err("source git probe failed".into());
            }
            String::from_utf8(out.stdout).map_err(|_| "invalid source git output".into())
        };
        let main = query(&["rev-parse", "--verify", "refs/heads/main^{commit}"])?
            .trim()
            .to_owned();
        let history = query(&["log", "--format=%H", &main])?;
        let mut result = LandingEvidence {
            confidence: "absent",
            matched_commit: None,
            main_oid: None,
            reason: "no landing evidence on source main".into(),
        };
        // Search the entire reachable history for exact trailers. Bodies are not trailers.
        let trailer_log = query(&["log", "--format=%H%x00%(trailers:only,unfold)%x00", &main])?;
        let mut records = trailer_log.split('\0');
        while let Some(commit) = records.next() {
            let commit = commit.trim();
            if commit.is_empty() {
                continue;
            }
            let trailers = records.next().ok_or("malformed trailer history")?;
            let runs: Vec<_> = trailers
                .lines()
                .filter_map(|l| l.split_once(':'))
                .filter(|(k, _)| k.eq_ignore_ascii_case("A2A-Run"))
                .map(|(_, v)| v.trim())
                .collect();
            let originals: Vec<_> = trailers
                .lines()
                .filter_map(|l| l.split_once(':'))
                .filter(|(k, _)| k.eq_ignore_ascii_case("A2A-Original"))
                .map(|(_, v)| v.trim())
                .collect();
            if runs == [ck.task_id.as_str()] && originals == [head] {
                result = LandingEvidence {
                    confidence: "confirmed-provenance",
                    matched_commit: Some(commit.into()),
                    main_oid: None,
                    reason: "exact A2A-Run/A2A-Original trailer pair reachable from main".into(),
                };
                break;
            }
        }
        if !result.confirmed() {
            let run_patch = patch_id(
                clone,
                &[
                    "diff",
                    "--no-ext-diff",
                    "--no-textconv",
                    "--binary",
                    "--no-renames",
                    "--no-color",
                    &ck.base_commit,
                    head,
                    "--",
                ],
                true,
            )?;
            if let Some(run_patch) = run_patch {
                for commit in history.lines().take(lookback as usize) {
                    if patch_id(
                        source,
                        &[
                            "show",
                            "--format=",
                            "--root",
                            "--no-ext-diff",
                            "--no-textconv",
                            "--binary",
                            "--no-renames",
                            "--no-color",
                            commit,
                            "--",
                        ],
                        false,
                    )?
                    .as_ref()
                        == Some(&run_patch)
                    {
                        result = LandingEvidence {
                            confidence: "high-patch-equivalence",
                            matched_commit: Some(commit.into()),
                            main_oid: None,
                            reason: "equal stable patch IDs; run provenance unproved".into(),
                        };
                        break;
                    }
                }
            }
        }
        if result.confidence == "absent" {
            let changed = clone_git_str(
                clone,
                &[
                    "diff",
                    "--no-renames",
                    "--name-only",
                    "-z",
                    &ck.base_commit,
                    head,
                    "--",
                ],
            )?;
            let paths: Vec<_> = changed.split('\0').filter(|p| !p.is_empty()).collect();
            let timestamp: i64 = clone_git_str(clone, &["show", "-s", "--format=%ct", head])?
                .parse()
                .map_err(|_| "invalid clone timestamp")?;
            if !paths.is_empty() {
                for commit in history.lines().take(lookback as usize) {
                    let time: i64 = query(&["show", "-s", "--format=%ct", commit])?
                        .trim()
                        .parse()
                        .map_err(|_| "invalid source timestamp")?;
                    if !(timestamp..=timestamp.saturating_add(14 * 86400)).contains(&time) {
                        continue;
                    }
                    let mut matches = true;
                    for path in &paths {
                        // Empty output represents a deletion; compare modes and object IDs as well.
                        let expected = clone_git_str(
                            clone,
                            &["--literal-pathspecs", "ls-tree", "-z", head, "--", path],
                        )?;
                        let actual =
                            query(&["--literal-pathspecs", "ls-tree", "-z", commit, "--", path])?;
                        if actual.trim() != expected.trim() {
                            matches = false;
                            break;
                        }
                    }
                    if matches {
                        result = LandingEvidence { confidence: "low-content-equivalence", matched_commit: Some(commit.into()), main_oid: None, reason: "all changed paths match together within 14 days; run provenance unproved".into() };
                        break;
                    }
                }
            }
        }
        if query(&["rev-parse", "refs/heads/main"])?.trim() != main {
            return Err("source main moved during classification".into());
        }
        if result.confidence == "absent" && history.lines().count() > lookback as usize {
            result.confidence = "unknown";
            result.reason = format!("no exact trailers; alternative equivalence search exhausted the {lookback}-commit window");
        }
        result.main_oid = Some(main);
        Ok(result)
    };
    probe().unwrap_or_else(|reason| LandingEvidence {
        confidence: "unknown",
        matched_commit: None,
        main_oid: None,
        reason,
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OperatorIdent {
    pub name: String,
    pub email: String,
}

/// Validated `[merge]` config.
#[derive(Debug, Clone)]
pub struct MergeConfig {
    pub target_ref: Option<String>,
    pub author: Option<OperatorIdent>,
}

/// How an Approved run is composed with the destination branch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MergeMode {
    /// ADR-0027: the destination must still equal the run's immutable base commit.
    ExactBase,
    /// ADR-0040: three-way integrate the run delta onto the fetched current destination, then CAS it.
    IntegrateCurrent,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MergePlan {
    /// Proceed to land on `target` (a SHORT branch name).
    Merge { target: String },
    /// Recoverable refusal (LoopStopped w/o --force; unresolvable target). `--force` MAY help.
    Refuse(String),
    /// Non-overridable refusal (non-terminal phase, or current_commit==None). `--force` CANNOT help.
    RefuseHard(String),
}

/// Mode-independent phase gate. Refuses `current_commit==None` HERE so the impure clone-HEAD preflight
/// (which compares `head_sha` against `current_commit`) never sees a `None` and mis-reports "HEAD moved".
pub fn decide_merge(
    phase: ImplementPhase,
    has_commit: bool,
    force: bool,
    target: &Result<String, String>,
) -> MergePlan {
    use ImplementPhase::*;
    match phase {
        Approved => {
            if !has_commit {
                return MergePlan::RefuseHard(
                    "run is Approved but has no commit — nothing to merge".into(),
                );
            }
            match target {
                Ok(t) => MergePlan::Merge { target: t.clone() },
                Err(e) => MergePlan::Refuse(format!("cannot resolve target: {e}")),
            }
        }
        LoopStopped => {
            if !has_commit {
                return MergePlan::RefuseHard(
                    "run finished without a commit — nothing to merge".into(),
                );
            }
            if !force {
                return MergePlan::Refuse(
                    "run finished but was not Approved — pass --force to merge anyway".into(),
                );
            }
            match target {
                Ok(t) => MergePlan::Merge { target: t.clone() },
                Err(e) => MergePlan::Refuse(format!("cannot resolve target: {e}")),
            }
        }
        Abort | NoCommitClean | NoCommitDirty | CommitFailed => {
            MergePlan::RefuseHard("run finished without a commit — nothing to merge".into())
        }
        Cloned | EditStarted | FirstCommitCreated | InLoop => MergePlan::RefuseHard(
            "run is not finished — `resume` it first (--force cannot override)".into(),
        ),
    }
}

/// Best-effort, PURE branch-name pre-check (NOT full check-ref-format parity; git is authoritative at
/// the push boundary). Rejects only what the STRING decides.
fn valid_branch_name(s: &str) -> bool {
    if s.is_empty() || s == "HEAD" {
        return false;
    }
    if s.starts_with('-') || s.ends_with('/') || s.ends_with('.') || s.ends_with(".lock") {
        return false;
    }
    if s.starts_with("refs/") || s.starts_with("origin/") {
        return false;
    }
    if s.contains("..") {
        return false;
    }
    // raw 40-hex SHA
    if s.len() == 40 && s.chars().all(|c| c.is_ascii_hexdigit()) {
        return false;
    }
    // forbidden chars (space, control, and git's special set) + "@{" + backslash
    if s.chars()
        .any(|c| c.is_whitespace() || c.is_control() || "~^:?*[\\".contains(c))
    {
        return false;
    }
    if s.contains("@{") {
        return false;
    }
    // no path component starting with '.' or empty
    if s.split('/')
        .any(|seg| seg.starts_with('.') || seg.is_empty())
    {
        return false;
    }
    true
}

/// Precedence: --onto > [merge].target_ref > checkpoint.base_ref. Returns a SHORT branch name.
pub fn resolve_target(
    cli_onto: Option<&str>,
    cfg: Option<&str>,
    base_ref: Option<&str>,
) -> Result<String, String> {
    let raw = cli_onto
        .or(cfg)
        .or(base_ref)
        .ok_or_else(|| "no target — pass --onto <branch> or set [merge].target_ref".to_string())?;
    if !valid_branch_name(raw) {
        return Err(format!(
            "invalid branch name {raw:?} (pass a plain branch like `main`)"
        ));
    }
    Ok(raw.to_string())
}

// ─── Impure git ops (temp-repo tested, docker-free) ──────────────────────────

/// Run git in `cwd`, returning trimmed stdout on success or a formatted Err.
fn git_str(cwd: &Path, args: &[&str]) -> Result<String, String> {
    let out = run_git(Some(cwd), args).map_err(|e| format!("git {}: {e}", args.join(" ")))?;
    if !out.status.success() {
        return Err(format!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn clone_git_str(clone: &Path, args: &[&str]) -> Result<String, String> {
    crate::implement::guard_clone(clone)?;
    let out = crate::implement::run_clone_git(clone, args).map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(format!(
            "git failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// `commit-tree` `current_commit`'s tree over `base_commit` as `op` (author==committer, same fresh date).
/// Reuses the identity-free pin prefix; identity via `GIT_*` env; message on stdin (`-F -`) so a multi-line
/// body survives. Does NOT move the clone's branch (retry-safe).
fn author_tree_commit(
    clone: &Path,
    tree: &str,
    parent_commit: &str,
    msg: &str,
    op: &OperatorIdent,
) -> Result<String, String> {
    crate::implement::guard_clone(clone)?;
    let t = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| e.to_string())?
        .as_secs();
    let date = format!("{t} +0000");
    let mut argv = pin_prefix_argv(&clone.to_string_lossy());
    argv.extend([
        "commit-tree".into(),
        tree.into(),
        "-p".into(),
        parent_commit.into(),
        "-F".into(),
        "-".into(),
    ]);

    let mut cmd = crate::implement::clone_git_command(clone).map_err(|e| e.to_string())?;
    cmd.args(&argv)
        .env("GIT_AUTHOR_NAME", &op.name)
        .env("GIT_AUTHOR_EMAIL", &op.email)
        .env("GIT_AUTHOR_DATE", &date)
        .env("GIT_COMMITTER_NAME", &op.name)
        .env("GIT_COMMITTER_EMAIL", &op.email)
        .env("GIT_COMMITTER_DATE", &date)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("git commit-tree spawn: {e}"))?;
    {
        use std::io::Write;
        child
            .stdin
            .take()
            .unwrap()
            .write_all(msg.as_bytes())
            .map_err(|e| format!("commit-tree stdin: {e}"))?;
    }
    let out = child
        .wait_with_output()
        .map_err(|e| format!("commit-tree wait: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "git commit-tree failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// `commit-tree` `current_commit`'s tree over `base_commit` as `op`. Kept as the exact-base primitive
/// and as a directly tested contract from ADR-0027.
pub fn reauthor_commit(
    clone: &Path,
    current_commit: &str,
    base_commit: &str,
    msg: &str,
    op: &OperatorIdent,
) -> Result<String, String> {
    author_tree_commit(
        clone,
        &format!("{current_commit}^{{tree}}"),
        base_commit,
        msg,
        op,
    )
}

/// Create an unreachable, deterministic side commit for the Git 2.39-compatible merge-tree
/// invocation below. This deliberately does not use the operator identity or move any ref.
fn synthetic_side_commit(
    clone: &Path,
    tree_commit: &str,
    base_commit: &str,
) -> Result<String, String> {
    crate::implement::guard_clone(clone)?;
    let mut argv = pin_prefix_argv(&clone.to_string_lossy());
    argv.extend([
        "commit-tree".into(),
        format!("{tree_commit}^{{tree}}"),
        "-p".into(),
        base_commit.into(),
        "-F".into(),
        "-".into(),
    ]);

    let mut child = crate::implement::clone_git_command(clone)
        .map_err(|e| e.to_string())?
        .args(&argv)
        .env("GIT_AUTHOR_NAME", "a2a-bridge synthetic merge-tree")
        .env("GIT_AUTHOR_EMAIL", "synthetic@a2a-bridge.invalid")
        .env("GIT_AUTHOR_DATE", "946684800 +0000")
        .env("GIT_COMMITTER_NAME", "a2a-bridge synthetic merge-tree")
        .env("GIT_COMMITTER_EMAIL", "synthetic@a2a-bridge.invalid")
        .env("GIT_COMMITTER_DATE", "946684800 +0000")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("git commit-tree spawn: {e}"))?;
    {
        use std::io::Write;
        child
            .stdin
            .take()
            .expect("piped commit-tree stdin")
            .write_all(b"a2a-bridge synthetic merge-tree side\n")
            .map_err(|e| format!("commit-tree stdin: {e}"))?;
    }
    let out = child
        .wait_with_output()
        .map_err(|e| format!("commit-tree wait: {e}"))?;
    if !out.status.success() {
        let detail = bounded_git_detail(&out.stdout, &out.stderr);
        return Err(format!(
            "git commit-tree failed{}",
            if detail.is_empty() {
                String::new()
            } else {
                format!(": {detail}")
            }
        ));
    }
    let synthetic = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if synthetic.is_empty() {
        return Err("git commit-tree returned no commit object".into());
    }
    Ok(synthetic)
}

#[derive(Debug, PartialEq, Eq)]
pub enum IntegrationError {
    Conflict(String),
    Other(String),
}

/// Fetch the destination ref into the quarantine clone and return the exact commit fetched. This updates
/// only clone-local FETCH_HEAD; the caller's per-run operation lock excludes another resume/merge on it.
fn fetch_target_commit(clone: &Path, source_repo: &Path, target: &str) -> Result<String, String> {
    crate::implement::guard_clone(clone)?;
    let source = source_repo.to_string_lossy().to_string();
    let target_ref = format!("refs/heads/{target}");
    let out = crate::implement::run_clone_git(
        clone,
        &["fetch", "--no-tags", "--force", &source, &target_ref],
    )
    .map_err(|e| format!("git fetch current target: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "git fetch current target failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    clone_git_str(clone, &["rev-parse", "--verify", "FETCH_HEAD^{commit}"])
}

fn require_ancestor(clone: &Path, ancestor: &str, descendant: &str) -> Result<(), String> {
    crate::implement::guard_clone(clone)?;
    let out = crate::implement::run_clone_git(
        clone,
        &["merge-base", "--is-ancestor", ancestor, descendant],
    )
    .map_err(|e| format!("git merge-base: {e}"))?;
    if out.status.success() {
        return Ok(());
    }
    if out.status.code() == Some(1) {
        return Err(format!(
            "destination {:.12} does not descend from the run base {:.12}",
            descendant, ancestor
        ));
    }
    Err(format!(
        "git merge-base failed: {}",
        String::from_utf8_lossy(&out.stderr).trim()
    ))
}

fn bounded_git_detail(stdout: &[u8], stderr: &[u8]) -> String {
    let raw = if stderr.is_empty() { stdout } else { stderr };
    let text = String::from_utf8_lossy(raw);
    let mut detail: String = text.chars().take(4096).collect();
    if text.chars().count() > 4096 {
        detail.push('…');
    }
    detail.trim().to_string()
}

/// Three-way integrate the immutable run delta (`base_commit..current_commit`) onto `target_commit`
/// without moving a branch or worktree. Returns the resulting tree object when clean.
pub fn integrate_run_tree(
    clone: &Path,
    base_commit: &str,
    target_commit: &str,
    current_commit: &str,
) -> Result<String, IntegrationError> {
    crate::implement::guard_clone(clone).map_err(IntegrationError::Other)?;
    require_ancestor(clone, base_commit, target_commit).map_err(IntegrationError::Other)?;
    let synthetic_target = synthetic_side_commit(clone, target_commit, base_commit)
        .map_err(|e| IntegrationError::Other(format!("synthetic target commit: {e}")))?;
    let synthetic_current = synthetic_side_commit(clone, current_commit, base_commit)
        .map_err(|e| IntegrationError::Other(format!("synthetic current commit: {e}")))?;
    let out = crate::implement::run_clone_git(
        clone,
        &[
            "merge-tree",
            "--write-tree",
            "--name-only",
            &synthetic_target,
            &synthetic_current,
        ],
    )
    .map_err(|e| IntegrationError::Other(format!("git merge-tree spawn: {e}")))?;
    if !out.status.success() {
        let detail = bounded_git_detail(&out.stdout, &out.stderr);
        if out.status.code() == Some(1) {
            return Err(IntegrationError::Conflict(detail));
        }
        return Err(IntegrationError::Other(format!(
            "git merge-tree failed{}",
            if detail.is_empty() {
                String::new()
            } else {
                format!(": {detail}")
            }
        )));
    }
    let tree = String::from_utf8_lossy(&out.stdout)
        .lines()
        .next()
        .unwrap_or_default()
        .trim()
        .to_string();
    if tree.is_empty() || clone_git_str(clone, &["cat-file", "-t", &tree]).as_deref() != Ok("tree")
    {
        return Err(IntegrationError::Other(
            "git merge-tree returned no valid tree object".into(),
        ));
    }
    Ok(tree)
}

/// Guard the commit-tree graft against a corrupted/unexpected clone (the bridge OWNS this dir — integrity,
/// not adversarial defense): both objects exist AND base is an ancestor of current.
pub fn check_clone_shape(
    clone: &Path,
    base_commit: &str,
    current_commit: &str,
) -> Result<(), String> {
    crate::implement::guard_clone(clone)?;
    for obj in [base_commit, current_commit] {
        clone_git_str(clone, &["cat-file", "-e", &format!("{obj}^{{commit}}")])
            .map_err(|_| format!("clone object {obj} missing — inspect {}", clone.display()))?;
    }
    let out = crate::implement::run_clone_git(
        clone,
        &["merge-base", "--is-ancestor", base_commit, current_commit],
    )
    .map_err(|e| format!("git merge-base: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "clone history unexpected: base {base_commit:.12} is not an ancestor of the run commit \
             {current_commit:.12} — inspect {}",
            clone.display()
        ));
    }
    Ok(())
}

/// Push failure classification. `CheckedOutTarget` is owned by the pre-push `source_head` preflight (it
/// returns before the push), so `push_landing` never constructs it (plan-review FIX 2). The
/// `denyCurrentBranch` backstop race maps to `Other` (its stderr varies by git version).
#[derive(Debug)]
pub enum PushError {
    StaleLease,
    Other(String),
}

/// Push `reauthored` into `source_repo` as `refs/heads/{target}` with
/// `--force-with-lease=refs/heads/{target}:{base_commit}` (FF iff target is still at base). On failure,
/// classify by OBSERVABLE post-failure ref state — never by parsing stderr.
pub fn push_landing(
    clone: &Path,
    source_repo: &Path,
    reauthored: &str,
    target: &str,
    base_commit: &str,
) -> Result<(), PushError> {
    crate::implement::guard_clone(clone).map_err(PushError::Other)?;
    let refspec = format!("{reauthored}:refs/heads/{target}");
    let lease = format!("--force-with-lease=refs/heads/{target}:{base_commit}");
    let src = source_repo.to_string_lossy().to_string();
    let out = crate::implement::run_clone_git(clone, &["push", &src, &refspec, &lease])
        .map_err(|e| PushError::Other(format!("git push spawn: {e}")))?;
    if out.status.success() {
        return Ok(());
    }
    // Classify from the source's current ref, not stderr.
    let now = run_git(
        Some(source_repo),
        &[
            "rev-parse",
            "-q",
            "--verify",
            &format!("refs/heads/{target}"),
        ],
    )
    .ok()
    .filter(|o| o.status.success())
    .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string());
    match now {
        Some(sha) if sha == reauthored => Ok(()), // raced to our value (idempotent re-push)
        Some(sha) if sha != base_commit => Err(PushError::StaleLease), // target moved off the lease base
        _ => Err(PushError::Other(
            String::from_utf8_lossy(&out.stderr).trim().to_string(),
        )),
    }
}

/// Establish a linearization point for an already-integrated tree without moving the destination ref. A
/// same-value `git push` is not a compare-and-swap: after advertisement Git may send no update command at all.
/// An `update-ref` verify transaction instead locks the local destination and compares it to `expected_commit`.
fn verify_target_unchanged(
    source_repo: &Path,
    target: &str,
    expected_commit: &str,
) -> Result<(), PushError> {
    let transaction =
        format!("start\nverify refs/heads/{target} {expected_commit}\nprepare\ncommit\n");
    let mut child = std::process::Command::new("git")
        .arg("-C")
        .arg(source_repo)
        .args(["update-ref", "--stdin"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| PushError::Other(format!("git update-ref spawn: {e}")))?;
    {
        use std::io::Write;
        child
            .stdin
            .take()
            .expect("piped update-ref stdin")
            .write_all(transaction.as_bytes())
            .map_err(|e| PushError::Other(format!("git update-ref stdin: {e}")))?;
    }
    let out = child
        .wait_with_output()
        .map_err(|e| PushError::Other(format!("git update-ref wait: {e}")))?;
    if out.status.success() {
        return Ok(());
    }

    // Classify from the source's current ref, not version-specific update-ref stderr.
    let observed = run_git(
        Some(source_repo),
        &[
            "rev-parse",
            "-q",
            "--verify",
            &format!("refs/heads/{target}"),
        ],
    )
    .map_err(|e| PushError::Other(format!("git rev-parse after update-ref failure: {e}")))?;
    if !observed.status.success()
        || String::from_utf8_lossy(&observed.stdout).trim() != expected_commit
    {
        return Err(PushError::StaleLease);
    }
    Err(PushError::Other(
        String::from_utf8_lossy(&out.stderr).trim().to_string(),
    ))
}

/// source_repo git config user.name+user.email, or a `[merge]` override. Fail loud if EITHER half is
/// missing and there is no override.
pub fn operator_from(
    repo: &Path,
    cfg_override: Option<&OperatorIdent>,
) -> Result<OperatorIdent, String> {
    if let Some(o) = cfg_override {
        return Ok(o.clone());
    }
    let unset = |half: &str| {
        format!(
            "operator identity unset: `git -C {} config user.{half}` is empty — set it or add [merge] author_name/author_email",
            repo.display()
        )
    };
    let name = git_str(repo, &["config", "--get", "user.name"]).map_err(|_| unset("name"))?;
    let email = git_str(repo, &["config", "--get", "user.email"]).map_err(|_| unset("email"))?;
    if name.is_empty() || email.is_empty() {
        return Err(format!(
            "operator identity unset in {} — set user.name/user.email or [merge] author_*",
            repo.display()
        ));
    }
    Ok(OperatorIdent { name, email })
}

/// The source's checked-out branch (short name), or None when detached / unborn.
pub fn source_head(repo: &Path) -> Result<Option<String>, String> {
    let out = run_git(Some(repo), &["symbolic-ref", "--short", "-q", "HEAD"])
        .map_err(|e| format!("git symbolic-ref: {e}"))?;
    if out.status.success() {
        Ok(Some(
            String::from_utf8_lossy(&out.stdout).trim().to_string(),
        ))
    } else {
        Ok(None) // detached HEAD: symbolic-ref -q exits 1
    }
}

pub fn is_bare(repo: &Path) -> Result<bool, String> {
    Ok(git_str(repo, &["rev-parse", "--is-bare-repository"])? == "true")
}

/// Guarded delete: only when `clone` canonicalizes to exactly `<root>/.a2a-implement/<basename>`, has a
/// `.git`, is under `root`, and is not the source. NEVER a bare rm of an arbitrary path.
pub fn reap_clone(clone: &Path, src: &Path, root: &Path) -> Result<(), String> {
    let croot = std::fs::canonicalize(root).map_err(|e| format!("canonicalize root: {e}"))?;
    let cclone = std::fs::canonicalize(clone).map_err(|e| format!("canonicalize clone: {e}"))?;
    let csrc = std::fs::canonicalize(src).map_err(|e| format!("canonicalize src: {e}"))?;
    let id = cclone
        .file_name()
        .and_then(|s| s.to_str())
        .ok_or("clone has no basename")?;
    let expected = croot.join(".a2a-implement").join(id);
    if cclone != expected
        || !cclone.join(".git").exists()
        || !cclone.starts_with(&croot)
        || cclone == csrc
    {
        return Err(format!(
            "refusing to reap unexpected path {} (keeping it)",
            cclone.display()
        ));
    }
    std::fs::remove_dir_all(&cclone).map_err(|e| format!("remove clone {}: {e}", cclone.display()))
}

/// The CLI exit semantics. `.code()` maps to the process exit code (see the spec's exit table).
pub enum MergeOutcome {
    Merged,           // 0
    UsageOrPreflight, // 1
    Unlanded,         // 3 (Approved but couldn't land / preflight-at-merge)
}
impl MergeOutcome {
    pub fn code(&self) -> i32 {
        match self {
            Self::Merged => 0,
            Self::UsageOrPreflight => 1,
            Self::Unlanded => 3,
        }
    }
}

/// Host composition invoked after either successful landing, under the operation lock.
pub struct MergeHooks<'a> {
    pub custody: &'a crate::custody_push::CustodyConfig,
    pub evidence: &'a crate::evidence::EvidenceConfig,
    pub post_landing: &'a mut dyn FnMut(&Path, &ImplementCheckpoint),
}

/// Shared core: lock, validate, gate, preflight, re-author, push, reap. Prints user-facing lines itself.
/// Both `merge_cmd` (after `resolve_clone`) and `implement --merge` (with a known clone) call this.
pub fn merge_clone(
    mcfg: Option<&MergeConfig>,
    clone: &Path,
    root: &Path,
    onto: Option<&str>,
    force: bool,
    mode: MergeMode,
    hooks: Option<&mut MergeHooks<'_>>,
) -> MergeOutcome {
    let operation = match acquire_operation_lock(clone) {
        Ok(lock) => lock,
        Err(e) => {
            eprintln!("merge: {e}; clone kept at {}", clone.display());
            return MergeOutcome::UsageOrPreflight;
        }
    };
    merge_clone_with_operation_lock(operation, mcfg, clone, root, onto, force, mode, hooks)
}

/// Merge using a run-operation guard already acquired by the caller. `implement --resume --merge` transfers
/// its resume guard here, making the resume-to-merge transition gap-free without recursively acquiring the
/// non-blocking lock. The guard remains live through push and guarded reap, then drops on return.
#[allow(clippy::too_many_arguments)]
pub fn merge_clone_with_operation_lock(
    operation: bridge_core::liveness::PersistentLockGuard,
    mcfg: Option<&MergeConfig>,
    clone: &Path,
    root: &Path,
    onto: Option<&str>,
    force: bool,
    mode: MergeMode,
    hooks: Option<&mut MergeHooks<'_>>,
) -> MergeOutcome {
    merge_clone_with_operation_lock_inner(
        operation,
        mcfg,
        clone,
        root,
        onto,
        MergeOptions { force, mode },
        hooks,
        || {},
    )
}

#[derive(Clone, Copy)]
struct MergeOptions {
    force: bool,
    mode: MergeMode,
}

#[allow(clippy::too_many_arguments)]
fn merge_clone_with_operation_lock_inner<F>(
    _operation: bridge_core::liveness::PersistentLockGuard,
    mcfg: Option<&MergeConfig>,
    clone: &Path,
    root: &Path,
    onto: Option<&str>,
    options: MergeOptions,
    hooks: Option<&mut MergeHooks<'_>>,
    before_landing: F,
) -> MergeOutcome
where
    F: FnOnce(),
{
    let MergeOptions { force, mode } = options;
    if let Err(e) = crate::clone_config::check_clone_with_policy(clone) {
        eprintln!(
            "merge: clone config not allowed ({e}); clone kept at {}",
            clone.display()
        );
        return MergeOutcome::UsageOrPreflight;
    }
    let ck: ImplementCheckpoint = match load_checkpoint(clone) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("merge: {e}");
            return MergeOutcome::UsageOrPreflight;
        }
    };
    if ck.schema_version != SCHEMA_VERSION {
        eprintln!(
            "merge: checkpoint schema {} unsupported (expects {SCHEMA_VERSION}) — rebuild with a current run",
            ck.schema_version
        );
        return MergeOutcome::UsageOrPreflight;
    }
    let src = match std::fs::canonicalize(&ck.source_repo).ok().filter(|p| {
        run_git(Some(p), &["rev-parse", "--git-dir"])
            .map(|o| o.status.success())
            .unwrap_or(false)
    }) {
        Some(p) => p,
        None => {
            eprintln!(
                "merge: source repo {:?} gone/moved/not-a-git-repo — keeping clone",
                ck.source_repo
            );
            return MergeOutcome::UsageOrPreflight;
        }
    };
    let target = resolve_target(
        onto,
        mcfg.and_then(|m| m.target_ref.as_deref()),
        ck.base_ref.as_deref(),
    );
    match decide_merge(ck.phase, ck.current_commit.is_some(), force, &target) {
        MergePlan::Refuse(m) | MergePlan::RefuseHard(m) => {
            eprintln!("merge: {m}");
            MergeOutcome::UsageOrPreflight
        }
        MergePlan::Merge { target } => {
            // clone preflight (current_commit guaranteed Some here)
            let cur = ck.current_commit.as_deref().unwrap();
            match (
                current_branch(clone),
                head_sha(clone),
                is_worktree_dirty(clone),
            ) {
                (Ok(b), _, _) if b != ck.branch => {
                    eprintln!(
                        "merge: clone on wrong branch ({b} != {}) — inspect {}",
                        ck.branch,
                        clone.display()
                    );
                    return MergeOutcome::UsageOrPreflight;
                }
                (_, Ok(h), _) if h != cur => {
                    eprintln!(
                        "merge: clone HEAD moved off the checkpoint — inspect {}",
                        clone.display()
                    );
                    return MergeOutcome::UsageOrPreflight;
                }
                (_, _, Ok(true)) => {
                    eprintln!("merge: clone worktree dirty — inspect {}", clone.display());
                    return MergeOutcome::UsageOrPreflight;
                }
                (Err(e), _, _) | (_, Err(e), _) | (_, _, Err(e)) => {
                    eprintln!("merge: clone preflight: {e}");
                    return MergeOutcome::UsageOrPreflight;
                }
                _ => {}
            }
            if let Err(e) = check_clone_shape(clone, &ck.base_commit, cur) {
                eprintln!("merge: {e}");
                return MergeOutcome::UsageOrPreflight;
            }
            let op = match operator_from(&src, mcfg.and_then(|m| m.author.as_ref())) {
                Ok(o) => o,
                Err(e) => {
                    eprintln!("merge: {e}");
                    return MergeOutcome::UsageOrPreflight;
                }
            };
            // source no-touch preflight (best-effort; denyCurrentBranch is the atomic backstop)
            if !is_bare(&src).unwrap_or(false)
                && source_head(&src).ok().flatten().as_deref() == Some(target.as_str())
            {
                eprintln!(
                    "merge: '{target}' is checked out in {} — switch off it or pick another target (clone kept)",
                    src.display()
                );
                return MergeOutcome::Unlanded;
            }
            let (msg, _) = commit_message(
                ck.original_message.clone(),
                None,
                &ck.task_brief,
                &ck.task_brief,
            );
            let msg = provenance_message(&msg, &ck.task_id, cur);
            let (rt, expected_target, already_integrated) = match mode {
                MergeMode::ExactBase => {
                    let rt = match reauthor_commit(clone, cur, &ck.base_commit, &msg, &op) {
                        Ok(r) => r,
                        Err(e) => {
                            eprintln!("merge: {e}");
                            return MergeOutcome::Unlanded;
                        }
                    };
                    (rt, ck.base_commit.clone(), false)
                }
                MergeMode::IntegrateCurrent => {
                    let target_commit = match fetch_target_commit(clone, &src, &target) {
                        Ok(commit) => commit,
                        Err(e) => {
                            eprintln!(
                                "merge: cannot fetch current '{target}': {e}; clone kept at {}",
                                clone.display()
                            );
                            return MergeOutcome::Unlanded;
                        }
                    };
                    let tree = match integrate_run_tree(clone, &ck.base_commit, &target_commit, cur)
                    {
                        Ok(tree) => tree,
                        Err(IntegrationError::Conflict(detail)) => {
                            eprintln!(
                                "merge: integration conflict for '{target}'{}; clone kept at {}",
                                if detail.is_empty() {
                                    String::new()
                                } else {
                                    format!(": {detail}")
                                },
                                clone.display()
                            );
                            return MergeOutcome::Unlanded;
                        }
                        Err(IntegrationError::Other(e)) => {
                            eprintln!(
                                "merge: cannot integrate into current '{target}': {e}; clone kept at {}",
                                clone.display()
                            );
                            return MergeOutcome::Unlanded;
                        }
                    };
                    let target_tree = match clone_git_str(
                        clone,
                        &["rev-parse", &format!("{target_commit}^{{tree}}")],
                    ) {
                        Ok(tree) => tree,
                        Err(e) => {
                            eprintln!("merge: {e}; clone kept at {}", clone.display());
                            return MergeOutcome::Unlanded;
                        }
                    };
                    if tree == target_tree {
                        // A verify-only ref transaction supplies the no-op linearization point without an empty
                        // commit. A same-value push may send no update command after its ref advertisement.
                        (target_commit.clone(), target_commit, true)
                    } else {
                        let rt = match author_tree_commit(clone, &tree, &target_commit, &msg, &op) {
                            Ok(r) => r,
                            Err(e) => {
                                eprintln!("merge: {e}");
                                return MergeOutcome::Unlanded;
                            }
                        };
                        (rt, target_commit, false)
                    }
                }
            };
            let noop_evidence = already_integrated.then(|| classify_landing(clone, &ck, cur, 2000));
            before_landing();
            let landing = if already_integrated {
                verify_target_unchanged(&src, &target, &expected_target)
            } else {
                push_landing(clone, &src, &rt, &target, &expected_target)
            };
            match landing {
                Ok(()) => {
                    if let Some(hooks) = hooks {
                        (hooks.post_landing)(clone, &ck);
                        let failures = crate::clone_deletion::failures(
                            clone,
                            &ck,
                            hooks.custody,
                            hooks.evidence,
                        );
                        if failures.is_empty() {
                            let cleanup = if hooks.custody.delete_remote_on_merge {
                                Some(crate::custody_push::RemoteCleanup::prepare(
                                    clone,
                                    hooks.custody,
                                    &ck,
                                ))
                            } else {
                                None
                            };
                            match reap_clone(clone, &src, root) {
                                Ok(()) => {
                                    if let Some(cleanup) = cleanup {
                                        if let Err(e) = cleanup.and_then(|c| c.delete()) {
                                            eprintln!("merge: warning: incomplete remote cleanup: {e}; remaining/unverified refs: refs/heads/{}{}, refs/heads/{}{}-dirty", hooks.custody.ref_prefix, ck.task_id, hooks.custody.ref_prefix, ck.task_id);
                                        }
                                    }
                                }
                                Err(e) => eprintln!(
                                    "merge: landed but {e}; clone kept at {}",
                                    clone.display()
                                ),
                            }
                        } else {
                            for failure in failures {
                                eprintln!(
                                    "merge: landed; {failure}; clone kept at {}",
                                    clone.display()
                                );
                            }
                        }
                    } else {
                        eprintln!("merge: landed; post-landing custody hook unavailable; clone kept at {}", clone.display());
                    }
                    if already_integrated {
                        let evidence = noop_evidence.as_ref().expect("no-op evidence");
                        println!(
                            "already integrated into {target} at {:.12}; {}: {} (matched {:?})",
                            rt, evidence.confidence, evidence.reason, evidence.matched_commit
                        );
                    } else {
                        println!("merged {:.12} into {target}", rt);
                    }
                    MergeOutcome::Merged
                }
                Err(PushError::StaleLease) => {
                    match mode {
                        MergeMode::ExactBase => eprintln!(
                            "merge: '{target}' moved off {:.12} since the clone was made. The clone's base is fixed, so exact-base merge can't land it; use `merge {} --onto {target} --integrate-current` for a reviewed parallel sibling, or start a fresh `implement` run. (clone kept at {})",
                            ck.base_commit,
                            ck.resume_id,
                            clone.display()
                        ),
                        MergeMode::IntegrateCurrent => eprintln!(
                            "merge: '{target}' moved off fetched commit {:.12} before the landing compare-and-swap; rerun the same non-agent `merge {} --onto {target} --integrate-current` command. (clone kept at {})",
                            expected_target,
                            ck.resume_id,
                            clone.display()
                        ),
                    }
                    MergeOutcome::Unlanded
                }
                Err(PushError::Other(e)) => {
                    eprintln!(
                        "merge: landing failed: {e}; clone kept at {}",
                        clone.display()
                    );
                    MergeOutcome::Unlanded
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::implement_resume::ImplementPhase::*;

    fn ok(t: &str) -> Result<String, String> {
        Ok(t.to_string())
    }

    #[test]
    fn gate_matrix() {
        // Approved + has commit + resolvable target -> Merge
        assert_eq!(
            decide_merge(Approved, true, false, &ok("main")),
            MergePlan::Merge {
                target: "main".into()
            }
        );
        // Approved but NO commit -> RefuseHard (force can't help)
        assert!(matches!(
            decide_merge(Approved, false, true, &ok("main")),
            MergePlan::RefuseHard(_)
        ));
        // LoopStopped w/o force -> Refuse; with force -> Merge
        assert!(matches!(
            decide_merge(LoopStopped, true, false, &ok("main")),
            MergePlan::Refuse(_)
        ));
        assert_eq!(
            decide_merge(LoopStopped, true, true, &ok("main")),
            MergePlan::Merge {
                target: "main".into()
            }
        );
        // Non-terminal phases -> RefuseHard even with force
        for p in [Cloned, EditStarted, FirstCommitCreated, InLoop] {
            assert!(matches!(
                decide_merge(p, true, true, &ok("main")),
                MergePlan::RefuseHard(_)
            ));
        }
        // Unresolvable target -> Refuse (even when Approved)
        assert!(matches!(
            decide_merge(Approved, true, false, &Err("no target".into())),
            MergePlan::Refuse(_)
        ));
    }

    #[test]
    fn resolve_target_precedence_and_validation() {
        // precedence: --onto > [merge].target_ref > base_ref
        assert_eq!(
            resolve_target(Some("a"), Some("b"), Some("c")),
            Ok("a".into())
        );
        assert_eq!(resolve_target(None, Some("b"), Some("c")), Ok("b".into()));
        assert_eq!(resolve_target(None, None, Some("c")), Ok("c".into()));
        assert!(resolve_target(None, None, None).is_err());
        // best-effort rejects (string-decidable only)
        for bad in [
            "",
            "HEAD",
            "refs/heads/main",
            "refs/tags/v1",
            "origin/main",
            "feat..x",
            "-bad",
            "trailing/",
            "ends.lock",
            "a b",
            "x~1",
            "deadbeefdeadbeefdeadbeefdeadbeefdeadbeef",
        ] {
            assert!(
                resolve_target(Some(bad), None, None).is_err(),
                "{bad:?} should be rejected"
            );
        }
        // accepted ordinary names
        for good in ["main", "feature/x", "release/1.2", "dev"] {
            assert_eq!(resolve_target(Some(good), None, None), Ok(good.to_string()));
        }
    }

    #[test]
    fn merge_outcome_exit_codes() {
        assert_eq!(MergeOutcome::Merged.code(), 0);
        assert_eq!(MergeOutcome::UsageOrPreflight.code(), 1);
        assert_eq!(MergeOutcome::Unlanded.code(), 3);
    }
}

#[cfg(test)]
mod git_tests {
    use super::*;
    use std::path::PathBuf;

    /// Retry `operation` while it fails with `ETXTBSY`, up to `attempts` times `pause` apart; any
    /// other outcome, or the last `ETXTBSY`, is returned unchanged. A test-only copy of
    /// `bridge-core`'s helper: a test that writes a script races every other test thread's fork,
    /// because a child forked while the write descriptor is open keeps a writable duplicate until
    /// its own exec, and the kernel refuses to exec the script meanwhile.
    fn retry_on_text_file_busy<T>(
        attempts: u32,
        pause: std::time::Duration,
        mut operation: impl FnMut() -> std::io::Result<T>,
    ) -> std::io::Result<T> {
        let mut attempt = 1;
        loop {
            match operation() {
                Err(error)
                    if error.kind() == std::io::ErrorKind::ExecutableFileBusy
                        && attempt < attempts =>
                {
                    attempt += 1;
                    std::thread::sleep(pause);
                }
                outcome => return outcome,
            }
        }
    }

    /// Execute a just-written hook once with no arguments and a closed stdin, retrying `ETXTBSY`,
    /// before Git executes it. One exec that is not refused proves no writer remains.
    fn warm_up_script(path: &std::path::Path) {
        retry_on_text_file_busy(50, std::time::Duration::from_millis(10), || {
            std::process::Command::new(path).output()
        })
        .expect("warm-up exec of the written script");
    }

    #[test]
    fn text_file_busy_retry_is_bounded_and_retries_only_etxtbsy() {
        use std::io::{Error, ErrorKind};
        let zero = std::time::Duration::ZERO;
        let mut calls = 0;
        let outcome = retry_on_text_file_busy(50, zero, || {
            calls += 1;
            if calls <= 3 {
                Err(Error::from(ErrorKind::ExecutableFileBusy))
            } else {
                Ok(calls)
            }
        });
        assert_eq!(outcome.unwrap(), 4);
        let mut calls = 0;
        let outcome: std::io::Result<()> = retry_on_text_file_busy(7, zero, || {
            calls += 1;
            Err(Error::from(ErrorKind::ExecutableFileBusy))
        });
        assert_eq!(outcome.unwrap_err().kind(), ErrorKind::ExecutableFileBusy);
        assert_eq!(calls, 7);
        let mut calls = 0;
        let outcome: std::io::Result<()> = retry_on_text_file_busy(50, zero, || {
            calls += 1;
            Err(Error::from(ErrorKind::PermissionDenied))
        });
        assert_eq!(outcome.unwrap_err().kind(), ErrorKind::PermissionDenied);
        assert_eq!(calls, 1);
    }

    /// A git repo with one commit on `main`; returns (guard, path, base_sha).
    fn repo_with_base() -> (tempfile::TempDir, PathBuf, String) {
        let td = tempfile::tempdir().unwrap();
        let p = td.path().to_path_buf();
        run_git(Some(&p), &["init", "-q", "-b", "main"]).unwrap();
        run_git(Some(&p), &["config", "user.name", "Op Erator"]).unwrap();
        run_git(Some(&p), &["config", "user.email", "op@example.com"]).unwrap();
        std::fs::write(p.join("base.txt"), "base\n").unwrap();
        run_git(Some(&p), &["add", "."]).unwrap();
        run_git(Some(&p), &["commit", "-q", "-m", "base"]).unwrap();
        let base = run_git_str(&p, &["rev-parse", "HEAD"]);
        (td, p, base)
    }
    fn run_git_str(p: &Path, args: &[&str]) -> String {
        let o = run_git(Some(p), args).unwrap();
        String::from_utf8_lossy(&o.stdout).trim().to_string()
    }

    #[test]
    fn reauthor_sets_operator_identity_same_tree_unmoved_branch() {
        let (_g, p, base) = repo_with_base();
        // add a "bot" commit on a work branch
        run_git(Some(&p), &["checkout", "-q", "-b", "implement/x"]).unwrap();
        std::fs::write(p.join("work.txt"), "work\n").unwrap();
        run_git(Some(&p), &["add", "."]).unwrap();
        run_git(
            Some(&p),
            &[
                "-c",
                "user.name=a2a-implement",
                "-c",
                "user.email=implement@a2a-bridge.local",
                "commit",
                "-q",
                "-m",
                "bot did work",
            ],
        )
        .unwrap();
        let current = run_git_str(&p, &["rev-parse", "HEAD"]);
        let op = OperatorIdent {
            name: "Op Erator".into(),
            email: "op@example.com".into(),
        };

        let rt = reauthor_commit(&p, &current, &base, "land it", &op).unwrap();

        // operator identity on both author and committer
        assert_eq!(
            run_git_str(&p, &["log", "-1", "--format=%an <%ae>", &rt]),
            "Op Erator <op@example.com>"
        );
        assert_eq!(
            run_git_str(&p, &["log", "-1", "--format=%cn <%ce>", &rt]),
            "Op Erator <op@example.com>"
        );
        // author date == committer date (both the captured T)
        assert_eq!(
            run_git_str(&p, &["log", "-1", "--format=%at", &rt]),
            run_git_str(&p, &["log", "-1", "--format=%ct", &rt])
        );
        // same tree as current; parent is base
        assert_eq!(
            run_git_str(&p, &["rev-parse", &format!("{rt}^{{tree}}")]),
            run_git_str(&p, &["rev-parse", &format!("{current}^{{tree}}")])
        );
        assert_eq!(run_git_str(&p, &["rev-parse", &format!("{rt}^")]), base);
        // clone branch UNMOVED (retry-safe)
        assert_eq!(run_git_str(&p, &["rev-parse", "HEAD"]), current);
    }

    #[test]
    fn clone_shape_rejects_non_ancestor_base() {
        let (_g, p, base) = repo_with_base();
        // current that does NOT descend from base: a fresh orphan commit
        run_git(Some(&p), &["checkout", "-q", "--orphan", "orphan"]).unwrap();
        std::fs::write(p.join("o.txt"), "o\n").unwrap();
        run_git(Some(&p), &["add", "."]).unwrap();
        run_git(Some(&p), &["commit", "-q", "-m", "orphan"]).unwrap();
        let current = run_git_str(&p, &["rev-parse", "HEAD"]);
        assert!(check_clone_shape(&p, &base, &current).is_err());

        // a real descendant passes
        run_git(Some(&p), &["checkout", "-q", "main"]).unwrap();
        run_git(Some(&p), &["checkout", "-q", "-b", "desc"]).unwrap();
        std::fs::write(p.join("d.txt"), "d\n").unwrap();
        run_git(Some(&p), &["add", "."]).unwrap();
        run_git(Some(&p), &["commit", "-q", "-m", "d"]).unwrap();
        let good = run_git_str(&p, &["rev-parse", "HEAD"]);
        assert!(check_clone_shape(&p, &base, &good).is_ok());
    }

    #[test]
    fn push_landing_ff_then_stalelease_and_source_unchanged() {
        // source: non-bare repo on `main`, sitting on `base`; we land onto `release` (not checked out).
        let (_gs, src, base) = repo_with_base();
        run_git(Some(&src), &["branch", "release", &base]).unwrap(); // release == base
                                                                     // clone: a sibling repo; share `base` by fetching from src, build a descendant + reauthor.
        let (_gc, clone, _b2) = repo_with_base();
        run_git(
            Some(&clone),
            &[
                "fetch",
                "-q",
                src.to_str().unwrap(),
                &format!("{base}:refs/heads/from-src"),
            ],
        )
        .unwrap();
        run_git(Some(&clone), &["checkout", "-q", "from-src"]).unwrap();
        std::fs::write(clone.join("w.txt"), "w\n").unwrap();
        run_git(Some(&clone), &["add", "."]).unwrap();
        run_git(Some(&clone), &["commit", "-q", "-m", "work"]).unwrap();
        let current = run_git_str(&clone, &["rev-parse", "HEAD"]);
        let op = OperatorIdent {
            name: "Op".into(),
            email: "op@x.com".into(),
        };
        let rt = reauthor_commit(&clone, &current, &base, "land", &op).unwrap();

        // capture source-unchanged baseline
        let src_head_before = run_git_str(&src, &["rev-parse", "HEAD"]);
        let status_before =
            run_git_str(&src, &["status", "--porcelain=v1", "--untracked-files=all"]);

        // FF: release is at base -> lands
        assert!(push_landing(&clone, &src, &rt, "release", &base).is_ok());
        assert_eq!(run_git_str(&src, &["rev-parse", "release"]), rt);

        // operator checkout untouched (HEAD + worktree byte-identical)
        assert_eq!(run_git_str(&src, &["rev-parse", "HEAD"]), src_head_before);
        assert_eq!(
            run_git_str(&src, &["status", "--porcelain=v1", "--untracked-files=all"]),
            status_before
        );

        // StaleLease: release moved off base -> a second push with the SAME base lease refuses, ref NOT moved.
        let moved_to = run_git_str(&src, &["rev-parse", "release"]); // now == rt, != base
        let rt2 = reauthor_commit(&clone, &current, &base, "land again", &op).unwrap();
        assert!(matches!(
            push_landing(&clone, &src, &rt2, "release", &base),
            Err(PushError::StaleLease)
        ));
        assert_eq!(run_git_str(&src, &["rev-parse", "release"]), moved_to); // unchanged
    }

    #[test]
    fn verify_target_unchanged_accepts_exact_ref_and_rejects_move() {
        let (_gs, src, base) = repo_with_base();
        run_git(Some(&src), &["branch", "release", &base]).unwrap();
        assert!(verify_target_unchanged(&src, "release", &base).is_ok());
        assert_eq!(run_git_str(&src, &["rev-parse", "release"]), base);

        std::fs::write(src.join("next.txt"), "next\n").unwrap();
        run_git(Some(&src), &["add", "next.txt"]).unwrap();
        run_git(Some(&src), &["commit", "-q", "-m", "next"]).unwrap();
        let moved = run_git_str(&src, &["rev-parse", "HEAD"]);
        run_git(Some(&src), &["branch", "-f", "release", &moved]).unwrap();

        assert!(matches!(
            verify_target_unchanged(&src, "release", &base),
            Err(PushError::StaleLease)
        ));
        assert_eq!(
            run_git_str(&src, &["rev-parse", "release"]),
            moved,
            "a failed compare-only transaction must not restore the stale value"
        );
    }

    #[test]
    fn concurrent_pushes_one_wins() {
        let (_gs, src, base) = repo_with_base();
        run_git(Some(&src), &["branch", "line", &base]).unwrap();
        // two clones, each a distinct descendant of base, each reauthored
        let mk = |name: &str| -> (tempfile::TempDir, PathBuf, String) {
            let (g, c, _b) = repo_with_base();
            run_git(
                Some(&c),
                &[
                    "fetch",
                    "-q",
                    src.to_str().unwrap(),
                    &format!("{base}:refs/heads/s"),
                ],
            )
            .unwrap();
            run_git(Some(&c), &["checkout", "-q", "s"]).unwrap();
            std::fs::write(c.join(format!("{name}.txt")), "x\n").unwrap();
            run_git(Some(&c), &["add", "."]).unwrap();
            run_git(Some(&c), &["commit", "-q", "-m", name]).unwrap();
            let cur = run_git_str(&c, &["rev-parse", "HEAD"]);
            let op = OperatorIdent {
                name: "Op".into(),
                email: "op@x.com".into(),
            };
            let rt = reauthor_commit(&c, &cur, &base, name, &op).unwrap();
            (g, c, rt)
        };
        let (_ga, ca, rta) = mk("a");
        let (_gb, cb, rtb) = mk("b");
        let r1 = push_landing(&ca, &src, &rta, "line", &base);
        let r2 = push_landing(&cb, &src, &rtb, "line", &base);
        // exactly one Ok, the other StaleLease (no lock — the lease IS the CAS)
        assert_ne!(r1.is_ok(), r2.is_ok());
        let loser = if r1.is_ok() { r2 } else { r1 };
        assert!(matches!(loser, Err(PushError::StaleLease)));
    }

    #[test]
    fn operator_from_sources_config_then_repo_then_fails() {
        let (_g, src, _b) = repo_with_base(); // sets user.name/email
        let ov = OperatorIdent {
            name: "Cfg".into(),
            email: "cfg@x.com".into(),
        };
        assert_eq!(operator_from(&src, Some(&ov)).unwrap(), ov); // override wins
        let got = operator_from(&src, None).unwrap(); // repo config when no override
        assert_eq!(got.email, "op@example.com");
        // Empty the LOCAL identity (local "" shadows any machine-global config → deterministic).
        run_git(Some(&src), &["config", "user.name", ""]).unwrap();
        run_git(Some(&src), &["config", "user.email", ""]).unwrap();
        assert!(operator_from(&src, None).is_err()); // empty identity -> error
    }

    #[test]
    fn source_head_and_is_bare() {
        let (_g, src, _b) = repo_with_base();
        assert_eq!(source_head(&src).unwrap().as_deref(), Some("main"));
        assert!(!is_bare(&src).unwrap());
        // detached HEAD -> None
        let sha = run_git_str(&src, &["rev-parse", "HEAD"]);
        run_git(Some(&src), &["checkout", "-q", &sha]).unwrap();
        assert_eq!(source_head(&src).unwrap(), None);
    }

    #[test]
    fn reap_clone_guards_path() {
        let root = tempfile::tempdir().unwrap();
        let id = "impl-1-abcd";
        let clone = root.path().join(".a2a-implement").join(id);
        std::fs::create_dir_all(clone.join(".git")).unwrap();
        let (_g, src, _b) = repo_with_base();
        // wrong shape (src is not <root>/.a2a-implement/<id>) is refused
        assert!(reap_clone(&src, &src, root.path()).is_err());
        // correct shape -> deletes
        assert!(reap_clone(&clone, &src, root.path()).is_ok());
        assert!(!clone.exists());
    }

    /// Build a named clone with one file change and a saved checkpoint at `phase`.
    fn clone_with_change(
        root: &Path,
        src: &Path,
        base: &str,
        id: &str,
        file: &str,
        body: &str,
        phase: ImplementPhase,
    ) -> (PathBuf, String) {
        use crate::implement_resume::save_checkpoint;
        let clone = root.join(".a2a-implement").join(id);
        std::fs::create_dir_all(&clone).unwrap();
        let branch = format!("implement/{id}");
        run_git(Some(&clone), &["init", "-q", "-b", &branch]).unwrap();
        run_git(Some(&clone), &["config", "user.name", "Bot"]).unwrap();
        run_git(Some(&clone), &["config", "user.email", "bot@x"]).unwrap();
        run_git(
            Some(&clone),
            &[
                "fetch",
                "-q",
                src.to_str().unwrap(),
                &format!("{base}:refs/heads/base-tmp"),
            ],
        )
        .unwrap();
        run_git(Some(&clone), &["reset", "-q", "--hard", base]).unwrap();
        std::fs::write(clone.join(file), body).unwrap();
        run_git(Some(&clone), &["add", "."]).unwrap();
        run_git(Some(&clone), &["commit", "-q", "-m", "bot work"]).unwrap();
        let current = run_git_str(&clone, &["rev-parse", "HEAD"]);
        let ck = ImplementCheckpoint {
            schema_version: SCHEMA_VERSION,
            resume_id: id.into(),
            task_id: id.into(),
            task_brief: "do x".into(),
            source_repo: src.to_path_buf(),
            clone_path: clone.clone(),
            config_path: src.to_path_buf(),
            branch,
            base_ref: Some("release".into()),
            base_commit: base.to_string(),
            current_commit: Some(current.clone()),
            original_message: Some("land x".into()),
            edit_workflow: "e".into(),
            fix_workflow: "f".into(),
            loop_max_attempts: 1,
            review_invocation: 0,
            final_review: None,
            attempt_next: 1,
            forced_depth: None,
            resolved_lang: None,
            activity_tally: None,
            terminal_evidence_counts: None,
            phase,
            created_at_ms: 0,
            updated_at_ms: 0,
        };
        save_checkpoint(&clone, &ck).unwrap();
        (clone, current)
    }

    fn with_test_hooks<T>(clone: &Path, f: impl FnOnce(&mut MergeHooks<'_>) -> T) -> T {
        let ck = load_checkpoint(clone).unwrap();
        // A local bare destination stands in for the operator's source remote.
        let remote = clone
            .parent()
            .unwrap()
            .join(format!("{}-remote", ck.task_id));
        if !remote.exists() {
            run_git(None, &["init", "--bare", remote.to_str().unwrap()]).unwrap();
        }
        run_git(Some(&ck.source_repo), &["remote", "remove", "origin"]).ok();
        run_git(
            Some(&ck.source_repo),
            &["remote", "add", "origin", remote.to_str().unwrap()],
        )
        .unwrap();
        std::fs::write(clone.join(".git/A2A_TASK.md"), "test spec").unwrap();
        let custody = crate::custody_push::CustodyConfig::default();
        let evidence = crate::evidence::EvidenceConfig {
            path: Some(clone.parent().unwrap().join("evidence")),
            ..Default::default()
        };
        let mut post = |clone: &Path, ck: &ImplementCheckpoint| {
            assert!(acquire_operation_lock(clone).is_err());
            let mut ck = ck.clone();
            let sha = ck.current_commit.clone().unwrap();
            crate::custody_push::push_before_review_or_merge(clone, &custody, &mut ck, &sha);
            crate::evidence::write_run_archive(clone, &ck, "merged", &evidence).unwrap();
        };
        f(&mut MergeHooks {
            custody: &custody,
            evidence: &evidence,
            post_landing: &mut post,
        })
    }
    fn merge_clone(
        mcfg: Option<&MergeConfig>,
        clone: &Path,
        root: &Path,
        onto: Option<&str>,
        force: bool,
        mode: MergeMode,
    ) -> MergeOutcome {
        with_test_hooks(clone, |hooks| {
            super::merge_clone(mcfg, clone, root, onto, force, mode, Some(hooks))
        })
    }
    #[allow(clippy::too_many_arguments)]
    fn merge_clone_with_operation_lock(
        operation: bridge_core::liveness::PersistentLockGuard,
        mcfg: Option<&MergeConfig>,
        clone: &Path,
        root: &Path,
        onto: Option<&str>,
        force: bool,
        mode: MergeMode,
    ) -> MergeOutcome {
        with_test_hooks(clone, |hooks| {
            super::merge_clone_with_operation_lock(
                operation,
                mcfg,
                clone,
                root,
                onto,
                force,
                mode,
                Some(hooks),
            )
        })
    }

    /// Backward-compatible fixture for the exact-base tests.
    fn clone_with_checkpoint(
        root: &Path,
        src: &Path,
        base: &str,
        phase: ImplementPhase,
    ) -> (PathBuf, String) {
        clone_with_change(root, src, base, "impl-1-abcd", "w.txt", "w\n", phase)
    }

    fn rewrite_archive(path: &Path, change: impl FnOnce(&mut serde_json::Value)) {
        use std::io::Read;
        let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(
            std::fs::File::open(path).unwrap(),
        ));
        let mut files = Vec::new();
        for entry in archive.entries().unwrap() {
            let mut entry = entry.unwrap();
            let name = entry.path().unwrap().into_owned();
            let mut body = Vec::new();
            entry.read_to_end(&mut body).unwrap();
            files.push((name, body));
        }
        let manifest = files
            .iter_mut()
            .find(|(p, _)| p == Path::new("manifest.json"))
            .unwrap();
        let mut value = serde_json::from_slice(&manifest.1).unwrap();
        change(&mut value);
        manifest.1 = serde_json::to_vec(&value).unwrap();
        let mut writer = tar::Builder::new(flate2::write::GzEncoder::new(
            std::fs::File::create(path).unwrap(),
            flate2::Compression::default(),
        ));
        for (name, body) in files {
            let mut header = tar::Header::new_gnu();
            header.set_size(body.len() as u64);
            header.set_mode(0o600);
            header.set_cksum();
            writer
                .append_data(&mut header, name, body.as_slice())
                .unwrap();
        }
        writer.into_inner().unwrap().finish().unwrap();
    }

    #[test]
    fn post_landing_allowlist_keeps_every_unrecognized_shape() {
        for (case, check) in [
            ("remote-absent", 1),
            ("remote-other", 1),
            ("push-disabled", 1),
            ("late-dirty", 2),
            ("archive-missing", 3),
            ("archive-digest", 3),
            ("archive-inside", 3),
            ("archive-kind", 3),
            ("archive-drop-reason", 3),
            ("archive-size", 3),
            ("archive-identity", 3),
            ("archive-phase", 3),
            ("side-branch", 4),
            ("tree-equal-side", 4),
            ("light-tag", 4),
            ("annotated-tag", 4),
            ("notes", 4),
            ("stash", 4),
            ("skip-worktree", 5),
            ("assume-unchanged", 5),
            ("merge-head", 5),
            ("bisect-head", 5),
            ("submodule", 5),
            ("gitlink", 5),
            ("ignored-nested", 5),
            ("linked-worktree", 5),
            ("stale-worktrees", 5),
            ("redirect", 0),
            ("filter", 0),
            ("rebase", 5),
            ("sequencer", 5),
            ("other-namespace", 4),
        ] {
            let (_gs, src, base) = repo_with_base();
            run_git(Some(&src), &["branch", "release", &base]).unwrap();
            let root = tempfile::tempdir().unwrap();
            let (clone, current) =
                clone_with_checkpoint(root.path(), &src, &base, ImplementPhase::Approved);
            with_test_hooks(&clone, |original| {
                let mut custody = original.custody.clone();
                custody.delete_remote_on_merge = true; // a kept clone must also keep remote refs
                if case == "push-disabled" {
                    custody.push = false;
                }
                let mut evidence = original.evidence.clone();
                if case == "archive-inside" {
                    evidence.path = Some(clone.join(".git/evidence"));
                }
                let external = original
                    .evidence
                    .path
                    .as_ref()
                    .unwrap()
                    .join(crate::evidence::repo_key(&src).unwrap())
                    .join("impl-1-abcd.tar.gz");
                let mut post = |clone: &Path, ck: &ImplementCheckpoint| {
                    (original.post_landing)(clone, ck);
                    // The operation lock remains held even inside the host callback.
                    assert!(acquire_operation_lock(clone).is_err());
                    let remote = run_git_str(&src, &["remote", "get-url", "origin"]);
                    let remote_ref = "refs/heads/wip/impl-1-abcd";
                    match case {
                        "remote-absent" => {
                            run_git(Some(Path::new(&remote)), &["update-ref", "-d", remote_ref])
                                .unwrap();
                        }
                        "remote-other" => {
                            run_git(Some(Path::new(&remote)), &["update-ref", remote_ref, &base])
                                .unwrap();
                        }
                        "late-dirty" => {
                            std::fs::write(clone.join("unique"), "late").unwrap();
                        }
                        "archive-missing" => {
                            std::fs::remove_file(&external).unwrap();
                        }
                        "archive-digest" => rewrite_archive(&external, |v| {
                            v["included"][0]["sha256"] = "wrong".into()
                        }),
                        "archive-size" => {
                            rewrite_archive(&external, |v| v["included"][0]["bytes"] = 999.into())
                        }
                        "archive-identity" => {
                            rewrite_archive(&external, |v| v["task_id"] = "impl-other".into())
                        }
                        "archive-phase" => {
                            rewrite_archive(&external, |v| v["terminal_phase"] = "approved".into())
                        }
                        "archive-drop-reason" => rewrite_archive(&external, |v| {
                            v["dropped"][0]["reason"] = "unreadable".into()
                        }),
                        "archive-kind" => rewrite_archive(&external, |v| {
                            v["included"] = serde_json::json!([]);
                            v["dropped"] = serde_json::json!([]);
                        }),
                        "archive-inside" => {
                            let dir = evidence
                                .path
                                .as_ref()
                                .unwrap()
                                .join(crate::evidence::repo_key(&src).unwrap());
                            std::fs::create_dir_all(&dir).unwrap();
                            std::fs::copy(&external, dir.join("impl-1-abcd.tar.gz")).unwrap();
                        }
                        "side-branch" | "tree-equal-side" | "light-tag" => {
                            let tree_ref = if case == "tree-equal-side" {
                                format!("{base}^{{tree}}")
                            } else {
                                "HEAD^{tree}".into()
                            };
                            let tree = run_git_str(clone, &["rev-parse", &tree_ref]);
                            let side =
                                run_git_str(clone, &["commit-tree", &tree, "-m", "unique side"]);
                            let reference = if case == "light-tag" {
                                "refs/tags/side"
                            } else {
                                "refs/heads/side"
                            };
                            run_git(Some(clone), &["update-ref", reference, &side]).unwrap();
                        }
                        "annotated-tag" => {
                            run_git(Some(clone), &["tag", "-a", "unique", "-m", "unique tag"])
                                .unwrap();
                        }
                        "notes" => {
                            run_git(Some(clone), &["notes", "add", "-m", "unique note"]).unwrap();
                        }
                        "stash" => {
                            std::fs::write(clone.join("unique"), "stash").unwrap();
                            run_git(Some(clone), &["stash", "push", "-u"]).unwrap();
                        }
                        "skip-worktree" | "assume-unchanged" => {
                            run_git(
                                Some(clone),
                                &[
                                    "update-index",
                                    if case == "skip-worktree" {
                                        "--skip-worktree"
                                    } else {
                                        "--assume-unchanged"
                                    },
                                    "w.txt",
                                ],
                            )
                            .unwrap();
                        }
                        "merge-head" | "bisect-head" => {
                            std::fs::write(
                                clone.join(if case == "merge-head" {
                                    ".git/MERGE_HEAD"
                                } else {
                                    ".git/BISECT_HEAD"
                                }),
                                &current,
                            )
                            .unwrap();
                        }
                        "submodule" => {
                            let out = run_git(
                                Some(clone),
                                &[
                                    "-c",
                                    "protocol.file.allow=always",
                                    "submodule",
                                    "add",
                                    src.to_str().unwrap(),
                                    "sub",
                                ],
                            )
                            .unwrap();
                            assert!(
                                out.status.success(),
                                "{}",
                                String::from_utf8_lossy(&out.stderr)
                            );
                            run_git(
                                Some(clone),
                                &["config", "--remove-section", "submodule.sub"],
                            )
                            .unwrap();
                        }
                        "gitlink" => {
                            run_git(None, &["init", clone.join("embedded").to_str().unwrap()])
                                .unwrap();
                            run_git(
                                Some(clone),
                                &[
                                    "update-index",
                                    "--add",
                                    "--cacheinfo",
                                    &format!("160000,{current},embedded"),
                                ],
                            )
                            .unwrap();
                            assert!(!clone.join(".gitmodules").exists());
                        }
                        "ignored-nested" => {
                            std::fs::write(clone.join(".git/info/exclude"), "ignored/\n").unwrap();
                            std::fs::create_dir_all(clone.join("ignored/deep/.git")).unwrap();
                        }
                        "linked-worktree" => {
                            run_git(
                                Some(clone),
                                &[
                                    "worktree",
                                    "add",
                                    "--detach",
                                    root.path().join("linked").to_str().unwrap(),
                                ],
                            )
                            .unwrap();
                        }
                        "stale-worktrees" => {
                            std::fs::create_dir_all(clone.join(".git/worktrees/deleted")).unwrap();
                        }
                        "redirect" => {
                            let copy = root.path().join("clean");
                            std::fs::create_dir(&copy).unwrap();
                            std::fs::write(clone.join("unique"), "unique clone bytes").unwrap();
                            run_git(
                                Some(clone),
                                &["config", "core.worktree", copy.to_str().unwrap()],
                            )
                            .unwrap();
                        }
                        "filter" => {
                            run_git(Some(clone), &["config", "filter.evil.clean", "false"])
                                .unwrap();
                        }
                        "rebase" => {
                            std::fs::create_dir(clone.join(".git/rebase-apply")).unwrap();
                        }
                        "sequencer" => {
                            std::fs::create_dir(clone.join(".git/sequencer")).unwrap();
                        }
                        "other-namespace" => {
                            run_git(
                                Some(clone),
                                &["update-ref", "refs/private/unique", &current],
                            )
                            .unwrap();
                        }
                        _ => {}
                    }
                };
                let mut hooks = MergeHooks {
                    custody: &custody,
                    evidence: &evidence,
                    post_landing: &mut post,
                };
                let result = super::merge_clone(
                    None,
                    &clone,
                    root.path(),
                    Some("release"),
                    false,
                    MergeMode::ExactBase,
                    Some(&mut hooks),
                );
                assert_eq!(result.code(), 0, "{case}");
                assert!(clone.exists(), "{case}");
                let ck = load_checkpoint(&clone).unwrap();
                let failures = crate::clone_deletion::failures(&clone, &ck, &custody, &evidence);
                assert!(
                    failures
                        .iter()
                        .any(|e| e.starts_with(&format!("check {check}:"))),
                    "{case}: {failures:?}"
                );
                assert!(
                    acquire_operation_lock(&clone).is_ok(),
                    "{case}: lock leaked"
                );
                if case != "remote-absent" {
                    assert!(
                        crate::custody_push::ls_remote_ref(
                            &src,
                            &clone,
                            &custody,
                            "refs/heads/wip/impl-1-abcd"
                        )
                        .unwrap()
                        .is_some(),
                        "{case}: remote deleted for kept clone"
                    );
                }
            });
        }
    }

    #[test]
    fn allowed_shape_deletes_after_source_moves_and_cleans_remote_only_when_enabled() {
        for cleanup in ["disabled", "delete", "stale-lease"] {
            let (_gs, src, base) = repo_with_base();
            run_git(Some(&src), &["tag", "-a", "source-tag", "-m", "source tag"]).unwrap();
            run_git(Some(&src), &["branch", "release", &base]).unwrap();
            let root = tempfile::tempdir().unwrap();
            let (clone, current) =
                clone_with_checkpoint(root.path(), &src, &base, ImplementPhase::Approved);
            // Source main moves beyond the cloned main and remote-tracking refs.
            std::fs::write(src.join("later"), "source main advanced").unwrap();
            run_git(Some(&src), &["add", "later"]).unwrap();
            run_git(Some(&src), &["commit", "-m", "later"]).unwrap();
            with_test_hooks(&clone, |original| {
                let mut custody = original.custody.clone();
                custody.delete_remote_on_merge = cleanup != "disabled";
                let remote = run_git_str(&src, &["remote", "get-url", "origin"]);
                let mut post = |clone: &Path, ck: &ImplementCheckpoint| {
                    (original.post_landing)(clone, ck);
                    // The dirty ref uses the same recorded custody lease mechanism.
                    let mut state = crate::implement_resume::load_custody(clone)
                        .unwrap()
                        .unwrap();
                    let dirty_ref = "refs/heads/wip/impl-1-abcd-dirty";
                    run_git(
                        Some(Path::new(&remote)),
                        &["update-ref", dirty_ref, &current],
                    )
                    .unwrap();
                    state.dirty.last_pushed_sha = Some(if cleanup == "stale-lease" {
                        base.clone()
                    } else {
                        current.clone()
                    });
                    crate::implement_resume::save_checkpoint_with_custody(clone, ck, Some(state))
                        .unwrap();
                    std::fs::write(clone.join(".git/info/exclude"), "ignored/\n").unwrap();
                    std::fs::create_dir(clone.join("ignored")).unwrap();
                    std::fs::write(clone.join("ignored/cache"), "disposable").unwrap();
                    #[cfg(unix)]
                    {
                        let external = root.path().join("external");
                        std::fs::create_dir_all(external.join(".git")).unwrap();
                        std::os::unix::fs::symlink(&external, clone.join("ignored/link")).unwrap();
                    }
                    std::fs::create_dir(clone.join(".git/worktrees")).unwrap();
                    std::fs::write(clone.join(".git/ORIG_HEAD"), &base).unwrap();
                    std::fs::write(clone.join(".git/FETCH_HEAD"), &base).unwrap();
                };
                let mut hooks = MergeHooks {
                    custody: &custody,
                    evidence: original.evidence,
                    post_landing: &mut post,
                };
                assert_eq!(
                    super::merge_clone(
                        None,
                        &clone,
                        root.path(),
                        Some("release"),
                        false,
                        MergeMode::ExactBase,
                        Some(&mut hooks)
                    )
                    .code(),
                    0
                );
                assert!(!clone.exists());
                for reference in [
                    "refs/heads/wip/impl-1-abcd",
                    "refs/heads/wip/impl-1-abcd-dirty",
                ] {
                    let out = run_git(
                        Some(Path::new(&remote)),
                        &["show-ref", "--verify", reference],
                    )
                    .unwrap();
                    let retained = cleanup == "disabled"
                        || (cleanup == "stale-lease" && reference.ends_with("-dirty"));
                    assert_eq!(out.status.success(), retained, "{cleanup}: {reference}");
                }
            });
        }
    }

    #[test]
    fn stalled_gate_transport_keeps_clone_and_releases_operation_lock() {
        let out = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "merge::git_tests::stalled_gate_child",
                "--nocapture",
            ])
            .env("I3_STALLED_CHILD", "1")
            .env("GIT_SSH_COMMAND", "sleep 30 #")
            .env("GIT_SSH_VARIANT", "ssh")
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(String::from_utf8_lossy(&out.stderr).contains("check 1: custody git timeout"));
    }

    #[test]
    fn stalled_gate_child() {
        if std::env::var_os("I3_STALLED_CHILD").is_none() {
            return;
        }
        let (_gs, src, base) = repo_with_base();
        run_git(Some(&src), &["branch", "release", &base]).unwrap();
        let root = tempfile::tempdir().unwrap();
        let (clone, _) = clone_with_checkpoint(root.path(), &src, &base, ImplementPhase::Approved);
        with_test_hooks(&clone, |original| {
            let mut custody = original.custody.clone();
            custody.push_timeout_secs = 1;
            let mut post = |clone: &Path, ck: &ImplementCheckpoint| {
                (original.post_landing)(clone, ck);
                run_git(
                    Some(&src),
                    &["remote", "set-url", "origin", "ssh://host/backup.git"],
                )
                .unwrap();
            };
            let mut hooks = MergeHooks {
                custody: &custody,
                evidence: original.evidence,
                post_landing: &mut post,
            };
            let start = std::time::Instant::now();
            assert_eq!(
                super::merge_clone(
                    None,
                    &clone,
                    root.path(),
                    Some("release"),
                    false,
                    MergeMode::ExactBase,
                    Some(&mut hooks)
                )
                .code(),
                0
            );
            assert!(start.elapsed() < std::time::Duration::from_secs(5));
            assert!(clone.exists());
            assert!(acquire_operation_lock(&clone).is_ok());
        });
    }

    #[test]
    fn initially_dirty_clone_is_preflight_exit_one_without_hook() {
        let (_gs, src, base) = repo_with_base();
        run_git(Some(&src), &["branch", "release", &base]).unwrap();
        let root = tempfile::tempdir().unwrap();
        let (clone, _) = clone_with_checkpoint(root.path(), &src, &base, ImplementPhase::Approved);
        std::fs::write(clone.join("untracked"), "unique").unwrap();
        with_test_hooks(&clone, |original| {
            let mut post = |_: &Path, _: &ImplementCheckpoint| panic!("hook on refused landing");
            let mut hooks = MergeHooks {
                custody: original.custody,
                evidence: original.evidence,
                post_landing: &mut post,
            };
            assert_eq!(
                super::merge_clone(
                    None,
                    &clone,
                    root.path(),
                    Some("release"),
                    false,
                    MergeMode::ExactBase,
                    Some(&mut hooks)
                )
                .code(),
                1
            );
            assert!(clone.exists());
        });
    }

    #[test]
    fn custody_regression_hidden_state_survives_landing() {
        let mut deleted = Vec::new();
        for case in ["stash", "skip-worktree", "side-branch", "linked-worktree"] {
            let (_gs, src, base) = repo_with_base();
            run_git(Some(&src), &["branch", "release", &base]).unwrap();
            let root = tempfile::tempdir().unwrap();
            let (clone, _) =
                clone_with_checkpoint(root.path(), &src, &base, ImplementPhase::Approved);
            match case {
                "stash" => {
                    std::fs::write(clone.join("scratch"), "unique").unwrap();
                    run_git(Some(&clone), &["stash", "push", "-u"]).unwrap();
                }
                "skip-worktree" => {
                    run_git(Some(&clone), &["update-index", "--skip-worktree", "w.txt"]).unwrap();
                }
                "side-branch" => {
                    let tree = run_git_str(&clone, &["rev-parse", "HEAD^{tree}"]);
                    let side = run_git_str(&clone, &["commit-tree", &tree, "-m", "side"]);
                    run_git(Some(&clone), &["update-ref", "refs/heads/side", &side]).unwrap();
                }
                _ => {
                    run_git(
                        Some(&clone),
                        &[
                            "worktree",
                            "add",
                            "--detach",
                            root.path().join("linked").to_str().unwrap(),
                        ],
                    )
                    .unwrap();
                }
            }
            let outcome = merge_clone(
                None,
                &clone,
                root.path(),
                Some("release"),
                false,
                MergeMode::ExactBase,
            );
            assert_eq!(outcome.code(), 0, "{case}");
            if !clone.exists() {
                deleted.push(case);
            }
        }
        assert!(
            deleted.is_empty(),
            "landed clones incorrectly deleted: {deleted:?}"
        );
    }

    #[test]
    fn merge_refuses_clone_config_at_preflight_and_keeps_clone() {
        let (_gs, src, base) = repo_with_base();
        let root = tempfile::tempdir().unwrap();
        let (clone, _) = clone_with_checkpoint(root.path(), &src, &base, ImplementPhase::Approved);
        run_git(
            Some(&clone),
            &["config", "core.fsmonitor", "secret-command"],
        )
        .unwrap();
        let out = merge_clone(
            None,
            &clone,
            root.path(),
            Some("release"),
            false,
            MergeMode::ExactBase,
        );
        assert!(matches!(out, MergeOutcome::UsageOrPreflight));
        assert_eq!(out.code(), 1);
        assert!(clone.join(".git/config").exists());
    }

    #[test]
    fn merge_clone_with_preheld_resume_lock_lands_and_reaps() {
        let (_gs, src, base) = repo_with_base();
        run_git(Some(&src), &["branch", "release", &base]).unwrap();
        let root = tempfile::tempdir().unwrap();
        let (clone, _cur) =
            clone_with_checkpoint(root.path(), &src, &base, ImplementPhase::Approved);

        let operation = acquire_operation_lock(&clone).unwrap();
        let out = merge_clone_with_operation_lock(
            operation,
            None,
            &clone,
            root.path(),
            Some("release"),
            false,
            MergeMode::ExactBase,
        );
        assert!(matches!(out, MergeOutcome::Merged));
        // release advanced off base, authored by the operator (src repo config), clone reaped.
        let landed = run_git_str(&src, &["rev-parse", "release"]);
        assert_ne!(landed, base);
        assert_eq!(
            run_git_str(&src, &["log", "-1", "--format=%an <%ae>", &landed]),
            "Op Erator <op@example.com>"
        );
        assert!(!clone.exists());
        // operator's checkout (main) untouched.
        assert_eq!(
            run_git_str(&src, &["symbolic-ref", "--short", "HEAD"]),
            "main"
        );
    }

    #[test]
    fn parallel_siblings_exact_second_refuses_then_integrate_current_lands_linearly() {
        let (_gs, src, base) = repo_with_base();
        run_git(Some(&src), &["branch", "release", &base]).unwrap();
        let root = tempfile::tempdir().unwrap();
        let (clone_a, _) = clone_with_change(
            root.path(),
            &src,
            &base,
            "impl-a",
            "a.txt",
            "a\n",
            ImplementPhase::Approved,
        );
        let (clone_b, _) = clone_with_change(
            root.path(),
            &src,
            &base,
            "impl-b",
            "b.txt",
            "b\n",
            ImplementPhase::Approved,
        );

        // A dirty operator checkout is custody, not integration scratch space.
        std::fs::write(src.join("base.txt"), "operator work in progress\n").unwrap();
        std::fs::write(src.join("untracked.txt"), "keep me\n").unwrap();
        let source_head = run_git_str(&src, &["rev-parse", "HEAD"]);
        let source_status =
            run_git_str(&src, &["status", "--porcelain=v1", "--untracked-files=all"]);

        assert!(matches!(
            merge_clone(
                None,
                &clone_a,
                root.path(),
                Some("release"),
                false,
                MergeMode::ExactBase,
            ),
            MergeOutcome::Merged
        ));
        let first_landed = run_git_str(&src, &["rev-parse", "release"]);

        // Fail-first control: ADR-0027 exact-base behavior cannot land sibling B after A moved release.
        assert!(matches!(
            merge_clone(
                None,
                &clone_b,
                root.path(),
                Some("release"),
                false,
                MergeMode::ExactBase,
            ),
            MergeOutcome::Unlanded
        ));
        assert!(clone_b.exists());
        assert_eq!(run_git_str(&src, &["rev-parse", "release"]), first_landed);

        assert!(matches!(
            merge_clone(
                None,
                &clone_b,
                root.path(),
                Some("release"),
                false,
                MergeMode::IntegrateCurrent,
            ),
            MergeOutcome::Merged
        ));
        let second_landed = run_git_str(&src, &["rev-parse", "release"]);
        assert_eq!(
            run_git_str(&src, &["rev-parse", &format!("{second_landed}^")]),
            first_landed,
            "parallel siblings must compose as a linear target"
        );
        assert_eq!(run_git_str(&src, &["show", "release:a.txt"]), "a");
        assert_eq!(run_git_str(&src, &["show", "release:b.txt"]), "b");
        assert!(!clone_a.exists());
        assert!(!clone_b.exists());
        assert_eq!(run_git_str(&src, &["rev-parse", "HEAD"]), source_head);
        assert_eq!(
            run_git_str(&src, &["status", "--porcelain=v1", "--untracked-files=all"]),
            source_status,
            "integration must not touch the operator checkout"
        );
        assert_eq!(
            run_git_str(&src, &["log", "-1", "--format=%an <%ae>", &second_landed]),
            "Op Erator <op@example.com>"
        );
    }

    #[test]
    fn integrate_current_conflict_keeps_clone_and_target() {
        let (_gs, src, base) = repo_with_base();
        run_git(Some(&src), &["branch", "release", &base]).unwrap();
        let root = tempfile::tempdir().unwrap();
        let (clone_a, _) = clone_with_change(
            root.path(),
            &src,
            &base,
            "impl-conflict-a",
            "base.txt",
            "from a\n",
            ImplementPhase::Approved,
        );
        let (clone_b, _) = clone_with_change(
            root.path(),
            &src,
            &base,
            "impl-conflict-b",
            "base.txt",
            "from b\n",
            ImplementPhase::Approved,
        );
        assert!(matches!(
            merge_clone(
                None,
                &clone_a,
                root.path(),
                Some("release"),
                false,
                MergeMode::ExactBase,
            ),
            MergeOutcome::Merged
        ));
        let before = run_git_str(&src, &["rev-parse", "release"]);

        assert!(matches!(
            merge_clone(
                None,
                &clone_b,
                root.path(),
                Some("release"),
                false,
                MergeMode::IntegrateCurrent,
            ),
            MergeOutcome::Unlanded
        ));
        assert_eq!(run_git_str(&src, &["rev-parse", "release"]), before);
        assert!(
            clone_b.exists(),
            "a conflicted sibling must remain inspectable"
        );
    }

    #[test]
    fn integrate_current_rejects_destination_outside_frozen_base_history() {
        let (_gs, src, base) = repo_with_base();
        run_git(Some(&src), &["branch", "release", &base]).unwrap();
        let root = tempfile::tempdir().unwrap();
        let (clone, _) = clone_with_change(
            root.path(),
            &src,
            &base,
            "impl-diverged",
            "work.txt",
            "work\n",
            ImplementPhase::Approved,
        );

        run_git(Some(&src), &["checkout", "-q", "--orphan", "unrelated"]).unwrap();
        std::fs::remove_file(src.join("base.txt")).unwrap();
        std::fs::write(src.join("unrelated.txt"), "unrelated\n").unwrap();
        run_git(Some(&src), &["add", "-A"]).unwrap();
        run_git(Some(&src), &["commit", "-q", "-m", "unrelated root"]).unwrap();
        let unrelated = run_git_str(&src, &["rev-parse", "HEAD"]);
        run_git(Some(&src), &["branch", "-f", "release", &unrelated]).unwrap();
        run_git(Some(&src), &["checkout", "-q", "main"]).unwrap();

        assert!(matches!(
            merge_clone(
                None,
                &clone,
                root.path(),
                Some("release"),
                false,
                MergeMode::IntegrateCurrent,
            ),
            MergeOutcome::Unlanded
        ));
        assert_eq!(run_git_str(&src, &["rev-parse", "release"]), unrelated);
        assert!(clone.exists(), "diverged history must retain the run clone");
    }

    #[test]
    fn integrate_current_identical_delta_is_atomic_noop_and_reaps() {
        let (_gs, src, base) = repo_with_base();
        run_git(Some(&src), &["branch", "release", &base]).unwrap();
        let root = tempfile::tempdir().unwrap();
        let (clone_a, _) = clone_with_change(
            root.path(),
            &src,
            &base,
            "impl-same-a",
            "same.txt",
            "same\n",
            ImplementPhase::Approved,
        );
        let (clone_b, _) = clone_with_change(
            root.path(),
            &src,
            &base,
            "impl-same-b",
            "same.txt",
            "same\n",
            ImplementPhase::Approved,
        );
        assert!(matches!(
            merge_clone(
                None,
                &clone_a,
                root.path(),
                Some("release"),
                false,
                MergeMode::ExactBase,
            ),
            MergeOutcome::Merged
        ));
        let already = run_git_str(&src, &["rev-parse", "release"]);

        // A verify-only reference transaction invokes this hook. The pre-fix same-value push sent no update
        // command and therefore never crossed this transaction boundary.
        let hook_log = root.path().join("reference-transaction.log");
        let hook = src.join(".git").join("hooks").join("reference-transaction");
        std::fs::write(
            &hook,
            format!(
                "#!/bin/sh\nprintf '%s\\n' \"$1\" >> '{}'\ncat >> '{}'\n",
                hook_log.display(),
                hook_log.display()
            ),
        )
        .unwrap();
        {
            use std::os::unix::fs::PermissionsExt;
            let mut permissions = std::fs::metadata(&hook).unwrap().permissions();
            permissions.set_mode(0o755);
            std::fs::set_permissions(&hook, permissions).unwrap();
        }
        warm_up_script(&hook);
        std::fs::remove_file(&hook_log).expect("the warm-up hook log");

        assert!(matches!(
            merge_clone(
                None,
                &clone_b,
                root.path(),
                Some("release"),
                false,
                MergeMode::IntegrateCurrent,
            ),
            MergeOutcome::Merged
        ));
        assert_eq!(run_git_str(&src, &["rev-parse", "release"]), already);
        assert!(!clone_b.exists());
        let transactions = std::fs::read_to_string(&hook_log).unwrap();
        assert!(transactions.lines().any(|line| line == "prepared"));
        assert!(transactions.lines().any(|line| line == "committed"));
    }

    #[test]
    fn integrate_current_identical_delta_move_before_compare_keeps_clone_and_new_target() {
        let (_gs, src, base) = repo_with_base();
        run_git(Some(&src), &["branch", "release", &base]).unwrap();
        let root = tempfile::tempdir().unwrap();
        let (clone_a, _) = clone_with_change(
            root.path(),
            &src,
            &base,
            "impl-race-a",
            "same.txt",
            "same\n",
            ImplementPhase::Approved,
        );
        let (clone_b, _) = clone_with_change(
            root.path(),
            &src,
            &base,
            "impl-race-b",
            "same.txt",
            "same\n",
            ImplementPhase::Approved,
        );
        assert!(matches!(
            merge_clone(
                None,
                &clone_a,
                root.path(),
                Some("release"),
                false,
                MergeMode::ExactBase,
            ),
            MergeOutcome::Merged
        ));
        let fetched = run_git_str(&src, &["rev-parse", "release"]);
        let tree = run_git_str(&src, &["rev-parse", &format!("{fetched}^{{tree}}")]);
        let moved_out = run_git(
            Some(&src),
            &[
                "commit-tree",
                &tree,
                "-p",
                &fetched,
                "-m",
                "concurrent destination move",
            ],
        )
        .unwrap();
        assert!(moved_out.status.success());
        let moved = String::from_utf8_lossy(&moved_out.stdout)
            .trim()
            .to_string();

        let operation = acquire_operation_lock(&clone_b).unwrap();
        let outcome = merge_clone_with_operation_lock_inner(
            operation,
            None,
            &clone_b,
            root.path(),
            Some("release"),
            MergeOptions {
                force: false,
                mode: MergeMode::IntegrateCurrent,
            },
            None,
            || {
                run_git(Some(&src), &["branch", "-f", "release", &moved]).unwrap();
            },
        );
        assert!(matches!(outcome, MergeOutcome::Unlanded));
        assert_eq!(run_git_str(&src, &["rev-parse", "release"]), moved);
        assert!(
            clone_b.exists(),
            "a stale compare-only transaction must retain the reviewed clone"
        );
    }

    #[test]
    fn merge_clone_refuses_loopstopped_without_force() {
        let (_gs, src, base) = repo_with_base();
        run_git(Some(&src), &["branch", "release", &base]).unwrap();
        let root = tempfile::tempdir().unwrap();
        let (clone, _cur) =
            clone_with_checkpoint(root.path(), &src, &base, ImplementPhase::LoopStopped);

        let out = merge_clone(
            None,
            &clone,
            root.path(),
            Some("release"),
            false,
            MergeMode::ExactBase,
        );
        assert!(matches!(out, MergeOutcome::UsageOrPreflight));
        assert!(clone.exists()); // kept
        assert_eq!(run_git_str(&src, &["rev-parse", "release"]), base); // not landed
    }
    #[test]
    fn both_landing_modes_write_exact_canonical_provenance_trailers() {
        for mode in [MergeMode::ExactBase, MergeMode::IntegrateCurrent] {
            let (_gs, src, base) = repo_with_base();
            run_git(Some(&src), &["checkout", "-q", "-b", "operator"]).unwrap();
            let root = tempfile::tempdir().unwrap();
            let (clone, current) =
                clone_with_checkpoint(root.path(), &src, &base, ImplementPhase::Approved);
            let mut ck = load_checkpoint(&clone).unwrap();
            ck.original_message = Some(
                "real subject\n\nA2A-Run: forged\na2a-original: bad\nA2A-Run: duplicate\n".into(),
            );
            crate::implement_resume::save_checkpoint(&clone, &ck).unwrap();
            assert_eq!(
                merge_clone(None, &clone, root.path(), Some("main"), false, mode).code(),
                0
            );
            let body = run_git_str(&src, &["show", "-s", "--format=%B", "main"]);
            assert_eq!(
                body.lines()
                    .filter(|l| l.starts_with("A2A-Run:"))
                    .collect::<Vec<_>>(),
                [format!("A2A-Run: {}", ck.task_id)]
            );
            assert_eq!(
                body.lines()
                    .filter(|l| l.starts_with("A2A-Original:"))
                    .collect::<Vec<_>>(),
                [format!("A2A-Original: {current}")]
            );
            assert!(
                !body.contains("forged") && !body.contains("bad") && !body.contains("duplicate")
            );
        }
    }

    #[test]
    fn failed_local_cleanup_after_landing_keeps_every_remote_ref() {
        let (_gs, src, base) = repo_with_base();
        run_git(Some(&src), &["branch", "release", &base]).unwrap();
        let root = tempfile::tempdir().unwrap();
        let wrong_root = tempfile::tempdir().unwrap();
        let (clone, _) = clone_with_checkpoint(root.path(), &src, &base, ImplementPhase::Approved);
        with_test_hooks(&clone, |original| {
            let mut custody = original.custody.clone();
            custody.delete_remote_on_merge = true;
            let mut hooks = MergeHooks {
                custody: &custody,
                evidence: original.evidence,
                post_landing: original.post_landing,
            };
            assert_eq!(
                super::merge_clone(
                    None,
                    &clone,
                    wrong_root.path(),
                    Some("release"),
                    false,
                    MergeMode::ExactBase,
                    Some(&mut hooks)
                )
                .code(),
                0
            );
            assert!(clone.exists());
            assert!(crate::custody_push::ls_remote_ref(
                &src,
                &clone,
                &custody,
                "refs/heads/wip/impl-1-abcd"
            )
            .unwrap()
            .is_some());
        });
    }
}
