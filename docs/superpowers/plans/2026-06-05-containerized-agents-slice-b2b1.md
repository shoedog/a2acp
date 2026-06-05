# Containerized Agents — Slice B2b-1 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add an `a2a-bridge implement <task> --repo <path>` subcommand that clones a repo into a quarantine, has the B2a `ContainerRw` `impl` agent edit + stage a change, the bridge deterministically commits the agent-staged index on a task branch, and the clone is left for the operator to review/merge/reap.

**Architecture:** Pure git-argv builders + the message-file reader + task-id + hand-off text live in a new `bin/a2a-bridge/src/implement.rs` (git-free unit tests); the impure git ops (stage classifier, HEAD guard, host-commit round-trip) get temp-repo git tests; `implement_cmd` in `main.rs` orchestrates clone → run the 1-node `implement-edit` workflow via `executor.run_with_context(session_cwd=clone)` → the commit state machine → hand-off. No new crate/backend/image.

**Tech Stack:** Rust (`std::process::Command` for host git — direct argv, no shell), `bridge-workflow::executor`, the shipped `ContainerRw` backend, Docker Desktop / **podman (preferred)**.

**Branch:** `feat/implement-b2b1` (spec `ea1d9d3`). **Commits:** task/code commits do NOT carry the `Co-Authored-By` trailer (this plan doc does). **Coverage:** after `cargo llvm-cov clean --workspace` — floors workspace 85, bridge-core 90, bridge-workflow 90.

**Scope:** B2b-1 only — clone + edit + host-commit + hand-off. NO build/test verify (B2b-2), NO review-the-diff/approval (B2b-3), NO `add -A` (agent owns staging). Rootful-Docker-on-Linux out of scope.

---

## File Structure

| File | Responsibility | Action |
|---|---|---|
| `bin/a2a-bridge/src/implement.rs` | pure helpers (git argv builders, msg-file reader+fallback, task-id, hand-off text) + impure git ops (`run_git`, stage classifier, HEAD/branch reads, host-commit) | Create |
| `bin/a2a-bridge/src/main.rs` | `mod implement;`, `Some("implement") => implement_cmd(…)`, `implement_cmd`, extract `make_spawn_fn` | Modify |
| `examples/a2a-bridge.containerized.toml` | the `implement-edit` 1-node workflow | Modify |
| `prompts/implement-edit.md` | the edit + stage + write-`.git/A2A_COMMIT_MSG` contract | Create |

**Convention:** pure helpers return `Vec<String>` argv (after `git` or after `git -C <clone>`); a `run_git(cwd: Option<&Path>, argv: &[&str])` runner prepends `git`/`git -C <cwd>`. All argv — no shell.

---

# Slice 1 — Pure helpers in `implement.rs` (git-free, Docker-free)

### Task 1: crate module + git argv builders

**Files:** Create `bin/a2a-bridge/src/implement.rs`; Modify `bin/a2a-bridge/src/main.rs` (add `mod implement;` near `mod config;`).

- [ ] **Step 1: Add the module declaration** in `main.rs` (next to `mod config;` at the top):

```rust
mod config;
mod implement;
mod route;
```

- [ ] **Step 2: Write the failing tests** (create `implement.rs` with a `#[cfg(test)]` block):

```rust
//! `a2a-bridge implement` — clone a repo into a quarantine, have the ContainerRw `impl` agent edit+stage
//! a change, host-commit the agent-staged index on a task branch, and leave the clone for the operator.

/// The bot identity the bridge commits under (rewritable pre-merge; operator re-authors at merge).
pub const BOT_NAME: &str = "a2a-implement";
pub const BOT_EMAIL: &str = "implement@a2a-bridge.local";

/// `git clone --no-hardlinks <repo> <dest>` (committed-only quarantine; independent object store).
pub fn clone_argv(repo: &str, dest: &str) -> Vec<String> {
    vec!["clone".into(), "--no-hardlinks".into(), repo.into(), dest.into()]
}

/// `checkout -b <branch>` (run with `git -C <clone>`).
pub fn checkout_new_branch_argv(branch: &str) -> Vec<String> {
    vec!["checkout".into(), "-b".into(), branch.into()]
}

/// The host commit argv (run with `git -C <clone>`). Hooks neutralized THREE ways (`--no-verify` alone
/// still runs prepare-commit-msg/post-commit and the agent can set core.hooksPath); signing off;
/// safe.directory pins the dubious-ownership guard for the container-root→host round-trip; bot identity.
pub fn commit_argv(clone: &str, msg: &str) -> Vec<String> {
    vec![
        "-c".into(), format!("safe.directory={clone}"),
        "-c".into(), "core.hooksPath=/dev/null".into(),
        "-c".into(), "commit.gpgsign=false".into(),
        "-c".into(), format!("user.name={BOT_NAME}"),
        "-c".into(), format!("user.email={BOT_EMAIL}"),
        "commit".into(), "--no-verify".into(), "-m".into(), msg.into(),
    ]
}
```

Append to `implement.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clone_argv_no_hardlinks() {
        assert_eq!(clone_argv("/src/repo", "/root/.a2a-implement/impl-1-ab"),
            vec!["clone", "--no-hardlinks", "/src/repo", "/root/.a2a-implement/impl-1-ab"]);
    }

    #[test]
    fn commit_argv_pins_hooks_signing_safedir_identity() {
        let a = commit_argv("/root/.a2a-implement/impl-1-ab", "subject");
        // -c pins present, BEFORE the `commit` subcommand
        let joined = a.join(" ");
        assert!(joined.contains("-c safe.directory=/root/.a2a-implement/impl-1-ab"));
        assert!(joined.contains("-c core.hooksPath=/dev/null"));
        assert!(joined.contains("-c commit.gpgsign=false"));
        assert!(joined.contains("-c user.name=a2a-implement"));
        assert!(joined.contains("-c user.email=implement@a2a-bridge.local"));
        let ci = a.iter().position(|x| x == "commit").unwrap();
        assert!(a.iter().take(ci).filter(|x| *x == "-c").count() == 5, "all -c before commit");
        assert_eq!(&a[ci..], &["commit", "--no-verify", "-m", "subject"]);
    }
}
```

- [ ] **Step 3: Run — verify fail.** Run: `cargo test -p a2a-bridge implement:: 2>&1 | tail -10`. Expected: PASS already? No — the impl is in Step 2's first block. If you wrote tests-first, the fns don't exist → compile error. Write the fns (Step 2 first block), then:
- [ ] **Step 4: Run — verify pass.** Run: `cargo test -p a2a-bridge implement::tests::commit_argv 2>&1 | tail -8`. Expected: PASS.
- [ ] **Step 5: Commit.** `git add bin/a2a-bridge/src/implement.rs bin/a2a-bridge/src/main.rs && git commit -m "feat(b2b1): implement.rs git argv builders (clone/checkout/commit pins)"`

---

### Task 2: the `.git/A2A_COMMIT_MSG` reader + task-derived fallback

**Files:** Modify `implement.rs`.

- [ ] **Step 1: Write the failing tests:**

```rust
    #[test]
    fn commit_message_reads_file_else_task_fallback() {
        // present + valid -> used as-is (trimmed)
        assert_eq!(commit_message(Some("  Fix the widget\n\ndetails\n".into()), "task ignored"),
            ("Fix the widget\n\ndetails".to_string(), false));
        // absent -> task-derived fallback
        assert_eq!(commit_message(None, "Add a FOO marker file to the repo root\nmore"),
            ("implement: Add a FOO marker file to the repo root".to_string(), true));
        // empty / whitespace-only -> fallback
        assert_eq!(commit_message(Some("   \n  ".into()), "Tidy up").1, true);
        // oversized subject is truncated in the fallback
        let long = "x".repeat(500);
        let (m, fb) = commit_message(None, &long);
        assert!(fb && m.starts_with("implement: ") && m.len() <= "implement: ".len() + 120);
    }
```

- [ ] **Step 2: Run — verify fail.** Run: `cargo test -p a2a-bridge commit_message 2>&1 | tail`. Expected: `cannot find function commit_message`.

- [ ] **Step 3: Implement** (append to `implement.rs`):

```rust
/// Resolve the commit message: the agent-written `.git/A2A_COMMIT_MSG` content if non-blank, else a
/// deterministic task-derived fallback `implement: <first line of task, truncated>`. Returns
/// (message, used_fallback). `raw` is the file content (None if absent/unreadable/invalid-UTF-8).
pub fn commit_message(raw: Option<String>, task: &str) -> (String, bool) {
    if let Some(s) = raw {
        let trimmed = s.trim();
        if !trimmed.is_empty() {
            return (trimmed.to_string(), false);
        }
    }
    let first = task.lines().next().unwrap_or("").trim();
    let mut subj = first.chars().take(120).collect::<String>();
    if subj.is_empty() { subj = "changes".into(); }
    (format!("implement: {subj}"), true)
}

/// Read `<clone>/.git/A2A_COMMIT_MSG`, capping the read so an oversized/binary file can't blow memory;
/// returns None on absent / unreadable / invalid-UTF-8 (caller falls back).
pub fn read_commit_msg_file(clone: &std::path::Path) -> Option<String> {
    let p = clone.join(".git").join("A2A_COMMIT_MSG");
    let bytes = std::fs::read(&p).ok()?;
    if bytes.len() > 64 * 1024 { return None; }
    String::from_utf8(bytes).ok()
}
```

- [ ] **Step 4: Run — verify pass.** Run: `cargo test -p a2a-bridge commit_message 2>&1 | tail`. Expected: PASS.
- [ ] **Step 5: Commit.** `git commit -am "feat(b2b1): commit-message file reader + task-derived fallback"`

---

### Task 3: task-id + branch name

**Files:** Modify `implement.rs`.

- [ ] **Step 1: Write the failing tests:**

```rust
    #[test]
    fn task_id_and_branch_shape() {
        let id = task_id(4242, "k3x9");
        assert_eq!(id, "impl-4242-k3x9");
        assert_eq!(branch_for(&id), "implement/impl-4242-k3x9");
        // nonce charset is filesystem- + branch-name-safe
        assert!(nonce(12).chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit()));
        assert_eq!(nonce(8).len(), 8);
    }
```

- [ ] **Step 2: Run — verify fail.** Run: `cargo test -p a2a-bridge task_id_and_branch 2>&1 | tail`.

- [ ] **Step 3: Implement** (append to `implement.rs`):

```rust
/// `impl-<pid>-<nonce>` — filesystem- and branch-name-safe (lowercase-alnum + hyphens).
pub fn task_id(pid: u32, nonce: &str) -> String {
    format!("impl-{pid}-{nonce}")
}
pub fn branch_for(task_id: &str) -> String {
    format!("implement/{task_id}")
}
/// A lowercase-alnum nonce of length `n` (collision-retried by the caller against existing clone dirs).
pub fn nonce(n: usize) -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    // Seed from the clock + pid; lowercase-alnum. Deterministic alphabet, not crypto — the caller retries
    // on dir/branch collision, so uniqueness is belt-and-suspenders.
    let seed = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0)
        ^ (std::process::id() as u128);
    const A: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789";
    let mut s = String::new();
    let mut x = seed;
    for _ in 0..n {
        s.push(A[(x % A.len() as u128) as usize] as char);
        x /= A.len() as u128;
        if x == 0 { x = seed.rotate_left(7) | 1; }
    }
    s
}
```

> `nonce` is non-deterministic by clock; the test only asserts the alphabet/length (not the value).

- [ ] **Step 4: Run — verify pass.** `cargo test -p a2a-bridge task_id_and_branch 2>&1 | tail`. Expected: PASS.
- [ ] **Step 5: Commit.** `git commit -am "feat(b2b1): task-id + branch-name + nonce"`

---

### Task 4: hand-off text (the CORRECTED re-author command)

**Files:** Modify `implement.rs`.

- [ ] **Step 1: Write the failing test:**

```rust
    #[test]
    fn handoff_text_has_corrected_reauthor() {
        let t = handoff_text("/root/.a2a-implement/impl-1-ab", "implement/impl-1-ab", "abc1234", "Fix widget", "/src/repo");
        assert!(t.contains("/root/.a2a-implement/impl-1-ab"));
        assert!(t.contains("implement/impl-1-ab"));
        assert!(t.contains("abc1234") && t.contains("Fix widget"));
        // CORRECTED: cherry-pick -n then commit -C --reset-author (cherry-pick has no --reset-author)
        assert!(t.contains("cherry-pick -n FETCH_HEAD"));
        assert!(t.contains("commit -C FETCH_HEAD --reset-author"));
        assert!(!t.contains("cherry-pick --reset-author"));
        assert!(t.contains("rm -rf /root/.a2a-implement/impl-1-ab"));
    }
```

- [ ] **Step 2: Run — verify fail.** `cargo test -p a2a-bridge handoff_text 2>&1 | tail`.

- [ ] **Step 3: Implement** (append to `implement.rs`):

```rust
/// The operator hand-off (informational): merge the bot-authored quarantine branch into <repo> RE-AUTHORED
/// as the operator, then reap the clone. The target repo should be clean; conflicts are operator-handled.
pub fn handoff_text(clone: &str, branch: &str, sha: &str, subject: &str, repo: &str) -> String {
    format!(
        "implement: committed {sha} \"{subject}\" on {branch}\n\
         clone: {clone}\n\
         To merge as YOURSELF (bot identity is pre-merge only) and reap the clone:\n\
         \x20 git -C {repo} fetch {clone} {branch}\n\
         \x20 git -C {repo} cherry-pick -n FETCH_HEAD && git -C {repo} commit -C FETCH_HEAD --reset-author\n\
         \x20 rm -rf {clone}\n"
    )
}
```

- [ ] **Step 4: Run — verify pass.** `cargo test -p a2a-bridge handoff_text 2>&1 | tail`. Expected: PASS.
- [ ] **Step 5: Commit.** `git commit -am "feat(b2b1): operator hand-off text (corrected re-author command)"`

---

# Slice 2 — Impure git ops in `implement.rs` (temp-repo git tests)

### Task 5: `run_git` runner + stage classifier (`status --porcelain`)

**Files:** Modify `implement.rs`.

- [ ] **Step 1: Write the failing test** (a temp-repo helper builds a real git repo; tests the classifier):

```rust
    use std::path::Path;
    use std::process::Command;

    /// Build a throwaway git repo in a tempdir with one committed file; return its path + the tempdir guard.
    fn temp_repo() -> (tempfile::TempDir, std::path::PathBuf) {
        let td = tempfile::tempdir().unwrap();
        let p = td.path().to_path_buf();
        for argv in [
            vec!["init", "-q", "-b", "main"],
            vec!["config", "user.name", "t"],
            vec!["config", "user.email", "t@t"],
        ] { assert!(Command::new("git").arg("-C").arg(&p).args(argv).status().unwrap().success()); }
        std::fs::write(p.join("README.md"), "hi\n").unwrap();
        assert!(Command::new("git").arg("-C").arg(&p).args(["add","README.md"]).status().unwrap().success());
        assert!(Command::new("git").arg("-C").arg(&p).args(["commit","-q","-m","init"]).status().unwrap().success());
        (td, p)
    }

    #[test]
    fn stage_state_classifies_staged_dirty_clean() {
        let (_g, p) = temp_repo();
        assert_eq!(stage_state(&p).unwrap(), StageState::Clean);
        // untracked new file, NOT staged -> DirtyUnstaged (git diff --quiet would MISS this)
        std::fs::write(p.join("FOO.md"), "bar\n").unwrap();
        assert_eq!(stage_state(&p).unwrap(), StageState::DirtyUnstaged);
        // stage it -> Staged
        assert!(Command::new("git").arg("-C").arg(&p).args(["add","FOO.md"]).status().unwrap().success());
        assert_eq!(stage_state(&p).unwrap(), StageState::Staged);
    }
```

- [ ] **Step 2: Run — verify fail.** `cargo test -p a2a-bridge stage_state 2>&1 | tail`. Expected: `cannot find … stage_state`.

- [ ] **Step 3: Implement** (append to `implement.rs`):

```rust
use std::path::Path;
use std::process::Command;

#[derive(Debug, PartialEq, Eq)]
pub enum StageState { Staged, DirtyUnstaged, Clean }

/// Run `git [-C cwd] <argv>` and capture output. Direct argv — no shell.
pub fn run_git(cwd: Option<&Path>, argv: &[&str]) -> std::io::Result<std::process::Output> {
    let mut c = Command::new("git");
    if let Some(d) = cwd { c.arg("-C").arg(d); }
    c.args(argv).output()
}

/// Classify the working tree via `git status --porcelain` (detects UNTRACKED files, unlike
/// `git diff --quiet`). Staged = any line whose index column (X) is not ' ' and not '?'.
pub fn stage_state(clone: &Path) -> std::io::Result<StageState> {
    let out = run_git(Some(clone), &["status", "--porcelain"])?;
    let text = String::from_utf8_lossy(&out.stdout);
    let mut any = false;
    let mut staged = false;
    for line in text.lines() {
        if line.len() < 2 { continue; }
        any = true;
        let x = line.as_bytes()[0] as char;
        if x != ' ' && x != '?' { staged = true; }
    }
    Ok(if staged { StageState::Staged } else if any { StageState::DirtyUnstaged } else { StageState::Clean })
}
```

- [ ] **Step 4: Run — verify pass.** `cargo test -p a2a-bridge stage_state 2>&1 | tail`. Expected: PASS.
- [ ] **Step 5: Commit.** `git commit -am "feat(b2b1): run_git + stage classifier (status --porcelain, detects untracked)"`

---

### Task 6: HEAD/branch reads + the HEAD guard

**Files:** Modify `implement.rs`.

- [ ] **Step 1: Write the failing test:**

```rust
    #[test]
    fn head_guard_detects_branch_switch_and_advance() {
        let (_g, p) = temp_repo();
        assert!(Command::new("git").arg("-C").arg(&p).args(["checkout","-q","-b","implement/x"]).status().unwrap().success());
        let pre = head_sha(&p).unwrap();
        // on-branch + no advance -> Ok
        assert!(head_guard(&p, "implement/x", &pre).is_ok());
        // agent advanced HEAD (committed) -> Err mentions advanced
        std::fs::write(p.join("A.md"), "a\n").unwrap();
        assert!(Command::new("git").arg("-C").arg(&p).args(["add","A.md"]).status().unwrap().success());
        assert!(Command::new("git").arg("-C").arg(&p).args(["commit","-q","-m","agent"]).status().unwrap().success());
        let e = head_guard(&p, "implement/x", &pre).unwrap_err();
        assert!(e.contains("advanced"), "got {e}");
        // agent switched branch -> Err mentions branch
        assert!(Command::new("git").arg("-C").arg(&p).args(["checkout","-q","main"]).status().unwrap().success());
        let e2 = head_guard(&p, "implement/x", &pre).unwrap_err();
        assert!(e2.contains("branch"), "got {e2}");
    }
```

- [ ] **Step 2: Run — verify fail.** `cargo test -p a2a-bridge head_guard 2>&1 | tail`.

- [ ] **Step 3: Implement** (append to `implement.rs`):

```rust
pub fn head_sha(clone: &Path) -> std::io::Result<String> {
    let o = run_git(Some(clone), &["rev-parse", "HEAD"])?;
    Ok(String::from_utf8_lossy(&o.stdout).trim().to_string())
}
pub fn current_branch(clone: &Path) -> std::io::Result<String> {
    let o = run_git(Some(clone), &["symbolic-ref", "--short", "HEAD"])?;
    Ok(String::from_utf8_lossy(&o.stdout).trim().to_string())
}
/// The agent has :rw + git and could switch branches or commit despite the contract. Assert HEAD is still
/// `expect_branch` and hasn't advanced past `pre_sha`. Returns a human error string for the subcommand.
pub fn head_guard(clone: &Path, expect_branch: &str, pre_sha: &str) -> Result<(), String> {
    let br = current_branch(clone).map_err(|e| format!("read branch: {e}"))?;
    if br != expect_branch {
        return Err(format!("agent switched branch: HEAD is {br:?}, expected {expect_branch:?}"));
    }
    let sha = head_sha(clone).map_err(|e| format!("read HEAD: {e}"))?;
    if sha != pre_sha {
        return Err(format!("agent advanced HEAD (committed itself?) {pre_sha}..{sha} — leaving clone for the operator"));
    }
    Ok(())
}
```

- [ ] **Step 4: Run — verify pass.** `cargo test -p a2a-bridge head_guard 2>&1 | tail`. Expected: PASS.
- [ ] **Step 5: Commit.** `git commit -am "feat(b2b1): HEAD/branch reads + head_guard (branch-switch + advance)"`

---

### Task 7: the host-commit round-trip (pins applied; a planted hook can't fire)

**Files:** Modify `implement.rs`.

- [ ] **Step 1: Write the failing test:**

```rust
    #[test]
    fn host_commit_uses_pins_and_neutralizes_planted_hook() {
        let (_g, p) = temp_repo();
        assert!(Command::new("git").arg("-C").arg(&p).args(["checkout","-q","-b","implement/x"]).status().unwrap().success());
        // plant a hook that would FAIL the commit if it ran (proves --no-verify + core.hooksPath neutralize it)
        let hooks = p.join(".git").join("hooks"); std::fs::create_dir_all(&hooks).unwrap();
        let pc = hooks.join("pre-commit");
        std::fs::write(&pc, "#!/bin/sh\nexit 1\n").unwrap();
        #[cfg(unix)] { use std::os::unix::fs::PermissionsExt; std::fs::set_permissions(&pc, std::fs::Permissions::from_mode(0o755)).unwrap(); }
        // stage a change, host-commit
        std::fs::write(p.join("FOO.md"), "bar\n").unwrap();
        assert!(Command::new("git").arg("-C").arg(&p).args(["add","FOO.md"]).status().unwrap().success());
        let sha = host_commit(&p, "subject line").expect("commit despite the failing hook");
        assert!(!sha.is_empty());
        // committed under the BOT identity
        let an = Command::new("git").arg("-C").arg(&p).args(["log","-1","--format=%an <%ae>"]).output().unwrap();
        assert_eq!(String::from_utf8_lossy(&an.stdout).trim(), "a2a-implement <implement@a2a-bridge.local>");
        let subj = Command::new("git").arg("-C").arg(&p).args(["log","-1","--format=%s"]).output().unwrap();
        assert_eq!(String::from_utf8_lossy(&subj.stdout).trim(), "subject line");
    }
```

- [ ] **Step 2: Run — verify fail.** `cargo test -p a2a-bridge host_commit 2>&1 | tail`.

- [ ] **Step 3: Implement** (append to `implement.rs`):

```rust
/// Deterministically commit the AGENT-STAGED index with the bot identity + the full hook/sign/ownership
/// pins. Removes a stale `.git/index.lock` first (the per-turn container that held it is being reaped).
/// Returns the new commit sha. Does NOT stage anything (agent owns staging).
pub fn host_commit(clone: &Path, msg: &str) -> Result<String, String> {
    let _ = std::fs::remove_file(clone.join(".git").join("index.lock")); // best-effort stale-lock clear
    let argv = commit_argv(&clone.to_string_lossy(), msg);
    let refs: Vec<&str> = argv.iter().map(String::as_str).collect();
    let out = run_git(Some(clone), &refs).map_err(|e| format!("git commit: {e}"))?;
    if !out.status.success() {
        return Err(format!("git commit failed: {}", String::from_utf8_lossy(&out.stderr)));
    }
    head_sha(clone).map_err(|e| format!("read new HEAD: {e}"))
}
```

- [ ] **Step 4: Run — verify pass.** `cargo test -p a2a-bridge host_commit 2>&1 | tail`. Expected: PASS (the planted `pre-commit exit 1` does NOT fire).
- [ ] **Step 5: Coverage + commit.** `cargo llvm-cov clean --workspace && cargo llvm-cov -p a2a-bridge 2>&1 | tail -3`; then `git commit -am "feat(b2b1): host_commit round-trip (pins, bot identity, stale-lock clear, hook neutralized)"`

---

# Slice 3 — Clone helpers + the `implement` subcommand

### Task 8: clone-dest guard + clone/checkout orchestration helpers

**Files:** Modify `implement.rs`.

- [ ] **Step 1: Write the failing test:**

```rust
    #[test]
    fn clone_dest_must_not_be_inside_a_worktree() {
        let (_g, repo) = temp_repo();
        // a path INSIDE the repo's worktree is rejected
        let inside = repo.join(".a2a-implement");
        assert!(assert_dest_outside_worktree(&inside).is_err());
        // a fresh tempdir (no enclosing repo) is OK
        let td = tempfile::tempdir().unwrap();
        assert!(assert_dest_outside_worktree(&td.path().join(".a2a-implement")).is_ok());
    }

    #[test]
    fn clone_then_branch_creates_quarantine() {
        let (_g, repo) = temp_repo();
        let dst = tempfile::tempdir().unwrap();
        let clone = dst.path().join("impl-1-ab");
        do_clone(&repo.to_string_lossy(), &clone.to_string_lossy()).unwrap();
        do_checkout_branch(&clone, "implement/impl-1-ab").unwrap();
        assert_eq!(current_branch(&clone).unwrap(), "implement/impl-1-ab");
        // the clone is independent (--no-hardlinks): committing in it doesn't touch the source
        let before = head_sha(&repo).unwrap();
        std::fs::write(clone.join("X.md"), "x\n").unwrap();
        run_git(Some(&clone), &["add","X.md"]).unwrap();
        host_commit(&clone, "c").unwrap();
        assert_eq!(head_sha(&repo).unwrap(), before, "source repo untouched");
    }
```

- [ ] **Step 2: Run — verify fail.** `cargo test -p a2a-bridge clone_ 2>&1 | tail`.

- [ ] **Step 3: Implement** (append to `implement.rs`):

```rust
/// Refuse a clone dest whose PARENT is inside a git worktree (cloning into a repo dirties it). Checks the
/// nearest existing ancestor (the dest itself doesn't exist yet).
pub fn assert_dest_outside_worktree(dest: &Path) -> Result<(), String> {
    let probe = dest.parent().unwrap_or(dest);
    let out = run_git(Some(probe), &["rev-parse", "--is-inside-work-tree"]).map_err(|e| e.to_string())?;
    if out.status.success() && String::from_utf8_lossy(&out.stdout).trim() == "true" {
        return Err(format!("clone dest {dest:?} is inside a git worktree — refusing (would dirty that repo)"));
    }
    Ok(())
}
pub fn do_clone(repo: &str, dest: &str) -> Result<(), String> {
    let argv = clone_argv(repo, dest);
    let refs: Vec<&str> = argv.iter().map(String::as_str).collect();
    let out = run_git(None, &refs).map_err(|e| e.to_string())?;
    if !out.status.success() { return Err(format!("git clone failed: {}", String::from_utf8_lossy(&out.stderr))); }
    Ok(())
}
pub fn do_checkout_branch(clone: &Path, branch: &str) -> Result<(), String> {
    let argv = checkout_new_branch_argv(branch);
    let refs: Vec<&str> = argv.iter().map(String::as_str).collect();
    let out = run_git(Some(clone), &refs).map_err(|e| e.to_string())?;
    if !out.status.success() { return Err(format!("git checkout -b failed: {}", String::from_utf8_lossy(&out.stderr))); }
    Ok(())
}
```

- [ ] **Step 4: Run — verify pass.** `cargo test -p a2a-bridge clone_ 2>&1 | tail`. Expected: PASS (incl. source-untouched).
- [ ] **Step 5: Commit.** `git commit -am "feat(b2b1): clone-dest guard + do_clone/do_checkout_branch helpers"`

---

### Task 9: extract `make_spawn_fn` (run-workflow stays green)

So `implement_cmd` builds the same registry as `run_workflow_cmd` without duplicating the ~50-line SpawnFn.

**Files:** Modify `bin/a2a-bridge/src/main.rs`.

- [ ] **Step 1: Lift the run-workflow `SpawnFn` closure** (currently at `run_workflow_cmd`, ~main.rs:177–228, the `let spawn = Arc::new(move |entry| { … })`) into a free fn:

```rust
/// The production `SpawnFn` (Acp compose-or-raw, Api, ContainerRw arms) used by run-workflow AND implement.
fn make_spawn_fn(
    policy_for_spawn: Arc<dyn bridge_core::ports::PolicyEngine>,
    owner_config_path: std::path::PathBuf,
) -> bridge_registry::registry::SpawnFn {
    Arc::new(move |entry: Arc<AgentEntry>| {
        let policy = Arc::clone(&policy_for_spawn);
        let owner_config_path = owner_config_path.clone();
        Box::pin(async move {
            // … MOVE the existing closure body here verbatim (cwd resolution + the Acp/Api/ContainerRw match) …
        })
    })
}
```

Replace the inline `let spawn = …` in `run_workflow_cmd` with `let spawn = make_spawn_fn(policy_for_spawn, config_path.clone());`. (Leave the `serve` closure as-is — out of scope.)

- [ ] **Step 2: Run — verify run-workflow still green.** Run: `cargo build -p a2a-bridge && cargo test -p a2a-bridge 2>&1 | tail -6`. Expected: PASS (behavior preserved).
- [ ] **Step 3: Clippy.** `cargo clippy -p a2a-bridge --all-targets -- -D warnings 2>&1 | tail -3`. Expected: clean.
- [ ] **Step 4: Commit.** `git commit -am "refactor(b2b1): extract make_spawn_fn (shared by run-workflow + implement)"`

---

### Task 10: the `implement` subcommand (arg parse + orchestration)

**Files:** Modify `bin/a2a-bridge/src/main.rs` (dispatch + `implement_cmd`); the orchestration is integration-validated by Task 12's live gate, but the **arg parser is unit-tested**.

- [ ] **Step 1: Write the failing arg-parser test** (in `main.rs` `#[cfg(test)] mod tests`):

```rust
    #[test]
    fn parse_implement_args_basic() {
        let a: Vec<String> = ["Add a FOO file","--repo","/src/repo","--base-ref","main","--config","c.toml"]
            .iter().map(|s| s.to_string()).collect();
        let p = super::parse_implement_args(&a).unwrap();
        assert_eq!(p.task, "Add a FOO file");
        assert_eq!(p.repo, std::path::PathBuf::from("/src/repo"));
        assert_eq!(p.base_ref.as_deref(), Some("main"));
        assert_eq!(p.workflow, "implement-edit"); // default
    }
    #[test]
    fn parse_implement_args_requires_task_and_repo() {
        assert!(super::parse_implement_args(&["--repo".into(),"/r".into()]).is_err()); // missing task
        assert!(super::parse_implement_args(&["task".into()]).is_err());                // missing --repo
    }
```

- [ ] **Step 2: Run — verify fail.** `cargo test -p a2a-bridge parse_implement_args 2>&1 | tail`.

- [ ] **Step 3: Implement the parser + dispatch + `implement_cmd`.** Add the dispatch arm (next to `Some("run-workflow")`, ~main.rs:878):

```rust
        Some("implement") => return implement_cmd(&raw_args[2..]).await,
```

Add the parser + struct:

```rust
struct ImplementArgs { task: String, repo: PathBuf, base_ref: Option<String>, config: PathBuf, workflow: String }

fn parse_implement_args(args: &[String]) -> Result<ImplementArgs, BoxError> {
    let mut iter = args.iter().peekable();
    let task = iter.next().cloned().ok_or("implement: missing <task>")?;
    let (mut repo, mut base_ref, mut config, mut workflow) = (None, None, None, None);
    while let Some(f) = iter.next() {
        match f.as_str() {
            "--repo" => repo = Some(PathBuf::from(iter.next().ok_or("implement: --repo needs a value")?)),
            "--base-ref" => base_ref = Some(iter.next().ok_or("implement: --base-ref needs a value")?.clone()),
            "--config" => config = Some(PathBuf::from(iter.next().ok_or("implement: --config needs a value")?)),
            "--workflow" => workflow = Some(iter.next().ok_or("implement: --workflow needs a value")?.clone()),
            other => return Err(format!("implement: unknown flag {other:?}").into()),
        }
    }
    Ok(ImplementArgs {
        task,
        repo: repo.ok_or("implement: --repo <path> is required")?,
        base_ref,
        config: config.unwrap_or_else(|| PathBuf::from(CONFIG_PATH)),
        workflow: workflow.unwrap_or_else(|| "implement-edit".into()),
    })
}
```

Implement `implement_cmd` (orchestration — uses the `implement::*` helpers + the executor, mirroring `run_workflow_cmd`'s registry/executor build via `make_spawn_fn`):

```rust
async fn implement_cmd(args: &[String]) -> Result<(), BoxError> {
    bridge_observ::init();
    let a = parse_implement_args(args)?;

    // 1. config + allowed_cwd_root (canonicalized, consistent with the ContainerRw rw-target gate).
    let raw = std::fs::read_to_string(&a.config).map_err(|e| format!("implement: read config {:?}: {e}", a.config))?;
    let cfg = config::RegistryConfig::parse(&raw).map_err(|e| format!("implement: config parse: {e}"))?;
    let root = cfg.allowed_cwd_root.clone().ok_or("implement: config needs allowed_cwd_root (the mount anchor)")?;
    let root = std::fs::canonicalize(&root).map_err(|e| format!("implement: allowed_cwd_root {root:?}: {e}"))?;

    // 2. task-id (collision-retry), clone dest, clone-dest guard.
    let (task_id, clone) = {
        let mut chosen = None;
        for _ in 0..8 {
            let id = implement::task_id(std::process::id(), &implement::nonce(8));
            let dir = root.join(".a2a-implement").join(&id);
            if !dir.exists() { chosen = Some((id, dir)); break; }
        }
        chosen.ok_or("implement: could not find a free task-id")?
    };
    std::fs::create_dir_all(root.join(".a2a-implement")).map_err(|e| format!("implement: mkdir .a2a-implement: {e}"))?;
    implement::assert_dest_outside_worktree(&clone)?;

    // 3. clone (committed-only) + base-ref + task branch.
    implement::do_clone(&a.repo.to_string_lossy(), &clone.to_string_lossy())?;
    if let Some(br) = &a.base_ref {
        let out = implement::run_git(Some(&clone), &["checkout", "-q", br])?;
        if !out.status.success() { return Err(format!("implement: base-ref {br:?}: {}", String::from_utf8_lossy(&out.stderr)).into()); }
    }
    let branch = implement::branch_for(&task_id);
    implement::do_checkout_branch(&clone, &branch)?;
    let pre = implement::head_sha(&clone)?;

    // 4. run the 1-node implement-edit workflow with session_cwd = the clone (B2a --session-cwd plumbing).
    let base = a.config.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or_else(|| std::path::Path::new(".")).to_path_buf();
    let wf_map = cfg.load_workflows(&base).map_err(|e| format!("implement: workflow load: {e}"))?;
    let wf_id = bridge_core::ids::WorkflowId::parse(a.workflow.clone()).map_err(|e| format!("implement: workflow id: {e:?}"))?;
    let graph = wf_map.get(&wf_id).cloned().ok_or_else(|| format!("implement: unknown workflow {:?}", a.workflow))?;
    let snapshot = cfg.into_snapshot().map_err(|e| format!("implement: snapshot: {e}"))?;
    let policy = Arc::new(bridge_policy::permission::AutoPolicy);
    let policy_for_spawn = Arc::clone(&policy) as Arc<dyn bridge_core::ports::PolicyEngine>;
    let spawn = make_spawn_fn(policy_for_spawn, a.config.clone());
    let registry = Arc::new(bridge_registry::registry::Registry::new(snapshot, spawn).map_err(|e| format!("implement: registry: {e:?}"))?);
    let executor = bridge_workflow::executor::WorkflowExecutor::new(Arc::clone(&registry) as Arc<dyn bridge_core::ports::AgentRegistry>);
    let run_id = format!("impl-{}", task_id);
    let ctx = bridge_workflow::executor::WorkflowRunContext { session_cwd: Some(bridge_core::SessionCwd::parse(&clone.to_string_lossy())?) };
    use bridge_workflow::executor::{WorkflowEvent, WorkflowOutcome};
    use futures::StreamExt;
    let mut stream = executor.run_with_context(graph, a.task.clone(), run_id, tokio_util::sync::CancellationToken::new(), ctx);
    let mut outcome = WorkflowOutcome::Failed;
    while let Some(item) = stream.next().await {
        match item {
            Ok(WorkflowEvent::NodeStarted { node }) => eprintln!("[implement] node {} started", node.as_str()),
            Ok(WorkflowEvent::NodeFinished { node, ok, .. }) => eprintln!("[implement] node {} {}", node.as_str(), if ok {"ok"} else {"failed"}),
            Ok(WorkflowEvent::Terminal { outcome: o, .. }) => outcome = o,
            Err(e) => eprintln!("[implement] error: {e:?}"),
        }
    }
    drop(stream); // end the run; the per-turn ContainerRw container is reaped (detached) — it doesn't touch the clone.

    // 5. commit state machine.
    if !matches!(outcome, WorkflowOutcome::Completed) {
        eprintln!("[implement] workflow did not complete (outcome={outcome:?}) — NO commit; clone left at {}", clone.display());
        return Err("implement: workflow did not complete".into());
    }
    implement::head_guard(&clone, &branch, &pre)?; // agent must not have switched branch / committed itself
    match implement::stage_state(&clone)? {
        implement::StageState::Clean => { println!("implement: made no changes; clone left at {}", clone.display()); return Ok(()); }
        implement::StageState::DirtyUnstaged => {
            eprintln!("[implement] agent edited but staged NOTHING — NOT committing (agent owns staging). Clone left at {} for inspection.", clone.display());
            return Ok(());
        }
        implement::StageState::Staged => {}
    }
    let (msg, fb) = implement::commit_message(implement::read_commit_msg_file(&clone), &a.task);
    if fb { eprintln!("[implement] no .git/A2A_COMMIT_MSG — using task-derived message"); }
    let sha = implement::host_commit(&clone, &msg)?;
    let _ = std::fs::remove_file(clone.join(".git").join("A2A_COMMIT_MSG"));
    // report leftover (uncommitted) changes so the operator knows.
    if !matches!(implement::stage_state(&clone)?, implement::StageState::Clean) {
        eprintln!("[implement] note: the clone still has uncommitted changes the agent left unstaged.");
    }
    let subject = msg.lines().next().unwrap_or("").to_string();
    println!("{}", implement::handoff_text(&clone.to_string_lossy(), &branch, &sha, &subject, &a.repo.to_string_lossy()));
    Ok(())
}
```

> Imports already in `main.rs`: `PathBuf`, `Arc`, `AgentEntry`, `BoxError`, `CONFIG_PATH`. Add any missing (`bridge_policy`, `bridge_workflow`, `bridge_registry` are deps).

- [ ] **Step 4: Run — verify the parser tests + the build.** Run: `cargo test -p a2a-bridge parse_implement && cargo build -p a2a-bridge 2>&1 | tail -6`. Expected: PASS + clean build.
- [ ] **Step 5: Clippy + commit.** `cargo clippy -p a2a-bridge --all-targets -- -D warnings 2>&1 | tail -3`; `git commit -am "feat(b2b1): implement subcommand (clone -> edit workflow -> commit state machine -> hand-off)"`

---

# Slice 4 — Config + prompt + the live gate

### Task 11: the `implement-edit` workflow + prompt

**Files:** Modify `examples/a2a-bridge.containerized.toml`; Create `prompts/implement-edit.md`.

- [ ] **Step 1: Add the workflow** to `examples/a2a-bridge.containerized.toml` (the `impl` agent already exists from B2a):

```toml
[[workflows]]
# B2b-1: a single write-capable edit node. Driven by `a2a-bridge implement`.
id = "implement-edit"
[[workflows.nodes]]
id = "edit"
agent = "impl"
prompt_file = "../prompts/implement-edit.md"
inputs = []
```

- [ ] **Step 2: Create `prompts/implement-edit.md`** (the edit+stage+write-message contract):

```markdown
You are a coding agent working INSIDE a writable git clone (your current working directory). Implement the
task below. You have read/write access to the files and may use `git diff`, `git stash`, `git status`, and
`git add` as tools.

CONTRACT — follow exactly:
- Make the change for the task by editing/creating files.
- STAGE exactly the files that belong in this change with `git add <paths>` (include new files). Stage with
  judgment — do NOT stage scratch/debug files you don't want committed.
- Write your commit message (a concise subject line, optional body) to the file `.git/A2A_COMMIT_MSG`.
- Do NOT run `git commit`. Do NOT switch branches or run `git checkout`/`git reset`. The bridge commits your
  staged change on the current branch for you.
- When done, STOP. Your reply text is not used as the commit message — only `.git/A2A_COMMIT_MSG` is.

TASK:
{{input}}
```

- [ ] **Step 3: Validate the config loads** (Docker-free — a bogus run-workflow id triggers registry validation): Run: `target/debug/a2a-bridge run-workflow __v__ --input README.md --config examples/a2a-bridge.containerized.toml 2>&1 | tail -2`. Expected: `unknown workflow "__v__"` (config + the `impl` agent validate OK).
- [ ] **Step 4: Commit.** `git add examples/a2a-bridge.containerized.toml prompts/implement-edit.md && git commit -m "feat(b2b1): implement-edit workflow + prompt (edit+stage+write .git/A2A_COMMIT_MSG)"`

---

### Task 12: live acceptance gate (Docker Desktop / podman — operator-run)

- [ ] **Step 1: Pre-flight** (proxy up, creds synced, a throwaway source repo):

```bash
deploy/containers/sync-creds.sh claude
docker compose -f deploy/containers/compose.egress.yaml up -d
# a throwaway clone of THIS repo as the source (so a stray commit can't hurt the real repo):
rm -rf /Users/wesleyjinks/code/.b2b1-src && git clone --no-hardlinks /Users/wesleyjinks/code/a2a-bridge /Users/wesleyjinks/code/.b2b1-src
cargo build
```

- [ ] **Step 2: Run `implement`** against the throwaway repo:

```bash
( docker events --since 0s --filter image=a2a-agent-reader:latest --format '{{.Action}} {{index .Actor.Attributes "name"}}' > /tmp/b2b1-events.log 2>&1 & echo $! > /tmp/b2b1-ev.pid )
./target/debug/a2a-bridge implement \
  "Create a file B2B1_OK.md at the repo root containing exactly the line B2B1_OK. Stage it with git add, write a one-line commit message to .git/A2A_COMMIT_MSG, and STOP." \
  --repo /Users/wesleyjinks/code/.b2b1-src \
  --config examples/a2a-bridge.containerized.toml
kill "$(cat /tmp/b2b1-ev.pid)" 2>/dev/null
```

- [ ] **Step 3: Assert the gate** (all must hold):

```bash
CLONE=$(ls -d /Users/wesleyjinks/code/.a2a-implement/impl-* | tail -1)
echo "clone: $CLONE"
# (a) a real implement/<id> branch with the staged change committed under the BOT identity:
git -C "$CLONE" log -1 --format='%an <%ae> | %s'    # -> a2a-implement <implement@a2a-bridge.local> | ...
git -C "$CLONE" show --stat HEAD | grep B2B1_OK.md && echo "FILE_COMMITTED_OK"
test "$(git -C "$CLONE" show HEAD:B2B1_OK.md)" = "B2B1_OK" && echo "CONTENT_OK"
# (b) the SOURCE repo is untouched (no new branch, clean):
git -C /Users/wesleyjinks/code/.b2b1-src status --porcelain | head ; git -C /Users/wesleyjinks/code/.b2b1-src branch | grep implement/ && echo "LEAK" || echo "SOURCE_UNTOUCHED_OK"
# (c) positive containment: a real a2a-rw-* container ran + was reaped:
grep -E "start a2a-rw-|die a2a-rw-|destroy a2a-rw-" /tmp/b2b1-events.log && echo "CONTAINED_OK"
docker ps -a --filter name=a2a-rw- --format '{{.Names}}' | grep . && echo "LEFTOVER" || echo "REAPED_OK"
# cleanup
rm -rf "$CLONE" /Users/wesleyjinks/code/.b2b1-src
```

Expected: `FILE_COMMITTED_OK`, `CONTENT_OK`, bot identity on the commit, `SOURCE_UNTOUCHED_OK`, `CONTAINED_OK`, `REAPED_OK`. **If the commit's author is NOT the bot or the file is root-owned and the commit failed → the uid/`safe.directory` posture needs podman (preferred) or Docker Desktop's remap; rootful-Docker-on-Linux is out of scope.**

- [ ] **Step 4: Empty-staging path** (a second run with a prompt that edits but does NOT stage) → expect `[implement] agent edited but staged NOTHING — NOT committing` + the clone left, no commit.

- [ ] **Step 5: Full verification + commit any fixes.** `cargo test --workspace 2>&1 | grep -E "FAILED|error\[" || echo green`; `cargo clippy --workspace --all-targets -- -D warnings 2>&1 | tail -2`; `cargo llvm-cov clean --workspace && cargo llvm-cov --workspace 2>&1 | tail -3` (floors 85/90/90).

---

## Self-Review

**1. Spec coverage:** `implement` subcommand (T10) ✓ · clone lifecycle + `--no-hardlinks` + canonicalized root + clone-not-inside-worktree + task-id collision-retry (T3,T8,T10) ✓ · host-commits soft gate with the full `-c` pins + `--no-verify` (T1,T7) ✓ · DROP `add -A` / agent-owns-staging (T10 DirtyUnstaged arm) ✓ · `.git/A2A_COMMIT_MSG` channel + fallback (T2,T11) ✓ · HEAD guard (T6,T10) ✓ · `status --porcelain` untracked detection (T5) ✓ · `Completed` gate + settle/stale-lock (T7,T10) ✓ · corrected hand-off re-author (T4) ✓ · `--base-ref` (T10) ✓ · direct argv no shell (T5 `run_git`) ✓ · podman-preferred/`safe.directory` posture (T1 pins, T12 note) ✓ · temp-repo tests for the impure classifier/commit (T5–T8) ✓ · live gate (T12) ✓.

**2. Placeholder scan:** the only deferred body is Task 9's "MOVE the existing closure body here verbatim" — that's a mechanical lift of code already in the repo (cited location), not new logic. No `TODO`/`add error handling`/`similar to`.

**3. Type consistency:** `implement::{clone_argv, commit_argv, commit_message, read_commit_msg_file, task_id, branch_for, nonce, handoff_text, run_git, StageState, stage_state, head_sha, current_branch, head_guard, host_commit, assert_dest_outside_worktree, do_clone, do_checkout_branch}` — names match across T1–T10. `WorkflowOutcome::Completed` + `WorkflowEvent::Terminal{outcome}` match executor.rs:35,51. `RegistryConfig.allowed_cwd_root` + `into_snapshot` + `load_workflows` + `Registry::new(snap, spawn)` match the `run_workflow_cmd` seams.

**Open item for the plan-review:** the container-reap-before-host-commit ordering (T10 drops the stream; the per-turn reaper is detached) — confirm the stale-`index.lock` clear in `host_commit` (T7) is a sufficient settle, or whether `implement_cmd` should additionally `retire()` the resolved `impl` backend before committing.

---

## Plan rev2 — dual-review corrections (BINDING; override the body where they conflict)

Both plan-reviews (containerized dogfood + a2a-local codex `gpt-5.5`) = **needs-changes**; both verified the
real seams compile and affirmed the decomposition/design. Apply ALL of R1–R14.

- **R1 — Remove unused test imports** (clippy `-D warnings` blocker). In Task 5's `#[cfg(test)]` block do
  NOT add `use std::path::Path;` (it's unused in tests — production already imports it); keep
  `use std::process::Command;` (used by `temp_repo`). Verify each test module compiles clean under
  `-D warnings`.
- **R2 — Genuinely tests-first ordering.** In every pure task (T1–T4), Step "write the failing test"
  contains ONLY the test; the production fns land in the "implement" step. (T1's first block currently mixes
  them — split it.)
- **R3 — Extract a pure `decide()` (the coverage keystone, P#6).** Add to `implement.rs`:
  ```rust
  pub enum Action { Commit(String), NoCommitDirty, NoCommitClean, Abort(String) }
  /// Pure soft-gate decision. `head_guard` is the head_guard result; `msg` is (message, used_fallback).
  pub fn decide(completed: bool, head_guard: Result<(), String>, stage: StageState, msg: (String, bool)) -> Action {
      if !completed { return Action::Abort("workflow did not complete".into()); }
      if let Err(e) = head_guard { return Action::Abort(e); }
      match stage {
          StageState::Clean => Action::NoCommitClean,
          StageState::DirtyUnstaged => Action::NoCommitDirty,
          StageState::Staged => Action::Commit(msg.0),
      }
  }
  ```
  **Unit-test the full matrix** (completed×head_guard×stage). `implement_cmd` resolves the inputs
  (outcome, `head_guard(...)`, `stage_state(...)`, `commit_message(...)`) then `match decide(...)`: `Commit`
  → `host_commit`+strip+leftover-report+hand-off; `NoCommitDirty` → flag + leave clone; `NoCommitClean` →
  "no changes" + leave; `Abort(r)` → eprintln + leave + Err. This closes the coverage hole on the riskiest
  logic and shrinks Task 10.
- **R4 — Split Task 10** into 10a (dispatch arm + `parse_implement_args`, unit-tested) and 10b
  (`implement_cmd` orchestration that resolves inputs + executes `decide`'s `Action`). State the expected
  workspace-coverage movement (positive — T1–T8 + `decide` add a heavily-tested `implement.rs` to the
  uncapped `a2a-bridge` crate).
- **R5 — Check git exit status** in `stage_state`, `head_sha`, `current_branch` (and any `run_git` reader):
  on `!out.status.success()` return an `Err` carrying stderr (else a failed `git status` → false `Clean`).
  Change their signatures to `Result<_, String>` and the temp-repo tests accordingly.
- **R6 — Clone-guard before mkdir.** Probe the **existing canonical `allowed_cwd_root`** (which exists)
  with `assert_dest_outside_worktree` BEFORE `create_dir_all(.a2a-implement)` — make
  `assert_dest_outside_worktree` walk to the nearest existing ancestor of its argument so it never depends
  on the not-yet-created dir. Reorder T10: read+canonicalize root → guard(root) → mkdir → task-id → clone.
- **R7 — `host_commit` retry-on-lock (not blind pre-clear)** [spec settle ratified]. Do NOT
  `remove_file(index.lock)` up front. Attempt `git commit`; on an index-lock error (`stderr` contains
  `index.lock` / `Another git process`) sleep ~200ms and retry up to 5×; only if still locked, remove the
  stale `.git/index.lock` and make one final attempt. Add `use std::time::Duration` + `std::thread::sleep`.
- **R8 — `read_commit_msg_file`: bounded read + NUL reject.** Open the file and read at most 64 KiB via a
  `take(65536)` reader (don't `fs::read` the whole file); return `None` on read error, oversize, invalid
  UTF-8, **or any NUL byte** (a NUL breaks `git commit -m`). Add a temp-file test for the NUL/oversize cases.
- **R9 — `--base-ref` → SHA.** After cloning (or before), resolve the ref to a SHA: when `--base-ref` is
  given, `git -C <clone> rev-parse <ref>` then `checkout <sha>`; when absent, resolve the **source repo's**
  HEAD to a SHA first (`git -C <repo> rev-parse HEAD`) and check that SHA out in the clone (honors the
  detached-HEAD clause; pins against a concurrent push). Error on an unresolvable ref.
- **R10 — Extend the Task 7 hook test** to also plant a `core.hooksPath`-pointed `pre-commit` AND a
  `prepare-commit-msg`/`post-commit` hook (all `exit 1` / side-effecting) and assert the commit still
  succeeds + no side effect — proving the full hook surface the spec claims, not just default `pre-commit`.
- **R11 — Strengthen the config-validation check (T11).** The "unknown workflow" run-workflow probe returns
  BEFORE `into_snapshot`, so it doesn't validate the registry. Add a Docker-free check/test that
  `RegistryConfig::parse` + `load_workflows` (finds `implement-edit`) + `into_snapshot` + `Registry::new`
  all succeed for `examples/a2a-bridge.containerized.toml` (the `impl` agent + the workflow validate).
- **R12 — `implement` usage discoverability.** Add `implement` to `main.rs`'s unknown-subcommand usage
  enumeration (`serve | run-workflow | submit | task | init` → `… | implement`) + the top command comment,
  and give `parse_implement_args` errors a one-line usage hint.
- **R13 — Strip `.git/A2A_COMMIT_MSG` before AND after the commit** (the spec says before/after; the body
  only stripped after). Quote the clone/repo **paths in the hand-off text** (so spaces don't garble the
  printed copy-paste). Task 12's gate **parses the clone path from `implement`'s printed hand-off** (the
  `clone:` line), not `ls … | tail`.
- **R14 — Confirmed intentional (no change):** `implement-edit` duplicates `impl-smoke`'s 1-node shape but
  with a distinct contract (the B2a smoke writes a marker; B2b-1's edit+stage+`.git/A2A_COMMIT_MSG`
  contract) — keep both.

**Both verdicts:** design + decomposition sound; R1–R14 are correctness/decomposition/coverage fixes + the
ratified settle. After applying them the plan builds green-per-task with the soft-gate matrix unit-covered;
no third plan-review needed — the inline TDD build is the verification.
