# LSP-over-MCP semantic nav — Slice B (in-container implementor) — design

**Date:** 2026-06-14
**Status:** Approved (brainstorming) — ready for plan
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

Two constraints the spikes pinned down: **(1)** the in-container index *needed egress* to resolve deps into the cold `~/.cargo` — the production impl container is egress-locked, so **dep sources must be provided offline** (see Architecture §2). **(2)** ~3 GB RA per impl container.

## Architecture

### 1. Image — bake RA + the Linux `lsp-mcp` into `a2a-toolchain`

The toolchain Dockerfile (`deploy/containers/`) gains:
- `rustup component add rust-analyzer` (the working RA — note the prior image shipped only a non-functional rustup *proxy*).
- The **Linux `lsp-mcp` binary**, built in a Docker build stage from the workspace (`cargo build --release -p lsp-mcp`) and `COPY`'d into the final image at a fixed path (e.g. `/usr/local/bin/lsp-mcp`).

The impl agent and verify share this image, so it's one Dockerfile change. (Slice A's host shim is a separate macOS binary; this is the Linux build of the same crate — no source changes to `lsp-mcp` are expected.)

### 2. Dep-access — bridge-managed cache volume, warmed via `[verify]` (Option B)

The egress-locked impl container's RA gets dependency **sources** from a **bridge-managed cargo-registry cache volume** mounted into the container; RA/cargo run **offline** (`CARGO_NET_OFFLINE=true`). This keeps the impl box sealed (no host-fs mount) and portable, consistent with how verify already gives a container cargo access.

**Warming reuses the `[verify]` infrastructure.** Before the impl agent's RA needs deps, a `cargo fetch --locked` runs on the clone through the **existing registries-only egress** (`a2a-verify-egress` / `a2a-verify-proxy`) into the **existing `a2a-verify-cache`** volume. So:
- The cache volume the impl RA reads **is** `a2a-verify-cache` (already per-repo keyed, already populated by verify runs).
- The warm step is a `cargo fetch` using the `[verify]` block's `egress`/`network`/`proxy`/`cache`, run as a **pre-edit step** in the `implement` flow (idempotent — fast when warm, ~minutes only on a truly cold cache). This eliminates the cold-cache chicken/egg (the impl RA runs *before* this attempt's verify, so it can't rely on this attempt's verify to warm the cache).

**Dependency:** a config **without a `[verify]` block** gets **no warm step**, so the impl RA resolves only workspace crates (no external-dep type resolution) — **degraded, not broken** nav. The dogfood configs have `[verify]`. The wiring must surface this clearly (a warning when `lsp` is on the impl agent but `[verify]` is absent).

### 3. Target cache — per-repo Linux volume (cross-container only)

The impl RA's `CARGO_TARGET_DIR` points at a **Linux docker volume** (e.g. `a2a-impl-lsp-target`), per-repo keyed (same origin-hash scheme as Slice A's `cache_dir`). This **cannot** be shared with the host shim's `~/.local/share/a2a/lsp-target-cache` — the container is Linux/aarch64, the host is macOS/aarch64, so the rustc fingerprints differ and the build artifacts are incompatible. It **can** be shared cross-impl-container (all Linux), for warm reuse across runs.

### 4. Wiring — `lsp` on the impl agent (CodexNative)

Add `[[agents.mcp]] lsp` to the **impl** agent in `examples/a2a-bridge.containerized.toml` (+ `.podman.toml`):
- `command` = the baked in-container path (`/usr/local/bin/lsp-mcp`).
- `args` = `["--repo", "{cwd}", "--lang", "rust", "--target-cache", "<container target-volume path>"]`. `{cwd}` resolves to the clone inside the container (the impl agent's session cwd; already correct for the impl agent, unlike the deferred host-reviewer asymmetry).
- Delivery is `CodexNative` (`-c mcp_servers.lsp.*`, baked into the codex-acp argv) — the impl agent is codex; the args resolve inside the container.
- The mounts (cache volume, target volume, `CARGO_NET_OFFLINE`) are added to the impl agent's `[agents.sandbox]` (or carried by the lsp config). **Zero `lsp-mcp` source changes**; this is image + config.

The host reviewers' Slice A wiring is unchanged. Only the impl agent gains in-container `lsp`.

### 5. Edit-sync — RA's server-side fs-watcher (no shim change)

The shim's `initialize` does **not** advertise client `didChangeWatchedFiles` dynamic registration, so RA falls back to its **own** `notify`/inotify server-side watcher (spike-validated to fire for in-container edits). When the agent writes a file via its tools, RA picks it up and incrementally re-analyzes (~50 ms, Slice A spike Q4). **No `lsp-mcp` change** — the shim already triggers server-side watching.

### 6. Warmth — free within a run

`lsp-mcp` is session-scoped; the impl agent's container + ACP session are warm across the edit→fix turns (ADR-0024). So RA indexes **once** per run (~7 s, absorbed by Slice A's lazy handshake) and stays warm for every subsequent turn/nav call. No warm pool.

## Non-goals

- **Slice C** — other languages (gopls/pyright/tsserver).
- **Cross-run warm pool** — pre-warmed RA standing by for the *next* `implement`. High LOE (pool manager + lifecycle + eviction) for ~7 s saved once per run while holding ~3 GB idle. Within-run warmth is already free (§6).
- The deferred Slice A follow-ups (codex host-reviewer `{cwd}` asymmetry; the `experimental/serverStatus` readiness signal).
- Giving `lsp` to the `:ro` reader agents (kiro) — only the `:rw` impl agent.

## Error handling & edge cases

- **No `[verify]` block** → no warm step → RA indexes workspace-only (degraded nav). Emit a warning at config build when `lsp` is on the impl agent without `[verify]`.
- **Cold cache** (first run, never verified) → the warm `cargo fetch` populates it (~minutes once); subsequent runs are warm. If `cargo fetch` fails (e.g. egress misconfig), RA degrades to workspace-only; the edit still proceeds (the agent has shell/file tools).
- **RA fails to spawn / crashes in-container** → the shim returns a structured tool error; the impl agent degrades to its normal tools. Never blocks the edit.
- **Memory** → ~3 GB RA per impl container, on top of codex. Concurrent impl runs each add ~3 GB; document it. The binding constraint is concurrent RA processes, not dep-access.
- **`CARGO_NET_OFFLINE` + a dep-bump mid-clone** → if the clone's Cargo.lock references a crate not in the warmed cache, RA can't resolve it offline → that crate's types degrade. The pre-edit `cargo fetch --locked` covers the lock's deps, so this only bites if the lock changes *during* the edit (rare; re-warm on the next attempt).

## Testing & DoD

- **Image build** verifies `rust-analyzer --version` succeeds and `/usr/local/bin/lsp-mcp` runs in the built image.
- **Live dogfood gate** (the bridge validating itself, as every slice has): run a real `a2a-bridge implement` on a repo, and confirm — via the **`lsp-mcp-calls.log`** observability added in Slice A (now written *inside* the impl container; mount or `docker logs` it out) — that the impl agent **issued at least one `lsp` tool call** during the edit, and that RA **reflected an edit** made mid-session (a nav query before vs after the agent changes a `pub` item). The per-repo target volume + `a2a-verify-cache` are confirmed populated.
- The hermetic-safe `lsp-mcp` unit/integration tests are unchanged (Slice B is image + config + a warm step; no `lsp-mcp` source change).

## Open questions / risks

- **`cargo fetch` completeness** — does `cargo fetch --locked` populate everything RA needs offline (registry index + `src` extraction + git deps)? Validate in the plan's first task; if `git` deps need `~/.cargo/git`, ensure the warm step + cache volume cover it.
- **Where the warm step lives** — a pre-edit phase in `implement` reusing the `[verify]` config. Confirm it composes with the existing `implement` flow without a `[verify]`-required hard error (degrade gracefully).
- **Container target-volume path** — pick a fixed in-container mount (e.g. `/lsp-target`) and key the per-repo subdir the same way Slice A's `cache_dir` does.
