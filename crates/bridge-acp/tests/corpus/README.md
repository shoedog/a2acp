# Captured-agent frame corpus — DoD gate status

This directory holds the captured-real-agent ACP frame corpus that the Increment 3a
**Definition-of-Done gate** depends on. Each `<agent>.jsonl` file is a sequence of
JSON-RPC messages, one per line, each wrapped as `{"dir": "send"|"recv", "line": <msg>}`.
The first line of every file is a `_provenance` header.

The corpus replay tests (`tests/corpus_replay.rs`) feed each **recv** frame (agent→bridge
direction) through `AcpBackend`'s REAL SDK parse + map path (`map_session_update`,
`decide_permission`, the prompt-result `stop_reason` mapping) and assert the resulting
`Update`/reply. This is the same code the live connection runs, so a real captured frame
is a real conformance proof — not the v1 circular one.

## GATE STATUS (per agent)

| agent      | real capture? | provenance                  |
|------------|---------------|-----------------------------|
| kiro-cli   | **YES — MET** | `REAL-CAPTURE` (v2.5.0)     |
| codex-acp  | **NO — UNMET**| `provisional-from-spec-§11A`|

- **kiro-cli — GATE MET.** `kiro-cli.jsonl` is a real round-trip captured from
  `kiro-cli acp` 2.5.0 in this environment (initialize → session/new → session/prompt →
  real `agent_message_chunk` → real `stopReason:end_turn` result). The inbound frames
  replay correctly through `AcpBackend`.

- **codex-acp — GATE UNMET.** `codex-acp.jsonl` is HAND-AUTHORED provisional scaffolding.
  `codex-acp` is NOT installed here: `codex-cli 0.130.0` is present but exposes no `acp`
  subcommand and no `codex-acp` binary. To CLOSE the codex gate, capture REAL frames from
  `codex-acp` (T9 gated e2e or a manual run) and replace `codex-acp.jsonl`, flipping its
  `_provenance` to `REAL-CAPTURE`.

The `real_capture_corpus_present` test in `tests/corpus_replay.rs` scans every file for a
`REAL-CAPTURE` provenance header. It is `#[ignore]`d precisely BECAUSE it does not yet pass
for all agents (codex is unmet) — running it (`cargo test -- --ignored
real_capture_corpus_present`) prints exactly which agents still need a real capture, so CI
can never imply the gate is met when it isn't.
