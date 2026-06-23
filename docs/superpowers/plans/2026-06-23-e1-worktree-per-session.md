# E1 — Worktree-per-Session Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give each warm session its own `git worktree` off a target repo so concurrent write-capable agents don't clobber each other's working tree — via a `WorktreeBackend` decorator over the host `AcpBackend`, reusing the `session_cwd` seam.

**Architecture:** A new `crates/bridge-worktree` crate holds a `WorktreeBackend` (implements `AgentBackend`, wraps an inner `Arc<dyn AgentBackend>`) + a `WorktreeProvider` trait + a `HostGitWorktree` git-shell-out impl (mirrors `implement.rs`'s `run_git`). At `configure_session` it materializes a per-session detached worktree under a gated root **outside any repo** and substitutes `spec.cwd`; teardown delegates-then-removes. Opt-in via `[worktrees]`; wired in `make_spawn_fn`'s Acp arm. Host-only, isolation-only, per-request-cwd only.

**Tech Stack:** Rust (new `bridge-worktree` crate; touches bridge-workflow, bin/a2a-bridge config+main), tokio, async-trait, std::process::Command (git), serde.

**Binding spec:** `docs/superpowers/specs/2026-06-23-e1-worktree-per-session.md` — the `## v2` section (SF-1..6 + SR-FIX-1..12). Base = `main` `165e7e2`. Branch `feat/e1-worktree-per-session`.

---

## Reference facts (verified — do not re-derive)
- `AgentBackend` trait (`crates/bridge-core/src/ports.rs:43-98`) — **10 methods** the decorator must delegate:
  `prompt` (:44), `prompt_observed` (:49, defaults to `prompt`), `cancel` (:57), `configure_turn` (:61, default no-op),
  `configure_session` (:64, default Ok), `forget_session` (:73, default no-op), `release_session` (:77, default
  `forget_session`), `reconcile_config` (:83, default NotAdvertised), `capabilities` (:91, default empty), `retire`
  (:95, default Ok).
- `SessionCwd` (`crates/bridge-core/src/session_cwd.rs`): `parse(&str)->Result` (:12, absolute+normalized+NUL-free,
  NO fs access), `as_str()`, `is_under(&self, root: &SessionCwd)->bool` (:48, lexical component-wise).
  `SessionSpec { config: EffectiveConfig, cwd: Option<SessionCwd> }` (`domain.rs:181-192`).
- The cwd is consumed at the warm mint: `session_manager.rs:559-576` (fingerprint from ORIGINAL cwd at `:559-563`
  BEFORE `configure_session` at `:576` — so substituting inside the decorator never leaks into the fingerprint).
  Cold path: executor calls `configure_session` then `forget_session` per node, **swallowing the configure error**
  (`let _ = ...configure_session` — SR-FIX-1).
- `make_spawn_fn` Acp arm (`bin/a2a-bridge/src/main.rs:513-528`) builds `AcpBackend` → `Arc::new(be) as Arc<dyn
  AgentBackend>` — the wrap site. `RegistryConfig` (`config.rs:115-153`) — add `[worktrees]` beside `[implement]`.
- B2b git idioms to mirror: `bin/a2a-bridge/src/implement.rs` — `run_git(cwd, argv)` raw `Command::new("git")`
  (:264-270), `assert_dest_outside_worktree` (:441-460). Dead-owner liveness sweep `main.rs:381`.
- Spike-confirmed: `git worktree add --detach <path-outside-repo> HEAD` isolates concurrent edits, source stays
  clean, `worktree remove --force` + `prune` clean up; a worktree INSIDE the source dirties its `git status`.

## File Structure
| File | Responsibility | Tasks |
|---|---|---|
| `crates/bridge-worktree/Cargo.toml` + `src/lib.rs` | new crate root | T1 |
| `crates/bridge-worktree/src/provider.rs` | `WorktreeProvider` trait + pure argv builders + path/gate fns | T1, T4 |
| `crates/bridge-worktree/src/host_git.rs` | `HostGitWorktree` (real git) + sidecar + real-repo smoke | T2, T4 |
| `crates/bridge-worktree/src/backend.rs` | `WorktreeBackend` decorator (10-method delegate + map) | T3 |
| `crates/bridge-worktree/src/sweep.rs` | boot-sweep (dead-owner orphan reap) | T7 |
| `bin/a2a-bridge/src/config.rs` | `[worktrees]` TOML + preflight (root-outside-repo) | T5 |
| `bin/a2a-bridge/src/main.rs` | SpawnFn wrap + boot-sweep call + run-workflow end-guard | T5, T7 |
| `crates/bridge-workflow/src/executor.rs` | honor `configure_session` errors (SR-FIX-1) | T6 |

Bottom-up order keeps the tree green per task: crate+provider (T1) → real git+smoke (T2) → decorator (T3) → path/gate/sidecar (T4) → config+wiring (T5) → executor configure-error (T6) → boot-sweep+end-guard (T7) → gate (T8).

---

## Task 1: `bridge-worktree` crate + `WorktreeProvider` trait + pure argv builders

**Files:**
- Create: `crates/bridge-worktree/Cargo.toml`, `crates/bridge-worktree/src/lib.rs`, `crates/bridge-worktree/src/provider.rs`
- Modify: root `Cargo.toml` (workspace members)

- [ ] **Step 1: Write the failing test** — in `provider.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn argv_builders_emit_expected_git_invocations() {
        assert_eq!(add_argv("/repo", "/wt/x", "HEAD"),
            vec!["-C", "/repo", "worktree", "add", "--detach", "/wt/x", "HEAD"]);
        assert_eq!(remove_argv("/repo", "/wt/x"),
            vec!["-C", "/repo", "worktree", "remove", "--force", "/wt/x"]);
        assert_eq!(is_repo_argv("/some/dir"),
            vec!["-C", "/some/dir", "rev-parse", "--is-inside-work-tree"]);
        assert_eq!(prune_argv("/repo"), vec!["-C", "/repo", "worktree", "prune"]);
    }
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p bridge-worktree argv_builders`
Expected: FAIL — crate/fns don't exist.

- [ ] **Step 3: Create the crate**

`crates/bridge-worktree/Cargo.toml`:
```toml
[package]
name = "bridge-worktree"
version = "0.1.0"
edition = "2021"

[dependencies]
bridge-core = { path = "../bridge-core" }
async-trait = "0.1"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
tracing = "0.1"
```

Add `"crates/bridge-worktree"` to the root `Cargo.toml` `[workspace] members`.

`src/lib.rs`:
```rust
//! Worktree-per-session isolation: a WorktreeBackend decorator + a host-git WorktreeProvider.
pub mod backend;
pub mod host_git;
pub mod provider;
pub mod sweep;
```
(Stub `backend.rs`/`host_git.rs`/`sweep.rs` as empty `// E1 Tx` files; later tasks fill them.)

`src/provider.rs` — the trait + pure argv builders:
```rust
use bridge_core::error::BridgeError;

/// Materializes/removes per-session git worktrees. Host impl shells out to git; tests use a fake.
#[async_trait::async_trait]
pub trait WorktreeProvider: Send + Sync {
    /// Create a detached worktree of `repo` at `worktree_path` (base ref = repo HEAD).
    async fn add(&self, repo: &str, worktree_path: &str) -> Result<(), BridgeError>;
    /// Remove the worktree + prune the source's dangling registration. Best-effort.
    async fn remove(&self, repo: &str, worktree_path: &str) -> Result<(), BridgeError>;
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
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p bridge-worktree && cargo build --workspace`
Expected: PASS / clean build.

- [ ] **Step 5: Commit**

```bash
git add crates/bridge-worktree/Cargo.toml crates/bridge-worktree/src/lib.rs crates/bridge-worktree/src/provider.rs Cargo.toml Cargo.lock
git commit -m "feat(worktree): T1 — bridge-worktree crate + WorktreeProvider trait + argv builders"
```

---

## Task 2: `HostGitWorktree` (real git) + worktree-isolation smoke (SR-FIX-11)

**Files:**
- Modify: `crates/bridge-worktree/src/host_git.rs`, `provider.rs` (`run_git` helper)

- [ ] **Step 1: Write the failing test** — in `host_git.rs`, a REAL-git integration test (the in-plan spike proof):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::WorktreeProvider;

    fn git(dir: &std::path::Path, args: &[&str]) {
        let st = std::process::Command::new("git").arg("-C").arg(dir).args(args).status().unwrap();
        assert!(st.success(), "git {args:?} failed");
    }

    #[tokio::test]
    async fn worktree_add_isolates_and_remove_cleans_up() {
        let tmp = std::env::temp_dir().join(format!("e1-wt-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let src = tmp.join("src");
        std::fs::create_dir_all(&src).unwrap();
        git(&src, &["init", "-q"]);
        git(&src, &["config", "user.email", "a@b.c"]);
        git(&src, &["config", "user.name", "x"]);
        std::fs::write(src.join("file.txt"), "base\n").unwrap();
        git(&src, &["add", "-A"]);
        git(&src, &["commit", "-q", "-m", "init"]);

        let p = HostGitWorktree::new();
        let src_s = src.to_str().unwrap();
        assert!(p.is_git_repo(src_s).await);
        assert!(!p.is_git_repo(tmp.to_str().unwrap()).await, "non-repo dir → false");

        let wt = tmp.join("wt1");
        let wt_s = wt.to_str().unwrap();
        p.add(src_s, wt_s).await.unwrap();
        // isolation: edit in the worktree; source stays clean
        std::fs::write(wt.join("only-in-wt.txt"), "x").unwrap();
        let status = std::process::Command::new("git").arg("-C").arg(&src).args(["status", "--porcelain"]).output().unwrap();
        assert!(status.stdout.is_empty(), "source working tree must stay clean");
        assert!(!src.join("only-in-wt.txt").exists(), "worktree edit must NOT appear in the source");

        p.remove(src_s, wt_s).await.unwrap();
        let list = std::process::Command::new("git").arg("-C").arg(&src).args(["worktree", "list"]).output().unwrap();
        assert_eq!(String::from_utf8_lossy(&list.stdout).lines().count(), 1, "only the source remains");
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p bridge-worktree worktree_add_isolates`
Expected: FAIL — `HostGitWorktree` undefined.

- [ ] **Step 3: Implement** — `host_git.rs`:

```rust
use crate::provider::{add_argv, is_repo_argv, prune_argv, remove_argv, WorktreeProvider};
use bridge_core::error::BridgeError;

pub struct HostGitWorktree;
impl HostGitWorktree {
    pub fn new() -> Self { Self }
}
impl Default for HostGitWorktree { fn default() -> Self { Self::new() } }

fn run_git(argv: &[&str]) -> Result<std::process::Output, BridgeError> {
    std::process::Command::new("git")
        .args(argv)
        .output()
        .map_err(|e| BridgeError::ConfigInvalid { reason: format!("git spawn: {e}") })
}

#[async_trait::async_trait]
impl WorktreeProvider for HostGitWorktree {
    async fn add(&self, repo: &str, wt: &str) -> Result<(), BridgeError> {
        let out = run_git(&add_argv(repo, wt, "HEAD"))?;
        if !out.status.success() {
            return Err(BridgeError::ConfigInvalid {
                reason: format!("worktree add failed: {}", String::from_utf8_lossy(&out.stderr).trim()),
            });
        }
        Ok(())
    }
    async fn remove(&self, repo: &str, wt: &str) -> Result<(), BridgeError> {
        // best-effort remove, then prune any dangling registration in the source
        let _ = run_git(&remove_argv(repo, wt));
        let _ = run_git(&prune_argv(repo));
        Ok(())
    }
    async fn is_git_repo(&self, path: &str) -> bool {
        matches!(run_git(&is_repo_argv(path)), Ok(o) if o.status.success()
            && String::from_utf8_lossy(&o.stdout).trim() == "true")
    }
}
```

- [ ] **Step 4: Run** (controller runs in host env — needs real git)

Run: `cargo test -p bridge-worktree`
Expected: PASS (argv + the isolation smoke).

- [ ] **Step 5: Commit**

```bash
git add crates/bridge-worktree/src/host_git.rs crates/bridge-worktree/src/provider.rs
git commit -m "feat(worktree): T2 — HostGitWorktree (real git) + worktree-isolation smoke"
```

---

## Task 3: `WorktreeBackend` decorator (full-trait delegate + idempotent map + delegate-then-remove)

**Files:**
- Modify: `crates/bridge-worktree/src/backend.rs`

**Design (SR-FIX-2/3/4):** wraps `inner: Arc<dyn AgentBackend>` + `provider: Arc<dyn WorktreeProvider>` + a config (root, owner, run). `configure_session`: if `spec.cwd = Some(repo)` AND `provider.is_git_repo(repo)` → compute the worktree path (T4's pure fn), `provider.add`, store `SessionId → WtEntry { source, worktree }`, delegate with the substituted cwd; same-source repeat = idempotent (reuse); different-source for a live SessionId = `InvalidStateTransition`; `None`/non-repo = pass-through. `release_session`/`forget_session`: **delegate to inner FIRST, then `provider.remove`**, drop the map entry. All other 8 methods delegate (reconcile substitutes the mapped worktree cwd).

- [ ] **Step 1: Write the failing test** — `backend.rs` with a recording fake inner + fake provider:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use bridge_core::domain::{EffectiveConfig, SessionSpec};
    use bridge_core::ids::SessionId;
    use bridge_core::ports::{AgentBackend, BackendStream};
    use bridge_core::SessionCwd;
    use std::sync::{Arc, Mutex};

    #[derive(Default)]
    struct Rec { configured_cwd: Mutex<Vec<Option<String>>>, order: Mutex<Vec<String>> }
    struct FakeInner { rec: Arc<Rec> }
    #[async_trait::async_trait]
    impl AgentBackend for FakeInner {
        async fn prompt(&self, _s: &SessionId, _p: Vec<bridge_core::domain::Part>) -> Result<BackendStream, bridge_core::error::BridgeError> {
            Ok(Box::pin(tokio_stream::iter(vec![])))
        }
        async fn cancel(&self, _s: &SessionId) -> Result<(), bridge_core::error::BridgeError> { Ok(()) }
        async fn configure_session(&self, _s: &SessionId, spec: &SessionSpec) -> Result<(), bridge_core::error::BridgeError> {
            self.rec.configured_cwd.lock().unwrap().push(spec.cwd.as_ref().map(|c| c.as_str().to_string()));
            Ok(())
        }
        async fn release_session(&self, _s: &SessionId) { self.rec.order.lock().unwrap().push("inner_release".into()); }
    }
    struct FakeProv { rec: Arc<Rec> }
    #[async_trait::async_trait]
    impl crate::provider::WorktreeProvider for FakeProv {
        async fn add(&self, _r: &str, _w: &str) -> Result<(), bridge_core::error::BridgeError> { Ok(()) }
        async fn remove(&self, _r: &str, _w: &str) -> Result<(), bridge_core::error::BridgeError> {
            self.rec.order.lock().unwrap().push("wt_remove".into()); Ok(())
        }
        async fn is_git_repo(&self, _p: &str) -> bool { true }
    }
    fn cfg() -> WorktreeConfig { WorktreeConfig { root: "/wtroot".into(), owner: "o".into(), run: "r".into() } }
    fn spec(cwd: Option<&str>) -> SessionSpec {
        SessionSpec { config: EffectiveConfig::default(), cwd: cwd.map(|c| SessionCwd::parse(c).unwrap()) }
    }

    #[tokio::test]
    async fn configure_substitutes_worktree_cwd_then_release_delegates_then_removes() {
        let rec = Arc::new(Rec::default());
        let be = WorktreeBackend::new(Arc::new(FakeInner { rec: rec.clone() }), Arc::new(FakeProv { rec: rec.clone() }), cfg(), Some(SessionCwd::parse("/repos").unwrap()));
        let sid = SessionId::parse("ctx-c1-g0").unwrap();
        be.configure_session(&sid, &spec(Some("/repos/app"))).await.unwrap();
        // inner saw a SUBSTITUTED cwd (under /wtroot), not /repos/app
        let seen = rec.configured_cwd.lock().unwrap()[0].clone().unwrap();
        assert!(seen.starts_with("/wtroot/"), "inner cwd substituted to the worktree: {seen}");
        be.release_session(&sid).await;
        assert_eq!(rec.order.lock().unwrap().as_slice(), ["inner_release", "wt_remove"], "delegate-then-remove");
    }

    #[tokio::test]
    async fn same_source_idempotent_diff_source_rejected_and_passthrough() {
        let rec = Arc::new(Rec::default());
        let be = WorktreeBackend::new(Arc::new(FakeInner { rec: rec.clone() }), Arc::new(FakeProv { rec: rec.clone() }), cfg(), Some(SessionCwd::parse("/repos").unwrap()));
        let sid = SessionId::parse("ctx-c1-g0").unwrap();
        be.configure_session(&sid, &spec(Some("/repos/app"))).await.unwrap();
        be.configure_session(&sid, &spec(Some("/repos/app"))).await.unwrap(); // idempotent
        assert!(be.configure_session(&sid, &spec(Some("/repos/other"))).await.is_err(), "different source rejected");
        // None cwd → pass-through (inner sees None)
        let sid2 = SessionId::parse("ctx-c2-g0").unwrap();
        be.configure_session(&sid2, &spec(None)).await.unwrap();
        assert!(rec.configured_cwd.lock().unwrap().last().unwrap().is_none(), "None cwd passes through");
    }
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p bridge-worktree backend::`
Expected: FAIL — `WorktreeBackend`/`WorktreeConfig` undefined.

- [ ] **Step 3: Implement** — `backend.rs`:

```rust
use crate::provider::WorktreeProvider;
use crate::provider_path::worktree_path; // T4
use bridge_core::domain::SessionSpec;
use bridge_core::error::BridgeError;
use bridge_core::ids::SessionId;
use bridge_core::ports::*;
use bridge_core::SessionCwd;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;

#[derive(Clone)]
pub struct WorktreeConfig { pub root: String, pub owner: String, pub run: String }
struct WtEntry { source: String, worktree: String }

pub struct WorktreeBackend {
    inner: Arc<dyn AgentBackend>,
    provider: Arc<dyn WorktreeProvider>,
    cfg: WorktreeConfig,
    allowed_root: Option<SessionCwd>,
    map: Mutex<HashMap<String, WtEntry>>,
}

impl WorktreeBackend {
    pub fn new(inner: Arc<dyn AgentBackend>, provider: Arc<dyn WorktreeProvider>, cfg: WorktreeConfig, allowed_root: Option<SessionCwd>) -> Self {
        Self { inner, provider, cfg, allowed_root, map: Mutex::new(HashMap::new()) }
    }
}

#[async_trait::async_trait]
impl AgentBackend for WorktreeBackend {
    async fn configure_session(&self, session: &SessionId, spec: &SessionSpec) -> Result<(), BridgeError> {
        let repo = match &spec.cwd {
            Some(c) => c.as_str().to_string(),
            None => return self.inner.configure_session(session, spec).await, // pass-through
        };
        // idempotency / mismatch
        {
            let map = self.map.lock().await;
            if let Some(e) = map.get(session.as_str()) {
                if e.source != repo {
                    return Err(BridgeError::InvalidStateTransition);
                }
                return Ok(()); // same source → already materialized
            }
        }
        if !self.provider.is_git_repo(&repo).await {
            return self.inner.configure_session(session, spec).await; // non-repo pass-through
        }
        // T4: self-gate (source under allowed_root) + path under root, canonicalized
        let wt = worktree_path(&self.cfg, &self.allowed_root, &repo, session.as_str())?;
        self.provider.add(&repo, &wt).await?; // failure → typed err, no map entry, no half-state
        let sub = SessionSpec { config: spec.config.clone(), cwd: Some(SessionCwd::parse(&wt)?) };
        if let Err(e) = self.inner.configure_session(session, &sub).await {
            let _ = self.provider.remove(&repo, &wt).await; // unwind
            return Err(e);
        }
        self.map.lock().await.insert(session.as_str().to_string(), WtEntry { source: repo, worktree: wt });
        Ok(())
    }

    async fn release_session(&self, session: &SessionId) {
        self.inner.release_session(session).await; // delegate FIRST (cancels the session)
        if let Some(e) = self.map.lock().await.remove(session.as_str()) {
            let _ = self.provider.remove(&e.source, &e.worktree).await;
        }
    }
    async fn forget_session(&self, session: &SessionId) {
        self.inner.forget_session(session).await;
        if let Some(e) = self.map.lock().await.remove(session.as_str()) {
            let _ = self.provider.remove(&e.source, &e.worktree).await;
        }
    }

    // --- pure delegation for the rest ---
    async fn prompt(&self, s: &SessionId, p: Vec<bridge_core::domain::Part>) -> Result<BackendStream, BridgeError> { self.inner.prompt(s, p).await }
    async fn prompt_observed(&self, s: &SessionId, p: Vec<bridge_core::domain::Part>, sink: Arc<dyn RichEventSink>) -> Result<BackendStream, BridgeError> { self.inner.prompt_observed(s, p, sink).await }
    async fn cancel(&self, s: &SessionId) -> Result<(), BridgeError> { self.inner.cancel(s).await }
    async fn configure_turn(&self, s: &SessionId, m: bridge_core::permission::TurnMeta) { self.inner.configure_turn(s, m).await }
    async fn reconcile_config(&self, s: &SessionId, spec: &SessionSpec) -> Result<bridge_core::orch::ReconcileOutcome, BridgeError> {
        // substitute the mapped worktree cwd (not the original) so a live reconcile stays consistent
        let mapped = self.map.lock().await.get(s.as_str()).map(|e| e.worktree.clone());
        match mapped {
            Some(wt) => { let sub = SessionSpec { config: spec.config.clone(), cwd: Some(SessionCwd::parse(&wt)?) }; self.inner.reconcile_config(s, &sub).await }
            None => self.inner.reconcile_config(s, spec).await,
        }
    }
    fn capabilities(&self) -> bridge_core::orch::AgentSessionCaps { self.inner.capabilities() }
    async fn retire(&self) -> Result<(), BridgeError> { self.inner.retire().await }
}
```

(If `BridgeError::InvalidStateTransition` isn't the exact variant, use the existing cwd-immutability error the SessionManager returns — verify in `error.rs` and match it.)

- [ ] **Step 4: Run tests**

Run: `cargo test -p bridge-worktree`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/bridge-worktree/src/backend.rs
git commit -m "feat(worktree): T3 — WorktreeBackend decorator (substitute + delegate-then-remove + idempotent)"
```

---

## Task 4: worktree path + self-gate + canonicalize + sidecar metadata (SR-FIX-5/6/7)

**Files:**
- Create: `crates/bridge-worktree/src/provider_path.rs` (add to `lib.rs` mods)
- Modify: `host_git.rs` (write/read sidecar on add/remove)

**Design:** `worktree_path(cfg, allowed_root, repo, session_id) -> Result<String>`: canonicalize `repo`; enforce `repo` is under `allowed_root` (when set, `is_under`); the worktree path = `<cfg.root>/<owner>-<run>-<hash(session_id)>/`; return it. `host_git`'s `add` writes a sidecar `<root>/<id>.json { canonical_source, owner }` next to the worktree; `remove` deletes it.

- [ ] **Step 1: Write the failing test** — `provider_path.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::WorktreeConfig;
    use bridge_core::SessionCwd;
    fn cfg() -> WorktreeConfig { WorktreeConfig { root: "/wtroot".into(), owner: "ownr".into(), run: "run7".into() } }
    #[test]
    fn path_is_owner_run_hash_scoped_and_gated() {
        let root = Some(SessionCwd::parse("/repos").unwrap());
        let p = worktree_path(&cfg(), &root, "/repos/app", "ctx-c1-g0").unwrap();
        assert!(p.starts_with("/wtroot/ownr-run7-"), "owner+run+hash scoped: {p}");
        // a source OUTSIDE allowed_root is rejected
        assert!(worktree_path(&cfg(), &root, "/etc", "ctx-c2-g0").is_err(), "source outside allowed_root rejected");
        // same session → stable path (deterministic hash)
        assert_eq!(p, worktree_path(&cfg(), &root, "/repos/app", "ctx-c1-g0").unwrap());
    }
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p bridge-worktree worktree_path`
Expected: FAIL — `worktree_path` undefined.

- [ ] **Step 3: Implement** — `provider_path.rs`:

```rust
use crate::backend::WorktreeConfig;
use bridge_core::error::BridgeError;
use bridge_core::SessionCwd;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

pub fn worktree_path(cfg: &WorktreeConfig, allowed_root: &Option<SessionCwd>, repo: &str, session_id: &str) -> Result<String, BridgeError> {
    // self-gate: the source must be under allowed_root (lexical, like the upstream gate)
    let repo_cwd = SessionCwd::parse(repo)?;
    if let Some(root) = allowed_root {
        if !repo_cwd.is_under(root) {
            return Err(BridgeError::InvalidRequest { field: "worktree source outside allowed_cwd_root" });
        }
    }
    let mut h = DefaultHasher::new();
    session_id.hash(&mut h);
    let hash = format!("{:016x}", h.finish());
    Ok(format!("{}/{}-{}-{}", cfg.root.trim_end_matches('/'), cfg.owner, cfg.run, hash))
}
```

Add `pub mod provider_path;` to `lib.rs`. In `host_git.rs` `add`, after a successful `git worktree add`, write a sidecar `{worktree_path}.meta.json` = `{"source": <repo>, "owner": <from cfg, threaded>}`; in `remove`, delete it (best-effort). (Thread `owner` into the provider's `add`/`remove` signature, or write the sidecar from the backend — pick the cleaner seam; the test in T7 reads these sidecars.)

- [ ] **Step 4: Run tests**

Run: `cargo test -p bridge-worktree`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/bridge-worktree/src/provider_path.rs crates/bridge-worktree/src/lib.rs crates/bridge-worktree/src/host_git.rs
git commit -m "feat(worktree): T4 — worktree path + self-gate + sidecar metadata"
```

---

## Task 5: `[worktrees]` config + preflight (root-outside-repo) + SpawnFn wiring (SR-FIX-6/12)

**Files:**
- Modify: `bin/a2a-bridge/src/config.rs` (`WorktreesToml` + `RegistryConfig.worktrees` + preflight in `into_snapshot` or a validate fn)
- Modify: `bin/a2a-bridge/src/main.rs` (thread the config into `make_spawn_fn`; wrap the Acp arm; default root)

- [ ] **Step 1: Write the failing tests** — in `config.rs` tests:

```rust
#[test]
fn worktrees_config_parses_and_preflight_rejects_root_in_repo() {
    let toml = format!("{AGENTS_HEADER}\n[worktrees]\nenabled = true\nroot = \"/tmp/a2a-wt\"\n{SERVER_FOOTER}");
    let cfg: RegistryConfig = toml::from_str(&toml).unwrap();
    let w = cfg.worktrees.as_ref().unwrap();
    assert!(w.enabled);
    assert_eq!(w.root.as_deref(), Some("/tmp/a2a-wt"));
    // preflight: a root that resolves inside a git repo is rejected
    assert!(preflight_worktrees_root(std::path::Path::new(env!("CARGO_MANIFEST_DIR"))).is_err(),
        "a root inside the a2a-bridge repo must be rejected");
    assert!(preflight_worktrees_root(std::path::Path::new("/tmp")).is_ok());
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p a2a-bridge worktrees_config_parses`
Expected: FAIL — `worktrees` field / `preflight_worktrees_root` undefined.

- [ ] **Step 3: Implement**

`config.rs`: add to `RegistryConfig` (after `merge`): `#[serde(default)] pub worktrees: Option<WorktreesToml>,` and:
```rust
#[derive(Debug, serde::Deserialize)]
pub struct WorktreesToml {
    #[serde(default)] pub enabled: bool,
    #[serde(default)] pub root: Option<String>,
}
/// Reject a worktrees root that resolves INSIDE a git repo (a worktree there dirties the source).
pub fn preflight_worktrees_root(root: &std::path::Path) -> Result<(), ConfigError> {
    // walk to the nearest existing ancestor, `git rev-parse --is-inside-work-tree`
    let mut probe = root;
    while !probe.exists() { probe = probe.parent().ok_or_else(|| ConfigError::Registry("worktrees root has no existing ancestor".into()))?; }
    let out = std::process::Command::new("git").arg("-C").arg(probe).args(["rev-parse", "--is-inside-work-tree"]).output();
    if matches!(out, Ok(o) if o.status.success() && String::from_utf8_lossy(&o.stdout).trim() == "true") {
        return Err(ConfigError::Registry(format!("[worktrees] root {root:?} is inside a git repo; choose a root outside any repo")));
    }
    Ok(())
}
```

`main.rs`: thread the resolved worktree config (enabled + root, defaulting `root` to `~/.a2a-bridge/worktrees` and running `preflight_worktrees_root` at serve startup) into `make_spawn_fn` (a new param, like `permission_registry`). In the Acp arm (`:518-527`), after building `be`:
```rust
let inner = Arc::new(be) as Arc<dyn AgentBackend>;
match &worktree_cfg {
    Some(wc) if wc.enabled => {
        let prov = Arc::new(bridge_worktree::host_git::HostGitWorktree::new());
        Ok(Arc::new(bridge_worktree::backend::WorktreeBackend::new(inner, prov, wc.backend_cfg(entry.id.as_str()), allowed_cwd_root.clone())) as Arc<dyn AgentBackend>)
    }
    _ => Ok(inner),
}
```
Add `bridge-worktree` to `bin/a2a-bridge/Cargo.toml`. (Per-agent enable per D2: gate on the agent too if `[agents.worktree]` is added; for the minimal cut a global `[worktrees].enabled` is acceptable — confirm in plan-review.)

- [ ] **Step 4: Run tests**

Run: `cargo test -p a2a-bridge worktrees_config && cargo build --workspace`
Expected: PASS / clean build.

- [ ] **Step 5: Commit**

```bash
git add bin/a2a-bridge/src/config.rs bin/a2a-bridge/src/main.rs bin/a2a-bridge/Cargo.toml Cargo.lock
git commit -m "feat(worktree): T5 — [worktrees] config + root-outside-repo preflight + SpawnFn wiring"
```

---

## Task 6: cold executor honors `configure_session` errors (SR-FIX-1)

**Files:**
- Modify: `crates/bridge-workflow/src/executor.rs` (the cold-path `configure_session` call, ~`:285`)

**Design:** the cold path does `let _ = resolved.backend.configure_session(...)`. A worktree-add failure must FAIL the node (not prompt in the wrong cwd). Observe the error, forget the session, return the error marker.

- [ ] **Step 1: Write the failing test** — `executor.rs` tests, a backend whose `configure_session` errors:

```rust
#[tokio::test]
async fn cold_configure_error_fails_node_without_prompting() {
    struct CfgErrBackend { rec: Arc<Rec> }
    #[async_trait::async_trait]
    impl AgentBackend for CfgErrBackend {
        async fn prompt(&self, _s: &SessionId, _p: Vec<Part>) -> Result<BackendStream, BridgeError> {
            *self.rec.prompts.lock().unwrap() = 1; // must NOT happen
            Ok(Box::pin(tokio_stream::iter(vec![])))
        }
        async fn cancel(&self, _s: &SessionId) -> Result<(), BridgeError> { Ok(()) }
        async fn configure_session(&self, _s: &SessionId, _spec: &SessionSpec) -> Result<(), BridgeError> {
            Err(BridgeError::ConfigInvalid { reason: "worktree add failed".into() })
        }
        async fn forget_session(&self, _s: &SessionId) { *self.rec.forgets.lock().unwrap() += 1; }
    }
    // FakeRegistry handing out CfgErrBackend → run a one-node graph → NodeFinished{ok:false}, prompt never called.
    // (assert rec.prompts == 0, the node output is an error marker, ok=false)
}
```
(Build it with a small registry returning `CfgErrBackend`, mirroring `captures_node_usage_smoke`.)

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p bridge-workflow cold_configure_error_fails_node`
Expected: FAIL — today configure errors are swallowed, the node prompts.

- [ ] **Step 3: Implement** — at `executor.rs` ~`:285`, change `let _ = resolved.backend.configure_session(...)` to observe the error:

```rust
if let Err(e) = resolved.backend.configure_session(&session, &SessionSpec { config: eff, cwd: ctx.session_cwd.clone() }).await {
    resolved.backend.forget_session(&session).await;
    return (format!("[node {} failed: configure {:?}]", node.id.as_str(), e), false, None);
}
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p bridge-workflow`
Expected: PASS (new + all existing — confirm no existing test relied on swallowing).

- [ ] **Step 5: Commit**

```bash
git add crates/bridge-workflow/src/executor.rs
git commit -m "feat(workflow): T6 — cold executor fails the node on configure_session error (SR-FIX-1)"
```

---

## Task 7: boot-sweep (dead-owner orphan reap) + run-workflow end-guard (SR-FIX-7/8)

**Files:**
- Modify: `crates/bridge-worktree/src/sweep.rs` (the sweep), `bin/a2a-bridge/src/main.rs` (call at serve boot + a `run-workflow` end-guard)

**Design:** `sweep_orphans(root, is_owner_alive)`: scan `<root>/*.meta.json`; for each whose `owner` is NOT alive (per a liveness fn mirroring `main.rs:381`), `git worktree remove`+`prune` the recorded source + delete the worktree dir + sidecar. A LIVE owner's worktree is left untouched. The `run-workflow` end-guard removes the current run's worktrees synchronously on exit (mirror ContainerRw's `RunEndGuard`).

- [ ] **Step 1: Write the failing test** — `sweep.rs`, with a temp root + two sidecars (one dead owner, one live):

```rust
#[tokio::test]
async fn sweep_reaps_dead_owner_keeps_live() {
    // write <root>/dead.meta.json {owner:"dead"} + a worktree dir, and live.meta.json {owner:"live"}
    // call sweep_orphans(root, |o| o == "live"); assert the dead sidecar+dir are gone, the live ones remain.
}
```
(Use a fake `is_owner_alive` closure; the git remove on a non-worktree dir is best-effort, so the test asserts the sidecar + dir removal for dead and retention for live.)

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p bridge-worktree sweep_reaps_dead_owner`
Expected: FAIL — `sweep_orphans` undefined.

- [ ] **Step 3: Implement** `sweep_orphans` in `sweep.rs` (read sidecars, filter dead owners, `provider.remove` + delete dir/sidecar). Wire `sweep_orphans` at serve boot in `main.rs` (after the existing container/liveness sweep), and a synchronous end-guard for `run-workflow` (a drop guard that removes the run's worktrees).

- [ ] **Step 4: Run tests**

Run: `cargo test -p bridge-worktree && cargo build --workspace`
Expected: PASS / clean build.

- [ ] **Step 5: Commit**

```bash
git add crates/bridge-worktree/src/sweep.rs crates/bridge-worktree/src/lib.rs bin/a2a-bridge/src/main.rs
git commit -m "feat(worktree): T7 — boot-sweep dead-owner orphans + run-workflow end-guard"
```

---

## Task 8: git-shape edge tests + workspace gate (SR-FIX-11)

**Files:**
- Modify: `crates/bridge-worktree/src/host_git.rs` tests (unborn HEAD, non-repo)

- [ ] **Step 1: Write the test** — unborn HEAD (empty repo) → `add` returns a clean typed error (no panic):

```rust
#[tokio::test]
async fn unborn_head_add_errors_cleanly() {
    let tmp = std::env::temp_dir().join(format!("e1-unborn-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    let src = tmp.join("src"); std::fs::create_dir_all(&src).unwrap();
    std::process::Command::new("git").arg("-C").arg(&src).args(["init","-q"]).status().unwrap(); // no commit → unborn HEAD
    let p = HostGitWorktree::new();
    let r = p.add(src.to_str().unwrap(), tmp.join("wt").to_str().unwrap()).await;
    assert!(r.is_err(), "unborn HEAD → typed error, not a panic");
    let _ = std::fs::remove_dir_all(&tmp);
}
```

- [ ] **Step 2: Run** — `cargo test -p bridge-worktree unborn_head` → PASS (the real git `worktree add HEAD` fails on an unborn HEAD; `add` maps it to `Err`).

- [ ] **Step 3: Full workspace gate** (controller, clean host env):

Run: `cargo fmt --all && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace --all-targets`
Expected: clean fmt, no clippy warnings, all tests pass (watch the pre-existing server.rs `warm_streaming_records_usage…` flake → re-run if it trips).

- [ ] **Step 4: Commit**

```bash
git add crates/bridge-worktree/src/host_git.rs
git commit -m "test(worktree): T8 — unborn-HEAD clean error + workspace gate"
```

---

## After the tasks (process — not plan steps)
1. **Whole-branch dual-lens review** (codex xhigh read-only + Opus architecture) over the full diff vs `main`. Fold blockers/majors.
2. **Live-gate** (per spec v2 "Updated live-gate"): `[worktrees] enabled` + two concurrent contexts on one real repo via `serve` + `submit`/`run-workflow` with two write-capable agents → assert (1) each edit lands in its own worktree, source `git status` CLEAN; (2) `continue` reuses the worktree; (3) release → `worktree remove`, no dangling registration; (5) source clean throughout; (6) root-in-repo rejected at preflight; (7) unborn-HEAD node fails cleanly; (8) kill serve mid-session → boot-sweep reaps the orphan on restart, a live concurrent worktree NOT reaped. Reuse a codex-HIGH impl config + the spec-review scaffolding port pattern.
3. **Merge** `--no-ff` + push; update memory (`e1-worktree-shipped`) + the orchestration handoff.

**Staging discipline:** stage ONLY each task's files. Pre-existing untracked `examples/*.toml`/`prompts/*.md` + `M examples/a2a-bridge.slicing-analysis.toml` — NEVER fold them.

## Self-Review (against the spec)
**1. SF/SR coverage:** SF-1/2 decorator + provider (T1–T4); SF-3 path+gate (T4); SF-4 detached (T2 `add_argv --detach`); SF-5 config (T5); SF-6 sweep (T7). SR-FIX-1 (T6); SR-FIX-2 delegate-then-remove (T3); SR-FIX-3 full-trait delegate (T3); SR-FIX-4 idempotent map (T3); SR-FIX-5 self-gate+canonicalize (T4); SR-FIX-6 root-outside-repo preflight (T5); SR-FIX-7 owner/run/hash path + sidecar (T4) + dead-owner sweep (T7); SR-FIX-8 sweep + end-guard (T7); SR-FIX-9 per-request-cwd-only (T3 `None`/non-repo pass-through — static cwd never reaches as `spec.cwd`); SR-FIX-11 unborn-HEAD/non-repo (T2/T8); SR-FIX-12 hot-reload (documented in T5). ✅
**2. Placeholders:** each step has real test + impl code + exact commands. The two soft spots flagged inline (the exact `BridgeError` immutability variant in T3; the sidecar owner-threading seam in T4) for the implementer to resolve against the real `error.rs`. ✅
**3. Type consistency:** `WorktreeConfig { root, owner, run }`, `WorktreeProvider::{add,remove,is_git_repo}`, `worktree_path(cfg, allowed_root, repo, session_id)`, `WorktreeBackend::new(inner, provider, cfg, allowed_root)` consistent across T1/T3/T4/T5. ✅
**4. Open items for plan-review:** (a) the exact immutability error variant (T3); (b) per-agent vs global enable (D2); (c) canonicalization depth in `worktree_path` (T4 uses lexical `is_under` like upstream — confirm a real `canonicalize` isn't required given the source may not exist); (d) the sidecar owner-threading seam (provider signature vs backend-writes). Confirm in the dual plan-review.

---

## v2 — dual plan-review folded (codex xhigh needs-revision: 5 BLOCKER + 7 MAJOR + 1 MINOR; Opus lens) — BINDING

> Supersedes the task bodies above where it conflicts. The DECORATOR SEAM HOLDS (both lenses CONFIRM: 10-method
> trait, `capabilities()` sync, substitute-after-fingerprint correct via `session_manager.rs:559` before `:574`).
> The folds make the plan compile-green per task + faithful to the spec's lifecycle/safety rigor. Apply each in its
> named task.

### PR-FIX-1 (codex BLOCKER-1 + Opus-1) — T3→T4 forward reference (won't compile)
T3 imports `crate::provider_path::worktree_path` (created in T4). **Reorder: do the path+gate task BEFORE the
decorator.** New order: T1 crate/provider → **T2 path+gate+sidecar (was T4)** → **T3 HostGitWorktree+smoke (was T2)**
→ **T4 WorktreeBackend decorator (was T3)** → T5 → T6 → T7 → T8. (Or keep numbering but move `worktree_path` +
the gate into the task that precedes the decorator.) The decorator task may then `use` `worktree_path`.

### PR-FIX-2 (codex BLOCKER-2) — the new crate's deps are incomplete
T1 `Cargo.toml` omits `tokio`/`tokio-stream`/`futures` but the tests use `#[tokio::test]`, `tokio::sync::Mutex`,
`tokio_stream`. Add (prefer workspace deps): `tokio = { workspace = true }` (or `{version="1", features=["sync","macros","rt"]}`),
`tokio-stream = { workspace = true }`, and `futures` if a stream type is named. Verify against the root `Cargo.toml`
`[workspace.dependencies]`.

### PR-FIX-3 (codex BLOCKER-3 + Opus-3) — SR-FIX-5 requires real CANONICALIZE, not lexical `is_under`
`SessionCwd::is_under` is lexical only (`session_cwd.rs:48`). The decorator runs at `configure_session` when the
SOURCE repo EXISTS → it CAN and MUST lenient-canonicalize. Mirror ContainerRw's lenient canonicalizer
(`bridge-container/src/lib.rs:713`): canonicalize source + root + worktree, then the containment check on the
canonical paths. REQUIRE `allowed_cwd_root` to be set when `[worktrees].enabled` (else reject at preflight). Gate
BEFORE any `git` op.

### PR-FIX-4 (codex BLOCKER-4 + Opus-1) — full sidecar + LEASE-aware sweep (reuse `run_identity`)
The sidecar must carry `{ canonical_source, common_dir, worktree_path, owner, run_id, host, lease }` (not `{source,
owner}`). Liveness is HOST+LEASE based (`crates/bridge-core/src/run_identity.rs:91`), NOT an owner-string compare.
The boot-sweep must classify dead-vs-live via the SAME lease semantics as the container `recover_orphans`
(`main.rs:381`). Capture `common_dir` (`git -C <repo> rev-parse --git-common-dir`) so cleanup can
`git worktree prune` the source even when the worktree dir is gone. (T-path/sidecar + T-sweep tasks.)

### PR-FIX-5 (codex BLOCKER-5) — `retire()` must drain the worktree map (else leak)
Registry retirement calls backend `retire()` (`registry.rs:285/327`) and requires idempotent/concurrent-safe
retire (`registry.rs:263`). The decorator's `retire()` must NOT just delegate — it must idempotently DRAIN the map,
delegate `inner.retire()`, then `provider.remove` every mapped worktree. Add a test: retire with N mapped sessions
→ all worktrees removed, map empty, second retire is a no-op.

### PR-FIX-6 (codex MAJOR-6) — same-source re-configure must RE-DELEGATE to inner (not early-return Ok)
Inbound follow-ups re-call `configure_session` (`server.rs:443`); `AcpBackend::configure_session` is insert-or-
replace (`acp_backend.rs:2605`). On the same source, the decorator must DELEGATE to inner again (with the existing
substituted worktree cwd) — just DON'T call `provider.add` a second time. (T-decorator: replace the `return Ok(())`
with a delegate-without-add.)

### PR-FIX-7 (codex MAJOR-7) — close the check-then-add race (single-flight)
The decorator drops the map lock before `provider.add` → two concurrent `configure_session` on one SessionId could
both add. Use a `Reserving|Ready` entry (insert `Reserving` under the lock; the loser awaits/sees it; only one
`add`). Add a concurrent-configure test proving exactly one `add`.

### PR-FIX-8 (codex MAJOR-8) — `make_spawn_fn` wiring ripples to ALL call sites
`make_spawn_fn` (`main.rs:482`) has no worktree param and is called at MULTIPLE sites — `:1984`, `:2665`, `:3869`,
`:4090` (+ implement/resume). Define a concrete runtime `WorktreeRuntimeCfg { enabled, root, allowed_root }` (built
once from `[worktrees]` + `allowed_cwd_root`) and thread it through EVERY `make_spawn_fn` call site (mirror how
`permission_registry` is threaded). Gate the T5 step with `cargo build --workspace` so a missed call site is a
compile error. `wc.backend_cfg(agent_id)` must be a real method producing `WorktreeConfig { root, owner, run }`.

### PR-FIX-9 (codex MAJOR-9) — root preflight: outside any repo AND outside `allowed_cwd_root`
T5's preflight only rejects "inside a git repo". v2 requires the root ALSO outside `allowed_cwd_root` (canonical).
Preflight the resolved root against BOTH (git containment + canonical `allowed_cwd_root` containment) in every
command path that can enable worktrees (serve + run-workflow + implement).

### PR-FIX-10 (codex MAJOR-10) — add-failure cleanup + bounded retry
On `worktree add` failure: remove any partial worktree dir + `git worktree prune` the source (no half-state, per
spec). Classify retryable git LOCK failures (index.lock / worktrees lock in stderr) → bounded retry (mirror B2b's
commit-with-retry). (T-HostGitWorktree.)

### PR-FIX-11 (codex MAJOR-11) — T6's test must compile against the real `Rec`
`Rec.prompts` is `Mutex<Vec<String>>` (`executor.rs:648`), not an int. The T6 test must `push` to the vec (or use
a separate `AtomicUsize` counter) and assert no-prompt + exactly-one-forget. The impl return shape is correct
(3-tuple `(String,bool,Option<UsageSnapshot>)`, `executor.rs:247`).

### PR-FIX-12 (codex MAJOR-12 + Opus-4) — SR-FIX-9 is a DOCUMENTED BYPASS, not silent coverage
The plan's self-review wording ("static cwd never reaches as `spec.cwd`") is misleading: `AcpBackend` falls back to
static `AcpConfig.cwd` (`acp_backend.rs:1651`, set at `main.rs:265`) → a static-`[agents].cwd` session with NO
per-request cwd shares the repo tree (NO worktree). This IS the intended per-request-cwd-only scope, but it's a
BYPASS to DOCUMENT explicitly (config doc + a test asserting `spec.cwd=None` → pass-through → no worktree), not to
imply as "covered". (T-decorator test + a doc line.)

### PR-FIX-13 (codex MINOR + Opus-2) — error variant
Use `BridgeError::ConfigMismatch { field: "cwd" }` (the SessionManager's cwd-immutability error,
`session_manager.rs:438`) for the different-source reject, NOT `InvalidStateTransition` (both exist, `error.rs:26/66`;
ConfigMismatch is the consistent client disposition). The gate reject uses `InvalidRequest { field: "..." }` (static str).

### CONFIRM (both lenses — do NOT re-litigate)
10-method `AgentBackend` (`ports.rs:43`); `capabilities()` SYNC (delegate without `.await`); substitute-after-
fingerprint holds (`session_manager.rs:559` before `:574`). No leak on reset/release_all (cascade to
`release_session`) — but RETIRE was the missed leak (PR-FIX-5).

### Revised task structure (net of the folds)
T1 crate+provider+argv → **T2 worktree_path + canonicalize/self-gate + full sidecar (PR-FIX-1/3/4/13)** →
**T3 HostGitWorktree + add-failure-cleanup + bounded-retry + isolation smoke (PR-FIX-2/10)** → **T4 WorktreeBackend
decorator: full-trait delegate + delegate-then-remove + idempotent-RE-DELEGATE + single-flight reserve + retire-
drains-map + None-bypass test (PR-FIX-5/6/7/12)** → **T5 [worktrees] config + dual-preflight + ALL-call-site
make_spawn_fn wiring (PR-FIX-8/9)** → T6 cold executor configure-error (fix the test, PR-FIX-11) → **T7 lease-aware
boot-sweep + run-workflow end-guard (PR-FIX-4)** → T8 unborn-HEAD + workspace gate. After folding PR-FIX-1..13 the
plan is **ready-to-implement**.
