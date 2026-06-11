# ADR-0030 — Podman support (macOS `podman machine`)

**Date:** 2026-06-11
**Status:** Accepted

**Builds on:** the runtime-agnostic seam (`SandboxConfig.runtime`, ADR-0013/0016/0017 containment;
ADR-0019 `:rw` implement; ADR-0021 reaper; the S3 `allowed_cmds` runtime allowlist). First slice of the
"container footprint" increment (podman first; the memory slices — concurrency cap, `--memory` caps + cargo
`-j` throttle, lower the OrbStack VM ceiling — follow).

**Spec:** `docs/superpowers/specs/2026-06-10-podman-support-design.md` (v2).
**Plan:** `docs/superpowers/plans/2026-06-10-podman-support.md`.
**Reviews (dogfooded, prism-grounded, codex + claude-fable):** `docs/superpowers/reviews/2026-06-10-podman-spec-review.md`,
`…-podman-cleanroom-design.md`, `…-podman-impl-codereview.md`, `…-podman-impl-rereview.md`.

---

## Context

The bridge runs containerized agents (`:ro` readers, a `:rw` implementor, a verify step) by shelling out to
a container runtime. It targeted Docker (via OrbStack on macOS). An operator needs to run it on a Mac with
**only podman** (rootless; macOS → `podman machine` → Linux containers) — OrbStack is unavailable there
(licensing). Two prism-grounded passes (a spec-review and an independent clean-room design) confirmed the
runtime is already a single configurable seam threaded as data through every compose/reaper/verify/CLI site,
with one pre-existing gap: `[verify].runtime` was never checked against `allowed_cmds`.

## Decision

Deliver podman as **config + a hand-rolled egress script + docs**, plus the one Rust change that closes the
verify-runtime gap. Docker stays the default and is untouched; podman is additive opt-in. Scope: **macOS
`podman machine`**; Linux rootless is a separate follow-up (uid/SELinux differ).

1. **Config** — `examples/a2a-bridge.containerized.podman.toml`: a byte-identical copy of the docker example
   modulo two axes — `allowed_cmds = ["podman"]` and `runtime = "podman"` on every `[agents.sandbox]` + `[verify]`.
   Pinned by a parse test that runs full snapshot/S3 validation and an **exact-remainder structural** parity
   compare (catches drift a line-set diff misses).
2. **Images** — qualify only the registry `FROM` bases for podman short-name resolution
   (`reader.Containerfile` → `docker.io/library/node:24-slim`, `proxy.Containerfile` → `…/debian:stable-slim`);
   leave `toolchain.Containerfile`'s **local** `FROM a2a-agent-reader:latest` unqualified.
3. **Egress** — `deploy/containers/podman-egress.sh up|status|down`: self-locating, idempotent, reproduces the
   same network/proxy names (config contract unchanged). It **asserts the internal networks are actually
   `--internal`** (else fails loud — a same-named non-internal net would void containment), **bind-mounts both
   tinyproxy filters**, **always recreates** the proxies on `up` (a restart is what loads an edited filter and
   guarantees correct wiring), supports env-driven subnet/IP pinning for old-podman (no internal-net DNS), and
   `down` reports non-not-found failures.
4. **Verify-runtime gate** (the only Rust) — a pure `config::gate_verify_runtime` validates the resolved
   `[verify].runtime` against the snapshot's `allowed_cmds` **after** `into_snapshot`, rejecting a disallowed
   runtime into the existing `VerifyOutcome::ConfigError` path (verify never runs on the wrong engine). It
   reads `bridge_core::domain::DEFAULT_RUNTIME` (the single "docker" default) and **normalizes** the runtime
   so no consumer re-defaults. Plus a **warn-level** runtime preflight that probes only **allowlisted**
   runtimes (never executes a config-named binary outside the allowlist), bounded so a wedged `podman info`
   can't hang startup.

## Consequences

- **Live gate (2026-06-11) — 6/6 PASS** on macOS `podman machine` (podman 5.8.2, 6 CPU / 6 GiB VM):
  - **G1 spawn:** a codex reader spawned through the full ACP path under podman and read the `:ro` mount (`SMOKE_OK`).
  - **G2 egress contract (the security gate):** agent net — `api.anthropic.com` via the proxy `405` (reached),
    `example.com` via the proxy `000` (refused), direct on the internal net no-route; verify net — `github.com`
    `200` (allowed), `api.anthropic.com` `000` (blocked) → creds-XOR-registries holds.
  - **G3 allowlist negative:** `allowed_cmds=["docker"]` + podman sandboxes → `sandbox runtime not allowed: podman`.
  - **G4 reap:** 0 leaked `a2a-ro`/`a2a-rw` containers after a run (synchronous podman `rm`).
  - **G5 `:rw` + uid/git round-trip:** a full `implement` converged — the warm `:rw` container edited the clone,
    the **host committed the container-staged index** (macOS virtiofs uid translation + `safe.directory`
    round-trip held, Docker-Desktop-like), `verify: PASS`, `review: APPROVE`, 1 attempt.
  - **G6 CLI:** `containers list` ran clean, and podman's `{{.Label "key"}}` go-template **matches docker's** —
    the design's top risk (template-dialect divergence) is resolved; **no template fork shipped**.
- **Design unknowns resolved live:** the local `FROM` resolves under podman (D5); podman 5.8 aardvark-dns
  **serves `--internal` networks** (D6) → the IP-pinning fallback was unnecessary (kept for old podman).
- **Built dogfooding the bridge:** the spec + plan were pressure-tested by the bridge's own `spec-review` and
  clean-room `design` workflows (codex + claude-fable, prism-wired); the implementation was authored then put
  through the bridge's own `code-review` twice — the first pass (REJECT) caught **two security blockers the
  author introduced** (preflight executing unvalidated runtimes; the egress script accepting a non-internal
  network), the re-review caught a **third blocker a fix introduced** (a "skip if running" short-circuit that
  trusted a possibly mis-wired proxy and didn't reload tinyproxy's startup-loaded filter). All fixed.
- **No Rust beyond the verify gate + preflight;** docker paths and tests are untouched (185 bin + 126 core
  tests green, clippy clean, fmt clean).
- **Follow-ups (deferred):** Linux rootless podman (SELinux `:z`/`:Z`, uid semantics — a separate validation
  increment); a `GatedVerifyConfig` newtype if a third verify site ever appears (today verify is implement-only,
  2 gated sites); the `static.crates.io` host is not in the verify filter (cache-covered; pre-existing on docker);
  the docker agent-proxy still bakes its filter while podman binds it (pre-existing divergence, doc-noted).
