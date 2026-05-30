# A2A Bridge — Increment 3a Design Spec (Conformant ACP Client via the SDK)

*Date: 2026-05-30*
*Status: Approved design — ready for implementation planning*
*Builds on: v1 + 2.5 + 2.6 (all merged to `main`)*
*Decomposition: first sub-project of "Increment 3" (multi-agent). 3b = agent registry; 3c = Gemini; 3d = N-way fan-out; 3e = real permission policy.*

---

## 1. Purpose

Replace the bridge's **non-conformant, hand-rolled ACP driver** with a **real, conformant,
bidirectional ACP client** built on the pinned `agent-client-protocol` crate (=0.12.1), and
validate it against **real `kiro-cli acp` and `codex-acp`**. This:
- **fixes a latent v1 bug** — `KiroBackend`/`replay` only pass because our Kiro *test scripts*
  mimic their simplified framing; against a real ACP agent they would fail (see §11 Appendix A);
- **resolves the deferred SDK adoption** (ADR-0003 Addendum 2);
- makes every later agent adapter (Gemini, Codex specifics) and the registry (3b) **thin**.

This is, deliberately, a **foundational conformance + SDK increment**, larger than the original
"add a Codex adapter" framing — that re-scope is the direct result of the 3a research (§11).

## 2. Decisions (locked)

| Decision | Choice |
|----------|--------|
| ACP client | Adopt `agent-client-protocol` =0.12.1 (already pinned); use its **client-side** connection (we are the ACP *client*, the spawned agent is the *agent*) — a real bidirectional peer (request/response correlation + reverse-request dispatch). |
| Generalize | `KiroBackend` → **`AcpBackend`** (protocol-generic); `kiro.rs` → `acp_backend.rs`. `main` spawns the configured `cmd` (`kiro-cli`/`codex-acp`). |
| Validate against | Real `kiro-cli acp` AND `codex-acp` (gated e2e). |
| Client capabilities | **Minimal — advertise NO `fs`/`terminal` caps** → agents fall back to local disk; we don't implement `fs/*`/`terminal/*` reverse handlers in 3a. |
| Permission | `request_permission` reverse-request handled as request/response; in 3a `PolicyEngine` is auto-approve → reply `selected:<allow option>`. The suspend→`input-required` path is deferred to 3e. |
| Model | `session/set_model` best-effort: called only if a `model` is configured; an error (builtin OpenAI rejects it) is logged and the agent's default model is used — not a backend failure. |
| Mode | `session/set_mode` (`read-only`/`auto`/`full-access`) called if `mode` configured; a bad mode id is a hard config error. Mode = agent posture, distinct from `PolicyEngine` (request resolution). |
| Conductor | Not in 3a (single client). With the SDK adopted, the conductor fork/no-fork re-eval moves to 3b (registry), where composing multiple agents is the actual question. |
| Codex transport | **codex-acp (ACP streaming)**, not `codex exec` one-shot. (The `a2a-local-bridge` PoC uses `codex exec`; noted as a reference alternative in §11 Appendix B, not adopted — it would make Codex a non-ACP backend and wouldn't fix the Kiro conformance.) |

## 3. Scope

### 3.1 In scope
1. `bridge-acp` depends on `agent-client-protocol` =0.12.1; an `AcpBackend` over its client-side
   connection implementing the **conformant** lifecycle (§5).
2. Correct wire shapes (§11 A): `initialize` (protocolVersion int `1`, minimal caps),
   `authenticate`, `session/new` (absolute `cwd` + `mcpServers`), `session/prompt`
   (`prompt:[ContentBlock]`, tagged text), `session/update` `agent_message_chunk` streaming,
   `session/cancel`, prompt-result `stopReason`.
3. Reverse-request handler for `session/request_permission` (auto-approve via `PolicyEngine`);
   `fs/*`/`terminal/*` not advertised (local-disk fallback), return "unsupported" if received.
4. `session/set_mode` + best-effort `session/set_model` from config; `AcpConfig{cwd,model,mode}`;
   `[agent]` config gains optional `model`/`mode`/`cwd`.
5. Spawn-time auth/reachability check → clean `AgentNotAuthenticated` (not a hang).
6. Tests via an **in-process fake ACP agent** (SDK agent-side connection); gated real e2es vs
   `kiro-cli acp` and `codex-acp`.
7. Rename ripple (`KiroBackend`→`AcpBackend`, `kiro.rs`→`acp_backend.rs`) + remove the old
   non-conformant scripted-child tests.

### 3.2 Out of scope (→ later sub-projects)
Agent registry / multiple simultaneous agents (3b); the conductor re-eval (3b); Gemini adapter
(3c); N-way fan-out across the registry (3d); real permission policy + the permission
suspend→`input-required` path + `session/set_mode` as a permission lever (3e); FS-over-ACP
(`fs/read_text_file`/`fs/write_text_file` client handlers) and `terminal/*`; `session/load`
resume; the `codex exec` one-shot backend (a possible future alt).

### 3.3 Success criteria
- **S1.** `AcpBackend` drives the full conformant lifecycle against the **in-process fake agent**:
  `initialize → authenticate → session/new(cwd,mcp) → session/prompt → agent_message_chunk
  stream → result`, yielding our `Update` stream; cancel → result `cancelled`.
- **S2.** A `request_permission` reverse-request from the (fake) agent is answered
  `{outcome:{outcome:"selected",optionId:<allow>}}` via `PolicyEngine`; on cancel, `cancelled`.
- **S3 (gated).** Against real `kiro-cli acp`: a prompt round-trips to a final artifact (proves
  the conformant client works with a real agent — the v1 path never actually did).
- **S4 (gated).** Against real `codex-acp`: same round-trip; `session/set_mode` applies; an
  unauthenticated Codex surfaces as `AgentNotAuthenticated`.
- **S5.** `bridge-core`/`bridge-acp` coverage gates hold; no FS/terminal caps advertised.

## 4. Architecture & component changes

```
AcpBackend (bridge-acp/src/acp_backend.rs)
  spawn(cmd,args,AcpConfig) -> Supervised child (process group; spawn/reap/kill as today)
       │  child stdin/stdout
       ▼
  agent-client-protocol CLIENT-SIDE connection  (bidirectional JSON-RPC peer)
   outbound (client→agent): initialize, authenticate, session/new, session/set_mode,
                            session/set_model, session/prompt, session/cancel
   inbound  (agent→client): session/update (notif) -> Update stream;
                            session/request_permission (REQUEST) -> PolicyEngine -> reply;
                            fs/*, terminal/* -> not advertised; "unsupported" if received
  AgentBackend impl: prompt(session,parts) -> BackendStream<Update>; cancel(session)
```

| File | Change |
|------|--------|
| `bridge-acp/Cargo.toml` | add `agent-client-protocol = { workspace = true }` (=0.12.1). |
| `bridge-acp/src/acp_backend.rs` (was `kiro.rs`) | `AcpBackend` over the SDK client connection; conformant lifecycle; `AcpConfig`; reverse-request handlers; in-process-fake-agent tests. |
| `bridge-acp/src/supervisor.rs` | unchanged (owns process lifecycle; SDK rides its pipes). |
| `bridge-acp/src/replay.rs`, `framing.rs` | unchanged — `replay` is a *translator* test double (yields `Update`s); not on the ACP wire. |
| `bridge-core/src/error.rs` | confirm `AgentNotAuthenticated`/`ModelNotAvailable` map as needed (no new variants expected). |
| `bin/.../config.rs` | `[agent]` gains optional `model`, `mode`, `cwd`. |
| `bin/.../main.rs` | wire `AcpBackend::spawn(cfg.agent.cmd, args, AcpConfig{...})` (replaces `KiroBackend::from_child`). |
| ADR | new ADR-0004: ACP SDK adopted (supersedes ADR-0003 Addendum 2's "not yet wired"). |

## 5. Lifecycle & data flow (conformant — see §11 A for exact shapes)

- **initialize:** send `protocolVersion:1` + minimal `clientCapabilities` (no `fs`, no `terminal`).
  Store the agent's `agentCapabilities`/`authMethods`.
- **authenticate:** if the agent advertised auth methods, call `authenticate{methodId}` with the
  appropriate id (`chatgpt`/`apikey` for Codex); failure → `AgentNotAuthenticated`.
- **session/new:** `{cwd:<absolute>, mcpServers:[]}` (3a passes no MCP servers); capture
  `sessionId` (+ reported `modes`/`models`).
- **set_mode/set_model:** if configured (§4 Section-4 rules).
- **session/prompt:** `{sessionId, prompt:[{type:"text",text:<part text>}...]}`.
- **streaming:** `session/update` notifications; `agent_message_chunk.content` (a ContentBlock)
  → `Update::Text`. (Other update variants — `agent_thought_chunk`, `tool_call*`,
  `available_commands_update`, `current_mode_update` — are ignored/no-op in 3a, tolerant reader.)
- **request_permission (reverse REQUEST):** `{sessionId, toolCall, options:[{optionId,name,kind}]}`
  → `PolicyEngine.decide`; auto-approve → reply `{outcome:{outcome:"selected",optionId:<an
  allow_once/allow_always option>}}`; on task cancel → `{outcome:{outcome:"cancelled"}}`.
- **result:** prompt result `stopReason` (`end_turn`→Done; `cancelled`→cancel completion;
  others→Done) → `Update::Done{stop_reason}`.
- **cancel:** `session/cancel` notification; the in-flight `session/prompt` returning `cancelled`
  is the completion signal (the existing, correct model).

## 6. Error model
Reuse `BridgeError`. Auth failure (from `authenticate` or an `auth_required` error) →
`AgentNotAuthenticated` (→ `TASK_STATE_AUTH_REQUIRED`). `set_model` error on a builtin provider →
logged, non-fatal. A non-JSON/oversize stdout frame remains a fatal `FrameError` (the SDK reads
NDJSON on stdout; stderr is captured, never parsed). A reverse `fs/*`/`terminal/*` request →
reply with the SDK's "method not supported" error.

## 7. Testing
- **In-process fake ACP agent** (SDK agent-side connection over in-memory duplex pipes):
  full-lifecycle round-trip (S1); cancel→`cancelled` (S1); `request_permission` round-trip,
  auto-approved (S2); `authenticate` failure→`AgentNotAuthenticated`; `set_mode` applied;
  `set_model` best-effort (success + builtin-error-is-non-fatal); a `session/update` variant we
  don't model is ignored (tolerant reader); an unsupported reverse `fs/*` request → "unsupported"
  reply. This replaces the removed non-conformant scripted-child tests.
- **Gated `#[ignore]` e2es:** real `kiro-cli acp` (S3) and real `codex-acp` (S4) — need the
  binaries + host auth; document run commands.
- Coverage gates unchanged (workspace ≥85; bridge-core/bridge-acp ≥90). The SDK code in
  `bridge-acp` must keep the crate ≥90 — the fake-agent tests make this achievable.

## 8. Ripple & cleanup
`KiroBackend`→`AcpBackend`, `kiro.rs`→`acp_backend.rs`; update `main`, the inbound server's
backend type references, the fan-out `local_kiro_source` naming, and any test constructing
`KiroBackend`. Remove the old non-conformant scripted-`/bin/sh` Kiro tests (replaced by the
fake-agent tests). `replay.rs`/`framing.rs` stay (translator test doubles). The 2.6 `AcpBackend`
(local) source used in fan-out is unaffected behaviorally (same `AgentBackend` contract).

## 9. Forward note
3b (registry) makes the conductor re-eval and multi-agent wiring; 3c adds Gemini (thin, on this
client); 3d wires N-way fan-out across the registry; 3e adds real permission policy + the
permission suspend→`input-required` path + `set_mode` as a permission lever; FS-over-ACP
(`fs/*` client handlers + MCP) and `session/load` are separate later items.

## 10. Open implementer input
Confirm the exact `agent-client-protocol` =0.12.1 **client-side** API (the connection
constructor + the client/handler trait method names + how to send agent-bound requests and
receive `session/update`/reverse requests) before building — the plan's first task is this SDK
discovery (the verify-then-build gate that worked in 2.5/2.6). Note the **version skew**:
`codex-acp` links ACP 0.9.2 (`unstable` features); we compile against 0.12.1. The driven methods
are wire-compatible (§11 A), but `unstable_session_model` gating differs — treat `set_model` as
best-effort.

---

## 11. Appendix — Research findings (grounding for implementers + reviewers)

### A. ACP / codex-acp protocol reference (quote-backed; sources = `agent-client-protocol` v0.12.1/v0.9.1 + `cola-io/codex-acp` source)

**Our current Kiro driver is NOT spec-ACP-conformant** — it passes only against our Kiro test
scripts. Concrete deltas a conformant `AcpBackend` must fix:
- **Skips `initialize`** (required first; negotiates protocolVersion + client caps).
- `session/new` needs `cwd` (absolute) + `mcpServers`; we send `{}`.
- Prompt field is **`prompt:[ContentBlock]`** (tagged `{"type":"text","text":...}`), not
  `parts:[{text}]`.
- Streamed text is `update.{sessionUpdate:"agent_message_chunk"}.content`, not `params.text`.
- `session/request_permission` is a **request needing a response** (`{sessionId, toolCall,
  options:[{optionId,name,kind}]}`; reply `{outcome:{outcome:"selected",optionId}|{cancelled}}`);
  the v1 `kind:"interactive"`/`requestId` parsing is fictional. (And the wire string DOES carry
  the `session/` prefix — spec §2.1's `request_permission` note was wrong.)
- Methods are **`session/set_mode`/`session/set_model`** (snake_case after the slash);
  `protocolVersion` is an **integer** (`1`).
- **Reverse-direction requests:** codex-acp issues `fs/read_text_file`/`fs/write_text_file`,
  `terminal/*`, and `session/request_permission` back to the client mid-turn — requiring a
  **bidirectional JSON-RPC peer** (what the SDK provides; the one-way reader can't).
- codex-acp runs an internal MCP fs server (`acp_fs`) and falls back to local disk if the client
  lacks FS caps → 3a advertises no FS caps.
- **Model switching is custom-provider-only** (builtin OpenAI → `models:null`, `set_model` errors
  `invalid_params`); **mode switching always works** (`read-only`/`auto`/`full-access`).
- `authenticate` relies on host creds (`codex login` / `OPENAI_API_KEY`); failure →
  `auth_required` error → map to `AgentNotAuthenticated`. (Exact numeric error code unconfirmed —
  read `agent-client-protocol/src/error.rs` `auth_required()` or capture a live frame.)
- stdout = pure JSON-RPC NDJSON; codex-acp logs to stderr/file — keep the "non-JSON on stdout =
  fatal" rule and ensure no logs leak to stdout.

(Full subagent report retained in session history; this is the distilled, actionable subset.)

### B. Reference: how `a2a-local-bridge` drives Codex (firewalled — operational facts only)

The PoC drives Codex via **`codex exec --model <id> --sandbox workspace-write --json --color
never -`** (one-shot, prompt-on-stdin, parse the `--json` event stream's
`item.completed`/`agent_message` to EOF) — **not** codex-acp/ACP. Its only stdio-JSON-RPC Codex
transport (`codex exec-server --listen stdio`) is a generic experimental process runner, not a
turn driver. This is a real second opinion that the simpler one-shot path is viable — but we
chose codex-acp (ACP streaming) because (a) it fits our streaming `AgentBackend`/ACP architecture
natively, (b) `codex exec` would make Codex a non-ACP backend type, and (c) it does not fix the
Kiro ACP non-conformance. `codex exec` is recorded as a possible future alternative backend, not
adopted. (Per the firewall: only these operational facts were taken; none of the PoC's Python
structure.)
