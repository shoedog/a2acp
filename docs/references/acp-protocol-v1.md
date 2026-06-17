# Reference — Agent Client Protocol (ACP) v1

> A durable quick-reference for the ACP v1 spec, folded down to the facts that bear on the bridge.
> The bridge is the **Client** (it launches agents as subprocesses and drives them); codex-acp / claude /
> kiro are **Agents**. Source: <https://agentclientprotocol.com/protocol/v1/overview.md> (fetched
> 2026-06-17). Keep this updated when the pinned SDK (`agent-client-protocol`) or the spec moves.

## Spec pages (v1)

| Topic | URL |
|---|---|
| Overview | <https://agentclientprotocol.com/protocol/v1/overview.md> |
| Initialization | <https://agentclientprotocol.com/protocol/v1/initialization.md> |
| Session Setup (new / load / resume / close) | <https://agentclientprotocol.com/protocol/v1/session-setup.md> |
| Session Modes | <https://agentclientprotocol.com/protocol/v1/session-modes.md> |
| Session Config Options | <https://agentclientprotocol.com/protocol/v1/session-config-options.md> |
| Prompt Turn (+ cancellation) | <https://agentclientprotocol.com/protocol/v1/prompt-turn.md> |
| Content | <https://agentclientprotocol.com/protocol/v1/content.md> |
| Tool Calls (+ permission) | <https://agentclientprotocol.com/protocol/v1/tool-calls.md> |
| File System | <https://agentclientprotocol.com/protocol/v1/file-system.md> |
| Agent Plan | <https://agentclientprotocol.com/protocol/v1/agent-plan.md> |
| Slash Commands | <https://agentclientprotocol.com/protocol/v1/slash-commands.md> |
| Transports | <https://agentclientprotocol.com/protocol/v1/transports.md> |
| (also) Authentication, Terminals, Extensibility, Schema | `/protocol/v1/{authentication,terminals,extensibility,schema}.md` |

## Methods & notifications (the full v1 surface)

**Client → Agent (requests):** `initialize`, `authenticate`, `logout`, `session/new`, `session/load`,
`session/resume`, `session/close`, `session/list`, `session/delete`, `session/prompt`, `session/set_mode`,
`session/set_config_option`.
**Client → Agent (notification):** `session/cancel`.
**Agent → Client (requests):** `session/request_permission`, `fs/read_text_file`, `fs/write_text_file`,
`terminal/create`, `terminal/output`, `terminal/release`, `terminal/wait_for_exit`, `terminal/kill`.
**Agent → Client (notification):** `session/update` (the streaming channel — all variants below).

The full session-management surface is **capability-gated** (`sessionCapabilities.{resume,close,list,delete}`
+ `loadSession`): nothing here is universal — check the flag first.

## Initialization & capabilities (the gating foundation)

`initialize { protocolVersion: int, clientCapabilities, clientInfo{name,title,version} }` →
`{ protocolVersion, agentCapabilities, agentInfo, authMethods[] }`. Version negotiation: client sends its
highest; agent answers with its latest if it can't match; client disconnects if it can't support that.

- **clientCapabilities:** `fs.readTextFile`, `fs.writeTextFile`, `terminal`.
- **agentCapabilities:** `loadSession`, `promptCapabilities.{image,audio,embeddedContext}`,
  `mcpCapabilities.{http,sse}`, `auth{…}` (incl. a `logout` capability), `delete`
  (`SessionDeleteCapabilities`), `additionalDirectories`, and `sessionCapabilities.{resume,close}`.

**Everything optional is capability-gated** — the Client MUST check the flag before using the method.
The bridge already **captures `agent_capabilities`** (`acp_backend.rs:234/970/1060`) but today acts only on
the config/mode surface; load/resume/close/delete are advertised-but-unused.

## Session lifecycle

- `session/new { cwd (abs), mcpServers[] }` → `{ sessionId }`. Lazy/at-first-prompt in the bridge.
- `session/load { sessionId, cwd, mcpServers }` — gated by `loadSession`. **Replays the ENTIRE
  conversation** back to the client as `session/update` chunks (so the agent persists history server-side).
- `session/resume` — gated by `sessionCapabilities.resume`. Reconnect **without** replay.
- `session/close` — gated by `sessionCapabilities.close`. Clean teardown of a **live** session.
- `session/list { cwd?, cursor? }` → `{ sessions[], nextCursor? }` — gated `sessionCapabilities.list`.
  Each session = `{ sessionId, cwd, title?, updatedAt? (ISO-8601), additionalDirectories?, _meta? }`.
  Enumerate persisted sessions (paginated, cwd-filterable). A discovery surface for the orchestrator.
- `session/delete { sessionId }` → `{}` — gated `sessionCapabilities.delete`. Removes a session from the
  agent's persisted **history** (disappears from `session/list`); soft or hard internally; deleting a
  missing/already-deleted session SHOULD succeed silently; effect on a *live* session is
  implementation-defined. **Distinct from `session/close`:** close = teardown the live session; delete =
  purge persisted history. They are orthogonal.
- **Implication for the bridge:** post-restart context rehydration IS possible *iff* the agent advertises
  `loadSession` (use `session/list` to discover the id, `session/load` to replay); otherwise warm context
  is lost (re-mint cold). `release` = `session/close` (live teardown) when advertised, optionally
  `session/delete` to purge history; else drop bridge-side state + let the process reap.

## Session config (mode / model / effort) — changeable mid-session

- **`session/set_config_option { sessionId, configId, value }`** → `{ configOptions[] }`. Reserved
  categories: **`mode`**, **`model`**, **`thought_level`** (effort), plus custom `_*`. Option type today
  is `select` only. ConfigOption = `{ id, name, description, category, type, currentValue, options[] }`.
- Agent-initiated change: `session/update` `config_option_update` (full state each time — supports
  dependent cascades, e.g. changing model changes available efforts).
- **`session/set_mode { sessionId, modeId }`** + `current_mode_update` notification; modes advertised at
  setup as `{ currentModeId, availableModes[]:{id,name,description?} }`. **Being deprecated in favor of
  Session Config Options.**
- **Key fact:** config (mode/model/effort) **can change at any point, idle OR generating — no new session
  required.** The bridge already sends `set_mode` (HARD after `session/new`) and `set_config_option`
  (model/effort pinning) — `acp_backend.rs:451/76`, golden-frame-tested. codex moved model/effort into
  `config_options`; kiro uses `models` + `session/set_model`; claude varies by version.

## Prompt turn

- Starts: `session/prompt { sessionId, prompt: ContentBlock[] }` (respect `promptCapabilities`).
- Streams `session/update` variants: **`plan`**, **`agent_message_chunk`** (`messageId?` for grouping),
  **`tool_call`**, **`tool_call_update`**, **`usage_update`**, `current_mode_update`,
  `config_option_update`, `available_commands_update`.
- Ends: the `session/prompt` response carries a **`StopReason`** ∈ `end_turn`, `max_tokens`,
  `max_turn_requests`, `refusal`, `cancelled`.
- **Cancellation:** `session/cancel` notification → agent returns `cancelled` (NOT an error); client
  pre-marks unfinished tool calls cancelled + answers any pending permission with a `cancelled` outcome.
- **One turn per session**; concurrent prompts on one session are not specified. **Mid-turn input
  injection is NOT defined** — the only live channel mid-turn is agent→client.

## Content blocks (shared with MCP `ContentBlock`)

`text`, `image` (gated `promptCapabilities.image`), `audio` (gated `audio`), `resource` (embedded:
uri+mimeType+content), `resource_link`. Common optional fields: `annotations`, `mimeType`, `uri`. Appear in
prompt parts, agent message chunks, and tool-call content.

## Tool calls

- `tool_call { toolCallId, title, kind?, status?, content?, locations?, rawInput?, rawOutput? }`;
  `tool_call_update { toolCallId, …only-changed-fields }`.
- **status:** `pending` → `in_progress` → `completed` | `failed`.
- **kind:** `read`, `edit`, `delete`, `move`, `search`, `execute`, `think`, `fetch`, `other`.
- **content kinds:** standard `content` blocks, `diff { path, oldText, newText }`,
  `terminal { terminalId }`. **locations:** `{ path, line? }` (follow-along).

## Permission

- `session/request_permission { sessionId, toolCall, options[] }` → outcome `cancelled` |
  `selected { optionId }`.
- **option kinds:** `allow_once`, `allow_always`, `reject_once`, `reject_always`.
- The bridge auto-answers via its `PolicyEngine` today (`acp_backend.rs:820/1009`). An orchestrator-routed
  decision = the same offload + a bounded await; a timeout/deny maps to `cancelled` or a `reject_*` option.

## File system & terminals (agent→client; capability-gated)

- `fs/read_text_file { sessionId, path, line?, limit? }` → `{ content }` (gated `fs.readTextFile`).
- `fs/write_text_file { sessionId, path, content }` → null; creates if absent (gated `fs.writeTextFile`).
- `terminal/{create,output,release,wait_for_exit,kill}` (gated `terminal`) — agent runs commands via the
  client's environment. Relevant to sandbox/containerization (the bridge as the controlled environment).

## Agent plan (a progress/journal source)

`session/update` `plan { entries[] }`, entry = `{ content, priority: high|medium|low,
status: pending|in_progress|completed }`. **Complete replacement** each update (client replaces the whole
plan). A natural feed for the orchestration journal's progress/plan view.

## Slash commands

`available_commands_update` notification (dynamic) advertises `{ name, description, input?:{hint} }`.
Invoked by sending `/name args` as a text part in `session/prompt`.

## Transports

**stdio** (primary): client launches the agent subprocess; **newline-delimited** UTF-8 JSON-RPC; messages
**MUST NOT contain embedded newlines**; stdout = ACP frames only, stderr = optional logging. Streamable
HTTP is a draft. (This is exactly the framing the bridge's lsp-mcp newline fix relies on.)
