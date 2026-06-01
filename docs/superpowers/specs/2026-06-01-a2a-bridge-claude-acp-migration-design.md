# A2A Bridge — Claude → claude-agent-acp migration (+ retire bridge-claude) Design

**Goal:** Migrate Claude from the hand-rolled `bridge-claude` warm-CLI backend (Increment 3c) to the **official `@agentclientprotocol/claude-agent-acp`** driven by the existing conformant `AcpBackend` (a `kind="acp"` registry entry, like gemini/codex/kiro), and **retire `bridge-claude`** entirely. The warm/cache-hot benefit is preserved (spike-proven), so this is a net simplification, not a regression — one atomic increment so `main` never holds both the new ACP Claude and the dead `bridge-claude`.

**Architecture:** Claude becomes a plain ACP agent. `@agentclientprotocol/claude-agent-acp` v0.39.0 (official; ACP org) spawns the `claude` Code CLI under the hood and keeps it **warm per ACP session** (one persistent process, streaming-input mode, prompt cache hot across turns — verified live), reusing the Pro/Max **subscription** with NO API key (the old subscription-block is behind `--hide-claude-auth`, which we do NOT pass). It speaks ACP protocolVersion 1 over newline-delimited JSON-RPC — identical transport to the agents `AcpBackend` already drives. So Claude needs **no new backend code**; it's a registry config entry plus the retirement of the bespoke crate.

**Tech stack:** No new Rust deps. `@agentclientprotocol/claude-agent-acp` 0.39.0 (Node; install globally so `claude-agent-acp` is on PATH, like `codex-acp`). Reuses `bridge-acp`/`AcpBackend`, `bridge-registry`, the gated-e2e + corpus harness.

**Spec status:** brainstormed; design approved; probe-grounded (live `claude-agent-acp` probes, 2026-06-01); **dual-review folded (Revision 2)**.

> **Revision 2** — folds the dual spec review (Codex gpt-5.5 + Claude opus-4.8). Both confirmed the architecture sound and verified the load-bearing claims against real code (empty `authMethods` → `AcpBackend` skips `authenticate`; the single-variant `match` + tautological `c.kind == e.kind` pass `clippy -D warnings`). Corrections folded: **(1, DoD-breaking)** deleting the `ClaudeCli` arm orphans `ext_u64`/`ext_usize` → dead_code → `clippy -D warnings` RED — they MUST be deleted (§3); **(2)** completed the blast radius with `Cargo.lock` regen + the dangling doc/string refs; **(3)** the `kind` config test is **rewritten** (acp-only), not deleted; **(4)** the warm gate is **registry-level** (corrected from "full bridge"), with the ACP inbound coverage argued preserved; **(5)** the gate must **assert the model used is Haiku** (set_model is best-effort → could silently run Opus); **(6)** corpus must land same-increment + the README gate-status row; **(7)** softened the coverage claim (deleting a 92%-covered crate can lower the workspace %); **(8)** stated the conductor evidence-loss (bridge becomes ACP-only until B1). The config-watcher tests are a known timing flake (pass in the main repo), not introduced here.

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

No domain change to register it (Claude is a process/exec ACP agent). Subscription auth is ambient (local `~/.claude` session); headless `CLAUDE_CODE_OAUTH_TOKEN` env support is a noted non-goal (§6). **Install (gated-e2e prereq, like `codex-acp`):** `npm install -g @agentclientprotocol/claude-agent-acp` puts the bin `claude-agent-acp` on PATH; it launches the ACP stdio agent with **no args** (probe-confirmed: `claude-agent-acp` over stdio answered `initialize`/`session/new`/`prompt`), so `cmd="claude-agent-acp"`, `args=[]` is correct. The plan re-confirms the bin name + no-arg launch.

## 3. Retire `bridge-claude` (3c hand-rolled warm backend)

The COMPLETE blast radius (review-verified — every non-historical `bridge-claude`/`ClaudeCli`/`claude-cli` reference; missing any breaks the `clippy -D warnings`/grep-clean DoD):

**Delete (code):**
- the entire `crates/bridge-claude/` crate (`config`, `wire`, `proc`, `backend`, `reaper`, all its tests). The root `members = ["crates/*", "bin/a2a-bridge"]` glob means removing the directory drops it from the workspace; **regenerate `Cargo.lock`** (it carries `bridge-claude` at lines ~15/412) via a `cargo build` after removal.
- `bin/a2a-bridge/tests/e2e_claude.rs` (the 3c inbound e2e — see §5.1 for why no ACP coverage is lost).
- the `bridge-claude` dependency line in `bin/a2a-bridge/Cargo.toml`.
- the `AgentKind::ClaudeCli` factory arm in `bin/a2a-bridge/src/main.rs:124-139` (the `match entry.kind` collapses to the `Acp` arm only).
- **`ext_u64` and `ext_usize` in `bin/a2a-bridge/src/config.rs:246-258`** (+ any inline tests for them) — they are called ONLY by the deleted ClaudeCli arm (`main.rs:130/133/135`); in a *binary* crate `pub` does NOT exempt unused fns, so leaving them makes `clippy --workspace --all-targets -- -D warnings` go RED. **This is the finding that decides "does the workspace stay green" — it must be in the deletion.**
- `parse_kind`'s `"claude-cli"` arm in `config.rs:236` — `parse_kind` now accepts only `"acp"`, errors otherwise.

**Update (doc/string refs — compile-harmless but required for grep-clean):**
- `crates/bridge-core/src/domain.rs:27-29` — the `AgentKind` enum doc (drop "`ClaudeCli` selects the warm Claude Code backend").
- `bin/a2a-bridge/src/config.rs:121` — the `kind` field doc (`"acp"` only, drop `| "claude-cli"`).
- `bin/a2a-bridge/src/config.rs:239` — `parse_kind`'s error string (`(expected acp)`).

**Rewrite (tests — do NOT just delete):**
- `bin/a2a-bridge/src/config.rs:665-680` `kind_parses_and_defaults_to_acp` uses `kind="claude-cli"` → **rewrite** to an `acp` entry + a default-kind entry, both asserting `AgentKind::Acp` (keeps acp-parse + default coverage). **Keep** `invalid_kind_is_config_error` (`kind="nope"` still errors) unchanged.
- `crates/bridge-core/src/domain.rs:222-235` `agent_entry_carries_kind` asserts `ClaudeCli` → **retarget** to assert the `kind` field round-trips with `Acp`.
- `crates/bridge-registry/src/registry.rs:790-792` `kind_change_forces_fresh_slot` flips `Acp`→`ClaudeCli` → **delete** (needs two variants; can't flip a single-variant enum), with a comment that it returns when a 2nd kind (B1) lands. Confirm the OTHER reuse-identity tests (a `cmd`/`args`/`cwd`/`auth_method` change still forces a fresh slot) remain and cover the reuse logic.

**ADR-0006** records the supersession: 3c built a hand-rolled warm-CLI backend because the ACP-Claude adapter then appeared to require an API key; re-investigation (2026-06-01) found `claude-agent-acp` runs warm-per-session on the subscription, so Claude moves to the proven `AcpBackend` path and `bridge-claude` is retired. The 3c warm-pool concurrency learnings (forget_session-drops-stash, invalidate_slot identity, deferred-init, the reaper-vs-follow-up TOCTOU) remain recorded in the 3c spec/plan for reference; the `AgentKind` seam is kept for B1.

## 4. Keep the `AgentKind` seam — `Acp`-only (for B1)

`AgentKind` becomes a single-variant enum:
```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AgentKind { #[default] Acp }
```
**Kept** (scaffolding a future B1 `ClaudeApi` will reuse): the `kind` field on `AgentEntry`, `parse_kind` (acp-only), the factory `match entry.kind` (now one arm), and `kind` in the registry slot-reuse identity. The `kind="acp"` TOML key stays valid (forward-compat). The single-variant `match`, the (now-always-true) `c.kind == e.kind` in the reuse-identity, and the test casualties are handled in §3 — review-verified clippy-safe: a one-arm `match` + a tautological `==` both pass `clippy --all-targets -- -D warnings` (no `match_single_binding`/tautology lint).

**Conductor evidence-loss (flag for the parked re-eval):** retiring `bridge-claude` makes the bridge **ACP-only** again — the kept `AgentKind` seam has **no second real backend implementation** validating it until B1 lands. This reverts the "two real backend kinds shipped (ACP process + non-ACP process)" evidence that the post-3c conductor re-evaluation was to weigh. The retirement is still justified (the warm benefit is probe-preserved, and we shed the bespoke reaper / TOCTOU / warm-pool concurrency surface), but the conductor decision now rests on a single backend kind until B1 — record this in ADR-0006.

> Note for B1 (parked): `claude-agent-acp` ALSO covers the API-key path (set `ANTHROPIC_API_KEY` → it bills the Console/API instead of the subscription) — but that is still **process-based ACP**, so it does NOT satisfy B1's *non-process* conductor purpose. Whether B1 = "a true non-process HTTP backend (`ClaudeApi`)" or "just claude-agent-acp + a key" is a B1-time decision; the kept seam supports either.

## 5. Testing & Definition of Done

1. **Live warm 2-turn gate (`bin/a2a-bridge/tests/e2e_registry.rs`) — REGISTRY-level:** add the Claude entry (§2, `model="haiku"`) + `"claude-agent-acp"` in `allowed_cmds`, appended LAST so existing indices are untouched. A separate `#[ignore]` gated test routes to `claude` by id and drives a **2-turn round-trip on ONE `SessionId`** ("Remember the number 7. Reply OK." → "What number did I ask you to remember? Reply with just the number." → asserts the 2nd reply contains `7`). This is a **genuine warm-continuity proof**: same `SessionId` reuses `AcpBackend`'s `ensure_session` `OnceCell` (the ACP session persists), so context is retained; a cold session would answer wrongly (the question contains no `7`). It needs a **prompt-parameterized helper** (the current `route_and_prompt` hardcodes `PONG_PROMPT`/one turn) — ideally one `resolve` with the lease held across both turns. **Wording correction (review):** this is a *registry-level* proof — it drives `Registry`→`AcpBackend` directly and picks the `SessionId` itself; it does NOT exercise `InboundServer`'s `TaskId`→`SessionId` derivation. That inbound continuity is **already covered for any `AcpBackend` agent** by the existing inbound integration tests (`integration_inbound_kiro.rs`) + the 3b `BindingGuard`/`forget_session`-drops-stash binding tests — those are backend-agnostic (the 3c `e2e_claude.rs` being deleted was specific to `ClaudeCliBackend`'s own `forget_session` semantics, which no longer exist), so **no ACP inbound coverage is lost**. Keep the proven kiro/codex/gemini tests untouched.
   - **Haiku cost guarantee — enforced at the CORPUS CAPTURE (probe-corrected):** `AcpBackend` treats `session/set_model` as best-effort (continues on failure, `acp_backend.rs:922-935`), so the cost guarantee must be verified. **The model is NOT observable through the bridge** — the ACP `session/prompt` result is `{"stopReason","usage":{tokens}}` with **no model id**, and `map_session_update` drops the `usage_update` (which carries `cost`), so `Update::Done` reaches the test carrying only `stop_reason`. Therefore the live gate CANNOT assert the model. Instead, the **corpus-capture driver (§5.2)** sets `model="haiku"` and **asserts `session/set_model(haiku)` returned `{}`** (success) and records the `usage_update` cost as Haiku-range evidence ($\approx$15× cheaper than Opus on the system-prompt cache-write) — probe-confirmed reliable. The registry entry sets `model="haiku"`; the residual risk (set_model silently failing at gate runtime despite succeeding in the capture) is low and documented. This is the most the ACP protocol allows.
2. **Real-frame corpus (`crates/bridge-acp/tests/corpus/claude-agent-acp.jsonl`) — MUST land in THIS increment:** capture a real `claude-agent-acp` round-trip (Haiku) — the `agent_message_chunk`(s), the terminal `result`/`stopReason`, the init/session-new results — and replay through the real `AcpBackend` path (`corpus_replay.rs`). Add `"claude-agent-acp"` to the `real_capture_corpus_present` agents array **AND** add a `claude-agent-acp` row to the gate-status table in `crates/bridge-acp/tests/corpus/README.md`. Note: `real_capture_corpus_present` is a NON-ignored test, so it goes RED the moment the agent is listed without a committed `REAL-CAPTURE`-provenance jsonl — the capture and the listing land together. Assert the captured `stopReason` is a modeled SDK variant (expected `end_turn`; a non-SDK value would panic `replay()`'s `.expect()` — escalate if so).
3. **Retirement is clean:** after the full §3 deletion (incl. `ext_u64`/`ext_usize`, the doc/string refs, and `Cargo.lock` regen) + the seam collapse, `cargo test --workspace` + `cargo clippy --workspace --all-targets -- -D warnings` + `cargo fmt --check` are all green, and `grep -rn "bridge.claude\|ClaudeCli\|claude-cli\|claude_cli"` over `bin crates Cargo.toml Cargo.lock` is clean (except the historical 3c spec/plan + ADR-0006). *Note:* the `notify`-based config-watcher tests (`config.rs` `watch_*`) are timing/fs-event-sensitive — they pass in the main repo but can flake under parallel/worktree load; that is a pre-existing condition, not introduced here. Measure "workspace green" in the main repo; if a watcher test hiccups, rerun it in isolation.
4. **Coverage:** re-measure after `cargo llvm-cov clean --workspace`. **Caveat (review):** `bridge-claude` was ~92% covered (likely ABOVE the workspace average), so deleting it can move the workspace % *down*, not just stay flat — judge the result against the gate, don't assume it holds. `bridge-core` ≥90% is low-risk (removing the `ClaudeCli` variant + retargeting `agent_entry_carries_kind` to `Acp` removes lines rather than uncovering them).

## 6. Scope boundary

**BUILDS:** the `claude` `kind="acp"` entry (+ `allowed_cmds`); the gated warm 2-turn live gate (Haiku); the `claude-agent-acp.jsonl` corpus + replay + DoD-gate wiring; ADR-0006; the full retirement of `bridge-claude` + the `AgentKind::ClaudeCli` arm; the seam collapse to `Acp`-only. **NON-GOALS:** no fs-proxying (the no-fs posture is kept, consistent with all agents); no per-entry env / `CLAUDE_CODE_OAUTH_TOKEN` headless support yet (local gate uses the ambient session — a small noted follow-on for deployment); B1 (the non-process HTTP backend) and the conductor re-evaluation stay **parked**; no production model policy beyond the entry (Haiku is a dev-slice testing choice, not a product default). This increment SUPERSEDES Increment 3c's `ClaudeCliBackend`.

## 7. Review
Revision 2 has folded the dual spec review (Codex gpt-5.5 + Claude opus-4.8) — both verdict **ready to plan against** once the corrections above are folded (now done). The decisive risks they resolved: the retirement blast radius is now complete (the `ext_u64`/`ext_usize` dead-code trap that would have turned `clippy -D warnings` RED is in the deletion; `Cargo.lock` + doc/string refs covered); the single-variant `AgentKind` is clippy-safe (probe-verified); the warm 2-turn gate is a genuine registry-level proof with the Haiku cost guarantee enforced; the corpus lands atomically. **Next: write the implementation plan; the plan then gets its own dual review before the build.**
