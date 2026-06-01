# A2A Bridge — Claude → claude-agent-acp migration (+ retire bridge-claude) Design

**Goal:** Migrate Claude from the hand-rolled `bridge-claude` warm-CLI backend (Increment 3c) to the **official `@agentclientprotocol/claude-agent-acp`** driven by the existing conformant `AcpBackend` (a `kind="acp"` registry entry, like gemini/codex/kiro), and **retire `bridge-claude`** entirely. The warm/cache-hot benefit is preserved (spike-proven), so this is a net simplification, not a regression — one atomic increment so `main` never holds both the new ACP Claude and the dead `bridge-claude`.

**Architecture:** Claude becomes a plain ACP agent. `@agentclientprotocol/claude-agent-acp` v0.39.0 (official; ACP org) spawns the `claude` Code CLI under the hood and keeps it **warm per ACP session** (one persistent process, streaming-input mode, prompt cache hot across turns — verified live), reusing the Pro/Max **subscription** with NO API key (the old subscription-block is behind `--hide-claude-auth`, which we do NOT pass). It speaks ACP protocolVersion 1 over newline-delimited JSON-RPC — identical transport to the agents `AcpBackend` already drives. So Claude needs **no new backend code**; it's a registry config entry plus the retirement of the bespoke crate.

**Tech stack:** No new Rust deps. `@agentclientprotocol/claude-agent-acp` 0.39.0 (Node; install globally so `claude-agent-acp` is on PATH, like `codex-acp`). Reuses `bridge-acp`/`AcpBackend`, `bridge-registry`, the gated-e2e + corpus harness.

**Spec status:** brainstormed; design approved; probe-grounded (live `claude-agent-acp` probes, 2026-06-01).

> **COST NOTE (this dev slice):** every `claude-agent-acp` invocation in this increment — the live gate, the corpus capture, any probe — uses **`model="haiku"`** (the cheapest model) to save token cost. Production users can pick `default`/`sonnet`/`haiku` per their config; only THIS development slice is pinned to Haiku.

---

## 1. Probe-pinned facts (the design's evidence)

Live `claude-agent-acp` 0.39.0 probes (2026-06-01), `ANTHROPIC_API_KEY` and `CLAUDE_CODE_OAUTH_TOKEN` UNSET, against the subscription-logged-in `claude` 2.1.159:

- **Warm-per-session, cache-hot (the decisive fact):** per ACP session the adapter creates one long-lived SDK `query({ prompt: <streaming input>, options })` and pushes successive `session/prompt`s into the SAME stream; the SDK spawns the `claude` child ONCE per session with `--input-format stream-json --output-format stream-json --verbose` (no `--print`). Live 2-turn probe: same `claude` PID across turns, turn 2 answered "7" (context retained), and turn 2 reported `cache_read_input_tokens: 23234` from the 1-hour ephemeral cache tier. This is exactly the property 3c's warm-pool existed to preserve.
- **Subscription auth, no key:** completed a full prompt turn with no auth env set. The adapter inherits the CLI's auth (precedence: `ANTHROPIC_API_KEY` → `CLAUDE_CODE_OAUTH_TOKEN` → cached CLI session). The subscription-reject path exists only behind `--hide-claude-auth` (off by default).
- **`initialize` to OUR client-capability shape** (`fs:{readTextFile:false,writeTextFile:false}, terminal:false`) → `protocolVersion: 1`, **`authMethods: []`** (empty), agentInfo `@agentclientprotocol/claude-agent-acp` 0.39.0. Empty authMethods → `AcpBackend` skips `authenticate` (ambient auth, like kiro) → **`auth_method = None`**.
- **`session/new`** → `sessionId` + permission modes (`auto` default, `bypassPermissions` available) + models: `default` (Opus 4.8 1M, recommended), `sonnet`, `sonnet[1m]`, **`haiku`**. So `set_model("haiku")` is valid (used this slice).
- **Reverse requests / fs:** the adapter issues `fs/read_text_file`, `fs/write_text_file`, and `session/request_permission` to the client — but **gates the fs calls on the client's advertised fs capability**. `AcpBackend` advertises NO fs/terminal → the adapter will not send `fs/*`. `session/request_permission` → `AutoPolicy` auto-approves (same as kiro/codex/gemini). `session/update` variants: `available_commands_update`, `usage_update`, `agent_message_chunk`, plus tool/plan/mode updates — all handled by the existing tolerant reader (`map_session_update` maps only `AgentMessageChunk` text).

## 2. The Claude registry entry

Registered like kiro/codex/gemini — a `kind="acp"` entry the existing factory spawns via `AcpBackend`:

| Field | Value | Rationale |
|---|---|---|
| `id` | `claude` | route-by-id target |
| `kind` | `acp` (default) | existing `AcpBackend` path |
| `cmd` | `claude-agent-acp` | the adapter bin (installed globally; do NOT pass `--hide-claude-auth`) |
| `args` | `[]` | none needed |
| `auth_method` | `None` | empty authMethods → ambient subscription (no `authenticate`) |
| `model` | `haiku` | **this dev slice** (cheap); production picks default/sonnet/haiku |
| `mode` | unset | default `auto` permission mode → `AutoPolicy` auto-approves |
| `allowed_cmds` | add `"claude-agent-acp"` | the `cmd`-allowlist gate |

No domain change to register it (Claude is a process/exec ACP agent). Subscription auth is ambient (local `~/.claude` session); headless `CLAUDE_CODE_OAUTH_TOKEN` env support is a noted non-goal (§5).

## 3. Retire `bridge-claude` (3c hand-rolled warm backend)

**Delete:**
- the entire `crates/bridge-claude/` crate (`config`, `wire`, `proc`, `backend`, `reaper`, all its tests) + its workspace membership.
- `bin/a2a-bridge/tests/e2e_claude.rs` (the 3c inbound e2e).
- the `bridge-claude` dependency in `bin/a2a-bridge/Cargo.toml`.
- the `AgentKind::ClaudeCli` factory arm in `bin/a2a-bridge/src/main.rs` (the `match entry.kind` collapses to the `Acp` arm only).
- `parse_kind`'s `"claude-cli"` arm + its config test (`config.rs`) — `parse_kind` now accepts only `"acp"`, errors otherwise.
- the `AgentKind::ClaudeCli` references in the `bridge-core` domain tests (`agent_entry_carries_kind` / the literal using `ClaudeCli`) and the `bridge-registry` `kind_change_forces_fresh_slot` test.

**ADR-0006** records the supersession: 3c built a hand-rolled warm-CLI backend because the ACP-Claude adapter then appeared to require an API key; re-investigation (2026-06-01) found `claude-agent-acp` runs warm-per-session on the subscription, so Claude moves to the proven `AcpBackend` path and `bridge-claude` is retired. The 3c warm-pool concurrency learnings (forget_session-drops-stash, invalidate_slot identity, deferred-init, the reaper-vs-follow-up TOCTOU) remain recorded in the 3c spec/plan for reference; the `AgentKind` seam is kept for B1.

## 4. Keep the `AgentKind` seam — `Acp`-only (for B1)

`AgentKind` becomes a single-variant enum:
```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AgentKind { #[default] Acp }
```
**Kept** (scaffolding a future B1 `ClaudeApi` will reuse): the `kind` field on `AgentEntry`, `parse_kind` (acp-only), the factory `match entry.kind` (now one arm), and `kind` in the registry slot-reuse identity. The `kind="acp"` TOML key stays valid (forward-compat). **Removed casualty:** `kind_change_forces_fresh_slot` (bridge-registry test) requires two variants to flip — delete it with a comment that it returns when a 2nd kind (B1) lands. The `bridge-core` domain test that asserts `ClaudeCli` is rewritten to assert the `kind` field round-trips with `Acp`.

> Note for B1 (parked): `claude-agent-acp` ALSO covers the API-key path (set `ANTHROPIC_API_KEY` → it bills the Console/API instead of the subscription) — but that is still **process-based ACP**, so it does NOT satisfy B1's *non-process* conductor purpose. Whether B1 = "a true non-process HTTP backend (`ClaudeApi`)" or "just claude-agent-acp + a key" is a B1-time decision; the kept seam supports either.

## 5. Testing & Definition of Done

1. **Live warm 2-turn gate (`bin/a2a-bridge/tests/e2e_registry.rs`):** add the Claude entry (§2, `model="haiku"`) + `"claude-agent-acp"` in `allowed_cmds`, appended LAST so existing indices are untouched. A separate `#[ignore]` gated test routes to `claude` by id and drives a **2-turn round-trip on one session** ("Remember the number 7. Reply OK." → "What number…" → asserts the 2nd reply contains `7`) — proving warm continuity via the adapter through the full bridge. Keep the proven kiro/codex/gemini tests untouched. Optionally extend the multi-agent test to **4 agents** (codex+kiro+gemini+claude) from one registry.
2. **Real-frame corpus (`crates/bridge-acp/tests/corpus/claude-agent-acp.jsonl`):** capture a real `claude-agent-acp` round-trip (Haiku) — the `agent_message_chunk`(s), the terminal `result`/`stopReason`, the init/session-new results — and replay through the real `AcpBackend` path (`corpus_replay.rs`). Add `"claude-agent-acp"` to `real_capture_corpus_present` (now 4 agents). Assert the captured `stopReason` is a modeled SDK variant (expected `end_turn`; a non-SDK value would panic `replay()`'s `.expect()` — escalate if so).
3. **Retirement is clean:** after deleting `bridge-claude` + rewiring the seam, full `cargo test --workspace` + `cargo clippy --workspace --all-targets -- -D warnings` + `cargo fmt --check` are all green. No dangling `bridge-claude`/`ClaudeCli`/`claude-cli` references (grep clean except the historical 3c docs + ADR-0006).
4. **Coverage:** the workspace coverage gate holds (deleting `bridge-claude` removes its lines from both numerator and denominator; `bridge-core` ≥90% must still hold after the domain-test edits). Re-measure after `cargo llvm-cov clean`.

## 6. Scope boundary

**BUILDS:** the `claude` `kind="acp"` entry (+ `allowed_cmds`); the gated warm 2-turn live gate (Haiku); the `claude-agent-acp.jsonl` corpus + replay + DoD-gate wiring; ADR-0006; the full retirement of `bridge-claude` + the `AgentKind::ClaudeCli` arm; the seam collapse to `Acp`-only. **NON-GOALS:** no fs-proxying (the no-fs posture is kept, consistent with all agents); no per-entry env / `CLAUDE_CODE_OAUTH_TOKEN` headless support yet (local gate uses the ambient session — a small noted follow-on for deployment); B1 (the non-process HTTP backend) and the conductor re-evaluation stay **parked**; no production model policy beyond the entry (Haiku is a dev-slice testing choice, not a product default). This increment SUPERSEDES Increment 3c's `ClaudeCliBackend`.

## 7. Review
This design is grounded in live `claude-agent-acp` probes (§1, incl. the decisive warm-per-session + cache-read evidence). It gets the usual Codex(gpt-5.5)+Claude(opus-4.8) dual review before the plan; the plan then gets its own dual review before the build. The reviews should focus on: (a) is the retirement blast radius complete (does deleting `bridge-claude` + collapsing the seam leave the workspace green, with no missed reference)?; (b) is the single-variant `AgentKind` acceptable (clippy/dead-code) or does keeping the seam create awkwardness worth reconsidering?; (c) is the warm 2-turn gate a genuine warm-continuity proof (not a false pass), and is the corpus a real capture; (d) the `claude-agent-acp` install/PATH assumption for the gated e2e.
