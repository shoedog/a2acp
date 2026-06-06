# Containerized Agents — Slice B2b-2 Design: the build+test verify step

**Date:** 2026-06-05
**Status:** Draft (rev2, post dual-review). Folds the containerized-dogfood + a2a-local-codex spec reviews
(needs-changes): reuse the `SandboxConfig`/`compose_sandbox` egress seam (no parallel schema), derive the
verdict from out-of-band container exit codes (not stdout), structured commands, `--locked` everywhere, a
separate verify egress, coverage opt-in, per-repo cache + single-flight.
**Builds on:** B2b-1 (the `implement` subcommand, merged 6bca5df, ADR-0019). B2b-3 (review→tweak loop +
review-the-diff + APPROVE/REJECT) follows.

## Goal

After the `implement` commit, the bridge runs a **trustworthy, config-driven build+test verify** in a
toolchain container on the quarantine clone and **reports the verdict** in the operator hand-off. The impl
agent moves to a toolchain image so it has `cargo` (latent in B2b-2; B2b-3 self-fixes). CI stays the
authoritative post-merge gate.

## Decisions locked (design dialogue + dual-review fold)

1. **Bridge-deterministic, after the commit** (`edit → commit → verify → hand-off-with-verdict`). The bridge
   runs cargo (not the agent); the verdict is REPORTED, not gating (the clone is quarantined; the loop is
   B2b-3). Not a workflow node.
2. **Trust via out-of-band container exit codes** [review BLOCKER]. `cargo test`/`clippy` run agent-authored
   code, so the verdict must NOT be parsed from a stdout marker stream (a `println!("=== VERIFY 5 ===")`
   could desync FAIL→PASS). Instead **each command runs as its own `docker run`** and the bridge reads the
   **container's exit code** (which in-container code can't fake) + captures its stdout/stderr for display.
   The per-command containers **share the cache volume**, so builds stay warm across them.
3. **Reuse the `SandboxConfig`/`compose_sandbox` seam — no parallel egress schema** [review BLOCKER]. A new
   `compose_verify` (sibling of `compose_container_rw` in `bridge-core/sandbox.rs`) derives the argv from a
   `SandboxConfig` (`mount=clone, access=Ro`, + the cache volume), reusing `compose_sandbox`. `VerifyConfig`
   carries its egress as a validated `EgressPolicy` (via the existing `parse_egress`), so "locked ⇒
   network+proxy" stays unrepresentable-otherwise — no `#[serde(default)]` flat-shape gap that could yield a
   container with no `--network` (full internet) where agent `build.rs` runs.
4. **Structured commands** [review BLOCKER]: `[[verify.commands]] { name, cmd, gate }` (gate=false ⇒
   reported, not failing). Generically implementable; no hard-coded "llvm-cov is special".
5. **`--locked` on every resolving command** [review BLOCKER] (clippy/build/test; fmt is exempt — no
   resolution). With the `:ro` clone, `--locked` makes a lock-changing build a deterministic verify failure,
   not a permission error.
6. **Separate verify egress** [owner-confirmed]: a dedicated `a2a-verify-egress` net + verify-proxy/filter
   allowlisting the registries (`crates.io`/`index.crates.io`/`static.crates.io`/`github.com`/
   `codeload.github.com`) for **verify only**; the cred-bearing `impl` agent + the `:ro` readers keep their
   provider-only `a2a-egress-internal`. **Verify mounts NO creds.** Creds and registry-egress never coexist.
7. **Coverage opt-in** [owner-confirmed]: `cargo-llvm-cov` + `cargo-tarpaulin` ship in the image (no
   "command not found"), but coverage is NOT in the default commands — it instruments (distinct RUSTFLAGS →
   separate fingerprints) so it can't reuse the warm non-instrumented cache, and it's report-only. The
   operator adds it per-repo. Default gates = fmt/clippy/build/test (warm-cacheable).
8. **Clone `:ro`** (cargo writes only the cache volumes); **per-repo cache** + **single-flight per repo**.

## Architecture

### The `a2a-toolchain` image
`deploy/containers/toolchain.Containerfile`: `FROM a2a-agent-reader:latest` + `build-essential` (gcc/linker
— `node:24-slim` has none) + the Rust toolchain **pinned to the repo's `rust-toolchain.toml` (1.94.0)** +
`rustup component add clippy rustfmt` + `cargo install --locked cargo-llvm-cov cargo-tarpaulin` with explicit
`--version` pins (exact versions chosen at plan time against the 1.94.0 toolchain, then frozen in the
Containerfile). One image for both the impl agent (ACP CLI + cargo) and verify (cargo).

### `[verify]` config (reuses the validated egress; structured commands)
```toml
[verify]
image = "a2a-toolchain:latest"
cache = "a2a-verify-cache"     # base name; the bridge appends a per-repo hash -> a2a-verify-cache-<repohash>
  [verify.egress]              # parsed by the EXISTING parse_egress -> EgressPolicy (locked ⇒ net+proxy req'd)
  mode    = "locked"
  network = "a2a-verify-egress"
  proxy   = "http://a2a-verify-proxy:8888"
  [[verify.commands]]
  name = "fmt";    cmd = "cargo fmt --all -- --check";                          gate = true
  [[verify.commands]]
  name = "clippy"; cmd = "cargo clippy --all-targets --all-features --locked -- -D warnings"; gate = true
  [[verify.commands]]
  name = "build";  cmd = "cargo build --locked";                               gate = true
  [[verify.commands]]
  name = "test";   cmd = "cargo test --locked";                                gate = true
  # opt-in coverage (cold instrumented build; reported):
  # [[verify.commands]]
  # name = "coverage"; cmd = "cargo llvm-cov --workspace --locked --summary-only"; gate = false
```
`VerifyConfig { image, cache, egress: EgressPolicy, commands: Vec<VerifyCommand{name,cmd,gate}> }` (top-level
in `RegistryConfig`, `Option`). Validation: non-empty `commands`; `egress` via `parse_egress` (locked
requires network+proxy); absent `[verify]` ⇒ verify is skipped (the only bypass — no `--no-verify` in
B2b-2).

### `compose_verify` (pure, in `bridge-core/sandbox.rs`)
```rust
/// PURE. One verify command's (program, argv): derive a SandboxConfig (mount=clone, access=Ro, + the cache
/// volume) and REUSE compose_sandbox so egress/runtime/argv stay one source of truth. The script exports
/// CARGO_HOME/CARGO_TARGET_DIR into the cache mount, then runs the single command. NO creds volume.
pub fn compose_verify(sb: &SandboxConfig, clone: &SessionCwd, cache_vol: &str, cmd: &str)
    -> (String, Vec<String>);   // -> <runtime> run --rm --network a2a-verify-egress -e *PROXY -v clone:clone:ro
                                //    -v <cache_vol>:/cache <image> sh -c 'export CARGO_HOME=/cache/cargo
                                //    CARGO_TARGET_DIR=/cache/target; mkdir -p …; <cmd>'
```
`SandboxConfig { mount: clone, access: Ro, egress: verify.egress, volumes: ["<cache_vol>:/cache"], image,
runtime }` → `compose_sandbox(.., "sh", ["-c", script])`. The egress comes from the validated `EgressPolicy`.

### The verify run (bridge-deterministic, per-command, cache-warm)
After the commit, the `implement` subcommand, **single-flight per repo** (a lockfile under the cache),
iterates the configured commands:
```
for c in verify.commands:
    (prog, argv) = compose_verify(verify_sandbox, clone, cache_vol, c.cmd)
    (exit, out)  = run(prog, argv)              # the bridge reads the CONTAINER exit code (unforgeable)
    results.push(VerifyResult { name: c.name, gate: c.gate, ok: exit == 0, output: truncate(out, 16 KiB) })
    if c.gate && exit != 0 { break }            # stop at the first GATE failure
verdict = VerifyVerdict { results, passed: results.iter().all(|r| !r.gate || r.ok) }
```
Per-command `docker run` sharing the `a2a-verify-cache-<repohash>` volume (`/cache/cargo` = CARGO_HOME,
`/cache/target` = CARGO_TARGET_DIR) → first run cold, the rest incremental across commands AND runs. The
cache is a **named volume** (container-owned, root — no host-uid remap). Per-repo keying isolates repos;
single-flight serializes same-repo runs (cargo locks + the poison surface).

### Integration into `implement_cmd`
The `Action::Commit` arm, after the host commit, before the hand-off: if `[verify]` is set, run verify, then
print the verdict. **Contract:** `implement` always commits + hands off + exits **0** (the commit
succeeded; verify is informational — a `--verify-strict` non-zero-on-fail flag is deferred). The hand-off
(stdout) carries the PASS/FAIL summary; failing-command output goes to stderr (truncated). Absent `[verify]`
→ the B2b-1 hand-off unchanged + a one-line "verify: not configured".
```
implement: committed <sha> "<subj>" on implement/<id>
verify: PASS  (fmt ✓ · clippy ✓ · build ✓ · test ✓)
   -- or -- verify: FAIL at clippy  (fmt ✓ · clippy ✗)   [output on stderr]
clone: <path>
<operator re-author/merge/reap commands>
```

## cargo-under-lockdown (operator infra)
A SECOND egress in `deploy/containers/compose.egress.yaml`: `a2a-verify-egress` net + `a2a-verify-proxy`
(its own `tinyproxy.verify.filter` with the registries). The agent/readers' `a2a-egress-internal` +
`a2a-egress-proxy` (provider-only) are untouched. The cache volume means deps fetch once.

## Component / file boundaries

| Concern | Home | Note |
|---|---|---|
| `a2a-toolchain` image | `deploy/containers/toolchain.Containerfile` | reader + build-essential + Rust(pinned) + clippy/fmt + llvm-cov/tarpaulin |
| verify egress (net + proxy + filter) | `deploy/containers/compose.egress.yaml`, `deploy/containers/tinyproxy.verify.filter` | separate from the agent egress |
| **pure** `compose_verify` | `crates/bridge-core/src/sandbox.rs` | reuses `compose_sandbox`; Docker-free unit tests |
| `VerifyConfig` + parse (reuse `parse_egress`) | `bin/a2a-bridge/src/config.rs` | structured commands; locked-egress validation |
| verify run + `VerifyVerdict` aggregate + hand-off line | `bin/a2a-bridge/src/verify.rs` (new) | the run is impure (live-gated); the aggregate + truncation + hand-off line are pure-tested |
| integration (run after commit; verdict in hand-off; single-flight) | `bin/a2a-bridge/src/main.rs` (`implement_cmd`) | the `Action::Commit` arm |
| `impl` agent image: `a2a-agent-reader:latest` → `a2a-toolchain:latest` | `examples/a2a-bridge.containerized.toml` (the `impl` agent block) | exact one-line change; readers unchanged |

## Testing
- **Unit (Docker-free):** `compose_verify` golden (the `:ro` clone mount, the verify-egress `--network`/
  proxy from the EgressPolicy, the cache volume, the `sh -c export…; <cmd>` script) — proving it reuses
  `compose_sandbox` (HTTP_PROXY+HTTPS_PROXY both, no stray `-w`); `VerifyVerdict` aggregation (all-gates-pass
  ⇒ passed; a gate fail ⇒ failed + stops; a non-gate fail ⇒ passed but reported; output truncation); the
  hand-off verdict line; `VerifyConfig` parse (present/absent; empty-commands reject; locked-egress-without-
  network reject via `parse_egress`).
- **Live gate (Docker, operator-run):** build `a2a-toolchain`; bring up `a2a-verify-egress`; `implement` a
  small change to a throwaway clone of *this* repo with `[verify]` → verdict ✅ per gate (cache warm on the
  2nd run); inject a clippy failure → ❌ at clippy + the output on stderr + verify stops; assert the verify
  container ran on `a2a-verify-egress` (NOT `a2a-egress-internal`) with **no creds mount** and the clone
  `:ro` (a write to the clone path fails), via `docker inspect`/`events`.
- Coverage after `cargo llvm-cov clean --workspace` (floors per **ci.yml** ground truth, NOT the stale
  README: workspace 85, **bridge-core 90, bridge-acp 90, bridge-api 90, bridge-workflow 90**). B2b-2's new
  pure code is in bridge-core (`compose_verify`) + the `a2a-bridge` bin (no per-package floor → workspace 85).

## Deferred
- **B2b-3:** review→tweak loop (re-prompt the agent — in its toolchain image — to fix a failing verify);
  review-the-diff lenses + synth + APPROVE/REJECT.
- `--verify-strict` (exit non-zero on a failing verify); `--no-verify`; per-language `[verify]` configs;
  a coverage-floor gate; cross-repo cache sharing of the download registry.

## Firewall
Designed from the bridge's own seams (`compose_sandbox`/`EgressPolicy`/`parse_egress`, the `implement`
`Action::Commit` arm, the egress proxy infra). Dual review = containerized dogfood PRIMARY + a2a-local
`codex-review` (gpt-5.5) backstop. Once B2b-3 lands, `implement` self-fixes against its own verify.
