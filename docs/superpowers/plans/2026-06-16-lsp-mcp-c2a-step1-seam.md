# LSP-MCP C2a Step 1 — the cache/env seam (byte-for-byte refactor) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Consolidate the in-container cache env + mounts — today hardcoded in THREE independent sites — behind ONE `cache_binding(profile, ctx)` seam fed by a hardcoded `rust` `LanguageProfile`, with **zero behavior change** (byte-for-byte identical container argv/scripts). This fixes the code smell and locks the seam that C2a Step 2 (config-driven profiles + Go) builds on.

**Architecture:** A new pure module `crates/bridge-core/src/profile.rs` defines `CacheCtx{Fetch,Lsp,Verify}`, `CacheBinding{env,mounts}`, `LanguageProfile`, a `cache_binding` method, and `rust_profile()`. The three current sites — `compose_warm_fetch` (`implement.rs`), `compose_verify` (`bridge-core/sandbox.rs`), and the `implement` in-container-lsp mount (`main.rs`) — delete their hardcoded cargo env/mounts and consult the seam instead. Step 1 only consolidates what is in CODE today: the **fetch** env+mount and **verify** env+mount (both currently hardcoded in those functions) and the **lsp** MOUNT (hardcoded in `main.rs`). The lsp runtime ENV (`CARGO_HOME`/`CARGO_NET_OFFLINE`) stays where it is today — the agent's MCP `env` in config — and moves into the profile only in Step 2.

**Tech Stack:** Rust (crates/bridge-core, bin/a2a-bridge), Docker/OrbStack container argv composition.

**Handoff context (for a fresh session executing this plan):**
- **Branch:** `feat/lsp-mcp-c2a` (off `main`; the C2 spec is committed here). `git checkout feat/lsp-mcp-c2a` first.
- **Spec (read for the spec-compliance review stage):** `docs/superpowers/specs/2026-06-15-lsp-mcp-slice-c2-design.md` §2.2 (the seam) + §1 (per-language atoms / set selection). This plan is **Step 1 of 2**: the byte-for-byte seam refactor. Step 2 (combined image, `lsp-mcp` dep + typed detection, `[[languages]]` parse, profile-drive the seam, `--lang auto|id|none`, migrate configs, Go live gate) is a SEPARATE follow-on plan written once Step 1 ships.
- **The invariant for EVERY task:** the composed `(program, argv)` / shell script for warm-fetch, verify, and the impl-lsp mount must be **identical** to today's. Each task's test asserts that equality. `cargo test -p bridge-core -p a2a-bridge` + `cargo clippy --workspace -- -D warnings` + `cargo fmt --all -- --check` stay green.
- **Today's exact values (the refactor must reproduce these):**
  - warm-fetch (`implement.rs::compose_warm_fetch`): env `CARGO_HOME=/cargo`; mount `{warm_vol}:/cargo`; command `cargo fetch --locked`.
  - verify (`sandbox.rs::compose_verify`): script `cd '{clone}' && export CARGO_HOME=/cache/cargo CARGO_TARGET_DIR=/cache/target && mkdir -p "$CARGO_HOME" "$CARGO_TARGET_DIR" && {command}`; mount `{cache_vol}:/cache`.
  - impl-lsp mount (`main.rs`, the warm-impl setup): `ccfg.sandbox.volumes.push(format!("{cache}:/cargo:ro"))` (the `/lsp-target` volume push is NOT cache-binding scope — leave it).

---

## File structure

| File | Create/Modify | Responsibility |
| --- | --- | --- |
| `crates/bridge-core/src/profile.rs` | Create | The seam: `CacheCtx`, `CacheBinding`, `LanguageProfile`, `cache_binding`, `rust_profile()`. Pure + total + unit-tested. |
| `crates/bridge-core/src/lib.rs` | Modify | `pub mod profile;` + re-exports (`pub use profile::{CacheCtx, CacheBinding, LanguageProfile, rust_profile};`). |
| `crates/bridge-core/src/sandbox.rs` | Modify | `compose_verify` takes a `&CacheBinding` (env+mounts) instead of `cache_vol: &str`; builds the `export …` from `binding.env` + the volume `-v`s from `binding.mounts`. |
| `bin/a2a-bridge/src/verify.rs` | Modify | `run_verify` computes the Verify `CacheBinding` from `rust_profile()` + the resolved verify cache volume and passes it to `compose_verify`. |
| `bin/a2a-bridge/src/implement.rs` | Modify | `compose_warm_fetch` takes the Fetch `CacheBinding` + `fetch_cmd` instead of `cache_vol`; emits `-e`/`-v` from the binding + the command from `fetch_cmd`. |
| `bin/a2a-bridge/src/main.rs` | Modify | The impl-lsp mount block consults `rust_profile().cache_binding(Lsp, warm_vol, _)` for the `{cache}:/cargo:ro` mount instead of hardcoding it. |

---

### Task 1: The `profile.rs` seam (pure, hardcoded `rust`)

Create the seam with a single hardcoded `rust_profile()` and unit tests pinning each context's binding to today's exact values. No call-site changes yet — this task is self-contained and the tests are the spec of the seam.

**Files:**
- Create: `crates/bridge-core/src/profile.rs`
- Modify: `crates/bridge-core/src/lib.rs`

- [ ] **Step 1: Write the failing test** (append to a new `#[cfg(test)] mod tests` in `profile.rs`)

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rust_fetch_binding_matches_today() {
        let p = rust_profile();
        let b = p.cache_binding(CacheCtx::Fetch, "warmvol", "verifyvol");
        assert_eq!(b.env, vec![("CARGO_HOME".to_string(), "/cargo".to_string())]);
        assert_eq!(b.mounts, vec!["warmvol:/cargo".to_string()]);
    }

    #[test]
    fn rust_lsp_binding_is_ro_mount_no_env() {
        // Step 1: the lsp runtime ENV stays in config (the agent MCP env); the seam owns only the MOUNT.
        let p = rust_profile();
        let b = p.cache_binding(CacheCtx::Lsp, "warmvol", "verifyvol");
        assert!(b.env.is_empty(), "lsp env stays config-side in Step 1");
        assert_eq!(b.mounts, vec!["warmvol:/cargo:ro".to_string()]);
    }

    #[test]
    fn rust_verify_binding_matches_today() {
        let p = rust_profile();
        let b = p.cache_binding(CacheCtx::Verify, "warmvol", "verifyvol");
        assert_eq!(
            b.env,
            vec![
                ("CARGO_HOME".to_string(), "/cache/cargo".to_string()),
                ("CARGO_TARGET_DIR".to_string(), "/cache/target".to_string()),
            ]
        );
        assert_eq!(b.mounts, vec!["verifyvol:/cache".to_string()]);
    }

    #[test]
    fn rust_fetch_cmd_is_cargo_fetch_locked() {
        assert_eq!(rust_profile().fetch_cmd, "cargo fetch --locked");
        assert_eq!(rust_profile().warm_cache_base, "a2a-impl-lsp-cache");
    }
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p bridge-core --lib profile::tests`
Expected: FAIL to COMPILE (`profile` module / `rust_profile` / `CacheCtx` not defined).

- [ ] **Step 3: Write the seam** (the rest of `crates/bridge-core/src/profile.rs`, above the `mod tests`)

```rust
//! The single cache/env seam (C2 §2.2). One place maps a (language profile, container context) to the
//! cache env + volume mounts to apply — consumed by warm-fetch, verify, and the in-container-lsp mount,
//! replacing three independently-hardcoded cargo sites. Step 1 hardcodes a `rust` profile (byte-for-byte);
//! Step 2 makes `LanguageProfile` config-parsed + adds `go`.

/// A container context that needs language-specific cache env + mounts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CacheCtx {
    /// The warm-deps fetch container (populates the dep cache; must reach the network).
    Fetch,
    /// The in-container language server (reads the dep cache; offline).
    Lsp,
    /// The verify container (build/test against a persistent cache).
    Verify,
}

/// The env + volume mounts a profile contributes for one context.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CacheBinding {
    /// `(key, value)` pairs to export in the container.
    pub env: Vec<(String, String)>,
    /// Docker `-v` specs, e.g. `"vol:/path"` or `"vol:/path:ro"`.
    pub mounts: Vec<String>,
}

/// A per-language profile (an ATOM — selected as a set; never per-combo; C2 §1). Step 1 carries only the
/// fields the seam + warm-fetch consume; Step 2 extends it (verify commands, image override, config parse).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LanguageProfile {
    pub id: String,
    /// The warm-deps command (the dep fetch).
    pub fetch_cmd: String,
    /// The cache-volume BASE name the fetch fills (the per-repo suffix is appended by the caller).
    pub warm_cache_base: String,
    /// Where the dep cache mounts in the Fetch (rw) + Lsp (ro) containers.
    dep_cache_path: String,
    /// Where the verify cache mounts in the Verify container.
    verify_cache_path: String,
    /// Env exported in the Fetch container (network-capable — NO offline flag).
    fetch_env: Vec<(String, String)>,
    /// Env exported in the Lsp container. Empty in Step 1 (the lsp env is still config-side).
    lsp_env: Vec<(String, String)>,
    /// Env exported in the Verify container.
    verify_env: Vec<(String, String)>,
}

impl LanguageProfile {
    /// PURE + TOTAL. The env + mounts for `ctx`, given the resolved per-repo `warm_vol` (the dep cache)
    /// and `verify_vol` (the verify cache). Fetch mounts the dep cache rw; Lsp mounts it ro; Verify mounts
    /// the verify cache.
    pub fn cache_binding(&self, ctx: CacheCtx, warm_vol: &str, verify_vol: &str) -> CacheBinding {
        match ctx {
            CacheCtx::Fetch => CacheBinding {
                env: self.fetch_env.clone(),
                mounts: vec![format!("{warm_vol}:{}", self.dep_cache_path)],
            },
            CacheCtx::Lsp => CacheBinding {
                env: self.lsp_env.clone(),
                mounts: vec![format!("{warm_vol}:{}:ro", self.dep_cache_path)],
            },
            CacheCtx::Verify => CacheBinding {
                env: self.verify_env.clone(),
                mounts: vec![format!("{verify_vol}:{}", self.verify_cache_path)],
            },
        }
    }
}

/// The hardcoded Rust profile — reproduces today's three cargo sites exactly (Step 1).
pub fn rust_profile() -> LanguageProfile {
    LanguageProfile {
        id: "rust".to_string(),
        fetch_cmd: "cargo fetch --locked".to_string(),
        warm_cache_base: "a2a-impl-lsp-cache".to_string(),
        dep_cache_path: "/cargo".to_string(),
        verify_cache_path: "/cache".to_string(),
        fetch_env: vec![("CARGO_HOME".to_string(), "/cargo".to_string())],
        lsp_env: vec![], // Step 1: lsp env stays config-side (the agent MCP env).
        verify_env: vec![
            ("CARGO_HOME".to_string(), "/cache/cargo".to_string()),
            ("CARGO_TARGET_DIR".to_string(), "/cache/target".to_string()),
        ],
    }
}
```

- [ ] **Step 4: Register the module** in `crates/bridge-core/src/lib.rs`

Add (next to the other `pub mod` lines): `pub mod profile;`
And a re-export (next to the other `pub use` lines): `pub use profile::{cache_binding_unused as _, CacheBinding, CacheCtx, LanguageProfile};` — actually just: `pub use profile::{rust_profile, CacheBinding, CacheCtx, LanguageProfile};`

- [ ] **Step 5: Run the test to verify it passes**

Run: `cargo test -p bridge-core --lib profile::tests`
Expected: PASS (4 tests).

- [ ] **Step 6: Lint + commit**

Run: `cargo clippy -p bridge-core -- -D warnings && cargo fmt --all -- --check`
```bash
git add crates/bridge-core/src/profile.rs crates/bridge-core/src/lib.rs
git commit -m "feat(bridge-core): cache/env seam — CacheCtx/CacheBinding/LanguageProfile + rust_profile (C2a step 1)"
```

---

### Task 2: Route `compose_verify` through the seam

`compose_verify` currently hardcodes `export CARGO_HOME=/cache/cargo CARGO_TARGET_DIR=/cache/target` + `volumes: vec![format!("{cache_vol}:/cache")]`. Change it to accept a `&CacheBinding` and build the `export` from `binding.env` + the `-v`s from `binding.mounts`. `run_verify` computes the Verify binding from `rust_profile()`. Byte-for-byte.

**Files:**
- Modify: `crates/bridge-core/src/sandbox.rs` (`compose_verify`)
- Modify: `bin/a2a-bridge/src/verify.rs` (`run_verify`)

- [ ] **Step 1: Write the failing test** (append to `sandbox.rs`'s `#[cfg(test)] mod tests`)

```rust
#[test]
fn compose_verify_via_binding_is_byte_for_byte() {
    use crate::profile::{rust_profile, CacheCtx};
    use crate::session_cwd::SessionCwd;
    let clone = SessionCwd::new("/Users/x/code/.a2a-implement/impl-1-abc").unwrap();
    let egress = EgressPolicy::Locked {
        network: "net".into(),
        proxy: "http://p:8888".into(),
        no_proxy: Some("localhost".into()),
    };
    let binding = rust_profile().cache_binding(CacheCtx::Verify, "warmvol", "a2a-verify-cache-x");
    let (prog, argv) = compose_verify(None, "img:latest", &egress, &clone, &binding, "cargo build --locked");
    // The script keeps cd + the SAME exported vars (now from the binding) + mkdir + the command.
    let script = argv.last().unwrap();
    assert!(script.contains("export CARGO_HOME=/cache/cargo CARGO_TARGET_DIR=/cache/target"), "{script}");
    assert!(script.starts_with("cd '/Users/x/code/.a2a-implement/impl-1-abc' && export "), "{script}");
    assert!(script.trim_end().ends_with("&& cargo build --locked"), "{script}");
    // The cache mount comes from the binding.
    assert!(argv.iter().any(|a| a == "a2a-verify-cache-x:/cache"), "{argv:?}");
    let _ = prog;
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p bridge-core --lib compose_verify_via_binding_is_byte_for_byte`
Expected: FAIL to COMPILE (`compose_verify` still takes `cache_vol: &str`, not `&CacheBinding`).

- [ ] **Step 3: Change `compose_verify`** in `crates/bridge-core/src/sandbox.rs`

Replace the signature + body (the doc comment above it can keep its intent; update the CARGO_HOME line in the comment to "the binding's env"):

```rust
pub fn compose_verify(
    runtime: Option<&str>,
    image: &str,
    egress: &EgressPolicy,
    clone: &crate::session_cwd::SessionCwd,
    cache: &crate::profile::CacheBinding,
    command: &str,
) -> (String, Vec<String>) {
    // Export the binding's env (each `K=V`), make the dirs it points at, then run the command. `cd` first
    // (compose_sandbox emits no --workdir; the reader base sets WORKDIR /work). `&&` chains so a failed cd
    // or export surfaces as a verify failure and the command's exit is the script's exit.
    let exports = cache
        .env
        .iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join(" ");
    let mkdirs = cache
        .env
        .iter()
        .map(|(k, _)| format!("\"${k}\""))
        .collect::<Vec<_>>()
        .join(" ");
    let script = format!(
        "cd '{clone}' && export {exports} && mkdir -p {mkdirs} && {command}",
        clone = clone.as_str(),
    );
    let sb = SandboxConfig {
        runtime: runtime.map(str::to_string),
        image: image.to_string(),
        mount: clone.as_str().to_string(),
        access: MountAccess::Ro,
        egress: egress.clone(),
        volumes: cache.mounts.clone(),
    };
    compose_sandbox(&sb, "sh", &["-c".to_string(), script], &[])
}
```

(Note: `mkdir -p "$CARGO_HOME" "$CARGO_TARGET_DIR"` is reproduced because the rust Verify binding's env keys are exactly `CARGO_HOME`, `CARGO_TARGET_DIR` in that order — the `mkdirs` join yields `"$CARGO_HOME" "$CARGO_TARGET_DIR"`. Byte-for-byte.)

- [ ] **Step 4: Update `run_verify`** in `bin/a2a-bridge/src/verify.rs` to compute + pass the binding

Replace the `compose_verify(...)` call inside the loop (and the surrounding `cache_vol` use). The Verify binding's `warm_vol` is irrelevant (Verify mounts only the verify cache), so pass `""`:

```rust
let binding = bridge_core::profile::rust_profile().cache_binding(
    bridge_core::profile::CacheCtx::Verify,
    "",
    cache_vol,
);
for c in &cfg.commands {
    let (prog, argv) = bridge_core::sandbox::compose_verify(
        cfg.runtime.as_deref(),
        &cfg.image,
        &cfg.egress,
        clone,
        &binding,
        &c.cmd,
    );
    // ... unchanged: run the runner, push VerifyResult, break on gate failure
}
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p bridge-core --lib compose_verify_via_binding_is_byte_for_byte && cargo test -p a2a-bridge --lib verify::`
Expected: PASS (incl. the existing verify tests — `compose_verify_ro_clone_plus_cache_reuses_compose_sandbox` and `compose_verify_open_egress_has_no_network` will need their `compose_verify(...)` calls updated to pass a `CacheBinding` built from `rust_profile()` — update them to the new signature; their assertions about egress/mount should still hold).

- [ ] **Step 6: Lint + commit**

Run: `cargo clippy -p bridge-core -p a2a-bridge -- -D warnings && cargo fmt --all -- --check`
```bash
git add crates/bridge-core/src/sandbox.rs bin/a2a-bridge/src/verify.rs
git commit -m "refactor(verify): compose_verify reads the cache CacheBinding (seam) — byte-for-byte (C2a step 1)"
```

---

### Task 3: Route `compose_warm_fetch` through the seam

`compose_warm_fetch` hardcodes `-e CARGO_HOME=/cargo`, `-v {cache_vol}:/cargo`, and `cargo fetch --locked`. Change it to take the Fetch `CacheBinding` + the `fetch_cmd`; the caller (`warm_lsp_deps_step`) computes them from `rust_profile()`.

**Files:**
- Modify: `bin/a2a-bridge/src/implement.rs` (`compose_warm_fetch`)
- Modify: `bin/a2a-bridge/src/main.rs` (`warm_lsp_deps_step` — the caller)

- [ ] **Step 1: Write the failing test** (append to `implement.rs`'s `#[cfg(test)] mod tests`; if none, create one)

```rust
#[test]
fn compose_warm_fetch_via_binding_is_byte_for_byte() {
    use bridge_core::profile::{rust_profile, CacheCtx};
    let p = rust_profile();
    let binding = p.cache_binding(CacheCtx::Fetch, "warmvol", "");
    let e = WarmEgress { network: "net".into(), proxy: "http://p:8888".into() };
    let (prog, argv) = compose_warm_fetch("docker", "img:latest", "/clone", &binding, &p.fetch_cmd, &e);
    assert_eq!(prog, "docker");
    assert!(argv.iter().any(|a| a == "CARGO_HOME=/cargo"), "{argv:?}");
    assert!(argv.iter().any(|a| a == "warmvol:/cargo"), "{argv:?}");
    assert_eq!(argv.last().unwrap(), "cargo fetch --locked");
    assert!(argv.iter().any(|a| a == "/clone:/work"), "{argv:?}");
}
```

(Confirm the real `compose_warm_fetch` signature + `WarmEgress` fields by reading `implement.rs` first; match the param order. Today it is `compose_warm_fetch(runtime, image, clone, cache_vol, e)`.)

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p a2a-bridge --lib compose_warm_fetch_via_binding_is_byte_for_byte`
Expected: FAIL to COMPILE (signature has `cache_vol`, not a binding).

- [ ] **Step 3: Change `compose_warm_fetch`** in `bin/a2a-bridge/src/implement.rs`

```rust
pub fn compose_warm_fetch(
    runtime: &str,
    image: &str,
    clone: &str,
    cache: &bridge_core::profile::CacheBinding,
    fetch_cmd: &str,
    e: &WarmEgress,
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
    argv.push(format!("{clone}:/work"));
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
```

(Byte-for-byte: with the rust Fetch binding, `cache.env = [("CARGO_HOME","/cargo")]` → `-e CARGO_HOME=/cargo`, `cache.mounts = ["warmvol:/cargo"]` → `-v warmvol:/cargo`, `fetch_cmd = "cargo fetch --locked"`. The `-e CARGO_HOME` now lands AFTER the proxy `-e`s and BEFORE the `clone:/work -v` — same as today, since today CARGO_HOME was the last `-e` before the `-v`s.)

- [ ] **Step 4: Update the caller** `warm_lsp_deps_step` in `bin/a2a-bridge/src/main.rs`

The function currently derives `cache_vol = verify::cache_volume_name("a2a-impl-lsp-cache", repo)` and calls `compose_warm_fetch(runtime, &vcfg.image, &clone_canon, &cache_vol, &egress)`. Change to use the profile:

```rust
let p = bridge_core::profile::rust_profile();
let cache_vol = verify::cache_volume_name(&p.warm_cache_base, &repo_canon.to_string_lossy());
let binding = p.cache_binding(bridge_core::profile::CacheCtx::Fetch, &cache_vol, "");
let (program, argv) =
    implement::compose_warm_fetch(runtime, &vcfg.image, &clone_canon, &binding, &p.fetch_cmd, &egress);
```

(Keep the rest of `warm_lsp_deps_step` — the egress extraction, the `eprintln!`, the `docker_runner` call, the `Some(cache_vol)` return — unchanged. The returned `cache_vol` still feeds the impl-lsp mount in Task 4.)

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p a2a-bridge --lib compose_warm_fetch_via_binding_is_byte_for_byte && cargo test -p a2a-bridge`
Expected: PASS.

- [ ] **Step 6: Lint + commit**

Run: `cargo clippy -p a2a-bridge -- -D warnings && cargo fmt --all -- --check`
```bash
git add bin/a2a-bridge/src/implement.rs bin/a2a-bridge/src/main.rs
git commit -m "refactor(warm): compose_warm_fetch reads the Fetch CacheBinding (seam) — byte-for-byte (C2a step 1)"
```

---

### Task 4: Route the impl-lsp mount through the seam

The warm-impl setup in `main.rs` hardcodes `ccfg.sandbox.volumes.push(format!("{cache}:/cargo:ro"))`. Replace it with the Lsp binding's mount(s). (Leave the separate `/lsp-target` volume push — it is RA's writable target dir, not a dep-cache binding.)

**Files:**
- Modify: `bin/a2a-bridge/src/main.rs` (the impl-lsp mount block, currently `if let Some(cache) = impl_lsp_cache_vol { ... "{cache}:/cargo:ro" ... }`)

- [ ] **Step 1: Read the block** to confirm the exact variable names (`impl_lsp_cache_vol`, `ccfg.sandbox.volumes`) and that the `/lsp-target` push directly follows.

- [ ] **Step 2: Replace the mount push** with the seam:

```rust
if let Some(cache) = impl_lsp_cache_vol {
    let lsp = bridge_core::profile::rust_profile()
        .cache_binding(bridge_core::profile::CacheCtx::Lsp, cache, "");
    ccfg.sandbox.volumes.extend(lsp.mounts);
}
```

(Byte-for-byte: the rust Lsp binding yields `mounts = ["{cache}:/cargo:ro"]`, so `extend` pushes the identical single string. The lsp ENV is unchanged — it still comes from the agent's MCP `env` in config.)

- [ ] **Step 3: Build + run the impl-path tests**

Run: `cargo test -p a2a-bridge`
Expected: PASS. (There may be no direct unit test asserting this mount string; if a test for the warm-impl mount exists, it should be unchanged. If none exists, the byte-for-byte guarantee is the single-string `extend` — add a focused assertion only if a seam already exists to test it; otherwise the existing impl tests + clippy/build are the gate.)

- [ ] **Step 4: Full workspace gate** — confirm no cross-crate regression from the `compose_verify` signature change:

Run: `cargo build --workspace && cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --all -- --check && cargo test -p bridge-core -p a2a-bridge`
Expected: all green.

- [ ] **Step 5: Commit**

```bash
git add bin/a2a-bridge/src/main.rs
git commit -m "refactor(impl-lsp): impl-lsp mount reads the Lsp CacheBinding (seam) — byte-for-byte (C2a step 1)"
```

---

## Step 1 done — what ships

The three cargo cache sites now route through one `cache_binding` seam fed by `rust_profile()`; behavior is byte-for-byte identical (every task's test pins the composed argv/script to today's values). The smell is gone and the seam is locked.

**Final review before the Step-2 plan:** run the bridge's own review on the Step-1 diff (`git diff main...HEAD` scoped to the profile/seam changes) — host code-review (codex gpt-5.5) — focusing on the byte-for-byte guarantee (no behavior drift) + the seam's totality. Then proceed to write the **C2a Step 2** plan against the now-concrete seam.

## C2a Step 2 (SEPARATE follow-on plan — NOT in scope here)

Written once Step 1 ships. Scope (from spec §1–§3, §6–§8): combined Rust+Go toolchain image (Containerfile); add `lsp-mcp` path dep + typed `detect_repo_langs`/`LangDetection`; `[[languages]]` config parse (`LanguageProfile` gains `verify_commands` + `image`; remove `[verify].commands` + add the legacy-reject parse error + move the ≥1-command invariant to the matched profile); make the seam profile-driven (selected profile, not hardcoded `rust_profile()`; move the lsp runtime env from the agent MCP config into `lsp_env`); `implement --lang <auto|id|none>` + the preflight (hard-fail-with-options / `none` → bare/verify-SKIPPED); flip the impl lsp `--lang auto`; migrate all tracked implement configs; the Go `implement` live gate (incl. third-party gopls nav from the warmed cache) + byte-for-byte Rust regression.

---

## Self-review notes

**Spec §2.2 coverage:** the seam (`CacheCtx`/`CacheBinding`/`LanguageProfile`/`cache_binding`) — Task 1; the three sites consolidated — Tasks 2 (verify), 3 (warm-fetch), 4 (impl-lsp mount). The "Step 1 = pure refactor, byte-for-byte" sequencing (spec §3/§8) — every task asserts equality to today's values. The lsp-env-stays-config-side nuance (the spec-review finding that lsp ENV is config, only the MOUNT is in main.rs) — Task 1's `lsp_env: vec![]` + Task 4 (mount only).

**Placeholder scan:** no TBD/TODO. Two "confirm against the real file" notes (Task 3 `compose_warm_fetch` param order + `WarmEgress` fields; Task 4 variable names) are READ-FIRST verifications of names I gave concrete current values for, not deferred work.

**Type consistency:** `CacheCtx`/`CacheBinding`/`LanguageProfile`/`rust_profile` are defined in Task 1 and used with those exact names in Tasks 2–4. `compose_verify`'s new `cache: &CacheBinding` param (Task 2) matches the `run_verify` call (Task 2 Step 4). `compose_warm_fetch`'s new `(cache: &CacheBinding, fetch_cmd: &str)` (Task 3) matches the `warm_lsp_deps_step` caller (Task 3 Step 4).
