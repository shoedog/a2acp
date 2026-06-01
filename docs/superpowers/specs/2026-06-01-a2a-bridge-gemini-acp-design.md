# A2A Bridge — Gemini ACP adapter Design

**Goal:** Add **Gemini** (`gemini --acp`, gemini-cli 0.41.2) as a registered agent — the **3rd real ACP agent** after kiro + codex — driven by the existing conformant `AcpBackend`. Prove it routes and answers a live round-trip through the bridge, register it **alongside** kiro + codex in the gated multi-agent e2e (3 real vendors from one registry), and capture its real frames into the conformance corpus.

**Architecture:** Pure registry **config entry** through the existing `AcpBackend` (no new crate). The `AgentKind` seam (3c) already routes `kind="acp"` → `AcpBackend`, so Gemini needs **no factory change**. Expected **zero production-code changes** — the live probe confirmed Gemini speaks the same protocol family the bridge already drives; the only conformance wrinkle (one unmodeled `session/update` variant) is already handled by the tolerant reader.

**Tech stack:** No new dependencies. `gemini` CLI 0.41.2 (installed, logged in via OAuth `oauth-personal`). Reuses `bridge-acp`/`AcpBackend`, `bridge-registry`, the gated-e2e + corpus test harness from 3a/3b.

**Spec status:** brainstormed; design approved; **dual-review folded (Revision 2)**. Probe findings (below) are LIVE-captured from `gemini --acp` on 2026-06-01.

> **Revision 2** — folds the dual spec review (Codex gpt-5.5 + Claude opus-4.8). Both confirmed "ready to plan, no hard blocker." Corrections: **(1)** the SDK mechanism for `available_commands_update` was factually wrong — it is **modeled** (`SessionUpdate::AvailableCommandsUpdate`), deserializes cleanly, and is dropped at the bridge's *map* layer (NOT "unmodeled, parse-layer drop"); unknown variants error-but-dispatch-continues. Conclusion (zero `AcpBackend` changes) stands. **(2)** the highest residual risk — `authenticate(oauth-personal)` — is now **probe-VERIFIED to return `{}`** (no interactive OAuth flow). **(3)** test-design: append Gemini LAST (index 2, so `live_edit`'s `entries[1]`/kiro is untouched); a SEPARATE gated test (don't fold into the proven 2-agent `route_to_each_agent_by_id`); thread `auth_method` through the `entry()` helper. **(4)** corpus: capture gemini-specific frames (chunk/stopReason/init — `available_commands_update` is redundant with codex), add an explicit deserialize+map assertion, add `"gemini-cli"` to `real_capture_corpus_present`, match the actual captured `stopReason`.

---

## 1. Live probe findings (the design's evidence)

A bounded ACP `initialize` + `session/new` probe against `gemini --acp` (2026-06-01) established:

- **`protocolVersion: 1`** — matches the `agent-client-protocol` =0.12.1 SDK the bridge uses (same as kiro + codex). `agentInfo`: `gemini-cli` 0.41.2.
- **`authMethods`**: `oauth-personal` ("Log in with Google"), `gemini-api-key`, `vertex-ai`, `gateway`. The machine is logged in via `oauth-personal` (`~/.gemini/oauth_creds.json`, `auth.selectedType="oauth-personal"`), no API key set. → configure `auth_method = "oauth-personal"`.
- **`authenticate(oauth-personal)` is a VERIFIED no-op** (extended probe, 2026-06-01): `initialize` → `authenticate{methodId:"oauth-personal"}` returns **`{}`** (no interactive OAuth flow, no auth-related stderr), and `session/new` succeeds immediately after. This was the single highest residual risk (an interactive OAuth flow would hang the bridge's bounded handshake and fail the gated test) — now measured, not inferred. Matches the codex-chatgpt precedent.
- **`agentCapabilities`**: `loadSession:true`, `promptCapabilities{image,audio,embeddedContext}`, `mcpCapabilities{http,sse}`. No client-FS dependency advertised.
- **`session/new` succeeds**: returns a `sessionId`, a `modes` set (`default`/`autoEdit`/`yolo`/`plan`, current `default`), and a `models` set (`auto-gemini-3` … `gemini-2.5-flash`, current `auto-gemini-3`). So `set_mode`/`set_model` are both supported.
- **`available_commands_update` is a MODELED `session/update` variant** (emitted right after `session/new`) — `SessionUpdate::AvailableCommandsUpdate` in the resolved schema (acp =0.12.1 pins schema 0.13.2; `client.rs` ~88/106). It deserializes **cleanly** as a known variant and is then **dropped at the bridge's MAP layer** — `map_session_update` only maps `AgentMessageChunk` text to `Update::Text` and returns `None` for everything else. **(Correction vs the first draft: this is NOT "unmodeled, dropped at the parse layer.")** The codex corpus already replays this exact modeled path (`codex-acp.jsonl` carries a real `available_commands_update` frame). A *genuinely* unknown `sessionUpdate` tag would **error at deserialize** (the typed handler returns a parse error, `handlers.rs:246`) — but the SDK dispatch **swallows** it (`send_error_notification`, the connection continues, `incoming_actor.rs:276`), so even a future truly-unknown variant is non-fatal. `usage_update` is that genuinely-unmodeled case (gated behind `unstable_session_usage`, enabled nowhere). **Net: zero `AcpBackend` changes is safe for Gemini because its extra variant is modeled — and even the unknown case is non-fatal.**
- **Trust model is non-fatal**: an untrusted cwd makes Gemini log `Skipping project agents … not trusted` / `Project hooks disabled` to **stderr** and run anyway. No trust dance needed for a round-trip.

Net: Gemini is a clean fit for `AcpBackend` as a config entry.

## 2. The Gemini registry entry

Registered exactly like kiro/codex — a `kind="acp"` entry the existing factory spawns via `AcpBackend`:

| Field | Value | Rationale |
|---|---|---|
| `id` | `gemini` | route-by-id target |
| `kind` | `acp` (default) | existing `AcpBackend` path |
| `cmd` | `gemini` | the installed CLI |
| `args` | `["--acp"]` | starts ACP mode (probe-confirmed) |
| `auth_method` | `oauth-personal` | advertised + already logged in |
| `model` | `gemini-2.5-flash` | a concrete fast model; `set_model` is best-effort |
| `mode` | unset | Gemini starts in `default`; no hard `set_mode` needed |
| `allowed_cmds` | add `"gemini"` | the `cmd`-allowlist gate |

No `cmd`/`allowed_cmds` domain changes (Gemini IS a process/exec agent — unlike the future non-process `claude-api`). No sample config to update (the repo ships none, per 3c).

## 3. Conformance handling (all via existing AcpBackend machinery)

- **Auth**: `AcpBackend`'s handshake advertises methods → `authenticate(auth_method)`. `authenticate("oauth-personal")` on the already-logged-in CLI **returns `{}` (VERIFIED no-op, §1)**. A **definitive** auth rejection → `BridgeError::AgentNotAuthenticated` → A2A `AuthRequired` (the existing mapping).
- **`available_commands_update`**: a **modeled** `SessionUpdate::AvailableCommandsUpdate` (§1) — deserializes cleanly, then dropped at the bridge's **map** layer (`map_session_update`→`None`, not text). No `AcpBackend` change needed; the codex corpus already exercises this modeled path. (Only a *genuinely* unknown future variant would deserialize-error, and the SDK dispatch swallows that too — connection continues. So there is no realistic "the SDK errors and breaks the connection" risk for Gemini.)
- **`set_model("gemini-2.5-flash")`**: best-effort (logs + continues on rejection — the existing `set_model` semantic). Mode left unset → no hard `set_mode` to fail the lazy mint.
- **Reverse requests**: any `session/request_permission` during a prompt → `AutoPolicy` auto-approves (same as kiro/codex); `fs/*` and `terminal/*` reverse requests → the bridge's method-not-found rejection handlers (the no-FS-caps bet that held for both kiro and codex).
- **cwd**: an absolute per-entry temp dir (ACP §11A), untrusted → non-fatal.

## 4. Testing & Definition of Done

The DoD is the **3-agent live gate + a captured-frame corpus**:

1. **3-agent gated e2e (`bin/a2a-bridge/tests/e2e_registry.rs`):** add the Gemini entry (§2) + `"gemini"` in `allowed_cmds`, with these **test-design constraints (review):**
   - **Append Gemini LAST (index 2), never before kiro.** `live_edit_changes_new_session_model` mutates `snapshot.entries[1]` (kiro) **by index** — inserting Gemini ahead of kiro silently breaks it. The new snapshot keeps `[codex(0), kiro(1), gemini(2)]`.
   - **Do NOT fold Gemini into the existing `route_to_each_agent_by_id`** assertion — that would make a missing/unauthed Gemini fail the *proven* codex+kiro assertions. Add a **separate `#[ignore]` Gemini round-trip test** (or a dedicated three-agent test) so the 2-agent assertions stay independently green.
   - **Thread `auth_method` through the `entry()` helper.** The current `entry()` hardcodes `auth_method: None`, so §2's `oauth-personal` would only take effect via `connect()`'s first-advertised fallback (order-dependent on Gemini advertising `oauth-personal` first). Extend `entry()` (test-only change; production "zero changes" preserved) to set `auth_method="oauth-personal"` explicitly on the Gemini entry.
   - The Gemini round-trip: `resolve("gemini")` → configure → prompt the deterministic `PONG_PROMPT` → `Done`, asserting the streamed text contains `PONG`. `#[ignore]`-gated (needs `gemini`+`kiro-cli`+`codex-acp` on PATH, all authed). Together with the kept 2-agent test, this proves one registry drives **3 real vendors**.
2. **Real-frame corpus capture (`crates/bridge-acp/tests/corpus/gemini-cli.jsonl`):** capture Gemini's **gemini-specific** frames — its `agent_message_chunk` content-block shape, the terminal `result`/`stopReason`, and the `initialize`/`session/new` results — these carry real conformance value. **Note (review):** the `available_commands_update` frame is the *same modeled path codex already covers* (`codex-acp.jsonl`), so it adds little; the gemini value is in the chunk/stopReason/init shapes. Wire the capture into `corpus_replay.rs` and **add `"gemini-cli"` to the `real_capture_corpus_present` agents array** (else the DoD gate won't enforce Gemini's real capture). Because `corpus_replay` currently collapses a deserialize *failure* and a mapped-non-text *drop* into the same `None`, **add an explicit assertion** that the captured `available_commands_update` frame deserializes as `SessionUpdate::AvailableCommandsUpdate` AND then maps to `None` (so the corpus genuinely guards the variant's parse, not just "it returned None"). The terminal-`result` corpus assertion must **match the actual captured `stopReason`**. **Note (plan-review correction):** an unknown `stopReason` is NOT "non-fatal" — the corpus `replay()` hard-`.expect()`s `StopReason` deserialization (the SDK models exactly five: `end_turn`/`max_tokens`/`max_turn_requests`/`refusal`/`cancelled`), so a non-SDK value would PANIC the replay *and* affects the live `session/prompt` result deserialization. The capture must therefore confirm Gemini's `stopReason` is one of the five (expected `end_turn`); anything else is a real conformance escalation, not a corpus detail. Also: the capture client advertises the **same capabilities production does** (no fs, no terminal), and Gemini's (chatty, untrusted-folder) **stderr is drained** so a full pipe can't stall the child.
3. **Run the gated e2e LIVE** to close the gate — Gemini answers PONG through the full bridge. Record the result. This is what makes "Gemini works" honest (the codex/kiro DoD-gate precedent).
4. **Coverage gates** stay at their existing thresholds (workspace ≥85%, bridge-core ≥90%); this increment adds config + tests, not new production paths, so coverage should hold without new units.

## 5. Scope boundary

**BUILDS:** the Gemini registry entry (`"gemini"` in `allowed_cmds` + a 3-agent snapshot with Gemini appended last); a **test-only** `entry()` helper extension to thread `auth_method`; a separate gated Gemini PONG round-trip test (kept independent of the proven 2-agent assertions); the `gemini-cli.jsonl` corpus capture + replay wiring + the explicit deserialize/map assertion + `"gemini-cli"` in `real_capture_corpus_present`; the live gate run. **CONFORMANCE FIX:** none expected — `available_commands_update` is modeled (§1), auth is a verified no-op, and even a hypothetical unknown variant is non-fatal. (Only a genuinely surprising live failure would prompt an `AcpBackend` change — not anticipated.) **NON-GOALS:** no new crate; no `AcpBackend` production redesign; no `cmd`/`allowed_cmds` domain change (Gemini is a process agent); no Gemini-specific model/mode policy beyond the entry; nothing touching the warm Claude backend, the conductor decision, or B1. This increment is intentionally small — it expands the agent roster and the regression corpus, and is a clean precursor to the B1 (non-process) increment and the conductor re-evaluation.

## 6. Review
Revision 2 has folded the dual spec review (Codex gpt-5.5 + Claude opus-4.8) — both confirmed the design is **ready to plan against**, with the corrections above (the central "zero `AcpBackend` changes" claim is verified safe; the auth no-op is probe-measured; the test-design and corpus details are pinned). Remaining open items are intentionally deferred to the **live gate** (§4.3): Gemini's full prompt-turn frames + its actual `stopReason`, and confirming stderr is drained. **Next: write the implementation plan; the plan then gets its own dual review before the build.**
