//! `a2a-bridge merge <id>` — land an Approved run's commit into its source_repo, re-authored to the
//! operator, via `git commit-tree` + `git push --force-with-lease`. Mode A (`--onto`) only.
//! Pure core here; impure git ops + the CLI orchestrator follow. See ADR-0027.

use crate::implement::{
    commit_message, current_branch, head_sha, is_worktree_dirty, pin_prefix_argv, run_git,
};
use crate::implement_resume::{
    load_checkpoint, ImplementCheckpoint, ImplementPhase, SCHEMA_VERSION,
};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OperatorIdent {
    pub name: String,
    pub email: String,
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

/// `commit-tree` `current_commit`'s tree over `base_commit` as `op` (author==committer, same fresh date).
/// Reuses the identity-free pin prefix; identity via `GIT_*` env; message on stdin (`-F -`) so a multi-line
/// body survives. Does NOT move the clone's branch (retry-safe).
pub fn reauthor_commit(
    clone: &Path,
    current_commit: &str,
    base_commit: &str,
    msg: &str,
    op: &OperatorIdent,
) -> Result<String, String> {
    let t = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| e.to_string())?
        .as_secs();
    let date = format!("{t} +0000");
    let tree = format!("{current_commit}^{{tree}}");
    let mut argv = pin_prefix_argv(&clone.to_string_lossy());
    argv.extend([
        "commit-tree".into(),
        tree,
        "-p".into(),
        base_commit.into(),
        "-F".into(),
        "-".into(),
    ]);

    let mut cmd = std::process::Command::new("git");
    cmd.arg("-C")
        .arg(clone)
        .args(&argv)
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

/// Guard the commit-tree graft against a corrupted/unexpected clone (the bridge OWNS this dir — integrity,
/// not adversarial defense): both objects exist AND base is an ancestor of current.
pub fn check_clone_shape(
    clone: &Path,
    base_commit: &str,
    current_commit: &str,
) -> Result<(), String> {
    for obj in [base_commit, current_commit] {
        git_str(clone, &["cat-file", "-e", &format!("{obj}^{{commit}}")])
            .map_err(|_| format!("clone object {obj} missing — inspect {}", clone.display()))?;
    }
    let out = run_git(
        Some(clone),
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
    let refspec = format!("{reauthored}:refs/heads/{target}");
    let lease = format!("--force-with-lease=refs/heads/{target}:{base_commit}");
    let src = source_repo.to_string_lossy().to_string();
    let out = run_git(Some(clone), &["push", &src, &refspec, &lease])
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

/// Shared core: validate, gate, preflight, re-author, push, reap. Prints user-facing lines itself.
/// Both `merge_cmd` (after `resolve_clone`) and `implement --merge` (with a known clone) call this.
pub fn merge_clone(
    mcfg: Option<&crate::config::MergeConfig>,
    clone: &Path,
    root: &Path,
    onto: Option<&str>,
    force: bool,
) -> MergeOutcome {
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
            let (msg, _) = commit_message(ck.original_message.clone(), &ck.task_brief);
            let rt = match reauthor_commit(clone, cur, &ck.base_commit, &msg, &op) {
                Ok(r) => r,
                Err(e) => {
                    eprintln!("merge: {e}");
                    return MergeOutcome::Unlanded;
                }
            };
            match push_landing(clone, &src, &rt, &target, &ck.base_commit) {
                Ok(()) => {
                    if let Err(e) = reap_clone(clone, &src, root) {
                        eprintln!("merge: landed but {e}");
                    }
                    println!("merged {:.12} into {target}", rt);
                    MergeOutcome::Merged
                }
                Err(PushError::StaleLease) => {
                    eprintln!(
                        "merge: '{target}' moved off {:.12} since the clone was made. The clone's base is fixed, so re-running can't land it — start a fresh `implement` run off the moved '{target}'. (clone kept at {})",
                        ck.base_commit,
                        clone.display()
                    );
                    MergeOutcome::Unlanded
                }
                Err(PushError::Other(e)) => {
                    eprintln!("merge: push failed: {e}; clone kept at {}", clone.display());
                    MergeOutcome::Unlanded
                }
            }
        }
    }
}

/// `a2a-bridge merge <id> [--config <path>] [--onto <branch>] [--force]`
pub async fn merge_cmd(args: &[String]) -> Result<(), crate::BoxError> {
    let mut id: Option<String> = None;
    let mut config_path = std::path::PathBuf::from(crate::CONFIG_PATH);
    let mut onto: Option<String> = None;
    let mut force = false;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--config" => {
                i += 1;
                config_path = args.get(i).ok_or("merge: --config needs a path")?.into();
            }
            "--onto" => {
                i += 1;
                onto = Some(args.get(i).ok_or("merge: --onto needs a branch")?.clone());
            }
            "--force" => force = true,
            s if !s.starts_with('-') && id.is_none() => id = Some(s.to_string()),
            s => return Err(format!("merge: unexpected arg {s:?}").into()),
        }
        i += 1;
    }
    let id =
        id.ok_or("merge: missing <id> (usage: a2a-bridge merge <id> [--onto <branch>] [--force])")?;
    let config_path = std::fs::canonicalize(&config_path)
        .map_err(|e| format!("merge: config {}: {e}", config_path.display()))?;
    let raw =
        std::fs::read_to_string(&config_path).map_err(|e| format!("merge: read config: {e}"))?;
    let cfg = crate::config::RegistryConfig::parse(&raw)
        .map_err(|e| format!("merge: config parse: {e}"))?;
    let root = cfg
        .allowed_cwd_root
        .clone()
        .ok_or("merge: config needs allowed_cwd_root")?;
    let root = std::fs::canonicalize(&root)
        .map_err(|e| format!("merge: allowed_cwd_root {root:?}: {e}"))?;
    let mcfg = cfg
        .merge
        .as_ref()
        .map(|m| m.to_config())
        .transpose()
        .map_err(|e| format!("merge: {e}"))?;
    let clone =
        crate::implement_resume::resolve_clone(&root, &id).map_err(|e| format!("merge: {e}"))?;

    let outcome = merge_clone(mcfg.as_ref(), &clone, &root, onto.as_deref(), force);
    use std::io::Write;
    std::io::stdout().flush().ok();
    std::process::exit(outcome.code());
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
}

#[cfg(test)]
mod git_tests {
    use super::*;
    use std::path::PathBuf;

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

    /// Build a clone at `<root>/.a2a-implement/<id>` sharing `base` from `src`, with a bot commit on
    /// `implement/x`, plus a saved checkpoint at `phase`. Returns (clone_path, current_sha).
    fn clone_with_checkpoint(
        root: &Path,
        src: &Path,
        base: &str,
        phase: ImplementPhase,
    ) -> (PathBuf, String) {
        use crate::implement_resume::save_checkpoint;
        let id = "impl-1-abcd";
        let clone = root.join(".a2a-implement").join(id);
        std::fs::create_dir_all(&clone).unwrap();
        run_git(Some(&clone), &["init", "-q", "-b", "implement/x"]).unwrap();
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
        std::fs::write(clone.join("w.txt"), "w\n").unwrap();
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
            branch: "implement/x".into(),
            base_ref: Some("release".into()),
            base_commit: base.to_string(),
            current_commit: Some(current.clone()),
            original_message: Some("land x".into()),
            edit_workflow: "e".into(),
            fix_workflow: "f".into(),
            loop_max_attempts: 1,
            attempt_next: 1,
            phase,
            created_at_ms: 0,
            updated_at_ms: 0,
        };
        save_checkpoint(&clone, &ck).unwrap();
        (clone, current)
    }

    #[test]
    fn merge_clone_happy_path_lands_and_reaps() {
        let (_gs, src, base) = repo_with_base();
        run_git(Some(&src), &["branch", "release", &base]).unwrap();
        let root = tempfile::tempdir().unwrap();
        let (clone, _cur) =
            clone_with_checkpoint(root.path(), &src, &base, ImplementPhase::Approved);

        let out = merge_clone(None, &clone, root.path(), Some("release"), false);
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
    fn merge_clone_refuses_loopstopped_without_force() {
        let (_gs, src, base) = repo_with_base();
        run_git(Some(&src), &["branch", "release", &base]).unwrap();
        let root = tempfile::tempdir().unwrap();
        let (clone, _cur) =
            clone_with_checkpoint(root.path(), &src, &base, ImplementPhase::LoopStopped);

        let out = merge_clone(None, &clone, root.path(), Some("release"), false);
        assert!(matches!(out, MergeOutcome::UsageOrPreflight));
        assert!(clone.exists()); // kept
        assert_eq!(run_git_str(&src, &["rev-parse", "release"]), base); // not landed
    }
}
