# LSP-over-MCP semantic nav — Slice B (in-container implementor) — design

**Date:** 2026-06-14
**Status:** Approved (brainstorming) — revised per codex gpt-5.5 xhigh spec review — ready for plan
**Predecessor:** `2026-06-13-lsp-mcp-nav-design.md` (Slice A — host-side reviewers — shipped `main` `112648e`)

## Goal

Give the bridge's containerized **`:rw` implementor agent** (codex) live, type-resolved LSP navigation *while it edits* — the same 7 `lsp` tools the host-side reviewers got in Slice A, but running **inside** the egress-locked impl container, with rust-analyzer reflecting the agent's own on-disk edits in real time.

## Context

Slice A wired `lsp` (a shim wrapping rust-analyzer) to the **host-side** reviewers via `[[agents.mcp]]`. Slice B brings the same shim to the **impl agent**, which runs in an egress-locked container (`a2a-toolchain`, mount = the clone, `network=a2a-egress-internal`, codex creds only). The macOS/OrbStack UDS boundary (Slice A spike Q3) rules out reaching the host shim from inside the container, so the impl agent gets a **co-located** shim + rust-analyzer baked into its image. The impl agent already runs **warm** across the edit→fix turns of a run (one container + one ACP session, ADR-0024), so a co-located shim is warm across turns for free.

## Empirical findings (spikes, 2026-06-14)

Both load-bearing unknowns were validated before committing:

| Spike | Result |
|---|---|
| **rust-analyzer runs in `a2a-toolchain`** | ✅ `rustup component add rust-analyzer` works (component `rust-analyzer-aarch64-unknown-linux-gnu` is available; base = Debian 12). On a mounted a2a-bridge clone: **Database loaded 6.64s** (≈ host's ~9s cold), peak RSS **3039 MB**. |
| **RA sees the agent's in-container edits** | ✅ inotify **fires** for writes made *inside* the container to the virtiofs bind-mount (`EVENT IN_CREATE`). RA's `notify`-based server-side watcher uses inotify → it will reflect the agent's edits. **Live nav works.** |

The in-container index *needed egress* to resolve deps into the cold `~/.cargo` — the production impl container is egress-locked, so **dep sources must be provided offline** (§2). And **~3 GB RA per impl container** (Memory, below).

The codex review additionally **confirmed in-code**: CodexNative `{cwd}` for `container_rw` resolves to the canonical clone (`bridge-container/src/lib.rs:232`) — so the impl agent's `lsp --repo` is *correct* (unlike the deferred host-reviewer asymmetry); and the shim's `initialize` omits client `didChangeWatchedFiles` registration (`lsp/mod.rs:91`) — so the server-side-watcher premise holds.

## Architecture

### 1. Image — bake RA + the Linux `lsp-mcp` into `a2a-toolchain`

- `rustup component add rust-analyzer` (the working RA — the prior image shipped only a non-functional rustup *proxy*).
- The **Linux `lsp-mcp` binary** at a fixed path (`/usr/local/bin/lsp-mcp`).

**Build context (codex #5).** The toolchain image is currently built from the `deploy/containers` context, from which a build stage *cannot* `cargo build -p lsp-mcp` (no workspace). Fix: build the toolchain image from the **repo-root context** with a `.dockerignore` (so a multi-stage build can compile `lsp-mcp` from the workspace and `COPY` the binary into the final stage), and update the Docker/Podman runbook (`docs/containerized-agents.md`). Alternative if repo-root context is undesirable: a separate `cargo build --release -p lsp-mcp` step that emits the Linux binary, `COPY`'d in — the plan picks one; repo-root multi-stage is the default.

### 2. Dep-access — a **separate**, runtime-wired, **read-only** dep-source cache (Option B, corrected)

The egress-locked impl container's RA gets dependency **sources** from a **dedicated, bridge-managed cargo cache** — **not** verify's cache. This corrects two codex BLOCKERs:
- **#1:** verify's cache volume name is computed at **runtime** (`<base>-<hash(canonical source)>`, `verify.rs:124`) and can't be expressed as a static `[agents.sandbox].volumes` mount. So the impl-lsp cache is wired at **runtime in the `implement` flow** (its own `cache_volume_name`-style per-repo derivation, a *distinct* volume e.g. `a2a-impl-lsp-cache-<hash>`), not static config.
- **#2:** sharing verify's **mutable** cache into the **creds-bearing** impl container is wrong — the agent could poison verify's trusted inputs, and RA's proc-macro/build-script execution would mix with creds. A **separate** cache removes the poisoning vector entirely (verify's cache is untouched).

**Warming — a new `warm_lsp_deps` phase (codex #3).** There is no pre-edit phase today (fresh `implement` gates `[verify]` then builds the warm impl backend at `main.rs:1507/1570`; verify is post-commit). So Slice B **adds an explicit `warm_lsp_deps` phase** to both the fresh and resume paths (`main.rs` ~1507 / ~1799), **before** `build_warm_impl`. It runs `cargo fetch --locked` (+ whatever the §Open spike proves is needed to fully extract sources) on the clone in a **no-creds, registries-only egress** container — reusing `compose_verify`/egress *mechanics* (`a2a-verify-egress`/proxy) but as its own phase, populating the dedicated impl-lsp cache. This eliminates the cold-cache chicken/egg (the impl RA runs before this attempt's verify).

**Offline + env (codex #4).** `SandboxToml` has no `env` field, and `compose_sandbox` only injects proxy env — so `CARGO_HOME`/`CARGO_NET_OFFLINE` go through **`[[agents.mcp.env]]`** on the `lsp` server (which CodexNative already renders into the codex-acp argv): `CARGO_HOME=<cache mount>`, `CARGO_NET_OFFLINE=true`. `lsp-mcp` already sets `CARGO_TARGET_DIR` for its RA child (`lsp/mod.rs:42`); the cargo-home/offline come from the MCP env.

**Read-only vs writable** is decided by the first plan spike (§Open / codex #7): prefer the cache mounted **read-only** (the `warm_lsp_deps` phase pre-extracts everything RA needs); fall back to a **writable own cache** only if RA provably needs to write cargo-home (src unpacking, locks, git checkouts) — and even then the poisoning risk is contained because it's the impl run's *own* per-repo cache, never verify's.

**Dependency:** a config **without a `[verify]` block** (so no egress/proxy to reuse) gets **no warm step** → RA indexes workspace-only (degraded, not broken). Emit a warning at config build when `lsp` is on the impl agent without `[verify]`.

### 3. Target cache — per-repo Linux volume (cross-container only)

RA's `CARGO_TARGET_DIR` → a **Linux docker volume** (`a2a-impl-lsp-target`), per-repo keyed (Slice A's origin-hash scheme). It **cannot** share the host shim's `~/.local/share/a2a/lsp-target-cache` (container Linux/aarch64 vs host macOS/aarch64 → incompatible rustc fingerprints). It **can** share cross-impl-container for warm reuse.

### 4. Wiring — `lsp` on the impl agent (CodexNative)

Add `[[agents.mcp]] lsp` to the **impl** agent in `examples/a2a-bridge.containerized.toml` (+ `.podman.toml`):
- `command` = `/usr/local/bin/lsp-mcp` (baked, in-container).
- `args` = `["--repo", "{cwd}", "--lang", "rust", "--target-cache", "<container target mount>"]`. `{cwd}` → the clone (confirmed correct for `container_rw`).
- `[[agents.mcp.env]]` = `CARGO_HOME`, `CARGO_NET_OFFLINE=true`, and **`LSP_MCP_LOG`** (codex #8 — see Observability).
- The dedicated cache + target volume mounts are added at **runtime** in the `implement` flow (not static config, per #1).

The host reviewers' Slice A wiring is unchanged. Only the impl agent gains in-container `lsp`. **Zero `lsp-mcp` source change** for the wiring; the only `lsp-mcp` change is idle-evict (§6).

### 5. Edit-sync — RA's server-side fs-watcher (no shim change)

The shim omits client `didChangeWatchedFiles` (confirmed), so RA uses its own `notify`/inotify watcher (spike-validated for in-container edits). The agent's edits surface in nav (~50 ms incremental, Slice A Q4). No `lsp-mcp` change.

**Dep-change restart (codex #6) — DEFERRED (NOT shipped in Slice B; follow-up).** Original intent: a running RA is **not** guaranteed to pick up a cargo-home/`Cargo.lock` change, so when `Cargo.toml`/`Cargo.lock` changed mid-edit, restart the RA child before the next turn, riding the §6 idle-evict/respawn machinery. **Why deferred:** the shipped `/cargo` dep cache is **READ-ONLY + `CARGO_NET_OFFLINE=true`** (Task 1 offline proof), so a dep ADDED mid-edit isn't in the cache and a bare RA restart can't resolve it — a correct dep-change restart also needs a **mid-edit `warm_lsp_deps` re-fetch** (writable/re-mounted cache), a separate increment. The idle-evict machinery (§6) IS shipped; only the lock-change *trigger* + re-fetch are deferred. Until then a dep added mid-run stays unresolved for that run (low frequency; the next run's `warm_lsp_deps` picks it up).

### 6. Lifecycle — warmth + **idle-evict** (the memory lever)

- **Warm within a run (free):** `lsp-mcp` is session-scoped; the impl container + ACP session are warm across edit→fix turns (ADR-0024). RA indexes **once** (~7 s, absorbed by Slice A's lazy handshake) and stays warm.
- **Idle-evict (new `lsp-mcp` feature):** `lsp-mcp` tracks last-tool-call time; after an idle timeout it **kills the RA child** (frees ~3 GB) and resets `readied=false`. The next tool call **lazily respawns** RA (~0.7 s warm off the shared target — reusing the FU2 `ensure_ready` path). Because the impl agent is **idle during the review phase**, its RA is evicted there automatically (Memory ladder, below). `lsp-mcp` *owns* its RA, so this lives in `lsp-mcp` — the bridge can't cleanly kill codex's MCP subprocess. (The dep-change restart §5 would reuse this kill+respawn, but is DEFERRED — see §5.)

## Memory

The dominant cost is **one RA ≈ 3 GB heap** (per process; not reducible by sharing a *disk* cache — the heap is rebuilt in each process). The codex revisions are correctness/isolation, **~zero RAM delta** (the separate cache is disk + reclaimable page cache; `warm_lsp_deps` is a transient pre-RA fetch container).

Review-phase peak ladder:

| | Review-phase resident RA |
|---|---|
| Slice B as first specced | impl RA (3 GB, held idle) + 2 host reviewer RAs (6 GB) = **~9 GB** |
| **+ idle-evict (this spec)** | impl RA **evicted while idle** + 2 host RAs = **~6 GB** (= the Slice A baseline) |
| + shared host-reviewer RA (deferred, see Non-goals) | one shared host RA = **~3 GB** |

Idle-evict drops the impl RA's contribution during review; the two host reviewer RAs are *actively querying* then, so only the shared-daemon change shrinks those.

## Non-goals

- **Slice C** — other languages (gopls/pyright/tsserver).
- **Shared host-reviewer `lsp` daemon** — one RA serving both host reviewers of the same clone (6 GB → 3 GB during review). Feasible host-side (no OrbStack boundary) but a real Slice A architecture change (MCP-over-socket daemon + lifecycle, the deferred "gateway" model). Its own follow-up.
- **Cross-run warm pool** — pre-warmed RA for the *next* `implement`. High LOE for ~7 s saved once/run while holding 3 GB idle; within-run warmth is free.
- The deferred Slice A follow-ups (codex host-reviewer `{cwd}` asymmetry; `experimental/serverStatus` readiness signal).
- `lsp` for the `:ro` reader agents (kiro) — only the `:rw` impl agent.

## Error handling & edge cases

- **No `[verify]` block** → no `warm_lsp_deps` → RA indexes workspace-only (degraded nav). Warn at config build.
- **`warm_lsp_deps` fetch fails** (egress misconfig) → RA degrades to workspace-only; the edit still proceeds (the agent has shell/file tools).
- **RA fails to spawn / crashes in-container** → the shim returns a structured tool error; the impl agent degrades to its normal tools. Never blocks the edit.
- **Dep-bump mid-edit** → the §5 re-fetch+restart is DEFERRED, so a dep added mid-run stays unresolved for that run; the next run's `warm_lsp_deps` picks it up (low frequency).
- **Observability under `--rm` (codex #8)** → the default `lsp-mcp-calls.log` (`/root/.local/share/...`) vanishes when the container is reaped. Set `LSP_MCP_LOG` (via `[[agents.mcp.env]]`) to a path **under the clone** (which the bridge fetches at hand-off) so the live gate can read it.
- **Memory** → ~3 GB RA per impl container; idle-evict bounds the review-phase peak (Memory). The binding constraint is concurrent RA processes.
- **Isolation note** → RA runs build-scripts/proc-macros (dependency code) in the creds-bearing impl container. This crosses **no new boundary**: the impl container is already `danger-full-access` with creds where the agent can run arbitrary code (incl. `cargo`); network egress is locked; the separate read-only cache removes the cache-poisoning vector; and the bridge commits only the agent-staged index (a proc-macro can't stage an exfil file). So the residual ("trusted-dep proc-macros execute with creds present") matches the existing impl posture — **no sidecar, no disabling proc-macros** (which would gut nav on this proc-macro-heavy repo).

## Testing & DoD

- **First plan task = the offline-completeness proof (codex #7).** Run `warm_lsp_deps`, then a **fresh no-egress, no-host-cache** RA container with *only* the intended mounts/env (the dedicated cache + target volume + `CARGO_HOME`/`CARGO_NET_OFFLINE`), and confirm registry crates **and git deps** resolve and index. This decides read-only vs writable cache (§2) before any wiring is built.
- **Image build** verifies `rust-analyzer --version` works and `/usr/local/bin/lsp-mcp` runs in the built image.
- **`lsp-mcp` idle-evict** gets unit coverage where pure (the idle/respawn decision) + an integration check that a query after eviction re-indexes and returns correct results.
- **Live dogfood gate:** a real `a2a-bridge implement`, confirming via the `LSP_MCP_LOG` written **under the clone** that the impl agent **issued an `lsp` tool call** during the edit, that RA **reflected a mid-session edit**, and (memory) that the impl RA was **evicted during the review phase** (no 3 GB impl RA resident then).

## Open questions / risks (for the plan)

- **Offline completeness (#7)** — the keystone spike above; does `cargo fetch --locked` + the chosen mounts give RA *everything* offline (registry index, `src` extraction, git deps, any writable cargo-home need)?
- **`warm_lsp_deps` placement** — the exact insertion point in the fresh + resume `implement` paths, and graceful degrade without `[verify]`.
- **Idle-evict timeout** — pick a default (e.g. 60 s) that evicts during review without thrashing within an active edit; make it configurable.
- **Container mount paths** — fixed in-container paths for the cache + target volumes, keyed per-repo like Slice A's `cache_dir`.
