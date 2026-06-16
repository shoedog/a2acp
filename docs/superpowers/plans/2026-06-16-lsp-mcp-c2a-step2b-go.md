# LSP-MCP C2a Step 2b — Go in-container implementor Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans. Steps use checkbox (`- [ ]`) syntax.

**Goal:** Make the containerized `implement` flow support **Go** end-to-end: a combined Rust+Go toolchain image, a `go` `[[languages]]` profile (warm `go mod download` + `go` verify), language **detection** (reusing `lsp_mcp::lang::detect_lang`) replacing the hardcoded `id=="rust"` selection, an `implement --lang <auto|id|none>` flag + preflight, and the **lsp-env move** so the in-container lsp-mcp gets the *selected* profile's env (cargo for Rust, go for Go). Rust stays byte-for-byte.

**Architecture:** C2a Step 2a (merged `5644414`) made the warm/verify seam config-driven via `[[languages]]` profiles, selected by `pick_rust_profile` (`id=="rust"`). Step 2b (1) adds `go`+`gopls` to the toolchain image; (2) adds a `go` profile to the configs + a rust `lsp_env`; (3) makes the bridge depend on `lsp-mcp` and adds a typed `LangDetection` over `detect_lang`; (4) replaces `pick_rust_profile` with detection + an `implement --lang` flag + a preflight (`none` → bare, verify SKIPPED; ambiguous/unsupported → hard-fail with options); (5) performs the **lsp-env move** — the impl agent's in-container lsp-mcp env comes from the selected `profile.lsp_env`, not config-static `CARGO_*`. Then a live Go gate + a Rust byte-for-byte regression.

**Tech Stack:** Rust (crates/lsp-mcp, crates/bridge-core, crates/bridge-container, bin/a2a-bridge), TOML config, a Docker toolchain image (Go + gopls).

**Handoff context:**
- **Branch:** `feat/lsp-mcp-c2a-step2b` (off `main` = `5644414`, which has C2a Step 1 + 2a + the lsp-mcp Go *nav* already shipped). `git checkout feat/lsp-mcp-c2a-step2b` first.
- **Spec:** `docs/superpowers/specs/2026-06-15-lsp-mcp-slice-c2-design.md` §1 (per-language atoms + detection), §2/§2.1 (`[[languages]]` schema, `--lang none`), §3/§6/§8 (image, go profile, gate). **Step 2a plan** (`…-c2a-step2a-config-driven.md`) "Step 2b" section is the source list.
- **What 2a already built (USE it, don't rebuild):** `bridge_core::profile::{LanguageProfile, VerifyCommand, CacheCtx, cache_binding, from_parts}`; `config::{LanguageToml, LanguageVerifyToml, language_profiles, to_profile}` (REQUIRED `dep_cache_path`/`verify_cache_path`; optional `lsp_env`/`verify_env`/`fetch_env`/`image`); infra-only `VerifyConfig`; `pick_rust_profile(&cfg)` (main.rs, called pre-`into_snapshot()` in BOTH fresh ~1623 + resume ~1920 paths) threaded into `warm_lsp_deps_step`/`build_warm_impl`/`run_verify`. The lsp-mcp `Lang{Rust,Python,Go}` + `detect_lang(repo)->anyhow::Result<Lang>` (ambiguous→Err, none→Err, single→Ok) already exists in `crates/lsp-mcp/src/lang.rs`.
- **GROUND TRUTH:** read each cited file/line before editing — 2a moved lines. The impl agent's MCP env flows `[[agents.mcp]]`→`config::McpToml.env`→`bridge_core::mcp::McpServerSpec.env`, applied at `crates/bridge-acp/src/acp_backend.rs:406` (`McpServerStdio::env`) and (for codex native) rendered to `-c mcp_servers.*` in `crates/bridge-container/src/lib.rs:224`. The impl agent + its MCP specs are assembled in `build_warm_impl` (main.rs ~1284), where `&profile` is already in scope.

---

## File structure

| File | Create/Modify | Responsibility |
| --- | --- | --- |
| `deploy/containers/toolchain.Containerfile` | Modify | Add Go (pinned) + `gopls` to the final image so the impl agent + verify + in-container gopls work. |
| `bin/a2a-bridge/Cargo.toml` | Modify | Add `lsp-mcp = { path = "../../crates/lsp-mcp" }` (reuse `detect_lang`). |
| `bin/a2a-bridge/src/detect.rs` | Create | `LangDetection` typed wrapper over `lsp_mcp::lang::detect_lang` (`Detected(Lang)` / `None` / `Ambiguous` with the marker detail). |
| `bin/a2a-bridge/src/main.rs` | Modify | `--lang <auto\|rust\|go\|none>` in `parse_implement_args`; replace `pick_rust_profile` with `select_profile(&cfg, lang, repo)` (detection/explicit/none + preflight); the **lsp-env move** in `build_warm_impl`. |
| `bin/a2a-bridge/src/implement.rs` (or wherever `ImplementArgs` lives) | Modify | `ImplementArgs.lang: LangArg` field (default `Auto`). |
| `examples/a2a-bridge.containerized.toml` (+ `.podman.toml`, `.sonnet.toml` on-disk) | Modify | Add a `go` `[[languages]]` profile; add `lsp_env` to the `rust` profile; REMOVE `CARGO_HOME`/`CARGO_NET_OFFLINE` from the impl agent's `[[agents.mcp]]` lsp `env` (they move to `rust.lsp_env`). |

---

### Task 1: Add Go + gopls to the toolchain image

**Files:** Modify `deploy/containers/toolchain.Containerfile`

- [ ] **Step 1:** After the Rust/rust-analyzer layers in the FINAL image stage (after the `rustup component add rust-analyzer rust-src` line), add a Go layer (its own layer so the Rust layers stay cached):

```dockerfile
# C2a Step 2b: Go toolchain + gopls so the impl agent can edit/build/test Go, the bridge can run a
# deterministic Go verify, and gopls runs in-container for live nav. Pinned for reproducibility.
ENV GO_VERSION=1.23.4
RUN curl --proto '=https' --tlsv1.2 -sSfL "https://go.dev/dl/go${GO_VERSION}.linux-$(dpkg --print-architecture).tar.gz" \
      -o /tmp/go.tgz \
 && tar -C /usr/local -xzf /tmp/go.tgz && rm /tmp/go.tgz
ENV PATH=/usr/local/go/bin:/root/go/bin:$PATH GOTOOLCHAIN=local
# gopls pinned (built against the pinned Go); GOFLAGS unset here so `go install` can fetch it.
RUN go install golang.org/x/tools/gopls@v0.17.1
```

- [ ] **Step 2: Build it.** Run (from repo root): `docker build -t a2a-toolchain:latest -f deploy/containers/toolchain.Containerfile .` Expected: build succeeds; the Go layer is added after the cached Rust layers.

- [ ] **Step 3: Smoke the tools in-image.** Run: `docker run --rm --entrypoint bash a2a-toolchain:latest -lc 'go version && gopls version && rustc --version && lsp-mcp --help >/dev/null && echo OK'` Expected: prints `go version go1.23.4 …`, a gopls version, the rustc version, and `OK`.

- [ ] **Step 4: Commit:**
```bash
git add deploy/containers/toolchain.Containerfile
git commit -m "build(toolchain): add Go 1.23.4 + gopls to the combined image (C2a step 2b)"
```

---

### Task 2: Add the `go` profile + the rust `lsp_env` to the configs (lsp-env move, config side)

**Files:** Modify `examples/a2a-bridge.containerized.toml`, `examples/a2a-bridge.containerized.podman.toml` (tracked); also migrate `examples/a2a-bridge.containerized.sonnet.toml` on-disk (untracked, used by the dogfood).

- [ ] **Step 1:** In each config, ADD `lsp_env` to the existing `rust` `[[languages]]` profile (the cargo env the in-container lsp-mcp needs, MOVED from the impl agent's mcp block):
```toml
lsp_env = { CARGO_HOME = "/cargo", CARGO_NET_OFFLINE = "true" }
```

- [ ] **Step 2:** ADD a `go` profile after the rust one:
```toml
[[languages]]
id = "go"
fetch = "go mod download"
warm_cache = "a2a-impl-lsp-cache-go"
dep_cache_path = "/go/pkg/mod"
verify_cache_path = "/cache"
fetch_env = { GOMODCACHE = "/go/pkg/mod", GOFLAGS = "-mod=mod" }
verify_env = { GOMODCACHE = "/go/pkg/mod", GOCACHE = "/cache/go-build", GOFLAGS = "-mod=mod" }
lsp_env = { GOMODCACHE = "/go/pkg/mod", GOFLAGS = "-mod=mod" }
[[languages.verify]]
name = "gofmt"
cmd  = "test -z \"$(gofmt -l .)\""
[[languages.verify]]
name = "vet"
cmd  = "go vet ./..."
[[languages.verify]]
name = "build"
cmd  = "go build ./..."
[[languages.verify]]
name = "test"
cmd  = "go test ./..."
```

- [ ] **Step 3 (the lsp-env move, config side):** In each config's impl agent `[[agents.mcp]]` block named `lsp`, DELETE the `[[agents.mcp.env]]` entries for `CARGO_HOME` and `CARGO_NET_OFFLINE` (they now come from the profile's `lsp_env`; Task 5 injects them). KEEP any non-language env (e.g. `LSP_MCP_LOG`). The lsp MCP `--lang` arg stays `auto` (it detects per-repo independently).

- [ ] **Step 4: Verify each config still parses + both profiles round-trip** — `cargo test -p a2a-bridge tracked_example_language_verify_commands_round_trip` plus a NEW assertion that `language_profiles()` now yields BOTH `rust` and `go` for the containerized config, with the go profile's 4 command names `["gofmt","vet","build","test"]`. (Extend the Task-2a round-trip test.)

- [ ] **Step 5: Commit:**
```bash
git add examples/a2a-bridge.containerized.toml examples/a2a-bridge.containerized.podman.toml bin/a2a-bridge/src/config.rs
git commit -m "config(implement): add go profile + rust lsp_env; move CARGO_* off the impl lsp mcp (C2a step 2b)"
```

---

### Task 3: Bridge depends on lsp-mcp + a typed `LangDetection`

**Files:** Modify `bin/a2a-bridge/Cargo.toml`; Create `bin/a2a-bridge/src/detect.rs`; register the module in `main.rs`.

- [ ] **Step 1: Write the failing test** (`detect.rs` `mod tests`): build temp dirs and assert detection:
```rust
#[test]
fn detect_classifies_rust_go_none_ambiguous() {
    let d = tempfile::tempdir().unwrap();
    // none
    assert!(matches!(LangDetection::detect(d.path()), LangDetection::None(_)));
    // rust
    std::fs::write(d.path().join("Cargo.toml"), "[package]\nname=\"x\"\n").unwrap();
    assert!(matches!(LangDetection::detect(d.path()), LangDetection::Detected(lsp_mcp::lang::Lang::Rust)));
    // ambiguous (add go.mod alongside Cargo.toml)
    std::fs::write(d.path().join("go.mod"), "module x\n").unwrap();
    assert!(matches!(LangDetection::detect(d.path()), LangDetection::Ambiguous(_)));
}
```

- [ ] **Step 2: Run → fail to compile** (`lsp-mcp` not a dep; `detect` absent).

- [ ] **Step 3: Add the dep** to `bin/a2a-bridge/Cargo.toml` `[dependencies]`: `lsp-mcp = { path = "../../crates/lsp-mcp" }` (+ `tempfile` under `[dev-dependencies]` if not present). Then create `detect.rs`:
```rust
//! Typed language detection for `implement`, wrapping lsp-mcp's `detect_lang` so the bridge can
//! distinguish detected / none / ambiguous and render a useful preflight message.
use lsp_mcp::lang::{detect_lang, Lang};
use std::path::Path;

pub enum LangDetection {
    Detected(Lang),
    None(String),      // human message (no markers)
    Ambiguous(String), // human message (multiple markers)
}

impl LangDetection {
    pub fn detect(repo: &Path) -> LangDetection {
        match detect_lang(repo) {
            Ok(l) => LangDetection::Detected(l),
            Err(e) => {
                let m = e.to_string();
                if m.contains("ambiguous") {
                    LangDetection::Ambiguous(m)
                } else {
                    LangDetection::None(m)
                }
            }
        }
    }
}
```
Add `mod detect;` to `main.rs`.

- [ ] **Step 4: Run → PASS** — `cargo test -p a2a-bridge detect_classifies`.

- [ ] **Step 5: Commit:**
```bash
git add bin/a2a-bridge/Cargo.toml bin/a2a-bridge/src/detect.rs bin/a2a-bridge/src/main.rs Cargo.lock
git commit -m "feat(implement): bridge depends on lsp-mcp + typed LangDetection (C2a step 2b)"
```

---

### Task 4: `--lang` flag + `select_profile` (detection/explicit/none + preflight)

**Files:** Modify `bin/a2a-bridge/src/main.rs` (`parse_implement_args`, `pick_rust_profile`→`select_profile`, the fresh + resume call sites) and `ImplementArgs`.

- [ ] **Step 1: Write the failing tests** (`main.rs` `mod tests`):
  - `parse_implement_args` accepts `--lang go` / `--lang none` / defaults to `Auto` when absent; rejects `--lang bogus`.
  - `select_profile`: with `LangArg::Explicit("go")` returns the config's go profile; with `LangArg::None` returns `None` (verify SKIPPED, no profile needed); with `LangArg::Auto` + a rust repo returns the rust profile; with `Auto` + ambiguous repo returns an `Err` whose message lists the candidate `--lang` options.

- [ ] **Step 2: Run → fail.**

- [ ] **Step 3: Implement.**
  - Add `LangArg { Auto, Explicit(String), None }` and `ImplementArgs.lang: LangArg` (default `Auto`). In `parse_implement_args` arg loop (main.rs ~712), handle `"--lang"`: `auto`→`Auto`, `none`→`None`, any other value `s`→`Explicit(s)` (validate later against profiles, so unknown ids fail with the available list).
  - Replace `pick_rust_profile` with:
    `fn select_profile(cfg: &config::RegistryConfig, lang: &LangArg, repo: &Path) -> Result<Option<bridge_core::profile::LanguageProfile>, BoxError>`:
    - `LangArg::None` → `Ok(None)` (bare run; verify SKIPPED — see Step 4).
    - `LangArg::Explicit(id)` → find profile by `id`; if absent → `Err` listing the configured profile ids.
    - `LangArg::Auto` → `LangDetection::detect(repo)`: `Detected(l)` → find profile `id==l.as_str()` (absent → `Err` "detected {l} but no [[languages]] id={l} profile; add one or pass --lang none"); `None(m)`/`Ambiguous(m)` → `Err` (the preflight: include `m` + "pass --lang <id|none>; configured: [rust, go]").
  - Both `implement` entry paths (fresh ~1623, resume ~1920): replace `let profile = pick_rust_profile(&cfg)?;` with `let profile = select_profile(&cfg, &args.lang, &repo)?;` BEFORE `into_snapshot()`. `profile` is now `Option<LanguageProfile>`. Thread the `Option` down: when `None`, skip warm (`warm_lsp_deps_step`) AND verify (verify becomes `VerifyOutcome::NotConfigured` / skipped) — the bare path. Document this in the handoff text the user sees.
  - **RESUME note:** a resumed run must reuse the SAME profile selection it started with — persist the resolved `lang`/profile-id in the checkpoint (CLONE/.git/a2a-bridge, alongside the existing attempt/sha — see implement_resume.rs) and re-select by id on resume rather than re-detecting (the clone's language doesn't change, but explicit `--lang`/`none` must survive a resume).

- [ ] **Step 4: Run → PASS** — `cargo test -p a2a-bridge -p bridge-core`.

- [ ] **Step 5: Commit:**
```bash
git add bin/a2a-bridge/src/main.rs bin/a2a-bridge/src/implement.rs bin/a2a-bridge/src/implement_resume.rs
git commit -m "feat(implement): --lang auto|id|none + detection-based select_profile + preflight (C2a step 2b)"
```

---

### Task 5: The lsp-env move (inject the selected profile's `lsp_env` into the impl agent's lsp MCP)

**Files:** Modify `bin/a2a-bridge/src/main.rs` (`build_warm_impl`), with a pure helper for the merge.

- [ ] **Step 1: Write the failing test** (pure helper, `main.rs` `mod tests`): `apply_lsp_env(specs: &mut [McpServerSpec], lsp_env: &[(String,String)])` sets/overrides the env on the spec whose `name == "lsp"` and leaves other specs (e.g. prism) untouched. Assert: a rust profile's `lsp_env` lands `CARGO_HOME=/cargo`+`CARGO_NET_OFFLINE=true` on the lsp spec; a go profile's lands `GOMODCACHE`/`GOFLAGS`; a non-lsp spec is unchanged; an existing same-key env value is overridden by the profile (profile wins).

- [ ] **Step 2: Run → fail.**

- [ ] **Step 3: Implement.** Add the pure helper `apply_lsp_env`, and call it in `build_warm_impl` (main.rs ~1284, `&profile` already threaded from Task 4 — now `&LanguageProfile`) right before the impl agent's `McpServerSpec`s are handed to the container backend (`cfg.mcp = …` in bridge-container, or the AcpBackend mint): merge `profile.lsp_env` onto the `lsp`-named spec. (Profile env OVERRIDES config env for matching keys so the selected language wins; this is what makes a Go repo's lsp-mcp get `GOMODCACHE` instead of `CARGO_HOME`.) When the profile is `None` (`--lang none`), do not inject (and there is no warm/verify; the lsp MCP keeps only its config env).

- [ ] **Step 4: Run → PASS** — `cargo test -p a2a-bridge`. Confirm the Step-2a byte-for-byte rust path is unchanged: with the rust profile, the lsp spec ends up with exactly `CARGO_HOME=/cargo`+`CARGO_NET_OFFLINE=true` (now from the profile, formerly from config) — same effective env.

- [ ] **Step 5: Commit:**
```bash
git add bin/a2a-bridge/src/main.rs
git commit -m "feat(implement): lsp-env move — inject selected profile.lsp_env into the impl lsp MCP (C2a step 2b)"
```

---

### Task 6: Live gate — Go implement + Rust byte-for-byte regression

**Files:** none (validation); a throwaway Go fixture repo.

- [ ] **Step 1: Rust regression (byte-for-byte).** Rebuild the binary (`cargo build --release --bin a2a-bridge`) and run a trivial Rust `implement` (`--lang auto`, or omit) against this repo with the migrated `containerized.toml`. Expected: warm + `verify: PASS (fmt ✓ · clippy ✓ · build ✓ · test ✓)` exactly as Step 2a — the lsp-env move + detection must NOT perturb Rust. Discard the hand-off.

- [ ] **Step 2: Go live gate.** Create a tiny throwaway Go module repo (under `allowed_cwd_root`) with a `go.mod`, a `main.go` (or a package with a function + a `_test.go`), commit it. Run `a2a-bridge implement "<trivial Go task, e.g. add a doc comment + a passing test>" --repo <gofixture> --lang auto --config examples/a2a-bridge.containerized.toml`. Expected: detection picks `go`; warm runs `go mod download`; the impl agent (codex) edits; `verify: PASS (gofmt ✓ · vet ✓ · build ✓ · test ✓)` via the go profile in the combined image; the in-container gopls (lsp) gets `GOMODCACHE` (the lsp-env move). Confirm the hand-off branch commits.

- [ ] **Step 3: Preflight checks.** Run `implement --lang none` on the Go fixture (expect: bare run, verify SKIPPED, still commits) and `--lang go` on a NO-go-mod dir + an AMBIGUOUS dir (rust+go) under `--lang auto` (expect: the preflight Err listing options, no container spawned). Reap all throwaway clones + containers.

- [ ] **Step 4:** No commit (validation only). Record the gate results in the wrap-up.

---

## Step 2b done — what ships

The containerized implementor is polyglot: detection (or explicit `--lang`) selects a `[[languages]]` profile (rust or go), the combined image carries both toolchains + gopls, warm + verify + the in-container lsp run with the selected profile's env (the lsp-env move), and `--lang none` is the eyes-open opt-out (verify skipped). Rust byte-for-byte. **This closes C2a.** Follow-ons: C2b, C2c (deferred).

**Final review before merge:** dual review (codex gpt-5.5-high + Opus 4.8) on the Step-2b diff (detection soundness, the preflight, the lsp-env move's rust byte-for-byte invariant, the go profile values, image reproducibility). Then finish-the-branch.

---

## Self-review notes

**Spec coverage:** §3 image (Task 1); §1/§2 go profile + schema reuse (Task 2); §1 detection (Tasks 3–4); §2.1 `--lang`/`none`/preflight (Task 4); §6 lsp-env move (Tasks 2+5); §8 live gate + Rust regression (Task 6). Task ordering: image → config → dep+detection → selection/flag → lsp-env move → gate (each builds on the prior; explicit `--lang go` works after Task 4, auto after detection in Task 4, env-correct after Task 5).

**Placeholder scan:** the go profile values (`/go/pkg/mod`, `GOCACHE=/cache/go-build`, `gofmt -l`, `go vet/build/test`) are concrete; the implementer must confirm the GOMODCACHE/GOCACHE mount paths against the image's `go env` (Task 1 Step 3 smoke). `select_profile`/`LangArg`/`LangDetection`/`apply_lsp_env` are named, with signatures. No "TBD".

**Type consistency:** `LangDetection` (detect.rs, Task 3) consumed by `select_profile` (Task 4). `select_profile` returns `Option<LanguageProfile>` (None = `--lang none`/bare) threaded through warm/verify (Task 4) and `apply_lsp_env` (Task 5). `LangArg` defined in Task 4, set in `parse_implement_args`, persisted/re-read on resume. `profile.lsp_env` (the 2a field, empty for rust until Task 2 populates it) is the move's source.

**Risks to flag for plan review:** (a) the lsp-env injection seam in `build_warm_impl` — confirm where the impl agent's `McpServerSpec`s are finalized (AcpBackend mint vs bridge-container `cfg.mcp`/`-c mcp_servers.*`) so `apply_lsp_env` runs on the right copy for BOTH delivery paths; (b) the go cache mount semantics (does `cache_binding` mount `warm_vol → dep_cache_path=/go/pkg/mod` correctly for go as it does cargo? verify the binding output); (c) `GOFLAGS=-mod=mod` vs a vendored/`-mod=readonly` repo under locked egress (warm `go mod download` must populate the cache the verify then reads offline — confirm the egress/proxy lets `go mod download` through like `cargo fetch`); (d) resume must not re-detect a different language.
