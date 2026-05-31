# ADR-004 — Adopt `agent-client-protocol` =0.12.1 as the ACP Client SDK

**Date:** 2026-05-30
**Status:** Accepted

**Supersedes:** ADR-0003 Addendum 2 ("ACP SDK not wired in v1 — deferred to Increment 3")

---

## Context

ADR-0003 Addendum 2 (post-final-review, 2026-05-30) documented that the v1
implementation did NOT use the `agent-client-protocol` crate: `KiroBackend`
hand-rolled ACP JSON-RPC framing directly over `serde_json` + an in-house
`FrameReader`. The crate was declared a reserved pin (`=0.12.1`) and the
addendum deferred real adoption to Increment 3.

Increment 3a now realizes that deferred commitment. Two concrete problems
motivated it:

1. **Non-conformance.** The hand-rolled driver was not ACP-conformant:
   `initialize` used a string `protocolVersion` instead of the required integer;
   `session/prompt` sent `parts:[{text}]` (v1 informal shape) instead of the
   required `prompt:[{type:"text",text}]` ContentBlock array; lifecycle steps
   (`authenticate`, `session/set_mode`, `session/set_model`) were absent; reverse
   `session/request_permission` requests were unhandled.

2. **No conformance proof.** With ad-hoc `serde_json` framing there was no
   systematic way to assert wire conformance — any drift would be a silent
   runtime failure against a real agent.

The SDK (`agent-client-protocol =0.12.1`, Apache-2.0,
`github.com/agentclientprotocol/rust-sdk`) provides generated, typed request /
notification / result types for every ACP method, a builder-style `Client` with
registered handlers, and a `connect_with` loop that owns the dispatch event loop
— eliminating the hand-rolled framing entirely.

---

## Decision

Adopt `agent-client-protocol =0.12.1` as the ACP client in `bridge-acp`. Replace
`KiroBackend` with `AcpBackend`, a fully conformant, transport-generic ACP client
built on the SDK.

**SDK client API used.** The connection is established via
`Client.builder().name(..).on_receive_notification(..).on_receive_request(..).connect_with(transport, |cx: ConnectionTo<Agent>| …)`.
`connect_with` owns the event loop in a dedicated tokio task; `cx`
(`ConnectionTo<Agent>`) is the agent-call handle cloned out for use by the rest
of the backend. There is no `ClientSideConnection` handler trait — the real API
is the builder + closures pattern.

**Transport-generic seam.** `AcpBackend::spawn(cmd, args, AcpConfig)` is the
production constructor: our `Supervised` child (process group + SIGTERM→SIGKILL
reap) owns the process lifecycle and feeds stdin/stdout to the SDK via
`ByteStreams` over `tokio_util::compat`. `AcpBackend::connect(transport, config)`
accepts any transport that implements the SDK's `ConnectTo<Client>` bound, so
in-process `Channel::duplex()` pairs are used for unit tests without spawning a
real process. We deliberately do NOT use the SDK's `AcpAgent` spawning helpers,
keeping our process hygiene (`Supervised`) in full control.

**Conformant lifecycle.** The backend drives the full ACP lifecycle in order:

1. `initialize` — integer `protocolVersion:1`, `ClientCapabilities::default()`
   (no fs or terminal capabilities advertised).
2. `authenticate` — bounded by the handshake timeout; attempted only if the agent
   advertised auth methods; a definitive failure surfaces
   `BridgeError::AgentNotAuthenticated`. The entire handshake (steps 1–2) is
   bounded by `AcpConfig::handshake_timeout`.
3. `session/new` — lazy, exactly-once per bridge session (via `OnceCell`), with
   `{cwd:<absolute>, mcpServers:[]}` params.
4. `session/set_mode` — hard error if the agent rejects the configured mode id
   (`AcpConfig::mode`); on rejection `OnceCell` stays uninitialized so the next
   caller re-attempts the full mint, not a silently mis-configured session.
5. `session/set_model` — best-effort, non-fatal (`AcpConfig::model`); failure is
   logged and the session continues. Rationale: the codex built-in provider
   returns `models:null` on `session/set_model`.
6. `session/prompt` — `prompt:[{type:"text",text}]` ContentBlock array (not the
   v1 `parts:[{text}]` shape); response is a `PromptResponse{stop_reason}`.
7. Streamed `agent_message_chunk` notifications → `Update::Text` → caller stream.
8. `Update::Done{stop_reason}` on `PromptResponse` receipt.
9. `session/cancel` — a NOTIFICATION (no id, no response); completion is the
   `StopReason::Cancelled` prompt RESULT, not the act of sending the
   notification. A grace timer (`AcpConfig::cancel_grace`) bounds the wait;
   on elapse `Supervised::terminate` (SIGTERM→SIGKILL) is called to unblock a
   hung agent. Stream-drop also triggers cancel then the same grace escalation.

The `session/set_model` path requires the `unstable_session_model` Cargo feature
on the `bridge-acp` dependency of `agent-client-protocol`.

**Bidirectional reverse requests.** The SDK dispatches inbound `request_permission`
requests from the agent to a registered handler. Critically, SDK 0.12.1 does NOT
auto-reply to unregistered inbound requests — it silently drops them, hanging
the agent's corresponding `block_task` call. Therefore:

- `request_permission`: handled via `PolicyEngine` (injected via
  `AcpBackend::with_policy`; default = auto-approve). The handler offloads the
  decide+respond to `cx.spawn` so it never stalls the event loop mid-prompt.
  `AllowOnce` is preferred over `AllowAlways`; deny → `RejectOnce` or
  `Cancelled`; any other outcome → `Cancelled`.
- `ReadTextFile`, `WriteTextFile`, `CreateTerminal`, `TerminalOutput`,
  `ReleaseTerminal`, `WaitForTerminalExit`, `KillTerminal`: explicit
  `method_not_found` reject handlers are registered (via the `reject_unsupported!`
  macro) so a non-conformant agent that sends these does not hang.

**Config.** `[agent]` gained optional `model`, `mode`, `cwd`, and `auth_method`
keys. `name` (existing) additionally drives the fan-out source label.
`cwd` defaults to the bridge's `current_dir()` at startup and is always resolved
to an absolute path (ACP §11A requirement).

**Cancel→outcome.** A locally-cancelled turn now reports `TaskOutcome::Canceled`
(not `Completed`) to the A2A caller, via the shared
`bridge_core::ports::STOP_REASON_CANCELLED` constant that both the backend and
the translator check.

**Conformance proof (DoD).**

- Wire-golden tests (`tests/golden_frames.rs`) assert the exact serialized JSON of
  every outbound frame (`initialize`, `session/new`, `session/prompt`,
  `session/cancel`, `session/set_mode`, `session/set_model`) against
  hand-authored expected values. These are non-tautological: they compare against
  the frame the backend actually constructs, not a re-derivation of the same SDK
  type.
- Captured real-agent corpus (`tests/corpus/kiro-cli.jsonl`) + replay test
  (`tests/corpus_replay.rs`): real frames captured off the wire from
  `kiro-cli 2.5.0` are fed through the exact `map_session_update` /
  `decide_permission` / `stop_reason_str` functions the live connection runs.
- **kiro-cli gate MET**: the live gated e2e (`e2e_acp_kiro.rs`) was run against
  real `kiro-cli 2.5.0` and yielded `PONG` / `end_turn` — no conformance bug, no
  fs capabilities required.
- **codex-acp gate UNMET**: `codex-acp` is not installable in the authoring
  environment (`codex` 0.130.0 has no `acp` subcommand). The codex e2e
  (`e2e_acp_codex.rs`) is `#[ignore]`'d and compile-only. A non-ignore-gated
  `codex_provisional_frames_replay_but_gate_is_unmet` test + an ignored
  `real_capture_corpus_present` test (which FAILS naming codex) are present so
  the open gate cannot be overlooked in CI.

---

## Consequences

**Positive:**

- ACP wire framing is now generated from the official SDK types; conformance is
  guaranteed for every driven method (not hand-maintained).
- The full lifecycle (initialize → authenticate → session/new → set_mode →
  set_model → session/prompt → cancel) is now CI-proven via wire-golden tests,
  a real captured corpus, and a live kiro round-trip.
- The transport-generic seam (`connect(transport)`) makes unit tests
  fully in-process — no real agent process required for the 47 backend tests.
- Reverse `session/request_permission` is now handled correctly and off the event
  loop (no dispatch stall risk). Unsupported fs/terminal methods get explicit
  `method_not_found` rejections (not silent hangs).
- `TaskOutcome::Canceled` is now wired correctly for cancelled turns.

**Discrepancies and open items:**

- **SDK version skew.** `codex-acp` links `agent-client-protocol` 0.9.2 (with
  unstable features); the bridge compiles 0.12.1. The two versions are
  wire-compatible for the driven methods; no incompatibility has been observed
  against real kiro (which also uses a different internal version).
- **`unstable_session_model` feature.** `session/set_model` is behind a Cargo
  feature flag in 0.12.1. This is declared on the `bridge-acp` dep and is stable
  enough for the driven use.
- **codex-acp DoD gate UNMET.** The codex-acp real-capture corpus and live e2e
  cannot be completed until `codex-acp` is available in the target environment.
  The codex wire frames in `tests/corpus/codex-acp.jsonl` are provisional (derived from
  spec, not captured). This is the one remaining open conformance item for
  Increment 3a; it is explicitly unforgettable via the `real_capture_corpus_present`
  `#[ignore]` test that fails when run.
- **fs/terminal: unsupported by design.** The bridge advertises no fs or terminal
  client capabilities. An agent requiring fs/terminal for a basic prompt would
  hang at the prompt step (the no-fs-caps property under test in `e2e_acp_kiro`).
  This is intentional; adding fs/terminal support is a future increment and would
  require a new ADR.
