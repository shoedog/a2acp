# LSP-MCP C2a Step 2a — config-driven profiles (Rust byte-for-byte) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans. Steps use checkbox (`- [ ]`) syntax.

**Goal:** Make the `cache_binding` seam **config-driven** — parse `[[languages]]` profiles from the config and select one (Rust, for now), instead of the hardcoded `rust_profile()`. The warm command + caches + verify commands + container image all come from the parsed profile. **Zero behavior change** (the migrated `rust` profile reproduces today's values byte-for-byte). This crystallizes the `[[languages]]` schema that Step 2b (Go) drops onto.

**Architecture:** Step 1 built `LanguageProfile` + `cache_binding` (bridge-core) with a hardcoded `rust_profile()`. Step 2a (1) extends `LanguageProfile` with `verify_commands` + an optional `image` + a `from_parts` constructor; (2) adds a `LanguageToml` config struct + `[[languages]]` parsing in `RegistryConfig`, with `to_profile()`, and makes **`VerifyConfig` infra-only** (`runtime`/`image`/`cache`/`egress`) — verify *commands* now live on the profile; (3) **removes `[verify].commands`** (legacy → explicit parse error) and **moves the "≥1 verify command" invariant** to the matched profile; (4) routes **all three** live `rust_profile()` sites — `warm_lsp_deps_step` (Fetch), `build_warm_impl`'s Lsp mount, and `run_verify` (Verify) — through the **config-selected** profile, including the `image` override; (5) migrates the example configs verbatim. **Deferred to Step 2b (NOT here):** language detection, `--lang`, the combined Go image, the **lsp runtime env move** (the impl agent's MCP `CARGO_HOME`/`CARGO_NET_OFFLINE` env STAYS in config for 2a — the Lsp binding stays mount-only as in Step 1, `lsp_env` empty), the separate `lsp_cache_mount` schema field (2a reuses `dep_cache_path` for the Lsp mount — byte-for-byte, both `/cargo` for Rust), and the `go` profile + live gate.

**Tech Stack:** Rust (crates/bridge-core, bin/a2a-bridge), TOML config.

**Handoff context:**
- **Branch:** `feat/lsp-mcp-c2a-step2` (off `main`, which has C2a Step 1 + the C2 spec). `git checkout feat/lsp-mcp-c2a-step2` first.
- **Spec:** `docs/superpowers/specs/2026-06-15-lsp-mcp-slice-c2-design.md` §2 (the `[[languages]]` schema), §2.1 (no-backward-compat + legacy-reject + the invariant move), §1 (per-language atoms). This plan is **Step 2a of C2a Step 2**; Step 2b (Go) is the follow-on plan.
- **Byte-for-byte invariant:** the migrated `rust` profile must reproduce today's `compose_warm_fetch`/`compose_verify`/Lsp-mount exactly. The Step-1 byte-for-byte tests (`compose_verify_via_binding_is_byte_for_byte`, `compose_warm_fetch_via_binding_is_byte_for_byte`, and the warm/verify argv tests in `verify.rs`/`implement.rs`) must STAY GREEN; the verify commands + image are now sourced from the profile, so a new test pins the rust profile's `verify_commands` to today's `[verify].commands`.
- **Today's rust values (the migration target):** `fetch = "cargo fetch --locked"`, `warm_cache = "a2a-impl-lsp-cache"`, fetch_env `CARGO_HOME=/cargo`, verify_env `CARGO_HOME=/cache/cargo`+`CARGO_TARGET_DIR=/cache/target`, dep_cache `/cargo`, verify_cache `/cache`; image = `None` (rust uses `[verify].image`); verify_commands = the 4 in `examples/a2a-bridge.containerized.toml` `[[verify.commands]]` (fmt/clippy/build/test, the test cmd with `--exclude bridge-container` + the 3 `--skip process::tests::…`). Confirm against the real config when migrating.
- **CONFIRMED call-site facts (from the plan review, ground them again before editing):** the three LIVE `rust_profile()` sites are `bin/a2a-bridge/src/main.rs:892` (Fetch, in `warm_lsp_deps_step`), `bin/a2a-bridge/src/main.rs:1316` (Lsp mount, in `build_warm_impl`), and `bin/a2a-bridge/src/verify.rs:146` (Verify, in `run_verify`). The two `rust_profile()` calls in `bin/a2a-bridge/src/implement.rs:522`/`:561` are INSIDE `#[cfg(test)]` (byte-for-byte pins) — **leave them alone.** `verify_cfg` is parsed pre-`into_snapshot()` at `main.rs:1608` (fresh) + `main.rs:1902` (resume); `cfg` is moved at `1620`/`1906`; `RegistrySnapshot` does NOT carry languages. The verify choke point is `run_verify_step(verify_cfg, clone_cwd, repo)` at `main.rs:820`, called via `ProdEffects::verify` (`main.rs:1222`); the `vcfg.commands.len()` log is at `main.rs:836`.

---

## File structure

| File | Create/Modify | Responsibility |
| --- | --- | --- |
| `crates/bridge-core/src/profile.rs` | Modify | `LanguageProfile` gains `verify_commands: Vec<VerifyCommand>` + `image: Option<String>`; add a `pub struct VerifyCommand { name, cmd, gate }`; add a `pub fn from_parts(...)` constructor (the private fields `dep_cache_path`/`verify_cache_path`/`*_env` force the constructor to live here, used by config's `to_profile`); `rust_profile()` keeps populating the existing fields + the new ones (today's 4 verify commands, `image: None`) so its tests + the Step-1 byte-for-byte tests stay green. |
| `bin/a2a-bridge/src/config.rs` | Modify | `LanguageToml` + `LanguageVerifyToml`; `RegistryConfig.languages: Vec<LanguageToml>`; `LanguageToml::to_profile() -> LanguageProfile` (the "≥1 verify command" invariant lives here). **`VerifyConfig` becomes infra-only** (`runtime`/`image`/`cache`/`egress`): DELETE `VerifyConfig.commands`, DELETE `config::VerifyCommand`, DELETE the `commands.is_empty()` check + the command-mapping in `to_config`. `VerifyToml.commands` → `Option<Vec<VerifyCommandToml>>` (a legacy probe whose PRESENCE errors in `parse`). Migrate/replace the embedded config tests that build `[[verify.commands]]` or assert `.commands`. |
| `bin/a2a-bridge/src/main.rs` | Modify | Parse the profile pre-move (`pick_rust_profile(&cfg)?`) alongside `verify_cfg` in BOTH the fresh + resume paths; thread `&LanguageProfile` into `warm_lsp_deps_step` (Fetch site), `build_warm_impl` (Lsp-mount site), and `run_verify_step` + `ProdEffects` (Verify site); update the `vcfg.commands.len()` log → `profile.verify_commands.len()`. |
| `bin/a2a-bridge/src/verify.rs` | Modify | `run_verify` takes the selected `&LanguageProfile`: iterates `profile.verify_commands`, builds the Verify `CacheBinding` from `profile` (not `rust_profile()`), and resolves the image via `profile.image.as_deref().unwrap_or(&cfg.image)`. |
| `examples/*.toml` (configs with `[[verify.commands]]`) | Modify | Migrate `[[verify.commands]]` → a `[[languages]] id="rust"` profile (warm/verify/env/cache/commands), **verbatim per config** (NOT a canonical 4 — `slicing-implement.toml` has no clippy); remove `[verify].commands`. Tracked: `containerized.toml`, `containerized.podman.toml`, `slicing-implement.toml`. Untracked-but-must-migrate-on-disk (or they fail to parse): `containerized.sonnet.toml` (the dogfood loop loads it) + the `slicing-implement-*` variants. |

---

### Task 1: Extend `LanguageProfile` with `verify_commands` + `image` + `from_parts`

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

- [ ] **Step 3: Add the fields + a `VerifyCommand` + `from_parts`** to `profile.rs`:

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
Add a public constructor (the private fields mean config.rs can't build the struct directly). Match the EXACT field set of `LanguageProfile` as it stands after adding the two fields above — read the struct and order the params to its fields:
```rust
impl LanguageProfile {
    /// Construct from config-parsed parts. `dep_cache_path`/`verify_cache_path`/`*_env` are private,
    /// so this is the only way the bin layer builds a profile.
    #[allow(clippy::too_many_arguments)]
    pub fn from_parts(
        id: String,
        fetch_cmd: String,
        warm_cache_base: String,
        dep_cache_path: String,
        verify_cache_path: String,
        fetch_env: Vec<(String, String)>,
        lsp_env: Vec<(String, String)>,
        verify_env: Vec<(String, String)>,
        image: Option<String>,
        verify_commands: Vec<VerifyCommand>,
    ) -> Self {
        Self {
            id,
            fetch_cmd,
            warm_cache_base,
            dep_cache_path,
            verify_cache_path,
            fetch_env,
            lsp_env,
            verify_env,
            image,
            verify_commands,
        }
    }
}
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
git commit -m "feat(bridge-core): LanguageProfile gains verify_commands + image + from_parts (C2a step 2a)"
```

---

### Task 2: `[[languages]]` parse + `to_profile` + legacy reject + `VerifyConfig` infra-only

**Files:** Modify `bin/a2a-bridge/src/config.rs`

- [ ] **Step 1: Write the failing tests** (append to `config.rs` `mod tests`). Note the test toml now sets `dep_cache_path`/`verify_cache_path` because they are REQUIRED fields (see Step 3).

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
dep_cache_path = "/cargo"
verify_cache_path = "/cache"
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
    assert_eq!(profs[0].image, None);
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
dep_cache_path = "/cargo"
verify_cache_path = "/cache"
"#;
    let c = RegistryConfig::parse(toml).unwrap();
    assert!(c.language_profiles().unwrap_err().to_string().contains("at least one"));
}
```

- [ ] **Step 2: Run → fail** — `cargo test -p a2a-bridge languages_parse_to_profile legacy_verify_commands_is_rejected profile_needs_at_least_one_verify_command` → FAIL (no `languages` field / `language_profiles` / `to_profile`).

- [ ] **Step 3: Add the config types + parsing + make `VerifyConfig` infra-only.** In `config.rs`:

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
    // REQUIRED (no default): every profile must state its cache paths explicitly. This pre-empts the
    // 2b footgun where a `go` profile would silently inherit Rust's `/cargo`/`/cache`.
    pub dep_cache_path: String,
    pub verify_cache_path: String,
    pub lsp_env: Option<std::collections::BTreeMap<String, String>>,
    pub verify_env: Option<std::collections::BTreeMap<String, String>>,
    pub image: Option<String>,
    #[serde(default)]
    pub verify: Vec<LanguageVerifyToml>,
}
```
Add to `RegistryConfig`: `#[serde(default)] pub languages: Vec<LanguageToml>,`.

**Make `VerifyConfig` infra-only.** Today (config.rs:396-409) `VerifyConfig` carries `commands: Vec<VerifyCommand>` and there is a `config::VerifyCommand` struct. Verify commands now live on the profile, so:
- DELETE the `pub commands: Vec<VerifyCommand>` field from `VerifyConfig` (it becomes `{ runtime, image, cache, egress }`).
- DELETE the `pub struct VerifyCommand { name, cmd, gate }` (config.rs:404-409) — it has no remaining consumer.
- In `VerifyToml::to_config` (config.rs:412): DELETE the `if self.commands.is_empty() { return Err(...) }` check AND the `commands: self.commands.iter().map(...).collect()` mapping. The body becomes just: parse egress, return `Ok(VerifyConfig { runtime, image, cache, egress })`.
- Change `VerifyToml.commands` (config.rs:391) from `#[serde(default)] pub commands: Vec<VerifyCommandToml>` to a presence-detecting legacy probe (KEEP `VerifyCommandToml` — it's the probe's element type):
```rust
    /// REMOVED — commands moved to `[[languages.verify]]`. Kept ONLY to reject legacy configs loudly
    /// (VerifyToml has no `deny_unknown_fields`, so a silent drop would otherwise mask stale configs).
    /// `Option` so PRESENCE errors — even `commands = []` is a stale config we should reject.
    pub commands: Option<Vec<VerifyCommandToml>>,
```
and in `RegistryConfig::parse` (after deserialize, before returning `Ok`), add:
```rust
    if let Some(v) = &cfg.verify {
        if v.commands.is_some() {
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
            self.dep_cache_path.clone(),
            self.verify_cache_path.clone(),
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

- [ ] **Step 4: Migrate the embedded tests that break, then run → PASS.** Removing `VerifyConfig.commands` + the empty-check breaks several in-file tests. Find them: `grep -n 'VerifyConfig\|\.commands\|VerifyCommand\|\[\[verify.commands\]\]' bin/a2a-bridge/src/config.rs bin/a2a-bridge/src/main.rs`. Known loci to fix:
  - `config.rs:~1591 verify_config_parses_structured_commands_and_locked_egress` — builds `[[verify.commands]]` and asserts `v.commands.len()==2`. **Rewrite** to a `[[languages]]` config asserting `c.language_profiles().unwrap()[0].verify_commands` (and keep asserting the locked-egress parse on `to_config()`).
  - `config.rs:~1655 verify_config_rejects_empty_commands` — tested the now-deleted `to_config` empty-check. **Delete it** (superseded by `profile_needs_at_least_one_verify_command` + `legacy_verify_commands_is_rejected`).
  - `main.rs:~836` — the `vcfg.commands.len()` log (handled in Task 3, but it will fail to COMPILE here once `VerifyConfig.commands` is gone; Task 3 lands the real fix — if you split commits, temporarily switch it to `0` or land Task 3 in the same change).
  - `main.rs:~1201` + `main.rs:~4038` and any other test constructing `VerifyConfig { commands: ... }` or a `[[verify.commands]]` toml — update to the infra-only struct / `[[languages]]` form.

  Then `cargo test -p a2a-bridge` (the 3 new config tests + the whole suite) → PASS.

- [ ] **Step 5: Commit** — clippy + fmt, then:
```bash
git add bin/a2a-bridge/src/config.rs crates/bridge-core/src/profile.rs
git commit -m "feat(config): [[languages]] profiles + infra-only VerifyConfig + legacy [verify].commands reject (C2a step 2a)"
```

---

### Task 3: Route all three sites through the CONFIG-selected profile

**Files:** Modify `bin/a2a-bridge/src/main.rs` (`pick_rust_profile`, `warm_lsp_deps_step`, `build_warm_impl`, `run_verify_step`, `ProdEffects`, the fresh + resume entry paths), `bin/a2a-bridge/src/verify.rs` (`run_verify`)

- [ ] **Step 1: Write the failing tests** in `verify.rs`:
  - A `run_verify` test that drives the profile's `verify_commands` (inject a fake runner; assert it runs the profile's commands in order, gating on the first failure) — pass a profile built from `rust_profile()`.
  - An **image-override** test: a profile with `image: Some("override:img")` makes `run_verify` use `override:img`, while `image: None` uses `cfg.image` (byte-for-byte today). (Shape mirrors the existing `run_verify` tests at `verify.rs:346-402`.)

- [ ] **Step 2: Run → fail** (signature mismatch — `run_verify` still reads `cfg.commands` + `rust_profile()`).

- [ ] **Step 3: Implement.**
  - **`pick_rust_profile`** (new fn in `main.rs`): `fn pick_rust_profile(cfg: &config::RegistryConfig) -> Result<bridge_core::profile::LanguageProfile, config::ConfigError>` = `cfg.language_profiles()?` then `.into_iter().find(|p| p.id == "rust").ok_or_else(|| ConfigError::Registry("no [[languages]] id=\"rust\" profile".into()))`. (Step 2b replaces the `id=="rust"` filter with detection.)
  - **Parse the profile pre-move** in BOTH the fresh path (next to `let verify_cfg = ...` at `main.rs:1608`) and the resume path (`main.rs:1902`), BEFORE `into_snapshot()` moves `cfg`: `let profile = pick_rust_profile(&cfg)?;`. `RegistrySnapshot` carries no languages, so this MUST happen pre-move (exactly like `verify_cfg`). Thread `&profile` (owned, lives across the move) down all three sites.
  - **`run_verify`** (`verify.rs:138`): take `profile: &bridge_core::profile::LanguageProfile` (in addition to `cfg`). Iterate `profile.verify_commands` (not `cfg.commands`); build the Verify binding from `profile.cache_binding(CacheCtx::Verify, "", cache_vol)` (not `rust_profile()`); resolve the image as `let image = profile.image.as_deref().unwrap_or(&cfg.image)` and use it where `&cfg.image` was used (verify.rs:154).
  - **`run_verify_step`** (`main.rs:820`) + **`ProdEffects`** (`main.rs:1197`, field at 1198, call at 1222): add a `profile: &LanguageProfile` param/field and pass it into `run_verify`. Update the `vcfg.commands.len()` log at `main.rs:836` → `profile.verify_commands.len()`.
  - **`warm_lsp_deps_step`** (`main.rs:859`, `rust_profile()` at 892): take `profile: &LanguageProfile`, use its `fetch_cmd` + `warm_cache_base` + `cache_binding(Fetch, …)` + the image override (`profile.image.as_deref().unwrap_or(&vcfg.image)`) instead of `rust_profile()`/`vcfg.image`. Thread `&profile` from both call sites (`main.rs:1665`, `main.rs:1943`).
  - **`build_warm_impl`** (`main.rs:1284`, Lsp mount at 1316): take `profile: &LanguageProfile`, replace `rust_profile().cache_binding(Lsp, cache, "")` with `profile.cache_binding(CacheCtx::Lsp, cache, "")`. Byte-for-byte for 2a (`lsp_env` empty, `dep_cache_path == "/cargo"`). Thread `&profile` from both call sites (`main.rs:1681`, `main.rs:1955`).
  - **Do NOT touch** `implement.rs:522`/`:561` — they're `#[cfg(test)]` byte-for-byte pins that legitimately use `rust_profile()`.
  - **Byte-for-byte:** with the migrated `rust` config profile (Task 4), the warm argv + verify scripts/commands + Lsp mount are identical to today; the Step-1 byte-for-byte tests + Task 1's verify-commands test + the new image-override test pin it.

- [ ] **Step 4: Run → PASS** — `cargo test -p a2a-bridge -p bridge-core`.

- [ ] **Step 5: Commit** — clippy + fmt, then:
```bash
git add bin/a2a-bridge/src/main.rs bin/a2a-bridge/src/verify.rs
git commit -m "refactor(implement): warm + Lsp-mount + verify use the config-selected LanguageProfile (C2a step 2a)"
```

---

### Task 4: Migrate the example configs (`[verify].commands` → `[[languages]]` rust profile)

**Files:** Modify every config with `[[verify.commands]]`. **Tracked (commit these):** `examples/a2a-bridge.containerized.toml`, `examples/a2a-bridge.containerized.podman.toml`, `examples/a2a-bridge.slicing-implement.toml`. **Untracked but MUST migrate on-disk (else they fail to parse the moment they're loaded):** `examples/a2a-bridge.containerized.sonnet.toml` (the dogfood loop loads it — Step 3 below depends on it), `examples/a2a-bridge.slicing-implement-fast.toml`, `…-s2xhigh.toml`, `…-s3.toml`, `…-sonnet.toml`. Re-derive the full list: `grep -rl '\[\[verify.commands\]\]' examples/`.

- [ ] **Step 1: Migrate each config VERBATIM.** For each config: keep `[verify]` infra (`image`/`cache`/`egress`/`network`/`proxy`/`no_proxy`/`runtime`) exactly as-is; DELETE the `[[verify.commands]]` blocks; ADD a `[[languages]] id="rust"` profile carrying `fetch = "cargo fetch --locked"`, `warm_cache = "a2a-impl-lsp-cache"`, `dep_cache_path = "/cargo"`, `verify_cache_path = "/cache"`, `fetch_env = { CARGO_HOME = "/cargo" }`, `verify_env = { CARGO_HOME = "/cache/cargo", CARGO_TARGET_DIR = "/cache/target" }`, and the `[[languages.verify]]` commands **copied byte-for-byte from THAT FILE's old `[[verify.commands]]`** — do NOT impose a canonical set. **Verified difference:** `containerized.toml`/`.podman.toml`/`.sonnet.toml` have fmt/clippy/build/test; `slicing-implement.toml` (and likely the slicing variants) have **fmt/build/test only — NO clippy**. Adding clippy would silently change behavior. (Leave `lsp_env` unset in 2a — the impl agent's MCP `CARGO_HOME`/`CARGO_NET_OFFLINE` env STAYS as-is; the lsp-env move is Step 2b.)

- [ ] **Step 2: Verify each config parses + the profile round-trips** — add a `config::tests` parse assertion that loads each TRACKED config file (read it from `examples/…`) and asserts `language_profiles()` yields the rust profile with that file's exact command set. Run `cargo test -p a2a-bridge` → PASS.

- [ ] **Step 3: DoD — the dogfood still works byte-for-byte.** Run an `implement` on a small Rust task with the migrated `containerized.sonnet.toml` (the loop currently used) and confirm warm + verify behave exactly as before (warm fetch runs, verify PASS with the same fmt/clippy/build/test). This is the live byte-for-byte gate for the config migration.

- [ ] **Step 4: Commit** (tracked configs + the round-trip test only; the untracked configs were migrated on-disk for correctness but stay untracked):
```bash
git add examples/a2a-bridge.containerized.toml examples/a2a-bridge.containerized.podman.toml examples/a2a-bridge.slicing-implement.toml bin/a2a-bridge/src/config.rs
git commit -m "config(implement): migrate [verify].commands -> [[languages]] rust profile, verbatim (C2a step 2a)"
```

---

## Step 2a done — what ships

The seam is now CONFIG-DRIVEN end-to-end: `[[languages]]` profiles are parsed + selected (Rust), feeding **all three** sites (warm fetch, the Lsp mount, verify) plus the image override; `[verify].commands` is gone (legacy-rejected) and `VerifyConfig` is infra-only; the `≥1 command` invariant lives on the profile. Byte-for-byte for Rust. **The `[[languages]]` schema is now real** — Step 2b's `go` profile is a config addition onto it.

**Final review before Step 2b:** run the bridge's own gpt-5.5-high review on the Step-2a diff (config-parse soundness + the byte-for-byte rust migration + the legacy-reject + the infra-only `VerifyConfig`). Then write the **Step 2b** plan.

## Step 2b (SEPARATE follow-on plan — NOT in scope here)

From spec §1/§3/§6/§8: the combined Rust+Go toolchain image (Containerfile: add `go` + `gopls`); add `lsp-mcp` as a path dep + typed `detect_repo_langs`/`LangDetection`; replace Task 3's `pick_rust_profile` (`id=="rust"`) with **detection** (select by detected language; `Unsupported`/`None`/`Ambiguous` → preflight per §2.1); `implement --lang <auto|id|none>` + the preflight (hard-fail-with-options / `none` → bare, verify SKIPPED); **the lsp-env move** (the impl agent's `CARGO_HOME`/`CARGO_NET_OFFLINE` MCP env → the rust profile's `lsp_env`; the go profile's `lsp_env` = `GOMODCACHE`/`GOFLAGS`; the impl-lsp setup applies the selected profile's `lsp_env` + flips `--lang auto`); the separate `lsp_cache_mount` schema field if the Lsp mount must differ from `dep_cache_path` (it doesn't for Rust); the `go` profile in the example configs; the Go `implement` live gate (incl. third-party gopls nav) + the byte-for-byte Rust regression.

---

## Self-review notes

**Spec coverage (2a scope):** §2 schema (`[[languages]]` parse + `to_profile`) — Tasks 1–2; §2.1 no-backward-compat (legacy `[verify].commands` reject + `VerifyConfig` infra-only + invariant move) — Task 2; profile-drive all three sites + image override — Task 3; config migration (verbatim) — Task 4. **Deferred to 2b (explicit):** detection, `--lang`, the Go image, the lsp-env move, `lsp_cache_mount`, the go profile, the live gate.

**Plan-review fixes folded in (codex gpt-5.5-high + opus, both fix-then-ship):** B1 — `VerifyConfig` made infra-only so migrated configs don't dead-end at `to_config`'s empty-check → `ConfigError` → silent verify-skip (Task 2). Profile threading made concrete: parsed pre-`into_snapshot()` in both fresh + resume, threaded through `warm_lsp_deps_step`/`build_warm_impl`/`run_verify_step`/`ProdEffects`, log updated (Task 3). `image` override now actually consumed at warm + verify with a live test (Task 3). The THIRD `rust_profile()` site (`main.rs:1316`, Lsp mount) is converted, not left behind (Task 3). Migration is verbatim-per-config (slicing has no clippy), covers all tracked + the dogfood `sonnet` config, and `git add` includes `slicing-implement.toml` (Task 4). Legacy reject keys on PRESENCE (`Option`, rejects `commands = []`). `dep_cache_path`/`verify_cache_path` are REQUIRED (no Rust-shaped default) to pre-empt the 2b `go`-profile footgun.

**Placeholder scan:** `pick_rust_profile` (Task 3) is concretely "the `language_profiles()` entry with `id == \"rust\"`, else a parse error" — a named, real selection (Step 2b swaps it for detection). `LanguageProfile::from_parts` is a named new constructor (Task 1, field-ordered to the struct). No vague gaps.

**Type consistency:** `VerifyCommand { name, cmd, gate }` defined in `profile.rs` (Task 1), consumed by `to_profile` (Task 2) + `run_verify` (Task 3). `LanguageProfile::from_parts(...)` signature (Task 1) matches the params `to_profile` passes (Task 2). `RegistryConfig.languages` + `language_profiles()` + `pick_rust_profile` (Tasks 2–3). `VerifyConfig` is `{runtime, image, cache, egress}` everywhere after Task 2 (no `.commands` reader survives).
