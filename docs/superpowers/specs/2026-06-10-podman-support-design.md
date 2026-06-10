# Podman Support — Design (v2)

**Date:** 2026-06-10
**Status:** Draft (for review) — v2 folds a dogfooded **spec-review** (codex + claude-fable, prism-grounded:
`docs/superpowers/reviews/2026-06-10-podman-spec-review.md`) and an independent **clean-room design**
(codex + claude-fable, prism-grounded: `docs/superpowers/reviews/2026-06-10-podman-cleanroom-design.md`).
Both passes converged on v1's architecture and enriched it; both blockers are resolved below.
**Slice:** 1 of the "container footprint" increment (podman first; the memory slices — concurrency cap →
`--memory` caps + cargo `-j` throttle → lower the OrbStack VM ceiling — follow as separate specs).

**Goal:** Run the bridge's containerized backends (`:ro` readers, `:rw` implementor, verify) under
**podman on macOS (`podman machine`)** with **zero behavioral change on docker**, so the bridge runs on a
podman-only Mac (the operator's work + dev Macs).

**Owner decisions (locked):** (1) a disallowed `[verify].runtime` **rejects into `VerifyOutcome::ConfigError`**
(verify never runs); (2) **Linux rootless podman is deferred** to a follow-up increment (uid/SELinux differ);
this increment validates macOS `podman machine` only.

---

## Context & current state

The bridge shells out to a container runtime for every sandboxed workload (spawn agent, verify, sweep,
reap, staleness probe, boot lease-recovery, the `containers` CLI). Two independent prism-grounded passes
confirmed:

- **The runtime is already the single seam, threaded as data with no branch anywhere.** `SandboxConfig.runtime`
  (`crates/bridge-core/src/domain.rs`, `runtime()` defaults `"docker"`) flows through the S3 allowlist
  (`crates/bridge-registry/src/registry.rs:96-106`), every pure argv composer (`compose_sandbox`,
  `compose_container_rw`, `compose_verify`, `compose_sandbox_named`, the sweep/reap/inspect helpers in
  `crates/bridge-core/src/sandbox.rs`), the reaper (`crates/bridge-core/src/reaper.rs`), the `:rw` backend,
  and the `containers` CLI (`bin/a2a-bridge/src/main.rs:2449-2538`, which already iterates per-runtime and
  reaps idempotently across engines). `docs/containerized-agents.md:7-9` already names rootless podman as a
  target. **Podman enters as config; no Rust is required for it to *function*.**
- **GAP — `[verify].runtime` bypasses the allowlist.** `allowed_cmds` has exactly one enforcement consumer
  (`validate_sandbox`, S3), which covers agent sandbox blocks only. `VerifyConfig.runtime`
  (`config.rs:397`, parsed `:378`/`:411`, consumed `verify.rs:147-154`) is never checked. Pre-existing, but
  the podman increment makes it dangerous: a half-converted config silently runs verify on docker, and on a
  dual-engine host can **false-pass against Docker's image store**. Closed in slice 4 (owner decision 1).
- **RISK — Go-template dialect.** Two sites emit Docker's `{{.Label "key"}}`: `bin/a2a-bridge/src/containers.rs:25`
  (`LIST_FORMAT`, operator CLI) and `crates/bridge-core/src/sandbox.rs:222-235` (`managed_inspect_argv`, used
  by **boot-time lease recovery**, `reaper.rs:110-134`). If podman's template dialect diverges, `parse_record`/
  `plan_recovery` skip malformed lines — failing *closed into invisibility* (stale containers unrecovered/
  unreaped). Gated live (G4/G6); contingency designed, not pre-built (§6).

---

## Scope & non-goals

**In scope:** macOS `podman machine`; the podman example config + parse tests; podman image-build path; the
egress bring-up script + contract; the verify-runtime allowlist gate + a warn-level runtime preflight; docs.

**Non-goals / deferred:**
- **Linux rootless podman** — a separate follow-up increment (carries the SELinux `:z`/`:Z` structured-mount
  question and the differing uid semantics; same config + script, re-run gates). Owner decision 2.
- **Re-architecting the runtime abstraction** — it already exists; this slice consumes it.
- **podman as default** — docker stays the default (`unwrap_or("docker")`); podman is additive opt-in.
- **`podman-compose` dependency** — avoided; a hand-rolled script is the supported egress path.
- **Freeform per-runtime args / a `default_runtime` knob** — YAGNI; the runtime is named per sandbox/verify
  block as today.
- **The memory slices** (concurrency cap, `--memory` caps, `-j` throttle, VM-ceiling lowering) — later specs.

---

## Design

### §1 — Podman example config + pinning tests

`examples/a2a-bridge.containerized.podman.toml`: a copy of `a2a-bridge.containerized.toml` with exactly two
kinds of edit, diff-obvious:

- `allowed_cmds = ["podman"]` (podman-only; the runtime is the allowlist's S3 entry).
- `runtime = "podman"` in **every** `[agents.sandbox]` block (including `impl`) **and** the `[verify]` block.

Everything else (mounts, egress nets/proxy URLs, creds, workflows) is byte-identical. A header comment states
the two-line rule. A distinct config path also yields a distinct `container_owner = hash(config_path, mount,
agent_id)`, so docker-owned and podman-owned fleets coexist cleanly on a dual-engine host (the established
per-project-concurrency mechanism).

**Pinning (repo convention — shipped examples have parse tests):** two sibling tests in `main.rs` (patterns at
`:3341` and the full-`validate()` test at `:3548`). The latter live-exercises S3 with `allowed_cmds =
["podman"]` at **zero runtime cost** (validation is pure), and asserts structural parity with the docker
example (same blocks; diffs confined to `runtime`/`allowed_cmds`) so a future docker-example edit can't
silently drift the artifact docs + the smoke point at.

### §2 — Images (`FROM` qualification + build order)

Podman's short-name resolution can prompt/refuse unqualified bases. Qualify **only the registry bases**, which
are no-ops on Docker:
- `deploy/containers/reader.Containerfile:3` → `FROM docker.io/library/node:24-slim`
- `deploy/containers/proxy.Containerfile:1` → `FROM docker.io/library/debian:stable-slim`

**Do NOT qualify `deploy/containers/toolchain.Containerfile:4`** — it is `FROM a2a-agent-reader:latest`, a
**local** image reference; qualifying it would point at a nonexistent registry image and break both engines.
The runbook documents the build order **reader → toolchain → proxy**; if podman ever refuses the local
short-name, the podman-only fix is `localhost/a2a-agent-reader:latest` (not applied while the file also
serves Docker).

### §3 — Egress bring-up (`podman-egress.sh`, a tested contract)

`compose.egress.yaml` stays the untouched Docker path. Ship one self-contained script
`deploy/containers/podman-egress.sh` with `up | status | down`, that **self-locates** (`cd "$(dirname "$0")"`)
and uses raw podman primitives — the same surface the bridge itself uses — reproducing the **same names** so
the bridge config contract is unchanged:

- Networks (idempotent — ignore "already exists"): `a2a-egress-internal` (`--internal`), `a2a-verify-egress`
  (`--internal`), `a2a-egress-external` (routed).
- Each proxy: `rm -f <name>` first (the reaper's own idiom — makes re-running `up` the recovery path) →
  `create --name <name> --network <its-internal-net> [absolute -v for the verify filter]
  a2a-egress-proxy:latest` → `network connect a2a-egress-external <name>` → `start`. (`run`/`create` take a
  single `--network`; tinyproxy dials upstream lazily, so attach-then-start is safe.)
- `status`: report the 3 networks + 2 proxies. `down`: tolerate-absent (proxies before networks).

**Post-condition contract (G2 tests this — it doubles as the compose↔script drift detector):** three networks
exist (two internal with no external route, one routed); both proxies run attached to their internal net +
the external net; each proxy is reachable from its internal net at exactly the URL the bridge config states
(`http://a2a-egress-proxy:8888`, `http://a2a-verify-proxy:8888`); the verify proxy serves the
registries-only filter.

**DNS caveat (aardvark-dns):** podman historically didn't serve DNS on `--internal` networks (fixed
netavark ≥1.6 / podman ≥4.5). G2's **first** probe is name-resolution of `a2a-egress-proxy` from the internal
net. If it fails on the operator's podman, the fallback is pure config (no code): create the internal nets
with `--subnet`, pin the proxies with `--ip`, and set `proxy = "http://<ip>:8888"` in the podman example —
`EgressPolicy::Locked{network, proxy, no_proxy}` carries the proxy as opaque data. Name-first
(debuggability), IP-second. Docs state a minimum podman version.

**Restart survival:** `--restart unless-stopped` does **not** survive `podman machine stop/start` (daemonless).
The supported recovery is documented: re-run `podman-egress.sh up` after a machine restart (idempotent).
A quadlet/systemd-managed proxy is deferred to the Linux increment.

### §4 — Verify-runtime allowlist gate (slice 4 — the only Rust; owner decision 1: reject)

Close the §Context gap so a disallowed verify runtime **never executes**. *(Mechanism revised after a fable
consult — `docs/superpowers/reviews/2026-06-10-podman-spec-review.md` framed the gap; a focused follow-up
chose this simpler shape over the clean-room design's parse-time `effective_allowed_cmds()`/`to_config_checked`
because it carries no drift risk against `into_snapshot`'s union logic.)*

A **pure** gate, validated **after** the snapshot exists (so it reuses the snapshot's already-resolved
allowlist), called once at each implement site:

```
fn gate_verify_runtime(
    verify_cfg: Option<Result<VerifyConfig, ConfigError>>,
    allowed_cmds: &[String],
) -> Option<Result<VerifyConfig, ConfigError>>
```

- Only an `Ok(vc)` is gated; a pre-existing `Err` (e.g. empty `commands`) is preserved untouched; `None`
  (no `[verify]`) passes through as `None` (stays `NotConfigured`, never becomes `ConfigError`).
- The runtime is resolved **inside** the gate as `vc.runtime.as_deref().unwrap_or("docker")` — `VerifyConfig.runtime`
  stays `None` through `to_config()`; the `"docker"` default otherwise only materializes later in
  `compose_sandbox` via `SandboxConfig::runtime()`, so the gate must apply the same default (comment-pin the
  literal to `SandboxConfig::runtime()` to keep them from disagreeing).
- If the resolved runtime ∉ `allowed_cmds`, return `Some(Err(ConfigError::…))` with an **actionable** message:
  `verify runtime not allowed: "<rt>" — add it to [registry].allowed_cmds or set [verify].runtime`.
- **Wiring:** at both implement sites the existing line `let verify_cfg = cfg.verify.as_ref().map(|t| t.to_config());`
  (`main.rs:1287` and the `--resume` path `:1550`) stays as-is; immediately **after** `into_snapshot()` succeeds
  (`:1291`/`:1554`) wrap `verify_cfg = gate_verify_runtime(verify_cfg, &snap.allowed_cmds)`. `verify_cfg` is owned,
  so post-snapshot gating has no borrow issue. A disallowed runtime then flows naturally into the existing
  `VerifyOutcome::ConfigError` path (`run_verify_step`, `main.rs:806`) — no container spawns.
- **Why no `effective_allowed_cmds()` extraction:** `snap.allowed_cmds` *is* the verbatim output of
  `into_snapshot`'s union (which already includes each sandbox `runtime()`), so a `[registry]`-less all-podman
  config has `"podman"` in the list and a defaulted-docker verify correctly rejects — the half-converted-config
  bug. Reading the snapshot avoids a duplicated union that could drift. If `into_snapshot` fails first, the
  command aborts there anyway (`:1292`/`:1555`), so the gate's result is never consumed — same user-visible
  fatal error under either shape.
- Note the B2b-3b interaction: a `ConfigError` verify can't contribute a PASS, so a misconfigured loop surfaces
  loudly rather than converging falsely (the locked behavior, not a regression).

*(Scope corrections from the consult: `validate_sandbox` is `registry.rs:96`; `to_config()` at `main.rs:1213`/
`1500` is the `[implement]` LoopConfig, NOT verify — the two verify sites are `:1287`/`:1550`.)*

### §5 — Runtime preflight (warn-level, slice 4)

A boot/start nicety, **not** the enforcement (the allowlist in §4 + S3 is the hard gate). A pure helper probes
`<runtime> info` once at `serve` boot / `implement` start for the distinct runtimes actually used; on failure
it **warns** (names the runtime, suggests `podman machine start`) but does not block — a genuinely missing
runtime already degrades safely (ENOENT → `AgentCrashed` + reap no-op; verify → failed result). The bridge
resolves `podman` via `PATH`; the docs note this for launchd-launched `serve`. Injected probe → unit-testable.

### §6 — Template-dialect contingency (gate live; build only if a gate fails)

Do **not** pre-build a fix for the `{{.Label "key"}}` risk. Gate it (G4 = the safety-relevant recovery site;
G6 = the CLI). If a gate fails:
- **only `.Label` diverges** → a per-runtime template fork inside the two pure builders (they already take
  `runtime: &str`; smallest diff).
- **anything else in the template surface diverges** → replace the label readers with an inspect-JSON path
  (`ps -aq --filter label=…` → `inspect` IDs → parse labels from JSON), eliminating the dialect dependency
  class. Either way the change is confined to `sandbox.rs`/`containers.rs`; zero domain movement.

### §7 — Docs (`docs/containerized-agents.md` podman section + `docs/onboarding.md` pointer)

- `podman machine init` sizing (`--cpus 6 --memory 8192 --disk-size 100`); confirm `/Users` is mounted so the
  identical-path `-v {m}:{m}` bind works.
- Image build order (reader → toolchain → proxy) with `podman build` commands.
- The egress script (§3) + "re-run `up` after `podman machine start`".
- **Disjoint image/volume stores:** podman cannot see docker-built images; the named volume `a2a-kiro-data`
  does **not** carry over → **kiro device-flow re-mint is required** under podman.
- **Verify containers are unmanaged** (no `a2a.managed=1` label — `sandbox.rs:136`): `containers list|reap`
  never sees them; the runbook must **not** promise verify cleanup via `containers reap` (true on Docker too).
- **podman `rm` is synchronous** — expect 0 containers immediately after a run, unlike Docker Desktop's ~2 s
  async removal (so the reap gates assert 0 *immediately* under podman).
- The `PATH` note (§5) for launchd `serve`.

### §8 — `sync-creds.sh` message

`deploy/containers/sync-creds.sh` is host-side and runtime-agnostic, **except** line 48 prints a hardcoded
`docker run` hint for the kiro re-login. Make the message runtime-neutral (or honor `CONTAINER_RUNTIME`).
Docs-slice nit, not a behavior change.

---

## §9 — Runtime-parity audit (docker-ism → podman)

| surface | bridge usage | podman | action |
| --- | --- | --- | --- |
| spawn | `run -i --rm --name --label -v --network <img> <cmd>` | identical | none |
| sweep | `ps -aq --filter name=…`, `ps -a --filter label=… --format {{…}}` | identical CLI; **template dialect is the risk** | §6 (gate) |
| reap | `rm -f <name>` (synchronous on podman) | identical | docs note |
| staleness | `logs --since <win> --tail 1 <name>` | identical; `is_stale` biases false on error → degrades safe | none |
| `:ro`/`:rw` bind | `-v host:dst[:ro]`, identical-path | identical (confirm `/Users` mounted in machine) | docs |
| internal net | `--network …-internal` + name-resolved proxy URL | `network create --internal`; **DNS caveat** | §3 (probe + IP fallback) |
| uid on bind writes | container root → host user | macOS machine: **virtiofs VM-remap** (Docker-Desktop-like); the B2b-1 `safe.directory` round-trip absorbs it | G5 |
| writable creds mount | token refresh writes back | works | G5 |

No code path needs a docker-vs-podman branch (the one possible exception, §6, is contingent and seam-local).

---

## §10 — Validation gates (macOS `podman machine`)

- **G1 — build + spawn:** build reader→toolchain→proxy under podman; a **single-agent** workflow completes
  (Slice-A finding: `design`/`code-review` fan out — use a single-agent workflow for the spawn gate). May use
  `egress = "open"` as a diagnostic config (`domain.rs:105`, parse arm `config.rs:626`); the shipped example
  stays locked-only.
- **G2 — egress contract (the security gate):** from the internal net — proxy resolves+reachable **by name**
  (else pin IPs + flip the config); allowlisted host via proxy OK; non-allowlisted refused by tinyproxy;
  `curl --noproxy '*'` → no route. Repeat on `a2a-verify-egress` with the registries filter (hosts taken from
  the checked `tinyproxy.filter`/`tinyproxy.verify.filter`: agent net allows `anthropic.com`, `chatgpt.com`
  for codex — not `api.openai.com` — and kiro/cognito hosts; verify net allows registries only, creds-XOR-
  registries preserved).
- **G3 — allowlist negatives:** podman config + `allowed_cmds = ["docker"]` → `sandbox runtime not allowed:
  podman` (live S3, mirrored by the parse test). Slice-4: verify-runtime mismatch → `ConfigError`, runner
  never called; and a `[registry]`-less podman-verify config default-allows.
- **G4 — reap + recovery:** `podman events`-asserted container **start** (not echo); kill mid-turn →
  owner-scoped boot-sweep; end → 0 containers **immediately**; **includes a lease-recovery pass exercising
  `managed_inspect_argv`'s template under podman** (the §6 gate).
- **G5 — `:rw` + uid + creds:** a full `implement` e2e — container writes the clone, host commits the staged
  index (the B2b-1 round-trip), ownership sane; a token refresh writes back through the writable creds bind.
- **G6 — `containers` CLI:** list/classify/reap podman-owned containers (pins `LIST_FORMAT` + the per-runtime
  loop; the §6 gate for the CLI template).

A failure at G2 (egress leak) or G4/G6 (template invisibility) or G5 (uid round-trip) is a real conformance
gap; a G1 build/machine failure is environmental.

---

## §11 — Slices + build order

1. **Config + images + docs:** the podman example + two pinning tests; the two registry-`FROM` qualifications
   (NOT toolchain); runbook section; `sync-creds.sh` message. *Exit: G3 + an open-egress spawn smoke (G1).*
2. **Egress:** `podman-egress.sh` implementing the post-condition contract; name-vs-IP decided by the G2 probe.
   *Exit: G2 in full, both nets.*
3. **Full loop:** kiro re-mint, creds round-trip, `implement` e2e, reap/recovery, `containers` CLI.
   *Exit: G4 + G5 + G6 → **podman support ships here**.*
4. **Hardening (this increment):** the verify-runtime gate (§4, reject→`ConfigError`) + `effective_allowed_cmds()`
   + the warn-level preflight (§5); the template fork (§6) **only if** G4/G6 demanded it.

Effort ~2–3 days, dominated by validation.

---

## §12 — Risks (ranked)

1. **Template-dialect divergence** — fails closed into recovery/CLI invisibility; gated twice (G4/G6);
   contingency seam-local (§6).
2. **Half-converted `[verify].runtime`** — false-pass on dual-engine hosts; closed by slice 4 (§4).
3. **Internal-net DNS** — fails safe-but-down; config-only IP fallback (§3).
4. **Compose↔script drift** — caught by G2-as-contract (§3).
5. **Proxy loss across `podman machine` restart** — fails closed; idempotent re-up documented (§3).

Failure modes already survivable in code: missing binary / stopped machine → ENOENT → `AgentCrashed` + reap
no-op; verify → failed result; a `logs --since` quirk degrades staleness detection without breaking reap.

---

## §13 — Testing

- **Unit:** the two example parse/parity tests (§1); the verify-gate `gate_verify_runtime` pure-function set
  (§4) — (1) defaulted runtime (`None`) + allowed=`["podman"]` → `Some(Err)` naming "docker"; (2) explicit
  `"docker"` + allowed=`["podman"]` → `Some(Err)`; (3) explicit `"podman"` allowed → `Some(Ok)` unchanged;
  (4) back-compat `None` + allowed contains `"docker"` → `Some(Ok)`; (5) a prior `Err` passes through
  untouched; (6) `None` input → `None`; plus (7) one TOML-level wiring pin (a `[registry]`-less podman config
  with a defaulted `[verify]` → `into_snapshot` → gate → `run_verify_step` returns `VerifyOutcome::ConfigError`,
  still runtime-free); the preflight helper (warn on probe failure, skip on empty runtime set) with an injected
  probe (§5). Optionally, a `"podman"` case on the argv builders
  asserting `argv[0]` is the configured runtime.
- **Live gates:** §10 G1–G6 on podman-machine here (the operator stops the stockTrading dev stack to free host
  RAM for the machine VM; tear the machine down after).
- Docker remains the default; its paths and tests are untouched.

---

## §14 — Out of scope / follow-ups

- **Linux rootless podman increment** (owner decision 2): re-run G1–G6 on Linux rootless; first known concern
  is SELinux relabeling (`:z`/`:Z`) — a narrow structured mount-label option only if validation demands it.
  Same config + script; uid semantics differ (the macOS gate masks Linux breaks — B2b-1 lineage).
- **Memory slices** (next specs): a bridge-wide concurrent-sandbox-workload cap (the measured OOM root cause);
  `--memory`/`--memory-swap` caps + a `CARGO_BUILD_JOBS` verify throttle (slower-not-OOM); lowering the
  OrbStack VM ceiling once the bridge's peak is bounded. Measured anchors: VM ceiling 17.59 GiB; stockTrading
  idle ~1.1 GiB; `:ro` reviewer ~0.54 GiB (container) vs ~0.47 GiB (host) → ~70 MB container overhead; verify
  build peak `-j15` 1.89 GiB vs `-j2` 1.03 GiB; rootless podman also lowers the baseline.
- **Disk:** the per-repo verify caches total ~67 GB; a cache-GC command is a candidate follow-up.
- **A third runtime (`nerdctl`)** would land as slices 1–2 of config work — the seam held under both
  architects' independent inspection.
