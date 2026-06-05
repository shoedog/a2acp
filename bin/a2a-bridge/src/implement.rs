//! `a2a-bridge implement` — clone a repo into a quarantine, have the ContainerRw `impl` agent edit+stage
//! a change, host-commit the agent-staged index on a task branch, and leave the clone for the operator.
//!
//! Pure helpers (argv builders, the `.git/A2A_COMMIT_MSG` reader, task-id, hand-off text, the `decide`
//! soft-gate) are git-free unit-tested; the impure git ops get temp-repo tests. The orchestration lives in
//! `main.rs::implement_cmd`.

use std::path::Path;
use std::process::Command;
use std::time::Duration;

/// The bot identity the bridge commits under (rewritable pre-merge; the operator re-authors at merge).
pub const BOT_NAME: &str = "a2a-implement";
pub const BOT_EMAIL: &str = "implement@a2a-bridge.local";

// ─── Pure argv builders ──────────────────────────────────────────────────────

/// `git clone --no-hardlinks <repo> <dest>` (committed-only quarantine; independent object store).
pub fn clone_argv(repo: &str, dest: &str) -> Vec<String> {
    vec![
        "clone".into(),
        "--no-hardlinks".into(),
        repo.into(),
        dest.into(),
    ]
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
        "-c".into(),
        format!("safe.directory={clone}"),
        "-c".into(),
        "core.hooksPath=/dev/null".into(),
        "-c".into(),
        "commit.gpgsign=false".into(),
        "-c".into(),
        format!("user.name={BOT_NAME}"),
        "-c".into(),
        format!("user.email={BOT_EMAIL}"),
        "commit".into(),
        "--no-verify".into(),
        "-m".into(),
        msg.into(),
    ]
}

// ─── Commit message ──────────────────────────────────────────────────────────

/// Resolve the commit message: the agent-written `.git/A2A_COMMIT_MSG` content if non-blank, else a
/// deterministic task-derived fallback `implement: <first line of task, truncated>`. Returns
/// (message, used_fallback). `raw` is the file content (None if absent/unreadable/oversize/NUL/non-UTF-8).
pub fn commit_message(raw: Option<String>, task: &str) -> (String, bool) {
    if let Some(s) = raw {
        let trimmed = s.trim();
        if !trimmed.is_empty() {
            return (trimmed.to_string(), false);
        }
    }
    let first = task.lines().next().unwrap_or("").trim();
    let mut subj: String = first.chars().take(120).collect();
    if subj.is_empty() {
        subj = "changes".into();
    }
    (format!("implement: {subj}"), true)
}

/// Read `<clone>/.git/A2A_COMMIT_MSG`, bounded to 64 KiB so an oversized/binary file can't blow memory.
/// Returns None on absent / unreadable / oversize / **any NUL byte** (breaks `git commit -m`) / non-UTF-8.
pub fn read_commit_msg_file(clone: &Path) -> Option<String> {
    use std::io::Read;
    let p = clone.join(".git").join("A2A_COMMIT_MSG");
    let f = std::fs::File::open(p).ok()?;
    let mut buf = Vec::new();
    f.take(64 * 1024 + 1).read_to_end(&mut buf).ok()?;
    if buf.len() > 64 * 1024 {
        return None;
    }
    if buf.contains(&0) {
        return None;
    }
    String::from_utf8(buf).ok()
}

// ─── Task id / branch ────────────────────────────────────────────────────────

/// `impl-<pid>-<nonce>` — filesystem- and branch-name-safe.
pub fn task_id(pid: u32, nonce: &str) -> String {
    format!("impl-{pid}-{nonce}")
}
pub fn branch_for(task_id: &str) -> String {
    format!("implement/{task_id}")
}
/// A lowercase-alnum nonce of length `n` (the caller retries against existing clone dirs/branches, so
/// uniqueness is belt-and-suspenders, not crypto).
pub fn nonce(n: usize) -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let seed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0)
        ^ (std::process::id() as u128);
    const A: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789";
    let mut s = String::new();
    let mut x = seed;
    for _ in 0..n {
        s.push(A[(x % A.len() as u128) as usize] as char);
        x /= A.len() as u128;
        if x == 0 {
            x = seed.rotate_left(7) | 1;
        }
    }
    s
}

// ─── Hand-off text ───────────────────────────────────────────────────────────

/// The operator hand-off (informational): merge the bot-authored quarantine branch into <repo> RE-AUTHORED
/// as the operator, then reap the clone. Paths are quoted so spaces survive the copy-paste. The `clone:`
/// line carries the bare path (the acceptance gate parses it).
pub fn handoff_text(clone: &str, branch: &str, sha: &str, subject: &str, repo: &str) -> String {
    format!(
        "implement: committed {sha} \"{subject}\" on {branch}\n\
         clone: {clone}\n\
         To merge as YOURSELF (bot identity is pre-merge only) and reap the clone:\n\
         \x20 git -C \"{repo}\" fetch \"{clone}\" {branch}\n\
         \x20 git -C \"{repo}\" cherry-pick -n FETCH_HEAD && git -C \"{repo}\" commit -C FETCH_HEAD --reset-author\n\
         \x20 rm -rf \"{clone}\"\n"
    )
}

// ─── Impure git ops (temp-repo tested) ───────────────────────────────────────

#[derive(Debug, PartialEq, Eq)]
pub enum StageState {
    Staged,
    DirtyUnstaged,
    Clean,
}

/// Run `git [-C cwd] <argv>` capturing output. Direct argv — no shell.
pub fn run_git(cwd: Option<&Path>, argv: &[&str]) -> std::io::Result<std::process::Output> {
    let mut c = Command::new("git");
    if let Some(d) = cwd {
        c.arg("-C").arg(d);
    }
    c.args(argv).output()
}

/// Run git, REQUIRE success, return trimmed stdout (else Err with stderr) — so a failed `git status` can't
/// be misread as "clean".
fn git_ok(cwd: Option<&Path>, argv: &[&str]) -> Result<String, String> {
    let out = run_git(cwd, argv).map_err(|e| format!("git {}: {e}", argv.join(" ")))?;
    if !out.status.success() {
        return Err(format!(
            "git {} failed: {}",
            argv.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Classify the working tree via `git status --porcelain` (detects UNTRACKED files, unlike
/// `git diff --quiet`). Staged = any line whose index column (X) is not ' ' and not '?'.
pub fn stage_state(clone: &Path) -> Result<StageState, String> {
    let text = git_ok(Some(clone), &["status", "--porcelain"])?;
    let mut any = false;
    let mut staged = false;
    for line in text.lines() {
        if line.len() < 2 {
            continue;
        }
        any = true;
        let x = line.as_bytes()[0] as char;
        if x != ' ' && x != '?' {
            staged = true;
        }
    }
    Ok(if staged {
        StageState::Staged
    } else if any {
        StageState::DirtyUnstaged
    } else {
        StageState::Clean
    })
}

pub fn head_sha(clone: &Path) -> Result<String, String> {
    git_ok(Some(clone), &["rev-parse", "HEAD"])
}
pub fn current_branch(clone: &Path) -> Result<String, String> {
    git_ok(Some(clone), &["symbolic-ref", "--short", "HEAD"])
}

/// The agent has :rw + git and could switch branches or commit despite the contract. Assert HEAD is still
/// `expect_branch` and hasn't advanced past `pre_sha`. Returns a human error for the subcommand.
pub fn head_guard(clone: &Path, expect_branch: &str, pre_sha: &str) -> Result<(), String> {
    let br = current_branch(clone)?;
    if br != expect_branch {
        return Err(format!(
            "agent switched branch: HEAD is {br:?}, expected {expect_branch:?}"
        ));
    }
    let sha = head_sha(clone)?;
    if sha != pre_sha {
        return Err(format!(
            "agent advanced HEAD (committed itself?) {pre_sha}..{sha} — leaving clone for the operator"
        ));
    }
    Ok(())
}

/// Deterministically commit the AGENT-STAGED index with the bot identity + the full hook/sign/ownership
/// pins. Bounded retry on an index-lock error (the per-turn container that held it is being reaped);
/// clears a stale `.git/index.lock` only AFTER retries exhaust. Returns the new commit sha. Stages nothing.
pub fn host_commit(clone: &Path, msg: &str) -> Result<String, String> {
    let argv = commit_argv(&clone.to_string_lossy(), msg);
    let refs: Vec<&str> = argv.iter().map(String::as_str).collect();
    for _ in 0..5 {
        let out = run_git(Some(clone), &refs).map_err(|e| format!("git commit: {e}"))?;
        if out.status.success() {
            return head_sha(clone);
        }
        let err = String::from_utf8_lossy(&out.stderr);
        if !(err.contains("index.lock") || err.contains("Another git process")) {
            return Err(format!("git commit failed: {}", err.trim()));
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    let _ = std::fs::remove_file(clone.join(".git").join("index.lock")); // stale-lock clear, last resort
    let out = run_git(Some(clone), &refs).map_err(|e| format!("git commit: {e}"))?;
    if out.status.success() {
        head_sha(clone)
    } else {
        Err(format!(
            "git commit failed after lock retries: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ))
    }
}

/// Refuse a clone dest inside a git worktree (cloning into a repo dirties it). Walks to the nearest
/// EXISTING ancestor of `dest` (dest may not exist yet) and probes it — so it's safe to call BEFORE the
/// clone dir is created.
pub fn assert_dest_outside_worktree(dest: &Path) -> Result<(), String> {
    let mut p = dest;
    let existing = loop {
        if p.exists() {
            break p;
        }
        match p.parent() {
            Some(par) => p = par,
            None => return Ok(()), // reached the root with no enclosing repo
        }
    };
    let out = run_git(Some(existing), &["rev-parse", "--is-inside-work-tree"])
        .map_err(|e| e.to_string())?;
    if out.status.success() && String::from_utf8_lossy(&out.stdout).trim() == "true" {
        return Err(format!(
            "clone dest {dest:?} is inside a git worktree — refusing (would dirty that repo)"
        ));
    }
    Ok(())
}

pub fn do_clone(repo: &str, dest: &str) -> Result<(), String> {
    let argv = clone_argv(repo, dest);
    let refs: Vec<&str> = argv.iter().map(String::as_str).collect();
    let out = run_git(None, &refs).map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(format!(
            "git clone failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(())
}
pub fn do_checkout_branch(clone: &Path, branch: &str) -> Result<(), String> {
    let argv = checkout_new_branch_argv(branch);
    let refs: Vec<&str> = argv.iter().map(String::as_str).collect();
    let out = run_git(Some(clone), &refs).map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(format!(
            "git checkout -b failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(())
}

// ─── The pure soft-gate decision ─────────────────────────────────────────────

/// What the commit state machine should do after the edit turn.
#[derive(Debug, PartialEq, Eq)]
pub enum Action {
    Commit(String),
    NoCommitDirty,
    NoCommitClean,
    Abort(String),
}

/// PURE soft gate: gate on workflow completion + the HEAD guard, then the stage state. Unit-tested matrix.
pub fn decide(
    completed: bool,
    head_guard: Result<(), String>,
    stage: StageState,
    msg: (String, bool),
) -> Action {
    if !completed {
        return Action::Abort("workflow did not complete".into());
    }
    if let Err(e) = head_guard {
        return Action::Abort(e);
    }
    match stage {
        StageState::Clean => Action::NoCommitClean,
        StageState::DirtyUnstaged => Action::NoCommitDirty,
        StageState::Staged => Action::Commit(msg.0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    // ── pure helpers ──────────────────────────────────────────────────────

    #[test]
    fn clone_argv_no_hardlinks() {
        assert_eq!(
            clone_argv("/src/repo", "/root/.a2a-implement/impl-1-ab"),
            vec![
                "clone",
                "--no-hardlinks",
                "/src/repo",
                "/root/.a2a-implement/impl-1-ab"
            ]
        );
    }

    #[test]
    fn commit_argv_pins_before_commit() {
        let a = commit_argv("/root/.a2a-implement/impl-1-ab", "subject");
        let joined = a.join(" ");
        assert!(joined.contains("-c safe.directory=/root/.a2a-implement/impl-1-ab"));
        assert!(joined.contains("-c core.hooksPath=/dev/null"));
        assert!(joined.contains("-c commit.gpgsign=false"));
        assert!(joined.contains("-c user.name=a2a-implement"));
        assert!(joined.contains("-c user.email=implement@a2a-bridge.local"));
        let ci = a.iter().position(|x| x == "commit").unwrap();
        assert_eq!(
            a.iter().take(ci).filter(|x| *x == "-c").count(),
            5,
            "all -c before commit"
        );
        assert_eq!(&a[ci..], &["commit", "--no-verify", "-m", "subject"]);
    }

    #[test]
    fn commit_message_file_else_fallback() {
        assert_eq!(
            commit_message(Some("  Fix the widget\n\ndetails\n".into()), "task ignored"),
            ("Fix the widget\n\ndetails".to_string(), false)
        );
        assert_eq!(
            commit_message(None, "Add a FOO marker file to the repo root\nmore"),
            (
                "implement: Add a FOO marker file to the repo root".to_string(),
                true
            )
        );
        assert!(commit_message(Some("   \n  ".into()), "Tidy up").1);
        let long = "x".repeat(500);
        let (m, fb) = commit_message(None, &long);
        assert!(fb && m.starts_with("implement: ") && m.len() <= "implement: ".len() + 120);
    }

    #[test]
    fn read_commit_msg_file_bounded_and_nul_rejected() {
        let td = tempfile::tempdir().unwrap();
        let gitdir = td.path().join(".git");
        std::fs::create_dir_all(&gitdir).unwrap();
        // valid
        std::fs::write(gitdir.join("A2A_COMMIT_MSG"), "hello").unwrap();
        assert_eq!(read_commit_msg_file(td.path()).as_deref(), Some("hello"));
        // NUL -> None
        std::fs::write(gitdir.join("A2A_COMMIT_MSG"), b"he\0llo").unwrap();
        assert_eq!(read_commit_msg_file(td.path()), None);
        // oversize -> None
        std::fs::write(gitdir.join("A2A_COMMIT_MSG"), "x".repeat(70 * 1024)).unwrap();
        assert_eq!(read_commit_msg_file(td.path()), None);
        // absent -> None
        std::fs::remove_file(gitdir.join("A2A_COMMIT_MSG")).unwrap();
        assert_eq!(read_commit_msg_file(td.path()), None);
    }

    #[test]
    fn task_id_branch_nonce() {
        assert_eq!(task_id(4242, "k3x9"), "impl-4242-k3x9");
        assert_eq!(branch_for("impl-4242-k3x9"), "implement/impl-4242-k3x9");
        assert!(nonce(12)
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit()));
        assert_eq!(nonce(8).len(), 8);
    }

    #[test]
    fn handoff_text_corrected_reauthor_and_quoted() {
        let t = handoff_text(
            "/root/.a2a-implement/impl-1-ab",
            "implement/impl-1-ab",
            "abc1234",
            "Fix widget",
            "/src/repo",
        );
        assert!(t.contains("clone: /root/.a2a-implement/impl-1-ab")); // bare path for the gate to parse
        assert!(
            t.contains("implement/impl-1-ab") && t.contains("abc1234") && t.contains("Fix widget")
        );
        assert!(t.contains("cherry-pick -n FETCH_HEAD"));
        assert!(t.contains("commit -C FETCH_HEAD --reset-author"));
        assert!(!t.contains("cherry-pick --reset-author"));
        assert!(t.contains("rm -rf \"/root/.a2a-implement/impl-1-ab\"")); // quoted
        assert!(t.contains("git -C \"/src/repo\" fetch"));
    }

    #[test]
    fn decide_matrix() {
        let msg = ("m".to_string(), false);
        assert_eq!(
            decide(false, Ok(()), StageState::Staged, msg.clone()),
            Action::Abort("workflow did not complete".into())
        );
        assert_eq!(
            decide(
                true,
                Err("switched".into()),
                StageState::Staged,
                msg.clone()
            ),
            Action::Abort("switched".into())
        );
        assert_eq!(
            decide(true, Ok(()), StageState::Clean, msg.clone()),
            Action::NoCommitClean
        );
        assert_eq!(
            decide(true, Ok(()), StageState::DirtyUnstaged, msg.clone()),
            Action::NoCommitDirty
        );
        assert_eq!(
            decide(true, Ok(()), StageState::Staged, msg.clone()),
            Action::Commit("m".into())
        );
    }

    // ── impure git ops (temp-repo) ────────────────────────────────────────

    fn temp_repo() -> (tempfile::TempDir, std::path::PathBuf) {
        let td = tempfile::tempdir().unwrap();
        let p = td.path().to_path_buf();
        for argv in [
            vec!["init", "-q", "-b", "main"],
            vec!["config", "user.name", "t"],
            vec!["config", "user.email", "t@t"],
        ] {
            assert!(Command::new("git")
                .arg("-C")
                .arg(&p)
                .args(argv)
                .status()
                .unwrap()
                .success());
        }
        std::fs::write(p.join("README.md"), "hi\n").unwrap();
        assert!(Command::new("git")
            .arg("-C")
            .arg(&p)
            .args(["add", "README.md"])
            .status()
            .unwrap()
            .success());
        assert!(Command::new("git")
            .arg("-C")
            .arg(&p)
            .args(["commit", "-q", "-m", "init"])
            .status()
            .unwrap()
            .success());
        (td, p)
    }

    #[test]
    fn stage_state_classifies_and_errors_on_non_repo() {
        let (_g, p) = temp_repo();
        assert_eq!(stage_state(&p).unwrap(), StageState::Clean);
        std::fs::write(p.join("FOO.md"), "bar\n").unwrap(); // untracked, NOT staged
        assert_eq!(stage_state(&p).unwrap(), StageState::DirtyUnstaged);
        assert!(Command::new("git")
            .arg("-C")
            .arg(&p)
            .args(["add", "FOO.md"])
            .status()
            .unwrap()
            .success());
        assert_eq!(stage_state(&p).unwrap(), StageState::Staged);
        // a non-git dir -> Err (not falsely Clean)
        let nd = tempfile::tempdir().unwrap();
        assert!(stage_state(nd.path()).is_err());
    }

    #[test]
    fn head_guard_detects_switch_and_advance() {
        let (_g, p) = temp_repo();
        assert!(Command::new("git")
            .arg("-C")
            .arg(&p)
            .args(["checkout", "-q", "-b", "implement/x"])
            .status()
            .unwrap()
            .success());
        let pre = head_sha(&p).unwrap();
        assert!(head_guard(&p, "implement/x", &pre).is_ok());
        std::fs::write(p.join("A.md"), "a\n").unwrap();
        assert!(Command::new("git")
            .arg("-C")
            .arg(&p)
            .args(["add", "A.md"])
            .status()
            .unwrap()
            .success());
        assert!(Command::new("git")
            .arg("-C")
            .arg(&p)
            .args(["commit", "-q", "-m", "agent"])
            .status()
            .unwrap()
            .success());
        assert!(head_guard(&p, "implement/x", &pre)
            .unwrap_err()
            .contains("advanced"));
        assert!(Command::new("git")
            .arg("-C")
            .arg(&p)
            .args(["checkout", "-q", "main"])
            .status()
            .unwrap()
            .success());
        assert!(head_guard(&p, "implement/x", &pre)
            .unwrap_err()
            .contains("branch"));
    }

    #[test]
    fn host_commit_pins_neutralize_all_hooks_and_uses_bot_identity() {
        let (_g, p) = temp_repo();
        assert!(Command::new("git")
            .arg("-C")
            .arg(&p)
            .args(["checkout", "-q", "-b", "implement/x"])
            .status()
            .unwrap()
            .success());
        // plant pre-commit (default path), a core.hooksPath-redirected hook dir, and prepare-commit-msg —
        // all `exit 1`; the commit must STILL succeed (proves --no-verify + core.hooksPath=/dev/null).
        let hooks = p.join(".git").join("hooks");
        std::fs::create_dir_all(&hooks).unwrap();
        for h in ["pre-commit", "prepare-commit-msg", "post-commit"] {
            let f = hooks.join(h);
            std::fs::write(&f, "#!/bin/sh\nexit 1\n").unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o755)).unwrap();
            }
        }
        // also a core.hooksPath the agent could have set, with its own failing pre-commit
        let alt = p.join(".git").join("althooks");
        std::fs::create_dir_all(&alt).unwrap();
        let af = alt.join("pre-commit");
        std::fs::write(&af, "#!/bin/sh\nexit 1\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&af, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        assert!(Command::new("git")
            .arg("-C")
            .arg(&p)
            .args(["config", "core.hooksPath", alt.to_str().unwrap()])
            .status()
            .unwrap()
            .success());

        std::fs::write(p.join("FOO.md"), "bar\n").unwrap();
        assert!(Command::new("git")
            .arg("-C")
            .arg(&p)
            .args(["add", "FOO.md"])
            .status()
            .unwrap()
            .success());
        let sha = host_commit(&p, "subject line").expect("commit despite the failing hooks");
        assert!(!sha.is_empty());
        let an = Command::new("git")
            .arg("-C")
            .arg(&p)
            .args(["log", "-1", "--format=%an <%ae>"])
            .output()
            .unwrap();
        assert_eq!(
            String::from_utf8_lossy(&an.stdout).trim(),
            "a2a-implement <implement@a2a-bridge.local>"
        );
        let subj = Command::new("git")
            .arg("-C")
            .arg(&p)
            .args(["log", "-1", "--format=%s"])
            .output()
            .unwrap();
        assert_eq!(String::from_utf8_lossy(&subj.stdout).trim(), "subject line");
    }

    #[test]
    fn clone_dest_guard_and_independent_quarantine() {
        let (_g, repo) = temp_repo();
        // a path inside the repo's worktree is rejected (probes the nearest existing ancestor)
        assert!(
            assert_dest_outside_worktree(&repo.join(".a2a-implement").join("impl-1-ab")).is_err()
        );
        // a fresh tempdir (no enclosing repo) is OK
        let dst = tempfile::tempdir().unwrap();
        assert!(assert_dest_outside_worktree(&dst.path().join("impl-1-ab")).is_ok());
        // clone + branch -> independent (--no-hardlinks): committing in the clone doesn't touch the source
        let clone = dst.path().join("impl-1-ab");
        do_clone(&repo.to_string_lossy(), &clone.to_string_lossy()).unwrap();
        do_checkout_branch(&clone, "implement/impl-1-ab").unwrap();
        assert_eq!(current_branch(&clone).unwrap(), "implement/impl-1-ab");
        let before = head_sha(&repo).unwrap();
        std::fs::write(clone.join("X.md"), "x\n").unwrap();
        run_git(Some(&clone), &["add", "X.md"]).unwrap();
        host_commit(&clone, "c").unwrap();
        assert_eq!(head_sha(&repo).unwrap(), before, "source repo untouched");
    }
}
