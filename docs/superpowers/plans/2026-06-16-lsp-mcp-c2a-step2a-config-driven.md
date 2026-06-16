# LSP-MCP C2a Step 2a — config-driven profiles (Rust byte-for-byte) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans. Steps use checkbox (`- [ ]`) syntax.

**Goal:** Make the `cache_binding` seam **config-driven** — parse `[[languages]]` profiles from the config and select one (Rust, for now), instead of the hardcoded `rust_profile()`. The warm command + caches + verify commands all come from the parsed profile. **Zero behavior change** (the migrated `rust` profile reproduces today's values byte-for-byte). This crystallizes the `[[languages]]` schema that Step 2b (Go) drops onto.

**Architecture:** Step 1 built `LanguageProfile` + `cache_binding` (bridge-core) with a hardcoded `rust_profile()`. Step 2a (1) extends `LanguageProfile` with `verify_commands` + an optional `image`; (2) adds a `LanguageToml` config struct + `[[languages]]` parsing in `RegistryConfig`, with `to_profile()`; (3) **removes `[verify].commands`** (legacy → explicit parse error) and **moves the "≥1 verify command" invariant** to the matched profile; (4) routes `warm_lsp_deps_step` + `run_verify` through the **config-selected** profile instead of `rust_profile()`; (5) migrates the example configs. **Deferred to Step 2b (NOT here):** language detection, `--lang`, the combined Go image, the **lsp runtime env move** (the impl agent's MCP `CARGO_HOME`/`CARGO_NET_OFFLINE` env STAYS in config for 2a — only WARM + VERIFY become profile-driven; the Lsp binding stays mount-only as in Step 1), and the `go` profile + live gate.

**Tech Stack:** Rust (crates/bridge-core, bin/a2a-bridge), TOML config.

**Handoff context:**
- **Branch:** `feat/lsp-mcp-c2a-step2` (off `main`, which has C2a Step 1 + the C2 spec). `git checkout feat/lsp-mcp-c2a-step2` first.
- **Spec:** `docs/superpowers/specs/2026-06-15-lsp-mcp-slice-c2-design.md` §2 (the `[[languages]]` schema), §2.1 (no-backward-compat + legacy-reject + the invariant move), §1 (per-language atoms). This plan is **Step 2a of C2a Step 2**; Step 2b (Go) is the follow-on plan.
- **Byte-for-byte invariant:** the migrated `rust` profile must reproduce today's `compose_warm_fetch`/`compose_verify` exactly. The Step-1 byte-for-byte tests (`compose_verify_via_binding_is_byte_for_byte`, `compose_warm_fetch_via_binding_is_byte_for_byte`) must STAY GREEN; the verify commands are now sourced from the profile, so a new test pins the rust profile's `verify_commands` to today's `[verify].commands`.
- **Today's rust values (the migration target):** `fetch = "cargo fetch --locked"`, `warm_cache = "a2a-impl-lsp-cache"`, fetch_env `CARGO_HOME=/cargo`, verify_env `CARGO_HOME=/cache/cargo`+`CARGO_TARGET_DIR=/cache/target`, dep_cache `/cargo`, verify_cache `/cache`; verify_commands = the 4 in `examples/a2a-bridge.containerized.toml` `[[verify.commands]]` (fmt/clippy/build/test, the test cmd with `--exclude bridge-container` + the 3 `--skip process::tests::…`). Confirm against the real config when migrating.

---

## File structure

| File | Create/Modify | Responsibility |
| --- | --- | --- |
| `crates/bridge-core/src/profile.rs` | Modify | `LanguageProfile` gains `verify_commands: Vec<VerifyCommand>` + `image: Option<String>`; add a `VerifyCommand { name, cmd, gate }` (or reuse a shared one); `rust_profile()` keeps populating the existing fields + the new ones (today's 4 verify commands) so its tests + the Step-1 byte-for-byte tests stay green. |
| `bin/a2a-bridge/src/config.rs` | Modify | `LanguageToml` + `LanguageVerifyToml`; `RegistryConfig.languages: Vec<LanguageToml>`; `LanguageToml::to_profile() -> LanguageProfile`; **reject `[verify].commands`** (legacy) with a clear parse error; the "≥1 verify command" check moves to `to_profile` (a profile needs ≥1 verify command). |
| `bin/a2a-bridge/src/main.rs` | Modify | `warm_lsp_deps_step` + the verify step select the profile from the parsed config (the single `rust` profile in 2a) instead of `rust_profile()`; `run_verify` is fed the profile's `verify_commands`. |
| `bin/a2a-bridge/src/verify.rs` | Modify | `run_verify` takes the verify commands from the profile (a `&[VerifyCommand]`) rather than `cfg.commands`. |
| `examples/a2a-bridge.containerized.toml` + `.sonnet.toml` + `.podman.toml` | Modify | Migrate `[[verify.commands]]` → a `[[languages]] id="rust"` profile (warm/verify/env/cache); remove `[verify].commands`. Byte-for-byte. |

---

### Task 1: Extend `LanguageProfile` with `verify_commands` + `image`

**Files:** Modify `crates/bridge-core/src/profile.rs`

- [ ] **Step 1: Write the failing test** (append to `profile.rs` `mod tests`)

```rust
#[test]
fn rust_profile_carries_verify_commands_and_no_image_override() {
    let p = rust_profile();
    assert_eq!(p.image, None, "rust uses [verify].image (no per-profile override)");
    // The 4 verify commands, in order, byte-for-byte with today's [verify].commands.
    let names: Vec<&str> = p.verify_commands.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names, vec!["fmt", "clippy", "build", "test"]);
    assert_eq!(p.verify_commands[0].cmd, "cargo fmt --all -- --check");
    assert!(p.verify_commands.iter().all(|c| c.gate), "all default-gate true");
    assert_eq!(
        p.verify_commands[3].cmd,
        "cargo test --workspace --locked --exclude bridge-container -- --skip process::tests::terminate_reaps_child_no_zombie --skip process::tests::term_ignoring_loop_forces_group_sigkill --skip process::tests::drop_group_kills_descendants"
    );
}
```

- [ ] **Step 2: Run it, see it fail** — `cargo test -p bridge-core --lib rust_profile_carries_verify_commands` → FAIL to COMPILE (`verify_commands`/`image` fields absent).

- [ ] **Step 3: Add the fields + a `VerifyCommand`** to `profile.rs`:

```rust
/// One verify command (the profile-owned analogue of the old `[[verify.commands]]`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifyCommand {
    pub name: String,
    pub cmd: String,
    pub gate: bool,
}
```
Add to `LanguageProfile`:
```rust
    /// Per-profile verify commands (replaces the old top-level `[verify].commands`).
    pub verify_commands: Vec<VerifyCommand>,
    /// Optional per-profile container image override (default: `[verify].image`).
    pub image: Option<String>,
```
Extend `rust_profile()` (keep all existing fields) with:
```rust
        image: None,
        verify_commands: vec![
            VerifyCommand { name: "fmt".into(),    cmd: "cargo fmt --all -- --check".into(),                          gate: true },
            VerifyCommand { name: "clippy".into(), cmd: "cargo clippy --all-targets --all-features --locked -- -D warnings".into(), gate: true },
            VerifyCommand { name: "build".into(),  cmd: "cargo build --locked".into(),                                gate: true },
            VerifyCommand { name: "test".into(),   cmd: "cargo test --workspace --locked --exclude bridge-container -- --skip process::tests::terminate_reaps_child_no_zombie --skip process::tests::term_ignoring_loop_forces_group_sigkill --skip process::tests::drop_group_kills_descendants".into(), gate: true },
        ],
```

- [ ] **Step 4: Run → PASS** — `cargo test -p bridge-core --lib`. The existing Step-1 byte-for-byte/profile tests stay green (the new fields don't change `cache_binding`).

- [ ] **Step 5: Commit** — `cargo clippy -p bridge-core -- -D warnings && cargo fmt --all -- --check`, then:
```bash
git add crates/bridge-core/src/profile.rs
git commit -m "feat(bridge-core): LanguageProfile gains verify_commands + image override (C2a step 2a)"
```

---

### Task 2: `[[languages]]` config parse + `to_profile` + legacy `[verify].commands` reject

**Files:** Modify `bin/a2a-bridge/src/config.rs`

- [ ] **Step 1: Write the failing tests** (append to `config.rs` `mod tests`)

```rust
#[test]
fn languages_parse_to_profile() {
    let toml = r#"
default = "impl"
[server]
addr = "127.0.0.1:8080"
[verify]
image = "a2a-toolchain:latest"
cache = "a2a-verify-cache"
egress = "open"
[[languages]]
id = "rust"
fetch = "cargo fetch --locked"
warm_cache = "a2a-impl-lsp-cache"
[[languages.verify]]
name = "build"
cmd = "cargo build --locked"
"#;
    let c = RegistryConfig::parse(toml).unwrap();
    let profs = c.language_profiles().unwrap();
    assert_eq!(profs.len(), 1);
    assert_eq!(profs[0].id, "rust");
    assert_eq!(profs[0].fetch_cmd, "cargo fetch --locked");
    assert_eq!(profs[0].verify_commands.len(), 1);
}

#[test]
fn legacy_verify_commands_is_rejected() {
    let toml = r#"
default = "impl"
[server]
addr = "127.0.0.1:8080"
[verify]
image = "img"
cache = "c"
egress = "open"
[[verify.commands]]
name = "build"
cmd = "cargo build"
"#;
    let err = RegistryConfig::parse(toml).unwrap_err().to_string();
    assert!(err.contains("verify.commands") && err.contains("languages"), "got {err}");
}

#[test]
fn profile_needs_at_least_one_verify_command() {
    let toml = r#"
default = "impl"
[server]
addr = "127.0.0.1:8080"
[[languages]]
id = "rust"
fetch = "cargo fetch --locked"
warm_cache = "a2a-impl-lsp-cache"
"#;
    let c = RegistryConfig::parse(toml).unwrap();
    assert!(c.language_profiles().unwrap_err().to_string().contains("at least one"));
}
```

- [ ] **Step 2: Run → fail** — `cargo test -p a2a-bridge languages_parse_to_profile legacy_verify_commands_is_rejected profile_needs_at_least_one_verify_command` → FAIL (no `languages` field / `language_profiles` / `to_profile`).

- [ ] **Step 3: Add the config types + parsing.** In `config.rs`:

```rust
#[derive(Debug, serde::Deserialize)]
pub struct LanguageVerifyToml {
    pub name: String,
    pub cmd: String,
    #[serde(default = "default_gate")]
    pub gate: bool,
}

#[derive(Debug, serde::Deserialize)]
pub struct LanguageToml {
    pub id: String,
    pub fetch: String,
    pub fetch_env: Option<std::collections::BTreeMap<String, String>>,
    pub warm_cache: String,
    pub dep_cache_path: Option<String>,
    pub verify_cache_path: Option<String>,
    pub lsp_env: Option<std::collections::BTreeMap<String, String>>,
    pub verify_env: Option<std::collections::BTreeMap<String, String>>,
    pub image: Option<String>,
    #[serde(default)]
    pub verify: Vec<LanguageVerifyToml>,
}
```
Add to `RegistryConfig`: `#[serde(default)] pub languages: Vec<LanguageToml>,`.
Add the legacy reject — `VerifyToml` gains an OPTIONAL legacy field whose PRESENCE errors. Since `VerifyToml.commands` is removed in this slice, change it to:
```rust
    /// REMOVED — commands moved to `[[languages.verify]]`. Kept ONLY to reject legacy configs loudly
    /// (VerifyToml has no `deny_unknown_fields`, so a silent drop would otherwise mask stale configs).
    #[serde(default)]
    pub commands: Vec<VerifyCommandToml>,
```
and in `RegistryConfig::parse` (after deserialize), add:
```rust
    if let Some(v) = &cfg.verify {
        if !v.commands.is_empty() {
            return Err(ConfigError::Registry(
                "[verify].commands / [[verify.commands]] is removed — move them to [[languages.verify]]".into(),
            ));
        }
    }
```
Add `RegistryConfig::language_profiles()` + `LanguageToml::to_profile()` (the "≥1 verify command" invariant lives here):
```rust
impl RegistryConfig {
    pub fn language_profiles(&self) -> Result<Vec<bridge_core::profile::LanguageProfile>, ConfigError> {
        self.languages.iter().map(LanguageToml::to_profile).collect()
    }
}
impl LanguageToml {
    pub fn to_profile(&self) -> Result<bridge_core::profile::LanguageProfile, ConfigError> {
        if self.verify.is_empty() {
            return Err(ConfigError::Registry(format!(
                "[[languages]] id={:?} needs at least one [[languages.verify]] command", self.id
            )));
        }
        Ok(bridge_core::profile::LanguageProfile::from_parts(
            self.id.clone(),
            self.fetch.clone(),
            self.warm_cache.clone(),
            self.dep_cache_path.clone().unwrap_or_else(|| "/cargo".into()),
            self.verify_cache_path.clone().unwrap_or_else(|| "/cache".into()),
            map_pairs(&self.fetch_env),
            map_pairs(&self.lsp_env),
            map_pairs(&self.verify_env),
            self.image.clone(),
            self.verify.iter().map(|v| bridge_core::profile::VerifyCommand {
                name: v.name.clone(), cmd: v.cmd.clone(), gate: v.gate,
            }).collect(),
        ))
    }
}
fn map_pairs(m: &Option<std::collections::BTreeMap<String, String>>) -> Vec<(String, String)> {
    m.as_ref().map(|m| m.iter().map(|(k, v)| (k.clone(), v.clone())).collect()).unwrap_or_default()
}
```
(`LanguageProfile::from_parts` is a new pub constructor on the struct — add it in Task 1's file, or here as a `bridge-core` change folded into Task 1. The private fields `dep_cache_path`/`verify_cache_path`/`*_env` mean the constructor must live in `profile.rs`; add `pub fn from_parts(...)` there.)

- [ ] **Step 4: Run → PASS** — the 3 new config tests + the whole `cargo test -p a2a-bridge` (existing config tests that build a `[verify]` with `[[verify.commands]]` must be MIGRATED to `[[languages]]` or updated to expect the reject — fix them).

- [ ] **Step 5: Commit** — clippy + fmt, then:
```bash
git add bin/a2a-bridge/src/config.rs crates/bridge-core/src/profile.rs
git commit -m "feat(config): [[languages]] profiles + legacy [verify].commands reject + invariant move (C2a step 2a)"
```

---

### Task 3: Route warm + verify through the CONFIG-selected profile

**Files:** Modify `bin/a2a-bridge/src/main.rs` (`warm_lsp_deps_step`, the verify step), `bin/a2a-bridge/src/verify.rs` (`run_verify`)

- [ ] **Step 1: Write the failing test** — a `run_verify` test that drives the profile's `verify_commands` (inject a fake runner; assert it runs the profile's commands in order, gating on the first failure), in `verify.rs`. (Shape mirrors the existing `run_verify` tests; pass a `&[VerifyCommand]` from `rust_profile().verify_commands`.)

- [ ] **Step 2: Run → fail** (signature mismatch — `run_verify` still reads `cfg.commands`).

- [ ] **Step 3: Implement.**
  - `run_verify` takes `commands: &[bridge_core::profile::VerifyCommand]` instead of reading `cfg.commands`; the loop iterates those. The Verify `CacheBinding` is computed from the SELECTED profile (`profile.cache_binding(Verify, "", cache_vol)`), not `rust_profile()`.
  - The verify step in `main.rs` selects the profile: `let profile = pick_profile(&snapshot_or_cfg)?;` — in Step 2a, `pick_profile` returns the single config `language_profiles()` entry whose `id == "rust"` (hardcoded id; Step 2b replaces this with detection). It then calls `run_verify(vcfg, clone, cache_vol, &profile.verify_commands, &profile, runner, max)` (thread the profile for the binding).
  - `warm_lsp_deps_step` uses the selected profile (its `fetch_cmd` + `warm_cache_base` + `cache_binding(Fetch)`) instead of `rust_profile()`.
  - **Byte-for-byte:** with the migrated `rust` config profile, the warm argv + verify scripts + commands are identical to today (Task 4 migrates the config; the Step-1 byte-for-byte tests + Task 1's verify-commands test pin it).

- [ ] **Step 4: Run → PASS** — `cargo test -p a2a-bridge -p bridge-core`.

- [ ] **Step 5: Commit** — clippy + fmt, then:
```bash
git add bin/a2a-bridge/src/main.rs bin/a2a-bridge/src/verify.rs
git commit -m "refactor(implement): warm + verify use the config-selected LanguageProfile (C2a step 2a)"
```

---

### Task 4: Migrate the example configs ([verify].commands → [[languages]] rust profile)

**Files:** Modify `examples/a2a-bridge.containerized.toml`, `examples/a2a-bridge.containerized.sonnet.toml`, `examples/a2a-bridge.containerized.podman.toml` (+ any other tracked config with `[[verify.commands]]` — `grep -rl '\[\[verify.commands\]\]' examples/`).

- [ ] **Step 1:** For each config: keep `[verify]` infra (`image`/`cache`/`egress`/`network`/`proxy`/`no_proxy`/`runtime`); DELETE the `[[verify.commands]]` blocks; ADD a `[[languages]] id="rust"` profile carrying `fetch = "cargo fetch --locked"`, `warm_cache = "a2a-impl-lsp-cache"`, `dep_cache_path = "/cargo"`, `verify_cache_path = "/cache"`, `fetch_env = { CARGO_HOME = "/cargo" }`, `verify_env = { CARGO_HOME = "/cache/cargo", CARGO_TARGET_DIR = "/cache/target" }`, and the 4 `[[languages.verify]]` commands (fmt/clippy/build/test) copied VERBATIM from the old `[[verify.commands]]`. (Leave `lsp_env` unset in 2a — the impl agent's MCP `CARGO_HOME`/`CARGO_NET_OFFLINE` env STAYS as-is; the lsp-env move is Step 2b.)

- [ ] **Step 2: Verify each config parses + the profile round-trips** — add a parse test (or run `a2a-bridge run-workflow … --config <each>` dry, or a `config::tests` parse assertion) confirming `language_profiles()` yields the rust profile with the 4 commands.

- [ ] **Step 3: DoD — the dogfood still works byte-for-byte.** Run an `implement` on a small Rust task with the migrated `containerized.sonnet.toml` (the loop currently used) and confirm warm + verify behave exactly as before (verify PASS, same commands). This is the live byte-for-byte gate for the config migration.

- [ ] **Step 4: Commit:**
```bash
git add examples/a2a-bridge.containerized.toml examples/a2a-bridge.containerized.sonnet.toml examples/a2a-bridge.containerized.podman.toml
git commit -m "config(implement): migrate [verify].commands -> [[languages]] rust profile (C2a step 2a)"
```

---

## Step 2a done — what ships

The seam is now CONFIG-DRIVEN: `[[languages]]` profiles are parsed + selected (Rust), feeding warm + verify; `[verify].commands` is gone (legacy-rejected); the `≥1 command` invariant lives on the profile. Byte-for-byte for Rust. **The `[[languages]]` schema is now real** — Step 2b's `go` profile is a config addition onto it.

**Final review before Step 2b:** run the bridge's own gpt-5.5-high review on the Step-2a diff (config-parse soundness + the byte-for-byte rust migration + the legacy-reject). Then write the **Step 2b** plan.

## Step 2b (SEPARATE follow-on plan — NOT in scope here)

From spec §1/§3/§6/§8: the combined Rust+Go toolchain image (Containerfile: add `go` + `gopls`); add `lsp-mcp` as a path dep + typed `detect_repo_langs`/`LangDetection`; replace Task 3's hardcoded `id=="rust"` `pick_profile` with **detection** (select by detected language; `Unsupported`/`None`/`Ambiguous` → preflight per §2.1); `implement --lang <auto|id|none>` + the preflight (hard-fail-with-options / `none` → bare, verify SKIPPED); **the lsp-env move** (the impl agent's `CARGO_HOME`/`CARGO_NET_OFFLINE` MCP env → the rust profile's `lsp_env`; the go profile's `lsp_env` = `GOMODCACHE`/`GOFLAGS`; the impl-lsp setup applies the selected profile's `lsp_env` + flips `--lang auto`); the `go` profile in the example configs; the Go `implement` live gate (incl. third-party gopls nav) + the byte-for-byte Rust regression.

---

## Self-review notes

**Spec coverage (2a scope):** §2 schema (`[[languages]]` parse + `to_profile`) — Tasks 1–2; §2.1 no-backward-compat (legacy `[verify].commands` reject + invariant move) — Task 2; profile-drive warm+verify — Task 3; config migration — Task 4. **Deferred to 2b (explicit):** detection, `--lang`, image, the lsp-env move, the go profile, the live gate.

**Placeholder scan:** `pick_profile` (Task 3) is concretely "the `language_profiles()` entry with `id == \"rust\"`" — a named, real selection (Step 2b swaps it for detection). `LanguageProfile::from_parts` is a named new constructor (Task 2 says add it in `profile.rs`). No vague gaps.

**Type consistency:** `VerifyCommand { name, cmd, gate }` defined in `profile.rs` (Task 1), consumed by `to_profile` (Task 2) + `run_verify` (Task 3). `LanguageProfile::from_parts(...)` signature (Task 2) matches the fields added in Task 1. `RegistryConfig.languages` + `language_profiles()` (Task 2) consumed by `pick_profile` (Task 3).
