# ADR-0020 — The build+test Verify Step (Containerized Agents, Slice B2b-2)

**Date:** 2026-06-05
**Status:** Accepted

**Builds on:** ADR-0019 (B2b-1 — the `implement` clone+edit+commit foundation). B2b-1 produced a reviewable
commit in a quarantine; B2b-2 adds a **trustworthy, deterministic build+test verify** of that commit and
reports the verdict in the operator hand-off. Third sub-slice of B2b (B2b-3 = review-the-diff + APPROVE/
REJECT + the review→tweak loop).

---

## Context

After `implement` commits the agent's change into the quarantine clone, the operator needs to know whether
it actually builds and tests pass — *before* merging. The agent itself can claim "tests pass," but a claim
from the entity being verified is not trustworthy. The verify must be run by the bridge (not the agent),
and it must be cheap enough to run on every `implement`. CI remains the authoritative post-merge gate; this
is a fast, local, pre-merge signal.

## Decision

`a2a-bridge implement` runs a **bridge-deterministic, config-driven build+test verify** on the committed
clone after the commit, and reports the verdict in the hand-off (`edit → commit → verify → hand-off`). The
impl agent moves to a Rust toolchain image so it has `cargo` (latent in B2b-2; B2b-3 self-fixes).

- **Reported, not gating.** The clone is quarantined and CI is authoritative post-merge, so a failing
  verify still commits + hands off (exit 0); the verdict (`verify: PASS/FAIL …`) is informational. The
  review→tweak loop is B2b-3.
- **Trust via out-of-band container exit codes.** `cargo test`/`clippy` run agent-authored code, so the
  verdict must NOT be parsed from a stdout marker stream (a crafted `println!` could desync FAIL→PASS).
  Instead **each command runs as its own `docker run`** and the bridge reads the **container's exit code**
  (which in-container code cannot fake). The per-command containers share the cache volume, so builds stay
  warm across them.
- **Reuse the `SandboxConfig`/`compose_sandbox`/`parse_egress` seam — no parallel egress schema.** A new
  pure `compose_verify` (sibling of `compose_container_rw` in `bridge-core/sandbox.rs`) derives the argv
  from a `SandboxConfig` (`mount=clone, access=Ro`, + the cache volume), reusing `compose_sandbox`.
  `VerifyConfig`'s egress is parsed by the existing `parse_egress_fields`, so "locked ⇒ network+proxy"
  stays unrepresentable-otherwise — there is no `#[serde(default)]` flat-shape gap that could silently
  yield a no-`--network` (full-internet) container where agent `build.rs` runs.
- **Structured commands + `--locked` everywhere.** `[[verify.commands]] { name, cmd, gate }` (gate=false ⇒
  reported, not failing). Defaults are fmt/clippy/build/test, each `--locked` against the `:ro` clone so a
  lock-changing build is a deterministic verify failure, not a permission error.
- **Separate verify egress — creds XOR registry-egress.** A dedicated `a2a-verify-egress` net + verify-
  proxy allowlists the cargo registries (`crates.io`/`github.com`, covering the sparse index / downloads /
  git-dep subdomains) for **verify only**; the cred-bearing `impl` agent + the `:ro` readers keep their
  provider-only `a2a-egress-internal`. **Verify mounts NO creds.** A malicious `build.rs` in verify can
  reach the registries but never the creds; the agent's container never gains registry egress.
- **Coverage opt-in.** `cargo-llvm-cov`/`cargo-tarpaulin` ship in the image (no "command not found"), but
  coverage is NOT a default command — it instruments (distinct RUSTFLAGS → separate fingerprints) so it
  can't reuse the warm non-instrumented cache, and it's report-only. The operator adds it per-repo.
- **`:ro` clone + per-repo cache + single-flight.** cargo writes only the cache volumes; the cache volume
  name is keyed by a hash of the **canonical** source-repo path (two spellings must not split the warm
  cache). Concurrent same-repo `implement` runs are not expected (the operator drives `implement`
  serially) and cargo's own package-cache + target locks serialize within the shared volume; an explicit
  cross-container verify lock is deferred to the warm-pool/concurrency slice.

The verify container runs cargo in the clone via a tested `cd '<clone>'` in the script — `compose_sandbox`
deliberately emits no `--workdir` (ACP cwd arrives via `session/new`) and the toolchain base inherits the
reader's `WORKDIR /work`, so a bare `sh -c` would otherwise build in the wrong directory.

## Components

| Concern | Home |
|---|---|
| pure `compose_verify` (argv for one verify command) | `crates/bridge-core/src/sandbox.rs` |
| `VerifyConfig`/`VerifyCommand` + `parse_egress_fields` refactor + `RegistryConfig.verify` | `bin/a2a-bridge/src/config.rs` |
| pure verdict (`aggregate`/`truncate_output`/`verdict_line`/`outcome_suffix`/`cache_volume_name`), `run_verify` (runner-injected), `docker_runner` | `bin/a2a-bridge/src/verify.rs` |
| integration (run verify after the commit; verdict in the hand-off) | `bin/a2a-bridge/src/main.rs` (`implement_cmd`) |
| `a2a-toolchain` image (reader + Rust 1.94.0 + clippy/fmt + coverage tools) | `deploy/containers/toolchain.Containerfile` |
| separate verify egress (net + proxy + filter) | `deploy/containers/compose.egress.yaml`, `tinyproxy.verify.filter` |
| `impl` image + `[verify]` block | `examples/a2a-bridge.containerized.toml` |

## Dual-review folds (the reviews earned their keep again)

Spec review: reuse the egress seam (no parallel schema — the flat-egress typo hole), out-of-band exit
capture (not an adversarial stdout stream), structured commands, separate verify egress, coverage opt-in.
Plan review: **three compile blockers**, with a notable cross-lens result — the compile lens *missed* the
`cfg` use-after-move (`into_snapshot` consumes `cfg` before the `Action::Commit` arm), which the coverage
lens caught; the compile lens *uniquely* caught the `/work` cwd bug. Also: `[server]`-missing fixtures
(`RegistryConfig.server` isn't `serde(default)`), the canonical-repo cache key, the pure `outcome_suffix`
coverage keystone, a discriminating non-gate test ordering, and the `--rm`-raced containment proof (→
`docker events` + a behavioral `:ro` probe). Ground-truth correction: the coverage floors are `ci.yml`'s
(workspace 85 + bridge-core/acp/api/workflow 90), NOT the stale README's bridge-registry.

## Validation

- Unit (Docker-free): `compose_verify` golden (the `:ro` clone, the verify-egress network/proxy from the
  `EgressPolicy`, the cache volume, the `cd '<clone>'` script, no creds); `run_verify` (stop-at-first-gate,
  non-gate-reported-then-continues, runner-error); `aggregate`/`truncate_output`/`verdict_line`/
  `outcome_suffix`/`cache_volume_name`; `VerifyConfig` parse (present/absent, empty-commands reject,
  locked-without-network reject). Full workspace `cargo test` GREEN; clippy `-D warnings` clean.
- Coverage (after `cargo llvm-cov clean --workspace`, floors per ci.yml): met — the new bridge-core code
  (`compose_verify`) is fully unit-tested; the new bin code (`verify.rs` pure helpers + `outcome_suffix`)
  is unit-tested; only `docker_runner` + the impure Commit-arm resolution are live-gated.
- Live gate (Docker, dogfooded on THIS repo): `implement` against a fresh clone →
  `verify: PASS (fmt ✓ · clippy ✓ · build ✓ · test ✓)` (committed `f351fb9`). Proven across the runs:
  FAIL stops at the first gate + still commits (verify is informational); the warm cache (cold first run
  → fast subsequent); `docker events` showed **4 verify containers on `a2a-verify-egress` + the agent's
  edit container on `a2a-egress-internal`** (creds⊥registries); verify mounts no creds (structural).

### Live-gate findings (the dogfood earned its keep)
- **`NO_PROXY=localhost,127.0.0.1`** is required on the verify egress, or `cargo test`'s local HTTP servers
  (wiremock) get hijacked by the cargo-fetch proxy → spurious failures. Folded into the example `[verify]`.
- **A hermetic verify can't run system-integration tests.** This repo's `bridge-container` tests need a
  Docker daemon (no docker-in-docker) and the `bridge-core` process-supervisor tests need host PID-1
  zombie-reaping / process-group SIGKILL semantics. The example `[verify]` scopes the test command to the
  hermetic-safe subset; CI runs the full suite post-merge. General principle, not a B2b-2 defect.
- **Output truncation must keep the tail** — cargo prints the failure list + summary last; a head-only
  clamp hid the failure. `truncate_output` now keeps head+tail.
- **Pre-existing reader-container leak (follow-up):** the `:ro` AcpBackend agents (review workflows) are
  launched `--rm` but linger when the `docker run` doesn't exit cleanly — ~15 reaped during this slice.
  Only `ContainerRw` has a reaper; the `:ro` path needs one too. Tracked as a follow-up (not B2b-2 scope).

## Deferred

- **B2b-3:** the review→tweak loop (re-prompt the agent — in its toolchain image — to fix a failing
  verify); review-the-diff lenses + synth + APPROVE/REJECT.
- `--verify-strict` (exit non-zero on a failing verify); `--no-verify`; per-language `[verify]` configs; a
  coverage-floor gate; an explicit cross-container verify lock (concurrency); README floor-table fix.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
