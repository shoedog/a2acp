# a2a-bridge

A single Rust binary that exposes a local CLI coding agent (**Kiro**, or any
ACP-speaking agent) as an **A2A**-compliant network service. Remote A2A callers
send tasks; the bridge drives the CLI agent over **ACP** (the Agent Client
Protocol, JSON-RPC/stdio) via a conformant SDK client and streams results back
over SSE.

Increment 3a (ADR-0004) replaced the hand-rolled ACP driver with a fully
conformant `AcpBackend` over the official `agent-client-protocol =0.12.1` SDK.
See `docs/superpowers/specs/2026-05-29-a2a-bridge-v1-design.md` for the design
and `docs/superpowers/plans/2026-05-30-a2a-bridge-v1.md` for the implementation
plan.

## Architecture

Hexagonal / ports-and-adapters. The domain-only `bridge-core` (Task/Session typestate,
all port traits, the translator) is driven by adapters; nothing in the core depends on an
adapter.

```
A2A caller ──HTTP/JSON-RPC/SSE──▶ bridge-a2a-inbound (axum)
                                      │  auth → route → translate → backend
                                      ▼
                                  bridge-core (Translator, typestate, ports)
                                      │  AgentBackend (streaming)
                                      ▼
                                  bridge-acp ──ACP/NDJSON/stdio──▶ kiro-cli acp
```

| Crate | Responsibility |
|-------|----------------|
| `bridge-core` | Domain types, `Task`/`Session` typestate, all port traits, the streaming translator (coalescer + anti-corruption rules) |
| `bridge-acp` | NDJSON frame reader, process supervisor (group-kill), Kiro `AgentBackend`, replay backend |
| `bridge-a2a-inbound` | A2A v1 JSON-RPC server (axum), Agent Card, SSE |
| `bridge-a2a-outbound` | `DelegationPort` stub (Increment 2.5) |
| `bridge-store` | SQLite `SessionStore` (task↔session + pending-request) |
| `bridge-policy` | `AutoPolicy` + `AlwaysGrant` auth (invoked seams) |
| `bridge-observ` | `tracing` setup + correlated task spans |
| `bin/a2a-bridge` | Composition root: config, routing, `main` |

## Protocol bindings

- **A2A v1** via the official `a2a` crate (package `a2a-lf` =0.3.0). Methods: `SendMessage`,
  `SendStreamingMessage`, `GetTask`, `CancelTask`, `SubscribeToTask`. Version pinned to
  `a2a::VERSION = "1.0"`; header `A2A-Version`.
- **ACP** via `agent-client-protocol` =0.12.1 (Apache-2.0,
  `github.com/agentclientprotocol/rust-sdk`) — the official SDK drives the full
  conformant lifecycle: `initialize` → `authenticate` → `session/new` →
  `session/set_mode` → `session/set_model` → `session/prompt` (streamed
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

Create `a2a-bridge.toml` (or rely on the built-in default):

```toml
[agent]
name = "kiro"          # display name; also the fan-out source label
cmd  = "kiro-cli"      # agent binary on PATH
args = ["acp"]         # arguments passed to cmd
# Optional ACP session settings (all default to absent / process cwd):
# model      = "gpt-4o"      # best-effort session/set_model (non-fatal if rejected)
# mode       = "read-only"   # hard session/set_mode (fatal if rejected)
# cwd        = "/work/dir"   # absolute working dir for session/new (defaults to current_dir)
# auth_method = "oauth"      # auth method id to use at initialize (defaults to first advertised)

[server]
addr = "127.0.0.1:8080"
```

| `[agent]` key | Required | Description |
|---|---|---|
| `name` | yes | Human name; also drives the fan-out source label in artifacts |
| `cmd` | yes | Agent binary to spawn (must be on PATH) |
| `args` | yes | Arguments passed to `cmd` (e.g. `["acp"]` for kiro-cli / codex-acp) |
| `model` | no | Model id for `session/set_model` (best-effort; ignored if agent rejects) |
| `mode` | no | Mode id for `session/set_mode` (hard error if agent rejects) |
| `cwd` | no | Absolute working directory for `session/new`; defaults to bridge's `current_dir()` |
| `auth_method` | no | Auth method id for `authenticate`; defaults to first method the agent advertises |

```bash
./target/release/a2a-bridge          # spawns `kiro-cli acp`, serves A2A on addr
```

The Agent Card is published at `GET /.well-known/agent-card.json`. `kiro-cli` must be
installed and authenticated on the host (`kiro-cli whoami`).

## Testing & coverage

```bash
cargo test --workspace                # ~200 tests, all in-process (no external agent)
```

Coverage is gated in CI (`cargo-llvm-cov`), enforced as a floor, measured per crate:

| Scope | Gate | Current (Increment 3a) |
|-------|------|---------|
| Workspace | ≥ 85% lines | ~94% |
| `bridge-core` (domain/typestate/translator) | ≥ 90% lines | ~99% |
| `bridge-acp` (conformant ACP client, supervisor) | ≥ 90% lines | ~95% |

```bash
cargo llvm-cov clean --workspace      # mandatory before measuring (stale-cache bug)
cargo llvm-cov --workspace --fail-under-lines 85
cargo llvm-cov -p bridge-core --fail-under-lines 90
cargo llvm-cov -p bridge-acp  --fail-under-lines 90
```

Typestate invariants (e.g. prompting a non-ready session, resuming a terminal task) are
proven uncompilable by `trybuild` compile-fail tests.

Wire conformance is verified by `tests/golden_frames.rs` (hand-authored expected
JSON for every outbound ACP frame) and `tests/corpus_replay.rs` (real captured
frames from `kiro-cli 2.5.0` and `codex-acp 0.15.0` fed through the live mapping
functions).

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
model; **conformant ACP client** (`agent-client-protocol` =0.12.1 SDK, bidirectional,
wire-golden-tested, live kiro-validated); **outbound delegation** (passthrough) and
**fan-out / second opinion** (Kiro + peer merged, source-labeled, degrade-to-survivor);
streaming with coalescing; cancellation (prompt-result semantics; both-source cancel on
inbound `CancelTask` and caller disconnect); permission/auth suspend→resume; **real
message content threaded to the agent and the peer**; process-group reaping; structured
tracing.

**Deferred:** multi-agent adapters beyond kiro/codex-acp (Claude Code/Gemini, Increment 3b+
— which generalizes the N-ary fan-out coordinator to >2 sources); real permission policy;
`session/load` resume; MCP-over-ACP;
fs/terminal client capabilities; JWT/mTLS enforcement; container isolation; multiple peers /
discovery / mesh; result reconciliation/voting.

### Known limitations (called out honestly; see ADR-0003, ADR-0004 + reviews)

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
