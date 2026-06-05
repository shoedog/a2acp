# Slice B1 — Enforced `[sandbox]` Block Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Turn the operator-typed `cmd="docker" args=[…]` into a **declared, bridge-composed, bridge-enforced** `[sandbox]` block for the `:ro`/Acp readers — the warm path, no new `AgentKind`, no per-task factory (those are B2).

**Architecture:** A `SandboxConfig` on `AgentEntry` + a **pure, total** `compose_sandbox` in `bridge-core/src/sandbox.rs` + **two-layer** validation (parse: S0/S2 in `config.rs::into_snapshot`; snapshot: S1/S3/S4/S5/S6 in `registry.rs::validate`) + wiring at **both** `SpawnFn` closures. `EgressPolicy` carries its data so `compose_sandbox` is total. `:rw` is rejected (B2).

**Tech Stack:** Rust (bridge-core / bridge-registry / bin/a2a-bridge), serde/TOML, the existing `SessionCwd` normalizer, Docker (acceptance gate only).

**Branch:** `feat/sandbox-block` (exists; spec committed). **Commit trailers:** the plan + ADR carry `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`; TDD task/code commits do NOT.

**Spec:** `docs/superpowers/specs/2026-06-05-containerized-agents-slice-b1-design.md` (dual-reviewed + dogfood-folded).

---

## File Structure

**Create:**
- `crates/bridge-core/src/sandbox.rs` — the pure `compose_sandbox` + its unit tests.

**Modify:**
- `crates/bridge-core/src/domain.rs` — `SandboxConfig`/`MountAccess`/`EgressPolicy` + `sandbox` field on `AgentEntry`.
- `crates/bridge-core/src/lib.rs` — `pub mod sandbox;`.
- `crates/bridge-registry/src/registry.rs` — `validate` invariants S1/S3/S4/S5/S6 + reuse predicate.
- `bin/a2a-bridge/src/config.rs` — `SandboxToml`, `parse_access`/`parse_egress`, `into_snapshot` S0/S2.
- `bin/a2a-bridge/src/main.rs` — both `SpawnFn` closures (compose-or-raw).
- `examples/a2a-bridge.containerized.toml` — migrate the 3 readers.
- ~14-15 `AgentEntry { … }` construction sites — add `sandbox: None`.

**No new crate** (B2 adds `bridge-container`). **Slices 1–4 are pure Rust / no Docker.**

---

## Task 1: Domain types + the `sandbox: None` ripple

**Files:**
- Modify: `crates/bridge-core/src/domain.rs`

- [ ] **Step 1: Write the failing test** (append to the `#[cfg(test)] mod tests` in domain.rs)

```rust
#[test]
fn sandbox_runtime_defaults_to_docker() {
    let sb = SandboxConfig {
        runtime: None,
        image: "img".into(),
        mount: "/work".into(),
        access: MountAccess::Ro,
        egress: EgressPolicy::Open,
        volumes: vec![],
    };
    assert_eq!(sb.runtime(), "docker");
    let sb2 = SandboxConfig { runtime: Some("podman".into()), ..sb };
    assert_eq!(sb2.runtime(), "podman");
}
```

- [ ] **Step 2: Run it — verify it fails to COMPILE** (types don't exist yet)

Run: `cargo test -p bridge-core sandbox_runtime_defaults_to_docker 2>&1 | tail -5`
Expected: compile error `cannot find type SandboxConfig`.

- [ ] **Step 3: Add the types** (in domain.rs, near `AgentEntry`)

```rust
/// How a containerized agent is launched (the enforced `[sandbox]` block). The bridge composes the
/// runtime argv from this (see `crate::sandbox::compose_sandbox`) and `validate()`/`into_snapshot`
/// enforce its invariants, so containment can't silently degrade via hand-typed args.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SandboxConfig {
    pub runtime: Option<String>, // resolve via `runtime()`; "docker" default. examples: docker|podman
    pub image: String,
    pub mount: String,           // primary identical-path source mount; MUST == allowed_cwd_root (S2)
    pub access: MountAccess,     // Ro | Rw (Rw rejected in B1 — S4)
    pub egress: EgressPolicy,    // data-carrying → compose is total
    pub volumes: Vec<String>,    // verbatim extra `-v` specs (creds / named vols); trusted passthrough
}

impl SandboxConfig {
    /// The resolved container runtime program (default `docker`). S3 allowlists THIS exact value, and
    /// `compose_sandbox` spawns THIS — a single source of truth so validate and spawn can't drift.
    pub fn runtime(&self) -> &str {
        self.runtime.as_deref().unwrap_or("docker")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MountAccess {
    Ro,
    Rw,
}

/// `Locked` CARRIES its network/proxy so `compose_sandbox` is total (no `unwrap`/panic) and the old
/// runtime "Locked ⇒ network+proxy" invariant is a TYPE guarantee. The TOML→enum conversion
/// (`config.rs::parse_egress`) is the only constructor and rejects `locked` without both.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EgressPolicy {
    Locked {
        network: String,
        proxy: String,
        no_proxy: Option<String>,
    },
    Open,
}
```

- [ ] **Step 4: Add the field to `AgentEntry`** (between `session_cwd` and `auth_method`)

```rust
    pub session_cwd: Option<String>,
    pub sandbox: Option<SandboxConfig>,
    pub auth_method: Option<String>,
```

- [ ] **Step 5: Run the test — verify it PASSES, but the crate's OTHER construction sites now fail to compile**

Run: `cargo build -p bridge-core 2>&1 | grep -E "missing field .sandbox.|error" | head`
Expected: `missing field sandbox in initializer of AgentEntry` at the in-crate test sites (domain.rs:247/274/299).

- [ ] **Step 6: Fix the in-crate construction sites** — add `sandbox: None,` to every `AgentEntry { … }` in domain.rs (the test builders ~247/274/299). Then `cargo test -p bridge-core sandbox_runtime_defaults_to_docker` → PASS.

- [ ] **Step 7: Ripple — add `sandbox: None` to every OTHER real construction site**

Run a workspace build to find them all (the compiler is exhaustive — `AgentEntry` has no `Default`):
```bash
cargo build --workspace 2>&1 | grep -E "missing field .sandbox" | sed -E 's/.*--> //' | sort -u
```
Expected sites (~14-15): `registry.rs:369/423`, `config.rs:315`, `route.rs:109`, `e2e_registry.rs:225/558/614`, `common/mod.rs:23`, `server.rs:3133/5394`, `workflow_producer.rs:39`, `executor.rs:391`. Add `sandbox: None,` to each. **DO NOT touch `bin/a2a-bridge/tests/integration_run_workflow.rs:86`** — that's a LOCAL test-double `struct AgentEntry`, not the domain type (it has its own fields).

- [ ] **Step 8: Run the full build + tests — all green**

Run: `cargo build --workspace 2>&1 | tail -3 && cargo test -p bridge-core 2>&1 | grep "test result"`
Expected: builds clean; bridge-core tests pass (incl. the new one).

- [ ] **Step 9: Commit**

```bash
git add crates/bridge-core/src/domain.rs crates/bridge-registry/src/registry.rs bin/a2a-bridge/src/config.rs crates/bridge-core/src/route.rs crates/bridge-registry/tests/e2e_registry.rs crates/bridge-a2a-inbound/tests/common/mod.rs crates/bridge-a2a-inbound/src/server.rs crates/bridge-a2a-inbound/tests/workflow_producer.rs crates/bridge-workflow/src/executor.rs
git commit -m "feat(core): SandboxConfig/MountAccess/EgressPolicy + AgentEntry.sandbox field + None ripple"
```
(Exact paths come from Step 7's output — `git add -u` after editing is fine.)

---

## Task 2: `compose_sandbox` (pure, total)

**Files:**
- Create: `crates/bridge-core/src/sandbox.rs`
- Modify: `crates/bridge-core/src/lib.rs` (add `pub mod sandbox;`)

- [ ] **Step 1: Register the module** — add `pub mod sandbox;` to `crates/bridge-core/src/lib.rs` (alphabetical with the other `pub mod`s).

- [ ] **Step 2: Write the failing tests** (`crates/bridge-core/src/sandbox.rs`)

```rust
//! Pure, total composition of a container runtime argv from a `SandboxConfig`. No Docker, no I/O.

use crate::domain::{EgressPolicy, MountAccess, SandboxConfig};

#[cfg(test)]
mod tests {
    use super::*;

    fn ro_locked() -> SandboxConfig {
        SandboxConfig {
            runtime: None,
            image: "a2a-agent-reader:latest".into(),
            mount: "/Users/w/code".into(),
            access: MountAccess::Ro,
            egress: EgressPolicy::Locked {
                network: "a2a-egress-internal".into(),
                proxy: "http://a2a-egress-proxy:8888".into(),
                no_proxy: None,
            },
            volumes: vec!["/host/creds:/root/.codex/auth.json".into()],
        }
    }

    #[test]
    fn ro_locked_argv_shape() {
        let (program, argv) = compose_sandbox(&ro_locked(), "codex-acp", &[]);
        assert_eq!(program, "docker");
        assert_eq!(
            argv,
            vec![
                "run", "-i", "--rm",
                "--network", "a2a-egress-internal",
                "-e", "HTTPS_PROXY=http://a2a-egress-proxy:8888",
                "-e", "HTTP_PROXY=http://a2a-egress-proxy:8888",
                "-v", "/Users/w/code:/Users/w/code:ro",
                "-v", "/host/creds:/root/.codex/auth.json",
                "a2a-agent-reader:latest",
                "codex-acp",
            ]
        );
    }

    #[test]
    fn open_emits_no_egress_flags() {
        let mut sb = ro_locked();
        sb.egress = EgressPolicy::Open;
        let (_p, argv) = compose_sandbox(&sb, "claude-agent-acp", &[]);
        assert!(!argv.iter().any(|a| a == "--network" || a.starts_with("HTTPS_PROXY")));
        assert!(argv.contains(&"-v".to_string()));
    }

    #[test]
    fn no_proxy_emitted_when_set() {
        let mut sb = ro_locked();
        sb.egress = EgressPolicy::Locked {
            network: "n".into(), proxy: "p".into(), no_proxy: Some("localhost,127.0.0.1".into()),
        };
        let (_p, argv) = compose_sandbox(&sb, "kiro-cli", &["acp".into()]);
        assert!(argv.windows(2).any(|w| w[0] == "-e" && w[1] == "NO_PROXY=localhost,127.0.0.1"));
        // agent args tail through after image:
        assert_eq!(argv.last().unwrap(), "acp");
    }

    #[test]
    fn rw_emits_no_ro_suffix() {
        let mut sb = ro_locked();
        sb.access = MountAccess::Rw;
        let (_p, argv) = compose_sandbox(&sb, "x", &[]);
        assert!(argv.windows(2).any(|w| w[0] == "-v" && w[1] == "/Users/w/code:/Users/w/code"));
    }

    #[test]
    fn runtime_override_and_default() {
        let mut sb = ro_locked();
        sb.runtime = Some("podman".into());
        assert_eq!(compose_sandbox(&sb, "x", &[]).0, "podman");
        sb.runtime = None;
        assert_eq!(compose_sandbox(&sb, "x", &[]).0, "docker");
    }
}
```

- [ ] **Step 3: Run — verify FAIL** (`compose_sandbox` undefined)

Run: `cargo test -p bridge-core --lib sandbox 2>&1 | tail -5`
Expected: `cannot find function compose_sandbox`.

- [ ] **Step 4: Implement** (top of `sandbox.rs`, above the test module)

```rust
/// Expand a `[sandbox]` declaration into `(runtime program, argv)`. PURE + TOTAL — the egress data
/// lives in the `EgressPolicy` variant, so no `unwrap`/panic. NO cwd / `--workdir`: the identical-path
/// `:ro` mount makes the ACP `session/new` cwd resolve in-container (Slice A).
pub fn compose_sandbox(
    sb: &SandboxConfig,
    agent_cmd: &str,
    agent_args: &[String],
) -> (String, Vec<String>) {
    let mut argv: Vec<String> = vec!["run".into(), "-i".into(), "--rm".into()];

    if let EgressPolicy::Locked { network, proxy, no_proxy } = &sb.egress {
        argv.push("--network".into());
        argv.push(network.clone());
        argv.push("-e".into());
        argv.push(format!("HTTPS_PROXY={proxy}"));
        argv.push("-e".into());
        argv.push(format!("HTTP_PROXY={proxy}"));
        if let Some(np) = no_proxy {
            argv.push("-e".into());
            argv.push(format!("NO_PROXY={np}"));
        }
    }

    // Primary identical-path source mount; `:ro` derived from the validated access (S4 rejects Rw in B1).
    let ro = matches!(sb.access, MountAccess::Ro);
    argv.push("-v".into());
    argv.push(format!("{m}:{m}{suffix}", m = sb.mount, suffix = if ro { ":ro" } else { "" }));

    // Extra volumes (creds / named vols) verbatim. S6 (validate) guarantees none nests under `mount`.
    for v in &sb.volumes {
        argv.push("-v".into());
        argv.push(v.clone());
    }

    argv.push(sb.image.clone());
    argv.push(agent_cmd.to_string());
    argv.extend(agent_args.iter().cloned());

    (sb.runtime().to_string(), argv)
}
```

- [ ] **Step 5: Run — verify PASS**

Run: `cargo test -p bridge-core --lib sandbox 2>&1 | grep "test result"`
Expected: `ok. 5 passed`.

- [ ] **Step 6: Commit**

```bash
git add crates/bridge-core/src/sandbox.rs crates/bridge-core/src/lib.rs
git commit -m "feat(core): pure total compose_sandbox (runtime argv from SandboxConfig)"
```

---

## Task 3: `validate` snapshot invariants (S1/S3/S4/S5/S6) + reuse predicate

**Files:**
- Modify: `crates/bridge-registry/src/registry.rs`

- [ ] **Step 1: Write the failing tests** (in registry.rs `#[cfg(test)] mod tests`; build a sandboxed entry helper)

```rust
fn sandboxed_entry(id: &str, access: bridge_core::domain::MountAccess, vols: Vec<String>) -> AgentEntry {
    use bridge_core::domain::{EgressPolicy, SandboxConfig};
    let mut e = entry(id); // existing helper: kind=Acp, cmd=Some("fake-cmd")
    e.cmd = Some("claude-agent-acp".into());
    e.sandbox = Some(SandboxConfig {
        runtime: Some("docker".into()),
        image: "img".into(),
        mount: "/work".into(),
        access,
        egress: EgressPolicy::Open,
        volumes: vols,
    });
    e
}

#[test]
fn s3_allowlists_runtime_not_inner_cmd() {
    use bridge_core::domain::MountAccess;
    // allowed_cmds has "docker" (the runtime), NOT "claude-agent-acp" (the inner cli) → must pass.
    let mut snap = snapshot(&["a"]);
    snap.entries = vec![sandboxed_entry("a", MountAccess::Ro, vec![])];
    snap.allowed_cmds = vec!["docker".into()];
    assert!(validate(&snap).is_ok());
    // runtime not allowlisted → reject
    snap.allowed_cmds = vec!["podman".into()];
    assert!(validate(&snap).is_err());
}

#[test]
fn s4_rejects_rw_in_b1() {
    use bridge_core::domain::MountAccess;
    let mut snap = snapshot(&["a"]);
    snap.entries = vec![sandboxed_entry("a", MountAccess::Rw, vec![])];
    snap.allowed_cmds = vec!["docker".into()];
    assert!(validate(&snap).is_err());
}

#[test]
fn s6_rejects_volume_nested_under_mount() {
    use bridge_core::domain::MountAccess;
    let mut snap = snapshot(&["a"]);
    // dest /work/secret is nested under the :ro mount /work → re-exposes the repo rw → REJECT
    snap.entries = vec![sandboxed_entry("a", MountAccess::Ro, vec!["/h:/work/secret".into()])];
    snap.allowed_cmds = vec!["docker".into()];
    assert!(validate(&snap).is_err());
    // a creds vol OUTSIDE the tree passes
    snap.entries = vec![sandboxed_entry("a", MountAccess::Ro, vec!["/h:/root/.codex/auth.json".into()])];
    assert!(validate(&snap).is_ok());
}

#[test]
fn s1_api_must_not_set_sandbox() {
    use bridge_core::domain::{EgressPolicy, MountAccess, SandboxConfig};
    let mut snap = api_snap(); // existing helper: kind=Api
    snap.entries[0].sandbox = Some(SandboxConfig {
        runtime: None, image: "i".into(), mount: "/work".into(),
        access: MountAccess::Ro, egress: EgressPolicy::Open, volumes: vec![],
    });
    assert!(validate(&snap).is_err());
}
```

- [ ] **Step 2: Run — verify FAIL** (validate doesn't enforce these yet — `s4_rejects_rw` and `s6` and `s1` fail; `s3` may pass-by-accident on the cmd check)

Run: `cargo test -p bridge-registry s4_rejects_rw_in_b1 s6_rejects_volume s1_api_must_not 2>&1 | grep -E "FAILED|test result"`
Expected: failures.

- [ ] **Step 3: Add a path-prefix helper + the invariants in `validate`'s Acp/Api arms**

Add a free helper near `validate`:
```rust
/// True if container path `dest` equals or is nested under `mount` (component-wise, so `/work` does
/// NOT match `/work2`). Both are absolute identical-path container paths.
fn dest_under_or_eq(dest: &str, mount: &str) -> bool {
    use std::path::Path;
    let (d, m) = (Path::new(dest), Path::new(mount));
    d == m || d.starts_with(m)
}
```
Replace the `AgentKind::Acp` arm body with a sandbox branch:
```rust
AgentKind::Acp => {
    let Some(_cmd) = e.cmd.as_deref() else {
        return Err(BridgeError::ConfigInvalid {
            reason: format!("acp agent {} requires cmd", e.id.as_str()),
        });
    };
    match &e.sandbox {
        Some(sb) => {
            // S3: allowlist the RESOLVED RUNTIME (not the inner agent cli, which runs contained).
            let runtime = sb.runtime();
            if !snap.allowed_cmds.iter().any(|c| c == runtime) {
                return Err(BridgeError::ConfigInvalid {
                    reason: format!("sandbox runtime not allowed: {runtime}"),
                });
            }
            // S4: :rw requires the container_rw kind (Slice B2).
            if sb.access == bridge_core::domain::MountAccess::Rw {
                return Err(BridgeError::ConfigInvalid {
                    reason: format!("sandbox agent {} access=rw requires the container_rw kind (Slice B2)", e.id.as_str()),
                });
            }
            // S5: mount must be an absolute/normalized path.
            bridge_core::session_cwd::SessionCwd::parse(&sb.mount).map_err(|_| {
                BridgeError::ConfigInvalid { reason: format!("sandbox mount must be an absolute path: {}", sb.mount) }
            })?;
            // S6: no volume DEST equal-to/nested-under `mount` (would re-expose the :ro repo rw).
            for v in &sb.volumes {
                let dest = v.split(':').nth(1).unwrap_or("");
                if dest_under_or_eq(dest, &sb.mount) {
                    return Err(BridgeError::ConfigInvalid {
                        reason: format!("sandbox volume dest {dest:?} is nested under the :ro mount {:?}", sb.mount),
                    });
                }
            }
        }
        None => {
            let cmd = e.cmd.as_deref().unwrap();
            if !snap.allowed_cmds.iter().any(|c| c == cmd) {
                return Err(BridgeError::ConfigInvalid { reason: format!("cmd not allowed: {cmd}") });
            }
        }
    }
}
```
And in the `AgentKind::Api` arm, after the existing checks, add **S1**:
```rust
                if e.sandbox.is_some() {
                    return Err(BridgeError::ConfigInvalid {
                        reason: format!("api agent {} must not set sandbox", e.id.as_str()),
                    });
                }
```
(Confirm `SessionCwd::parse` is the real path — adjust the `use` path if it differs; the spec cites `session_cwd.rs:24-41`.)

- [ ] **Step 4: Run — verify PASS**

Run: `cargo test -p bridge-registry s1_api s3_allowlists s4_rejects s6_rejects 2>&1 | grep "test result"`
Expected: all pass.

- [ ] **Step 5: Reuse predicate — failing test then fix**

Add the test:
```rust
#[tokio::test]
async fn sandbox_or_session_cwd_or_api_key_change_forces_new_slot() {
    let count = Arc::new(AtomicUsize::new(0));
    let retired = Arc::new(AtomicUsize::new(0));
    let reg = Registry::new(snapshot(&["a"]), counting_spawn_recording(count.clone(), 0, retired.clone())).unwrap();
    let a = AgentId::parse("a").unwrap();
    let _r = reg.resolve(&a).await.unwrap();
    let before = reg.slot_arc(&a).unwrap();
    // change session_cwd → must be a NEW slot (was silently reused before)
    let mut snap = snapshot(&["a"]);
    snap.entries[0].session_cwd = Some("/work/x".into());
    reg.apply(snap).await.unwrap();
    assert!(!Arc::ptr_eq(&before, &reg.slot_arc(&a).unwrap()));
}
```
Run it → FAIL (slot reused). Then extend the predicate at `registry.rs:264-272`:
```rust
            c.cmd == e.cmd
                && c.base_url == e.base_url
                && c.args == e.args
                && c.cwd == e.cwd
                && c.auth_method == e.auth_method
                && c.kind == e.kind
                && c.sandbox == e.sandbox
                && c.session_cwd == e.session_cwd
                && c.api_key_env == e.api_key_env
```
Run → PASS. **BEHAVIOR CHANGE (note in the commit):** a hot-edit of `session_cwd`/`api_key_env`/`sandbox` now drains + respawns the warm backend (previously silently ignored). Correct for `:ro` readers; mechanism: `session_cwd`→`AcpConfig.cwd`, `api_key_env`→`ApiConfig`, both frozen at spawn.

- [ ] **Step 6: Commit**

```bash
git add crates/bridge-registry/src/registry.rs
git commit -m "feat(registry): sandbox validate invariants S1/S3/S4/S5/S6 + all-three reuse-key fix (BEHAVIOR CHANGE: session_cwd/api_key_env edits now respawn)"
```

---

## Task 4: `config.rs::into_snapshot` parse layer (SandboxToml + S0 + S2 + EgressPolicy conversion)

**Files:**
- Modify: `bin/a2a-bridge/src/config.rs`

- [ ] **Step 1: Write the failing tests** (config.rs test module)

```rust
#[test]
fn sandbox_mount_must_equal_allowed_cwd_root() {
    let toml = r#"
default = "a"
allowed_cwd_root = "/work"
[[agents]]
id = "a"
cmd = "claude-agent-acp"
[agents.sandbox]
image = "img"
mount = "/work"
access = "ro"
egress = "open"
"#;
    let cfg: RegistryConfig = toml::from_str(toml).unwrap();
    assert!(cfg.into_snapshot().is_ok());

    let bad = toml.replace(r#"mount = "/work""#, r#"mount = "/work/sub""#);
    let cfg: RegistryConfig = toml::from_str(&bad).unwrap();
    assert!(cfg.into_snapshot().is_err(), "mount != allowed_cwd_root must reject");
}

#[test]
fn sandbox_default_allowed_cmds_uses_runtime_not_cli() {
    // No [registry] → allowed_cmds defaults; a sandboxed agent must NOT self-reject (default = docker).
    let toml = r#"
default = "a"
allowed_cwd_root = "/work"
[[agents]]
id = "a"
cmd = "claude-agent-acp"
[agents.sandbox]
image = "img"
mount = "/work"
access = "ro"
egress = "open"
"#;
    let snap = toml::from_str::<RegistryConfig>(toml).unwrap().into_snapshot().unwrap();
    assert!(snap.allowed_cmds.contains(&"docker".to_string()));
    assert!(!snap.allowed_cmds.contains(&"claude-agent-acp".to_string()));
}

#[test]
fn egress_locked_requires_network_and_proxy() {
    let toml = r#"
default = "a"
allowed_cwd_root = "/work"
[[agents]]
id = "a"
cmd = "claude-agent-acp"
[agents.sandbox]
image = "img"
mount = "/work"
access = "ro"
egress = "locked"
"#; // network/proxy MISSING
    let cfg: RegistryConfig = toml::from_str(toml).unwrap();
    assert!(cfg.into_snapshot().is_err(), "locked without network/proxy must reject");
}
```

- [ ] **Step 2: Run — verify FAIL** (no `SandboxToml`, no parse)

Run: `cargo test -p a2a-bridge sandbox_mount_must_equal egress_locked_requires sandbox_default_allowed 2>&1 | tail -6`
Expected: compile/assert failures.

- [ ] **Step 3: Add `SandboxToml` + parsers + the `sandbox` field on `AgentEntryToml`**

```rust
#[derive(Debug, serde::Deserialize)]
pub struct SandboxToml {
    #[serde(default)] pub runtime: Option<String>,
    pub image: String,
    pub mount: String,
    pub access: String,                 // "ro" | "rw"
    pub egress: String,                 // "locked" | "open"
    #[serde(default)] pub network: Option<String>,
    #[serde(default)] pub proxy: Option<String>,
    #[serde(default)] pub no_proxy: Option<String>,
    #[serde(default)] pub volumes: Vec<String>,
}

fn parse_access(s: &str) -> Result<bridge_core::domain::MountAccess, ConfigError> {
    use bridge_core::domain::MountAccess;
    match s.to_ascii_lowercase().as_str() {
        "ro" => Ok(MountAccess::Ro),
        "rw" => Ok(MountAccess::Rw),
        other => Err(ConfigError::Registry(format!("invalid access: {other:?} (expected ro|rw)"))),
    }
}

fn parse_egress(t: &SandboxToml) -> Result<bridge_core::domain::EgressPolicy, ConfigError> {
    use bridge_core::domain::EgressPolicy;
    match t.egress.to_ascii_lowercase().as_str() {
        "open" => Ok(EgressPolicy::Open),
        "locked" => {
            let network = t.network.clone().ok_or_else(|| ConfigError::Registry("egress=locked requires network".into()))?;
            let proxy = t.proxy.clone().ok_or_else(|| ConfigError::Registry("egress=locked requires proxy".into()))?;
            Ok(EgressPolicy::Locked { network, proxy, no_proxy: t.no_proxy.clone() })
        }
        other => Err(ConfigError::Registry(format!("invalid egress: {other:?} (expected locked|open)"))),
    }
}
```
Add to `AgentEntryToml`: `#[serde(default)] pub sandbox: Option<SandboxToml>,`.

- [ ] **Step 4: S0 — make the `allowed_cmds` default use the runtime for sandboxed entries**

Replace the default-union arm (config.rs:276-282):
```rust
            _ => {
                let mut v: Vec<String> = self
                    .agents
                    .iter()
                    .map(|a| match &a.sandbox {
                        Some(sb) => sb.runtime.clone().unwrap_or_else(|| "docker".into()),
                        None => a.cmd.clone().unwrap_or_default(),
                    })
                    .filter(|s| !s.is_empty())
                    .collect();
                v.sort();
                v.dedup();
                v
            }
```

- [ ] **Step 5: S2 + build the `SandboxConfig` in the per-entry loop** (before pushing the `AgentEntry`)

```rust
            // Sandbox: convert the TOML mirror to the typed domain value + S2 mount==allowed_cwd_root.
            // NOTE (boot-fixed): the LIVE cwd gate reads `allowed_cwd_root` copied into InboundServer
            // ONCE at boot (main.rs:1024); hot-reload re-applies only the RegistrySnapshot, not the
            // server root — so a sandbox mount/root change needs a RESTART. This S2 check re-fires only
            // where into_snapshot runs (today the sole ConfigSource); a future 2nd source must re-thread it.
            let sandbox = match a.sandbox {
                None => None,
                Some(sb) => {
                    let root = self.allowed_cwd_root.as_deref().ok_or_else(|| {
                        ConfigError::Registry(format!("sandboxed agent {:?} requires allowed_cwd_root", id.as_str()))
                    })?;
                    let mount_n = bridge_core::session_cwd::SessionCwd::parse(&sb.mount)
                        .map_err(|e| ConfigError::Registry(format!("sandbox mount: {e:?}")))?;
                    let root_n = bridge_core::session_cwd::SessionCwd::parse(root)
                        .map_err(|e| ConfigError::Registry(format!("allowed_cwd_root: {e:?}")))?;
                    if mount_n.as_str() != root_n.as_str() {
                        return Err(ConfigError::Registry(format!(
                            "sandbox mount {:?} must equal allowed_cwd_root {:?}", sb.mount, root
                        )));
                    }
                    Some(bridge_core::domain::SandboxConfig {
                        runtime: sb.runtime.clone(),
                        image: sb.image.clone(),
                        mount: sb.mount.clone(),
                        access: parse_access(&sb.access)?,
                        egress: parse_egress(&sb)?,
                        volumes: sb.volumes.clone(),
                    })
                }
            };
```
Then add `sandbox,` to the `AgentEntry { … }` literal in this loop.

- [ ] **Step 6: Run — verify PASS**

Run: `cargo test -p a2a-bridge sandbox_mount_must_equal egress_locked_requires sandbox_default_allowed 2>&1 | grep "test result"`
Expected: pass.

- [ ] **Step 7: Commit**

```bash
git add bin/a2a-bridge/src/config.rs
git commit -m "feat(config): parse [sandbox] (SandboxToml + access/egress) + S0 runtime default + S2 mount==allowed_cwd_root (boot-fixed)"
```

---

## Task 5: Wire BOTH `SpawnFn` closures (compose-or-raw)

**Files:**
- Modify: `bin/a2a-bridge/src/main.rs` (the Acp arm at **both** `main.rs:163` and `main.rs:844`)

- [ ] **Step 1: At BOTH sites, replace the Acp arm body**

Both closures currently build `args`/`args_ref` up top and call `AcpBackend::spawn(cmd, &args_ref, acp)`. In the **Acp arm** at each site, derive `(program, argv)` from the sandbox:
```rust
                AgentKind::Acp => {
                    let cmd = entry.cmd.as_deref().ok_or(BridgeError::ConfigInvalid {
                        reason: format!("acp agent {} missing cmd", entry.id.as_str()),
                    })?;
                    // Compose-or-raw: a [sandbox] agent runs the runtime (docker) wrapping the agent cli;
                    // a raw agent runs cmd+args directly (Slice A compat). BOTH spawn sites must do this
                    // or run-workflow (this site) diverges from serve.
                    let (program, argv): (String, Vec<String>) = match &entry.sandbox {
                        Some(sb) => bridge_core::sandbox::compose_sandbox(sb, cmd, &entry.args),
                        None => (cmd.to_string(), entry.args.clone()),
                    };
                    let argv_ref: Vec<&str> = argv.iter().map(String::as_str).collect();
                    let acp = AcpConfig {
                        cwd,
                        model: entry.model.clone(),
                        mode: entry.mode.clone(),
                        auth_method: entry.auth_method.clone(),
                        ..AcpConfig::default()
                    };
                    let be = AcpBackend::spawn(&program, &argv_ref, acp).await?.with_policy(policy);
                    Ok(Arc::new(be) as Arc<dyn AgentBackend>)
                }
```
(At the run-workflow site use the fully-qualified `bridge_acp::acp_backend::AcpConfig`/`AcpBackend` it already uses.) Remove the now-unused top-of-closure `args`/`args_ref` lets at both sites (the Api arm doesn't use them) — or leave them and let the Acp arm shadow; the cleanest is to delete the two top lets. Run `cargo build` and fix any unused-var warning.

- [ ] **Step 2: Build + clippy**

Run: `cargo build --workspace 2>&1 | tail -3 && cargo clippy --workspace 2>&1 | grep -E "warning|error" | head`
Expected: clean (no unused `args`).

- [ ] **Step 3: Full test suite (no regressions — zero behavior change for raw agents)**

Run: `cargo test --workspace 2>&1 | grep -E "test result: FAILED" | head; echo done`
Expected: no FAILED lines.

- [ ] **Step 4: Commit**

```bash
git add bin/a2a-bridge/src/main.rs
git commit -m "feat(spawn): wire BOTH SpawnFn closures (compose_sandbox-or-raw) at run-workflow + serve"
```

---

## Task 6: Migrate the config + the acceptance gate

**Files:**
- Modify: `examples/a2a-bridge.containerized.toml`

- [ ] **Step 1: Migrate the 3 readers to `[sandbox]`** (before/after for `claude`; do codex + kiro the same)

BEFORE (Slice A, hand-typed):
```toml
[[agents]]
id   = "claude"
cmd  = "docker"
args = ["run","-i","--rm","--network","a2a-egress-internal","-e","HTTPS_PROXY=http://a2a-egress-proxy:8888","-e","HTTP_PROXY=http://a2a-egress-proxy:8888","-v","/Users/wesleyjinks/code:/Users/wesleyjinks/code:ro","-v","/Users/wesleyjinks/.config/a2a-creds/claude/.credentials.json:/root/.claude/.credentials.json","a2a-agent-reader:latest","claude-agent-acp"]
```
AFTER (B1, declared — keep the top-level `allowed_cwd_root` + `[registry]` + `[server]` as-is):
```toml
[[agents]]
id  = "claude"
cmd = "claude-agent-acp"
[agents.sandbox]
image   = "a2a-agent-reader:latest"
mount   = "/Users/wesleyjinks/code"     # == allowed_cwd_root (S2)
access  = "ro"
egress  = "locked"
network = "a2a-egress-internal"
proxy   = "http://a2a-egress-proxy:8888"
volumes = ["/Users/wesleyjinks/.config/a2a-creds/claude/.credentials.json:/root/.claude/.credentials.json"]
```
codex: `cmd="codex-acp"`, `volumes=[".../codex/auth.json:/root/.codex/auth.json"]`. kiro: `cmd="kiro-cli"`, `args=["acp"]`, `volumes=["a2a-kiro-data:/root/.local/share"]`. **ollama + ollama-cloud: UNCHANGED** (`kind="api"`, no sandbox). Keep `[registry] allowed_cmds = ["docker"]` (or rely on the S0 default).

- [ ] **Step 2: Config parses + the migrated readers spawn**

Run (from repo root): `cargo run -q -p a2a-bridge -- run-workflow smoke-claude --input README.md --config examples/a2a-bridge.containerized.toml 2>&1 | tr -d '\033' | sed 's/\[[0-9;]*m//g' | grep -o 'SMOKE_OK.*' | head -1`
Expected: `SMOKE_OK: …` (claude composed by the bridge, not hand-typed args).

- [ ] **Step 3: ACCEPTANCE GATE — all five smokes + POSITIVE containment, BOTH code paths** *(operator-run; Docker + Slice-A creds/volumes; not CI)*

```bash
# all five via run-workflow (main.rs:163 SpawnFn) — from the repo root:
for wf in smoke-claude smoke-codex smoke-kiro; do
  echo "== $wf =="; cargo run -q -p a2a-bridge -- run-workflow $wf --input README.md --config examples/a2a-bridge.containerized.toml 2>&1 | tr -d '\033' | sed 's/\[[0-9;]*m//g' | grep -o 'SMOKE_OK.*' | head -1
done
OLLAMA_API_KEY=$(zsh -lic 'printf %s "$OLLAMA_API_KEY"') ; export OLLAMA_API_KEY
for wf in smoke-ollama smoke-ollama-cloud; do
  echo "== $wf =="; cargo run -q -p a2a-bridge -- run-workflow $wf --input README.md --config examples/a2a-bridge.containerized.toml 2>&1 | tr -d '\033' | sed 's/\[[0-9;]*m//g' | grep -o 'SMOKE_OK' | head -1
done
```
Expected: five `SMOKE_OK`s.

**POSITIVE CONTAINMENT (SMOKE_OK alone false-greens if main.rs:163 is mis-wired → uncontained host spawn):** while a reader smoke runs, confirm a real container exists:
```bash
( cargo run -q -p a2a-bridge -- run-workflow smoke-claude --input README.md --config examples/a2a-bridge.containerized.toml >/tmp/g.txt 2>&1 ) &
sleep 4; docker ps --filter ancestor=a2a-agent-reader:latest --format '{{.Image}} {{.Status}}' | head; wait
```
Expected: a live `a2a-agent-reader:latest` container during the run (the proof the bridge composed + spawned the sandbox). Also assert `:ro` from inside (re-use the Task 7 probe from the Slice A runbook) and the egress curl-triad.

**BOTH code paths:** repeat ONE reader through `serve`+A2A (main.rs:844) to prove that site too:
```bash
cargo run -q -p a2a-bridge -- serve --config examples/a2a-bridge.containerized.toml & SERVE=$!; sleep 3
curl -sS -X POST http://127.0.0.1:8080/ -H 'content-type: application/json' -H 'A2A-Version: 1.0' -d '{"jsonrpc":"2.0","id":1,"method":"SendMessage","params":{"message":{"role":"user","parts":[{"kind":"text","text":"List the top-level files here and STOP."}],"metadata":{"a2a-bridge.cwd":"/Users/wesleyjinks/code/a2a-bridge"}}}}' | head -c 300
kill $SERVE
```
Expected: the containerized agent reads the repo via the serve path too.

- [ ] **Step 4: Commit**

```bash
git add examples/a2a-bridge.containerized.toml
git commit -m "config: migrate the 3 containerized readers to the [sandbox] block (ollama unchanged); acceptance gate PASS (5 smokes + containment, both code paths)"
```

---

## Final verification

- [ ] `cargo fmt --all` (commit any churn separately if large).
- [ ] `cargo clippy --workspace --all-targets 2>&1 | grep -E "warning|error"` → clean.
- [ ] `cargo llvm-cov clean --workspace && cargo llvm-cov --workspace --summary-only` → floors met (workspace ≥85, bridge-core ≥90). `compose_sandbox` + the invariants are pure → easy core coverage.
- [ ] ADR-0017 (B1) — short: the enforced `[sandbox]` block; amends ADR-0016; the two-layer validation + the data-carrying `EgressPolicy`; what the dogfood caught (nested-volumes S6). Carry the `Co-Authored-By` trailer.

---

## Self-Review

**Spec coverage:** Types+ripple → T1; `compose_sandbox` (total) → T2; validate S1/S3/S4/S5/S6 + reuse → T3; parse S0/S2 + EgressPolicy conversion → T4; both SpawnFn → T5; migration + the all-five-smokes + containment gate (both paths) → T6. The boot-fixed S2 caveat is a code comment (T4 Step 5). The behavior-change note is in T3 Step 5's commit. ADR in Final. Covered.

**Placeholder scan:** the only "discover during the task" steps are the exact ripple-site list (T1 Step 7 — the compiler enumerates them) and the `SessionCwd::parse` import path (T3 — confirm against `session_cwd.rs`). No silent TBDs.

**Type consistency:** `SandboxConfig`/`MountAccess`/`EgressPolicy::Locked{network,proxy,no_proxy}` and `sb.runtime()` are used identically in T1 (def), T2 (compose), T3 (validate), T4 (parse build). `compose_sandbox(sb, agent_cmd, agent_args)` signature matches T2 def and T5 call.

---

## Execution Handoff

Per the loop, this plan gets its **own Codex + Claude dual-review** (a2a-local-bridge) — optionally the containerized dogfood `plan-review` — before the build. After folding: slices 1–5 are pure-Rust TDD I run inline; the T6 acceptance gate is operator-run (Docker + Slice-A creds/volumes, both present). **Two execution options:** (1) Subagent-Driven (fresh subagent per task + two-stage review); (2) Inline Execution (this session, checkpoints — fits the pure-Rust TDD + the human-gated Docker acceptance, like Slice A).
