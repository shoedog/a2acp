# Merge Hand-off (`a2a-bridge merge <id>`) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build `a2a-bridge merge <id>` (+ an `implement --merge` sugar) that lands an `Approved` run's commit into its `source_repo`, re-authored to the operator, via `git commit-tree` + `git push --force-with-lease`, never touching the operator's checkout — Mode A (`--onto`) only.

**Architecture:** A new `bin/a2a-bridge/src/merge.rs` module mirroring `implement_resume.rs`: a pure, git-free core (`MergePlan`/`decide_merge`/`resolve_target`) unit-tested exhaustively, plus impure git ops (`reauthor_commit`/`push_landing`/`operator_from`/`source_head`/`reap_clone`) tested over temp repos (docker-free). The CLI orchestrator (`merge_clone`/`merge_cmd`) is shared by the `merge` subcommand and the `implement --merge` sugar. `git push --force-with-lease=refs/heads/<target>:<base_commit>` IS the concurrency CAS — no external lock.

**Tech Stack:** Rust (workspace), `std::process::Command` via the existing `implement::run_git` helper, `serde`/`toml` for config, `tempfile` for tests. No new dependencies.

**Spec:** `docs/superpowers/specs/2026-06-08-merge-handoff-design.md` (v6, Mode-A-only, consolidated).

**Ground-truth references (read before starting):**
- `bin/a2a-bridge/src/implement.rs`: `commit_argv` (line 36), `BOT_NAME`/`BOT_EMAIL` (13–14), `run_git` (149, `pub`), `head_sha`/`current_branch`/`is_worktree_dirty` (196–214), `commit_message(raw: Option<String>, task: &str) -> (String, bool)` (60), the `temp_repo()` test helper (545) and `host_commit` test helper.
- `bin/a2a-bridge/src/implement_resume.rs`: `ImplementPhase` (7), `ImplementCheckpoint` (17; fields `schema_version: u32`, `task_id`, `task_brief`, `source_repo: PathBuf`, `branch`, `base_ref: Option<String>`, `base_commit: String`, `current_commit: Option<String>`, `original_message: Option<String>`, `phase`), `SCHEMA_VERSION: u32 = 1` (43), `resolve_clone(allowed_cwd_root, id)` (111), `load_checkpoint(clone)` (66).
- `bin/a2a-bridge/src/main.rs`: subcommand dispatch (2380), `TOP_USAGE` (82), `run_warm_loop` (943, returns `()`, computes the `Approved`/`LoopStopped` terminal + prints the hand-off at 999–1020), its fresh caller (~1304) and resume caller (~1473), and the per-subcommand config load pattern (`config::RegistryConfig::parse(&raw)` + `cfg.allowed_cwd_root` at 1046–1056).
- `bin/a2a-bridge/src/config.rs`: the `ImplementToml`/`to_config` fail-loud pattern and `RegistryConfig` (the `[implement]`/`[verify]`/`[review]` optional-block style to mirror for `[merge]`).

**Conventions:** TDD per task (failing test first). Run `cargo test -p a2a-bridge <name>` to scope. Commit after each task with a `feat(merge):`/`refactor(merge):` message. Subagent task commits do NOT carry the `Co-Authored-By` trailer. Keep `cargo fmt --all -- --check` and `cargo clippy -p a2a-bridge --all-targets -- -D warnings` green (gate both before each commit — a fmt nit slipped a prior increment).

---

### Task 1: Extract the identity-free git-config pin prefix

Extract the three pins shared by `commit_argv` (and the second commit-builder in `implement.rs`) into one helper, so `reauthor_commit` (Task 3) reuses the EXACT pins with a different identity mechanism. Identity stays per-caller (`commit_argv` keeps `-c user.name/email`; `reauthor_commit` will use `GIT_*` env).

**Files:**
- Modify: `bin/a2a-bridge/src/implement.rs:36-53` (`commit_argv`) and the second pin site (~287)
- Test: `bin/a2a-bridge/src/implement.rs` (the existing `commit_argv_pins_before_commit` test at ~428 must stay green)

- [ ] **Step 1: Write the failing test**

Add to the `#[cfg(test)] mod tests` in `implement.rs`:

```rust
#[test]
fn pin_prefix_is_identity_free_and_complete() {
    let p = pin_prefix_argv("/root/.a2a-implement/impl-1-ab");
    let joined = p.join(" ");
    assert_eq!(joined, "-c safe.directory=/root/.a2a-implement/impl-1-ab -c core.hooksPath=/dev/null -c commit.gpgsign=false");
    // identity-free: no user.name / user.email here
    assert!(!joined.contains("user.name"));
    assert!(!joined.contains("user.email"));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p a2a-bridge pin_prefix_is_identity_free -- --exact 2>&1 | tail -5`
Expected: FAIL — `cannot find function pin_prefix_argv`.

- [ ] **Step 3: Implement the helper and refactor `commit_argv`**

Add above `commit_argv` in `implement.rs`:

```rust
/// The identity-FREE git `-c` pins shared by every bridge-driven commit: dubious-ownership guard,
/// hook suppression, no signing. Identity is attached per-caller (`-c user.*` for `commit`,
/// `GIT_*` env for `commit-tree`) — NOT shared, so callers can't accidentally cross identities.
pub fn pin_prefix_argv(clone: &str) -> Vec<String> {
    vec![
        "-c".into(),
        format!("safe.directory={clone}"),
        "-c".into(),
        "core.hooksPath=/dev/null".into(),
        "-c".into(),
        "commit.gpgsign=false".into(),
    ]
}
```

Rewrite `commit_argv` to reuse it:

```rust
pub fn commit_argv(clone: &str, msg: &str) -> Vec<String> {
    let mut v = pin_prefix_argv(clone);
    v.extend([
        "-c".into(),
        format!("user.name={BOT_NAME}"),
        "-c".into(),
        format!("user.email={BOT_EMAIL}"),
        "commit".into(),
        "--no-verify".into(),
        "-m".into(),
        msg.into(),
    ]);
    v
}
```

Apply the same `pin_prefix_argv(clone)` reuse to the second pin site at ~287 (the other commit builder — replace its three inline `-c safe.directory / core.hooksPath / commit.gpgsign` pairs with a `pin_prefix_argv` prefix, leaving its identity `-c user.*` and subcommand untouched).

- [ ] **Step 4: Run the tests**

Run: `cargo test -p a2a-bridge pin_prefix commit_argv 2>&1 | tail -8`
Expected: PASS (new test + the existing `commit_argv_pins_before_commit` and the second builder's test).

- [ ] **Step 5: fmt + clippy + commit**

```bash
cargo fmt --all && cargo clippy -p a2a-bridge --all-targets -- -D warnings
git add bin/a2a-bridge/src/implement.rs
git commit -m "refactor(merge): extract identity-free pin_prefix_argv (slice 1)"
```

---

### Task 2: Pure core — `MergePlan`, `OperatorIdent`, `decide_merge`, `resolve_target`

The git-free decision core. Mode-independent (`decide_merge` takes no mode — Mode A is the only mode; the deferred Mode B reuses it unchanged).

**Files:**
- Create: `bin/a2a-bridge/src/merge.rs`
- Modify: `bin/a2a-bridge/src/main.rs` (add `mod merge;` near the other `mod` decls, ~top of file)
- Test: `bin/a2a-bridge/src/merge.rs` (`#[cfg(test)]`)

- [ ] **Step 1: Create the module skeleton + register it**

Create `bin/a2a-bridge/src/merge.rs`:

```rust
//! `a2a-bridge merge <id>` — land an Approved run's commit into its source_repo, re-authored to the
//! operator, via `git commit-tree` + `git push --force-with-lease`. Mode A (`--onto`) only.
//! Pure core here; impure git ops + the CLI orchestrator follow.

use crate::implement_resume::ImplementPhase;

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
```

Add `mod merge;` in `main.rs` alongside the existing `mod implement;` / `mod implement_resume;` declarations.

- [ ] **Step 2: Write the failing `decide_merge` test**

In `merge.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::implement_resume::ImplementPhase::*;

    fn ok(t: &str) -> Result<String, String> { Ok(t.to_string()) }

    #[test]
    fn gate_matrix() {
        // Approved + has commit + resolvable target -> Merge
        assert_eq!(decide_merge(Approved, true, false, &ok("main")), MergePlan::Merge { target: "main".into() });
        // Approved but NO commit -> RefuseHard (force can't help)
        assert!(matches!(decide_merge(Approved, false, true, &ok("main")), MergePlan::RefuseHard(_)));
        // LoopStopped w/o force -> Refuse; with force -> Merge
        assert!(matches!(decide_merge(LoopStopped, true, false, &ok("main")), MergePlan::Refuse(_)));
        assert_eq!(decide_merge(LoopStopped, true, true, &ok("main")), MergePlan::Merge { target: "main".into() });
        // Non-terminal phases -> RefuseHard even with force
        for p in [Cloned, EditStarted, FirstCommitCreated, InLoop] {
            assert!(matches!(decide_merge(p, true, true, &ok("main")), MergePlan::RefuseHard(_)));
        }
        // Unresolvable target -> Refuse (even when Approved)
        assert!(matches!(decide_merge(Approved, true, false, &Err("no target".into())), MergePlan::Refuse(_)));
    }
}
```

- [ ] **Step 3: Run it (fails — `decide_merge` undefined)**

Run: `cargo test -p a2a-bridge gate_matrix -- --exact 2>&1 | tail -5`
Expected: FAIL — `cannot find function decide_merge`.

- [ ] **Step 4: Implement `decide_merge`**

```rust
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
                return MergePlan::RefuseHard("run finished without a commit — nothing to merge".into());
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
```

- [ ] **Step 5: Run it (passes)**

Run: `cargo test -p a2a-bridge gate_matrix -- --exact 2>&1 | tail -5`
Expected: PASS.

- [ ] **Step 6: Write the failing `resolve_target` test**

```rust
#[test]
fn resolve_target_precedence_and_validation() {
    // precedence: --onto > [merge].target_ref > base_ref
    assert_eq!(resolve_target(Some("a"), Some("b"), Some("c")), Ok("a".into()));
    assert_eq!(resolve_target(None, Some("b"), Some("c")), Ok("b".into()));
    assert_eq!(resolve_target(None, None, Some("c")), Ok("c".into()));
    assert!(resolve_target(None, None, None).is_err());
    // best-effort rejects (string-decidable only)
    for bad in ["", "HEAD", "refs/heads/main", "refs/tags/v1", "origin/main",
                "feat..x", "-bad", "trailing/", "ends.lock", "a b", "x~1", "deadbeefdeadbeefdeadbeefdeadbeefdeadbeef"] {
        assert!(resolve_target(Some(bad), None, None).is_err(), "{bad:?} should be rejected");
    }
    // accepted ordinary names
    for good in ["main", "feature/x", "release/1.2", "dev"] {
        assert_eq!(resolve_target(Some(good), None, None), Ok(good.to_string()));
    }
}
```

- [ ] **Step 7: Run it (fails)**

Run: `cargo test -p a2a-bridge resolve_target_precedence -- --exact 2>&1 | tail -5`
Expected: FAIL — `cannot find function resolve_target`.

- [ ] **Step 8: Implement `resolve_target` + `valid_branch_name`**

```rust
/// Best-effort, PURE branch-name pre-check (NOT full check-ref-format parity; git is authoritative at
/// the push boundary). Rejects only what the STRING decides.
fn valid_branch_name(s: &str) -> bool {
    if s.is_empty() || s == "HEAD" { return false; }
    if s.starts_with('-') || s.ends_with('/') || s.ends_with('.') || s.ends_with(".lock") { return false; }
    if s.starts_with("refs/") || s.starts_with("origin/") { return false; }
    if s.contains("..") { return false; }
    // raw 40-hex SHA
    if s.len() == 40 && s.chars().all(|c| c.is_ascii_hexdigit()) { return false; }
    // forbidden chars (space, control, and git's special set) + "@{" + backslash
    if s.chars().any(|c| c.is_whitespace() || c.is_control() || "~^:?*[\\".contains(c)) { return false; }
    if s.contains("@{") { return false; }
    // no path component starting with '.'
    if s.split('/').any(|seg| seg.starts_with('.') || seg.is_empty()) { return false; }
    true
}

/// Precedence: --onto > [merge].target_ref > checkpoint.base_ref. Returns a SHORT branch name.
pub fn resolve_target(
    cli_onto: Option<&str>,
    cfg: Option<&str>,
    base_ref: Option<&str>,
) -> Result<String, String> {
    let raw = cli_onto.or(cfg).or(base_ref)
        .ok_or_else(|| "no target — pass --onto <branch> or set [merge].target_ref".to_string())?;
    if !valid_branch_name(raw) {
        return Err(format!("invalid branch name {raw:?} (pass a plain branch like `main`)"));
    }
    Ok(raw.to_string())
}
```

- [ ] **Step 9: Run it (passes), fmt/clippy, commit**

Run: `cargo test -p a2a-bridge merge:: 2>&1 | tail -8` (Expected: PASS)

```bash
cargo fmt --all && cargo clippy -p a2a-bridge --all-targets -- -D warnings
git add bin/a2a-bridge/src/merge.rs bin/a2a-bridge/src/main.rs
git commit -m "feat(merge): pure core — MergePlan/decide_merge/resolve_target (slice 2)"
```

---

### Task 3: `reauthor_commit` + clone shape/ancestry preflight (impure, temp-repo)

Re-author the clone's commit as the operator via `git commit-tree` (clone branch unmoved → retry-safe), and a preflight that proves the clone's one-commit-over-base shape before the graft.

**Files:**
- Modify: `bin/a2a-bridge/src/merge.rs`
- Test: `bin/a2a-bridge/src/merge.rs` (`#[cfg(test)]`, temp repos)

- [ ] **Step 1: Add a temp-repo test helper + the failing `reauthor_commit` test**

In `merge.rs` tests:

```rust
#[cfg(test)]
mod git_tests {
    use super::*;
    use crate::implement::run_git;
    use std::path::{Path, PathBuf};

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
        run_git(Some(&p), &["-c", "user.name=a2a-implement", "-c", "user.email=implement@a2a-bridge.local",
                            "commit", "-q", "-m", "bot did work"]).unwrap();
        let current = run_git_str(&p, &["rev-parse", "HEAD"]);
        let op = OperatorIdent { name: "Op Erator".into(), email: "op@example.com".into() };

        let rt = reauthor_commit(&p, &current, &base, "land it", &op).unwrap();

        // operator identity on both author and committer
        assert_eq!(run_git_str(&p, &["log", "-1", "--format=%an <%ae>", &rt]), "Op Erator <op@example.com>");
        assert_eq!(run_git_str(&p, &["log", "-1", "--format=%cn <%ce>", &rt]), "Op Erator <op@example.com>");
        // author date == committer date (both the captured T)
        assert_eq!(run_git_str(&p, &["log", "-1", "--format=%at", &rt]),
                   run_git_str(&p, &["log", "-1", "--format=%ct", &rt]));
        // same tree as current; parent is base
        assert_eq!(run_git_str(&p, &["rev-parse", &format!("{rt}^{{tree}}")]),
                   run_git_str(&p, &["rev-parse", &format!("{current}^{{tree}}")]));
        assert_eq!(run_git_str(&p, &["rev-parse", &format!("{rt}^")]), base);
        // clone branch UNMOVED (retry-safe)
        assert_eq!(run_git_str(&p, &["rev-parse", "HEAD"]), current);
    }
}
```

- [ ] **Step 2: Run it (fails)**

Run: `cargo test -p a2a-bridge reauthor_sets_operator -- --exact 2>&1 | tail -5`
Expected: FAIL — `cannot find function reauthor_commit`.

- [ ] **Step 3: Implement `reauthor_commit` (+ a small git helper)**

In `merge.rs` (non-test):

```rust
use crate::implement::{pin_prefix_argv, run_git};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

/// Run git in `cwd`, returning trimmed stdout on success or a formatted Err.
fn git_str(cwd: &Path, args: &[&str]) -> Result<String, String> {
    let out = run_git(Some(cwd), args).map_err(|e| format!("git {}: {e}", args.join(" ")))?;
    if !out.status.success() {
        return Err(format!("git {} failed: {}", args.join(" "), String::from_utf8_lossy(&out.stderr).trim()));
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
    let t = SystemTime::now().duration_since(UNIX_EPOCH).map_err(|e| e.to_string())?.as_secs();
    let date = format!("{t} +0000");
    let tree = format!("{current_commit}^{{tree}}");
    let mut argv = pin_prefix_argv(&clone.to_string_lossy());
    argv.extend(["commit-tree".into(), tree, "-p".into(), base_commit.into(), "-F".into(), "-".into()]);

    let mut cmd = std::process::Command::new("git");
    cmd.arg("-C").arg(clone)
        .args(&argv)
        .env("GIT_AUTHOR_NAME", &op.name).env("GIT_AUTHOR_EMAIL", &op.email).env("GIT_AUTHOR_DATE", &date)
        .env("GIT_COMMITTER_NAME", &op.name).env("GIT_COMMITTER_EMAIL", &op.email).env("GIT_COMMITTER_DATE", &date)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let mut child = cmd.spawn().map_err(|e| format!("git commit-tree spawn: {e}"))?;
    {
        use std::io::Write;
        child.stdin.take().unwrap().write_all(msg.as_bytes()).map_err(|e| format!("commit-tree stdin: {e}"))?;
    }
    let out = child.wait_with_output().map_err(|e| format!("commit-tree wait: {e}"))?;
    if !out.status.success() {
        return Err(format!("git commit-tree failed: {}", String::from_utf8_lossy(&out.stderr).trim()));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}
```

- [ ] **Step 4: Run it (passes)**

Run: `cargo test -p a2a-bridge reauthor_sets_operator -- --exact 2>&1 | tail -5`
Expected: PASS.

- [ ] **Step 5: Write the failing clone-shape test**

```rust
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
```

- [ ] **Step 6: Run it (fails), then implement `check_clone_shape`**

Run: `cargo test -p a2a-bridge clone_shape_rejects -- --exact 2>&1 | tail -5` → FAIL.

```rust
/// Guard the commit-tree graft against a corrupted/unexpected clone (the bridge OWNS this dir — integrity,
/// not adversarial defense): both objects exist AND base is an ancestor of current.
pub fn check_clone_shape(clone: &Path, base_commit: &str, current_commit: &str) -> Result<(), String> {
    for obj in [base_commit, current_commit] {
        git_str(clone, &["cat-file", "-e", &format!("{obj}^{{commit}}")])
            .map_err(|_| format!("clone object {obj} missing — inspect {}", clone.display()))?;
    }
    let out = run_git(Some(clone), &["merge-base", "--is-ancestor", base_commit, current_commit])
        .map_err(|e| format!("git merge-base: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "clone history unexpected: base {base_commit:.12} is not an ancestor of the run commit \
             {current_commit:.12} — inspect {}", clone.display()
        ));
    }
    Ok(())
}
```

- [ ] **Step 7: Run it (passes), fmt/clippy, commit**

Run: `cargo test -p a2a-bridge merge:: 2>&1 | tail -8` (Expected: PASS)

```bash
cargo fmt --all && cargo clippy -p a2a-bridge --all-targets -- -D warnings
git add bin/a2a-bridge/src/merge.rs
git commit -m "feat(merge): reauthor_commit + clone shape/ancestry preflight (slice 3)"
```

---

### Task 4: `push_landing` + observable-state classification

Push the re-authored commit with the lease as the CAS; classify failures by post-failure ref state (never by stderr text).

**Files:**
- Modify: `bin/a2a-bridge/src/merge.rs`
- Test: `bin/a2a-bridge/src/merge.rs`

- [ ] **Step 1: Write the failing FF + StaleLease + source-unchanged test**

Add a helper to make a non-bare "source" repo whose checked-out branch is NOT the target, plus a clone:

```rust
#[test]
fn push_landing_ff_then_stalelease_and_source_unchanged() {
    // source: non-bare repo on `main`, sitting on `base`; we land onto `release` (not checked out).
    let (_gs, src, base) = repo_with_base();
    run_git(Some(&src), &["branch", "release", &base]).unwrap();         // release == base
    // clone: a sibling repo whose work descends from base; reauthor over base.
    let (_gc, clone, _b2) = repo_with_base();
    // make clone share `base` object by fetching from src, then build a descendant + reauthor:
    run_git(Some(&clone), &["fetch", "-q", src.to_str().unwrap(), &format!("{base}:refs/heads/from-src")]).unwrap();
    run_git(Some(&clone), &["checkout", "-q", "from-src"]).unwrap();
    std::fs::write(clone.join("w.txt"), "w\n").unwrap();
    run_git(Some(&clone), &["add", "."]).unwrap();
    run_git(Some(&clone), &["commit", "-q", "-m", "work"]).unwrap();
    let current = run_git_str(&clone, &["rev-parse", "HEAD"]);
    let op = OperatorIdent { name: "Op".into(), email: "op@x.com".into() };
    let rt = reauthor_commit(&clone, &current, &base, "land", &op).unwrap();

    // capture source-unchanged baseline
    let src_head_before = run_git_str(&src, &["rev-parse", "HEAD"]);
    let status_before = run_git_str(&src, &["status", "--porcelain=v1", "--untracked-files=all"]);

    // FF: release is at base -> lands
    assert!(push_landing(&clone, &src, &rt, "release", &base).is_ok());
    assert_eq!(run_git_str(&src, &["rev-parse", "release"]), rt);

    // operator checkout untouched (HEAD + worktree byte-identical)
    assert_eq!(run_git_str(&src, &["rev-parse", "HEAD"]), src_head_before);
    assert_eq!(run_git_str(&src, &["status", "--porcelain=v1", "--untracked-files=all"]), status_before);

    // StaleLease: release moved off base -> a second push with the SAME base lease refuses, ref NOT moved.
    let moved_to = run_git_str(&src, &["rev-parse", "release"]); // now == rt, != base
    let rt2 = reauthor_commit(&clone, &current, &base, "land again", &op).unwrap();
    assert!(matches!(push_landing(&clone, &src, &rt2, "release", &base), Err(PushError::StaleLease)));
    assert_eq!(run_git_str(&src, &["rev-parse", "release"]), moved_to); // unchanged
}
```

- [ ] **Step 2: Run it (fails — `push_landing`/`PushError` undefined)**

Run: `cargo test -p a2a-bridge push_landing_ff_then -- --exact 2>&1 | tail -5`
Expected: FAIL.

- [ ] **Step 3: Implement `PushError` + `push_landing`**

```rust
#[derive(Debug)]
pub enum PushError {
    StaleLease,
    CheckedOutTarget,
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
    let now = run_git(Some(source_repo), &["rev-parse", "-q", "--verify", &format!("refs/heads/{target}")])
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
```

- [ ] **Step 4: Run it (passes)**

Run: `cargo test -p a2a-bridge push_landing_ff_then -- --exact 2>&1 | tail -5`
Expected: PASS.

- [ ] **Step 5: Write the failing concurrency test**

```rust
#[test]
fn concurrent_pushes_one_wins() {
    let (_gs, src, base) = repo_with_base();
    run_git(Some(&src), &["branch", "line", &base]).unwrap();
    // two clones, each a distinct descendant of base, each reauthored
    let mk = |name: &str| -> (tempfile::TempDir, PathBuf, String) {
        let (g, c, _b) = repo_with_base();
        run_git(Some(&c), &["fetch", "-q", src.to_str().unwrap(), &format!("{base}:refs/heads/s")]).unwrap();
        run_git(Some(&c), &["checkout", "-q", "s"]).unwrap();
        std::fs::write(c.join(format!("{name}.txt")), "x\n").unwrap();
        run_git(Some(&c), &["add", "."]).unwrap();
        run_git(Some(&c), &["commit", "-q", "-m", name]).unwrap();
        let cur = run_git_str(&c, &["rev-parse", "HEAD"]);
        let op = OperatorIdent { name: "Op".into(), email: "op@x.com".into() };
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
```

- [ ] **Step 6: Run it (passes — no new code needed), fmt/clippy, commit**

Run: `cargo test -p a2a-bridge concurrent_pushes_one_wins -- --exact 2>&1 | tail -5` (Expected: PASS)

```bash
cargo fmt --all && cargo clippy -p a2a-bridge --all-targets -- -D warnings
git add bin/a2a-bridge/src/merge.rs
git commit -m "feat(merge): push_landing with lease CAS + observable classification (slice 4)"
```

---

### Task 5: `[merge]` config — `MergeToml`/`MergeConfig`/`to_config`

Fail-loud config block mirroring `ImplementToml`.

**Files:**
- Modify: `bin/a2a-bridge/src/config.rs` (add `MergeToml`/`MergeConfig`, the `merge` field on the toml struct + on `RegistryConfig` if it exposes parsed sub-configs, and `to_config`)
- Test: `bin/a2a-bridge/src/config.rs` (`#[cfg(test)]`)

> Read the existing `ImplementToml` + its `to_config` in `config.rs` first and mirror its exact placement (the raw toml struct vs the validated config, and how `RegistryConfig::parse` surfaces it).

- [ ] **Step 1: Write the failing config tests**

```rust
#[test]
fn merge_config_validation() {
    // both identity halves -> Some
    let raw = r#"
allowed_cwd_root = "/x"
[merge]
target_ref = "main"
author_name = "Op"
author_email = "op@x.com"
"#;
    let cfg = RegistryConfig::parse(raw).unwrap();
    let m = cfg.merge_config().unwrap().unwrap();
    assert_eq!(m.target_ref.as_deref(), Some("main"));
    assert_eq!(m.author.as_ref().unwrap().email, "op@x.com");

    // half identity -> error
    let half = "[merge]\nauthor_name = \"Op\"\n";
    assert!(RegistryConfig::parse(half).unwrap().merge_config().is_err());

    // empty target_ref -> error
    let empty = "[merge]\ntarget_ref = \"\"\n";
    assert!(RegistryConfig::parse(empty).unwrap().merge_config().is_err());

    // absent [merge] -> Ok(None)
    let none = "allowed_cwd_root = \"/x\"\n";
    assert!(RegistryConfig::parse(none).unwrap().merge_config().unwrap().is_none());
}
```

- [ ] **Step 2: Run it (fails)**

Run: `cargo test -p a2a-bridge merge_config_validation -- --exact 2>&1 | tail -5`
Expected: FAIL — `no method merge_config` / `no field merge`.

- [ ] **Step 3: Add the toml struct + validated config + accessor**

In `config.rs`, add the raw block near `ImplementToml`:

```rust
#[derive(Debug, Clone, serde::Deserialize)]
pub struct MergeToml {
    pub target_ref: Option<String>,
    pub author_name: Option<String>,
    pub author_email: Option<String>,
}

#[derive(Debug, Clone)]
pub struct MergeConfig {
    pub target_ref: Option<String>,
    pub author: Option<crate::merge::OperatorIdent>,
}

impl MergeToml {
    pub fn to_config(&self) -> Result<MergeConfig, String> {
        if let Some(t) = &self.target_ref {
            if t.trim().is_empty() {
                return Err("[merge].target_ref must be non-empty".into());
            }
        }
        let author = match (&self.author_name, &self.author_email) {
            (Some(n), Some(e)) => Some(crate::merge::OperatorIdent { name: n.clone(), email: e.clone() }),
            (None, None) => None,
            _ => return Err("[merge] author_name and author_email must BOTH be set or both omitted".into()),
        };
        Ok(MergeConfig { target_ref: self.target_ref.clone(), author })
    }
}
```

Add `#[serde(default)] pub merge: Option<MergeToml>,` to the raw config struct that `RegistryConfig::parse` deserializes (the same struct that holds `implement: Option<ImplementToml>`), and an accessor on `RegistryConfig`:

```rust
/// Validated `[merge]` config: Ok(None) when absent, Err on a malformed block.
pub fn merge_config(&self) -> Result<Option<MergeConfig>, String> {
    self.merge.as_ref().map(|m| m.to_config()).transpose()
}
```

(If `RegistryConfig` stores the raw toml inside a field rather than flattening, mirror exactly how `implement` is surfaced — read the existing code and follow it; do NOT introduce a new pattern.)

- [ ] **Step 4: Run it (passes), fmt/clippy, commit**

Run: `cargo test -p a2a-bridge merge_config_validation -- --exact 2>&1 | tail -5` (Expected: PASS)

```bash
cargo fmt --all && cargo clippy -p a2a-bridge --all-targets -- -D warnings
git add bin/a2a-bridge/src/config.rs
git commit -m "feat(merge): [merge] config — MergeToml/MergeConfig/to_config (slice 5)"
```

---

### Task 6: Orchestration + CLI — `operator_from`, source preflight, `reap_clone`, `merge_clone`, `merge_cmd`

Wire the impure ops into the end-to-end command. This is the integration task; use a standard model.

**Files:**
- Modify: `bin/a2a-bridge/src/merge.rs` (`operator_from`, `source_head`, `is_bare`, `reap_clone`, `merge_clone`, `merge_cmd`)
- Modify: `bin/a2a-bridge/src/main.rs` (dispatch arm + `TOP_USAGE` + the unknown-subcommand list)
- Test: `bin/a2a-bridge/src/merge.rs`

- [ ] **Step 1: Write failing tests for `operator_from`, `source_head`/`is_bare`, `reap_clone`**

```rust
#[test]
fn operator_from_sources_config_then_repo_then_fails() {
    let (_g, src, _b) = repo_with_base(); // sets user.name/email
    // override wins
    let ov = OperatorIdent { name: "Cfg".into(), email: "cfg@x.com".into() };
    assert_eq!(operator_from(&src, Some(&ov)).unwrap(), ov);
    // repo config when no override
    let got = operator_from(&src, None).unwrap();
    assert_eq!(got.email, "op@example.com");
    // unset -> error
    run_git(Some(&src), &["config", "--unset", "user.name"]).unwrap();
    run_git(Some(&src), &["config", "--unset", "user.email"]).unwrap();
    assert!(operator_from(&src, None).is_err());
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
    // wrong shape (clone == src guard): a path that is not under <root>/.a2a-implement/<id> is refused
    assert!(reap_clone(&src, &src, root.path()).is_err());
    // correct shape -> deletes
    assert!(reap_clone(&clone, &src, root.path()).is_ok());
    assert!(!clone.exists());
}
```

- [ ] **Step 2: Run it (fails)**

Run: `cargo test -p a2a-bridge operator_from_sources source_head_and_is_bare reap_clone_guards 2>&1 | tail -8`
Expected: FAIL — those functions are undefined.

- [ ] **Step 3: Implement the impure helpers**

```rust
/// source_repo git config user.name+user.email, or a `[merge]` override. Fail loud if EITHER half is
/// missing and there is no override.
pub fn operator_from(repo: &Path, cfg_override: Option<&OperatorIdent>) -> Result<OperatorIdent, String> {
    if let Some(o) = cfg_override {
        return Ok(o.clone());
    }
    let name = git_str(repo, &["config", "--get", "user.name"])
        .map_err(|_| format!("operator identity unset: `git -C {} config user.name` is empty — set it or add [merge] author_name/author_email", repo.display()))?;
    let email = git_str(repo, &["config", "--get", "user.email"])
        .map_err(|_| format!("operator identity unset: `git -C {} config user.email` is empty — set it or add [merge] author_name/author_email", repo.display()))?;
    if name.is_empty() || email.is_empty() {
        return Err(format!("operator identity unset in {} — set user.name/user.email or [merge] author_*", repo.display()));
    }
    Ok(OperatorIdent { name, email })
}

/// The source's checked-out branch (short name), or None when detached / unborn.
pub fn source_head(repo: &Path) -> Result<Option<String>, String> {
    let out = run_git(Some(repo), &["symbolic-ref", "--short", "-q", "HEAD"])
        .map_err(|e| format!("git symbolic-ref: {e}"))?;
    if out.status.success() {
        Ok(Some(String::from_utf8_lossy(&out.stdout).trim().to_string()))
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
    let id = cclone.file_name().and_then(|s| s.to_str()).ok_or("clone has no basename")?;
    let expected = croot.join(".a2a-implement").join(id);
    if cclone != expected || !cclone.join(".git").exists() || !cclone.starts_with(&croot) || cclone == csrc {
        return Err(format!("refusing to reap unexpected path {} (keeping it)", cclone.display()));
    }
    std::fs::remove_dir_all(&cclone).map_err(|e| format!("remove clone {}: {e}", cclone.display()))
}
```

- [ ] **Step 4: Run the helper tests (pass)**

Run: `cargo test -p a2a-bridge operator_from_sources source_head_and_is_bare reap_clone_guards 2>&1 | tail -8`
Expected: PASS.

- [ ] **Step 5: Implement `merge_clone` (the shared orchestrator) + `MergeOutcome`**

```rust
use crate::implement::{current_branch, head_sha, is_worktree_dirty, commit_message};
use crate::implement_resume::{load_checkpoint, ImplementCheckpoint, SCHEMA_VERSION};

/// The CLI exit semantics. `.code()` maps to the process exit code (see the spec's exit table).
pub enum MergeOutcome {
    Merged,            // 0
    UsageOrPreflight,  // 1
    Unlanded,          // 3 (Approved but couldn't land / preflight-at-merge)
}
impl MergeOutcome { pub fn code(&self) -> i32 { match self { Self::Merged => 0, Self::UsageOrPreflight => 1, Self::Unlanded => 3 } } }

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
        Err(e) => { eprintln!("merge: {e}"); return MergeOutcome::UsageOrPreflight; }
    };
    if ck.schema_version != SCHEMA_VERSION {
        eprintln!("merge: checkpoint schema {} unsupported (expects {SCHEMA_VERSION}) — rebuild with a current run", ck.schema_version);
        return MergeOutcome::UsageOrPreflight;
    }
    let src = match std::fs::canonicalize(&ck.source_repo)
        .ok()
        .filter(|p| run_git(Some(p), &["rev-parse", "--git-dir"]).map(|o| o.status.success()).unwrap_or(false))
    {
        Some(p) => p,
        None => { eprintln!("merge: source repo {:?} gone/moved/not-a-git-repo — keeping clone", ck.source_repo); return MergeOutcome::UsageOrPreflight; }
    };
    let target = resolve_target(onto, mcfg.and_then(|m| m.target_ref.as_deref()), ck.base_ref.as_deref());
    match decide_merge(ck.phase, ck.current_commit.is_some(), force, &target) {
        MergePlan::Refuse(m) | MergePlan::RefuseHard(m) => { eprintln!("merge: {m}"); return MergeOutcome::UsageOrPreflight; }
        MergePlan::Merge { target } => {
            // clone preflight (current_commit guaranteed Some here)
            let cur = ck.current_commit.as_deref().unwrap();
            match (current_branch(clone), head_sha(clone), is_worktree_dirty(clone)) {
                (Ok(b), _, _) if b != ck.branch => { eprintln!("merge: clone on wrong branch ({b} != {}) — inspect {}", ck.branch, clone.display()); return MergeOutcome::UsageOrPreflight; }
                (_, Ok(h), _) if h != cur => { eprintln!("merge: clone HEAD moved off the checkpoint — inspect {}", clone.display()); return MergeOutcome::UsageOrPreflight; }
                (_, _, Ok(true)) => { eprintln!("merge: clone worktree dirty — inspect {}", clone.display()); return MergeOutcome::UsageOrPreflight; }
                (Err(e), _, _) | (_, Err(e), _) | (_, _, Err(e)) => { eprintln!("merge: clone preflight: {e}"); return MergeOutcome::UsageOrPreflight; }
                _ => {}
            }
            if let Err(e) = check_clone_shape(clone, &ck.base_commit, cur) { eprintln!("merge: {e}"); return MergeOutcome::UsageOrPreflight; }
            let op = match operator_from(&src, mcfg.and_then(|m| m.author.as_ref())) {
                Ok(o) => o, Err(e) => { eprintln!("merge: {e}"); return MergeOutcome::UsageOrPreflight; }
            };
            // source no-touch preflight (best-effort; denyCurrentBranch is the atomic backstop)
            if !is_bare(&src).unwrap_or(false) && source_head(&src).ok().flatten().as_deref() == Some(target.as_str()) {
                eprintln!("merge: '{target}' is checked out in {} — switch off it or pick another target (clone kept)", src.display());
                return MergeOutcome::Unlanded;
            }
            let (msg, _) = commit_message(ck.original_message.clone(), &ck.task_brief);
            let rt = match reauthor_commit(clone, cur, &ck.base_commit, &msg, &op) {
                Ok(r) => r, Err(e) => { eprintln!("merge: {e}"); return MergeOutcome::Unlanded; }
            };
            match push_landing(clone, &src, &rt, &target, &ck.base_commit) {
                Ok(()) => {
                    if let Err(e) = reap_clone(clone, &src, root) { eprintln!("merge: landed but {e}"); }
                    println!("merged {:.12} into {target}", rt);
                    MergeOutcome::Merged
                }
                Err(PushError::StaleLease) => {
                    eprintln!("merge: '{target}' moved off {:.12} since the clone was made. The clone's base is fixed, so re-running can't land it — start a fresh `implement` run off the moved '{target}'. (clone kept at {})", ck.base_commit, clone.display());
                    MergeOutcome::Unlanded
                }
                Err(PushError::CheckedOutTarget) => { eprintln!("merge: '{target}' is checked out in {} — switch off it (clone kept)", src.display()); MergeOutcome::Unlanded }
                Err(PushError::Other(e)) => { eprintln!("merge: push failed: {e}; clone kept at {}", clone.display()); MergeOutcome::Unlanded }
            }
        }
    }
}
```

- [ ] **Step 6: Implement `merge_cmd` (arg parse + config load + dispatch)**

```rust
/// `a2a-bridge merge <id> [--config <path>] [--onto <branch>] [--force]`
pub async fn merge_cmd(args: &[String]) -> Result<(), crate::BoxError> {
    let mut id: Option<String> = None;
    let mut config_path = std::path::PathBuf::from(crate::CONFIG_PATH);
    let mut onto: Option<String> = None;
    let mut force = false;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--config" => { i += 1; config_path = args.get(i).ok_or("merge: --config needs a path")?.into(); }
            "--onto" => { i += 1; onto = Some(args.get(i).ok_or("merge: --onto needs a branch")?.clone()); }
            "--force" => force = true,
            s if !s.starts_with('-') && id.is_none() => id = Some(s.to_string()),
            s => return Err(format!("merge: unexpected arg {s:?}").into()),
        }
        i += 1;
    }
    let id = id.ok_or("merge: missing <id> (usage: a2a-bridge merge <id> [--onto <branch>] [--force])")?;
    let config_path = std::fs::canonicalize(&config_path).map_err(|e| format!("merge: config {}: {e}", config_path.display()))?;
    let raw = std::fs::read_to_string(&config_path).map_err(|e| format!("merge: read config: {e}"))?;
    let cfg = crate::config::RegistryConfig::parse(&raw).map_err(|e| format!("merge: config parse: {e}"))?;
    let root = cfg.allowed_cwd_root.clone().ok_or("merge: config needs allowed_cwd_root")?;
    let root = std::fs::canonicalize(&root).map_err(|e| format!("merge: allowed_cwd_root {root:?}: {e}"))?;
    let mcfg = cfg.merge_config().map_err(|e| format!("merge: {e}"))?;
    let clone = crate::implement_resume::resolve_clone(&root, &id).map_err(|e| format!("merge: {e}"))?;

    let outcome = merge_clone(mcfg.as_ref(), &clone, &root, onto.as_deref(), force);
    use std::io::Write;
    std::io::stdout().flush().ok();
    std::process::exit(outcome.code());
}
```

> Confirm `CONFIG_PATH`, `BoxError`, and `RegistryConfig::parse`/`allowed_cwd_root` are the exact names in `main.rs`/`config.rs`; adjust the `crate::` paths to match. If `BoxError` isn't `pub`, return `Result<(), Box<dyn std::error::Error>>`.

- [ ] **Step 7: Register the subcommand**

In `main.rs` dispatch (line ~2382), add:

```rust
        Some("merge") => return merge::merge_cmd(&raw_args[2..]).await,
```

Add `merge` to the unknown-subcommand error list (line ~2397) and to `TOP_USAGE` (line ~82): a `merge <id>` usage line like the others.

- [ ] **Step 8: Build + run the whole merge suite, fmt/clippy, commit**

Run: `cargo build -p a2a-bridge 2>&1 | tail -5 && cargo test -p a2a-bridge merge:: 2>&1 | tail -10`
Expected: build OK; all merge tests PASS.

```bash
cargo fmt --all && cargo clippy -p a2a-bridge --all-targets -- -D warnings
git add bin/a2a-bridge/src/merge.rs bin/a2a-bridge/src/main.rs
git commit -m "feat(merge): merge_clone orchestrator + merge <id> CLI (slice 6)"
```

---

### Task 7: `run_warm_loop` typed outcome + `implement --merge` sugar

Make `run_warm_loop` return its terminal phase so `implement --merge` can land an `Approved` run on the just-finished clone. Plain `implement` exit behavior is unchanged.

**Files:**
- Modify: `bin/a2a-bridge/src/main.rs` (`run_warm_loop` return type at 943/1021; the two callers ~1304 and ~1473; `implement_cmd` arg parsing; the resume arg parsing; `TOP_USAGE`)
- Test: `bin/a2a-bridge/src/merge.rs` — an `exit-code mapping` unit test on `MergeOutcome` (the end-to-end `--merge` path is covered by the live gate)

- [ ] **Step 1: Change `run_warm_loop` to return the terminal phase**

At `run_warm_loop` (line 943) change the signature return from `)` to `) -> crate::implement_resume::ImplementPhase {`. At the end (line 1014–1021) return the computed `terminal` instead of discarding it:

```rust
    let terminal = if final_.report.stop_reason == tweak::StopReason::Success {
        implement_resume::ImplementPhase::Approved
    } else {
        implement_resume::ImplementPhase::LoopStopped
    };
    implement_resume::write_terminal(clone, prod_ckpt.ck.clone(), terminal);
    let _ = runner.retire().await;
    terminal
}
```

- [ ] **Step 2: Capture the outcome in both callers**

At the fresh caller (~1304) and the resume caller (~1473), bind the result: `let outcome_phase = run_warm_loop(...).await;` (both sites). Build to confirm nothing else breaks:

Run: `cargo build -p a2a-bridge 2>&1 | tail -5`
Expected: OK (a now-used return value; if a caller is in a non-returning position, just `let _ = ` until Step 4 wires it).

- [ ] **Step 3: Parse `--merge`/`--onto` in `implement_cmd` (fresh + resume)**

In the fresh `implement_cmd` arg loop and the resume arg loop, add two flags: `--merge` (bool) and (if not already present for implement) `--onto <branch>`. Reject `--merge` combined with `--force` for the sugar. Thread them to where the warm loop returns.

- [ ] **Step 4: After the loop, run the merge on Approved**

Right after each `run_warm_loop` call site, add (using the `clone`, `root`, and the loaded `cfg` already in scope at that site):

```rust
    if merge_requested {
        match outcome_phase {
            implement_resume::ImplementPhase::Approved => {
                let mcfg = cfg.merge_config().map_err(|e| format!("implement --merge: {e}"))?;
                let outcome = merge::merge_clone(mcfg.as_ref(), &clone, &root, onto.as_deref(), false);
                use std::io::Write; std::io::stdout().flush().ok();
                std::process::exit(outcome.code());
            }
            _ => {
                eprintln!("not merged: run ended {:?}, not Approved — resume/re-run the agent", outcome_phase);
                std::process::exit(2);
            }
        }
    }
```

> `clone`, `root`, and `cfg` are already bound at both call sites (see `implement_cmd` 1046–1056 and the resume block 1334–1350). Use those exact bindings; do not reload.

- [ ] **Step 5: Add the exit-code unit test**

In `merge.rs` tests:

```rust
#[test]
fn merge_outcome_exit_codes() {
    assert_eq!(MergeOutcome::Merged.code(), 0);
    assert_eq!(MergeOutcome::UsageOrPreflight.code(), 1);
    assert_eq!(MergeOutcome::Unlanded.code(), 3);
}
```

- [ ] **Step 6: Update `TOP_USAGE` / `implement` help**

Add `[--merge [--onto <branch>]]` to the `implement` usage in `TOP_USAGE` (and the `implement --help` text if separate), noting `--merge` is Approved-only mode-A and rejects `--force`.

- [ ] **Step 7: Build, full test, fmt/clippy, commit**

Run: `cargo build -p a2a-bridge 2>&1 | tail -5 && cargo test -p a2a-bridge merge:: 2>&1 | tail -6`
Expected: build OK; tests PASS.

```bash
cargo fmt --all && cargo clippy -p a2a-bridge --all-targets -- -D warnings
git add bin/a2a-bridge/src/main.rs bin/a2a-bridge/src/merge.rs
git commit -m "feat(merge): run_warm_loop typed outcome + implement --merge sugar (slice 7)"
```

---

### Task 8: Docs + ADR-0027

**Files:**
- Create: `docs/adr/0027-merge-handoff.md`
- Modify: `AGENTS.md` and/or `docs/containerized-agents.md` (a `merge <id>` usage line + the `receive.denyCurrentBranch=updateInstead` out-of-scope note)

- [ ] **Step 1: Write ADR-0027**

Capture: the decision (push-`commit-tree` + `--force-with-lease`, Mode A only, Mode B deferred), the alternatives (worktree+cherry-pick+CAS-ref+lock — rejected), the no-touch guard's two layers (best-effort preflight + `denyCurrentBranch=refuse` atomic; `updateInstead` out of scope), and the 4-round review provenance. Reference the spec.

- [ ] **Step 2: Add the operator usage note**

Document `a2a-bridge merge <id> [--onto <branch>] [--force]` and `implement --merge`, the exit-code table, and the `denyCurrentBranch` caveat.

- [ ] **Step 3: Commit (ADR carries the Co-Authored-By trailer per repo convention)**

```bash
git add docs/adr/0027-merge-handoff.md AGENTS.md docs/containerized-agents.md
git commit -m "docs: ADR-0027 merge hand-off (Mode A) + operator usage"
```

---

## Live gate (operator-run, after Task 7)

Not a unit test — run against a real `Approved` clone (peers IDLE to avoid the dogfood OOM; ensure the gate repo has a committed `Cargo.lock` if verify runs). Verify:

1. A real `Approved` run → `a2a-bridge merge <id> --onto <branch>` lands the re-authored commit on `<branch>` (author == committer == operator), clone reaped, **exit 0**.
2. The `source_repo`'s `git rev-parse HEAD` + `git status --porcelain` are byte-identical before/after (only `refs/heads/<branch>` moved).
3. A `LoopStopped` run → `merge <id>` refuses without `--force` (**exit 1**); with `--force` it lands.
4. `merge <id> --onto <the source's checked-out branch>` refuses cleanly (`CheckedOutTarget`, **exit 3**), checkout untouched.
5. A target moved off `base_commit` → `StaleLease` recovery line, clone kept, **exit 3**.
6. `a2a-bridge implement <task> --repo <path> --merge --onto <branch>` lands on `Approved`; a non-Approved run prints `not merged:` and **exit 2**.

---

## Self-Review (run after writing — checklist, not a dispatch)

**Spec coverage:** Goal→Tasks 6/7; push-`commit-tree`→T3/T4; gate→T2; source guard→T6; components→all; pure core→T2; impure ops→T3/T4/T6; config→T5; command surface + exit codes→T6/T7; control flow→T6; testing→every task's tests + live gate; build order→T1–T7; risks→ADR/docs (T8); deferred Mode B→ADR note (T8). No spec section is unimplemented.

**Placeholder scan:** every code step shows complete code; commands have expected output; no "handle errors"/"TBD".

**Type consistency:** `OperatorIdent` (T2) used by `reauthor_commit`/`operator_from` (T3/T6) and `MergeConfig` (T5); `MergePlan`/`decide_merge`/`resolve_target` (T2) consumed by `merge_clone` (T6); `PushError`/`push_landing` (T4) matched in `merge_clone` (T6); `MergeOutcome.code()` (T6) used by `merge_cmd`/`implement --merge` (T6/T7); `run_warm_loop` return type (T7) consumed at both callers. Names are consistent across tasks.
