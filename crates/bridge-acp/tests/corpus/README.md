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

| agent            | real capture? | provenance                        |
|------------------|---------------|-----------------------------------|
| kiro-cli         | **YES — MET** | `REAL-CAPTURE` (v2.5.0)           |
| codex-acp        | **YES — MET** | `REAL-CAPTURE` (v0.15.0)          |
| gemini-cli       | **YES — MET** | `REAL-CAPTURE (v0.41.2)`          |
| claude-agent-acp | **YES — MET** | `REAL-CAPTURE (v0.39.0)`          |

- **kiro-cli — GATE MET.** `kiro-cli.jsonl` is a real round-trip captured from
  `kiro-cli acp` 2.5.0 in this environment (initialize → session/new → session/prompt →
  real `agent_message_chunk` → real `stopReason:end_turn` result). The inbound frames
  replay correctly through `AcpBackend`.

- **codex-acp — GATE MET.** `codex-acp.jsonl` is a real round-trip captured from
  zed-industries/codex-acp 0.15.0 (initialize → authenticate(chatgpt) → session/new →
  set_mode(read-only) → session/prompt → 2× real `agent_message_chunk` → real
  `stopReason:end_turn` result). The agent streamed `PONG` across two chunks (`"P"` +
  `"ONG"`) and emitted several unmodeled `session/update` variants
  (`available_commands_update`, `config_option_update`, `usage_update`); the inbound
  frames replay correctly through `AcpBackend`, the chunks join to `PONG`, and the
  unmodeled updates are DROPPED. The live `e2e_acp_codex` round-trip also passed against
  this agent (PONG / end_turn). Note: `usage_update` is NOT a variant of the SDK 0.12.1
  `SessionNotification` type, so it fails SDK deserialization — and is dropped exactly as
  the live SDK dispatch drops it (parse-error → `send_error_notification`, connection
  continues), which the replay path mirrors.

- **gemini-cli — GATE MET.** `gemini-cli.jsonl` is a real round-trip captured from
  `gemini --acp` 0.41.2 (initialize → authenticate(oauth-personal)={} → session/new →
  session/prompt → real `agent_message_chunk` → real `stopReason:end_turn` result). The
  inbound frames replay correctly through `AcpBackend`, the single chunk equals `PONG`,
  and the two extra `session/update` variants emitted by gemini are correctly DROPPED at
  the map layer. Specifically: `available_commands_update` IS a modeled
  `SessionUpdate::AvailableCommandsUpdate` variant in the SDK (distinct from codex's
  genuinely-unmodeled `usage_update` which fails SDK deserialization entirely) — it
  deserializes as `SessionNotification` but `AcpBackend::map_session_update` returns
  `None` because it carries no assistant text. Gemini also emits a modeled
  `agent_thought_chunk` reasoning frame (a `SessionUpdate::AgentThoughtChunk` variant)
  that is likewise dropped at the map layer — it replays to `None`, never producing a
  `Text` update. Both drops are guarded by the `gemini_available_commands_update_is_modeled_not_parse_error`
  test and the `Some(other) => panic!` arm in `gemini_real_capture_replays_through_backend`.

- **claude-agent-acp — GATE MET.** `claude-agent-acp.jsonl` is a real round-trip captured
  from `claude-agent-acp` 0.39.0 on the Pro/Max subscription (Haiku model, `session/set_model`
  verified). The agent replied with a single `agent_message_chunk` chunk (`""` + `"PONG"` →
  concat `"PONG"`) and terminated with `stopReason:end_turn`. The capture also emits several
  unmodeled `session/update` variants — `available_commands_update`, `current_mode_update`,
  `config_option_update`, `usage_update`, and `agent_thought_chunk` — all dropped at the map
  layer (`map_session_update` returns `None`). The inbound frames replay correctly through
  `AcpBackend`.

The `real_capture_corpus_present` test in `tests/corpus_replay.rs` scans every file for a
`REAL-CAPTURE` provenance header. All four agents now have real captures, so it is a
normal (non-ignored) test that PASSES. If any corpus is ever regressed back to provisional
scaffolding, the default `cargo test` run fails naming exactly which agent lost its real
capture, so CI can never imply the gate is met when it isn't.
