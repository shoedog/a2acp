# LSP-MCP C2a Step 2b — Go in-container implementor Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans. Steps use checkbox (`- [ ]`) syntax.

**Goal:** Make the containerized `implement` flow support **Go** end-to-end: a combined Rust+Go toolchain image, a `go` `[[languages]]` profile (warm `go mod download` + `go` verify), the verify-egress allowlist extended for Go module hosts, language **detection** (a typed API in lsp-mcp) replacing the hardcoded `id=="rust"` selection, an `implement --lang <auto|id|none>` flag + preflight (`none` → bare, verify SKIPPED, NO lsp), and the **lsp-env move** so the in-container lsp-mcp gets the *selected* profile's env (cargo for Rust, go for Go) and runs `--lang auto`. Rust stays byte-for-byte.

**Architecture:** C2a Step 2a (merged `5644414`) made the warm/verify seam config-driven via `[[languages]]` profiles selected by `pick_rust_profile` (`id=="rust"`). Step 2b (1) adds `go`+`gopls` to the toolchain image AND Go module hosts to the verify-egress filter; (2) adds a `go` profile + a rust `lsp_env` to the configs AND flips the impl agent's lsp `--lang rust`→`auto`; (3) adds a typed `Detection` to lsp-mcp + makes the bridge depend on it; (4) replaces `pick_rust_profile` with `select_profile` (detection/explicit/none + preflight) returning `Option<LanguageProfile>`, an `implement --lang` flag, a distinct `SKIPPED` verify outcome for `--lang none`, and resume-persisted language selection; (5) the **lsp-env move** — inject the selected profile's Lsp-context env into the impl agent's lsp `McpServerSpec` (via `cache_binding`, since the field is private), and DROP the lsp server entirely for `--lang none`. Then a live Go gate + a Rust byte-for-byte regression.

**Tech Stack:** Rust (crates/lsp-mcp, crates/bridge-core, crates/bridge-container, bin/a2a-bridge), TOML config, a Docker toolchain image (Go + gopls), a tinyproxy egress allowlist.

**Handoff context:**
- **Branch:** `feat/lsp-mcp-c2a-step2b` (off `main` = `5644414`: C2a Step 1 + 2a + the lsp-mcp Go *nav* already shipped). `git checkout feat/lsp-mcp-c2a-step2b` first.
- **Spec:** `…/specs/2026-06-15-lsp-mcp-slice-c2-design.md` §1 (atoms + detection; "never parse error strings"; "in-container lsp flips `--lang rust`→`auto`"), §2/§2.1 (`[[languages]]` schema; `--lang none` → bare + verify `SKIPPED`; verify `GOMODCACHE=/cache/gomodcache`), §3/§6/§8 (image, go profile, gate).
- **What 2a built (USE it):** `bridge_core::profile::{LanguageProfile, VerifyCommand, CacheCtx{Fetch,Lsp,Verify}, cache_binding, from_parts}` — **`lsp_env`/`verify_env`/`fetch_env`/`dep_cache_path`/`verify_cache_path` are PRIVATE**; read per-context env via `profile.cache_binding(ctx, warm_vol, verify_vol).env`. `config::{LanguageToml (REQUIRED dep_cache_path/verify_cache_path; optional lsp_env/verify_env/fetch_env/image), LanguageVerifyToml, language_profiles, to_profile}`; infra-only `VerifyConfig`; the `[verify].commands` legacy-reject (`config.rs:787`). `pick_rust_profile(&cfg)` called pre-`into_snapshot()` at the FRESH (~`main.rs:1624`) + RESUME (~`main.rs:1921`) sites + a TEST caller (~`main.rs:4072`), threaded into `warm_lsp_deps_step`/`build_warm_impl`/`run_verify_step`/`run_warm_loop`/`ProdEffects`/`tweak::run_tweak_loop` (all take `&LanguageProfile`).
- **lsp-mcp:** `Lang{Rust,Python,Go}` + `Lang::as_str()` (`"rust"`/`"go"` — the `[[languages]].id` key) + `detect_lang(repo)->anyhow::Result<Lang>` (ambiguous→Err, none→Err, single→Ok) in `crates/lsp-mcp/src/lang.rs`. The impl agent's lsp MCP arg is currently **`--lang rust`** (`examples/a2a-bridge.containerized.toml:185`); on a Go repo `lsp-mcp` hard-bails (`lib.rs:60-69`).
- **MCP env path (the lsp-env-move seam, CONFIRMED by both plan-reviewers):** `[[agents.mcp]]`→`config::McpToml.env`→`bridge_core::mcp::McpServerSpec.env`. For the impl agent (container_rw, codex-native) the live copy is `ccfg.mcp` in `build_warm_impl` (`main.rs:~1317`), set after `container_rw_cfg_from_entry`, before `ContainerRwBackend::new_warm`, and rendered to `-c mcp_servers.*` at `crates/bridge-container/src/lib.rs:233`. Mutate THAT copy.
- **Verify egress:** warm + verify run behind `a2a-verify-proxy`, a default-deny allowlist `deploy/containers/tinyproxy.verify.filter` (currently `crates.io` + `github.com` ONLY).
- **Resume:** `ImplementCheckpoint` (`implement_resume.rs:16`) has NO lang field; schema-v1 decode tests exist (`implement_resume.rs:308`) — any new field MUST be `#[serde(default)]`.

---

## File structure

| File | Create/Modify | Responsibility |
| --- | --- | --- |
| `deploy/containers/toolchain.Containerfile` | Modify | Add Go (pinned) + `gopls`. |
| `deploy/containers/tinyproxy.verify.filter` | Modify | Allowlist Go module/sum hosts (`golang.org`). |
| `crates/lsp-mcp/src/lang.rs` | Modify | Add a typed `Detection{Detected(Lang),None,Ambiguous}` + `detect(repo)->Detection` (no error-string parsing); `detect_lang` keeps its Result API (delegates). |
| `bin/a2a-bridge/Cargo.toml` | Modify | `lsp-mcp = { path = "../../crates/lsp-mcp" }`. |
| `bin/a2a-bridge/src/main.rs` | Modify | `--lang` flag; `select_profile`→`Option<LanguageProfile>`; thread the Option; `SKIPPED` verify for `none`; the lsp-env move + lsp-drop in `build_warm_impl`; update all 4 `pick_rust_profile` sites. |
| `bin/a2a-bridge/src/{implement.rs, implement_resume.rs, verify.rs, tweak.rs}` | Modify | `ImplementArgs.lang`; checkpoint `profile_id`; `VerifyOutcome::Skipped`; `classify` handles it. |
| `examples/a2a-bridge.containerized.toml` (+ `.podman.toml`; `.sonnet.toml` on-disk) | Modify | go profile; rust `lsp_env`; impl lsp `--lang rust`→`auto`; remove `CARGO_*` from impl lsp mcp env. |

---

### Task 1: Combined image (Go + gopls) + verify-egress allowlist for Go

**Files:** Modify `deploy/containers/toolchain.Containerfile`, `deploy/containers/tinyproxy.verify.filter`

- [ ] **Step 1: Image.** After `rustup component add rust-analyzer rust-src` + the `COPY --from=lspbuild` in the FINAL stage, add (own layer → Rust layers stay cached):
```dockerfile
# C2a Step 2b: Go toolchain + gopls. Pinned; GOTOOLCHAIN=local prevents auto-download drift.
ENV GO_VERSION=1.23.4
RUN curl --proto '=https' --tlsv1.2 -sSfL "https://go.dev/dl/go${GO_VERSION}.linux-$(dpkg --print-architecture).tar.gz" \
      -o /tmp/go.tgz \
 && tar -C /usr/local -xzf /tmp/go.tgz && rm /tmp/go.tgz
ENV PATH=/usr/local/go/bin:/root/go/bin:$PATH GOTOOLCHAIN=local
RUN go install golang.org/x/tools/gopls@v0.17.1
```

- [ ] **Step 2: Egress allowlist (B2 — both reviewers).** The verify proxy is default-deny with cargo-only hosts. `go mod download` + the verify's `go build/test` need Go module hosts. Read `deploy/containers/tinyproxy.verify.filter` (anchored ERE, one host per line) and append:
```
(^|\.)golang\.org$
(^|\.)storage\.googleapis\.com$
```
(`golang.org` covers `proxy.golang.org` + `sum.golang.org`; `storage.googleapis.com` covers the default GOPROXY's blob redirects.) If a separate podman filter or a `deploy/containers/*egress*.sh` runbook references the cargo hosts, add the Go hosts there too (grep `crates\.io` under `deploy/`).

- [ ] **Step 3: Build + recreate proxy + smoke.** `docker build -t a2a-toolchain:latest -f deploy/containers/toolchain.Containerfile .`; recreate the verify proxy so it reloads the bind-mounted filter (the egress compose/script — grep `a2a-verify-proxy`). Then: `docker run --rm --entrypoint bash a2a-toolchain:latest -lc 'go version && gopls version && rustc --version && lsp-mcp --help >/dev/null && echo OK'` → expect `go1.23.4`, a gopls version, rustc, `OK`.

- [ ] **Step 4: Commit:**
```bash
git add deploy/containers/toolchain.Containerfile deploy/containers/tinyproxy.verify.filter
git commit -m "build(toolchain): Go 1.23.4 + gopls; allowlist Go module hosts in verify egress (C2a step 2b)"
```

---

### Task 2: Configs — go profile, rust `lsp_env`, flip impl lsp to `auto`, move `CARGO_*`

**Files:** Modify `examples/a2a-bridge.containerized.toml`, `…podman.toml` (tracked); `…containerized.sonnet.toml` on-disk (untracked dogfood).

- [ ] **Step 1: rust `lsp_env`** — add to the existing `rust` profile (the cargo env the in-container lsp-mcp needs, MOVED off the impl agent's mcp block):
```toml
lsp_env = { CARGO_HOME = "/cargo", CARGO_NET_OFFLINE = "true" }
```

- [ ] **Step 2: go profile** — add after the rust one. NOTE the verify cache lives UNDER `/cache` (the only volume verify mounts) — mirroring Rust's `CARGO_HOME=/cache/cargo` (M1, both reviewers):
```toml
[[languages]]
id = "go"
fetch = "go mod download"
warm_cache = "a2a-impl-lsp-cache-go"
dep_cache_path = "/go/pkg/mod"
verify_cache_path = "/cache"
fetch_env = { GOMODCACHE = "/go/pkg/mod", GOFLAGS = "-mod=mod" }
verify_env = { GOMODCACHE = "/cache/gomodcache", GOCACHE = "/cache/go-build", GOFLAGS = "-mod=readonly" }
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
(`fetch`/`lsp` use the warm dep volume `/go/pkg/mod`; `verify` uses its own persistent cache under `/cache` and `-mod=readonly` so verify can't mutate the committed `go.mod`/`go.sum` — it relies on the agent having tidied.)

- [ ] **Step 3: flip the impl agent's lsp to `auto` (B1 — both reviewers).** In EACH config's impl agent `[[agents.mcp]] name="lsp"` block, change `args = [..., "--lang", "rust", ...]` → `"--lang", "auto"` (the host reviewer lsp blocks are already `auto`; only the impl one is `rust`). `--target-cache /lsp-target` is Rust-only and harmlessly ignored for Go — leave it. Then DELETE the `[[agents.mcp.env]]` entries for `CARGO_HOME` and `CARGO_NET_OFFLINE` on that lsp block (they move to the profile; Task 5 re-injects per selected language). KEEP non-language env (e.g. `LSP_MCP_LOG`).

- [ ] **Step 4: Tests** — extend the Task-2a round-trip test: `language_profiles()` for the containerized config now yields BOTH `rust` and `go`, with the go profile's commands `[("gofmt",…),("vet","go vet ./...",true),("build","go build ./...",true),("test","go test ./...",true)]`. ADD a test asserting the impl agent's lsp MCP arg is `--lang auto` (parse the config, find the impl agent, find the `lsp` mcp spec, assert its args contain `auto` not `rust`). `cargo test -p a2a-bridge` → PASS.

- [ ] **Step 5: Commit:**
```bash
git add examples/a2a-bridge.containerized.toml examples/a2a-bridge.containerized.podman.toml bin/a2a-bridge/src/config.rs
git commit -m "config(implement): go profile + rust lsp_env; flip impl lsp to --lang auto; move CARGO_* to profile (C2a step 2b)"
```

---

### Task 3: Typed detection in lsp-mcp (no error-string parsing) + bridge dep

**Files:** Modify `crates/lsp-mcp/src/lang.rs`, `bin/a2a-bridge/Cargo.toml`; register use in `main.rs`.

- [ ] **Step 1: Failing test** (`lang.rs` `mod tests`): a temp dir with no markers → `Detection::None`; with `Cargo.toml` → `Detection::Detected(Lang::Rust)`; with `Cargo.toml`+`go.mod` → `Detection::Ambiguous`; with only `go.mod` → `Detection::Detected(Lang::Go)`.

- [ ] **Step 2: Run → fail** (`Detection`/`detect` absent).

- [ ] **Step 3: Implement (spec §1: classify markers in typed code, never parse error strings).** Refactor so the marker logic lives in ONE typed function and `detect_lang` delegates:
```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Detection { Detected(Lang), None, Ambiguous }

/// Typed root-marker detection (spec §1). No bail/strings — callers branch on the variant.
pub fn detect(repo: &Path) -> Detection {
    let is_rust = repo.join("Cargo.toml").is_file();
    let is_python = python_markers(repo) || has_real_pyproject(repo) || shallow_py_scan(repo);
    let is_go = repo.join("go.mod").is_file();
    match [is_rust, is_python, is_go].iter().filter(|b| **b).count() {
        0 => Detection::None,
        1 if is_rust => Detection::Detected(Lang::Rust),
        1 if is_python => Detection::Detected(Lang::Python),
        1 => Detection::Detected(Lang::Go),
        _ => Detection::Ambiguous,
    }
}
```
Rewrite `detect_lang` to call `detect` and map (`Detected→Ok`, `None`/`Ambiguous`→the existing `bail!` messages) so the `--lang auto` shim CLI path is unchanged.

- [ ] **Step 4: Run → PASS** — `cargo test -p lsp-mcp detect`.

- [ ] **Step 5: bridge dep + commit.** Add to `bin/a2a-bridge/Cargo.toml` `[dependencies]`: `lsp-mcp = { path = "../../crates/lsp-mcp" }` (+ `tempfile` dev-dep if absent). Build to confirm it links. Commit:
```bash
git add crates/lsp-mcp/src/lang.rs bin/a2a-bridge/Cargo.toml Cargo.lock
git commit -m "feat(lsp-mcp): typed Detection API; bridge depends on lsp-mcp (C2a step 2b)"
```

---

### Task 4: `--lang` flag, `select_profile` (Option), SKIPPED verify, resume persistence

**Files:** Modify `bin/a2a-bridge/src/main.rs`, `implement.rs`, `implement_resume.rs`, `verify.rs`, `tweak.rs`

- [ ] **Step 1: Failing tests.**
  - `parse_implement_args`: `--lang go`→`Explicit("go")`, `--lang none`→`None`, absent→`Auto`; `--resume` path does NOT accept `--lang`.
  - `select_profile`: `Explicit("go")`→go profile; `Explicit("bogus")`→Err listing configured ids; `None`→`Ok(None)`; `Auto`+rust-repo→rust profile; `Auto`+ambiguous→Err listing options; `Auto`+detected-but-no-matching-profile→Err.
  - `tweak::classify`: a `VerifyOutcome::Skipped` is `verify_ok=true` (does not block) AND is distinguishable from `NotConfigured`.
  - `ImplementCheckpoint`: round-trips a `profile_id` AND an old (no-field) schema-v1 blob still decodes (extend the existing `checkpoint_round_trips_*` test).

- [ ] **Step 2: Run → fail.**

- [ ] **Step 3: Implement.**
  - `LangArg { Auto, Explicit(String), None }` + `ImplementArgs.lang` (default `Auto`); handle `"--lang"` in `parse_implement_args` (~712): `auto|none|<id>`.
  - `VerifyOutcome::Skipped { reason: String }` in `verify.rs` (rendered `verify: SKIPPED (<reason>)`), distinct from `NotConfigured`. `tweak::classify` (`tweak.rs:60`) treats `Skipped` as `verify_ok=true` (eyes-open opt-out) — NOT a silent NotConfigured pass.
  - `fn select_profile(cfg, lang: &LangArg, repo) -> Result<Option<LanguageProfile>, BoxError>`: `None`→`Ok(None)`; `Explicit(id)`→find by id else Err(list ids); `Auto`→`lsp_mcp::lang::detect(repo)`: `Detected(l)`→find `id==l.as_str()` (else Err "detected {l}, no [[languages]] id={l}; add one or --lang none"); `None`/`Ambiguous`→Err (preflight: which markers + "pass --lang <id|none>; configured: [rust, go]").
  - Replace ALL 4 `pick_rust_profile` sites: fresh (~1624) + resume (~1921) + the test caller (~4072) + the def. Fresh: `let profile = select_profile(&cfg, &args.lang, &repo)?;` (`Option`). Resume: select by the persisted `profile_id` (NOT re-detect) — `Some(id)`→`Explicit(id)`, the bare sentinel→`None`.
  - Thread `Option<&LanguageProfile>` through `warm_lsp_deps_step`, `run_verify_step`, `build_warm_impl`, `run_warm_loop`, `ProdEffects`, `tweak::run_tweak_loop` (all currently `&LanguageProfile`). `None` ⇒ warm SKIPPED, verify ⇒ `VerifyOutcome::Skipped { reason: "--lang none" }`, lsp dropped (Task 5).
  - `ImplementCheckpoint` (+`#[serde(default)] profile_id: Option<String>` where `Some("rust")`/`Some("go")` = the chosen profile and `None` = bare/`--lang none`; persist post-selection in the fresh path). Old checkpoints decode (`#[serde(default)]`); resume re-selects by it.

- [ ] **Step 4: Run → PASS** — `cargo test -p a2a-bridge -p bridge-core -p lsp-mcp`.

- [ ] **Step 5: Commit:**
```bash
git add bin/a2a-bridge/src/main.rs bin/a2a-bridge/src/implement.rs bin/a2a-bridge/src/implement_resume.rs bin/a2a-bridge/src/verify.rs bin/a2a-bridge/src/tweak.rs
git commit -m "feat(implement): --lang auto|id|none, select_profile->Option, SKIPPED verify, resume lang (C2a step 2b)"
```

---

### Task 5: The lsp-env move (+ drop lsp for `--lang none`)

**Files:** Modify `bin/a2a-bridge/src/main.rs` (`build_warm_impl`), pure helpers.

- [ ] **Step 1: Failing tests** (pure helpers, `main.rs` `mod tests`):
  - `apply_lsp_env(specs: &mut Vec<McpServerSpec>, lsp_env: &[(String,String)])` sets/overrides env on the `name=="lsp"` spec (profile env WINS over a same-key config env), leaves others (prism) untouched.
  - `drop_lsp(specs: &mut Vec<McpServerSpec>)` removes the `name=="lsp"` spec, leaves others.

- [ ] **Step 2: Run → fail.**

- [ ] **Step 3: Implement** in `build_warm_impl` (~`main.rs:1317`), on the `ccfg.mcp` copy (the codex-native delivery source — confirmed by both reviewers), after `container_rw_cfg_from_entry`, before `ContainerRwBackend::new_warm`:
  - `profile.lsp_env` is PRIVATE — get the env via the existing Lsp `cache_binding` call: `let lsp = profile.cache_binding(CacheCtx::Lsp, cache, ""); ... apply_lsp_env(&mut ccfg.mcp, &lsp.env);` (the 2a code already calls `cache_binding(Lsp, …)` here for the mount — reuse its `.env`). Profile env OVERRIDES config env so the selected language wins (rust→`CARGO_HOME=/cargo`+`CARGO_NET_OFFLINE=true`; go→`GOMODCACHE`/`GOFLAGS`).
  - When the resolved profile is `None` (`--lang none`): `drop_lsp(&mut ccfg.mcp)` so NO lsp server starts (M3 — else it would launch with `--lang auto` and no warm cache).

- [ ] **Step 4: Run → PASS** — `cargo test -p a2a-bridge`. Confirm Rust byte-for-byte: with the rust profile the lsp spec ends with exactly `CARGO_HOME=/cargo`+`CARGO_NET_OFFLINE=true` (now from the profile via `cache_binding(Lsp).env`, formerly config) — same effective env, and prism untouched.

- [ ] **Step 5: Commit:**
```bash
git add bin/a2a-bridge/src/main.rs
git commit -m "feat(implement): lsp-env move (profile Lsp env -> impl lsp MCP) + drop lsp for --lang none (C2a step 2b)"
```

---

### Task 6: Live gate — Go implement + Rust regression + preflight

**Files:** none (validation); a throwaway Go fixture.

- [ ] **Step 1: Rust regression (byte-for-byte).** Rebuild (`cargo build --release --bin a2a-bridge`); run a trivial Rust `implement` (omit `--lang` → Auto re-detects rust) against this repo with `containerized.toml`. Expect warm + `verify: PASS (fmt ✓ · clippy ✓ · build ✓ · test ✓)` exactly as Step 2a; the in-container lsp (now `--lang auto`) re-detects rust + gets `CARGO_*` from the profile. Discard hand-off.
- [ ] **Step 2: Go live gate.** Create a tiny throwaway Go module (under `allowed_cwd_root`: `go.mod` + a pkg with a func + a `_test.go`), commit. `a2a-bridge implement "<trivial Go task + passing test>" --repo <gofixture> --lang auto --config examples/a2a-bridge.containerized.toml`. Expect: detection→`go`; warm `go mod download` (through the extended egress); impl edits; `verify: PASS (gofmt ✓ · vet ✓ · build ✓ · test ✓)` via the go profile + combined image; in-container gopls (lsp `--lang auto`) gets `GOMODCACHE` (lsp-env move). Hand-off commits.
- [ ] **Step 3: Preflight + none.** `--lang none` on the Go fixture → bare run, `verify: SKIPPED (--lang none)`, no lsp container, still commits. `--lang auto` on a no-marker dir AND an ambiguous (rust+go) dir → the preflight Err listing options, NO container spawned. Reap all clones + containers.
- [ ] **Step 4:** Validation only (no commit). Record gate results in the wrap-up.

---

## Step 2b done — what ships

Polyglot containerized implementor: detection (or explicit `--lang`) selects a `rust`/`go` profile; the combined image carries both toolchains + gopls; warm + verify + the in-container lsp (`--lang auto`) run with the selected profile's env (the lsp-env move); the verify egress allows Go module hosts; `--lang none` is the eyes-open opt-out (verify `SKIPPED`, no lsp). Rust byte-for-byte. **Closes C2a.** Follow-ons: C2b, C2c (deferred).

**Final review before merge:** dual review (codex gpt-5.5-high + Opus 4.8) on the diff. Then finish-the-branch.

---

## Self-review notes

**Plan-review fixes folded in (codex + opus, both fix-then-ship):** B1 impl lsp `--lang rust`→`auto` (Task 2 Step 3 + test); B2 Go egress hosts (Task 1 Step 2); M1 verify `GOMODCACHE=/cache/gomodcache`+`GOCACHE=/cache/go-build` (Task 2 Step 2); M2 `lsp_env` is private → use `cache_binding(Lsp).env`, seam = `ccfg.mcp` in `build_warm_impl` (Task 5); M3 drop the lsp spec for `--lang none` (Task 5); M4 distinct `VerifyOutcome::Skipped` so `--lang none` reports `SKIPPED` (not silent `NotConfigured`) (Task 4); M5 resume persists `profile_id`, all 4 `pick_rust_profile` sites updated, old checkpoints decode (Task 4); m1 enumerated `Option<&LanguageProfile>` signatures (Task 4 Step 3); m2 typed `Detection` in lsp-mcp instead of error-string parsing (Task 3).

**Spec coverage:** §3 image (T1); egress (T1); §1/§2 go profile + schema reuse + flip-to-auto (T2); §1 typed detection (T3); §2.1 `--lang`/`none`/preflight/SKIPPED (T4); §6 lsp-env move (T2+T5); §8 gate + Rust regression (T6).

**Type consistency:** `Detection` (lsp-mcp, T3) → `select_profile` (T4) → `Option<LanguageProfile>` threaded (T4) + `apply_lsp_env`/`drop_lsp` (T5). `LangArg` (T4) set in `parse_implement_args`, persisted as checkpoint `profile_id` (T4). `VerifyOutcome::Skipped` (T4, verify.rs) consumed by `tweak::classify` + the hand-off renderer. `profile.cache_binding(Lsp).env` (T5) is the private-field-safe source of `lsp_env`.

**Ordering:** T1 (image+egress) → T2 (configs; note: between T2's CARGO_* removal and T5's re-inject, an *interim* rust run would lose the lsp cargo env — acceptable since tasks land together before any rust DoD; the lsp still starts, just without offline cargo env, non-fatal to verify which has its own cache) → T3 (detection) → T4 (selection/flag/resume) → T5 (env move) → T6 (gate). Each compiles + tests green on its own except the T2↔T5 env-on-lsp interim (cosmetic, not a verify break).
