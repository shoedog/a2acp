# LSP-over-MCP Slice B (in-container implementor nav) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give the containerized `:rw` impl agent (codex) live, type-resolved `lsp` nav while it edits — rust-analyzer + the Linux `lsp-mcp` baked into `a2a-toolchain`, dep sources from a separate runtime-wired read-only cache warmed via the verify egress, with `lsp-mcp` idle-eviction bounding the review-phase memory peak.

**Architecture:** Per `docs/superpowers/specs/2026-06-14-lsp-mcp-slice-b-design.md`. Image bake + config wiring (no `lsp-mcp` source change except idle-evict) + a new `warm_lsp_deps` phase in `implement` + a separate per-repo dep cache (reusing `verify::cache_volume_name` with a distinct base). The offline-completeness proof is **Task 1** — it decides read-only vs writable cache before any wiring is built.

**Tech Stack:** Rust (workspace, 1.94.0), Docker/OrbStack + the `a2a-toolchain` image, rust-analyzer 1.94.0, the existing `verify`/`compose_verify` egress+cache machinery, `lsp-mcp` (Slice A).

---

## File structure

```
deploy/containers/toolchain.Containerfile   # MODIFY: + rust-analyzer component; + lsp-mcp build stage (repo-root ctx)
.dockerignore                               # CREATE (repo root): exclude target/, .git for the build stage
docs/containerized-agents.md                # MODIFY: toolchain build now uses repo-root context
crates/lsp-mcp/src/lsp/mod.rs               # MODIFY: idle-evict (store repo/target, track activity, kill+respawn)
crates/lsp-mcp/src/mcp/mod.rs               # MODIFY: touch activity on each tools/call; call ensure_ready (already there)
bin/a2a-bridge/src/implement.rs             # MODIFY (or main.rs): warm_lsp_deps phase + impl-lsp cache wiring
bin/a2a-bridge/src/verify.rs                # REUSE: cache_volume_name (distinct base); maybe a compose helper
examples/a2a-bridge.containerized.toml      # MODIFY: [[agents.mcp]] lsp + [[agents.mcp.env]] on the impl agent
examples/a2a-bridge.containerized.podman.toml  # MODIFY: mirror
docs/superpowers/spikes/2026-06-14-slice-b-offline-proof.md  # CREATE (Task 1 record)
```

---

## Task 1: Offline-completeness proof spike (decides read-only vs writable cache)

**This is a validation task, not TDD.** It proves RA can index a deps-heavy repo fully offline with only the intended mounts/env, and decides whether the cache must be writable. **No production code is written until this passes.**

**Files:**
- Create: `docs/superpowers/spikes/2026-06-14-slice-b-offline-proof.md` (record the result)

- [ ] **Step 1: Build a throwaway RA image with the component**

```bash
docker build -t a2a-ra-spike - <<'DOCKER'
FROM a2a-toolchain:latest
RUN rustup component add rust-analyzer
DOCKER
```
Expected: builds; image has a working `rust-analyzer`.

- [ ] **Step 2: Warm a dedicated cache with a no-creds, registries-egress fetch**

```bash
rm -rf /tmp/sb-clone /tmp/sb-cargo; git clone --no-hardlinks /Users/wesleyjinks/code/a2a-bridge /tmp/sb-clone
# fetch deps into a dedicated CARGO_HOME with egress (mirrors warm_lsp_deps); a2a-verify-egress allows registries
docker run --rm --network a2a-verify-egress -e HTTPS_PROXY=http://a2a-verify-proxy:8888 -e HTTP_PROXY=http://a2a-verify-proxy:8888 \
  -e CARGO_HOME=/cargo -v /tmp/sb-clone:/work -v /tmp/sb-cargo:/cargo \
  --entrypoint bash a2a-ra-spike -c 'cd /work && cargo fetch --locked'
```
Expected: `cargo fetch` succeeds (all registry + any git deps downloaded into `/tmp/sb-cargo`).

- [ ] **Step 3: Index OFFLINE with the cache mounted READ-ONLY, no egress, no host cache**

```bash
rm -rf /tmp/sb-target; mkdir /tmp/sb-target
docker run --rm --network a2a-egress-internal \
  -e CARGO_HOME=/cargo -e CARGO_NET_OFFLINE=true -e CARGO_TARGET_DIR=/target \
  -v /tmp/sb-clone:/work -v /tmp/sb-cargo:/cargo:ro -v /tmp/sb-target:/target \
  --entrypoint bash a2a-ra-spike -c 'cd /work && rust-analyzer analysis-stats . 2>&1 | grep -aE "Database loaded|Total:|crates:|error|panicked"'
```
Expected: **PASS path** = `Database loaded` + a crate count comparable to host (~30), no "error"/"failed to resolve" noise → **read-only cache works**. **FAIL path** = resolution errors / RA wants to write `/cargo` → re-run Step 3 with `-v /tmp/sb-cargo:/cargo` (writable) and record that the cache must be writable (the impl-lsp cache is then writable-but-separate-from-verify; the spec's contained-poisoning note applies).

- [ ] **Step 4: Record the decision**

Write `docs/superpowers/spikes/2026-06-14-slice-b-offline-proof.md` with: the exact mounts/env that worked, read-only vs writable verdict, index time + RSS, and any git-dep caveat. **This verdict drives Tasks 5–6.**

- [ ] **Step 5: Clean up + commit the record**

```bash
docker rmi a2a-ra-spike; rm -rf /tmp/sb-clone /tmp/sb-cargo /tmp/sb-target
git add docs/superpowers/spikes/2026-06-14-slice-b-offline-proof.md
git commit -m "spike(slice-b): offline RA indexing proof — read-only vs writable cache decision"
```

---

## Task 2: Bake the rust-analyzer component into the toolchain image

**Files:**
- Modify: `deploy/containers/toolchain.Containerfile`

- [ ] **Step 1: Add the component to the rustup install line**

In the `curl ... rustup.sh` block, add `--component rust-analyzer`:
```dockerfile
RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \
      | sh -s -- -y --no-modify-path --default-toolchain 1.94.0 --profile minimal \
        --component rustfmt --component clippy --component llvm-tools-preview --component rust-analyzer
```

- [ ] **Step 2: Build and verify the component works**

```bash
docker build -t a2a-toolchain:latest -f deploy/containers/toolchain.Containerfile deploy/containers
docker run --rm --entrypoint rust-analyzer a2a-toolchain:latest --version
```
Expected: `rust-analyzer 1.94.0 ...` (a real version, NOT "Unknown binary" — the prior proxy bug).

- [ ] **Step 3: Commit**

```bash
git add deploy/containers/toolchain.Containerfile
git commit -m "feat(toolchain): bake the working rust-analyzer component (was a non-functional proxy)"
```

---

## Task 3: Bake the Linux `lsp-mcp` binary (repo-root build stage)

**Files:**
- Modify: `deploy/containers/toolchain.Containerfile`
- Create: `.dockerignore`
- Modify: `docs/containerized-agents.md` (build-context note)

- [ ] **Step 1: Create `.dockerignore` (repo root)**

```
target/
.git/
.a2a-implement/
docs/
**/node_modules/
```

- [ ] **Step 2: Add a build stage + COPY to the Containerfile**

Prepend a builder stage and copy the binary into the final image:
```dockerfile
# Builder: compile the Linux lsp-mcp from the workspace (requires repo-root build context).
FROM a2a-agent-reader:latest AS lspbuild
ENV RUSTUP_HOME=/usr/local/rustup CARGO_HOME=/usr/local/cargo PATH=/usr/local/cargo/bin:$PATH
RUN apt-get update && apt-get install -y --no-install-recommends build-essential pkg-config libssl-dev && rm -rf /var/lib/apt/lists/*
RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --no-modify-path --default-toolchain 1.94.0 --profile minimal
WORKDIR /src
COPY . .
RUN cargo build --release -p lsp-mcp && cp target/release/lsp-mcp /lsp-mcp
```
And in the final stage (after the existing toolchain setup), add:
```dockerfile
COPY --from=lspbuild /lsp-mcp /usr/local/bin/lsp-mcp
```

- [ ] **Step 3: Build from the REPO ROOT context and verify the binary runs**

```bash
docker build -t a2a-toolchain:latest -f deploy/containers/toolchain.Containerfile .
docker run --rm --entrypoint /usr/local/bin/lsp-mcp a2a-toolchain:latest --help 2>&1 | head -3
```
Expected: the lsp-mcp usage/help prints (the Linux binary runs).

- [ ] **Step 4: Update the runbook**

In `docs/containerized-agents.md`, change the documented toolchain build command from `... deploy/containers` context to the **repo-root** context (`docker build -t a2a-toolchain:latest -f deploy/containers/toolchain.Containerfile .`), and note the `.dockerignore`. Mirror for podman (`podman build ... .`).

- [ ] **Step 5: Commit**

```bash
git add deploy/containers/toolchain.Containerfile .dockerignore docs/containerized-agents.md
git commit -m "feat(toolchain): bake the Linux lsp-mcp binary (repo-root build stage + .dockerignore)"
```

---

## Task 4: The impl-lsp dep cache volume name (pure, reuse verify)

**Files:**
- Modify: `bin/a2a-bridge/src/implement.rs` (or a small helper near the warm phase)
- Test: same file (`#[cfg(test)]`)

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn impl_lsp_cache_name_is_per_repo_and_distinct_from_verify() {
    let a = crate::verify::cache_volume_name("a2a-impl-lsp-cache", "/clones/x");
    let b = crate::verify::cache_volume_name("a2a-impl-lsp-cache", "/clones/y");
    assert_ne!(a, b, "different repos must get different cache volumes");
    // distinct base from verify so the two caches never collide
    let v = crate::verify::cache_volume_name("a2a-verify-cache", "/clones/x");
    assert_ne!(a, v, "impl-lsp cache must not share verify's volume");
    assert!(a.starts_with("a2a-impl-lsp-cache-"));
}
```

- [ ] **Step 2: Run to verify it fails / passes**

Run: `cargo test -p a2a-bridge impl_lsp_cache_name`
Expected: PASS immediately if `cache_volume_name` is `pub` and reachable (it is — `verify.rs`). If the test can't see it, make `cache_volume_name` `pub(crate)` and re-run. (This task mostly *pins the decision* to reuse the pure helper with a distinct base.)

- [ ] **Step 3: Commit**

```bash
git add bin/a2a-bridge/src/implement.rs
git commit -m "test(implement): impl-lsp dep cache reuses verify::cache_volume_name with a distinct base"
```

---

## Task 5: The `warm_lsp_deps` phase

**Files:**
- Modify: `bin/a2a-bridge/src/main.rs` (the fresh `implement_cmd` ~1487–1570 and the resume path ~1770–1841) and/or `bin/a2a-bridge/src/implement.rs` for the pure helper.

Adds a pre-edit phase that runs `cargo fetch --locked` on the clone in a **no-creds, registries-egress** container, into the impl-lsp cache volume, **before** `build_warm_impl`. Reuses `compose_verify`'s egress/proxy wiring (see `verify.rs`/`sandbox.rs`); gated on a `[verify]` block existing (degrade with a warning otherwise).

- [ ] **Step 1: Write the failing test for the pure compose**

```rust
// in implement.rs #[cfg(test)]
#[test]
fn warm_lsp_fetch_argv_uses_egress_offline_false_and_cache_mount() {
    // compose_warm_fetch(clone, cache_vol, egress_cfg) -> (program, argv) for `docker run ... cargo fetch --locked`
    let (program, argv) = compose_warm_fetch(
        "/clones/x", "a2a-impl-lsp-cache-deadbeef",
        &WarmEgress { network: "a2a-verify-egress".into(), proxy: "http://a2a-verify-proxy:8888".into() },
    );
    assert_eq!(program, "docker");
    let joined = argv.join(" ");
    assert!(joined.contains("--network a2a-verify-egress"), "{joined}");
    assert!(joined.contains("a2a-impl-lsp-cache-deadbeef:/cargo"), "{joined}");
    assert!(joined.contains("CARGO_HOME=/cargo"), "{joined}");
    assert!(joined.contains("cargo fetch --locked"), "{joined}");
    assert!(!joined.contains("auth.json"), "warm fetch must mount NO creds");
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p a2a-bridge warm_lsp_fetch_argv`
Expected: FAIL — `compose_warm_fetch`/`WarmEgress` not found.

- [ ] **Step 3: Implement `compose_warm_fetch` (pure)**

```rust
pub struct WarmEgress { pub network: String, pub proxy: String }

/// PURE. The `(program, argv)` to fetch deps into the impl-lsp cache via the registries egress, NO creds.
pub fn compose_warm_fetch(clone: &str, cache_vol: &str, e: &WarmEgress) -> (String, Vec<String>) {
    let argv = vec![
        "run".into(), "--rm".into(),
        "--network".into(), e.network.clone(),
        "-e".into(), format!("HTTPS_PROXY={}", e.proxy),
        "-e".into(), format!("HTTP_PROXY={}", e.proxy),
        "-e".into(), "CARGO_HOME=/cargo".into(),
        "-v".into(), format!("{clone}:/work"),
        "-v".into(), format!("{cache_vol}:/cargo"),
        "--workdir".into(), "/work".into(),
        "--entrypoint".into(), "bash".into(),
        "a2a-toolchain:latest".into(),
        "-c".into(), "cargo fetch --locked".into(),
    ];
    ("docker".into(), argv)
}
```

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p a2a-bridge warm_lsp_fetch_argv`
Expected: PASS.

- [ ] **Step 5: Wire the phase into both `implement` paths (impure)**

In `implement_cmd` after `clone_cwd` (≈1487) and before `build_warm_impl` (≈1570), and identically in the resume path (≈1770/1841): if a `[verify]` block exists, derive the impl-lsp cache name (`verify::cache_volume_name("a2a-impl-lsp-cache", &clone_canon)`), `compose_warm_fetch(...)` from the verify egress config, run it (reuse the same `Runner`/spawn used by verify), and log the result. On no `[verify]` or a fetch failure: `eprintln!` a warning and continue (degraded nav). Surface the cache volume name so Task 6's runtime mount uses the same value.

- [ ] **Step 6: Build + test**

Run: `cargo build -p a2a-bridge && cargo test -p a2a-bridge warm_lsp`
Expected: green.

- [ ] **Step 7: Commit**

```bash
git add bin/a2a-bridge/src/main.rs bin/a2a-bridge/src/implement.rs
git commit -m "feat(implement): warm_lsp_deps phase — fetch deps into the impl-lsp cache via the verify egress (no creds)"
```

---

## Task 6: Wire `lsp` + env + runtime mounts onto the impl agent

**Files:**
- Modify: `bin/a2a-bridge/src/main.rs` (runtime mounts for the impl agent's container spawn), `examples/a2a-bridge.containerized.toml` (+ `.podman.toml`)

- [ ] **Step 1: Add the `[[agents.mcp]] lsp` block + env to the impl agent (both configs)**

```toml
[[agents.mcp]]
name = "lsp"
command = "/usr/local/bin/lsp-mcp"
args = ["--repo", "{cwd}", "--lang", "rust", "--target-cache", "/lsp-target"]
[[agents.mcp.env]]
# CARGO_HOME points at the dep cache mount; offline so RA never tries to fetch; log under the clone (survives --rm).
# (key,value pairs — render order preserved by render_codex_mcp_args)
```
Add env entries `CARGO_HOME=/cargo`, `CARGO_NET_OFFLINE=true`, `LSP_MCP_LOG={cwd}/.git/a2a-bridge/lsp-mcp-calls.log` (the `{cwd}` is substituted; `.git/` survives the loop reset and is fetched at hand-off).

- [ ] **Step 2: Add the runtime cache/target volume mounts for the impl agent**

At the impl agent's `:rw` container spawn (where `ContainerRwBackend`/the sandbox compose runs), append `-v <impl-lsp-cache>:/cargo[:ro per Task 1]` and `-v <impl-lsp-target>:/lsp-target` using the **runtime-derived** cache name from Task 5 (NOT static config, per codex #1). The target volume name = `verify::cache_volume_name("a2a-impl-lsp-target", &clone_canon)`.

- [ ] **Step 3: Podman parity test**

Run: `cargo test -p a2a-bridge podman`
Expected: PASS (the two configs still mirror with the lsp block).

- [ ] **Step 4: Commit**

```bash
git add bin/a2a-bridge/src/main.rs examples/a2a-bridge.containerized.toml examples/a2a-bridge.containerized.podman.toml
git commit -m "feat(config): wire lsp into the impl agent (CodexNative + [[agents.mcp.env]] + runtime cache/target mounts)"
```

---

## Task 7: `lsp-mcp` idle-evict (frees the impl RA during the review phase)

**Files:**
- Modify: `crates/lsp-mcp/src/lsp/mod.rs` (store `repo`/`target_cache`; activity timestamp; `evict`/`ensure_ready`-respawn), `crates/lsp-mcp/src/mcp/mod.rs` (touch activity on each `tools/call`; spawn the idle watcher)

- [ ] **Step 1: Write the failing unit test for the pure idle decision**

```rust
// in lsp/mod.rs #[cfg(test)]
#[test]
fn should_evict_after_idle_timeout() {
    assert!(should_evict(/*idle_secs=*/120, /*timeout_secs=*/60));
    assert!(!should_evict(30, 60));
    assert!(!should_evict(120, 0), "timeout 0 disables eviction");
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p lsp-mcp should_evict`
Expected: FAIL — `should_evict` not found.

- [ ] **Step 3: Implement `should_evict` + store respawn inputs + evict/respawn**

```rust
/// PURE. Evict when idle longer than the (non-zero) timeout.
pub fn should_evict(idle_secs: u64, timeout_secs: u64) -> bool {
    timeout_secs > 0 && idle_secs >= timeout_secs
}
```
Add to `LspSession`: `repo: PathBuf`, `target_cache: Option<PathBuf>` (stored in `start`), and `last_activity: Arc<Mutex<Instant>>`. Add `pub fn touch(&self)` (set last_activity = now), `pub fn evict(&mut self)` (kill the RA child, set `readied=false`), and have `ensure_ready` **respawn** RA if the child was evicted (re-run the spawn from `start`, reusing `repo`/`target_cache`). A background thread polls `should_evict(last_activity.elapsed(), timeout)` and calls `evict` when true. Timeout from env `LSP_MCP_IDLE_EVICT_SECS` (default 60; 0 disables).

- [ ] **Step 4: Touch activity on each tool call**

In `mcp/mod.rs` `dispatch()`, after `log_tool_call`, call `s.touch()` (and `ensure_ready` already runs there — it now respawns if evicted).

- [ ] **Step 5: Run unit + integration**

Run: `cargo test -p lsp-mcp` (host, real RA)
Expected: `should_evict` passes; the integration tests still pass. Add an integration test `evict_then_query_reindexes` (guarded by `ra_available`): start a session, `evict()`, then `workspace_symbol("add")` returns the symbol (RA respawned + re-indexed).

- [ ] **Step 6: Commit**

```bash
git add crates/lsp-mcp/src/lsp/mod.rs crates/lsp-mcp/src/mcp/mod.rs
git commit -m "feat(lsp-mcp): idle-evict the RA child (frees ~3GB while idle; lazy respawn on next call)"
```

---

## Task 8: Build, full gate, and the live dogfood DoD

**Files:** none (validation), then memory.

- [ ] **Step 1: Build the toolchain image + the host lsp-mcp + the full workspace gate**

```bash
docker build -t a2a-toolchain:latest -f deploy/containers/toolchain.Containerfile .
cargo build --release && cargo fmt --all -- --check && cargo clippy --all-targets --all-features --locked -- -D warnings
cargo test --workspace --locked --exclude bridge-container -- --skip process::tests::terminate_reaps_child_no_zombie --skip process::tests::term_ignoring_loop_forces_group_sigkill --skip process::tests::drop_group_kills_descendants
```
Expected: image builds; gate green.

- [ ] **Step 2: Live dogfood gate**

Run `a2a-bridge implement "<small real change touching a pub fn>" --repo <repo> --base-ref main --config examples/a2a-bridge.containerized.toml`. Confirm the DoD (spec §Testing):
- The impl agent **issued an `lsp` tool call** during the edit — read `<clone>/.git/a2a-bridge/lsp-mcp-calls.log` (now under the clone, survives `--rm`; the clone is fetched at hand-off, or `docker cp` it before reap).
- RA **reflected a mid-session edit** (the agent changes a `pub` symbol and a later nav reflects it).
- The impl RA was **evicted during the review phase** — a `docker exec`/`docker stats` watcher shows the `rust-analyzer` process gone inside the impl container while the host reviewers run.
- `warm_lsp_deps` populated the impl-lsp cache + target volumes (`docker volume ls | grep a2a-impl-lsp`).

- [ ] **Step 3: Record memory**

Write a memory file: Slice B shipped; the spikes (RA in-container 6.6s/3GB, inotify edit-sync); the codex-review corrections (separate runtime-wired read-only cache, warm_lsp_deps phase, env via mcp.env, repo-root build context); idle-evict (review peak 9→6GB); the deferred shared-host-RA daemon. Add the MEMORY.md index line.

- [ ] **Step 4: Finish the branch**

REQUIRED SUB-SKILL: Use superpowers:finishing-a-development-branch.

---

## Self-review notes

- **Spec coverage:** image bake (Tasks 2–3), dep-access B / separate runtime-wired cache (Tasks 4–6), `warm_lsp_deps` (Task 5), env via `[[agents.mcp.env]]` (Task 6), repo-root build context (Task 3), edit-sync (no task — validated, unchanged), idle-evict (Task 7); dep-change restart DEFERRED (needs a writable/re-fetch path — read-only offline cache; see spec §5), offline proof first (Task 1), live gate + memory (Task 8). All codex findings #1–#8 map to a task.
- **Read-only vs writable** is gated by Task 1's verdict; Task 6 Step 2 references `[:ro per Task 1]`.
- **No `lsp-mcp` source change except idle-evict** — wiring is image + config; idle-evict is the one crate change (Task 7).
- **The warm fetch is no-creds** — asserted in Task 5's test (`!joined.contains("auth.json")`), preserving the isolation argument.
