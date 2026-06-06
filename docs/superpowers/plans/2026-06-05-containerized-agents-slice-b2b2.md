# Containerized Agents — Slice B2b-2 (build+test verify) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** After the `implement` commit, run a trustworthy, config-driven `cargo` build+test verify in a toolchain container on the quarantine clone and report the verdict in the operator hand-off.

**Architecture:** A new pure `compose_verify` in `bridge-core/sandbox.rs` reuses `compose_sandbox` (clone `mount=clone, access=Ro` + a cache volume) so egress stays one source of truth. Verify runs **each command as its own `docker run`** sharing a per-repo cache volume; the bridge reads each **container exit code** (unforgeable). A new `bin/a2a-bridge/src/verify.rs` holds the pure verdict aggregation + a runner-injected loop; the real Docker runner is the thin live-gated piece. `[verify]` is a new top-level config block whose egress is parsed by the existing `parse_egress` machinery. Coverage is opt-in; a separate `a2a-verify-egress` proxy carries the registries so creds and registry-egress never coexist.

**Tech Stack:** Rust (workspace: bridge-core + bin/a2a-bridge), Docker/Podman, tinyproxy, cargo.

**Spec:** `docs/superpowers/specs/2026-06-05-containerized-agents-slice-b2b2-design.md` (rev2, committed `1448f02`, branch `feat/implement-verify`).

**Conventions:** TDD green-per-task; task/code commits do NOT carry the `Co-Authored-By` trailer (the ADR doc does). Coverage after `cargo llvm-cov clean --workspace` (floors: workspace 85, bridge-core 90, bridge-registry 90). All host process spawns = direct argv `std::process::Command`, no shell. The plan gets its own Codex+Claude dual-review before the build.

---

## File Structure

| File | Responsibility | Change |
|---|---|---|
| `crates/bridge-core/src/sandbox.rs` | pure `compose_verify` (argv for one verify command) | Modify (add fn + tests) |
| `bin/a2a-bridge/src/config.rs` | `VerifyToml`/`VerifyConfig`/`VerifyCommand`; `parse_egress_fields` refactor; `RegistryConfig.verify` | Modify |
| `bin/a2a-bridge/src/verify.rs` | `VerifyResult`/`VerifyVerdict`, `aggregate`, `truncate_output`, `verdict_line`, `cache_volume_name`, `run_verify` (runner-injected) + `docker_runner` | Create |
| `bin/a2a-bridge/src/main.rs` | `mod verify;`; wire verify into the `Action::Commit` arm | Modify (`implement_cmd`) |
| `deploy/containers/toolchain.Containerfile` | reader + build-essential + Rust 1.94.0 + cargo tools | Create |
| `deploy/containers/tinyproxy.verify.filter` | registries allowlist | Create |
| `deploy/containers/compose.egress.yaml` | `a2a-verify-egress` net + `verify-proxy` service | Modify |
| `examples/a2a-bridge.containerized.toml` | `impl` image → `a2a-toolchain`; add `[verify]` | Modify |
| `docs/adr/0020-containerized-agents-b2b2-verify.md` | the increment's ADR | Create (trailer) |

---

## Task 1: pure `compose_verify` in bridge-core

**Files:**
- Modify: `crates/bridge-core/src/sandbox.rs` (add fn after `compose_container_rw`, ~line 84; add tests in the `#[cfg(test)]` module)

- [ ] **Step 1: Write the failing test**

Add to the `#[cfg(test)] mod tests` in `crates/bridge-core/src/sandbox.rs` (reuse the existing `ro_locked()` helper at ~line 119 for an `EgressPolicy::Locked`):

```rust
#[test]
fn compose_verify_ro_clone_plus_cache_reuses_compose_sandbox() {
    use crate::session_cwd::SessionCwd;
    let egress = EgressPolicy::Locked {
        network: "a2a-verify-egress".into(),
        proxy: "http://a2a-verify-proxy:8888".into(),
        no_proxy: None,
    };
    let clone = SessionCwd::parse("/Users/w/code/.a2a-implement/impl-1-ab").unwrap();
    let (prog, argv) = compose_verify(
        None,
        "a2a-toolchain:latest",
        &egress,
        &clone,
        "a2a-verify-cache-deadbeef",
        "cargo build --locked",
    );
    assert_eq!(prog, "docker");
    // egress from the EgressPolicy (both proxies, like compose_sandbox)
    assert!(argv.windows(2).any(|w| w == ["--network", "a2a-verify-egress"]));
    assert!(argv.iter().any(|a| a == "HTTPS_PROXY=http://a2a-verify-proxy:8888"));
    assert!(argv.iter().any(|a| a == "HTTP_PROXY=http://a2a-verify-proxy:8888"));
    // the clone mounted :ro (identical path) — NOT :rw
    let mnt = "/Users/w/code/.a2a-implement/impl-1-ab";
    assert!(argv.iter().any(|a| a == &format!("{mnt}:{mnt}:ro")));
    // the cache volume
    assert!(argv.iter().any(|a| a == "a2a-verify-cache-deadbeef:/cache"));
    // NO creds volume (verify mounts nothing but the clone + cache)
    assert!(!argv.iter().any(|a| a.contains(".credentials.json") || a.contains("auth.json")));
    // the command runs under sh -c with the cargo env exported into the cache
    assert_eq!(argv[argv.len() - 3], "sh");
    assert_eq!(argv[argv.len() - 2], "-c");
    let script = argv.last().unwrap();
    assert!(script.contains("CARGO_HOME=/cache/cargo"));
    assert!(script.contains("CARGO_TARGET_DIR=/cache/target"));
    assert!(script.contains("cargo build --locked"));
}

#[test]
fn compose_verify_open_egress_has_no_network() {
    use crate::session_cwd::SessionCwd;
    let clone = SessionCwd::parse("/repo/clone").unwrap();
    let (_p, argv) = compose_verify(
        Some("podman"), "img", &EgressPolicy::Open, &clone, "c", "cargo test --locked",
    );
    assert!(!argv.iter().any(|a| a == "--network"));
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p bridge-core compose_verify`
Expected: FAIL — `cannot find function compose_verify`.

- [ ] **Step 3: Implement `compose_verify`**

Add after `compose_container_rw` (after line 84) in `crates/bridge-core/src/sandbox.rs`:

```rust
/// PURE+TOTAL. The `(program, argv)` for ONE verify command. Reuses [`compose_sandbox`] (clone
/// `mount=clone, access=Ro`, the cache volume appended) so egress / runtime / suffix derivation stay
/// ONE source of truth. The command runs under `sh -c` with CARGO_HOME/CARGO_TARGET_DIR exported into
/// the cache mount — so its exit code (read by the caller from the container) IS the command's verdict.
/// NO creds: the only volumes are the `:ro` clone + the cache.
pub fn compose_verify(
    runtime: Option<&str>,
    image: &str,
    egress: &EgressPolicy,
    clone: &crate::session_cwd::SessionCwd,
    cache_vol: &str,
    command: &str,
) -> (String, Vec<String>) {
    let script = format!(
        "export CARGO_HOME=/cache/cargo CARGO_TARGET_DIR=/cache/target; \
         mkdir -p \"$CARGO_HOME\" \"$CARGO_TARGET_DIR\"; {command}"
    );
    let sb = SandboxConfig {
        runtime: runtime.map(str::to_string),
        image: image.to_string(),
        mount: clone.as_str().to_string(),
        access: MountAccess::Ro,
        egress: egress.clone(),
        volumes: vec![format!("{cache_vol}:/cache")],
    };
    compose_sandbox(&sb, "sh", &["-c".to_string(), script])
}
```

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p bridge-core compose_verify`
Expected: PASS (2 tests).

- [ ] **Step 5: Commit**

```bash
git add crates/bridge-core/src/sandbox.rs
git commit -m "feat(b2b2): pure compose_verify reuses compose_sandbox (:ro clone + cache)"
```

---

## Task 2: `[verify]` config — `VerifyToml`/`VerifyConfig` + `parse_egress_fields` refactor

**Files:**
- Modify: `bin/a2a-bridge/src/config.rs`

- [ ] **Step 1: Write the failing tests**

Add to the `#[cfg(test)] mod tests` in `bin/a2a-bridge/src/config.rs`:

```rust
#[test]
fn verify_config_parses_structured_commands_and_locked_egress() {
    let c = RegistryConfig::parse(
        r#"
        default = "x"
        [[agents]]
        id = "x"
        cmd = "echo"
        [verify]
        image = "a2a-toolchain:latest"
        cache = "a2a-verify-cache"
        egress = "locked"
        network = "a2a-verify-egress"
        proxy = "http://a2a-verify-proxy:8888"
        [[verify.commands]]
        name = "fmt"
        cmd = "cargo fmt --all -- --check"
        [[verify.commands]]
        name = "test"
        cmd = "cargo test --locked"
        gate = false
        "#,
    )
    .unwrap();
    let v = c.verify.as_ref().unwrap().into_config().unwrap();
    assert_eq!(v.image, "a2a-toolchain:latest");
    assert_eq!(v.cache, "a2a-verify-cache");
    assert!(matches!(v.egress, bridge_core::domain::EgressPolicy::Locked { .. }));
    assert_eq!(v.commands.len(), 2);
    assert_eq!(v.commands[0].name, "fmt");
    assert!(v.commands[0].gate); // gate defaults to true
    assert!(!v.commands[1].gate); // explicit gate=false
}

#[test]
fn verify_config_rejects_locked_without_network() {
    let c = RegistryConfig::parse(
        r#"
        default = "x"
        [[agents]]
        id = "x"
        cmd = "echo"
        [verify]
        image = "i"
        cache = "c"
        egress = "locked"
        proxy = "http://p:8888"
        [[verify.commands]]
        name = "t"
        cmd = "cargo test"
        "#,
    )
    .unwrap();
    let e = c.verify.as_ref().unwrap().into_config().unwrap_err();
    assert!(format!("{e:?}").contains("requires network"));
}

#[test]
fn verify_config_rejects_empty_commands() {
    let c = RegistryConfig::parse(
        r#"
        default = "x"
        [[agents]]
        id = "x"
        cmd = "echo"
        [verify]
        image = "i"
        cache = "c"
        egress = "open"
        "#,
    )
    .unwrap();
    let e = c.verify.as_ref().unwrap().into_config().unwrap_err();
    assert!(format!("{e:?}").contains("at least one command"));
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p a2a-bridge --bin a2a-bridge verify_config`
Expected: FAIL — no field `verify` on `RegistryConfig`.

- [ ] **Step 3: Refactor `parse_egress` and add the verify types**

In `bin/a2a-bridge/src/config.rs`, refactor `parse_egress` (lines 228-249) to delegate to a field-level helper so verify reuses the "locked ⇒ network+proxy" invariant:

```rust
/// The locked-vs-open invariant, on raw fields, so both [`SandboxToml`] and `[verify]` share it.
fn parse_egress_fields(
    egress: &str,
    network: &Option<String>,
    proxy: &Option<String>,
    no_proxy: &Option<String>,
) -> Result<bridge_core::domain::EgressPolicy, ConfigError> {
    use bridge_core::domain::EgressPolicy;
    match egress.to_ascii_lowercase().as_str() {
        "open" => Ok(EgressPolicy::Open),
        "locked" => {
            let network = network.clone().ok_or_else(|| {
                ConfigError::Registry("egress=locked requires network".into())
            })?;
            let proxy = proxy
                .clone()
                .ok_or_else(|| ConfigError::Registry("egress=locked requires proxy".into()))?;
            Ok(EgressPolicy::Locked { network, proxy, no_proxy: no_proxy.clone() })
        }
        other => Err(ConfigError::Registry(format!(
            "invalid egress: {other:?} (expected locked|open)"
        ))),
    }
}

fn parse_egress(t: &SandboxToml) -> Result<bridge_core::domain::EgressPolicy, ConfigError> {
    parse_egress_fields(&t.egress, &t.network, &t.proxy, &t.no_proxy)
}
```

Add the TOML + parsed types (near `SandboxToml`, ~line 213):

```rust
fn default_gate() -> bool {
    true
}

#[derive(Debug, serde::Deserialize)]
pub struct VerifyCommandToml {
    pub name: String,
    pub cmd: String,
    #[serde(default = "default_gate")]
    pub gate: bool,
}

#[derive(Debug, serde::Deserialize)]
pub struct VerifyToml {
    #[serde(default)]
    pub runtime: Option<String>,
    pub image: String,
    pub cache: String,
    pub egress: String,
    #[serde(default)]
    pub network: Option<String>,
    #[serde(default)]
    pub proxy: Option<String>,
    #[serde(default)]
    pub no_proxy: Option<String>,
    #[serde(default)]
    pub commands: Vec<VerifyCommandToml>,
}

/// Parsed `[verify]`: structured commands + a validated egress policy.
#[derive(Debug, Clone)]
pub struct VerifyConfig {
    pub runtime: Option<String>,
    pub image: String,
    pub cache: String,
    pub egress: bridge_core::domain::EgressPolicy,
    pub commands: Vec<VerifyCommand>,
}

#[derive(Debug, Clone)]
pub struct VerifyCommand {
    pub name: String,
    pub cmd: String,
    pub gate: bool,
}

impl VerifyToml {
    pub fn into_config(&self) -> Result<VerifyConfig, ConfigError> {
        if self.commands.is_empty() {
            return Err(ConfigError::Registry(
                "[verify] needs at least one command".into(),
            ));
        }
        let egress = parse_egress_fields(&self.egress, &self.network, &self.proxy, &self.no_proxy)?;
        Ok(VerifyConfig {
            runtime: self.runtime.clone(),
            image: self.image.clone(),
            cache: self.cache.clone(),
            egress,
            commands: self
                .commands
                .iter()
                .map(|c| VerifyCommand {
                    name: c.name.clone(),
                    cmd: c.cmd.clone(),
                    gate: c.gate,
                })
                .collect(),
        })
    }
}
```

Add the field to `RegistryConfig` (after `allowed_cwd_root`, line 118):

```rust
    #[serde(default)]
    pub verify: Option<VerifyToml>,
```

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p a2a-bridge --bin a2a-bridge verify_config`
Expected: PASS (3 tests). Also run `cargo test -p a2a-bridge --bin a2a-bridge egress` to confirm the `parse_egress` refactor didn't regress (the existing locked/open tests still pass).

- [ ] **Step 5: Commit**

```bash
git add bin/a2a-bridge/src/config.rs
git commit -m "feat(b2b2): [verify] config (structured commands, egress via parse_egress_fields)"
```

---

## Task 3: pure `verify.rs` helpers — verdict, truncation, hand-off line, cache name

**Files:**
- Create: `bin/a2a-bridge/src/verify.rs`

- [ ] **Step 1: Write the file with the pure helpers + their tests**

Create `bin/a2a-bridge/src/verify.rs`:

```rust
//! The `implement` build+test VERIFY step: run each configured command as its own container (sharing a
//! per-repo cache), read each CONTAINER exit code (unforgeable — agent code in `cargo test` can't fake
//! it), aggregate a reported (non-gating) verdict for the operator hand-off. The Docker run is the only
//! impure piece (`docker_runner`, live-gated); everything else is pure + unit-tested.

use crate::config::{VerifyCommand, VerifyConfig};

/// One command's outcome. `gate=false` commands are reported but never fail the verdict.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifyResult {
    pub name: String,
    pub gate: bool,
    pub ok: bool,
    pub output: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifyVerdict {
    pub results: Vec<VerifyResult>,
    pub passed: bool,
}

/// PURE. The verdict passes iff every GATE command succeeded (non-gate commands are reported only).
pub fn aggregate(results: Vec<VerifyResult>) -> VerifyVerdict {
    let passed = results.iter().all(|r| !r.gate || r.ok);
    VerifyVerdict { results, passed }
}

/// PURE. Clamp captured output to `max` bytes on a char boundary, marking truncation.
pub fn truncate_output(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let mut end = max;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}\n…[truncated {} bytes]", &s[..end], s.len() - end)
}

/// PURE. The one-line verdict for the operator hand-off (stdout). Failing-command OUTPUT goes to
/// stderr separately; this is the summary line.
pub fn verdict_line(v: &VerifyVerdict) -> String {
    let marks: Vec<String> = v
        .results
        .iter()
        .map(|r| format!("{} {}", r.name, if r.ok { "✓" } else { "✗" }))
        .collect();
    if v.passed {
        format!("verify: PASS  ({})", marks.join(" · "))
    } else {
        let failed = v
            .results
            .iter()
            .find(|r| r.gate && !r.ok)
            .map(|r| r.name.as_str())
            .unwrap_or("?");
        format!("verify: FAIL at {}  ({})", failed, marks.join(" · "))
    }
}

/// PURE. A stable per-repo cache volume name: `<base>-<hash(canonical repo path)>`. Per-repo keying
/// isolates repos; same-repo runs share (single-flight serializes them — see `run_verify`'s caller).
/// Reuses the codebase's `DefaultHasher` owner-token pattern (`main::container_owner`).
pub fn cache_volume_name(base: &str, repo_canon: &str) -> String {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    repo_canon.hash(&mut h);
    format!("{base}-{:016x}", h.finish())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(name: &str, gate: bool, ok: bool) -> VerifyResult {
        VerifyResult { name: name.into(), gate, ok, output: String::new() }
    }

    #[test]
    fn aggregate_passes_when_all_gates_pass() {
        let v = aggregate(vec![r("fmt", true, true), r("test", true, true)]);
        assert!(v.passed);
    }

    #[test]
    fn aggregate_fails_on_a_gate_failure() {
        let v = aggregate(vec![r("fmt", true, true), r("clippy", true, false)]);
        assert!(!v.passed);
    }

    #[test]
    fn aggregate_ignores_a_nongate_failure() {
        let v = aggregate(vec![r("test", true, true), r("coverage", false, false)]);
        assert!(v.passed);
    }

    #[test]
    fn truncate_marks_oversized_output() {
        let out = truncate_output(&"x".repeat(100), 10);
        assert!(out.starts_with(&"x".repeat(10)));
        assert!(out.contains("truncated 90 bytes"));
        assert_eq!(truncate_output("short", 10), "short");
    }

    #[test]
    fn verdict_line_pass_and_fail() {
        let pass = aggregate(vec![r("fmt", true, true), r("test", true, true)]);
        assert_eq!(verdict_line(&pass), "verify: PASS  (fmt ✓ · test ✓)");
        let fail = aggregate(vec![r("fmt", true, true), r("clippy", true, false)]);
        assert_eq!(verdict_line(&fail), "verify: FAIL at clippy  (fmt ✓ · clippy ✗)");
    }

    #[test]
    fn cache_volume_name_is_stable_and_per_repo() {
        let a = cache_volume_name("a2a-verify-cache", "/Users/w/code/proj-a");
        let b = cache_volume_name("a2a-verify-cache", "/Users/w/code/proj-b");
        assert_eq!(a, cache_volume_name("a2a-verify-cache", "/Users/w/code/proj-a"));
        assert_ne!(a, b);
        assert!(a.starts_with("a2a-verify-cache-"));
    }
}
```

- [ ] **Step 2: Wire the module + run the tests**

Add `mod verify;` near the other `mod` declarations in `bin/a2a-bridge/src/main.rs` (next to `mod implement;`).

Run: `cargo test -p a2a-bridge --bin a2a-bridge verify::`
Expected: PASS (6 tests).

- [ ] **Step 3: Commit**

```bash
git add bin/a2a-bridge/src/verify.rs bin/a2a-bridge/src/main.rs
git commit -m "feat(b2b2): pure verify helpers (verdict, truncate, hand-off line, per-repo cache name)"
```

---

## Task 4: `run_verify` (runner-injected loop) + the real `docker_runner`

**Files:**
- Modify: `bin/a2a-bridge/src/verify.rs`

- [ ] **Step 1: Write the failing test (stub runner)**

Add to `bin/a2a-bridge/src/verify.rs` tests:

```rust
fn cfg(cmds: &[(&str, bool)]) -> VerifyConfig {
    VerifyConfig {
        runtime: None,
        image: "img".into(),
        cache: "cache".into(),
        egress: bridge_core::domain::EgressPolicy::Open,
        commands: cmds
            .iter()
            .map(|(c, gate)| VerifyCommand { name: (*c).into(), cmd: format!("cargo {c}"), gate: *gate })
            .collect(),
    }
}

#[test]
fn run_verify_stops_at_first_gate_failure() {
    use crate::session_cwd_helpers::sc; // see Step 3 note; or inline SessionCwd::parse
    let clone = bridge_core::SessionCwd::parse("/repo/clone").unwrap();
    // runner: fmt ok, clippy FAILS (gate) -> build/test must NOT run.
    let runner = |_p: &str, argv: &[String]| -> std::io::Result<(i32, String)> {
        let script = argv.last().unwrap();
        if script.contains("cargo clippy") {
            Ok((1, "error: clippy".into()))
        } else {
            Ok((0, "ok".into()))
        }
    };
    let v = run_verify(
        &cfg(&[("fmt", true), ("clippy", true), ("build", true), ("test", true)]),
        &clone,
        "cache-x",
        &runner,
        4096,
    );
    assert!(!v.passed);
    assert_eq!(v.results.len(), 2); // stopped after clippy
    assert_eq!(v.results[1].name, "clippy");
    assert!(!v.results[1].ok);
}

#[test]
fn run_verify_reports_nongate_failure_but_passes() {
    let clone = bridge_core::SessionCwd::parse("/repo/clone").unwrap();
    let runner = |_p: &str, argv: &[String]| -> std::io::Result<(i32, String)> {
        let script = argv.last().unwrap();
        if script.contains("cargo coverage") { Ok((1, "cov fail".into())) } else { Ok((0, "ok".into())) }
    };
    let v = run_verify(&cfg(&[("test", true), ("coverage", false)]), &clone, "cache-x", &runner, 4096);
    assert!(v.passed); // the non-gate coverage failure doesn't fail the verdict
    assert_eq!(v.results.len(), 2);
}

#[test]
fn run_verify_runner_error_is_a_failure() {
    let clone = bridge_core::SessionCwd::parse("/repo/clone").unwrap();
    let runner = |_p: &str, _argv: &[String]| -> std::io::Result<(i32, String)> {
        Err(std::io::Error::other("docker missing"))
    };
    let v = run_verify(&cfg(&[("build", true)]), &clone, "cache-x", &runner, 4096);
    assert!(!v.passed);
    assert!(v.results[0].output.contains("docker missing"));
}
```

(Delete the `use crate::session_cwd_helpers::sc;` placeholder line — it was illustrative; the tests use `bridge_core::SessionCwd::parse` directly.)

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p a2a-bridge --bin a2a-bridge run_verify`
Expected: FAIL — `cannot find function run_verify`.

- [ ] **Step 3: Implement `run_verify` + `docker_runner`**

Add to `bin/a2a-bridge/src/verify.rs` (above the test module):

```rust
/// A command runner: given `(program, argv)`, run it and return `(exit_code, combined_output)`. The real
/// impl spawns Docker; tests inject a stub. The exit code is the CONTAINER's — unforgeable by in-container
/// agent code.
pub type Runner<'a> = dyn Fn(&str, &[String]) -> std::io::Result<(i32, String)> + 'a;

/// Run every configured command as its own container (sharing the per-repo cache volume), reading each
/// container's exit code. Stops at the FIRST gate failure. Pure given an injected `runner`.
pub fn run_verify(
    cfg: &VerifyConfig,
    clone: &bridge_core::SessionCwd,
    cache_vol: &str,
    runner: &Runner,
    max_bytes: usize,
) -> VerifyVerdict {
    let mut results = Vec::new();
    for c in &cfg.commands {
        let (prog, argv) = bridge_core::sandbox::compose_verify(
            cfg.runtime.as_deref(),
            &cfg.image,
            &cfg.egress,
            clone,
            cache_vol,
            &c.cmd,
        );
        let (exit, out) = match runner(&prog, &argv) {
            Ok((e, o)) => (e, o),
            Err(e) => (-1, format!("verify: runner error: {e}")),
        };
        let ok = exit == 0;
        results.push(VerifyResult {
            name: c.name.clone(),
            gate: c.gate,
            ok,
            output: truncate_output(&out, max_bytes),
        });
        if c.gate && !ok {
            break; // stop at the first gate failure
        }
    }
    aggregate(results)
}

/// The real runner: spawn the container, capture stdout+stderr combined, return the exit code.
pub fn docker_runner(program: &str, argv: &[String]) -> std::io::Result<(i32, String)> {
    let out = std::process::Command::new(program).args(argv).output()?;
    let mut combined = String::from_utf8_lossy(&out.stdout).into_owned();
    combined.push_str(&String::from_utf8_lossy(&out.stderr));
    Ok((out.status.code().unwrap_or(-1), combined))
}
```

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p a2a-bridge --bin a2a-bridge run_verify`
Expected: PASS (3 tests).

- [ ] **Step 5: Commit**

```bash
git add bin/a2a-bridge/src/verify.rs
git commit -m "feat(b2b2): run_verify per-command loop (unforgeable container exit) + docker_runner"
```

---

## Task 5: wire verify into `implement_cmd`'s `Action::Commit` arm

**Files:**
- Modify: `bin/a2a-bridge/src/main.rs` (the `Action::Commit` arm, lines 504-526)

- [ ] **Step 1: Add the integration**

Replace the body of the `implement::Action::Commit(message) => { ... }` arm (lines 504-526) so verify runs after the commit, before the hand-off. The verdict line is appended to the hand-off (stdout); failing-command output goes to stderr. Verify only runs if `[verify]` is configured; `implement` always exits `Ok` (verify is informational).

```rust
        implement::Action::Commit(message) => {
            let sha = implement::host_commit(&clone, &message)?;
            let _ = std::fs::remove_file(clone.join(".git").join("A2A_COMMIT_MSG")); // R13: strip after
            if !matches!(
                implement::stage_state(&clone)
                    .map_err(|e| format!("implement: post-commit stage: {e}"))?,
                implement::StageState::Clean
            ) {
                eprintln!("[implement] note: the clone still has uncommitted changes the agent left unstaged.");
            }
            let subject = message.lines().next().unwrap_or("").to_string();
            let mut handoff = implement::handoff_text(
                &clone.to_string_lossy(),
                &branch,
                &sha,
                &subject,
                &a.repo.to_string_lossy(),
            );

            // B2b-2: deterministic build+test verify on the committed clone (reported, not gating).
            match cfg.verify.as_ref().map(|t| t.into_config()).transpose() {
                Ok(Some(vcfg)) => {
                    let clone_cwd = bridge_core::SessionCwd::parse(&clone.to_string_lossy())?;
                    let cache_vol = verify::cache_volume_name(&vcfg.cache, &a.repo.to_string_lossy());
                    eprintln!("[implement] verify: running {} command(s) in {}", vcfg.commands.len(), vcfg.image);
                    let verdict = verify::run_verify(
                        &vcfg,
                        &clone_cwd,
                        &cache_vol,
                        &verify::docker_runner,
                        16 * 1024,
                    );
                    for r in &verdict.results {
                        if !r.ok {
                            eprintln!("[implement] verify: {} failed:\n{}", r.name, r.output);
                        }
                    }
                    handoff.push('\n');
                    handoff.push_str(&verify::verdict_line(&verdict));
                }
                Ok(None) => handoff.push_str("\nverify: not configured"),
                Err(e) => {
                    eprintln!("[implement] verify: config error: {e:?} — skipping verify");
                    handoff.push_str("\nverify: skipped (config error)");
                }
            }

            println!("{handoff}");
            Ok(())
        }
```

- [ ] **Step 2: Build + run the existing implement tests**

Run: `cargo build -p a2a-bridge && cargo test -p a2a-bridge --bin a2a-bridge implement::`
Expected: compiles; the existing `decide_matrix`/`host_commit`/argv tests still PASS (this arm has no new unit test — it's the impure orchestration, covered by the live gate; `run_verify`/`compose_verify` are unit-tested in Tasks 1/4).

- [ ] **Step 3: clippy**

Run: `cargo clippy -p a2a-bridge --all-targets -- -D warnings`
Expected: clean.

- [ ] **Step 4: Commit**

```bash
git add bin/a2a-bridge/src/main.rs
git commit -m "feat(b2b2): run verify after the implement commit; verdict in the hand-off"
```

---

## Task 6: the `a2a-toolchain` image

**Files:**
- Create: `deploy/containers/toolchain.Containerfile`

- [ ] **Step 1: Write the Containerfile**

Create `deploy/containers/toolchain.Containerfile` (pin the cargo-tool versions explicitly; the toolchain channel matches `rust-toolchain.toml`'s 1.94.0):

```dockerfile
# a2a-bridge toolchain image (Slice B2b-2): the reader image (ACP CLIs) + the Rust build toolchain, so
# the `impl` agent can build/test AND the bridge can run a deterministic verify. Used by `a2a-bridge
# implement`. NOT for the :ro reader agents (they don't compile).
FROM a2a-agent-reader:latest

# Native build deps node:24-slim (debian bookworm) lacks: a C toolchain + linker for cargo's codegen.
RUN apt-get update && apt-get install -y --no-install-recommends \
      build-essential pkg-config libssl-dev \
    && rm -rf /var/lib/apt/lists/*

# Rust pinned to the repo's rust-toolchain.toml channel (1.94.0) + the components CI uses.
ENV RUSTUP_HOME=/usr/local/rustup CARGO_HOME=/usr/local/cargo PATH=/usr/local/cargo/bin:$PATH
RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \
      | sh -s -- -y --no-modify-path --default-toolchain 1.94.0 --profile minimal \
        --component rustfmt --component clippy --component llvm-tools-preview

# Coverage tools available so an opt-in `cargo llvm-cov` command never hits "command not found".
# Pinned for reproducibility (chosen against the 1.94.0 toolchain).
RUN cargo install --locked cargo-llvm-cov --version 0.6.21 \
 && cargo install --locked cargo-tarpaulin --version 0.32.7
```

- [ ] **Step 2: Build the image (operator)**

Run:
```bash
docker build -t a2a-toolchain:latest -f deploy/containers/toolchain.Containerfile deploy/containers
docker run --rm a2a-toolchain:latest sh -c 'cargo --version && rustc --version && claude-agent-acp --version && cargo llvm-cov --version'
```
Expected: `cargo 1.94`, `rustc 1.94.0`, the ACP CLI version, and a llvm-cov version. (If a pinned cargo-tool version fails to resolve against 1.94.0, bump the `--version` to the latest compatible and note it in the commit.)

- [ ] **Step 3: Commit**

```bash
git add deploy/containers/toolchain.Containerfile
git commit -m "feat(b2b2): a2a-toolchain image (reader + Rust 1.94.0 + cargo coverage tools)"
```

---

## Task 7: the separate verify egress (net + proxy + filter)

**Files:**
- Create: `deploy/containers/tinyproxy.verify.filter`
- Modify: `deploy/containers/compose.egress.yaml`

- [ ] **Step 1: Write the verify allowlist**

Create `deploy/containers/tinyproxy.verify.filter` (cargo registries + git deps ONLY — verify needs no provider hosts; one POSIX-ERE per line, anchored like the agent filter):

```
# Verify-only egress: the cargo registries + GitHub for git deps. Anchored ERE, default-deny.
# Sparse index = index.crates.io; crate downloads = static.crates.io; git deps = github/codeload.
(^|\.)crates\.io$
(^|\.)static\.crates\.io$
(^|\.)index\.crates\.io$
(^|\.)github\.com$
(^|\.)codeload\.github\.com$
```

- [ ] **Step 2: Add the verify net + proxy to compose**

In `deploy/containers/compose.egress.yaml`, add the verify-only internal network under `networks:`:

```yaml
  a2a-verify-egress:
    name: a2a-verify-egress
    internal: true          # verify containers reach ONLY the verify-proxy
```

and add the `verify-proxy` service under `services:` (reuses the proxy image; overrides the filter via a read-only bind):

```yaml
  verify-proxy:
    build:
      context: .
      dockerfile: proxy.Containerfile
    image: a2a-egress-proxy:latest
    container_name: a2a-verify-proxy
    networks:
      - a2a-verify-egress     # reachable by verify containers
      - a2a-egress-external   # can reach the registries
    volumes:
      - ./tinyproxy.verify.filter:/etc/tinyproxy/filter:ro
    restart: unless-stopped
```

- [ ] **Step 3: Bring it up + gate the allowlist (operator)**

Run:
```bash
docker compose -f deploy/containers/compose.egress.yaml up -d --build
# crates.io reachable through the verify proxy:
docker run --rm --network a2a-verify-egress -e HTTPS_PROXY=http://a2a-verify-proxy:8888 \
  a2a-toolchain:latest sh -c 'curl -sS -o /dev/null -w "%{http_code}\n" https://static.crates.io/'
# a non-allowlisted host is REFUSED (default-deny):
docker run --rm --network a2a-verify-egress -e HTTPS_PROXY=http://a2a-verify-proxy:8888 \
  a2a-toolchain:latest sh -c 'curl -sS -o /dev/null -w "%{http_code}\n" https://example.com/ || echo blocked'
```
Expected: the registries return a 2xx/3xx; `example.com` is `blocked` (proxy refuses).

- [ ] **Step 4: Commit**

```bash
git add deploy/containers/tinyproxy.verify.filter deploy/containers/compose.egress.yaml
git commit -m "feat(b2b2): separate a2a-verify-egress proxy (registries only; creds never coexist)"
```

---

## Task 8: example config — `impl` toolchain image + `[verify]` block

**Files:**
- Modify: `examples/a2a-bridge.containerized.toml`

- [ ] **Step 1: Move the `impl` agent to the toolchain image**

In `examples/a2a-bridge.containerized.toml`, the `impl` agent block (lines 68-79): change ONLY its sandbox `image` from `a2a-agent-reader:latest` to `a2a-toolchain:latest` (everything else — kind, cmd, mount, access=rw, egress, creds volume — unchanged):

```toml
[agents.sandbox]
image   = "a2a-toolchain:latest"               # B2b-2: toolchain so the agent (and verify) has cargo
mount   = "/Users/wesleyjinks/code"            # == allowed_cwd_root (S2); :rw target is the session cwd under it
access  = "rw"
egress  = "locked"
network = "a2a-egress-internal"
proxy   = "http://a2a-egress-proxy:8888"
volumes = ["/Users/wesleyjinks/.config/a2a-creds/claude/.credentials.json:/root/.claude/.credentials.json"]
```

- [ ] **Step 2: Add the `[verify]` block**

Add after the `allowed_cwd_root` line (line 14), before `[registry]`:

```toml
# B2b-2: the build+test verify run after `a2a-bridge implement` commits. Bridge-deterministic (the
# agent can't fake it), REPORTED not gating. Separate egress (registries only; NO creds) so creds and
# registry-egress never coexist. Coverage is opt-in (commented) — it can't reuse the warm non-instrumented
# cache. Default gates are fmt/clippy/build/test, all --locked against the :ro clone.
[verify]
image   = "a2a-toolchain:latest"
cache   = "a2a-verify-cache"                   # per-repo: the bridge appends a hash of the source path
egress  = "locked"
network = "a2a-verify-egress"
proxy   = "http://a2a-verify-proxy:8888"
[[verify.commands]]
name = "fmt"
cmd  = "cargo fmt --all -- --check"
[[verify.commands]]
name = "clippy"
cmd  = "cargo clippy --all-targets --all-features --locked -- -D warnings"
[[verify.commands]]
name = "build"
cmd  = "cargo build --locked"
[[verify.commands]]
name = "test"
cmd  = "cargo test --locked"
# opt-in coverage (instrumented recompile; reported, never gates):
# [[verify.commands]]
# name = "coverage"
# cmd  = "cargo llvm-cov --workspace --locked --summary-only"
# gate = false
```

- [ ] **Step 3: Verify the config parses**

Run: `cargo run -q -p a2a-bridge -- --help >/dev/null 2>&1; cargo test -p a2a-bridge --bin a2a-bridge verify_config`
Then a parse smoke (the example must load):
```bash
cargo run -q -p a2a-bridge -- run-workflow nonexistent --input README.md --config examples/a2a-bridge.containerized.toml 2>&1 | grep -qi "unknown workflow" && echo "config parses"
```
Expected: `config parses` (the config loaded; only the workflow id is unknown — proves `[verify]` + the toolchain image parse without error).

- [ ] **Step 4: Commit**

```bash
git add examples/a2a-bridge.containerized.toml
git commit -m "feat(b2b2): impl agent on a2a-toolchain + the [verify] block in the example config"
```

---

## Task 9: live acceptance gate (operator-run, Docker/Podman)

**Files:** none (validation).

Prereqs: Tasks 6-8 done; `a2a-toolchain:latest` built; `docker compose -f deploy/containers/compose.egress.yaml up -d --build` (both proxies up); creds synced (`deploy/containers/sync-creds.sh claude`).

- [ ] **Step 1: happy path — verify PASS on a throwaway clone of THIS repo**

```bash
cargo build -p a2a-bridge
target/debug/a2a-bridge implement "Add a line to docs/SCRATCH-VERIFY.md noting the verify gate ran" \
  --repo /Users/wesleyjinks/code/a2a-bridge \
  --config examples/a2a-bridge.containerized.toml
```
Expected (stdout hand-off): the commit line + `verify: PASS  (fmt ✓ · clippy ✓ · build ✓ · test ✓)`. Source repo untouched; the clone left under `.a2a-implement/<id>`.

- [ ] **Step 2: warm cache — second run is faster**

Re-run Step 1 (a different scratch line). Expected: PASS again, and the build/test commands are noticeably faster (the `a2a-verify-cache-<hash>` volume is warm). Confirm the volume exists: `docker volume ls | grep a2a-verify-cache`.

- [ ] **Step 3: failure path — a gate failure stops + reports**

Prompt the agent to introduce a clippy violation (e.g. `--repo` a clone and a task that adds an unused `let x = 5;` with `-D warnings`), or manually create a clone with a clippy error and run a single-command `[verify]`. Expected (stderr): `[implement] verify: clippy failed:` + the clippy output; (stdout) `verify: FAIL at clippy  (fmt ✓ · clippy ✗)`; build/test did NOT run; **the commit still happened** and the hand-off still printed (verify is informational).

- [ ] **Step 4: containment proof (the load-bearing assertion)**

While a verify container runs (or via `docker inspect` on it), assert:
```bash
# verify ran on the verify egress, NOT the agent egress:
docker ps -a --filter ancestor=a2a-toolchain:latest --format '{{.Names}} {{.Networks}}' | grep a2a-verify-egress
# and that the verify container has NO creds mount + the clone is :ro:
docker inspect <verify-container> --format '{{json .Mounts}}' | grep -q '"RO":true' && echo "clone :ro OK"
docker inspect <verify-container> --format '{{json .Mounts}}' | grep -qi credentials && echo "LEAK: creds mounted" || echo "no creds mount OK"
```
Expected: the verify container is on `a2a-verify-egress`; the clone mount is `:ro`; **no creds mount**. (macOS Docker Desktop remaps bind ownership — prove containment via the mount flags + the network, not file ownership.)

- [ ] **Step 5: record the gate result** in the ADR (Task 10) — PASS/FAIL per step + the warm-vs-cold timing.

---

## Task 10: ADR-0020

**Files:**
- Create: `docs/adr/0020-containerized-agents-b2b2-verify.md`

- [ ] **Step 1: Write the ADR**

Create `docs/adr/0020-containerized-agents-b2b2-verify.md` capturing: the decision (bridge-deterministic post-commit verify, reported-not-gating, CI authoritative); the dual-review keystones folded (reuse `compose_sandbox`/`parse_egress` — no parallel egress schema; unforgeable per-command container-exit verdict; structured commands; `--locked` everywhere; separate verify egress; coverage opt-in; per-repo cache + single-flight); the component map; the live-gate result (from Task 9); and the deferred items (B2b-3 review→tweak loop, `--verify-strict`, per-language configs, coverage-floor gate). End with the trailer.

- [ ] **Step 2: Commit (with the trailer)**

```bash
git add docs/adr/0020-containerized-agents-b2b2-verify.md
git commit -F - <<'EOF'
docs(adr): ADR-0020 — B2b-2 build+test verify step

[summary per Step 1]

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
```

---

## Final verification (before finishing the branch)

- [ ] `cargo fmt --all` (workspace-wide; zero semantic change)
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` → clean
- [ ] `cargo llvm-cov clean --workspace && cargo llvm-cov --workspace --fail-under-lines 85` → workspace ≥ 85
- [ ] `cargo llvm-cov -p bridge-core --fail-under-lines 90` and `-p bridge-registry --fail-under-lines 90` → both met (new bridge-core code = `compose_verify`)
- [ ] the Task 9 live gate PASS recorded in ADR-0020
- [ ] Use **superpowers:finishing-a-development-branch** (Wesley's pattern: merge to main, then push)

---

## Self-review (spec coverage)

- Decision 1 (bridge-deterministic, post-commit, reported) → Task 5 integration.
- Decision 2 (unforgeable out-of-band exit) → Task 4 per-command `docker run` + container exit; Task 9 Step 3.
- Decision 3 (reuse `SandboxConfig`/`compose_sandbox`, no parallel schema; `VerifyConfig` via `parse_egress`) → Tasks 1 + 2.
- Decision 4 (structured `{name,cmd,gate}`) → Task 2 + Task 3 aggregation.
- Decision 5 (`--locked` everywhere) → Task 8 default commands.
- Decision 6 (separate verify egress; no creds) → Task 7 + Task 9 Step 4.
- Decision 7 (coverage opt-in; tools in image) → Task 6 (tools) + Task 8 (commented command).
- Decision 8 (`:ro` clone; per-repo cache; single-flight) → Task 1 (`access=Ro`) + Task 3 (`cache_volume_name`) + Task 8 (`cache` base). **Single-flight note:** B2b-2 ships per-repo cache keying (different repos isolated); concurrent *same-repo* `implement` runs are not expected (the operator drives `implement` serially) and cargo's own package-cache + target locks serialize within a shared volume — an explicit cross-container verify lock is deferred to the warm-pool/concurrency slice and called out in ADR-0020.
- Toolchain image pin / impl image before-after / hand-off contract / floors → Tasks 6 / 8 / 5 / Final.
