# A2A Bridge — Vendor-neutral OpenAI-compatible API backend (`kind="api"`) Design

**Goal:** Add a **non-process** `AgentBackend` that speaks the **OpenAI-compatible** HTTP API (`POST {base_url}/chat/completions`), registered as a new `kind="api"` entry. It is the cheap/free replacement for the originally-scoped Claude-specific **B1** (`ClaudeApi`, Anthropic Messages API, needs a paid key). Validated live at **$0** against a local **Ollama** tool-capable model. It deliberately exercises the **two port surfaces the parked conductor decision must weigh**: **A = lifecycle/transport** (no `Supervised` child, `cmd`→optional, a URL-bearing entry, SSE streaming → `Update::Text`) and **B = permission/policy** (a `tool_call` → the existing `PolicyEngine`, mirroring `AcpBackend`).

**Architecture:** A new crate `crates/bridge-api` holds `ApiBackend`, implementing `bridge_core::ports::AgentBackend` over the workspace `reqwest`. It owns **no child process** — it holds a `base_url` + a `reqwest::Client`. The whole prompt turn (stream text, request a tool, decide via policy, execute the stub tool or deny, loop to a final answer) runs **inside `prompt()`** — no warm-session / suspend-resume state machine (the bridge-claude complexity we just retired is NOT reintroduced). The `AgentKind` seam (kept `Acp`-only after the bridge-claude retirement, expressly for this) re-expands to `{ #[default] Acp, Api }`; the `main.rs` factory gains a second arm that builds the HTTP backend.

**Tech stack:** Reuses the workspace `reqwest` 0.12 (already a dep of `bridge-a2a-outbound` + `bin/a2a-bridge`, already in `Cargo.lock`) + `serde`/`serde_json` + `futures`/`tokio-stream`. One new **dev-dep**: `wiremock` (offline mock HTTP server for deterministic replay tests). Live gate: local Ollama (`qwen3.5:9b` default; `OmniCoder-9B` a config swap — both expose OpenAI-compatible streaming tool calls via Ollama). No new runtime dep classes; no paid API; no key for the local gate.

**Spec status:** brainstormed; design approved 2026-06-01; grounded in the current code (post-bridge-claude-retirement, verified against `domain.rs`/`ports.rs`/`acp_backend.rs`/`main.rs`/`config.rs`/`translator.rs`/`registry.rs`/`permission.rs`). **Revision 2 — dual review (Codex `gpt-5.5` + Claude `opus-4.8`) folded** (2 blockers + majors; see §9). The two decisive corrections: the backend now **decides tool permissions silently** (no `Update::Permission` emission — the prior emission tripped the translator's `Err`-driven suspend, §4.3/§9-B1), and §3 now enumerates the **complete** blast radius including the load-bearing `registry::validate` gate (§9-B2).

**Firewall:** the `~/code/a2a-local-bridge` Python PoC is referenced black-box only; nothing in its schema/methodology informs this design. All wire shapes here derive from the public OpenAI/Ollama HTTP API and the Rust codebase.

---

## 1. Why — the conductor framing (what evidence this produces)

ADR-0002→0005 deferred the **fork-conductor-vs-greenfield** decision to "post-3c, a second protocol family / a non-process backend." Retiring `bridge-claude` (ADR-0006) left the bridge **ACP-only** — a single backend kind, all local-process. The conductor's decisive question is **"do the ports absorb a NON-process backend cleanly?"** This increment answers it across both surfaces, at $0:

- **Surface A — lifecycle/transport.** A process backend is launched via `cmd`/`args` and supervised (`Supervised` child). A non-process backend has **no child** — it has a `base_url`. Forcing `cmd` → `Option` and making `allowed_cmds` not apply to it is *the* exec-centric residue the conductor wants to measure. The **size of that ripple is the deliverable** (§3).
- **Surface B — permission/policy.** The bridge's `PolicyEngine` port governs "the agent wants to do X; the policy decides." OpenAI function-calling is a **structurally different** permission model (client-side: the model emits `tool_calls`, the *client* decides and feeds back a result — there is no agent blocking over a wire). The API backend routes each `tool_call` through the **same `PolicyEngine` port `AcpBackend` uses internally** (`decide_permission`) — consulted *inside the backend*, not surfaced as `Update::Permission` (§4.3 explains why the emission was removed). Showing that port absorbs a second permission model **with zero change to the port itself** is strong evidence it is general — *plus* a documented finding that the port is tool-blind (§4.4) and that its `Update::Permission`/translator suspend path is **not reusable** for non-interactive client-side denials without enrichment (§9-B1).

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

This is the deliverable, not an accident. We make the exec-centric fields optional **honestly** (dodging via the freeform `extensions` map is explicitly rejected — it would hide the very signal the conductor needs). **Rev2 — the review proved the original enumeration both undercounted and would not compile**; the complete, build-verified site list follows.

**`crates/bridge-core/src/domain.rs`**
- `AgentEntry.cmd: String` → **`Option<String>`**.
- Add **`pub base_url: Option<String>`** (peer to `cmd` — the non-process "where", parallel to the process "how to launch").
- Add **`pub api_key_env: Option<String>`** (the *name* of an env var holding a bearer token; never the secret).
- `AgentEntry { … }` literals to update with `cmd: Some(..)`, `base_url: None`, `api_key_env: None`: `domain.rs` tests (`agent_entry_carries_kind`, `effective_config_layers_override_over_entry`), `route.rs:95`, `bridge-a2a-inbound/src/server.rs:1683`.

**`bin/a2a-bridge/src/config.rs`**
- The raw TOML struct's `cmd` becomes `Option<String>`; add `base_url`/`api_key_env`.
- `into_snapshot` (`:164`): `allowed_cmds` default union (`:170`) **skips `None`** cmds (`filter_map(|a| a.cmd.clone())`); assign the new fields onto `AgentEntry`.
- **Parse-shape validation (defense-in-depth):** in `into_snapshot`, `kind="acp"` requires `cmd = Some(..)`; `kind="api"` requires `base_url = Some(..)` and **forbids `cmd`** — fail loud as `ConfigError::Registry`. (The canonical invariant ALSO lives in `registry::validate`, below — that is the authoritative guard; this is the earlier, friendlier TOML-shape error.)
- `config.rs` test literals/snapshots that set `cmd`.

**`crates/bridge-registry/src/registry.rs` — the load-bearing surface-A site (review B2):**
- **`validate` (`:102-106`)** currently does `!snap.allowed_cmds.iter().any(|c| c == &e.cmd)` and `format!("cmd not allowed: {}", e.cmd)`. Under `cmd: Option<String>` this **fails to compile** (`String` vs `Option<String>`; `Option` has no `Display`) **and** would wrongly reject `kind="api"` entries (`cmd=None`). Fix: only enforce the allowed-cmds gate **when `e.cmd` is `Some`**; for `None`/non-process entries skip it. **Add the kind-invariant here** (acp⇒`cmd.is_some()`; api⇒`base_url.is_some()` && `cmd.is_none()`) so it covers `Registry::new` (boot), `apply()` (reconcile), **and** the future `ConfigStore::upsert(entry: AgentEntry)` (`ports.rs:161`) which bypasses `into_snapshot` entirely.
- **reuse-identity (`:245`)**: `c.cmd == e.cmd` now compares `Option<String>` (logic unchanged); add `c.base_url == e.base_url` so a base_url edit re-spawns the slot. **`model`/`api_key_env` do NOT join the tuple** — staleness is handled in §4.1 (per-prompt key resolution + `configure_session` model stash), mirroring how ACP keeps `model` out of the tuple and applies it per-session.
- **registry test literals/mutations:** `registry.rs:349` literal + `:559`/`:646` `.cmd = "…".into()` mutations → `Some("…".into())`; **add a `base_url_change_replaces_slot` test** parallel to the existing `cmd_change_replaces_slot`.

**`bin/a2a-bridge/src/main.rs`** — the factory (`:107`):
- `AgentKind::Acp =>` arm now reads `let cmd = entry.cmd.as_deref().ok_or(BridgeError::ConfigInvalid { reason: "acp entry missing cmd".into() })?;` before `AcpBackend::spawn(cmd, …)`. (cwd computation stays in this arm; an Api backend needs no cwd.)
- New `AgentKind::Api =>` arm builds `ApiConfig` from the entry and constructs `ApiBackend::new(cfg).with_policy(policy)`. No `Supervised`, no cwd, no `cmd`.

**`bin/a2a-bridge/tests/` — the production-shaped e2e factory (review B2):**
- `tests/e2e_registry.rs:114` has a **second** spawn factory that calls `AcpBackend::spawn(&entry.cmd, …)`. It must become **kind-aware** (add an `AgentKind::Api` arm building a real `ApiBackend`) so **DoD-8's `api` entry exercises the real backend**, not a panic/skip. Update its `&entry.cmd` to the `Option` form.
- `AgentEntry` literals in `tests/e2e_registry.rs:211` and `tests/common/mod.rs:23`.

**Clippy/dead-code:** the three new fields are `pub` and read by the factory/validate — no `ext_u64`-style orphan; the two-arm `match entry.kind` is clippy-clean. The change is **mechanical but wider than rev1 claimed** (~10 sites incl. the validate gate + the e2e factory). No behavior change for existing ACP entries. This enumeration is the conductor's absorption-cost measurement.

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
    policy: Arc<StdMutex<Arc<dyn PolicyEngine>>>, // default auto-approve; with_policy swaps it
    sessions: Arc<StdMutex<HashMap<SessionId, SessionState>>>, // model stash + cancel latch
}
// SessionState { model: Option<String>, cancel: Arc<AtomicBool> }

impl ApiBackend {
    pub fn new(cfg: ApiConfig) -> Self { /* default policy = AutoApprove (mirrors AcpBackend) */ }
    #[must_use] pub fn with_policy(self, policy: Arc<dyn PolicyEngine>) -> Self { /* swap, builder-style */ }
}
```

`ApiBackend` carries **no per-session conversation state** (an OpenAI completion is stateless, so `prompt(session, parts)` builds the message list from `parts` and runs one full turn). Per-session state is a small `SessionState` map holding only (a) the **effective model** and (b) the **cancel latch** (§4.6).

**Config-staleness fix (review F4).** A config-only edit (model / api_key_env) keeps the warm slot (§3 reuse-identity), so neither may be frozen at construction:
- **`api_key`** is resolved **per `prompt()`** by reading `std::env::var(api_key_env)` each turn — never cached. A rotated key is picked up with no re-spawn; absent var ⇒ no `Authorization` header (Ollama needs none).
- **`model`** is applied via **`configure_session`** (implemented, NOT the trait no-op): the binding flow already calls `configure_session(session, EffectiveConfig{model,…})` with the entry default layered under any per-request `AgentOverride`; the backend stashes `EffectiveConfig.model` into `SessionState`, and `prompt()` reads it (falling back to `cfg.model`). This mirrors `AcpBackend` exactly (model out of the reuse tuple, applied per-session) and — unlike the rev1 no-op — makes **per-request model overrides actually take effect**.

`forget_session` drops the session's `SessionState` entry (stash + latch); `retire` uses the trait default. `cancel` is overridden (§4.6).

### 4.2 The turn loop (inside `prompt()` → one `BackendStream`)

`prompt()` returns a `BackendStream` (an `async_stream`/channel) that runs:

1. Seed `messages = [{role:"user", content: <joined parts.text>}]`; resolve `model` (session stash → `cfg.model`) and `api_key` (per-prompt env read, §4.1).
2. **Loop** (≤ `max_tool_rounds`):
   a. `POST {base_url}/chat/completions` with `{ model, messages, tools: [TIME_TOOL], stream: true }` (+ `Authorization: Bearer` if a key resolved). **Check the cancel latch inside the per-chunk SSE read loop** (§4.6) so a mid-stream cancel aborts promptly.
   b. Parse the SSE stream (§5, **tolerant**): emit `Update::Text(delta.content)` as text arrives; accumulate any `delta.tool_calls[]` fragments (by `index` when present, else positionally — Ollama may omit `index`).
   c. If the turn ended with **no** accumulated tool calls (`finish_reason == "stop"` or stream end) → emit `Update::Done { stop_reason: "stop" }` and END.
   d. If tool calls were accumulated (regardless of whether `finish_reason` is `"tool_calls"` or `"stop"` — Ollama issue #7881 uses `"stop"`) → for **each** tool call: run §4.3 (decide **silently** via policy + execute/deny — **no `Update::Permission` is emitted**), appending the assistant tool-call message + a `{role:"tool", tool_call_id, content}` result message; then continue the loop (follow-up completion).
3. If the loop hits `max_tool_rounds` without a terminal stop, emit `Update::Done { stop_reason: "max_tool_rounds" }` (bounded; no infinite tool loops).

The only `Update`s this backend ever yields are **`Text`** and **`Done`** (never `Permission`) — see §4.3 for why.

### 4.3 Tool call → permission (surface B — the thin port, decided *silently*)

The backend consults the injected `PolicyEngine` **internally and silently** for each tool call — it routes through the **same port** `AcpBackend::decide_permission` (`acp_backend.rs:732`) uses, and **does NOT emit `Update::Permission`**:

```rust
let perm = PermissionRequest::with_id(tool_call.id.clone(), /*interactive=*/ false);
// NO `yield Update::Permission` — the backend is the sole authority (see "Why silent" below).
let decision = policy.lock().ok().map(|p| p.decide(&perm, &SessionContext));
match decision {
    Some(Ok(PermissionDecision::Approve)) => { let result = run_tool(&tool_call)?; push_tool_result(&tool_call, result); }
    Some(Err(BridgeError::PermissionDenied)) => { push_tool_result(&tool_call, "permission denied: tool not executed"); }
    _ /* abstain (other Err) / poisoned lock */ => { push_tool_result(&tool_call, "permission unavailable: tool not executed"); }
}
// In every arm a `{role:"tool", tool_call_id, content}` message is appended so the model can continue;
// on Approve the content is the tool result, on Deny/abstain a refusal string. The loop (§4.2) then re-POSTs.
```

**Why silent (review B1 — the decisive correction).** The rev1 design emitted `Update::Permission(.., interactive:false)` as an "observable signal," on the false premise that the translator "suspends only on `interactive=true`." It does not: `translator.rs:140` does `match policy.decide(&req,&ctx) { Ok => continue, Err => put_pending + Err(PermissionRequired) }` — it **never reads `interactive`**; non-interactive only "continues" because `AutoPolicy` (`permission.rs`) returns `Ok` for it. Since `main.rs` threads the **same policy `Arc`** into both the backend and the translator, a **deny** policy would make the backend deny-and-continue *while* the translator independently **suspends the A2A task** with an **unresumable** `PendingRequest` (this backend has no resume — §7). Emitting `Update::Permission` is therefore unsafe under any non-AutoPolicy. **Resolution:** the backend decides silently and yields only `Text`/`Done` — which is precisely how `AcpBackend` resolves the agent's reverse `session/request_permission` (internally, never as an `Update::Permission` to the translator). The translator's `Update::Permission` path remains exclusively for ACP's *interactive* asks.

- **Verdict mapping** is the **analog** of `decide_permission` (not identical): Approve→execute+result; `Err(PermissionDenied)`→denial result; abstain/poisoned→refusal result. (It deliberately diverges from ACP's abstain→`Cancelled`: an HTTP completion has no "cancel this one tool" outcome, so abstain becomes a refusal-and-continue. The abstain arm is **explicitly tested**, §6.)
- The policy is **tool-blind** (`PermissionRequest` carries only `request_id`+`interactive`; `SessionContext` is empty) — *as it already is for ACP*. We do NOT enrich it (decided: reuse the thin port). Recorded as a conductor finding (§4.4), not a defect.
- **Deny is honest & falsifiably tested:** on `Err(PermissionDenied)` the stub-tool effect string is NOT produced, the `{role:"tool"}` content is the denial, AND — run through `Translator::run` — **no pending is persisted and the task does not suspend** (§6 DoD-3, the test that actually catches a regression here).

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

- **`cancel(session)`**: sets the session's `AtomicBool` latch. The stream **polls the latch inside the per-chunk SSE read loop** (not merely between rounds — review F9, so DoD-4's *mid-stream* cancel aborts promptly), drops the in-flight `reqwest` response stream, and ends the turn with `Update::Done { stop_reason: STOP_REASON_CANCELLED }` (`ports.rs:17`, the shared cancelled-spelling const — reused so translator/​adapter agree).
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
- **Tool-call assembly (TOLERANT — review Codex-F3).** Streamed `tool_calls` fragments: `id`/`function.name` appear once, `function.arguments` is concatenated across deltas. The backend buffers per `index` **when present**; Ollama (issue #7881) may omit `index` and/or finish with `finish_reason:"stop"` rather than `"tool_calls"`, so assembly keys defensively (positional fallback) and treats "any accumulated tool calls" as a tool round regardless of the finish_reason string. The arguments JSON is parsed only at the end; partial/whitespace fragments are tolerated.
- **`stream:false` fallback (a DIFFERENT response shape).** A non-streamed response is **not** an SSE-delta stream — it is one JSON object with `choices[0].message.tool_calls` (and `message.content`), a *separate parse path* from `delta.tool_calls`. The backend supports a non-streaming mode (config flag, default streaming) for endpoints/models that don't reliably stream tool calls; the design does **not** pretend "the same accumulator" handles both.
- **Follow-up turn** after a tool result appends `{"role":"assistant","tool_calls":[…]}` then `{"role":"tool","tool_call_id":"call_1","content":"<result>"}` and re-POSTs.

**Compatibility pin.** Verified against Ollama's OpenAI-compatibility + tool-calling docs and issue #7881 (streamed tool calls without `index`, finishing `"stop"`). The live gate pins **Ollama ≥ a tool-streaming-capable release** and a tool-capable model (`qwen3.5:9b` default); the spec does NOT assume byte-exact OpenAI delta shape — hence the tolerant parser + the `stream:false` fallback.

## 6. Testing & Definition of Done

**Offline / CI (deterministic, `wiremock`):**
- **DoD-1 — text round-trip:** stub `/chat/completions` with a recorded text-only SSE body; `prompt()` yields `Text…` then `Done{stop}`. The backend yields **only `Text`/`Done`** — assert **no `Update::Permission` ever appears** (locks in the §4.3 silent-decision invariant).
- **DoD-2 — tool approve path:** stub returns (call-1) a `tool_calls` SSE, then (call-2) a final text SSE. With **auto-approve**, assert the `Update` order is `Text* , Text* , Done` (**no `Permission`**), the second HTTP request body contains the `{role:"tool"}` result, and the final text appears. Assert the **exact** recorded second-request JSON (tool_call_id + tool content), not a substring — guarding the UUID-'7'-style false-pass.
- **DoD-3 — tool deny path, THROUGH THE TRANSLATOR (review F5/Codex-F4 — the test that catches B1):** drive the api turn via **`Translator::run`** with a `DenyPolicy` (`decide → Err(PermissionDenied)` for the non-interactive req). Assert: (a) the stub-tool effect string is **absent**; (b) the second HTTP request's `{role:"tool"}` content is the denial; (c) **`store.take_pending()` is `None`** and the run **completes** (does NOT yield `PermissionRequired`) — i.e. the backend's silent decision does not trip the translator's suspend path. A direct-`prompt()` variant additionally asserts the raw `Update` sequence. (Driving only `prompt()` — as rev1 planned — structurally could not see the suspend divergence.)
- **DoD-4 — mid-stream cancel:** stub a slow/long SSE body; `cancel()` **while chunks are still arriving** ends the turn with `Done{stop_reason="cancelled"}` promptly (exercises the in-loop latch poll, §4.6).
- **DoD-5 — error:** a 500 response and a connection-refused base_url → the stream yields `Err(AgentCrashed)`, no restart.
- **DoD-5b — malformed SSE / abstain / stream:false (review F7):** (i) a truncated/garbage SSE frame → frame `Err`, no restart; (ii) an **abstain** policy (`decide → Err(other)`, not `PermissionDenied`) → the `{role:"tool"}` content is the "permission unavailable" refusal and the turn completes (covers the §4.3 `_` arm); (iii) a `stream:false` stubbed response with `message.tool_calls` → the non-streaming parse path produces the same approve-path behavior. These exist to make the **HARD 90% floor** reachable deterministically (the `#[ignore]` live test adds no coverage).

**Provenance (the non-ACP corpus analog — review F8/Codex-F5):**
- **DoD-6 — `crates/bridge-api/tests/fixtures/ollama-openai-compat.json`** carries a `_provenance: "REAL-CAPTURE"` header (model `qwen3.5:9b`, captured date, capture harness) + the real captured request/SSE frames. **These captured frames ARE the source of the DoD-1/2 wiremock stub bodies (single source of truth), and a replay test feeds them through the real SSE/tool-call parser** — not a mere presence check (which would be forgeable theater for an HTTP backend). A provenance test still asserts the header is REAL-CAPTURE (parallels `real_capture_corpus_present`).

**Gated live (manual, `#[ignore]`):**
- **DoD-7 — `api_live_two_turns`** against real local Ollama (`OLLAMA_BASE_URL` or `http://localhost:11434/v1`, model `qwen3.5:9b`): turn-1 a text prompt → asserts non-empty agent text; turn-2 a prompt that **explicitly instructs tool use** (e.g. "What time is it? You must call the get_current_time tool.") under auto-approve → asserts the model emitted a tool call, the stub tool ran (its result reached the follow-up request), and the turn reached `Done` with a final text answer. (No `Permission` is emitted — §4.3.) The **deterministic** guarantees of the approve/deny/abstain control-flow are owned by the offline DoD-2/3/5b; DoD-7 is the real-endpoint smoke that the same path works against a live OpenAI-compatible server. Documented run steps: `brew install ollama && ollama serve && ollama pull qwen3.5:9b`, then `cargo test -p bridge-api -- --ignored api_live_two_turns`.

**Registry / wiring:**
- **DoD-8** — a `kind="api"` entry loads, **validates through `registry::validate`** (cmd-forbidden / base_url-required — assert both rejection paths *and* the allowed-cmds gate is skipped for it), resolves through `Registry`, and `bin/a2a-bridge` boots with an `api` agent alongside the ACP agents. The e2e adds the `api` entry to the multi-agent snapshot **via the kind-aware test spawn factory (§3, `e2e_registry.rs:114`) so it exercises the real `ApiBackend`** (pointed at a `wiremock` stub), not a panic/skip. Plus the **`base_url_change_replaces_slot`** reuse-identity test (§3).

**Coverage (HARD CI gates, measured after `cargo llvm-cov clean --workspace`):** workspace **85** (unchanged); a **new `bridge-api` floor at 90** added to `.github/workflows/ci.yml` `--fail-under-lines`; bridge-core 90 / bridge-acp 90 unchanged. (Domain changes in §3 must not drop bridge-core below 90.)

**Quality gate:** `cargo clippy --all-targets -- -D warnings` clean (the two-arm `match entry.kind` is trivially clippy-clean); `cargo fmt --check`; full `cargo test` green.

## 7. Scope boundary

**BUILDS:** the `bridge-api` crate (`ApiBackend` + `ApiConfig` + the stub tool + SSE/tool-call parsing + the policy-driven tool loop); the `AgentKind::Api` variant + `parse_kind` + factory arm; the §3 domain ripple (`cmd`→`Option`, `base_url`/`api_key_env` fields, validation, allowed_cmds skip, reuse-identity); the wiremock offline suite (DoD-1..5), the REAL-CAPTURE fixture (DoD-6), the gated live Ollama test (DoD-7), the registry/boot wiring (DoD-8); the CI coverage floor; an ADR (0007) recording the API backend + the surface-A/B conductor evidence + the tool-blindness finding.

**NON-GOALS (YAGNI):** **port enrichment** (tool name/args → policy / `SessionContext`) — the documented follow-on; **interactive** tool-permission suspend/resume to the A2A caller (would need warm-session state we just retired); **multiple or real tools**, fs/terminal capability; non-OpenAI-compatible vendors (Anthropic-native, etc. — a config swap to any OpenAI-compat endpoint needs no code; truly different schemas are out); per-session warm state / prompt-cache optimization (completions are stateless); a production model policy (Ollama/`qwen3.5:9b` is a dev/test choice). **The conductor re-evaluation stays parked** — this increment produces its evidence; it does not make the decision.

## 8. Conductor evidence (summary for the parked re-eval)

| Surface | What this increment shows | Where |
|---|---|---|
| A — lifecycle/transport | The ports absorb a non-process backend; the exact exec-centric ripple (`cmd`→`Option`, the `registry::validate` allowed-cmds gate, factory + e2e-factory arms, reuse tuple, ~10 sites) is enumerated and bounded. | §3 |
| B — permission/policy | The `PolicyEngine` **port itself** absorbs a structurally different (client-side function-calling) model with **no change to the port** — the backend routes each `tool_call` through it internally (as `AcpBackend` does), deny/abstain proven through the translator. | §4.3, DoD-3/5b |
| Finding (refined by review) | The port's **`Update::Permission`/translator suspend path is NOT reusable** for non-interactive client-side denials (it keys on policy `Err`, not `interactive`, and the backend has no resume) — so per-tool/non-interactive permission needs port enrichment. Combined with tool-blindness, this is the concrete, conductor-weighable evidence for what a non-process permission model costs the current ports. | §4.3, §4.4 |

## 9. Review

**Revision 2 folds the dual review** (Codex `gpt-5.5`, status `review_required`; Claude `opus-4.8`, status `input_required`), launched detached via the `~/code/a2a-local-bridge` tooling against the rev1 commit `3d311a6`. Both converged independently on the two blockers.

- **B1 — double-decision hazard (both, BLOCKER).** Rev1 emitted `Update::Permission(interactive:false)` *and* decided internally; the translator (`translator.rs:140`) suspends on policy `Err`, not on `interactive`, so a deny policy diverged (backend continues, translator suspends unresumably). **Folded:** backend decides **silently**, yields only `Text`/`Done` (§4.3); evidence claim corrected (§1/§8); the catching test is DoD-3 *through the translator* (§6).
- **B2 — incomplete/non-compiling blast radius (both, BLOCKER).** Rev1 missed the load-bearing `registry::validate` allowed-cmds gate (`e.cmd: String` + `format!(e.cmd)` — won't compile under `Option`, wrongly rejects api entries), the e2e spawn factory, and several test literals. **Folded:** §3 enumerates all ~10 sites + the validate fix + the kind-aware e2e factory.
- **F3 (Claude, major) — validation placement.** **Folded:** canonical kind-invariant in `registry::validate` (covers boot/reconcile/future `upsert`) + a friendlier parse-shape check in `into_snapshot` (§3).
- **F4 (Claude, major) — config staleness.** **Folded:** `api_key` resolved per-prompt from env; `model` via implemented `configure_session` (not the no-op) — staleness gone without bloating the reuse tuple; per-request overrides now work (§4.1).
- **Codex-F3 (major) — Ollama tool-call shape.** **Folded:** tolerant assembly (missing `index`, `finish_reason:"stop"` with tool calls) + an explicit `stream:false` fallback as a *distinct* `message.tool_calls` shape + a version/model pin (§5).
- **F5 (Claude) / Codex-F4 — deny test false-pass.** **Folded:** DoD-3 runs through `Translator::run`, asserts no pending + exact `{role:"tool"}` payload (§6).
- **F6–F9 / F7 / F8 (minors).** **Folded:** abstain arm defined + tested (§4.3/DoD-5b); "mirrors `decide_permission` exactly" → "analog" (§4.3); offline tests for malformed-SSE/mid-stream-cancel/abstain/`stream:false` to reach the 90% floor (§6); REAL-CAPTURE frames are the wiremock stub source + replayed through the parser (§6 DoD-6); cancel polled inside the SSE loop (§4.6/DoD-4).

Reviewers' three questions resolved: (1) **decide silently**; (2) **`registry::validate` + `into_snapshot`**; (3) **per-prompt key + `configure_session`** (no reuse-tuple growth). Confirmed-correct by both (no change needed): the SSE turn loop / tool-call accumulator / `max_tool_rounds` bounding (§4.2/§5) and single-plan sizing.
