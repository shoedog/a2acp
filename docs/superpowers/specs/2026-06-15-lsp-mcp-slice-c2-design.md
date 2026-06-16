# LSP-MCP Slice C2 — polyglot in-container implementor — Design

**Status:** approved direction (brainstormed 2026-06-15; dual spec-review codex+opus 2026-06-16 → fix-then-ship folded). Extends the in-container `implement` + per-turn `ContainerRw` paths from Rust-only to **language-aware (polyglot)**, so the bridge's containerized implementor can edit + verify + nav **Go** repos (and any future language) the way the **host reviewers** already nav any language via `--lang auto`. Builds on the shipped **Go (gopls) host-reviewer nav** increment (the `--lang`→language-server registry + `go_config` + `Readiness::Gopls`).

**Predecessors:** Slice A (host reviewers), Slice B/B2b (the `ContainerRw` backend + `implement` clone→edit→verify→review→commit loop + warm-deps), Slice C1 (the multi-language lsp registry: Python/basedpyright), and the Go host-reviewer increment (gopls). This slice carries that registry's language-awareness **into the container** (the implementor side).

## Goal

Today the containerized implementor is hard-wired to Rust: the `impl` agent's in-container lsp is `--lang rust`, the warm-deps step runs `cargo fetch --locked`, `[verify]` runs cargo commands, and **the cache env + mounts are Rust-shaped, hardcoded in three separate code sites** (`compose_warm_fetch`, `compose_verify`, the `implement` in-container-lsp mount in `main.rs`). That three-site scatter is a **code smell in its own right** (three places independently encode "the Rust cache layout") — so C2a's FIRST step is a pure refactor: consolidate them behind a **single cache/env seam** (§2.2, Rust-only, byte-for-byte), and only THEN make that one seam profile-driven. The **host reviewers** are already `--lang auto`. C2 closes the asymmetry: detect the session/clone language, drive a per-language **profile** (fetch + lsp-runtime + verify env, cache mounts, verify commands, lsp lang) through that single seam, and run the implementor on **Go** (then any language) — across both `implement` and the per-turn `ContainerRw` path, so **one serve handles mixed-language sessions**.

## Non-goals (this slice)

- **Multi-language WITHIN one cwd** (a monorepo root spanning `services/foo/go.mod` + `services/bar/Cargo.toml`, edited + verified in ONE run) → **C2c** (deferred, designed-for below). C2a/C2b are **single-language-per-cwd**; a monorepo is covered by narrowing `--repo` / `--session-cwd` to the single-language service subdir. C2c needs multi-root detection + per-service verify + **multi-root LSP nav** (folds in the lsp-mcp `--project-root`/subdir-rooted deferral from Slice C1).
- **Third-party dep cache under `serve`** (per-turn `ContainerRw`) → deferred (§4). C2b delivers **workspace-scoped** Go nav under serve; warmed third-party nav is `implement`-only.
- **TypeScript / JS / Python in-container** — the profile schema + combined-image seam are designed so these are later config + image additions; C2a/C2b ship **Rust + Go** profiles only.
- **No lsp-mcp client changes** — `--lang auto` + `go_config` already shipped; C2 consumes them. The in-container lsp flips `--lang rust` → `--lang auto`.
- **No new review/loop semantics** — the verify→review→tweak loop, `ContainerRw` lifecycle, reaping, merge hand-off are unchanged; C2 makes the **fetch/verify/lsp env + commands + cache mounts + image** language-selected.

## §1. Architecture — detect → select profile → run (Approach A)

A **combined Rust+Go toolchain image** + **config-driven per-language profiles** + **auto-detection**:

1. **Detect** the session/clone language via the **single source of truth** — `lsp_mcp::lang::detect_lang`. **`bin/a2a-bridge` must add `lsp-mcp` as a path dependency** (it is a lib+bin; the bridge does NOT depend on it today). `detect_lang` returns `anyhow::Result<Lang{Rust,Python,Go}>` where **`None` and `Ambiguous` are ERRORS, not 0-element results**, and its message ("pass an explicit `--lang`") is the *reviewer* affordance (there is no `--lang` knob on `implement`). So wrap it in a **typed bridge-side outcome — `LangDetection::{ Detected(LangRoot{id,path}), NoMarker, Ambiguous, Unsupported(Lang) }`** — and never parse `detect_lang`'s error strings. `Unsupported(Lang)` = a language detected but with no matching `[[languages]]` profile. `id` is `Lang::as_str()` (`"rust"`/`"go"`). The seam is **multi-root-ready** (`detect_repo_langs(root) -> Vec<LangRoot>`); for C2a/C2b it wraps the single-root `detect_lang` and yields one `Detected` (or `NoMarker`/`Ambiguous`); the multi-root walk is C2c.
2. **Select** the `[[languages]]` profile whose `id` equals the detected `Lang::as_str()`.
3. **Run** that profile's fetch/verify commands + per-context env + cache mounts (below), in the profile's image (default `[verify].image`), and spawn the impl agent's in-container lsp as `--lang auto`.

**Profiles are per-language ATOMS, selected as a SET — never per-combo.** `[[languages]]` defines one profile per language (`rust`, `go`, …); the selection for a repo is the *set* of detected languages (`detect_repo_langs -> Vec<LangRoot>`). A "Rust+Go" repo is the set `{rust, go}` → apply the **union** of the `rust` and `go` bindings; there is NEVER a `rust+go` profile (that path is 2^N — combinatorial and untenable). "None/n/a" is the **empty set** (no profile applied; `--lang none` forces it) — not a profile. **C2a/C2b gate the selection to ≤1** (single-language-per-cwd; `>1` → refuse, narrow `--repo`); **C2c lifts the gate and iterates the set** (union of per-profile bindings + per-service verify + multi-root LSP). The gate is a restriction, not a model limit — so C2c is purely additive. Because the `cache_binding` seam (§2.2) is per-profile, "apply a set" = "apply each member"; C2a applies the one.

Language knowledge lives in **config**, not Rust branches — the next language is a profile + an image. The Rust changes are: the `lsp-mcp` dep + typed detection, profile parsing/selection, and routing all cache/env through the **single seam** (§2.2) the profile drives.

## §2. Config surface — `[verify]` (infra) + `[[languages]]` (per-language)

`[verify]` keeps the **language-agnostic infra** — `image`, `egress`, `network`, `proxy`, `no_proxy`, `runtime`, `cache`. Its `commands` field is **removed** (§2.1). A new `[[languages]]` table carries the per-language specifics. **The cache story spans THREE distinct contexts with DIFFERENT env** (the spec-review caught that a single `warm_env` conflates them — e.g. `CARGO_NET_OFFLINE=true` is correct for the LSP runtime but would BREAK a cold `cargo fetch`):

```toml
[verify]
image    = "a2a-toolchain:latest"   # COMBINED rust+go for C2a; default image for all profiles
egress   = "locked"
network  = "a2a-verify-egress"
proxy    = "http://a2a-verify-proxy:8888"
no_proxy = "localhost,127.0.0.1"
# (no `commands` here anymore — they live in [[languages.verify]])

[[languages]]
id      = "rust"                          # matches Lang::as_str() from lsp_mcp's detect_lang
# (1) WARM/FETCH — compose_warm_fetch. Populates the dep cache; MUST be able to reach the network (NO offline).
fetch       = "cargo fetch --locked"
fetch_env   = { CARGO_HOME = "/cargo" }   # NO CARGO_NET_OFFLINE here (would break the fetch)
warm_cache  = "a2a-impl-lsp-cache"        # the named volume the fetch fills (per-repo suffix appended)
# (2) IN-CONTAINER LSP runtime — the impl agent's gopls/RA. warm_cache mounts at lsp_cache_mount (:ro);
#     lsp_env is the runtime env (offline so RA never fetches). Drives the main.rs cache-mount, profile-selected.
lsp_cache_mount = "/cargo"
lsp_env         = { CARGO_HOME = "/cargo", CARGO_NET_OFFLINE = "true" }
# (3) VERIFY — compose_verify. Exports verify_env (REPLACING today's hardcoded cargo exports) + mounts the
#     verify cache. (image override optional, default [verify].image.)
verify_env  = { CARGO_HOME = "/cache/cargo", CARGO_TARGET_DIR = "/cache/target" }
[[languages.verify]]
name = "fmt";    cmd = "cargo fmt --all -- --check"
[[languages.verify]]
name = "clippy"; cmd = "cargo clippy --all-targets --all-features --locked -- -D warnings"
[[languages.verify]]
name = "build";  cmd = "cargo build --locked"
[[languages.verify]]
name = "test";   cmd = "cargo test --workspace --locked --exclude bridge-container -- --skip process::tests::terminate_reaps_child_no_zombie --skip process::tests::term_ignoring_loop_forces_group_sigkill --skip process::tests::drop_group_kills_descendants"

[[languages]]
id      = "go"                            # matches Lang::as_str() == "go"
fetch       = "go mod download all"
fetch_env   = { GOMODCACHE = "/gomodcache" }   # NO offline during download
warm_cache  = "a2a-impl-go-cache"
lsp_cache_mount = "/gomodcache"
lsp_env         = { GOMODCACHE = "/gomodcache", GOFLAGS = "-mod=mod" }
verify_env  = { GOMODCACHE = "/cache/gomodcache", GOCACHE = "/cache/go-build" }
[[languages.verify]]
name = "build"; cmd = "go build ./..."
[[languages.verify]]
name = "vet";   cmd = "go vet ./..."
[[languages.verify]]
name = "test";  cmd = "go test ./..."
[[languages.verify]]
name = "fmt";   cmd = "test -z \"$(gofmt -l .)\""
```

**Profile fields:** `id` (matches `Lang::as_str()`); `fetch` + `fetch_env` + `warm_cache` (the warm-deps command, its **network-capable** env, the cache volume it fills); `lsp_cache_mount` + `lsp_env` (where `warm_cache` mounts `:ro` for the in-container lsp + the lsp's runtime env — profile-selected, replacing the hardcoded `/cargo:ro`); `verify_env` (exported by `compose_verify`, replacing the hardcoded `CARGO_HOME`/`CARGO_TARGET_DIR`); an OPTIONAL `image`; `[[languages.verify]]` (ordered `{name, cmd}`, optional `gate`). The verify cache reuses `[verify].cache` at `/cache` (so `verify_env` paths live under `/cache`).

## §2.1 No backward-compat + the unprofiled-language SAFETY rule

`[verify].commands` is **removed**, and because `VerifyToml` does not `deny_unknown_fields`, a deleted field would become **silent dead config** — so add an **explicit parse error** for a legacy `[verify].commands`/`[[verify.commands]]` ("moved to `[[languages.verify]]`"). The "≥1 verify command" invariant (today in `VerifyToml::to_config`) **moves** to "the *matched profile* has ≥1 verify command."

**Migration is broader than two files:** several tracked `implement` configs use `[[verify.commands]]` (e.g. `a2a-bridge.slicing-implement.toml`, the two `containerized` configs). Migrate **all tracked `implement` configs** to a `rust` profile (+ add a `go` profile to the containerized ones).

**SAFETY + the `none` escape hatch.** The danger to avoid is a *silent* skip: the tweak loop treats `VerifyOutcome::NotConfigured` as `verify_ok = true` (`tweak.rs::classify`, verified), so an auto-skip + a review APPROVE → `Approved` → **merge of UNVERIFIED code**. The rule:
- **`implement --lang <auto|id|none>`** (default `auto`). `auto` detects via §1; `<id>` forces a configured profile (operator override of detection); **`none`** is the explicit, eyes-open opt-out — run with **no language profile**: no warm-deps, the impl agent's lsp not spawned (no LSP weight), and **verify reported as `SKIPPED (--lang none)` in the hand-off** (NOT silently "passed"). For someone whose language isn't profiled, or who doesn't want LSP/verify weight.
- **Auto-detect on a `NoMarker` / `Ambiguous` / `Unsupported(Lang)` repo, with no explicit `--lang`, HARD-FAILS preflight** (before any edit) with a message that **LISTS the valid options** — the configured profile `id`s, plus `--lang none` to run bare, plus the narrow-`--repo`-to-a-single-language-subdir hint. So the operator is never trapped, but a skip is always an explicit choice, never silent.
- Best-effort *implicit* skip survives ONLY for the LSP **warm-deps** step (degrades to workspace-only nav) and non-`implement` paths — never for the `implement` verify gate.

## §2.2 The single cache/env seam (a cleanup, not just polyglot)

The cache env + mounts are currently **hardcoded in three independent sites** — `compose_warm_fetch` (`CARGO_HOME=/cargo`, `cargo fetch`), `compose_verify` (`CARGO_HOME=/cache/cargo CARGO_TARGET_DIR=/cache/target` + the `/cache` mount), and the `implement` in-container-lsp mount in `main.rs` (`{cache}:/cargo:ro`). Threading a profile into three hardcoded sites would just spread the smell. Instead, **introduce ONE seam** all three consume:

```
enum CacheCtx { Fetch, Lsp, Verify }
struct CacheBinding { env: Vec<(String, String)>, mounts: Vec<VolumeMount> }
fn cache_binding(profile: &LanguageProfile, ctx: CacheCtx) -> CacheBinding
```

`compose_warm_fetch` consumes `cache_binding(p, Fetch)`, `compose_verify` consumes `cache_binding(p, Verify)`, and the `main.rs` impl-lsp mount consumes `cache_binding(p, Lsp)` — each site's hardcoded cargo env/mount is **deleted** and replaced by the seam's output. A new context (or language) becomes a single-place change.

**Sequencing (the cleanup goes FIRST):** C2a **step 1** extracts this seam with a **hardcoded `rust` `LanguageProfile`** and routes the three sites through it — a **pure refactor, byte-for-byte identical behavior**, regression-locked before any Go exists. C2a **step 2** makes `LanguageProfile` config-parsed (`[[languages]]`) + detection-selected, and adds the `go` profile. The §2 fields (`fetch_env`/`lsp_env`/`verify_env`/`warm_cache`/`lsp_cache_mount`) are exactly the data `cache_binding` reads per context.

## §3. C2a — `implement` (single-language-per-cwd)

The first buildable slice, in two steps. **Step 1 (pure refactor, no Go):** extract the §2.2 `cache_binding` seam with a hardcoded `rust` profile; route `compose_warm_fetch`, `compose_verify`, and the `main.rs` impl-lsp mount through it; byte-for-byte regression-locked. **Step 2 (profile + Go):** the remaining touch-sites:

- **Combined image:** augment `a2a-toolchain` (Containerfile) with `go` + `gopls` (+ existing Linux `lsp-mcp`/rust-analyzer). `[verify].image` points at it. (Build-cost note: §6.)
- **`lsp-mcp` dep + typed detection (§1):** add the path dep; implement `detect_repo_langs` → `LangDetection`. **`implement --lang <auto|id|none>`** (§2.1): `auto` selects via detection; `<id>` forces a configured profile; `none` runs bare (no profile, verify `SKIPPED` in hand-off). Auto-detect on `NoMarker`/`Ambiguous`/`Unsupported` with no `--lang` → preflight error **listing the valid options** (profile `id`s + `--lang none` + narrow-`--repo` hint) — never the raw `detect_lang` "pass --lang" message.
- **Config parse:** `[[languages]]` → `LanguageProfile`; remove `[verify].commands` + add the legacy-reject parse error (§2.1); move the "≥1 verify command" invariant to the matched profile.
- **Profile-drive the seam:** the §2.2 `cache_binding` now reads the selected profile (Fetch→`fetch`+`fetch_env`+`warm_cache`; Lsp→`warm_cache`@`lsp_cache_mount`+`lsp_env`; Verify→`verify_env`+`[verify].cache`). Flip the impl lsp `args` to `--lang auto`.

**DoD:** `a2a-bridge implement <task> --repo <go-repo-or-go-service-subdir>` converges — codex edits Go → warm `go mod download` fills `warm_cache` → in-container gopls resolves third-party from that cache (mounted `:ro` at `/gomodcache`) → verify `go build`/`go test`/`go vet`/`gofmt` against the persistent `[verify].cache` (cold-once, then warm — mirrors Rust; the warm cache feeds nav, not verify) → review-the-diff → commit/amend → hand-off. The **Rust** path stays **byte-for-byte** (the migrated `rust` profile + the §2.2 seam reproduce today's fetch/verify/lsp env + mounts exactly).

## §4. C2b — polyglot `serve` / `run-workflow` (workspace-scoped)

The per-turn `ContainerRw` path runs the `impl` agent per turn; the `--lang auto` + combined-image flip make a **Go session** edit + nav Go with no loop code. There is **no verify/review** here (those are `implement`-only).

- **Scoped DoD — workspace-only nav under serve.** The per-turn path composes ONLY the static sandbox volumes (`compose_container_rw`); the runtime `warm_cache`/`/lsp-target` mounts exist ONLY in the warm `implement` path (`main.rs`), and there is no per-turn warm-deps step. So per-turn nav is **cold / workspace-scoped for Rust today too** — Go is identical, not a regression. C2b's DoD is: one serve handles a Go session AND a Rust session, each editing + **workspace-scoped** navigating correctly (`docker events`/the lsp call log shows `lang=go` vs `lang=rust` per session). Third-party-resolving nav under serve (per-session prewarm/mount) is **deferred** (below), not an open question.

## §5. C2c — multi-language-within-one-cwd (DEFERRED, designed-for)

Out of scope; **additive on the locked-in model** — C2c just **lifts the ≤1 gate** and applies the **union of per-profile bindings** over the full `detect_repo_langs` set (no new profile model, no per-combo profiles). **Warm:** warm each selected language's cache (iterate the set through the same `cache_binding`/`Fetch` seam). **Per-service verify:** map the diff's changed files to their nearest language-root, run each touched root's profile verify in its subdir. **Multi-root LSP nav:** the only genuinely-hard part — `lsp-mcp --lang auto` is single-root and refuses multi-marker roots; subdir-rooted/multi-root nav (`--project-root`, concurrent language servers) is the lsp-mcp deferral folded in here. C2a/C2b only ensure `detect_repo_langs` returns a set, selection is keyed by `id`, and the seam is per-profile.

## §6. Image strategy

C2a ships ONE **combined** Rust+Go image (`a2a-toolchain`) as `[verify].image` — simplest for polyglot serve. The OPTIONAL per-profile `image` is the designed-for seam for future **separate** images (rust / go / rust+go / +Python / +js-ts), chosen by memory/footprint, config-only. **Build-cost note (spec-review):** the Containerfile already compiles `lsp-mcp` from source + a full Rust toolchain + rust-analyzer + coverage tools; adding `gopls` (typically `go install`, another toolchain compile) is a non-trivial extra layer — build *time* (not just size) grows, and the `.dockerignore`/build-context discipline from B2b-2 must be preserved.

## §7. Tests / regression

- **Pure / hermetic:** the **`cache_binding(profile, ctx)` seam** — for each ctx (Fetch/Lsp/Verify) assert the `{env, mounts}` for the `rust` and `go` profiles (this is the keystone — it's where all three sites' behavior is now decided). The detection wrapper → `LangDetection` (`Cargo.toml`→`Detected(rust)`, `go.mod`→`Detected(go)`, none→`NoMarker`, ambiguous→`Ambiguous`, a `Lang` with no profile→`Unsupported`) + profile selection by `id`. Assert the **`implement` preflight FAILS with the options listed** on `NoMarker`/`Ambiguous`/`Unsupported` (no `--lang`), and that **`--lang none` runs with verify reported `SKIPPED`** (the false-green-merge guard + the eyes-open escape). Assert a **legacy `[verify].commands` config → parse error**. Assert the Go `fmt` command `test -z "$(gofmt -l .)"` composes to a runnable `sh -c` script (the TOML→`sh` quoting footgun). (Multi-root detection tests are C2c.)
- **Regression:** the **Rust** `implement` + verify path is **byte-for-byte** via the migrated `rust` profile (assert the composed fetch/verify env + mounts equal today's hardcoded values). The lsp-mcp suites are untouched.
- **Go live gate (C2a DoD):** a containerized `implement` on a Go repo converges with `go build`/`go test` verify AND in-container gopls resolves a third-party symbol (cache mounted). **Serve live gate (C2b DoD):** one serve handles a Go session + a Rust session (workspace-scoped nav).
- **Config:** the migrated example configs parse (a parse test pinning the rust+go profiles + the legacy-`commands`-rejected path).

## §8. Execution order

1. **C2a step 1 (pure refactor, no Go):** extract the §2.2 `cache_binding` seam with a hardcoded `rust` `LanguageProfile`; route `compose_warm_fetch`/`compose_verify`/the `main.rs` impl-lsp mount through it; **byte-for-byte Rust regression-locked** (the composed env+mounts equal today's hardcoded values). **C2a step 2 (profile + Go):** combined image (Containerfile) → `lsp-mcp` dep + typed `detect_repo_langs`/`LangDetection` → `[[languages]]` parse (remove `[verify].commands`, add legacy-reject, move the ≥1-command invariant) → make `LanguageProfile` config-parsed so the seam is profile-driven → `implement --lang <auto|id|none>` + preflight (unprofiled→hard-fail-with-options; `none`→bare/verify-SKIPPED) → impl lsp `--lang auto` → migrate all tracked implement configs → Go `implement` live gate (incl. third-party gopls nav from the warmed cache).
2. **C2b:** confirm the per-turn `ContainerRw` path inherits the combined image + `--lang auto` → serve polyglot live gate (workspace-scoped).
3. **C2c:** deferred (its own spec when triggered).

Each of C2a/C2b is its own plan → subagent-driven implementation → review, dogfooded on the bridge (containerized codex implementor + gpt-5.5 reviewer), per the Go-increment playbook.

---

## Deferred (its own design pass when triggered)

- **C2c — multi-language-within-one-cwd** (§5): multi-root detection + per-service verify + multi-root LSP nav (folds in the lsp-mcp `--project-root`/subdir-rooted deferral).
- **Third-party dep cache under `serve`** (§4): per-session prewarm + profile-aware per-session cache mounts for the per-turn `ContainerRw` path (today only `implement` warms + mounts the dep cache).
- **Separate per-language images** (§6): config-only via the profile `image` override; tuned by memory/footprint when the combined image gets heavy.
- **TypeScript/JS + Python in-container:** later profiles + image additions onto this exact seam.
