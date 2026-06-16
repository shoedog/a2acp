# LSP-MCP Slice C2 — polyglot in-container implementor — Design

**Status:** approved direction (brainstormed 2026-06-15). Extends the in-container `implement` + per-turn `ContainerRw` paths from Rust-only to **language-aware (polyglot)**, so the bridge's containerized implementor can edit + verify + nav **Go** repos (and any future language) the way the **host reviewers** already nav any language via `--lang auto`. Builds directly on the shipped **Go (gopls) host-reviewer nav** increment (the `--lang`→language-server registry + `go_config` + `Readiness::Gopls`).

**Predecessors:** Slice A (host reviewers), Slice B/B2b (the `ContainerRw` backend + `implement` clone→edit→verify→review→commit loop + warm-deps), Slice C1 (the multi-language lsp registry: Python/basedpyright), and the Go host-reviewer increment (gopls). This slice carries that registry's language-awareness **into the container** (the implementor side).

## Goal

Today the containerized implementor is hard-wired to Rust: the `impl` agent's in-container lsp is `--lang rust`, the warm-deps step runs `cargo fetch --locked`, and `[verify]` runs cargo commands. The **host reviewers** are already `--lang auto` and serve Rust/Python/Go per repo. C2 closes that asymmetry: detect the session/clone language, drive a per-language **profile** (warm command + verify commands + lsp lang + optional image), and run the implementor on **Go** (then any language) — across both the `implement` subcommand and the per-turn `ContainerRw` path under `serve`/`run-workflow`, so **one serve handles mixed-language sessions** (a Go service in one request, a Rust service in another).

## Non-goals (this slice)

- **Multi-language WITHIN one cwd** (a monorepo root spanning `services/foo/go.mod` + `services/bar/Cargo.toml`, edited + verified in ONE run) → **C2c** (deferred, designed-for below). C2a/C2b are **single-language-per-cwd**; a monorepo is covered by narrowing `--repo` / `--session-cwd` to the single-language service subdir. C2c needs multi-root detection + per-service verify + **multi-root LSP nav**, the last of which folds in the lsp-mcp `--project-root`/subdir-rooted deferral from Slice C1.
- **TypeScript / JS / Python in-container** — the profile schema + combined-image seam are designed so these are later config + image additions, but C2a/C2b ship **Rust + Go** profiles only.
- **No lsp-mcp client changes** — the `--lang auto` detection + `go_config`/`pyright_config` registry already shipped; C2 consumes them. The in-container lsp just flips `--lang rust` → `--lang auto`.
- **No new review/loop semantics** — the `implement` verify→review→tweak loop, `ContainerRw` warm/per-turn lifecycle, reaping, and merge hand-off are unchanged; C2 only makes the **warm command + verify commands + image** language-selected instead of Rust-hard-coded.

## §1. Architecture — detect → select profile → run (Approach A)

A **combined Rust+Go toolchain image** + **config-driven per-language profiles** + **auto-detection**:

1. **Detect** the session/clone language via the **single source of truth** — `lsp_mcp::lang::detect_lang` (the same `Cargo.toml`→rust / `go.mod`→go / ambiguous→refuse predicate the host reviewers' `--lang auto` uses). Wrapped as `detect_repo_langs(root) -> Vec<LangRoot { id, path }>` whose `id` is `Lang::as_str()` (`"rust"`/`"go"`/…). The signature is **multi-root-ready** (returns a set); for C2a/C2b it wraps the single-root `detect_lang` and returns 0 or 1 element (the multi-root walk is C2c). There is exactly ONE detector — no config-side marker list to drift from it.
2. **Select** the `[[languages]]` profile whose `id` equals the detected `Lang::as_str()`.
3. **Run** that profile's **warm command** (replacing the hard-coded `cargo fetch`) and **verify commands** (replacing `[verify].commands`), in the profile's **image** (default `[verify].image`), and spawn the impl agent's in-container lsp as `--lang auto`.

Language knowledge lives in **config** (where the `--lang` registry already lives), not in Rust branches — adding the next language is a profile + an image, **zero bridge code**. The only Rust changes are: detection, profile selection, and making `compose_warm_fetch` + the verify runner read the selected profile.

## §2. Config surface — `[verify]` (infra) + `[[languages]]` (per-language)

`[verify]` keeps the **language-agnostic infra** it already has — `image`, `egress`, `network`, `proxy`, `no_proxy`, `runtime`, `cache`. Its `commands` field is **removed** (migrated into profiles; see §2.1). A new top-level `[[languages]]` table carries the per-language specifics:

```toml
[verify]
image    = "a2a-toolchain:latest"   # COMBINED rust+go for C2a (the default image for all profiles)
egress   = "locked"
network  = "a2a-verify-egress"
proxy    = "http://a2a-verify-proxy:8888"
no_proxy = "localhost,127.0.0.1"
# (no `commands` here anymore — they live in [[languages]])

[[languages]]
id      = "rust"                    # matches Lang::as_str() from lsp_mcp's detect_lang
warm    = "cargo fetch --locked"
warm_env   = { CARGO_HOME = "/cargo", CARGO_NET_OFFLINE = "true" }
warm_cache = "a2a-impl-lsp-cache"
# image   = "..."                   # OPTIONAL per-profile image override; defaults to [verify].image
[[languages.verify]]
name = "fmt";    cmd = "cargo fmt --all -- --check"
[[languages.verify]]
name = "clippy"; cmd = "cargo clippy --all-targets --all-features --locked -- -D warnings"
[[languages.verify]]
name = "build";  cmd = "cargo build --locked"
[[languages.verify]]
name = "test";   cmd = "cargo test --workspace --locked --exclude bridge-container -- --skip process::tests::terminate_reaps_child_no_zombie --skip process::tests::term_ignoring_loop_forces_group_sigkill --skip process::tests::drop_group_kills_descendants"

[[languages]]
id      = "go"                      # matches Lang::as_str() == "go"
warm    = "go mod download all"
warm_env   = { GOMODCACHE = "/gomodcache", GOFLAGS = "-mod=mod" }
warm_cache = "a2a-impl-go-cache"
[[languages.verify]]
name = "build"; cmd = "go build ./..."
[[languages.verify]]
name = "vet";   cmd = "go vet ./..."
[[languages.verify]]
name = "test";  cmd = "go test ./..."
[[languages.verify]]
name = "fmt";   cmd = "test -z \"$(gofmt -l .)\""
```

**Profile fields:** `id` (matches `lsp_mcp::lang::Lang::as_str()` from `detect_lang` — the single detection source; no config-side marker list), `warm` + `warm_env` + `warm_cache` (the warm-deps command, its env, and the per-repo cache volume base), an OPTIONAL per-profile `image` (default `[verify].image`), and `[[languages.verify]]` (the ordered verify commands, each `{name, cmd}` — same shape as today's `[verify].commands`, optional per-command `gate`). An `id` that no `detect_lang` result can produce is dead config (a parse-time warning is reasonable).

### §2.1 No backward-compat (explicit migration)

`[verify].commands` is **removed**, not kept as a fallback. A detected language with **no matching `[[languages]]` profile → skip warm + verify (reported, non-gating)** — NOT an implicit Rust default. The shipped example configs (`a2a-bridge.containerized.toml`, `a2a-bridge.containerized.podman.toml`) are **migrated**: their existing `[verify].commands` (Rust) become a `[[languages]] id="rust"` profile, and a `[[languages]] id="go"` profile is added. (The owner approved removing backward-compat; the only consumers are these example configs, migrated in this slice.)

## §3. C2a — `implement` (single-language-per-cwd)

The first buildable slice — the containerized `implement` loop on a Go repo.

- **Combined image:** augment the `a2a-toolchain` image (Dockerfile) with `go` + `gopls` + (already-present) the Linux `lsp-mcp`/rust-analyzer, so one image serves both languages. `[verify].image` points at it.
- **impl lsp `--lang auto`:** flip the impl agent's in-container lsp `args` from `--lang rust` to `--lang auto` (config). Its MCP `env` carries BOTH cache envs (`CARGO_HOME=/cargo` + `GOMODCACHE=/gomodcache`, plus the offline flags) — each language server reads its own; the cross-language env is inert (rust-analyzer ignores `GOMODCACHE`, gopls ignores `CARGO_HOME`).
- **Language-aware warm-deps:** `compose_warm_fetch` (and `warm_lsp_deps_step`) detect the clone's language → run the selected profile's `warm` command with `warm_env`, mounting the profile's `warm_cache` volume. (Replaces the hard-coded `cargo fetch --locked` / `CARGO_HOME=/cargo`.)
- **Language-aware verify:** the verify runner (`run_verify` + the `main.rs` verify step) selects the profile by detected language and runs its `[[languages.verify]]` commands in the profile image. Unmatched language → skip (reported).
- **Detection:** `detect_repo_langs(clone)` (the multi-root-ready seam) wraps single-root `detect_lang` for C2a and returns 0 or 1 `LangRoot`. Exactly one → select its profile. Zero (no root marker — e.g. a monorepo root whose languages live in subdirs → `detect_lang`'s "cannot detect") or `detect_lang`-ambiguous (multiple markers at the bare root) → surface the error, guiding the operator to narrow `--repo` to a single-language service subdir. True multi-root subdir detection is C2c.

**DoD:** `a2a-bridge implement <task> --repo <go-repo-or-go-service-subdir>` converges like the Rust dogfood — containerized codex edits Go → verify (`go build`/`go test`/`go vet`/`gofmt`) → review-the-diff → commit/amend → hand-off. The **Rust** `implement` path stays green (the migrated rust profile reproduces today's behavior byte-for-byte).

## §4. C2b — polyglot `serve` / `run-workflow`

The per-turn `ContainerRw` path under `serve`/`run-workflow` already runs the `impl` agent per turn in a container; C2a's foundation (combined image + lsp `--lang auto`) makes a **Go session** work with no further loop code — the agent edits + navigates Go. There is **no verify/review** here (those are `implement`-only).

- **One serve, mixed-language:** because the impl lsp is `--lang auto` and the image is combined, a `run-workflow … --session-cwd <go-service>` request navigates/edits Go, while the same serve on `--session-cwd <rust-service>` does Rust — each session single-language by its cwd.
- **Design point to settle:** whether the per-turn path **pre-warms** per session (an analogue of `warm_lsp_deps_step` keyed on the session repo) or the in-container lsp resolves lazily / from a mounted per-repo cache if present. Default lean: **no per-turn pre-warm** (warm-deps stays `implement`-specific); the in-container gopls/RA resolve from the combined image's toolchain + any present per-repo cache, degrading to workspace-only nav if a third-party cache is cold (same posture as a cold host lsp). Revisit only if cold third-party nav proves painful.

**DoD:** a single `serve` (or two `run-workflow` invocations against one config) handles a Go-service session AND a Rust-service session — the impl container nav/edits each correctly; `docker events`/the lsp call log shows `lang=go` vs `lang=rust` per session.

## §5. C2c — multi-language-within-one-cwd (DEFERRED, designed-for)

The richer "cover multiple services in one run" value. Out of scope here, but the seam is built to extend:

- `detect_repo_langs` already returns the FULL set of language-roots → C2c consumes all of them.
- **Per-service verify:** after the agent commits, map the diff's changed files (`git diff --name-only`) to their nearest language-root, and run each touched root's profile verify in that subdir.
- **Multi-root LSP nav:** the hard part — `lsp-mcp --lang auto` is single-root and **refuses** multi-marker roots; subdir-rooted/multi-root nav (`--project-root`, multiple concurrent language servers under one repo) is the lsp-mcp deferral this slice would fold in.
- **Warm-deps:** warm each detected language's cache.

C2a/C2b do NOT implement any of this; they only ensure `detect_repo_langs` returns a set (not a scalar) and that profile selection is keyed by language id, so C2c is additive.

## §6. Image strategy

C2a ships ONE **combined** Rust+Go image (`a2a-toolchain`) as `[verify].image`, the default for all profiles — simplest for polyglot serve (one container image serves any session language). The profile's OPTIONAL `image` override is the designed-for seam for the owner's likely follow-on: **separate** per-language images (a rust image, a go image, a rust+go image, + future Python and js/ts combinations) chosen by memory/footprint — config-only, no bridge change. (The combined image grows the toolchain footprint; separate images trade image count for per-container size. C2a defers that tuning.)

## §7. Tests / regression

- **Pure / hermetic:** the detection wrapper over `detect_lang` (`Cargo.toml`→rust, `go.mod`→go, none→cannot-detect/skip, ambiguous→refuse) + profile selection (match by `id`; unmatched → skip) + the `compose_warm_fetch`/verify-command composition reading a profile (no real container). These are the coverage keystones — the container-spawning paths can't run in the hermetic unit suite. (Multi-root detection tests are C2c.)
- **Regression:** the **Rust** `implement` + verify path stays green via the migrated `id="rust"` profile (run an existing-style Rust implement gate; confirm the verify commands + warm are byte-identical to today). The lsp-mcp suites are untouched.
- **Go live gate (C2a DoD):** a containerized `implement` on a Go repo converges with `go build`/`go test` verify. **Serve live gate (C2b DoD):** one serve handles a Go session + a Rust session.
- **Config:** the migrated example configs parse (a parse test pinning the rust+go profiles + the removed `[verify].commands`).

## §8. Execution order

1. **C2a:** combined image (Dockerfile) → `[[languages]]` profile schema + parse (remove `[verify].commands`) → `detect_repo_langs` (multi-root-ready) + selection → language-aware `compose_warm_fetch`/warm-deps → language-aware verify runner → impl lsp `--lang auto` → migrate example configs → Go `implement` live gate + Rust regression.
2. **C2b:** confirm the per-turn `ContainerRw` path inherits the combined image + `--lang auto` (mostly free) → settle the per-turn pre-warm design point → serve polyglot live gate.
3. **C2c:** deferred (its own spec when triggered).

Each of C2a/C2b is its own plan → subagent-driven implementation → review, dogfooded on the bridge (containerized codex implementor + gpt-5.5 reviewer), following the Go-increment playbook.

---

## Deferred (its own design pass when triggered)

- **C2c — multi-language-within-one-cwd** (§5): multi-root detection + per-service verify + multi-root LSP nav (folds in the lsp-mcp `--project-root`/subdir-rooted deferral).
- **Separate per-language images** (§6): config-only via the profile `image` override; tuned by memory/footprint when the combined image gets heavy.
- **TypeScript/JS + Python in-container:** later profiles + image additions onto this exact seam.
- **Per-turn serve pre-warm** (§4 design point): only if cold third-party nav under `serve` proves painful.
