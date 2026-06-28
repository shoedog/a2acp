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

pub struct WarmEgress {
    pub network: String,
    pub proxy: String,
}

/// PURE. The `(program, argv)` to fetch deps into the impl-lsp cache via the registries egress, NO creds.
/// `runtime` (docker|podman) + `image` come from `[verify]` so the warm fetch tracks the same runtime the
/// rest of the pipeline uses — hardcoding `docker`/`a2a-toolchain:latest` silently broke the podman config.
pub fn compose_warm_fetch(
    runtime: &str,
    image: &str,
    clone: &str,
    cache: &bridge_core::profile::CacheBinding,
    fetch_cmd: &str,
    e: &WarmEgress,
    read_only: bool,
) -> (String, Vec<String>) {
    let mut argv = vec![
        "run".into(),
        "--rm".into(),
        "--network".into(),
        e.network.clone(),
        "-e".into(),
        format!("HTTPS_PROXY={}", e.proxy),
        "-e".into(),
        format!("HTTP_PROXY={}", e.proxy),
    ];
    for (k, v) in &cache.env {
        argv.push("-e".into());
        argv.push(format!("{k}={v}"));
    }
    argv.push("-v".into());
    argv.push(if read_only {
        format!("{clone}:/work:ro")
    } else {
        format!("{clone}:/work")
    });
    for m in &cache.mounts {
        argv.push("-v".into());
        argv.push(m.clone());
    }
    argv.push("--workdir".into());
    argv.push("/work".into());
    argv.push("--entrypoint".into());
    argv.push("bash".into());
    argv.push(image.into());
    argv.push("-c".into());
    argv.push(fetch_cmd.to_string());
    (runtime.to_string(), argv)
}

// ─── Commit message ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommitSource {
    Typed,
    File,
    Title,
    Derived,
}

fn strip_html_comments(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;

    while let Some(start) = rest.find("<!--") {
        out.push_str(&rest[..start]);
        let after_start = &rest[start + "<!--".len()..];
        let Some(end) = after_start.find("-->") else {
            return out;
        };
        rest = &after_start[end + "-->".len()..];
    }

    out.push_str(rest);
    out
}

fn commit_candidate(raw: String, strip_comments: bool) -> Option<String> {
    let without_comments = if strip_comments {
        strip_html_comments(&raw)
    } else {
        raw
    };
    let without_nuls = without_comments.replace('\0', "");
    let trimmed = without_nuls.trim();
    if trimmed.is_empty() || trimmed.len() > 64 * 1024 {
        return None;
    }
    Some(trimmed.to_string())
}

/// Resolve the commit message from typed spec content, the agent-written `.git/A2A_COMMIT_MSG`,
/// the parsed task title, or finally a deterministic task-derived fallback.
pub fn commit_message(
    typed: Option<String>,
    file: Option<String>,
    title: &str,
    task: &str,
) -> (String, CommitSource) {
    if let Some(s) = typed.and_then(|s| commit_candidate(s, true)) {
        return (s, CommitSource::Typed);
    }
    if let Some(s) = file.and_then(|s| commit_candidate(s, true)) {
        return (s, CommitSource::File);
    }
    if let Some(s) = commit_candidate(title.to_string(), false) {
        return (s, CommitSource::Title);
    }
    let first = task.lines().next().unwrap_or("").replace('\0', "");
    let first = first.trim();
    let mut subj: String = first.chars().take(120).collect();
    if subj.is_empty() {
        subj = "changes".into();
    }
    (format!("implement: {subj}"), CommitSource::Derived)
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

// ─── claude OAuth cred preflight ──────────────────────────────────────────────

/// Parse the `expiresAt` (epoch millis) from a claude `.credentials.json` body. Tolerates the
/// `{"claudeAiOauth":{"expiresAt":..}}` wrapper or a flat object; `expiresAt` or `expires_at`.
pub fn parse_claude_expiry_ms(json: &str) -> Option<i64> {
    let v: serde_json::Value = serde_json::from_str(json).ok()?;
    let o = v.get("claudeAiOauth").unwrap_or(&v);
    o.get("expiresAt")
        .or_else(|| o.get("expires_at"))
        .and_then(|e| e.as_i64())
}

/// Preflight for a claude-agent-acp agent: if it mounts a `.credentials.json` whose OAuth token is already
/// EXPIRED, fail fast with a clear message instead of letting the in-container agent crash with the opaque
/// `AgentCrashed{session/prompt failed: transport error or kill-switch escalation}`. `now_ms` is injected so
/// the comparison is testable. `volumes` are the sandbox `host:container[:opts]` mount specs. No-op for
/// non-claude agents, no mounted cred, or an unreadable/unparseable cred (don't block on uncertainty).
pub fn claude_cred_preflight(
    cmd: Option<&str>,
    volumes: &[String],
    now_ms: i64,
) -> Result<(), String> {
    if cmd != Some("claude-agent-acp") {
        return Ok(());
    }
    for vol in volumes {
        let host = vol.split(':').next().unwrap_or("");
        if !host.ends_with(".credentials.json") {
            continue;
        }
        let Ok(body) = std::fs::read_to_string(host) else {
            continue;
        };
        if let Some(exp) = parse_claude_expiry_ms(&body) {
            if exp < now_ms {
                return Err(format!(
                    "claude OAuth creds at {host} are EXPIRED (expiresAt={exp} < now={now_ms}). Re-login \
                     (`claude`) and refresh the a2a-creds copy, then retry. This is the usual cause of the \
                     opaque agent crash 'session/prompt failed: transport error or kill-switch escalation'."
                ));
            }
        }
    }
    Ok(())
}

/// `now_ms` for the preflight (epoch millis; 0 if the clock is before the epoch, which never happens).
pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
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

/// True iff the worktree has staged or unstaged changes (untracked-aware via --porcelain).
pub fn is_worktree_dirty(clone: &Path) -> Result<bool, String> {
    let out =
        run_git(Some(clone), &["status", "--porcelain"]).map_err(|e| format!("git status: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "git status: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(!String::from_utf8_lossy(&out.stdout).trim().is_empty())
}

/// The subject (first line) of HEAD's commit message, recomputed for hand-off after resume.
pub fn commit_subject(clone: &Path) -> Result<String, String> {
    let out =
        run_git(Some(clone), &["log", "-1", "--format=%s"]).map_err(|e| format!("git log: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "git log: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
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

/// Run a prepared commit argv with `git -C <clone>`: the bounded index-lock retry, the stale-`.git/index.
/// lock` clear after retries, and reading the new sha. Shared by `host_commit` (fresh) + `host_amend_commit`.
fn host_commit_argv_run(clone: &Path, argv: &[String]) -> Result<String, String> {
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

/// Deterministically commit the AGENT-STAGED index with the bot identity + the full hook/sign/ownership
/// pins. Bounded retry on an index-lock error (the per-turn container that held it is being reaped);
/// clears a stale `.git/index.lock` only AFTER retries exhaust. Returns the new commit sha. Stages nothing.
pub fn host_commit(clone: &Path, msg: &str) -> Result<String, String> {
    host_commit_argv_run(clone, &commit_argv(&clone.to_string_lossy(), msg))
}

/// `commit --amend --no-edit` argv with the SAME pins as `commit_argv` (run with `git -C <clone>`). Folds
/// the freshly-staged fix into the single commit, KEEPING the stored message + parent — so the operator
/// hand-off (`cherry-pick -n FETCH_HEAD`) stays byte-identical across attempts.
pub fn commit_amend_argv(clone: &str) -> Vec<String> {
    let mut v = pin_prefix_argv(clone);
    v.extend([
        "-c".into(),
        format!("user.name={BOT_NAME}"),
        "-c".into(),
        format!("user.email={BOT_EMAIL}"),
        "commit".into(),
        "--no-verify".into(),
        "--amend".into(),
        "--no-edit".into(),
    ]);
    v
}

/// Amend the agent-staged fix into the single commit (keeps the original message + parent + bot identity).
pub fn host_amend_commit(clone: &Path) -> Result<String, String> {
    host_commit_argv_run(clone, &commit_amend_argv(&clone.to_string_lossy()))
}

/// Reset the working tree to the committed HEAD (discard unstaged tracked changes + untracked files) so
/// VERIFY tests EXACTLY the committed tree, not the agent's leftover scratch.
pub fn reset_worktree_to_head(clone: &Path) -> Result<(), String> {
    let sd = format!("safe.directory={}", clone.to_string_lossy());
    git_ok(Some(clone), &["-c", &sd, "reset", "--hard", "HEAD"])?;
    git_ok(Some(clone), &["-c", &sd, "clean", "-fdq"]).map(|_| ())
}

/// Restore OUR task branch to a trusted commit after a fix turn mutated HEAD (advanced OR switched branches).
/// `checkout -f <branch>` returns to our branch (robust to a switch; discards the rogue working tree), then
/// `reset --hard <sha>` moves the branch ref to the trusted tip — which is what the hand-off FETCHES. This is
/// the no-work-loss fix: a bare `reset --hard` on the agent's (possibly switched) HEAD would leave OUR branch
/// at the rogue tip.
pub fn restore_branch(clone: &Path, branch: &str, sha: &str) -> Result<(), String> {
    let sd = format!("safe.directory={}", clone.to_string_lossy());
    git_ok(Some(clone), &["-c", &sd, "checkout", "-q", "-f", branch])?;
    git_ok(Some(clone), &["-c", &sd, "reset", "--hard", sha]).map(|_| ())
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
    msg: (String, CommitSource),
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
    fn pin_prefix_is_identity_free_and_complete() {
        let p = pin_prefix_argv("/root/.a2a-implement/impl-1-ab");
        let joined = p.join(" ");
        assert_eq!(joined, "-c safe.directory=/root/.a2a-implement/impl-1-ab -c core.hooksPath=/dev/null -c commit.gpgsign=false");
        // identity-free: no user.name / user.email here
        assert!(!joined.contains("user.name"));
        assert!(!joined.contains("user.email"));
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
    fn impl_lsp_cache_name_is_per_repo_and_distinct_from_verify() {
        let a = crate::verify::cache_volume_name("a2a-impl-lsp-cache", "/clones/x");
        let b = crate::verify::cache_volume_name("a2a-impl-lsp-cache", "/clones/y");
        assert_ne!(a, b, "different repos must get different cache volumes");
        // distinct base from verify so the two caches never collide
        let v = crate::verify::cache_volume_name("a2a-verify-cache", "/clones/x");
        assert_ne!(a, v, "impl-lsp cache must not share verify's volume");
        assert!(a.starts_with("a2a-impl-lsp-cache-"));
    }

    #[test]
    fn warm_lsp_fetch_argv_uses_egress_offline_false_and_cache_mount() {
        // compose_warm_fetch(runtime, image, clone, cache, fetch_cmd, egress) -> (program, argv) for
        // `<runtime> run ... <image> cargo fetch --locked`. Runtime+image come from [verify] (podman parity).
        let p = bridge_core::profile::rust_profile();
        let binding = p.cache_binding(
            bridge_core::profile::CacheCtx::Fetch,
            "a2a-impl-lsp-cache-deadbeef",
            "",
        );
        let (program, argv) = compose_warm_fetch(
            "podman",
            "a2a-toolchain:latest",
            "/clones/x",
            &binding,
            &p.fetch_cmd,
            &WarmEgress {
                network: "a2a-verify-egress".into(),
                proxy: "http://a2a-verify-proxy:8888".into(),
            },
            false,
        );
        assert_eq!(
            program, "podman",
            "the runtime is honored (not hardcoded docker)"
        );
        let joined = argv.join(" ");
        assert!(joined.contains("--network a2a-verify-egress"), "{joined}");
        assert!(joined.contains("a2a-toolchain:latest"), "{joined}");
        assert!(
            joined.contains("a2a-impl-lsp-cache-deadbeef:/cargo"),
            "{joined}"
        );
        assert!(joined.contains("CARGO_HOME=/cargo"), "{joined}");
        assert!(joined.contains("cargo fetch --locked"), "{joined}");
        assert!(
            !joined.contains("auth.json"),
            "warm fetch must mount NO creds"
        );
    }

    #[test]
    fn compose_warm_fetch_via_binding_is_byte_for_byte() {
        use bridge_core::profile::{rust_profile, CacheCtx};
        let p = rust_profile();
        let binding = p.cache_binding(CacheCtx::Fetch, "warmvol", "");
        let e = WarmEgress {
            network: "net".into(),
            proxy: "http://p:8888".into(),
        };
        let (prog, argv) = compose_warm_fetch(
            "docker",
            "img:latest",
            "/clone",
            &binding,
            &p.fetch_cmd,
            &e,
            false,
        );
        assert_eq!(prog, "docker");
        // EXACT byte-for-byte: pin the WHOLE argv (order + content) so a positional drift can't slip
        // through (the env -e's land AFTER the proxy -e's and BEFORE the clone -v, then the cache -v).
        let expected: Vec<String> = [
            "run",
            "--rm",
            "--network",
            "net",
            "-e",
            "HTTPS_PROXY=http://p:8888",
            "-e",
            "HTTP_PROXY=http://p:8888",
            "-e",
            "CARGO_HOME=/cargo",
            "-v",
            "/clone:/work",
            "-v",
            "warmvol:/cargo",
            "--workdir",
            "/work",
            "--entrypoint",
            "bash",
            "img:latest",
            "-c",
            "cargo fetch --locked",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        assert_eq!(argv, expected);
    }

    #[test]
    fn compose_warm_fetch_read_only_mounts_work_ro() {
        let p = bridge_core::profile::rust_profile();
        let e = WarmEgress {
            network: "n".into(),
            proxy: "http://p:8888".into(),
        };
        let binding = p.cache_binding(bridge_core::profile::CacheCtx::Fetch, "vol", "");
        let (_prog, argv) = compose_warm_fetch(
            "docker",
            "img:latest",
            "/clone",
            &binding,
            &p.fetch_cmd,
            &e,
            true,
        );
        assert!(
            argv.iter().any(|a| a == "/clone:/work:ro"),
            "expected :ro work mount, got {argv:?}"
        );
        assert!(
            !argv.iter().any(|a| a == "/clone:/work"),
            "must not also mount rw"
        );
    }

    #[test]
    fn commit_message_file_else_fallback() {
        assert_eq!(
            commit_message(None, Some("  Fix the widget\n\ndetails\n".into()), "", "task ignored"),
            ("Fix the widget\n\ndetails".to_string(), CommitSource::File)
        );
        assert_eq!(
            commit_message(None, None, "", "Add a FOO marker file to the repo root\nmore"),
            (
                "implement: Add a FOO marker file to the repo root".to_string(),
                CommitSource::Derived
            )
        );
        assert_eq!(
            commit_message(None, Some("   \n  ".into()), "", "Tidy up").1,
            CommitSource::Derived
        );
        let long = "x".repeat(500);
        let (m, source) = commit_message(None, None, "", &long);
        assert!(
            source == CommitSource::Derived
                && m.starts_with("implement: ")
                && m.len() <= "implement: ".len() + 120
        );
    }

    #[test]
    fn commit_precedence_and_comment_only_falls_back_to_title() {
        assert_eq!(
            commit_message(Some("feat: x".into()), None, "Add foo", "task body").0,
            "feat: x"
        );
        assert_eq!(
            commit_message(
                Some("<!-- OPTIONAL -->".into()),
                None,
                "Add foo endpoint",
                "task body"
            )
            .0,
            "Add foo endpoint"
        );
        assert_eq!(
            commit_message(None, None, "Add foo endpoint", "task body").0,
            "Add foo endpoint"
        );
        assert_eq!(
            commit_message(None, Some("from file".into()), "T", "task").0,
            "from file"
        );
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
        let msg = ("m".to_string(), CommitSource::File);
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
    fn commit_amend_argv_pins_and_amends_no_edit() {
        let a = commit_amend_argv("/c");
        let joined = a.join(" ");
        assert!(joined.contains("-c safe.directory=/c"));
        assert!(joined.contains("-c core.hooksPath=/dev/null"));
        assert!(joined.contains("-c user.name=a2a-implement"));
        let ci = a.iter().position(|x| x == "commit").unwrap();
        assert_eq!(&a[ci..], &["commit", "--no-verify", "--amend", "--no-edit"]);
    }

    #[test]
    fn host_amend_folds_into_one_commit_keeping_parent_and_message() {
        let (_g, p) = temp_repo();
        let base = head_sha(&p).unwrap();
        run_git(Some(&p), &["checkout", "-q", "-b", "implement/x"]).unwrap();
        std::fs::write(p.join("A.md"), "a\n").unwrap();
        run_git(Some(&p), &["add", "A.md"]).unwrap();
        let sha1 = host_commit(&p, "feat: the change").unwrap();
        std::fs::write(p.join("B.md"), "b\n").unwrap();
        run_git(Some(&p), &["add", "B.md"]).unwrap();
        let sha2 = host_amend_commit(&p).unwrap();
        assert_ne!(sha1, sha2);
        let count = run_git(Some(&p), &["rev-list", "--count", &format!("{base}..HEAD")]).unwrap();
        assert_eq!(String::from_utf8_lossy(&count.stdout).trim(), "1");
        let parent = run_git(Some(&p), &["rev-parse", "HEAD^"]).unwrap();
        assert_eq!(String::from_utf8_lossy(&parent.stdout).trim(), base);
        let subj = run_git(Some(&p), &["log", "-1", "--format=%s"]).unwrap();
        assert_eq!(
            String::from_utf8_lossy(&subj.stdout).trim(),
            "feat: the change"
        );
        assert!(p.join("A.md").exists() && p.join("B.md").exists());
        let an = run_git(Some(&p), &["log", "-1", "--format=%an"]).unwrap();
        assert_eq!(String::from_utf8_lossy(&an.stdout).trim(), "a2a-implement");
    }

    #[test]
    fn reset_worktree_to_head_discards_unstaged_and_untracked() {
        let (_g, p) = temp_repo();
        run_git(Some(&p), &["checkout", "-q", "-b", "implement/x"]).unwrap();
        std::fs::write(p.join("A.md"), "a\n").unwrap();
        run_git(Some(&p), &["add", "A.md"]).unwrap();
        host_commit(&p, "feat").unwrap();
        std::fs::write(p.join("A.md"), "MUTATED\n").unwrap();
        std::fs::write(p.join("scratch.tmp"), "junk\n").unwrap();
        assert_ne!(stage_state(&p).unwrap(), StageState::Clean);
        reset_worktree_to_head(&p).unwrap();
        assert_eq!(stage_state(&p).unwrap(), StageState::Clean);
        assert_eq!(std::fs::read_to_string(p.join("A.md")).unwrap(), "a\n");
        assert!(!p.join("scratch.tmp").exists());
    }

    #[test]
    fn restore_branch_recovers_after_head_advance() {
        let (_g, p) = temp_repo();
        run_git(Some(&p), &["checkout", "-q", "-b", "implement/x"]).unwrap();
        std::fs::write(p.join("A.md"), "a\n").unwrap();
        run_git(Some(&p), &["add", "A.md"]).unwrap();
        let good = host_commit(&p, "feat").unwrap();
        std::fs::write(p.join("rogue.md"), "r\n").unwrap();
        run_git(Some(&p), &["add", "rogue.md"]).unwrap();
        run_git(Some(&p), &["commit", "-q", "-m", "rogue"]).unwrap();
        restore_branch(&p, "implement/x", &good).unwrap();
        assert_eq!(head_sha(&p).unwrap(), good);
        assert_eq!(current_branch(&p).unwrap(), "implement/x");
        assert!(p.join("A.md").exists() && !p.join("rogue.md").exists());
    }

    #[test]
    fn restore_branch_recovers_after_branch_switch() {
        // the deeper hole: a bare `reset --hard` would reset the WRONG branch. restore_branch must force
        // OUR branch (the one the hand-off fetches) back to the trusted tip.
        let (_g, p) = temp_repo();
        run_git(Some(&p), &["checkout", "-q", "-b", "implement/x"]).unwrap();
        std::fs::write(p.join("A.md"), "a\n").unwrap();
        run_git(Some(&p), &["add", "A.md"]).unwrap();
        let good = host_commit(&p, "feat").unwrap();
        run_git(Some(&p), &["checkout", "-q", "-b", "rogue-branch"]).unwrap();
        std::fs::write(p.join("rogue.md"), "r\n").unwrap();
        run_git(Some(&p), &["add", "rogue.md"]).unwrap();
        run_git(Some(&p), &["commit", "-q", "-m", "rogue"]).unwrap();
        restore_branch(&p, "implement/x", &good).unwrap();
        assert_eq!(current_branch(&p).unwrap(), "implement/x");
        let tip = run_git(Some(&p), &["rev-parse", "implement/x"]).unwrap();
        assert_eq!(String::from_utf8_lossy(&tip.stdout).trim(), good);
        assert!(p.join("A.md").exists() && !p.join("rogue.md").exists());
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

    #[test]
    fn parse_claude_expiry_wrapper_flat_and_missing() {
        assert_eq!(
            parse_claude_expiry_ms(r#"{"claudeAiOauth":{"expiresAt":123}}"#),
            Some(123)
        );
        assert_eq!(parse_claude_expiry_ms(r#"{"expires_at":456}"#), Some(456));
        assert_eq!(parse_claude_expiry_ms(r#"{"other":1}"#), None);
        assert_eq!(parse_claude_expiry_ms("not json"), None);
    }

    #[test]
    fn claude_cred_preflight_errors_only_when_expired() {
        // non-claude agent: never blocks
        assert!(claude_cred_preflight(
            Some("codex-acp"),
            &["/x/.credentials.json:/c".into()],
            1000
        )
        .is_ok());
        // claude but no .credentials.json mount: ok
        assert!(
            claude_cred_preflight(Some("claude-agent-acp"), &["/x/foo:/c".into()], 1000).is_ok()
        );
        let dir = std::env::temp_dir();
        let pid = std::process::id();
        let exp = dir.join(format!("a2a-cred-exp-{pid}.credentials.json"));
        std::fs::write(&exp, r#"{"claudeAiOauth":{"expiresAt":500}}"#).unwrap();
        let r = claude_cred_preflight(
            Some("claude-agent-acp"),
            &[format!("{}:/root/.claude/.credentials.json", exp.display())],
            1000,
        );
        assert!(r.is_err(), "expired (500 < 1000) must error");
        assert!(r.unwrap_err().contains("EXPIRED"));
        let ok = dir.join(format!("a2a-cred-ok-{pid}.credentials.json"));
        std::fs::write(&ok, r#"{"claudeAiOauth":{"expiresAt":9999}}"#).unwrap();
        assert!(claude_cred_preflight(
            Some("claude-agent-acp"),
            &[format!("{}:/root/.claude/.credentials.json", ok.display())],
            1000
        )
        .is_ok());
        let _ = std::fs::remove_file(&exp);
        let _ = std::fs::remove_file(&ok);
    }
}
