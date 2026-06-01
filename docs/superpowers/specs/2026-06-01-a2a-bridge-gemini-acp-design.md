# A2A Bridge — Gemini ACP adapter Design

**Goal:** Add **Gemini** (`gemini --acp`, gemini-cli 0.41.2) as a registered agent — the **3rd real ACP agent** after kiro + codex — driven by the existing conformant `AcpBackend`. Prove it routes and answers a live round-trip through the bridge, register it **alongside** kiro + codex in the gated multi-agent e2e (3 real vendors from one registry), and capture its real frames into the conformance corpus.

**Architecture:** Pure registry **config entry** through the existing `AcpBackend` (no new crate). The `AgentKind` seam (3c) already routes `kind="acp"` → `AcpBackend`, so Gemini needs **no factory change**. Expected **zero production-code changes** — the live probe confirmed Gemini speaks the same protocol family the bridge already drives; the only conformance wrinkle (one unmodeled `session/update` variant) is already handled by the tolerant reader.

**Tech stack:** No new dependencies. `gemini` CLI 0.41.2 (installed, logged in via OAuth `oauth-personal`). Reuses `bridge-acp`/`AcpBackend`, `bridge-registry`, the gated-e2e + corpus test harness from 3a/3b.

**Spec status:** brainstormed; design approved. Probe findings (below) are LIVE-captured from `gemini --acp` on 2026-06-01.

---

## 1. Live probe findings (the design's evidence)

A bounded ACP `initialize` + `session/new` probe against `gemini --acp` (2026-06-01) established:

- **`protocolVersion: 1`** — matches the `agent-client-protocol` =0.12.1 SDK the bridge uses (same as kiro + codex). `agentInfo`: `gemini-cli` 0.41.2.
- **`authMethods`**: `oauth-personal` ("Log in with Google"), `gemini-api-key`, `vertex-ai`, `gateway`. The machine is logged in via `oauth-personal` (`~/.gemini/oauth_creds.json`, `auth.selectedType="oauth-personal"`), no API key set. → configure `auth_method = "oauth-personal"`; `authenticate(oauth-personal)` is expected to be a no-op when already logged in (the codex-chatgpt precedent).
- **`agentCapabilities`**: `loadSession:true`, `promptCapabilities{image,audio,embeddedContext}`, `mcpCapabilities{http,sse}`. No client-FS dependency advertised.
- **`session/new` succeeds**: returns a `sessionId`, a `modes` set (`default`/`autoEdit`/`yolo`/`plan`, current `default`), and a `models` set (`auto-gemini-3` … `gemini-2.5-flash`, current `auto-gemini-3`). So `set_mode`/`set_model` are both supported.
- **One unmodeled `session/update` variant**: `available_commands_update` (emitted right after `session/new`). The SDK 0.12.1 `SessionUpdate` enum does not model it → **tolerantly dropped** at the parse layer — the **exact codex `usage_update` situation** proven in 3a. The bridge's notification handler (`map_session_update`) only maps `AgentMessageChunk` text and drops everything else.
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

- **Auth**: `AcpBackend`'s handshake advertises methods → `authenticate(auth_method)`. `authenticate("oauth-personal")` on an already-logged-in CLI is expected to return `{}` (no-op). A **definitive** auth rejection → `BridgeError::AgentNotAuthenticated` → A2A `AuthRequired` (the existing mapping).
- **`available_commands_update`**: tolerantly dropped (unmodeled `SessionUpdate` → SDK parse-layer drop, mirroring `usage_update`). Confirmed surviving by the live e2e. **The one risk:** if the SDK 0.12.1 enum *errors* on this variant instead of dropping it, add a parse-layer tolerance fix in `bridge-acp` (small, mirrors the existing `usage_update` handling) — but the codex precedent says it drops.
- **`set_model("gemini-2.5-flash")`**: best-effort (logs + continues on rejection — the existing `set_model` semantic). Mode left unset → no hard `set_mode` to fail the lazy mint.
- **Reverse requests**: any `session/request_permission` during a prompt → `AutoPolicy` auto-approves (same as kiro/codex); `fs/*` and `terminal/*` reverse requests → the bridge's method-not-found rejection handlers (the no-FS-caps bet that held for both kiro and codex).
- **cwd**: an absolute per-entry temp dir (ACP §11A), untrusted → non-fatal.

## 4. Testing & Definition of Done

The DoD is the **3-agent live gate + a captured-frame corpus**:

1. **3-agent gated e2e (`bin/a2a-bridge/tests/e2e_registry.rs`):** extend the existing `two_agent_snapshot` to a **`three_agent_snapshot`** adding the Gemini entry (§2) + `"gemini"` in `allowed_cmds`, and add a **Gemini PONG round-trip** (`resolve("gemini")` → configure → prompt the deterministic `PONG_PROMPT` → `Done`, asserting the streamed text contains `PONG`) alongside the existing kiro + codex round-trips. Proves one registry snapshot drives **3 real vendors**. `#[ignore]`-gated (needs `gemini` + `kiro-cli` + `codex-acp` on PATH, all authed). The pre-existing 2-agent assertions stay green.
2. **Real-frame corpus capture:** capture Gemini's actual ACP frames (the `initialize`/`session/new` results, `available_commands_update`, the assistant `agent_message_chunk`s, the terminal `result`) into **`crates/bridge-acp/tests/corpus/gemini-cli.jsonl`**, and wire them into `corpus_replay.rs` + the `real_capture_corpus_present` check (the 3a corpus mechanism). Durable regression coverage: a future SDK bump that breaks Gemini parsing fails the replay.
3. **Run the gated e2e LIVE** to close the gate — Gemini answers PONG through the full bridge. Record the result. This is what makes "Gemini works" honest (the codex/kiro DoD-gate precedent).
4. **Coverage gates** stay at their existing thresholds (workspace ≥85%, bridge-core ≥90%); this increment adds config + tests, not new production paths, so coverage should hold without new units.

## 5. Scope boundary

**BUILDS:** the Gemini registry entry (`"gemini"` in `allowed_cmds` + the `three_agent_snapshot` in the gated e2e); the Gemini PONG round-trip assertion; the `gemini-cli.jsonl` corpus capture + replay wiring; the live gate run. **CONFORMANCE FIX (only if the live run surfaces it):** a parse-layer tolerance for `available_commands_update` (expected unnecessary). **NON-GOALS:** no new crate; no `AcpBackend` redesign; no `cmd`/`allowed_cmds` domain change (Gemini is a process agent); no Gemini-specific model/mode policy beyond the entry; nothing touching the warm Claude backend, the conductor decision, or B1. This increment is intentionally small — it expands the agent roster and the regression corpus, and is a clean precursor to the B1 (non-process) increment and the conductor re-evaluation.

## 6. Review
This design is grounded in a live `gemini --acp` probe (§1). It will get the usual Codex(gpt-5.5)+Claude(opus-4.8) dual review before the plan; the plan then gets its own dual review before the build. Given the increment's small, config-shaped surface, the reviews should focus on: (a) is "zero AcpBackend changes" actually safe, or does `available_commands_update` (or some prompt-time reverse request) force a real code change; (b) is the 3-agent e2e correctly extended without destabilizing the proven 2-agent assertions; (c) is the corpus capture faithful + non-tautological.
