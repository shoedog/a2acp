# Podman Support — Design

**Date:** 2026-06-10
**Status:** Draft (for review)
**Slice:** 1 of the "container footprint" increment (podman first, then the memory slices: concurrency cap → per-container caps/throttle, which then let the OrbStack VM ceiling be *lowered*).

**Goal:** Run the bridge's containerized backends (`:ro` readers, `:rw` implementor, verify) under **rootless podman** with **zero behavioral change on docker**, so the bridge can be used on a podman-only machine (the operator's work Mac → `podman machine` → Linux containers, same shape as this dev Mac).

---

## Context & current state

The bridge shells out to a container runtime for every sandboxed workload (spawn agent, verify, sweep, reap, staleness probe). An audit + live probing during the memory-measurement session established:

- **The runtime is already a single configurable knob, threaded as a parameter everywhere.** `SandboxConfig.runtime: Option<String>` (`crates/bridge-core/src/domain.rs:67`) defaults to `"docker"` via `runtime()` (`:85`). The compose builders (`crates/bridge-core/src/sandbox.rs`), the reaper (`crates/bridge-core/src/reaper.rs` — `reap_once`/`production_reap_fn`/`run_scoped_reap`/`classify_sweep`/`is_stale` all take `runtime: &str`), the verify runner (`bin/a2a-bridge/src/verify.rs`), and the `containers` CLI (`bin/a2a-bridge/src/main.rs`) all receive the runtime name. The only non-test `"docker"` literal is the `unwrap_or("docker")` default. **No hardcoded bypass.**
- **The runtime is also security-allowlisted.** `validate_sandbox` (`crates/bridge-registry/src/registry.rs:104`, the S3 invariant) rejects any sandbox whose resolved `runtime` is not in `allowed_cmds`. So selecting podman is two config facts: `runtime = "podman"` **and** `"podman"` ∈ `allowed_cmds`.
- **Every command the bridge issues is podman-CLI-identical:** `run -i --rm`, `ps -aq --filter name=…`, `ps -a --filter label=… --format {{…}}`, `rm -f`, `logs --since … --tail 1`, `-v host:dst[:ro]`, `--network <name>`, `--label k=v`, `--name`. Podman matches Docker's CLI surface for all of these.

**What is NOT done (this slice):**
1. There is no shipped **podman example config**, so an operator must hand-edit `runtime`/`allowed_cmds` into every sandbox block.
2. The **egress infrastructure** (`deploy/containers/compose.egress.yaml`) is `docker compose`-only — three networks (two `internal: true`) and two tinyproxy services. Podman needs an equivalent bring-up path.
3. There is **no preflight** that the configured runtime binary actually exists/responds, so a missing or unstarted podman surfaces as a cryptic per-spawn failure instead of a clear boot error.
4. **Docs** (`docs/containerized-agents.md`, `docs/onboarding.md`) describe docker only.

---

## Non-goals

- **Re-architecting the runtime abstraction** — it already exists; this slice consumes it.
- **Linux rootful docker** — already unsupported (the `:rw` implement design rejects rootful Docker on Linux due to bind-mount uid ownership); unchanged.
- **The memory slices** (concurrency cap, `--memory` caps, `-j` throttle, VM-ceiling lowering) — separate specs, follow this one.
- **podman as the default** — docker stays the default (`unwrap_or("docker")`); podman is opt-in.
- **A cross-runtime abstraction layer / runtime auto-detection** — YAGNI; the operator names the runtime in config.

---

## Design

### §1 — Podman example config

Ship `examples/a2a-bridge.containerized.podman.toml`: a copy of `a2a-bridge.containerized.toml` with exactly two kinds of edit, so it's diff-obvious:

- `allowed_cmds = [..., "podman"]` (the runtime joins the cmd allowlist).
- `runtime = "podman"` added to **every** `[agents.sandbox]` block **and** the `[verify]` block.

Everything else (mounts, egress nets, proxies, creds, workflows) is byte-identical to the docker config. A header comment states the two-line rule so operators can convert their own configs.

**Why a separate file, not a doc note:** the runtime appears in N sandbox blocks; a ready-to-run file is less error-prone than "add `runtime = "podman"` to each block" prose, and it's the artifact the live smoke and the docs both point at.

### §2 — Podman egress bring-up

`compose.egress.yaml` uses Docker-Compose networks (`internal: true`) and two services built from `proxy.Containerfile`. Rather than depend on `podman-compose` (a third-party tool with its own version skew and quirks), ship a **hand-rolled, idempotent shell script** that uses podman primitives directly — the same surface the bridge itself uses:

`deploy/containers/podman-egress-up.sh` (+ a `…-down.sh`):
- `podman network create --internal a2a-egress-internal` (idempotent: ignore "already exists")
- `podman network create a2a-egress-external`
- `podman network create --internal a2a-verify-egress`
- `podman build -t a2a-egress-proxy:latest -f proxy.Containerfile .`
- `podman run -d --name a2a-egress-proxy --network a2a-egress-internal --restart unless-stopped a2a-egress-proxy:latest`, then `podman network connect a2a-egress-external a2a-egress-proxy` (podman attaches one network at `run`; additional nets via `network connect`).
- `podman run -d --name a2a-verify-proxy --network a2a-verify-egress -v ./tinyproxy.verify.filter:/etc/tinyproxy/filter:ro --restart unless-stopped a2a-egress-proxy:latest`, then `podman network connect a2a-egress-external a2a-verify-proxy`.

The script is the **tested** egress recipe; `podman-compose -f compose.egress.yaml up` is mentioned in docs as an untested convenience, not the supported path. The agent/verify images (`a2a-agent-reader`, `a2a-toolchain`) are built with `podman build` into podman's image store (separate from docker's) — the script (or docs) covers `podman build` for those too, since podman cannot see docker-built images.

### §3 — Runtime preflight (small code, recommended)

Add a boot-time check so a missing/unstarted runtime fails **loud and early** instead of deep in the first spawn.

- Pure helper `preflight_runtimes(runtimes: &BTreeSet<String>, probe: &dyn Fn(&str) -> bool) -> Result<(), BridgeError>` (injectable probe → unit-testable). It returns `ConfigInvalid { reason }` naming the first runtime whose probe fails, with a hint (`"… not found or not responding; is the runtime installed and (for podman) is 'podman machine' started?"`).
- The production probe runs `<runtime> version` with a short timeout; success = exit 0.
- Collect the distinct runtimes from the snapshot's sandbox blocks **and** the verify config; call the preflight once at boot in the `serve` / `run-workflow` / `implement` paths, **only when at least one sandboxed workload exists** (host-only configs skip it entirely — no runtime needed).
- This is the **only** Rust change in the slice; it is additive and runtime-neutral (it equally catches a missing docker).

### §4 — Docs

- `docs/containerized-agents.md`: a **Podman** section — the two-line config rule (§1), the egress script (§2), `podman build` for the images, the `podman machine` note for macOS, and the rootless caveats (§6).
- `docs/onboarding.md`: one line pointing at the podman config + section.

---

## §5 — Runtime-parity audit (docker-ism → podman)

| surface | bridge usage | podman | action |
| --- | --- | --- | --- |
| spawn | `run -i --rm --name --label -v --network <img> <cmd>` | identical | none |
| sweep | `ps -aq --filter name=…`, `ps -a --filter label=… --format {{…}}` | identical | none |
| reap | `rm -f <name>` | identical | none |
| staleness | `logs --since <win> --tail 1 <name>` | identical | none |
| `:ro`/`:rw` bind | `-v host:dst[:ro]` | identical | none |
| internal net | `--network a2a-egress-internal` (no gateway) | `network create --internal` | §2 script |
| uid on bind writes | container root → host user | **rootless podman: native userns remap** (Docker Desktop/OrbStack: VM remap) | none (handled by runtime); note in docs |
| writable creds mount | token-refresh writes back into the mounted creds | works on both | docs caveat |

No code path needs a docker-vs-podman branch; the parity is in the CLI surface, which is already parameterized.

---

## §6 — Validation (live smoke on this Mac = representative of the work Mac)

Because the operator's work machine is also a Mac, `podman machine` on this dev Mac is the representative environment (Mac host → podman VM → Linux containers), not Linux-rootful. The smoke runs here **after the operator frees host RAM** (pause Prism + stockTrading agents; stop the stockTrading dev stack — the host currently swaps ~7 GB, so a second VM needs that headroom).

Smoke checklist (operator + controller):
1. **Pre:** stockTrading dev stack stopped; `podman machine init` (modest size, e.g. 6 GiB / 6 CPU) + `podman machine start`.
2. **Images:** `podman build` the `a2a-agent-reader` (and `a2a-toolchain` if testing verify) into podman's store.
3. **Egress:** run `deploy/containers/podman-egress-up.sh`; confirm 3 networks + 2 proxies up (`podman ps`, `podman network ls`).
4. **Preflight:** `run-workflow` with a host-only config → preflight skipped; with the podman config but `podman machine` stopped → boot fails with the clear runtime error (§3).
5. **Spawn + egress-lock:** run a containerized `code-review` (or `design`) smoke with `…podman.toml` on a tiny input → readers spawn as podman containers; confirm an agent on `a2a-egress-internal` reaches the provider **only via the proxy** (egress lock holds — a direct-egress attempt fails).
6. **Reap:** after the run, `podman ps -a` shows no leaked `a2a-ro-*`/`a2a-rw-*`; the `containers` CLI lists/reaps under podman.
7. **Teardown:** `podman-egress-down.sh`; `podman machine stop`/`rm`. Operator restarts the stockTrading dev stack.

A failure at step 5 (egress leak) or 6 (leak) is a real conformance gap; a step-1/2 failure is environmental.

---

## §7 — Risks & mitigations

- **podman multi-network attach at `run`:** podman attaches a single `--network` at run; the proxies need two. *Mitigation:* `run` with the primary net, then `network connect` the second (§2) — explicit in the script.
- **Separate image store:** podman cannot see docker-built images. *Mitigation:* the script/docs build images with `podman build`; the smoke step 2 makes this explicit.
- **`podman machine` VM resource pressure on a 24 GB host:** *Mitigation:* gated on the operator stopping the stockTrading stack; podman-machine sized modestly; torn down right after.
- **podman-compose drift:** avoided by not depending on it (hand-rolled script is the supported path).
- **Rootless port/permission differences:** the bridge binds no host ports for agents (stdio ACP) and mounts under the user's home; rootless podman handles both. The proxies expose no host ports either (container-to-container only). Low risk; covered by the smoke.

---

## §8 — Testing

- **Unit:** `preflight_runtimes` — passes when all probes succeed; returns `ConfigInvalid` naming the first failing runtime; skips cleanly for an empty runtime set (host-only config). Injected probe; no real process.
- **Unit (parity, optional):** assert `compose_sandbox`/`compose_verify`/the sweep/reap argv builders emit the configured runtime verbatim as `argv[0]` when `runtime = "podman"` (most are already covered for `"docker"`; add a `"podman"` case).
- **Live smoke:** §6, on podman-machine here.
- No change to existing docker tests; docker remains the default and its paths are untouched.

---

## §9 — Out of scope / follow-ups

- **Memory slices** (next specs): (a) a bridge-wide concurrent-sandbox-workload semaphore (the measured root cause of OOM: overlapping runs inside a fixed VM); (b) configurable `--memory`/`--memory-swap` caps + a `CARGO_BUILD_JOBS` verify throttle (the "slower-not-OOM" backstop); (c) lowering the OrbStack VM ceiling once the bridge's peak is bounded (frees host RAM, ends the 7 GB host swap). Measured anchors for those: VM ceiling 17.59 GiB; stockTrading idle ~1.1 GiB; `:ro` reviewer ~0.54 GiB (container) vs ~0.47 GiB (host) → ~70 MB container overhead; verify build peak `-j15` 1.89 GiB vs `-j2` 1.03 GiB.
- **Disk:** the per-repo verify caches total ~67 GB; a `containers prune`/cache-GC command is a candidate follow-up.
