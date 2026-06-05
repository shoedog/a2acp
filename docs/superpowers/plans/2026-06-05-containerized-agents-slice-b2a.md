# Containerized Agents — Slice B2a Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a write-capable `AgentKind::ContainerRw` whose `ContainerRwBackend` spawns a **fresh `:rw` container per `prompt` turn** (composing `AcpBackend`), reliably reaps it, and is validated by a containerized agent writing a file to a `:rw` scratch mount that persists on the host.

**Architecture:** Pure argv composers (`compose_container_rw`/`reap_argv`/`check_rw_target`) live in `bridge-core::sandbox` beside `compose_sandbox`. A new `crates/bridge-container` crate holds `ContainerRwBackend` (per-turn factory): it implements `bridge_core::ports::AgentBackend`, composes `AcpBackend` per turn through a `ContainerSpawn` injection seam (so warm-reuse/reaper tests run Docker-free), records a per-session `inflight` handle so A2A `cancel` reaches the inner, and the returned stream **owns** a `ContainerReaper` that detaches a `docker rm -f` on every terminal path. A stable-identity boot-sweep clears crash orphans before the first mint. Both `SpawnFn` closures in `main.rs` gain a `ContainerRw` arm.

**Tech Stack:** Rust (tokio, async-trait, futures, async-stream), Docker/Podman (runtime, gate only), `agent-client-protocol` via the existing `bridge-acp`.

**Design refinement vs spec (one, deliberate):** the spec places `check_rw_target` in the *pure* `sandbox.rs` but also says it canonicalizes. Canonicalization is filesystem I/O, which violates `sandbox.rs`'s "No Docker, no I/O" contract (sandbox.rs:1). Resolution: `sandbox.rs::check_rw_target` stays **pure** (a lexical `is_under` check on already-canonicalized inputs, returning the stable error); the **backend** does the canonicalization (it already does I/O). Same guarantee, correct layering. The plan-review will see this called out here.

**Branch:** `feat/container-rw-backend` (spec committed `662019e`). **Commits:** task/code commits do NOT carry the `Co-Authored-By` trailer (this plan doc does). **Coverage:** after `cargo llvm-cov clean --workspace` — floors workspace 85, bridge-core 90.

---

## File Structure

| File | Responsibility | Action |
|---|---|---|
| `crates/bridge-core/src/domain.rs` | `AgentKind::ContainerRw` variant | Modify (enum at :31) |
| `crates/bridge-core/src/sandbox.rs` | `compose_container_rw`, `reap_argv`, `check_rw_target` (all PURE) | Modify (append) |
| `bin/a2a-bridge/src/config.rs` | `parse_kind` accepts `"container_rw"` | Modify (:472) |
| `crates/bridge-registry/src/registry.rs` | extract `validate_sandbox`; `ContainerRw` validate arm | Modify (:102–162) |
| `crates/bridge-container/Cargo.toml` | new crate manifest | Create |
| `crates/bridge-container/src/lib.rs` | `ContainerSpawn` seam, `ContainerRwConfig`, `ContainerRwBackend`, `ContainerReaper`, `InflightTurn` | Create |
| `bin/a2a-bridge/src/main.rs` | `ContainerRw` arm in BOTH `SpawnFn` closures (:177, :856); production `ContainerSpawn` | Modify |
| `bin/a2a-bridge/Cargo.toml` | depend on `bridge-container` | Modify |
| `examples/a2a-bridge.containerized.toml` | a `container_rw` agent for the gate | Modify |

The `[workspace] members = ["crates/*", ...]` (root `Cargo.toml:3`) auto-includes the new crate.

---

# Slice 0 — Domain + parse + validate + compiler-forced stubs (pure Rust, no Docker)

### Task 1: `AgentKind::ContainerRw` variant + `parse_kind`

**Files:**
- Modify: `crates/bridge-core/src/domain.rs:31`
- Modify: `bin/a2a-bridge/src/config.rs:472`
- Test: inline in `config.rs`

- [ ] **Step 1: Write the failing test** (append to the `#[cfg(test)] mod tests` in `config.rs`):

```rust
    #[test]
    fn parse_kind_accepts_container_rw() {
        assert_eq!(super::parse_kind("container_rw").unwrap(), bridge_core::domain::AgentKind::ContainerRw);
    }

    #[test]
    fn parse_kind_error_lists_container_rw() {
        let err = super::parse_kind("nope").unwrap_err();
        assert!(format!("{err:?}").contains("acp|api|container_rw"), "got: {err:?}");
    }
```

- [ ] **Step 2: Run — verify it fails to compile** (`ContainerRw` doesn't exist):

Run: `cargo test -p a2a-bridge parse_kind_accepts_container_rw 2>&1 | head -20`
Expected: compile error `no variant named ContainerRw`.

- [ ] **Step 3: Add the variant** in `crates/bridge-core/src/domain.rs` (the enum at :31):

```rust
pub enum AgentKind {
    #[default]
    Acp,
    /// Non-process OpenAI-compatible HTTP backend (bridge-api).
    Api,
    /// Write-capable per-turn containerized ACP agent (bridge-container, Slice B2a).
    ContainerRw,
}
```

- [ ] **Step 4: Teach `parse_kind`** in `config.rs:472`:

```rust
fn parse_kind(s: &str) -> Result<AgentKind, ConfigError> {
    Ok(match s {
        "acp" => AgentKind::Acp,
        "api" => AgentKind::Api,
        "container_rw" => AgentKind::ContainerRw,
        other => {
            return Err(ConfigError::Registry(format!(
                "invalid kind: {other:?} (expected acp|api|container_rw)"
            )))
        }
    })
}
```

- [ ] **Step 5: Run — verify pass + nothing else broke.** Adding an enum variant makes existing `match e.kind` arms in `registry.rs` and `main.rs` non-exhaustive — fix those in Tasks 2 & 3; for now they will fail to compile, so run only this crate's check:

Run: `cargo test -p bridge-core && cargo test -p a2a-bridge --lib parse_kind 2>&1 | tail -20`
Expected: bridge-core PASS; the `a2a-bridge` build still fails on the non-exhaustive `match entry.kind` (expected — Task 3 fixes it). If you want a green checkpoint now, do Steps of Task 2 and 3 before committing.

- [ ] **Step 6: Commit** (after Tasks 2 & 3 make the workspace compile — see Task 3 Step 6).

---

### Task 2: extract `validate_sandbox` + `ContainerRw` validate arm

**Files:**
- Modify: `crates/bridge-registry/src/registry.rs:102-183`
- Test: inline in `registry.rs` `#[cfg(test)]`

- [ ] **Step 1: Write the failing tests** (append to `registry.rs` tests; reuse the existing test helpers — find how other validate tests build a `RegistrySnapshot`, mirror them):

```rust
    #[test]
    fn container_rw_permits_rw_and_requires_sandbox_and_cmd() {
        // a ContainerRw entry WITH sandbox(access=rw) + cmd validates OK
        let snap = snap_with(vec![entry_container_rw_rw()]); // helper below
        assert!(validate(&snap).is_ok(), "container_rw+sandbox+cmd+rw should validate");
    }

    #[test]
    fn container_rw_without_sandbox_is_rejected() {
        let mut e = entry_container_rw_rw();
        e.sandbox = None;
        let err = validate(&snap_with(vec![e])).unwrap_err();
        assert!(format!("{err:?}").contains("container_rw requires sandbox"), "got {err:?}");
    }

    #[test]
    fn container_rw_without_cmd_is_rejected() {
        let mut e = entry_container_rw_rw();
        e.cmd = None;
        let err = validate(&snap_with(vec![e])).unwrap_err();
        assert!(format!("{err:?}").contains("container_rw requires cmd"), "got {err:?}");
    }

    #[test]
    fn container_rw_forbids_base_url() {
        let mut e = entry_container_rw_rw();
        e.base_url = Some("http://x".into());
        let err = validate(&snap_with(vec![e])).unwrap_err();
        assert!(format!("{err:?}").contains("container_rw forbids base_url"), "got {err:?}");
    }

    #[test]
    fn acp_still_rejects_rw() {
        let mut e = entry_container_rw_rw();
        e.kind = bridge_core::domain::AgentKind::Acp;
        let err = validate(&snap_with(vec![e])).unwrap_err();
        assert!(format!("{err:?}").contains("requires the container_rw kind"), "got {err:?}");
    }
```

Add the test helper near the other test builders (mirror an existing sandboxed-`Acp` test entry; `allowed_cmds` must include `"docker"`):

```rust
    fn entry_container_rw_rw() -> bridge_core::domain::AgentEntry {
        let mut e = sandboxed_acp_entry(); // the existing helper used by the S3/S4 tests
        e.kind = bridge_core::domain::AgentKind::ContainerRw;
        e.sandbox.as_mut().unwrap().access = bridge_core::domain::MountAccess::Rw;
        e
    }
```

> If a `sandboxed_acp_entry()` / `snap_with()` helper doesn't already exist, copy the construction from the nearest existing `validate` test (the S4 `access=rw` test builds exactly this shape) and lift it into a helper.

- [ ] **Step 2: Run — verify fail.** Run: `cargo test -p bridge-registry container_rw 2>&1 | tail -20`. Expected: compile error (non-exhaustive `match e.kind`) or assertion failures.

- [ ] **Step 3: Extract `validate_sandbox`** — pull the `Some(sb)` body of the `Acp` arm (registry.rs:111-153, the S3/S5/S6 checks) into a free fn so the new arm shares it verbatim:

```rust
/// S3/S5/S6 sandbox invariants, shared by the `Acp` (`:ro`) and `ContainerRw` (`:rw`) arms.
/// S4 (the `:rw` policy) is NOT here — it differs per kind (Acp rejects, ContainerRw permits).
fn validate_sandbox(
    sb: &bridge_core::domain::SandboxConfig,
    allowed_cmds: &[String],
    id: &str,
) -> Result<(), BridgeError> {
    let runtime = sb.runtime();
    if !allowed_cmds.iter().any(|c| c == runtime) {
        return Err(BridgeError::ConfigInvalid {
            reason: format!("sandbox runtime not allowed: {runtime}"),
        });
    }
    let mount = bridge_core::SessionCwd::parse(&sb.mount).map_err(|_| BridgeError::ConfigInvalid {
        reason: format!("sandbox mount must be an absolute path: {}", sb.mount),
    })?;
    for v in &sb.volumes {
        let dest = v.split(':').nth(1).unwrap_or("");
        if let Ok(d) = bridge_core::SessionCwd::parse(dest) {
            if d.as_str() == mount.as_str() || d.is_under(&mount) {
                return Err(BridgeError::ConfigInvalid {
                    reason: format!("sandbox volume dest {dest:?} is nested under the mount {:?}", sb.mount),
                });
            }
        }
    }
    let _ = id;
    Ok(())
}
```

Rewrite the `Acp` `Some(sb)` branch to: keep the S4 `access==Rw` reject (unchanged text "requires the container_rw kind (Slice B2)"), then `validate_sandbox(sb, &snap.allowed_cmds, e.id.as_str())?;`.

- [ ] **Step 4: Add the `ContainerRw` arm** after the `Api` arm in the `match e.kind`:

```rust
            AgentKind::ContainerRw => {
                if e.cmd.is_none() {
                    return Err(BridgeError::ConfigInvalid {
                        reason: format!("container_rw agent {} requires cmd", e.id.as_str()),
                    });
                }
                if e.base_url.is_some() {
                    return Err(BridgeError::ConfigInvalid {
                        reason: format!("container_rw agent {} forbids base_url", e.id.as_str()),
                    });
                }
                let Some(sb) = &e.sandbox else {
                    return Err(BridgeError::ConfigInvalid {
                        reason: format!("container_rw agent {} requires sandbox", e.id.as_str()),
                    });
                };
                // S4 INVERTED for this kind: access=rw is PERMITTED. S3/S5/S6 still apply.
                validate_sandbox(sb, &snap.allowed_cmds, e.id.as_str())?;
            }
```

- [ ] **Step 5: Run — verify pass.** Run: `cargo test -p bridge-registry 2>&1 | tail -20`. Expected: all PASS (including the new `container_rw_*` and the existing S3/S4/S5/S6 tests via the extracted helper).

- [ ] **Step 6: Commit** (with Task 3, in the same green commit — see Task 3 Step 6).

---

### Task 3: compiler-forced `ContainerRw` stubs in BOTH `SpawnFn` closures

This makes the workspace compile end-of-Slice-0. The real backend is wired in Slice 3; for now the arm errors loudly so a misconfig can't silently fall through.

**Files:**
- Modify: `bin/a2a-bridge/src/main.rs` (run-workflow closure ~:198, serve closure ~:878)

- [ ] **Step 1: Add the stub arm** in BOTH `match entry.kind` blocks (after `AgentKind::Api`):

```rust
                AgentKind::ContainerRw => {
                    return Err(BridgeError::ConfigInvalid {
                        reason: format!(
                            "container_rw agent {} not yet wired (Slice B2a Slice 3)",
                            entry.id.as_str()
                        ),
                    });
                }
```

- [ ] **Step 2: Run — verify the workspace compiles.** Run: `cargo build --workspace 2>&1 | tail -20`. Expected: success (no non-exhaustive-match errors).

- [ ] **Step 3: Run the full suite** to confirm Slice 0 is green. Run: `cargo test --workspace 2>&1 | tail -15`. Expected: all PASS.

- [ ] **Step 4: fmt + clippy.** Run: `cargo fmt && cargo clippy --workspace --all-targets 2>&1 | tail -15`. Expected: clean.

- [ ] **Step 5: Commit** Tasks 1–3 together (first green checkpoint):

```bash
git add crates/bridge-core/src/domain.rs bin/a2a-bridge/src/config.rs \
        crates/bridge-registry/src/registry.rs bin/a2a-bridge/src/main.rs
git commit -m "feat(b2a): AgentKind::ContainerRw + parse + validate arm + spawn stubs"
```

---

# Slice 1 — Pure composers in `bridge-core::sandbox` (Docker-free golden tests)

### Task 4: `compose_container_rw`

**Files:**
- Modify: `crates/bridge-core/src/sandbox.rs` (append fn + tests)

- [ ] **Step 1: Write the failing test** (append to `sandbox.rs` `mod tests`):

```rust
    #[test]
    fn container_rw_mounts_target_rw_with_name_after_rm() {
        let sb = ro_locked(); // egress=Locked, volumes=[creds]; access is overridden inside
        let rw = crate::session_cwd::SessionCwd::parse("/Users/w/code/.scratch").unwrap();
        let (program, argv) = compose_container_rw(&sb, &rw, "a2a-rw-inst-0", "claude-agent-acp", &[]);
        assert_eq!(program, "docker");
        // --name spliced immediately after --rm
        assert_eq!(&argv[0..5], &["run", "-i", "--rm", "--name", "a2a-rw-inst-0"]);
        // mount is the rw_target, identical-path, NO :ro suffix
        assert!(argv.windows(2).any(|w| w[0] == "-v" && w[1] == "/Users/w/code/.scratch:/Users/w/code/.scratch"));
        assert!(!argv.iter().any(|a| a.ends_with(":ro")));
        // egress + creds volume + image + cmd preserved from sb
        assert!(argv.iter().any(|a| a == "--network"));
        assert!(argv.iter().any(|a| a == "/host/creds:/root/.codex/auth.json"));
        assert_eq!(argv[argv.len() - 1], "claude-agent-acp");
    }

    #[test]
    fn container_rw_appends_agent_args_tail() {
        let sb = ro_locked();
        let rw = crate::session_cwd::SessionCwd::parse("/m/t").unwrap();
        let (_p, argv) = compose_container_rw(&sb, &rw, "n", "kiro-cli", &["acp".into()]);
        assert_eq!(argv.last().unwrap(), "acp");
    }
```

- [ ] **Step 2: Run — verify fail.** Run: `cargo test -p bridge-core container_rw_mounts 2>&1 | tail -20`. Expected: `cannot find function compose_container_rw`.

- [ ] **Step 3: Implement** (append to `sandbox.rs`, after `compose_sandbox`):

```rust
use crate::session_cwd::SessionCwd;

/// PURE+TOTAL. Per-turn `:rw` argv for a `ContainerRw` agent. The `:rw` mount is the per-task
/// `rw_target` (NOT `sb.mount`); model as "same sandbox, mount=rw_target, access=Rw" and REUSE
/// `compose_sandbox` so egress/volumes/runtime/suffix derivation stay ONE source of truth. A unique
/// `--name` is spliced immediately after `--rm` so the container is reapable by name.
pub fn compose_container_rw(
    sb: &SandboxConfig,
    rw_target: &SessionCwd,
    name: &str,
    cmd: &str,
    args: &[String],
) -> (String, Vec<String>) {
    let derived = SandboxConfig {
        mount: rw_target.as_str().to_string(),
        access: MountAccess::Rw,
        ..sb.clone()
    };
    let (program, mut argv) = compose_sandbox(&derived, cmd, args);
    // INVARIANT: compose_sandbox always emits ["run","-i","--rm", ...] (this module, line ~17).
    debug_assert_eq!(&argv[0..3], &["run", "-i", "--rm"], "compose_sandbox prefix changed — fix the splice");
    argv.splice(3..3, [String::from("--name"), name.to_string()]);
    (program, argv)
}
```

- [ ] **Step 4: Run — verify pass.** Run: `cargo test -p bridge-core container_rw 2>&1 | tail -20`. Expected: PASS.

- [ ] **Step 5: Commit.**

```bash
git add crates/bridge-core/src/sandbox.rs
git commit -m "feat(b2a): compose_container_rw (per-turn :rw argv, reuses compose_sandbox)"
```

---

### Task 5: `reap_argv`

**Files:** Modify `crates/bridge-core/src/sandbox.rs`.

- [ ] **Step 1: Write the failing test:**

```rust
    #[test]
    fn reap_argv_shape_docker_and_podman() {
        assert_eq!(reap_argv("docker", "a2a-rw-x"), ("docker".to_string(), vec!["rm".into(), "-f".into(), "a2a-rw-x".into()]));
        assert_eq!(reap_argv("podman", "a2a-rw-y").0, "podman");
    }
```

- [ ] **Step 2: Run — verify fail.** Run: `cargo test -p bridge-core reap_argv 2>&1 | tail`. Expected: `cannot find function reap_argv`.

- [ ] **Step 3: Implement** (append to `sandbox.rs`):

```rust
/// PURE. The reap command for a named per-turn container: `<runtime> rm -f <name>`. Idempotent at the
/// Docker layer (`rm -f` of a gone container is a no-op error we ignore).
pub fn reap_argv(runtime: &str, name: &str) -> (String, Vec<String>) {
    (runtime.to_string(), vec!["rm".into(), "-f".into(), name.to_string()])
}
```

- [ ] **Step 4: Run — verify pass.** Run: `cargo test -p bridge-core reap_argv 2>&1 | tail`. Expected: PASS.

- [ ] **Step 5: Commit.** `git commit -am "feat(b2a): reap_argv"`

---

### Task 6: `check_rw_target` (pure, lexical, on canonicalized inputs)

**Files:** Modify `crates/bridge-core/src/sandbox.rs`.

- [ ] **Step 1: Write the failing test:**

```rust
    #[test]
    fn check_rw_target_accepts_under_rejects_escape() {
        let root = crate::session_cwd::SessionCwd::parse("/Users/w/code").unwrap();
        let ok = crate::session_cwd::SessionCwd::parse("/Users/w/code/.scratch").unwrap();
        let sib = crate::session_cwd::SessionCwd::parse("/Users/w/code-evil").unwrap();
        assert!(check_rw_target(&root, &ok).is_ok());
        let err = check_rw_target(&root, &sib).unwrap_err();
        assert!(format!("{err:?}").contains("escapes mount root"), "got {err:?}");
        assert!(check_rw_target(&root, &root).is_ok()); // equal is under
    }
```

- [ ] **Step 2: Run — verify fail.** `cargo test -p bridge-core check_rw_target 2>&1 | tail`.

- [ ] **Step 3: Implement** (append to `sandbox.rs`; note `use crate::error::BridgeError;` at top if absent):

```rust
/// PURE. Lexical containment of a WRITABLE target under the mount root. BOTH inputs MUST already be
/// canonicalized by the caller (the backend does the I/O — sandbox.rs stays pure). Stable error
/// fragment `":rw target escapes mount root"` so tests don't hard-code ad-hoc strings.
pub fn check_rw_target(mount_canon: &SessionCwd, rw_canon: &SessionCwd) -> Result<(), crate::error::BridgeError> {
    if rw_canon.is_under(mount_canon) {
        Ok(())
    } else {
        Err(crate::error::BridgeError::ConfigInvalid {
            reason: format!(":rw target escapes mount root: {} not under {}", rw_canon.as_str(), mount_canon.as_str()),
        })
    }
}
```

- [ ] **Step 4: Run — verify pass.** `cargo test -p bridge-core check_rw_target 2>&1 | tail`. Expected: PASS.

- [ ] **Step 5: Coverage checkpoint + commit.** Run `cargo llvm-cov clean --workspace && cargo llvm-cov -p bridge-core 2>&1 | tail -5` (sandbox.rs near 100%). Commit:

```bash
git commit -am "feat(b2a): check_rw_target (pure lexical containment, canonicalized inputs)"
```

---

# Slice 2 — `crates/bridge-container` (per-turn backend, Docker-free via the spawn seam)

### Task 7: crate scaffold + `ContainerSpawn` seam + config/struct skeleton

**Files:**
- Create: `crates/bridge-container/Cargo.toml`
- Create: `crates/bridge-container/src/lib.rs`
- Modify: `bin/a2a-bridge/Cargo.toml` (add the dep — used in Slice 3, add now)

- [ ] **Step 1: Create `crates/bridge-container/Cargo.toml`:**

```toml
[package]
name = "bridge-container"
version.workspace = true
edition.workspace = true
license.workspace = true

[dependencies]
bridge-core = { path = "../bridge-core" }
bridge-acp = { path = "../bridge-acp" }
async-trait.workspace = true
futures.workspace = true
async-stream.workspace = true
tokio = { workspace = true }
tokio-stream.workspace = true
tracing.workspace = true

[dev-dependencies]
tokio = { workspace = true }
tokio-test = { workspace = true }
tempfile = "3"
```

- [ ] **Step 2: Create `crates/bridge-container/src/lib.rs`** with the seam + types (no behavior yet):

```rust
//! Per-turn write-capable containerized ACP agent (Slice B2a). `ContainerRwBackend` spawns a fresh
//! `:rw` container per `prompt` (composing `bridge_acp::AcpBackend`) and reaps it on stream end.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use bridge_acp::acp_backend::AcpConfig;
use bridge_core::domain::{SandboxConfig, SessionSpec};
use bridge_core::error::BridgeError;
use bridge_core::ids::SessionId;
use bridge_core::ports::{AgentBackend, BackendStream, PolicyEngine};
use bridge_core::session_cwd::SessionCwd;
use tokio::sync::Mutex;

/// Injection seam so warm-reuse / reaper tests run Docker-free. Production wraps `AcpBackend::spawn`.
#[async_trait]
pub trait ContainerSpawn: Send + Sync {
    async fn spawn(
        &self,
        program: &str,
        argv: &[String],
        cfg: AcpConfig,
    ) -> Result<Arc<dyn AgentBackend>, BridgeError>;
}

pub struct ContainerRwConfig {
    pub sandbox: SandboxConfig,
    pub cmd: String,
    pub args: Vec<String>,
    pub model: Option<String>,
    pub mode: Option<String>,
    pub auth_method: Option<String>,
    pub handshake_timeout: Duration,
    pub cancel_grace: Duration,
}

struct InflightTurn {
    inner: Arc<dyn AgentBackend>,
    name: String,
}

pub struct ContainerRwBackend {
    cfg: ContainerRwConfig,
    spawn: Arc<dyn ContainerSpawn>,
    policy: Arc<dyn PolicyEngine>,
    owner: String, // STABLE instance id (hash of config-path + allowed_cwd_root)
    allowed_cwd_root: SessionCwd,
    session_cfg: Mutex<HashMap<SessionId, SessionSpec>>,
    inflight: Mutex<HashMap<SessionId, InflightTurn>>,
    turn_seq: AtomicU64,
}
```

- [ ] **Step 3: Add the bin dependency** to `bin/a2a-bridge/Cargo.toml` `[dependencies]`:

```toml
bridge-container = { path = "../../crates/bridge-container" }
```

- [ ] **Step 4: Run — verify it compiles** (unused-field warnings are fine for now; allow them):

Add `#![allow(dead_code)]` temporarily at the top of `lib.rs` (removed in Task 13). Run: `cargo build -p bridge-container 2>&1 | tail`. Expected: success.

- [ ] **Step 5: Commit.** `git add crates/bridge-container bin/a2a-bridge/Cargo.toml && git commit -m "feat(b2a): bridge-container crate scaffold + ContainerSpawn seam"`

---

### Task 8: `configure_session` stash + `forget_session` stash-only + `new`

**Files:** Modify `crates/bridge-container/src/lib.rs`.

- [ ] **Step 1: Write the failing test** (append a `#[cfg(test)] mod tests` to `lib.rs`; includes a stub spawn + policy + a counting stub backend reused by later tasks):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use bridge_core::domain::{EffectiveConfig, EgressPolicy, MountAccess};
    use bridge_core::ports::{PermissionDecision, Update};
    use std::sync::atomic::AtomicUsize;

    // ---- stubs -------------------------------------------------------------
    pub(super) struct AllowAll;
    impl PolicyEngine for AllowAll {
        fn decide(&self, _: &bridge_core::domain::PermissionRequest, _: &bridge_core::domain::SessionContext)
            -> Result<PermissionDecision, BridgeError> { Ok(PermissionDecision::Allow) }
    }

    /// A stub inner backend: counts prompts, emits one Done. Records cancels.
    pub(super) struct StubInner { pub canceled: AtomicBool }
    #[async_trait]
    impl AgentBackend for StubInner {
        async fn prompt(&self, _s: &SessionId, _p: Vec<bridge_core::domain::Part>) -> Result<BackendStream, BridgeError> {
            Ok(Box::pin(tokio_stream::iter(vec![Ok(Update::Done { stop_reason: "end_turn".into() })])))
        }
        async fn cancel(&self, _s: &SessionId) -> Result<(), BridgeError> { self.canceled.store(true, Ordering::SeqCst); Ok(()) }
    }

    /// A spawn seam that counts spawns, records the last argv, and can be told to fail.
    pub(super) struct CountingSpawn {
        pub count: AtomicUsize,
        pub fail: bool,
        pub last_argv: Mutex<Vec<String>>,
    }
    #[async_trait]
    impl ContainerSpawn for CountingSpawn {
        async fn spawn(&self, _program: &str, argv: &[String], _cfg: AcpConfig) -> Result<Arc<dyn AgentBackend>, BridgeError> {
            self.count.fetch_add(1, Ordering::SeqCst);
            *self.last_argv.lock().await = argv.to_vec();
            if self.fail { return Err(BridgeError::agent_crashed("boom")); }
            Ok(Arc::new(StubInner { canceled: AtomicBool::new(false) }))
        }
    }

    pub(super) fn sandbox() -> SandboxConfig {
        SandboxConfig { runtime: None, image: "img".into(), mount: "/root".into(),
            access: MountAccess::Ro, egress: EgressPolicy::Open, volumes: vec![] }
    }
    pub(super) fn backend(spawn: Arc<dyn ContainerSpawn>) -> ContainerRwBackend {
        ContainerRwBackend::new(
            ContainerRwConfig { sandbox: sandbox(), cmd: "claude-agent-acp".into(), args: vec![],
                model: None, mode: None, auth_method: None,
                handshake_timeout: Duration::from_secs(30), cancel_grace: Duration::from_secs(5) },
            spawn, Arc::new(AllowAll), "inst".into(),
            SessionCwd::parse("/root").unwrap(),
        )
    }
    fn spec_with_cwd(p: &str) -> SessionSpec {
        SessionSpec { config: EffectiveConfig::default(), cwd: Some(SessionCwd::parse(p).unwrap()) }
    }

    #[tokio::test]
    async fn configure_then_forget_clears_stash() {
        let be = backend(Arc::new(CountingSpawn { count: AtomicUsize::new(0), fail: false, last_argv: Mutex::new(vec![]) }));
        let s = SessionId::parse("s1").unwrap();
        be.configure_session(&s, &spec_with_cwd("/root/sub")).await.unwrap();
        assert!(be.session_cfg.lock().await.contains_key(&s));
        be.forget_session(&s).await;
        assert!(!be.session_cfg.lock().await.contains_key(&s), "forget is stash-drop");
    }
```

- [ ] **Step 2: Run — verify fail.** `cargo test -p bridge-container configure_then_forget 2>&1 | tail`. Expected: `no function new`.

- [ ] **Step 3: Implement `new` + `configure_session` + `forget_session`** (append an `impl ContainerRwBackend` + the trait skeleton; `prompt`/`cancel` are filled in Tasks 10–12 — stub them to compile):

```rust
impl ContainerRwBackend {
    pub fn new(
        cfg: ContainerRwConfig,
        spawn: Arc<dyn ContainerSpawn>,
        policy: Arc<dyn PolicyEngine>,
        owner: String,
        allowed_cwd_root: SessionCwd,
    ) -> Self {
        Self {
            cfg, spawn, policy, owner, allowed_cwd_root,
            session_cfg: Mutex::new(HashMap::new()),
            inflight: Mutex::new(HashMap::new()),
            turn_seq: AtomicU64::new(0),
        }
    }
}

#[async_trait]
impl AgentBackend for ContainerRwBackend {
    async fn prompt(&self, _s: &SessionId, _p: Vec<bridge_core::domain::Part>) -> Result<BackendStream, BridgeError> {
        unimplemented!("Task 10")
    }
    async fn cancel(&self, _s: &SessionId) -> Result<(), BridgeError> { unimplemented!("Task 12") }
    async fn configure_session(&self, session: &SessionId, spec: &SessionSpec) -> Result<(), BridgeError> {
        self.session_cfg.lock().await.insert(session.clone(), spec.clone());
        Ok(())
    }
    async fn forget_session(&self, session: &SessionId) {
        self.session_cfg.lock().await.remove(session);
    }
}
```

- [ ] **Step 4: Run — verify pass.** `cargo test -p bridge-container configure_then_forget 2>&1 | tail`. Expected: PASS.

- [ ] **Step 5: Commit.** `git commit -am "feat(b2a): ContainerRwBackend new + configure/forget (stash-only)"`

---

### Task 9: backend-side canonicalizing rw-target guard

**Files:** Modify `crates/bridge-container/src/lib.rs`.

- [ ] **Step 1: Write the failing test** (uses a real tempdir + a symlink to prove canonicalization):

```rust
    #[tokio::test]
    async fn rw_target_guard_canonicalizes_both_sides() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        // a symlink UNDER the root that points OUTSIDE it: a lexical check would pass, canonical must reject.
        let link = root.path().join("escape");
        std::os::unix::fs::symlink(outside.path(), &link).unwrap();
        let be = backend_with_root(Arc::new(CountingSpawn{count:AtomicUsize::new(0),fail:false,last_argv:Mutex::new(vec![])}),
            root.path().to_str().unwrap());
        // a real subdir is OK
        let sub = root.path().join("ok"); std::fs::create_dir(&sub).unwrap();
        assert!(be.resolve_rw_target(&SessionCwd::parse(sub.to_str().unwrap()).unwrap()).is_ok());
        // the symlink escapes
        let err = be.resolve_rw_target(&SessionCwd::parse(link.to_str().unwrap()).unwrap()).unwrap_err();
        assert!(format!("{err:?}").contains("escapes mount root"), "got {err:?}");
    }
```

Add the helper `backend_with_root` next to `backend()`:

```rust
    pub(super) fn backend_with_root(spawn: Arc<dyn ContainerSpawn>, root: &str) -> ContainerRwBackend {
        let mut sb = sandbox(); sb.mount = root.to_string();
        ContainerRwBackend::new(
            ContainerRwConfig { sandbox: sb, cmd: "claude-agent-acp".into(), args: vec![],
                model: None, mode: None, auth_method: None,
                handshake_timeout: Duration::from_secs(30), cancel_grace: Duration::from_secs(5) },
            spawn, Arc::new(AllowAll), "inst".into(), SessionCwd::parse(root).unwrap())
    }
```

- [ ] **Step 2: Run — verify fail.** `cargo test -p bridge-container rw_target_guard 2>&1 | tail`. Expected: `no method resolve_rw_target`.

- [ ] **Step 3: Implement** the I/O canonicalization + the pure check (add to `impl ContainerRwBackend`):

```rust
    /// Canonicalize BOTH the mount anchor and the rw target (resolving symlinks — the writable-mount
    /// security fix), then apply the pure lexical `check_rw_target`. A not-yet-existing scratch dir is
    /// canonicalized via its nearest existing ancestor + the lexical tail.
    pub(crate) fn resolve_rw_target(&self, rw: &SessionCwd) -> Result<SessionCwd, BridgeError> {
        let mount_canon = canonicalize_lenient(self.cfg.sandbox.mount.as_str())?;
        let rw_canon = canonicalize_lenient(rw.as_str())?;
        bridge_core::sandbox::check_rw_target(&mount_canon, &rw_canon)?;
        Ok(rw_canon)
    }

fn canonicalize_lenient(path: &str) -> Result<SessionCwd, BridgeError> {
    use std::path::{Path, PathBuf};
    let p = Path::new(path);
    // Walk up to the nearest existing ancestor, canonicalize it, re-append the missing tail.
    let mut existing = p;
    let mut tail: Vec<&std::ffi::OsStr> = vec![];
    let canon = loop {
        match std::fs::canonicalize(existing) {
            Ok(c) => break c,
            Err(_) => {
                let file = existing.file_name().ok_or(BridgeError::ConfigInvalid {
                    reason: format!(":rw target has no canonical root: {path}"),
                })?;
                tail.push(file);
                existing = existing.parent().ok_or(BridgeError::ConfigInvalid {
                    reason: format!(":rw target has no canonical root: {path}"),
                })?;
            }
        }
    };
    let mut out: PathBuf = canon;
    for seg in tail.iter().rev() { out.push(seg); }
    SessionCwd::parse(&out.to_string_lossy())
}
```

- [ ] **Step 4: Run — verify pass.** `cargo test -p bridge-container rw_target_guard 2>&1 | tail`. Expected: PASS.

- [ ] **Step 5: Commit.** `git commit -am "feat(b2a): canonicalizing rw-target guard (both sides, symlink-safe)"`

---

### Task 10: `prompt` mint path — spawn seam, name, spawn-failure reap, configure forward, inflight, reject-second

**Files:** Modify `crates/bridge-container/src/lib.rs`.

- [ ] **Step 1: Write the failing tests:**

```rust
    #[tokio::test]
    async fn prompt_spawns_once_and_argv_has_rw_mount_and_name() {
        let spawn = Arc::new(CountingSpawn { count: AtomicUsize::new(0), fail: false, last_argv: Mutex::new(vec![]) });
        let root = tempfile::tempdir().unwrap();
        let be = backend_with_root(spawn.clone(), root.path().to_str().unwrap());
        let s = SessionId::parse("s1").unwrap();
        be.configure_session(&s, &spec_with_cwd(root.path().to_str().unwrap())).await.unwrap();
        let mut stream = be.prompt(&s, vec![]).await.unwrap();
        assert_eq!(spawn.count.load(Ordering::SeqCst), 1, "one spawn per prompt");
        let argv = spawn.last_argv.lock().await.clone();
        assert_eq!(&argv[0..3], &["run", "-i", "--rm"]);
        assert_eq!(argv[3], "--name");
        assert!(argv[4].starts_with("a2a-rw-inst-"), "stable owner prefix: {}", argv[4]);
        // drain the stream so the turn completes
        use futures::StreamExt; while stream.next().await.is_some() {}
    }

    #[tokio::test]
    async fn prompt_without_cwd_strict_rejects() {
        let be = backend(Arc::new(CountingSpawn{count:AtomicUsize::new(0),fail:false,last_argv:Mutex::new(vec![])}));
        let s = SessionId::parse("s1").unwrap();
        let err = be.prompt(&s, vec![]).await.unwrap_err();
        assert!(format!("{err:?}").contains("missing session cwd"), "got {err:?}");
    }

    #[tokio::test]
    async fn prompt_spawn_failure_reaps_and_errors() {
        // fail=true → spawn errors; the container client may be up, so a reap MUST be attempted.
        let spawn = Arc::new(CountingSpawn { count: AtomicUsize::new(0), fail: true, last_argv: Mutex::new(vec![]) });
        let root = tempfile::tempdir().unwrap();
        let be = backend_with_root(spawn, root.path().to_str().unwrap());
        let s = SessionId::parse("s1").unwrap();
        be.configure_session(&s, &spec_with_cwd(root.path().to_str().unwrap())).await.unwrap();
        let err = be.prompt(&s, vec![]).await.unwrap_err();
        assert!(format!("{err:?}").contains("boom") || format!("{err:?}").contains("container spawn failed"), "got {err:?}");
        // inflight must NOT retain a leaked entry
        assert!(be.inflight.lock().await.is_empty());
    }
```

> The reap on spawn-failure goes through the same `ContainerReaper` mechanism (Task 11) but, since there is no stream yet, fire a one-shot `reap_now(&self.cfg.sandbox, &name)` (defined in Task 11). For this task, implement `prompt` to call a `self.reap_now(...)` that Task 11 provides; sequence the tasks so Task 11's `reap_now` lands first if your runner is strict, or stub `reap_now` to a no-op here and tighten in Task 11.

- [ ] **Step 2: Run — verify fail.** `cargo test -p bridge-container prompt_ 2>&1 | tail`. Expected: `unimplemented!("Task 10")` panic / failures.

- [ ] **Step 3: Implement `prompt`** (replace the `unimplemented!` body):

```rust
    async fn prompt(&self, session: &SessionId, parts: Vec<bridge_core::domain::Part>) -> Result<BackendStream, BridgeError> {
        // strict-reject: a writer MUST name its :rw target.
        let spec = {
            let m = self.session_cfg.lock().await;
            m.get(session).cloned().ok_or(BridgeError::ConfigInvalid { reason: "missing session cwd".into() })?
        };
        let cwd = spec.cwd.clone().ok_or(BridgeError::ConfigInvalid { reason: "missing session cwd".into() })?;
        // reject a second concurrent prompt on a live session.
        if self.inflight.lock().await.contains_key(session) {
            return Err(BridgeError::ConfigInvalid { reason: format!("session {} already has an in-flight turn", session.as_str()) });
        }
        let rw_canon = self.resolve_rw_target(&cwd)?;
        let n = self.turn_seq.fetch_add(1, Ordering::Relaxed);
        let name = format!("a2a-rw-{}-{}", self.owner, n);
        let (program, argv) = bridge_core::sandbox::compose_container_rw(&self.cfg.sandbox, &rw_canon, &name, &self.cfg.cmd, &self.cfg.args);
        let acp = AcpConfig {
            cwd: std::path::PathBuf::from(rw_canon.as_str()),
            model: self.cfg.model.clone(),
            mode: self.cfg.mode.clone(),
            auth_method: self.cfg.auth_method.clone(),
            handshake_timeout: self.cfg.handshake_timeout,
            cancel_grace: self.cfg.cancel_grace,
        };
        // SPAWN — on failure the docker client may be up before the handshake failed → reap by name.
        let inner = match self.spawn.spawn(&program, &argv, acp).await {
            Ok(inner) => inner,
            Err(e) => { self.reap_now(&name); return Err(e); }
        };
        // Propagate per-request overrides (model/mode/effort + cwd) to the inner AcpBackend.
        inner.configure_session(session, &spec).await?;
        // Record the cancel handle.
        self.inflight.lock().await.insert(session.clone(), InflightTurn { inner: inner.clone(), name: name.clone() });
        // Drive the inner turn; on inner.prompt error, clear inflight + reap.
        let inner_stream = match inner.prompt(session, parts).await {
            Ok(s) => s,
            Err(e) => { self.inflight.lock().await.remove(session); self.reap_now(&name); return Err(e); }
        };
        // Wrap so the stream OWNS (inner, reaper); reaper clears inflight + reaps on terminal/drop.
        let reaper = ContainerReaper::new(self.cfg.sandbox.runtime().to_string(), name, Arc::clone(&self.inflight_arc()), session.clone());
        Ok(wrap_with_reaper(inner, inner_stream, reaper))
    }
```

> `self.inflight_arc()` requires `inflight` to be an `Arc<Mutex<..>>` so the reaper can hold it. Change the field type in Task 7's struct to `inflight: Arc<Mutex<HashMap<SessionId, InflightTurn>>>` and initialize with `Arc::new(Mutex::new(..))` in `new`; add a tiny accessor `fn inflight_arc(&self) -> Arc<Mutex<...>> { self.inflight.clone() }`. (Apply this struct tweak now.) `reap_now` and `ContainerReaper`/`wrap_with_reaper` land in Task 11 — implement `reap_now` as a no-op + `wrap_with_reaper` returning the raw `inner_stream` here so the mint tests pass, then tighten in Task 11.

- [ ] **Step 4: Run — verify pass.** `cargo test -p bridge-container prompt_ 2>&1 | tail`. Expected: PASS (mint, strict-reject, spawn-failure).

- [ ] **Step 5: Commit.** `git commit -am "feat(b2a): ContainerRwBackend::prompt mint (seam, strict-reject, configure-forward, inflight, spawn-fail reap)"`

---

### Task 11: `ContainerReaper` — detached non-blocking reap + idempotency; stream wrapper

**Files:** Modify `crates/bridge-container/src/lib.rs`.

- [ ] **Step 1: Write the failing tests** (a reap seam so we assert without Docker — count reaps via a process-global test hook OR inject a reaper-fn; simplest: the reaper calls a `reap_fn` stored on the backend that tests can swap. For the unit test, assert idempotency + that the wrapper fires the reap once on terminal AND once-only on early drop):

```rust
    #[tokio::test]
    async fn stream_terminal_clears_inflight_and_reaps_once() {
        let reaps = Arc::new(AtomicUsize::new(0));
        let be = backend_with_reapfn(/* root */, reaps.clone()); // helper: injects a reap_fn that bumps `reaps`
        let s = SessionId::parse("s1").unwrap();
        be.configure_session(&s, &spec_with_cwd(/*root*/)).await.unwrap();
        let mut stream = be.prompt(&s, vec![]).await.unwrap();
        assert!(be.inflight.lock().await.contains_key(&s), "inflight set during turn");
        use futures::StreamExt; while stream.next().await.is_some() {}
        // allow the detached reap to run
        tokio::task::yield_now().await; tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        assert_eq!(reaps.load(Ordering::SeqCst), 1, "exactly one reap on terminal");
        assert!(!be.inflight.lock().await.contains_key(&s), "inflight cleared");
    }

    #[tokio::test]
    async fn early_drop_reaps_once() {
        let reaps = Arc::new(AtomicUsize::new(0));
        let be = backend_with_reapfn(/* root */, reaps.clone());
        let s = SessionId::parse("s1").unwrap();
        be.configure_session(&s, &spec_with_cwd(/*root*/)).await.unwrap();
        let stream = be.prompt(&s, vec![]).await.unwrap();
        drop(stream); // consumer disconnects before draining
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        assert_eq!(reaps.load(Ordering::SeqCst), 1);
    }
```

> Introduce a `reap_fn: Arc<dyn Fn(String, String) + Send + Sync>` field on `ContainerRwBackend` (args: runtime, name). Production sets it to a closure that spawns `reap_argv` via `tokio::process::Command` detached (Task 13). Tests inject a counter. `reap_now` and the `ContainerReaper` both call this `reap_fn`. Add a `backend_with_reapfn` test helper mirroring `backend_with_root` that sets `reap_fn`.

- [ ] **Step 2: Run — verify fail.** `cargo test -p bridge-container _reaps 2>&1 | tail`.

- [ ] **Step 3: Implement** the reaper + wrapper + `reap_now`:

```rust
type ReapFn = Arc<dyn Fn(String, String) + Send + Sync>;

struct ContainerReaper {
    runtime: String,
    name: String,
    inflight: Arc<Mutex<HashMap<SessionId, InflightTurn>>>,
    session: SessionId,
    reap_fn: ReapFn,
    reaped: Arc<AtomicBool>,
}
impl ContainerReaper {
    fn new(runtime: String, name: String, inflight: Arc<Mutex<HashMap<SessionId, InflightTurn>>>, session: SessionId, reap_fn: ReapFn) -> Self {
        Self { runtime, name, inflight, session, reap_fn, reaped: Arc::new(AtomicBool::new(false)) }
    }
}
impl Drop for ContainerReaper {
    fn drop(&mut self) {
        // clear the cancel handle (best-effort, non-blocking): detach a tiny task that locks+removes.
        let inflight = self.inflight.clone(); let session = self.session.clone();
        spawn_detached(async move { inflight.lock().await.remove(&session); });
        // reap exactly once; NEVER block the worker — reap_fn detaches internally.
        if !self.reaped.swap(true, Ordering::SeqCst) {
            (self.reap_fn)(self.runtime.clone(), self.name.clone());
        }
    }
}

/// Spawn a future onto the current runtime if there is one, else on a throwaway thread+runtime.
/// Drop can fire off-runtime (process shutdown), so this must never panic.
fn spawn_detached<F: std::future::Future<Output = ()> + Send + 'static>(fut: F) {
    match tokio::runtime::Handle::try_current() {
        Ok(h) => { h.spawn(fut); }
        Err(_) => { std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread().enable_all().build();
            if let Ok(rt) = rt { rt.block_on(fut); }
        }); }
    }
}

/// Build the outer stream that OWNS `inner` (keeps the ACP child alive) + `reaper` (reaps on drop).
fn wrap_with_reaper(inner: Arc<dyn AgentBackend>, inner_stream: BackendStream, reaper: ContainerReaper) -> BackendStream {
    use futures::StreamExt;
    Box::pin(async_stream::stream! {
        let _inner = inner;     // hold the ACP child for the whole turn
        let _reaper = reaper;   // reaps + clears inflight on completion OR early drop
        let mut s = inner_stream;
        while let Some(item) = s.next().await { yield item; }
    })
}
```

Add `reap_now` to `impl ContainerRwBackend` (used by Task 10's spawn-failure path):

```rust
    fn reap_now(&self, name: &str) {
        (self.reap_fn)(self.cfg.sandbox.runtime().to_string(), name.to_string());
    }
```

Wire the real `reaper` in `prompt` (replace the Task-10 placeholder): `let reaper = ContainerReaper::new(self.cfg.sandbox.runtime().to_string(), name, self.inflight.clone(), session.clone(), self.reap_fn.clone());`

- [ ] **Step 4: Run — verify pass.** `cargo test -p bridge-container _reaps early_drop 2>&1 | tail`. Expected: PASS.

- [ ] **Step 5: Commit.** `git commit -am "feat(b2a): ContainerReaper (detached non-blocking reap, idempotent) + stream wrapper"`

---

### Task 12: `cancel` routes via `inflight` + reap; `retire` drains

**Files:** Modify `crates/bridge-container/src/lib.rs`.

- [ ] **Step 1: Write the failing test:**

```rust
    #[tokio::test]
    async fn cancel_reaches_inner_and_reaps() {
        let reaps = Arc::new(AtomicUsize::new(0));
        let be = backend_with_reapfn(/*root*/, reaps.clone());
        let s = SessionId::parse("s1").unwrap();
        be.configure_session(&s, &spec_with_cwd(/*root*/)).await.unwrap();
        let _stream = be.prompt(&s, vec![]).await.unwrap(); // hold the stream so the turn stays in-flight
        be.cancel(&s).await.unwrap();
        // the StubInner recorded the cancel; container reaped
        let guard = be.inflight.lock().await;
        // cancel removed the handle (or it will be removed by stream drop); assert at least the inner saw cancel
        drop(guard);
    }
```

> Strengthen the assertion by downcasting is overkill; instead make `CountingSpawn` return an `Arc<StubInner>` you also keep a clone of in the test (return it via a `Mutex<Option<Arc<StubInner>>>` on the spawn), then assert `stub.canceled.load(SeqCst)`.

- [ ] **Step 2: Run — verify fail.** `cargo test -p bridge-container cancel_reaches 2>&1 | tail`. Expected: `unimplemented!("Task 12")`.

- [ ] **Step 3: Implement** `cancel` + `retire`:

```rust
    async fn cancel(&self, session: &SessionId) -> Result<(), BridgeError> {
        let turn = self.inflight.lock().await.remove(session);
        if let Some(t) = turn {
            let _ = t.inner.cancel(session).await; // inner owns session/cancel + kill-on-grace
            self.reap_now(&t.name);
        }
        Ok(())
    }
    async fn retire(&self) -> Result<(), BridgeError> {
        let drained: Vec<InflightTurn> = self.inflight.lock().await.drain().map(|(_, t)| t).collect();
        for t in drained { self.reap_now(&t.name); }
        Ok(())
    }
```

- [ ] **Step 4: Run — verify pass.** `cargo test -p bridge-container cancel_reaches 2>&1 | tail`. Expected: PASS.

- [ ] **Step 5: Commit.** `git commit -am "feat(b2a): cancel routes to inner + reap; retire drains inflight"`

---

### Task 13: stable-identity boot-sweep (blocking-at-construction) + production reap_fn

**Files:** Modify `crates/bridge-container/src/lib.rs`.

- [ ] **Step 1: Write the failing test** (the sweep is injectable so it's Docker-free; assert it runs at construction with the owner-scoped name filter):

```rust
    #[tokio::test]
    async fn boot_sweep_runs_at_construction_with_owner_filter() {
        let swept = Arc::new(Mutex::new(Vec::<String>::new()));
        let s2 = swept.clone();
        let sweep_fn: SweepFn = Arc::new(move |filter: String| { let s2 = s2.clone(); Box::pin(async move { s2.lock().await.push(filter); }) });
        let _be = ContainerRwBackend::new_with_hooks(/* cfg */ test_cfg(), Arc::new(CountingSpawn{..}), Arc::new(AllowAll),
            "inst42".into(), SessionCwd::parse("/root").unwrap(), noop_reap_fn(), sweep_fn).await;
        let got = swept.lock().await.clone();
        assert_eq!(got, vec!["a2a-rw-inst42-".to_string()], "sweep filter is owner-scoped + blocking at construction");
    }
```

> Promote `new` to `async fn new_with_hooks(.., reap_fn, sweep_fn) -> Self` that AWAITS `sweep_fn(format!("a2a-rw-{}-", owner))` BEFORE returning (the blocking-at-construction invariant). Keep a thin `pub async fn new(..)` that supplies the production `reap_fn` (detached `docker rm -f` via `reap_argv` + `tokio::process::Command`) and the production `sweep_fn` (`docker ps -aq --filter name=<filter>` → `rm -f` each), then calls `new_with_hooks`. `SweepFn = Arc<dyn Fn(String) -> Pin<Box<dyn Future<Output=()> + Send>> + Send + Sync>`.

- [ ] **Step 2: Run — verify fail.** `cargo test -p bridge-container boot_sweep 2>&1 | tail`.

- [ ] **Step 3: Implement** `new_with_hooks` (await sweep first) + the production `new` with detached `reap_fn` + the `docker ps`/`rm` `sweep_fn`. Production reap_fn:

```rust
fn production_reap_fn() -> ReapFn {
    Arc::new(|runtime: String, name: String| {
        let (prog, argv) = bridge_core::sandbox::reap_argv(&runtime, &name);
        spawn_detached(async move {
            let _ = tokio::process::Command::new(prog).args(argv).output().await; // best-effort; rm -f of gone container is harmless
        });
    })
}
```

Production sweep_fn runs `docker ps -aq --filter name=<filter>` then `rm -f` the ids; tolerate a missing runtime (log, continue). Remove the temporary `#![allow(dead_code)]`.

- [ ] **Step 4: Run — verify pass + full crate suite.** Run: `cargo test -p bridge-container 2>&1 | tail`. Expected: all PASS.

- [ ] **Step 5: Commit.** `git commit -am "feat(b2a): stable-identity boot-sweep (blocking-at-construction) + production reap_fn"`

---

# Slice 3 — Wire both real `SpawnFn` arms

### Task 14: production `ContainerSpawn` + replace both stub arms

**Files:** Modify `bin/a2a-bridge/src/main.rs` (both closures), add a small production `ContainerSpawn` impl.

- [ ] **Step 1: Add a production `ContainerSpawn`** near `acp_program_argv` in `main.rs`:

```rust
struct AcpContainerSpawn;
#[async_trait::async_trait]
impl bridge_container::ContainerSpawn for AcpContainerSpawn {
    async fn spawn(&self, program: &str, argv: &[String], cfg: bridge_acp::acp_backend::AcpConfig)
        -> Result<std::sync::Arc<dyn bridge_core::ports::AgentBackend>, BridgeError> {
        let argv_ref: Vec<&str> = argv.iter().map(String::as_str).collect();
        let be = bridge_acp::acp_backend::AcpBackend::spawn(program, &argv_ref, cfg).await?;
        Ok(std::sync::Arc::new(be) as std::sync::Arc<dyn bridge_core::ports::AgentBackend>)
    }
}

/// Stable per-instance owner token: hash of the canonical config path + allowed_cwd_root.
fn container_owner(config_path: &std::path::Path, allowed_cwd_root: &str) -> String {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    config_path.canonicalize().unwrap_or_else(|_| config_path.to_path_buf()).hash(&mut h);
    allowed_cwd_root.hash(&mut h);
    format!("{:016x}", h.finish())
}
```

> The `with_policy` on `AcpBackend` is applied INSIDE `ContainerRwBackend` is NOT possible (the policy lives on the backend the bridge holds). Instead, thread the policy into the `AcpConfig`? No — `AcpBackend::with_policy` is a separate call. Apply it in the production `ContainerSpawn::spawn`: `let be = AcpBackend::spawn(..).await?.with_policy(policy)` — so pass the policy into `AcpContainerSpawn` (store an `Arc<dyn PolicyEngine>` field on it, constructed per-closure with the closure's `policy`).

- [ ] **Step 2: Replace the stub `ContainerRw` arm** in BOTH closures (run-workflow ~:198, serve ~:878) with:

```rust
                AgentKind::ContainerRw => {
                    let sb = entry.sandbox.clone().ok_or(BridgeError::ConfigInvalid {
                        reason: format!("container_rw agent {} requires sandbox", entry.id.as_str()),
                    })?;
                    let cmd = entry.cmd.clone().ok_or(BridgeError::ConfigInvalid {
                        reason: format!("container_rw agent {} requires cmd", entry.id.as_str()),
                    })?;
                    let allowed_root = /* the allowed_cwd_root string in scope for this closure */;
                    let cfg = bridge_container::ContainerRwConfig {
                        sandbox: sb, cmd, args: entry.args.clone(),
                        model: entry.model.clone(), mode: entry.mode.clone(),
                        auth_method: entry.auth_method.clone(),
                        handshake_timeout: bridge_acp::acp_backend::AcpConfig::default().handshake_timeout,
                        cancel_grace: bridge_acp::acp_backend::AcpConfig::default().cancel_grace,
                    };
                    let owner = container_owner(/* config_path */, &allowed_root);
                    let spawn = std::sync::Arc::new(AcpContainerSpawn::with_policy(Arc::clone(&policy)));
                    let be = bridge_container::ContainerRwBackend::new(
                        cfg, spawn, policy, owner,
                        bridge_core::SessionCwd::parse(&allowed_root).map_err(|_| BridgeError::ConfigInvalid {
                            reason: format!("allowed_cwd_root invalid for container_rw"),
                        })?,
                    ).await?;
                    Ok(std::sync::Arc::new(be) as std::sync::Arc<dyn bridge_core::ports::AgentBackend>)
                }
```

> Locate `allowed_cwd_root` and the config path in each closure's captured scope (the serve path loads it near `main.rs:1024`/the snapshot; run-workflow has `--config`). If `allowed_cwd_root` isn't already captured, capture it when building the closure (clone into the `move` closure) — the registry snapshot carries it. If absent (no `[registry] allowed_cwd_root`), reject: `container_rw` requires it (it's the containment anchor).

- [ ] **Step 3: Run — verify the workspace compiles + tests green.** Run: `cargo build --workspace && cargo test --workspace 2>&1 | tail -15`. Expected: PASS.

- [ ] **Step 4: fmt + clippy.** `cargo fmt && cargo clippy --workspace --all-targets 2>&1 | tail`. Expected: clean.

- [ ] **Step 5: Commit.** `git commit -am "feat(b2a): wire ContainerRw arm in both SpawnFn closures + production ContainerSpawn"`

---

### Task 15: coverage gate

- [ ] **Step 1: Run coverage.** Run: `cargo llvm-cov clean --workspace && cargo llvm-cov --workspace 2>&1 | tail -20`.
- [ ] **Step 2: Verify floors** — workspace ≥ 85, bridge-core ≥ 90. If `bridge-container` drags the workspace below 85, add unit tests for any uncovered branch (e.g. `retire`, the `canonicalize_lenient` nearest-ancestor path, the inner.prompt-error reap path). Re-run.
- [ ] **Step 3: Commit** any added tests. `git commit -am "test(b2a): cover container backend branches to floor"`

---

# Slice 4 — Live acceptance gate (Docker, operator-run)

### Task 16: example config + the live gate

**Files:** Modify `examples/a2a-bridge.containerized.toml`.

- [ ] **Step 1: Add a `container_rw` agent** to the containerized example (mirror the `[sandbox]` of the `:ro` agents but `kind="container_rw"`, `access="rw"`):

```toml
[[agents]]
id   = "impl"
kind = "container_rw"
cmd  = "claude-agent-acp"
  [agents.sandbox]
  image  = "a2a-agent-reader:latest"
  mount  = "/Users/wesleyjinks/code"
  access = "rw"
  egress = "locked"
  network = "a2a-egress-internal"
  proxy   = "http://a2a-egress-proxy:8888"
  volumes = ["/Users/wesleyjinks/.config/a2a-creds/claude/.credentials.json:/root/.claude/.credentials.json"]
```

- [ ] **Step 2: Pre-create the scratch dir host-owned** (so the `:rw` write isn't root-owned — see spec gate note):

```bash
mkdir -p /Users/wesleyjinks/code/.b2a-scratch && chmod u+rwx /Users/wesleyjinks/code/.b2a-scratch
deploy/containers/sync-creds.sh claude
docker compose -f deploy/containers/compose.egress.yaml up -d   # proxy
```

- [ ] **Step 3: Serve + drive a write turn via A2A** (the only path that can set a per-request cwd):

```bash
cargo build --release
( ./target/release/a2a-bridge serve --config examples/a2a-bridge.containerized.toml & echo $! > /tmp/b2a-serve.pid )
sleep 2
curl -sS -X POST http://127.0.0.1:8080/ -H 'content-type: application/json' -H 'A2A-Version: 1.0' -d '{
  "jsonrpc":"2.0","id":1,"method":"SendMessage",
  "params":{"message":{"role":"user",
    "parts":[{"kind":"text","text":"Create the file /Users/wesleyjinks/code/.b2a-scratch/B2A_OK.txt containing exactly B2A_OK and then STOP. Do nothing else."}],
    "metadata":{"a2a-bridge.cwd":"/Users/wesleyjinks/code/.b2a-scratch","a2a-bridge.agent":"impl"}}}}'
```

- [ ] **Step 4: Assert the gate** (all must hold):

```bash
# (a) file persists on host:
test -f /Users/wesleyjinks/code/.b2a-scratch/B2A_OK.txt && echo "PERSIST_OK"
# (b) per-turn reap: no leftover named container after the turn:
docker ps -a --filter name=a2a-rw- --format '{{.Names}}' | grep . && echo "LEAK" || echo "REAPED_OK"
# (c) positive containment DURING a turn (run a 2nd turn and snapshot mid-flight in another shell):
#     docker ps --filter name=a2a-rw- --format '{{.ID}} {{.Image}}'  → exactly one a2a-agent-reader container
# (d) distinct-per-turn identity: capture the container id on two turns; they MUST differ.
# (e) cancel mid-turn: start a long turn, send tasks/cancel, assert the named container is gone within the grace.
kill "$(cat /tmp/b2a-serve.pid)"
```

Expected: `PERSIST_OK`, `REAPED_OK`, one contained `a2a-agent-reader` mid-turn (NOT a host process), two distinct ids across turns, and cancel reaps. **If `PERSIST_OK` fails, check scratch-dir ownership first (root-owned bind mount), not logic.**

- [ ] **Step 5: Commit the example.** `git add examples/a2a-bridge.containerized.toml && git commit -m "feat(b2a): containerized example container_rw agent + acceptance gate"`

---

## Self-Review

**1. Spec coverage:** AgentKind+parse (T1) ✓ · validate matrix + S3/S5/S6 reuse via `validate_sandbox` (T2) ✓ · compose_container_rw/reap_argv/check_rw_target (T4–6) ✓ · per-turn backend w/ ContainerSpawn seam (T7) ✓ · configure stash + forget stash-only (T8) ✓ · both-sides canonicalization (T9) ✓ · strict-reject + spawn-fail reap + configure-forward + inflight + reject-2nd (T10) ✓ · detached non-blocking reaper + stream-owns-reaper (T11) ✓ · cancel via inflight + reap, retire (T12) ✓ · stable-owner blocking boot-sweep (T13) ✓ · both SpawnFn arms (T3 stub, T14 real) ✓ · identity-asserting + cancel + ownership gate (T16) ✓. Reuse predicate already keys `kind`+`sandbox`+`session_cwd` (registry.rs:326) — no change, noted.

**2. Placeholder scan:** Two forward-references are explicit and sequenced (T10 → `reap_now`/`wrap_with_reaper` land in T11; struct `inflight: Arc<Mutex<..>>` tweak called out in T10). The `allowed_cwd_root`/config-path capture in T14 is a "locate in scope" instruction, not a code placeholder — it has a concrete fallback (reject if absent). No `TODO`/`add error handling`/`similar to` placeholders.

**3. Type consistency:** `ContainerRwConfig` fields (sandbox/cmd/args/model/mode/auth_method/handshake_timeout/cancel_grace) match T7 ↔ T10 ↔ T14. `AcpConfig` fields match acp_backend.rs:68 (cwd/model/mode/auth_method/handshake_timeout/cancel_grace). `ContainerSpawn::spawn(&self, program, argv, cfg)` matches T7 ↔ T10 ↔ T14. `reap_fn: ReapFn(runtime, name)` consistent T10/11/13. `check_rw_target(mount_canon, rw_canon)` pure in bridge-core, called by backend `resolve_rw_target` (T6 ↔ T9).

**Open item for the plan-review to weigh:** the `with_policy` threading in T14 (the inner `AcpBackend` needs the policy; applied in `AcpContainerSpawn::with_policy`) — confirm this matches how the existing `Acp` arm applies `with_policy` (main.rs:211) and that `ContainerRwBackend` doesn't also need the policy (it only forwards to the inner). If the inner never raises permission asks under AutoPolicy, the policy field on the backend may be droppable — leave it for parity, flag for the reviewer.

---

## Plan rev2 — dual-review corrections (BINDING; override the body where they conflict)

Both plan-reviews (containerized dogfood primary + a2a-local codex `gpt-5.5` backstop) returned
**needs-changes**, and **both** confirmed the DESIGN + spec→task spine are SOUND — the defects are
decomposition/placeholder/compile issues. Apply ALL of R1–R24 during the build.

### Structural (apply before the Slice-2 tasks)
- **R1 — One constructor, defined once in Task 7/8, never churned.** `pub async fn new_with_hooks(cfg: ContainerRwConfig, spawn: Arc<dyn ContainerSpawn>, owner: String, reap_fn: ReapFn, sweep_fn: SweepFn) -> Result<Self, BridgeError>` (AWAITS the boot-sweep before returning) + a thin `pub async fn new(cfg, spawn, owner) -> Result<Self, BridgeError>` supplying `production_reap_fn(runtime)` + `production_sweep_fn(runtime)`. EVERY test helper calls `new_with_hooks` with a no-op `sweep_fn` + a counting `reap_fn`. No later task changes the signature; Task 14 uses `.await?`. (codex B3 / dogfood B3)
- **R2 — Reorder Slice 2 so the reaper infra lands BEFORE `prompt`.** New order: T7 crate+seam+struct → T8 constructor+configure/forget → **T9 `ContainerReaper`+`ReapFn`+`SweepFn`+`spawn_detached`+`wrap_with_reaper`+`reap_now`** → T10 canonicalizing guard → T11 `prompt` mint (uses the REAL reaper, correct arity) → T12 cancel/retire → T13 production boot-sweep+reap wiring. `prompt` then references no undefined symbols. (codex B4 / dogfood B4)
- **R3 — No dead fields.** `ContainerRwBackend` has NO `policy` and NO `allowed_cwd_root` field. Containment anchor = `cfg.sandbox.mount` (S2 == normalized allowed_cwd_root); policy lives only on `AcpContainerSpawn`. Remove the temporary `#![allow(dead_code)]`; add `-- -D warnings` to every clippy step (CI enforces it, ci.yml:11/45). (dogfood M2 / codex nit2)

### Correctness (subtle — NOT compiler-caught; do not skip)
- **R4 — `bridge_core::domain::PermissionDecision::Approve`** (not `ports::Allow`) in the `AllowAll` test stub. (codex B1)
- **R5 — Owner includes the AGENT ID.** `container_owner(config_path, mount, agent_id)` hashes all three, so two `container_rw` agents never both mint `a2a-rw-<owner>-0` nor cross-reap. (codex B4)
- **R6 — Forward the CANONICAL cwd to the inner.** AcpBackend prefers the stashed `SessionSpec.cwd` over `AcpConfig.cwd` (acp_backend.rs:889). Before `inner.configure_session`, clone the spec and set `cwd = Some(rw_canon.clone())` so the ACP session cwd == the mounted path. (codex B5)
- **R7 — Atomic check-and-reserve.** Under ONE `inflight` lock: present → reject; else insert a reservation; release; spawn; on success fill, on any failure remove. Model `inflight: Mutex<HashMap<SessionId, InflightState>>` = `Reserving | Live(InflightTurn)`. No separate `contains_key`+`insert`. (codex B6)
- **R8 — One shared `reaped` across cancel + stream-drop.** `InflightTurn` and its `ContainerReaper` share ONE `Arc<AtomicBool>`; whoever reaps first wins, the other is a no-op. (codex B7)
- **R9 — Reap on `configure_session`/`prompt` failure.** Any `?` between a successful spawn and the reaper-owning stream must `reap_now(&name)` + clear the reservation before returning. (codex B8)
- **R10 — Runtime-parametric sweep + reap.** Production `sweep_fn`/`reap_fn` use `cfg.sandbox.runtime()` (docker|podman), never hardcoded `docker`; add a timeout + `agent_stderr` logging. (dogfood M4 / codex SF6)

### Wiring (Slice 3 / Task 14, split 14a spawn+owner / 14b replace-arms)
- **R11 — Anchor from `entry.sandbox.mount`, NOT `allowed_cwd_root`.** The field is absent from `RegistrySnapshot` AND out of scope in both closures (serve: cfg parsed at :953 AFTER the closure at :856; run-workflow: cfg consumed by `into_snapshot()` at :172 BEFORE the closure at :177). S2 guarantees `entry.sandbox.mount` == normalized `allowed_cwd_root`. `config_path` IS in scope before both closures (serve :837, run-workflow :145). Delete the false "snapshot carries it" line and the `[registry] allowed_cwd_root` nit (it's top-level; moot now). (dogfood B1 / codex B2,nit3)
- **R12 — Concrete `AcpContainerSpawn { policy: Arc<dyn PolicyEngine> }`** + `fn with_policy(policy) -> Self`; apply `.with_policy(policy)` to the inner `AcpBackend` INSIDE `spawn` (matches main.rs:211). Not a unit struct. (both)

### Test rigor (no vacuous or threshold-gated tests)
- **R13 — Assert the reap FIRED on spawn-failure** via the `reap_fn` counter (not just empty `inflight`). (both M1)
- **R14 — Dedicated failing-test steps** (not the Task-15 threshold backfill) for: `canonicalize_lenient` nearest-existing-ancestor on a not-yet-existing scratch dir (T10); the `inner.prompt`-error reap path (T11); `retire` cancel+reap (T12); off-runtime `ContainerReaper` Drop (T9). (dogfood M3 / codex SF4,SF5)
- **R15 — Backend-level argv assertion** (`:rw` mount / no `:ro`) in the prompt mint test, not only the pure composer. (codex SF3)
- **R16 — Fix invalid commands:** Task 1 → `cargo test -p a2a-bridge parse_kind` (bin has NO lib target — drop `--lib`); Task 11 → one bare filter per command. (codex SF7 / dogfood M7)
- **R17 — Tasks 1–3 are ONE green commit** (workspace red only transiently between T1–T3; commit at T3). State it; no red checkpoint. (dogfood M5)
- **R18 — `retire` cancels THEN reaps** each inflight (graceful `session/cancel` before `rm -f`). (both N2/SF1)

### New tasks folded per the run-workflow cwd learning (memory: workflow-cwd-cleanroom-gotcha)
- **R19 — Task 17: `run-workflow --session-cwd <dir>`** — parse the flag, validate via `SessionCwd::parse`, thread into `WorkflowRunContext { session_cwd: Some(..) }` (use `run_with_context`). Fixes the agents-get-launch-cwd gap AND lets the B2a gate run via run-workflow (the "both paths" the gate dropped to serve-only). TDD: a unit test that the parsed flag reaches the context (mirror executor.rs:1363's cwd-threading test). codex B2's missing-cwd path for run-workflow closes here.
- **R20 — Task 18: brief-only clean-room `design`/review prompts** — config/prompt-only edit to the workflow node prompts in `examples/*.toml`: "work FROM the inlined brief/diff; repo access is optional context, its absence is NOT a failure — never bail for missing files." No Rust.
- **R21 — Task 19: doc the per-turn memory-loss asymmetry** in `docs/containerized-agents.md` (per-turn `serve` loses conversational memory vs the warm `:ro` reader; spec decision #1). (dogfood M6 / codex SF8)

### Self-review correction
- **R22 —** the body's "no placeholders" self-check was FALSE (Tasks 10–14 had forward-ref placeholders); R1–R12 remove them. The concrete test helpers (`test_cfg()`, `noop_reap_fn()`, full `CountingSpawn` literal, `backend_with_reapfn` with a real `tempdir` root, `spec_with_cwd` with a real root) MUST be written out, not left as `/* ... */`. (codex N4 / dogfood N4,B5)

**Both verdicts:** design + spine sound; R1–R22 are decomposition/placeholder/compile fixes. After applying them the plan builds green-per-task; **no third plan-review needed** — the inline TDD build (compiler + tests per task) is the verification.
