# a2a-bridge

A single Rust binary that exposes one or more local CLI coding agents (**Kiro**,
**codex-acp**, or any ACP-speaking agent) as an **A2A**-compliant network service.
Remote A2A callers send tasks; the bridge resolves the target agent from a
runtime-mutable registry, drives it over **ACP** (the Agent Client Protocol,
JSON-RPC/stdio) via a conformant SDK client, and streams results back over SSE.

Increment 3b (ADR-0005) added the agent registry: a hot-reloadable, runtime-mutable
`[[agents]]` config that supports live add/edit/remove of agents without restarting the
bridge. Increment 3a (ADR-0004) replaced the hand-rolled ACP driver with a fully
conformant `AcpBackend` over the official `agent-client-protocol =0.12.1` SDK.
See `docs/superpowers/specs/2026-05-31-a2a-bridge-v3b-design.md` for the 3b design
and `docs/superpowers/specs/2026-05-29-a2a-bridge-v1-design.md` for the v1 design.

## Architecture

Hexagonal / ports-and-adapters. The domain-only `bridge-core` (Task/Session typestate,
all port traits, the translator) is driven by adapters; nothing in the core depends on an
adapter.

```
A2A caller ──HTTP/JSON-RPC/SSE──▶ bridge-a2a-inbound (axum)
                                      │  auth → route → registry → translate → backend
                                      ▼
                                  bridge-core (Translator, typestate, ports)
                                      │  AgentRegistry / AgentBackend (streaming)
                                      ▼
                                  bridge-registry ──SpawnFn──▶ bridge-acp ──ACP/NDJSON/stdio──▶ kiro-cli / codex-acp
```

| Crate | Responsibility |
|-------|----------------|
| `bridge-core` | Domain types, `Task`/`Session` typestate, all port traits, the streaming translator (coalescer + anti-corruption rules) |
| `bridge-registry` | Runtime-mutable `Registry` over `ArcSwap<RegistryState>`; lazy-spawn via `SpawnFn`; atomic `apply()` reconcile; lease-draining async retirement |
| `bridge-acp` | NDJSON frame reader, process supervisor (group-kill), conformant `AcpBackend`, replay backend |
| `bridge-a2a-inbound` | A2A v1 JSON-RPC server (axum), Agent Card, SSE, task binding |
| `bridge-a2a-outbound` | `DelegationPort` stub (Increment 2.5) |
| `bridge-store` | SQLite `SessionStore` (task↔session + pending-request) |
| `bridge-policy` | `AutoPolicy` + `AlwaysGrant` auth (invoked seams) |
| `bridge-observ` | `tracing` setup + correlated task spans |
| `bin/a2a-bridge` | Composition root: `[[agents]]` config, `FileConfigSource`, registry wiring, `main` |

## Protocol bindings

- **A2A v1** via the official `a2a` crate (package `a2a-lf` =0.3.0). Methods: `SendMessage`,
  `SendStreamingMessage`, `GetTask`, `CancelTask`, `SubscribeToTask`. Version pinned to
  `a2a::VERSION = "1.0"`; header `A2A-Version`.
- **ACP** via `agent-client-protocol` =0.12.1 (Apache-2.0,
  `github.com/agentclientprotocol/rust-sdk`) — the official SDK drives the full
  conformant lifecycle: `initialize` → `authenticate` → `session/new` →
  `session/set_mode` → `session/set_config_option` (model + effort) →
  `session/prompt` (streamed
  `agent_message_chunk` → `PromptResponse`) → `session/cancel`. Reverse
  `request_permission` from the agent is handled bidirectionally via `PolicyEngine`.
  See ADR-0004.

Both SDK versions are pinned (`Cargo.lock` committed) and maintained per the
dependency-currency policy in the spec (§11.2).

## Build & run

Requires the pinned toolchain (`rust-toolchain.toml`, Rust 1.94.0).

```bash
cargo build --release
```

Create `a2a-bridge.toml` (or rely on the built-in default — the bridge materializes a
single-agent kiro config if the file is absent):

```toml
# Top-level default agent id — required. Must match one [[agents]] entry's id.
default = "kiro"

# Optional registry section. If absent, every entry's cmd is automatically allowed.
[registry]
allowed_cmds = ["kiro-cli", "codex-acp"]

# One [[agents]] table per agent. Add as many as needed.
[[agents]]
id   = "kiro"       # Caller-facing agent id; also used in a2a-bridge.agent metadata
cmd  = "kiro-cli"   # Agent binary (must be on PATH and in allowed_cmds if [registry] is set)
args = ["acp"]      # Arguments passed to cmd
# Optional per-agent ACP session settings:
# name         = "Kiro"           # Human-readable name (fan-out source label); defaults to id
# model        = "sonnet"         # Validated via session/set_config_option(model); hard-fails at mint if the agent doesn't advertise it (aliases: fable, opus)
# model_provider = "openai"      # LLM vendor label — descriptive only, never on the wire
# effort       = "high"          # Effort tier: minimal / low / medium / high / xhigh / max (model-dependent; falls back to highest supported ≤ requested)
# mode         = "read-only"     # Hard session/set_mode (fatal if agent rejects)
# cwd          = "/work/dir"     # Absolute working dir for session/new (defaults to current_dir)
# auth_method  = "oauth"         # Auth method id for authenticate (defaults to first advertised)

[[agents]]
id   = "codex"
cmd  = "codex-acp"
args = []
# name, model, effort, mode, cwd, auth_method — all optional

[server]
addr = "127.0.0.1:8080"
```

### `[[agents]]` entry keys

| Key | Required | Description |
|---|---|---|
| `id` | yes | Caller-facing agent id; used in `a2a-bridge.agent` request metadata and as `default` value |
| `cmd` | yes | Agent binary to spawn (must be on PATH; must be in `allowed_cmds` if `[registry]` is set) |
| `args` | no | Arguments passed to `cmd` (e.g. `["acp"]` for kiro-cli) |
| `name` | no | Human-readable display name; drives the fan-out source label in artifacts (defaults to `id`) |
| `model` | no | Model id set on the agent's advertised surface — `session/set_config_option(category="model")` for claude 0.44.0 / codex, or the unstable `models` + `session/set_model` for kiro (`auto`/`claude-sonnet-4.5`/…); **validated** against advertised values (hard-fails at mint if not advertised). Aliases resolve first (`fable`→`claude-fable-5[1m]`, `opus`→`default`) |
| `model_provider` | no | LLM vendor label — descriptive/routing metadata only, never sent on the wire |
| `effort` | no | Effort tier set via `session/set_config_option` for agents that advertise one (codex `reasoning_effort`, claude `effort`): `minimal` / `low` / `medium` / `high` / `xhigh` / `max`. Falls back to the highest supported level ≤ requested |
| `mode` | no | Mode id for `session/set_mode` (hard error if agent rejects) |
| `cwd` | no | Working directory for `session/new`; relative values are joined onto the bridge's `current_dir()` |
| `auth_method` | no | Auth method id for `authenticate`; defaults to first method the agent advertises |
| `description` | no | Human description (seamed for future per-entry Agent Cards) |
| `tags` | no | String tags (seamed for future per-entry Agent Cards) |
| `version` | no | Config version string (seamed for future per-entry Agent Cards) |

### `[registry]` section

| Key | Required | Description |
|---|---|---|
| `allowed_cmds` | no | Allowlist of binary names agents may use; if absent, defaults to the union of all entry `cmd` values |

### `[server]` section

| Key | Default | Description |
|---|---|---|
| `addr` | `127.0.0.1:8080` | TCP address the bridge listens on |

```bash
./target/release/a2a-bridge    # loads a2a-bridge.toml, serves A2A on addr
```

The Agent Card is published at `GET /.well-known/agent-card.json`. Each configured agent
binary must be installed and authenticated on the host (e.g. `kiro-cli whoami`).

### Hot-reload (no restart required)

The bridge watches `a2a-bridge.toml`'s parent directory for changes using `notify`
(atomic-rename–safe: editors that save by write-then-rename are handled correctly). When
the file changes:

1. A 200 ms debounce window settles any burst of filesystem events.
2. The file is re-read and re-parsed.
3. On **success**: `Registry::apply()` reconciles the new snapshot atomically —
   config-only edits (same `cmd`/`args`/`cwd`/`auth_method`) reuse the warm backend
   with no respawn; cmd/args changes replace the slot; removed agents are retired
   (lease-draining: in-flight tasks finish before the backend is shut down).
4. On **parse error**: the error is logged and the last-good snapshot is kept. The bridge
   does not go down.

Hot-reload was validated live: a model edit to a running registry took effect on the next
new session with no respawn (`Arc::ptr_eq` warm-backend reuse proven against kiro-cli
2.5.0 + codex-acp 0.15.0 simultaneously). See ADR-0005.

### Breaking config change: `[agent]` → `[[agents]]` + `default =`

**Increment 3b replaces the Increment 3a `[agent]` config schema.** Old configs will
fail with a TOML parse error on startup. To migrate:

```toml
# Before (Increment 3a and prior — NO LONGER VALID):
[agent]
name = "kiro"
cmd  = "kiro-cli"
args = ["acp"]
model = "gpt-4o"

[server]
addr = "127.0.0.1:8080"

# After (Increment 3b):
default = "kiro"

[[agents]]
id   = "kiro"
cmd  = "kiro-cli"
args = ["acp"]
model = "gpt-4o"

[server]
addr = "127.0.0.1:8080"
```

## Testing & coverage

```bash
cargo test --workspace                # ~200 tests, all in-process (no external agent)
```

Coverage is gated in CI (`cargo-llvm-cov`), enforced as a floor, measured per crate:

| Scope | Gate | Current (Increment 3b) |
|-------|------|---------|
| Workspace | ≥ 85% lines | ~94% |
| `bridge-core` (domain/typestate/translator/ports) | ≥ 90% lines | ~98% |
| `bridge-registry` (registry, reconcile, retirement) | ≥ 90% lines | ~93% |

```bash
cargo llvm-cov clean --workspace          # mandatory before measuring (stale-cache bug)
cargo llvm-cov --workspace --fail-under-lines 85
cargo llvm-cov -p bridge-core     --fail-under-lines 90
cargo llvm-cov -p bridge-registry --fail-under-lines 90
```

Typestate invariants (e.g. prompting a non-ready session, resuming a terminal task) are
proven uncompilable by `trybuild` compile-fail tests.

Wire conformance is verified by `tests/golden_frames.rs` (hand-authored expected
JSON for every outbound ACP frame) and `tests/corpus_replay.rs` (real captured
frames from `kiro-cli 2.5.0` and `codex-acp 0.15.0` fed through the live mapping
functions).

### Per-request agent selection and overrides

Send per-request metadata keys in the A2A `SendMessage` or `SendStreamingMessage`
`message.metadata` object to select the agent and override its model/effort/mode for
that request only. All keys are optional and orthogonal to each other.

| Metadata key | Type | Description |
|---|---|---|
| `a2a-bridge.agent` | string | Agent id to route to (must match an `[[agents]]` entry `id`). Absent → registry `default` |
| `a2a-bridge.model` | string | Model id override for this request's ACP session (agent-native id, passed verbatim) |
| `a2a-bridge.effort` | string | Effort tier override: `minimal` / `low` / `medium` / `high` / `max` |
| `a2a-bridge.mode` | string | Mode id override for `session/set_mode` (hard error if agent rejects) |
| `a2a-bridge.skill` | string | Routing skill: `delegate` (outbound peer) or `fan-out` (default + peer concurrently) |

Override keys layer on top of the entry's defaults for the selected agent. An invalid
`a2a-bridge.agent` (unknown id or empty string) or invalid `a2a-bridge.effort` value
returns a clean JSON-RPC `InvalidRequest` error to the caller.

Example request JSON-RPC payload:

```json
{
  "message": {
    "text": "PING",
    "metadata": {
      "a2a-bridge.agent": "codex",
      "a2a-bridge.model": "gpt-5.5",
      "a2a-bridge.effort": "high"
    }
  }
}
```

### Gated real-agent e2e tests (ACP conformant client)

Two gated end-to-end tests drive the conformant `AcpBackend` directly against a
real agent (`#[ignore]`-gated, not in default CI):

**kiro-cli** (gate MET — run and passing against kiro-cli 2.5.0):

```bash
cargo test -p a2a-bridge --test e2e_acp_kiro -- --ignored --nocapture
# Prereqs: kiro-cli on PATH and authenticated (kiro-cli whoami), network access
```

This test spawns a real `kiro-cli acp` process, drives the full conformant
lifecycle (`initialize` → `session/new` → `session/prompt`), asserts the streamed
text contains `PONG` and the turn ends with `end_turn`. This was run against
kiro-cli 2.5.0 and passed — the kiro DoD gate is MET.

**codex-acp** (gate MET — run and passing against zed-industries/codex-acp 0.15.0):

```bash
cargo test -p a2a-bridge --test e2e_acp_codex -- --ignored --nocapture
# Prereqs: codex-acp on PATH and authenticated; codex-acp is distinct from codex-cli
```

This test spawns a real zed-industries/codex-acp 0.15.0 process, drives the full
conformant lifecycle (`initialize` → `authenticate` → `session/new` →
`session/set_mode` → `session/prompt`), and yielded streamed `PONG` (across two
`agent_message_chunk` frames) and `end_turn` — the codex DoD gate is MET. The real
captured round-trip lives in `tests/corpus/codex-acp.jsonl` (`_provenance:REAL-CAPTURE`)
and replays through the live mapping functions; the `real_capture_corpus_present` test
now passes (un-ignored) since both kiro-cli and codex-acp have real captures. codex-acp
emits a few unmodeled `session/update` variants (`available_commands_update`,
`config_option_update`, `usage_update`) which the tolerant reader drops.

### Gated multi-agent registry e2e test (Increment 3b)

The registry e2e test validates the full 3b story: two real agents registered
simultaneously, per-request routing by id, per-request override, and live
config-only model edit taking effect on a new session with no respawn.

```bash
cargo test -p a2a-bridge --test e2e_registry -- --ignored --nocapture
# Prereqs: BOTH kiro-cli (authenticated) AND codex-acp on PATH and authenticated
```

This test:

1. Starts the full bridge stack (registry + inbound server + ACP backends) in-process.
2. Sends a `SendStreamingMessage` with `a2a-bridge.agent=kiro` — asserts `PONG` from kiro-cli.
3. Sends a `SendStreamingMessage` with `a2a-bridge.agent=codex` — asserts `PONG` from codex-acp.
4. Sends with a per-request `a2a-bridge.model` override — asserts the override was applied.
5. Edits the registry config (model-only change on kiro's entry) and calls `apply()` — asserts
   that `Arc::ptr_eq` confirms the **same slot instance** was reused (no respawn), and that the
   new model is live on the next session.

Gate MET: run live against kiro-cli 2.5.0 + codex-acp 0.15.0 — both returned `PONG`,
per-request routing and overrides worked, and the warm-backend reuse was proven. See ADR-0005.

### Original gated smoke (pre-3a, v1 inbound pipeline)

```bash
cargo test -p a2a-bridge --test e2e_kiro -- --ignored --nocapture   # needs kiro-cli whoami
```

### Outbound delegation (Increment 2.5)

The bridge can forward a task to a configured remote A2A peer: send a `SendStreamingMessage`
with `metadata["a2a-bridge.skill"]="delegate"`, and the bridge POSTs it to the peer and
streams the peer's `StreamResponse` events back to the caller. Configure one peer:

```toml
[delegation]
peer_url    = "https://peer.example/"
auth        = "bearer:${PEER_TOKEN}"   # ${ENV} expanded; missing var = error
timeout_secs = 60                       # optional
```

Without a `[delegation]` section the bridge runs local-only (delegation is a no-op). A gated
bridge-to-bridge e2e (one bridge's `delegate` skill → another bridge's `kiro-code`) is
`#[ignore]`d: `cargo test -p a2a-bridge --test e2e_delegate_bridge -- --ignored`.

### Fan-out / second opinion (Increment 2.6)

Send a `SendStreamingMessage` with `metadata["a2a-bridge.skill"]="fan-out"` to run the **same
prompt on both Kiro and the configured peer concurrently**, merged into one SSE. Every frame
is source-labeled (`metadata["a2a-bridge.source"]="kiro"|"peer"`, and `artifact.name` on
artifacts); both run to completion (two labeled artifacts) and the task ends with one terminal
`StatusUpdate`. **Degrade-to-survivor:** if one source fails, its labeled error frame plus the
survivor's result still come back and the task completes (`Completed`); only if both fail does
it `Fail`. Cancel/disconnect cancels both sources. Requires a `[delegation]` peer. Gated e2e:
`cargo test -p a2a-bridge --test e2e_fanout_bridge -- --ignored`.

### A2A terminal model

A streamed task ends with a terminal `StatusUpdate` (`Completed`/`Failed`/`Canceled`) — not an
artifact's `lastChunk` (which marks only that artifact's completion). This holds for single-
source and fan-out alike, so a task can carry multiple artifacts before its terminal status.

## What the bridge does / doesn't do

**In:** inbound A2A with **A2A-conformant `StreamResponse` SSE** + a terminal-status task
model; **runtime-mutable agent registry** (hot-reload, per-request agent selection and
model/effort/mode overrides, lease-draining retirement, task binding); **conformant ACP
client** (`agent-client-protocol` =0.12.1 SDK, bidirectional, wire-golden-tested, live
kiro + codex validated); **outbound delegation** (passthrough) and **fan-out / second
opinion** (Kiro + peer merged, source-labeled, degrade-to-survivor); streaming with
coalescing; cancellation (prompt-result semantics; both-source cancel on inbound
`CancelTask` and caller disconnect); permission/auth suspend→resume; **real message
content threaded to the agent and the peer**; process-group reaping; structured tracing.

**Deferred:** per-entry A2A AgentCards (Option-3); admin HTTP API + `ConfigStore`
write-back (3b.2); DB/remote `ConfigSource` adapters; fan-out across the registry (3d);
conductor fork/continue decision (post-3c); real permission policy; `session/load` resume;
MCP-over-ACP; fs/terminal client capabilities; JWT/mTLS enforcement; container isolation;
multiple peers / discovery / mesh; result reconciliation/voting.

### Known limitations (called out honestly; see ADR-0003, ADR-0004, ADR-0005 + reviews)

- **The `Task`/`Session` typestate is a compile-time spec artifact, not yet load-bearing.**
  It is `trybuild`-verified but the runtime pipeline does not yet route through
  `Session<Ready>::send_prompt`. The seam is preserved for later wiring.
- **Coalescing is char-cap only** (1200 chars + boundary flush); the 200 ms idle-flush half
  of the spec contract is not yet implemented.
- **The running binary uses an in-memory SQLite store** (`open_in_memory`), so persisted
  state (pending-request resume, delegated-task mapping) is not durable across restart. The
  store seam supports a file-backed DB; wiring it is a one-line change.
- **Agent Card path:** served at `/.well-known/agent-card.json`; the published A2A v1
  standard may use `/.well-known/agent.json` — verify before claiming external conformance.
- **Outbound passthrough only; caller identity is not forwarded** to the peer (a configured
  bearer is presented instead) — identity propagation is a later concern.

## License

Apache-2.0.
