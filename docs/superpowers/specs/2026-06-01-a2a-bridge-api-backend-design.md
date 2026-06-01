# A2A Bridge — Vendor-neutral OpenAI-compatible API backend (`kind="api"`) Design

**Goal:** Add a **non-process** `AgentBackend` that speaks the **OpenAI-compatible** HTTP API (`POST {base_url}/chat/completions`), registered as a new `kind="api"` entry. It is the cheap/free replacement for the originally-scoped Claude-specific **B1** (`ClaudeApi`, Anthropic Messages API, needs a paid key). Validated live at **$0** against a local **Ollama** tool-capable model. It deliberately exercises the **two port surfaces the parked conductor decision must weigh**: **A = lifecycle/transport** (no `Supervised` child, `cmd`→optional, a URL-bearing entry, SSE streaming → `Update::Text`) and **B = permission/policy** (a `tool_call` → the existing `PolicyEngine`, mirroring `AcpBackend`).

**Architecture:** A new crate `crates/bridge-api` holds `ApiBackend`, implementing `bridge_core::ports::AgentBackend` over the workspace `reqwest`. It owns **no child process** — it holds a `base_url` + a `reqwest::Client`. The whole prompt turn (stream text, request a tool, decide via policy, execute the stub tool or deny, loop to a final answer) runs **inside `prompt()`** — no warm-session / suspend-resume state machine (the bridge-claude complexity we just retired is NOT reintroduced). The `AgentKind` seam (kept `Acp`-only after the bridge-claude retirement, expressly for this) re-expands to `{ #[default] Acp, Api }`; the `main.rs` factory gains a second arm that builds the HTTP backend.

**Tech stack:** Reuses the workspace `reqwest` 0.12 (already a dep of `bridge-a2a-outbound` + `bin/a2a-bridge`, already in `Cargo.lock`) + `serde`/`serde_json` + `futures`/`tokio-stream`. One new **dev-dep**: `wiremock` (offline mock HTTP server for deterministic replay tests). Live gate: local Ollama (`qwen3.5:9b` default; `OmniCoder-9B` a config swap — both expose OpenAI-compatible streaming tool calls via Ollama). No new runtime dep classes; no paid API; no key for the local gate.

**Spec status:** brainstormed; design approved 2026-06-01; grounded in the current code (post-bridge-claude-retirement, verified against `domain.rs`/`ports.rs`/`acp_backend.rs`/`main.rs`/`config.rs`/`translator.rs`/`registry.rs`). **Dual review (Codex + Claude) pending — folds into Revision 2 before the plan.**

**Firewall:** the `~/code/a2a-local-bridge` Python PoC is referenced black-box only; nothing in its schema/methodology informs this design. All wire shapes here derive from the public OpenAI/Ollama HTTP API and the Rust codebase.

---

## 1. Why — the conductor framing (what evidence this produces)

ADR-0002→0005 deferred the **fork-conductor-vs-greenfield** decision to "post-3c, a second protocol family / a non-process backend." Retiring `bridge-claude` (ADR-0006) left the bridge **ACP-only** — a single backend kind, all local-process. The conductor's decisive question is **"do the ports absorb a NON-process backend cleanly?"** This increment answers it across both surfaces, at $0:

- **Surface A — lifecycle/transport.** A process backend is launched via `cmd`/`args` and supervised (`Supervised` child). A non-process backend has **no child** — it has a `base_url`. Forcing `cmd` → `Option` and making `allowed_cmds` not apply to it is *the* exec-centric residue the conductor wants to measure. The **size of that ripple is the deliverable** (§3).
- **Surface B — permission/policy.** The bridge's `PolicyEngine` + `Update::Permission` + the translator's suspend/resume govern "the agent wants to do X; the policy decides." OpenAI function-calling is a **structurally different** permission model (client-side: the model emits `tool_calls`, the *client* decides and feeds back a result — there is no agent blocking over a wire). Showing the **existing thin port absorbs that second model with zero domain change** (§4.3) is strong evidence the permission port is general — *plus* a documented finding that the port is tool-blind (§4.4).

Downstream consumer: the **conductor re-eval stays parked** until this lands; this increment is the evidence it was waiting on.

## 2. The `api` registry entry + `AgentKind` re-expansion

`AgentKind` (`crates/bridge-core/src/domain.rs:31`) gains a second variant:

```rust
pub enum AgentKind {
    #[default]
    Acp,
    Api, // non-process OpenAI-compatible HTTP backend
}
```

`parse_kind` (`bin/a2a-bridge/src/config.rs:233`) accepts `"api"`; the error message becomes `expected acp|api`. A representative entry (TOML):

```toml
[[agents]]
id          = "ollama"
kind        = "api"
base_url    = "http://localhost:11434/v1"   # OpenAI-compat base; backend POSTs {base_url}/chat/completions
model       = "qwen3.5:9b"                   # reuses the existing `model` field
# api_key_env = "OPENAI_API_KEY"             # optional: NAME of the env var holding a bearer token; omit for Ollama
# no `cmd` — a non-process backend has no command to launch
```

A process entry (`kind="acp"`) is unchanged and still carries `cmd`. **A process backend has a `cmd`; a non-process backend has a `base_url`; the `kind` selects which is required** (§3). The `kind="api"` factory arm builds `ApiBackend`, NOT an `AcpBackend` (§4).

## 3. The non-process domain ripple — the conductor signal (surface A)

This is the deliverable, not an accident. We make the exec-centric fields optional **honestly** (dodging via the freeform `extensions` map is explicitly rejected — it would hide the very signal the conductor needs).

**`crates/bridge-core/src/domain.rs`**
- `AgentEntry.cmd: String` → **`Option<String>`**.
- Add **`pub base_url: Option<String>`** (peer to `cmd` — the non-process "where", parallel to the process "how to launch").
- Add **`pub api_key_env: Option<String>`** (the *name* of an env var holding a bearer token; never the secret).
- All `AgentEntry { … }` literals (in `domain.rs` tests, `route.rs:95`, `bridge-a2a-inbound/src/server.rs:1683`, `config.rs` tests) set `cmd: Some("…".into())`, `base_url: None`, `api_key_env: None`. Mechanical.

**`bin/a2a-bridge/src/config.rs`**
- The raw TOML struct's `cmd` becomes `Option<String>`; add `base_url`/`api_key_env`.
- `into_snapshot` (`:164`): `allowed_cmds` default union (`:170`) **skips `None`** cmds (`filter_map(|a| a.cmd.clone())`); assign the new fields onto `AgentEntry`.
- **Validation:** `kind="acp"` requires `cmd = Some(..)` (else `ConfigError::Registry`); `kind="api"` requires `base_url = Some(..)` and **forbids `cmd`** (a `cmd` on an api entry is a config error — fail loud). This validation lives in `into_snapshot` (boot fails loud on bad config, per the existing spec §7 posture).

**`bin/a2a-bridge/src/main.rs`** — the factory (`:107`):
- `AgentKind::Acp =>` arm now reads `let cmd = entry.cmd.as_deref().ok_or(BridgeError::ConfigInvalid { reason: "acp entry missing cmd".into() })?;` before `AcpBackend::spawn(cmd, …)`. (cwd computation stays in this arm; an Api backend needs no cwd.)
- New `AgentKind::Api =>` arm builds `ApiConfig` from the entry and constructs `ApiBackend::new(cfg).with_policy(policy)`. No `Supervised`, no cwd, no `cmd`.

**`crates/bridge-registry/src/registry.rs`** — reuse-identity (`:245`): `c.cmd == e.cmd` now compares `Option<String>` (logic unchanged); add `c.base_url == e.base_url` to the reuse tuple so a base_url edit re-spawns the slot (consistent with how cmd/args/cwd/auth_method/kind already key reuse).

**Blast radius:** bounded and mechanical — one enum variant, one parser arm, three `Option` field changes on a shared struct + their literals, one factory arm, one validation block, one reuse-tuple line. No behavior change for existing ACP entries. The spec records this enumeration so the conductor can weigh the absorption cost directly.

## 4. `ApiBackend` — the non-process backend (surfaces A + B)

### 4.1 Construction & shape

`crates/bridge-api/src/lib.rs` (split into focused modules: `config.rs`, `wire.rs`, `backend.rs`, `tool.rs`):

```rust
pub struct ApiConfig {
    pub base_url: String,          // e.g. http://localhost:11434/v1
    pub model: Option<String>,     // request model id; None → omit (server default)
    pub api_key_env: Option<String>,
    pub max_tool_rounds: usize,    // default 4 — bounds the tool loop
    pub request_timeout: Duration, // default 120s
}

pub struct ApiBackend {
    cfg: ApiConfig,
    client: reqwest::Client,
    api_key: Option<String>,       // resolved from api_key_env at construction
    policy: Arc<StdMutex<Arc<dyn PolicyEngine>>>, // default auto-approve; with_policy swaps it
}

impl ApiBackend {
    pub fn new(cfg: ApiConfig) -> Self { /* default policy = AutoApprove (mirrors AcpBackend) */ }
    #[must_use] pub fn with_policy(self, policy: Arc<dyn PolicyEngine>) -> Self { /* swap, builder-style */ }
}
```

`ApiBackend` carries **no per-session conversation state** (an OpenAI completion is stateless, so `prompt(session, parts)` builds the message list from `parts` and runs one full turn) — the only per-session state is a small **cancel-latch map** (§4.6). `configure_session`/`forget_session`/`retire` use the trait defaults; `cancel` is overridden (§4.6). The `session: &SessionId` arg keys the cancel latch so an in-flight turn can be aborted.

### 4.2 The turn loop (inside `prompt()` → one `BackendStream`)

`prompt()` returns a `BackendStream` (an `async_stream`/channel) that runs:

1. Seed `messages = [{role:"user", content: <joined parts.text>}]`.
2. **Loop** (≤ `max_tool_rounds`):
   a. `POST {base_url}/chat/completions` with `{ model, messages, tools: [TIME_TOOL], stream: true }` (+ `Authorization: Bearer` if a key resolved).
   b. Parse the SSE stream (§5): emit `Update::Text(delta.content)` as text arrives; accumulate any `delta.tool_calls[]` fragments by index.
   c. On `finish_reason == "stop"` with no tool calls → emit `Update::Done { stop_reason: "stop" }` and END.
   d. On `finish_reason == "tool_calls"` (or accumulated tool calls present) → for **each** tool call: run §4.3 (permission + execute/deny), appending the assistant tool-call message + a `{role:"tool", tool_call_id, content}` result message; then continue the loop (follow-up completion).
3. If the loop hits `max_tool_rounds` without a `stop`, emit `Update::Done { stop_reason: "max_tool_rounds" }` (bounded; no infinite tool loops).

### 4.3 Tool call → permission (surface B — the thin port, reused as-is)

Mirrors `AcpBackend::decide_permission` (`acp_backend.rs:732`) exactly — the same reduction, the same policy port, the same verdict→action mapping:

```rust
let perm = PermissionRequest::with_id(tool_call.id.clone(), /*interactive=*/ false);
// Observable signal into the stream (translator auto-approves non-interactive & continues — no behavioral effect):
yield Update::Permission(perm.clone());
let decision = policy.lock().ok().map(|p| p.decide(&perm, &SessionContext));
match decision {
    Some(Ok(PermissionDecision::Approve)) => { let result = run_tool(&tool_call)?; push_tool_result(result); }
    Some(Err(BridgeError::PermissionDenied)) => { push_tool_result("permission denied: tool not executed"); }
    _ /* abstain / poisoned */               => { push_tool_result("permission unavailable: tool not executed"); }
}
```

- **`interactive: false`** by design: the translator (`translator.rs:133`) suspends only on `interactive=true`. Non-interactive keeps the turn self-contained — no A2A-caller suspend/resume, no warm-session state. (Interactive tool-permission = an explicit non-goal, §7.)
- The policy is **tool-blind** (`PermissionRequest` carries only `request_id`+`interactive`; `SessionContext` is empty) — *exactly* as it already is for ACP. We do NOT enrich it (decided: reuse the thin port). This is recorded as a conductor finding (§4.4), not a defect.
- **Deny is honest:** on `Err(PermissionDenied)` the tool's effect string is NOT produced — the offline test asserts its absence (§6).

### 4.4 Documented conductor finding (tool-blindness)

The existing permission port reduces *any* tool/permission ask to `{request_id, interactive}` with an empty `SessionContext` and an `Approve`-only `PermissionDecision` (deny = `Err`). Both ACP and this API backend therefore decide permissions **without the policy seeing the tool name or arguments**. The increment **surfaces this as evidence** for the conductor: the port is general enough to absorb a second permission model unchanged, *but* per-tool policy decisions would require enriching `PermissionRequest`/`SessionContext` — a clean, separately-weighable follow-on, not folded here.

### 4.5 The stub tool (`tool.rs`)

One trivial, **side-effect-free** tool whose only job is to make the model emit a `tool_call`:

```
name: "get_current_time"
description: "Return the current server time as an ISO-8601 string."
parameters: { type: "object", properties: {}, required: [] }
run_tool() -> a deterministic string (a fixed/stub clock value — NOT wall-clock, to keep tests deterministic)
```

No fs, no network, no real capability — consistent with the bridge's no-fs posture across all agents. (`run_tool` returns a constant in tests; production behavior is identical — its purpose is the control-flow, not the value.)

### 4.6 Cancel, error & usage mapping

- **`cancel(session)`**: a per-session `Notify`/`AtomicBool` latch the stream checks between rounds and that aborts the in-flight `reqwest` request (drop the response stream) → the turn ends with `Update::Done { stop_reason: STOP_REASON_CANCELLED }` (`ports.rs:17`, the shared cancelled-spelling const — reused so translator/​adapter agree).
- **Errors**: connection refused / DNS / timeout → `BridgeError::AgentCrashed`; non-2xx HTTP → `BridgeError::AgentCrashed` with the status in context; malformed SSE / JSON → a frame error (stream yields `Err`, no restart — matches the ACP frame-error posture).
- **Usage**: the trailing `usage` (tokens) is **dropped at the port** — `Update::Done` carries only `stop_reason`, exactly like ACP. We control the request `model`, so no model-id assertion is needed or possible through the bridge (consistent with the documented ACP model-non-observability).

## 5. Wire shapes (OpenAI-compatible, Ollama-verified)

**Request** (`POST {base_url}/chat/completions`):
```json
{ "model": "qwen3.5:9b", "stream": true,
  "messages": [{"role":"user","content":"…"}],
  "tools": [{"type":"function","function":{"name":"get_current_time","description":"…","parameters":{"type":"object","properties":{}}}}] }
```

**Streaming response** — `text/event-stream`, `data: {chunk}\n\n` lines, terminated by `data: [DONE]`. Each chunk:
```json
{"choices":[{"delta":{"content":"par"},"finish_reason":null}]}                       // text delta
{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_1","function":{"name":"get_current_time","arguments":""}}]},"finish_reason":null}]}
{"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"{}"}}]},"finish_reason":null}]}  // args streamed in fragments
{"choices":[{"delta":{},"finish_reason":"tool_calls"}]}                               // tool round complete
```
- **Tool-call assembly:** `tool_calls` arrive as fragments keyed by `index`; `id`/`function.name` appear once, `function.arguments` is concatenated across deltas. The backend buffers per-index until `finish_reason`. A non-streamed single-chunk tool call (some servers/models) is handled by the same accumulator.
- **Follow-up turn** after a tool result appends `{"role":"assistant","tool_calls":[…]}` then `{"role":"tool","tool_call_id":"call_1","content":"<result>"}` and re-POSTs.

(Ollama's `/v1/chat/completions` exposes standard OpenAI `tool_calls` regardless of the model's native format — verified: Ollama supports streaming tool calls for Qwen3/3.5.)

## 6. Testing & Definition of Done

**Offline / CI (deterministic, `wiremock`):**
- **DoD-1 — text round-trip:** stub `/chat/completions` to return a recorded text-only SSE body; `prompt()` yields `Text…` then `Done{stop}`.
- **DoD-2 — tool approve path:** stub returns (call-1) a `tool_calls` SSE, then (call-2) a final text SSE. With the **default auto-approve** policy, assert the `Update` order is `Text* , Permission, Text* , Done`, the tool result was fed back, and the final text appears.
- **DoD-3 — tool deny path:** same stubs, but inject a **`DenyPolicy`** (`decide → Err(PermissionDenied)`); assert the stub-tool effect string is **absent**, the `{role:"tool"}` message carried "permission denied", and the turn still completes (mirrors `permission_deny_*`).
- **DoD-4 — cancel:** a `cancel()` mid-stream ends the turn with `Done{stop_reason="cancelled"}`.
- **DoD-5 — error:** a 500 / connection-refused stub → the stream yields `Err(AgentCrashed)`, no restart.

**Provenance (the non-ACP corpus analog):**
- **DoD-6 — `crates/bridge-api/tests/fixtures/ollama-openai-compat.json`** carries a `_provenance: "REAL-CAPTURE"` header (model `qwen3.5:9b`, captured date, capture harness) + the real captured request/SSE frames used to build the wiremock stubs. A presence test asserts the fixture exists and is REAL-CAPTURE (parallels `real_capture_corpus_present`).

**Gated live (manual, `#[ignore]`):**
- **DoD-7 — `api_live_two_turns`** against real local Ollama (`OLLAMA_BASE_URL` or `http://localhost:11434/v1`, model `qwen3.5:9b`): turn-1 a text prompt → asserts non-empty agent text; turn-2 a prompt that **explicitly instructs tool use** (e.g. "What time is it? You must call the get_current_time tool.") under auto-approve → asserts a `Permission` was surfaced, the tool ran (stub string present), and `Done`. The **deterministic** guarantees of the tool approve/deny control-flow are owned by the offline DoD-2/3; DoD-7 is the real-endpoint smoke that the same path works against a live OpenAI-compatible server. Documented run steps: `brew install ollama && ollama serve && ollama pull qwen3.5:9b`, then `cargo test -p bridge-api -- --ignored api_live_two_turns`.

**Registry / wiring:**
- **DoD-8** — a `kind="api"` entry loads, validates (cmd-forbidden / base_url-required), resolves through `Registry`, and `bin/a2a-bridge` boots with an `api` agent alongside the ACP agents. An `e2e` adds the `api` entry to the multi-agent snapshot.

**Coverage (HARD CI gates, measured after `cargo llvm-cov clean --workspace`):** workspace **85** (unchanged); a **new `bridge-api` floor at 90** added to `.github/workflows/ci.yml` `--fail-under-lines`; bridge-core 90 / bridge-acp 90 unchanged. (Domain changes in §3 must not drop bridge-core below 90.)

**Quality gate:** `cargo clippy --all-targets -- -D warnings` clean (the two-arm `match entry.kind` is trivially clippy-clean); `cargo fmt --check`; full `cargo test` green.

## 7. Scope boundary

**BUILDS:** the `bridge-api` crate (`ApiBackend` + `ApiConfig` + the stub tool + SSE/tool-call parsing + the policy-driven tool loop); the `AgentKind::Api` variant + `parse_kind` + factory arm; the §3 domain ripple (`cmd`→`Option`, `base_url`/`api_key_env` fields, validation, allowed_cmds skip, reuse-identity); the wiremock offline suite (DoD-1..5), the REAL-CAPTURE fixture (DoD-6), the gated live Ollama test (DoD-7), the registry/boot wiring (DoD-8); the CI coverage floor; an ADR (0007) recording the API backend + the surface-A/B conductor evidence + the tool-blindness finding.

**NON-GOALS (YAGNI):** **port enrichment** (tool name/args → policy / `SessionContext`) — the documented follow-on; **interactive** tool-permission suspend/resume to the A2A caller (would need warm-session state we just retired); **multiple or real tools**, fs/terminal capability; non-OpenAI-compatible vendors (Anthropic-native, etc. — a config swap to any OpenAI-compat endpoint needs no code; truly different schemas are out); per-session warm state / prompt-cache optimization (completions are stateless); a production model policy (Ollama/`qwen3.5:9b` is a dev/test choice). **The conductor re-evaluation stays parked** — this increment produces its evidence; it does not make the decision.

## 8. Conductor evidence (summary for the parked re-eval)

| Surface | What this increment shows | Where |
|---|---|---|
| A — lifecycle/transport | The ports absorb a non-process backend; the exact exec-centric ripple (`cmd`→`Option`, `allowed_cmds` skip, factory arm, reuse tuple) is enumerated and bounded. | §3 |
| B — permission/policy | The existing thin `PolicyEngine`/`Update::Permission` port absorbs a structurally different (client-side function-calling) permission model with **zero domain change**, deny-path proven. | §4.3, DoD-2/3 |
| Finding | The port is **tool-blind** (no tool name/args to the policy) — port-enrichment is the clean follow-on the conductor can weigh separately. | §4.4 |

## 9. Review

_Dual review (Codex `gpt-5.5` + Claude `opus-4.8`) pending — launched detached via the `~/code/a2a-local-bridge` tooling. Findings fold into Revision 2 before the implementation plan._
