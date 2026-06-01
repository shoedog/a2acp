# ADR-0006 — Claude via `claude-agent-acp` supersedes the hand-rolled `bridge-claude`

**Date:** 2026-06-01
**Status:** Accepted

**Supersedes:** Increment 3c's `bridge-claude` crate (`ClaudeCliBackend`, the warm `claude --input-format stream-json` backend) and the `AgentKind::ClaudeCli` factory arm.

---

## Context

Increment 3c built a **hand-rolled warm Claude backend** (`bridge-claude` / `ClaudeCliBackend`):
a `claude --input-format stream-json --output-format stream-json` process kept warm per
bridge `SessionId`, with a bounded warm-pool (idle-TTL / `max_warm` LRU / hard
`max_sessions`), an `invalidate_slot` teardown primitive (reaper-holds-turn-lock +
post-lock `terminated` revalidation closing a TOCTOU), `forget_session`-drops-only-stash,
deferred-init capture, and a `pending_terminal` stash. That route was chosen because, at the
time, the official ACP-Claude adapter (`@zed-industries/claude-code-acp`, now
`@agentclientprotocol/claude-agent-acp`) appeared to **require an `ANTHROPIC_API_KEY` and
reject the Pro/Max subscription** — so it could not reuse the logged-in subscription the way
the warm CLI process could.

A 2026-06-01 re-investigation (prompted by the Claude Agent SDK subscription-token reports)
overturned that premise. Live-probed against `@agentclientprotocol/claude-agent-acp` 0.39.0:

- It runs on `@anthropic-ai/claude-agent-sdk`, which **spawns the `claude` Code CLI** and
  inherits its auth — so it runs on the **subscription with NO API key**. The
  subscription-reject path exists only behind `--hide-claude-auth` (a flag Zed passes to force
  API billing), **off by default**. A full prompt turn completed with no auth env set.
- It is **warm-per-session**: one long-lived SDK `query({ prompt: <streaming input> })` per ACP
  session, `claude` spawned once, fed successive prompts — verified by a 2-turn probe (same
  `claude` PID across turns, turn 2 read `cache_read_input_tokens` from the 1-hour ephemeral
  cache tier, recalled the planted number). The warm/cache-hot property `bridge-claude` existed
  to preserve is preserved.
- `protocolVersion 1`, `authMethods: []` to our client-capability shape (so the existing
  `AcpBackend` skips `authenticate` — `auth_method=None`), newline-delimited JSON-RPC over
  stdio — **identical transport** to the agents `AcpBackend` already drives (kiro, codex, gemini).

So Claude can be a plain `kind="acp"` registry entry through the **existing conformant
`AcpBackend`**, like Gemini — making the entire bespoke `bridge-claude` machinery redundant.

## Decision

**Retire `bridge-claude`. Register Claude as a `kind="acp"` entry backed by
`@agentclientprotocol/claude-agent-acp`, driven by the existing `AcpBackend`.**

- Deleted: the `crates/bridge-claude/` crate, the `bin/a2a-bridge/tests/e2e_claude.rs` inbound
  e2e, the `AgentKind::ClaudeCli` factory arm + its now-orphaned `ext_u64`/`ext_usize` config
  getters, `parse_kind`'s `"claude-cli"` arm, and the `ClaudeCli` doc/string/test references.
- **The `AgentKind` seam is KEPT, `Acp`-only** (single-variant `enum AgentKind { #[default] Acp }`,
  the `kind` field, `parse_kind`, the one-arm factory `match`, and `kind` in the registry
  slot-reuse identity), so a future non-process backend (B1 `ClaudeApi`, the Anthropic Messages
  API over HTTP) re-expands the seam without reintroducing it. A one-arm `match` over a
  single-variant enum is clippy-clean under `-D warnings`.
- Validated **LIVE against the subscription** (Haiku, to bound cost this slice): a gated warm
  2-turn round-trip through the bridge (`claude_warm_two_turns_via_acp` — turn 2 recalls the
  planted number from the same warm ACP session) and a real captured-frame corpus
  (`crates/bridge-acp/tests/corpus/claude-agent-acp.jsonl`, `set_model(haiku)` verified to
  return `{}`, cost in the Haiku range) replayed through the real `AcpBackend` path and added to
  the `real_capture_corpus_present` DoD gate (now 4 real ACP vendors: kiro, codex, gemini, claude).

## Consequences

- **Net simplification.** The bridge sheds the entire bespoke warm-pool / reaper / TOCTOU /
  deferred-init / `pending_terminal` concurrency surface. Claude is now "just another ACP agent",
  consistent with kiro/codex/gemini, and gains native ACP session/fork/resume/model-switch for
  free. Coverage gates held (workspace 93.85%, bridge-core 97.83%, bridge-acp 95.32%).
- **The 3c learnings are preserved** in the 3c spec/plan (`docs/superpowers/{specs,plans}/2026-0*-a2a-bridge-v3c*`)
  as a record of the warm-pool concurrency design, even though the code is retired.
- **The bridge is now ACP-only.** The `AgentKind` seam has **no second real backend
  implementation** validating it until B1 lands. This reverts the "two real backend kinds shipped
  (ACP process + non-ACP process)" evidence the **post-3c conductor re-evaluation** (deferred per
  ADR-0005 §9) was to weigh. **The conductor decision now rests on a single backend kind until
  B1 adds a non-process `ClaudeApi`** — which remains the genuine non-process test (note:
  `claude-agent-acp + ANTHROPIC_API_KEY` would bill the API but is STILL process-based ACP, so it
  does not satisfy the non-process dimension).
- **New runtime dependency surface:** `claude-agent-acp` is a Node package that spawns the `claude`
  Code CLI (also Node). This is not a new *class* of dependency (codex-acp is Node; 3c's
  `ClaudeCliBackend` already spawned the `claude` Node CLI) — and it is irreducible: the Claude
  *subscription* is only reachable through the `claude` Code CLI. Headless deployment uses a
  portable `CLAUDE_CODE_OAUTH_TOKEN` (`claude setup-token`) with `ANTHROPIC_API_KEY` unset;
  per-entry env support for that is a noted follow-on (the local gate uses the ambient `~/.claude`
  session).
- **No-fs posture retained:** `AcpBackend` advertises no fs/terminal capability, so the adapter
  won't send `fs/*` reverse calls — Claude (like kiro/codex/gemini through the bridge) cannot
  read/write files. fs-proxying is a separate, larger decision, out of scope here.

## Restoring `bridge-claude` (if we need it back)

The retirement was done as **one atomic commit** specifically so it is trivially recoverable.

- **Retirement commit:** `15f89ac` *(refactor: retire bridge-claude + collapse AgentKind to Acp-only)*,
  also reachable as the annotated tag **`bridge-claude-retired`**. It deleted, in one diff: the
  `crates/bridge-claude/` crate (config/wire/proc/backend/reaper + all tests), `bin/a2a-bridge/tests/e2e_claude.rs`,
  the `AgentKind::ClaudeCli` factory arm + the orphaned `ext_u64`/`ext_usize`, `parse_kind`'s `"claude-cli"` arm,
  the `ClaudeCli` enum variant + doc/string refs, and the `bridge-claude` dep — and rewired the 3 affected tests.
- **Restore everything at once:** `git revert 15f89ac` (or `git revert bridge-claude-retired`) reinstates the whole
  backend + the `ClaudeCli` seam arm + the tests in one step. Resolve any conflicts against current `main`.
- **Extract just the crate files:** `git checkout 15f89ac^ -- crates/bridge-claude bin/a2a-bridge/tests/e2e_claude.rs`
  (then re-add the `bridge-claude` dep in `bin/a2a-bridge/Cargo.toml` and the `AgentKind::ClaudeCli` factory arm in
  `main.rs`).
- **Pristine 3c crate** (as originally shipped, ~92% covered, fully dual-reviewed): the Increment-3c merge **`a5b6b2e`** —
  `git show a5b6b2e:crates/bridge-claude/src/backend/mod.rs`, etc.
- **Design rationale** (warm-pool, `invalidate_slot` identity teardown, reaper-vs-follow-up TOCTOU, deferred-init,
  the `pending_terminal` stash): `docs/superpowers/specs/2026-05-31-a2a-bridge-v3c-claude-design.md` (rev3) +
  `docs/superpowers/plans/2026-06-01-a2a-bridge-v3c.md`.

When it would come back: if a future need requires the bridge to own Claude's process lifecycle (e.g. a custom warm-pool
policy, non-ACP stream-json features, or fine CLI-flag control the `claude-agent-acp` allowlist blocks) that
`claude-agent-acp` cannot serve.
