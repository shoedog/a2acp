# A2A Bridge Increment 3c — Claude adapter (warm Claude Code, kind-seam) Design

**Goal:** Add Claude as a registered agent via a **new non-ACP `AgentBackend`** (`bridge-claude`) that drives a **warm interactive Claude Code process per bridge session** (`claude --input-format stream-json --output-format stream-json`, reusing the logged-in subscription — no API key), behind an **adapter-`kind` seam** on the registry so the same factory can later flex to an HTTP Anthropic-API backend (B1) or the ACP-Claude adapter (A) with no redesign.

**Architecture:** `AgentEntry` gains a `kind` discriminator (`acp` default | `claude-cli` new | `claude-api` future). The registry's backend factory dispatches on `kind`: `acp → AcpBackend::spawn` (today), `claude-cli → ClaudeCliBackend::spawn` (3c). `ClaudeCliBackend` (new crate `bridge-claude`) mirrors `AcpBackend`'s warm-process machinery (Supervised child, a stdout reader task demuxing NDJSON to per-turn channels, per-session turn lock, lazy exactly-once spawn) but **keyed per bridge `SessionId`** (stream-json is one conversation per process). Turns are stream-json user envelopes in / assistant deltas + a terminal `result` out → `Update::{Text, Done}`. Reuses the login; cancel via a stream-json interrupt (or terminate the session process). The `claude-api` (B1) arm is **designed-in, not built** — its presence in the dispatch is the concrete test of whether the registry/`AgentEntry`/lease/retire machinery generalizes to a non-process transport (the ADR-0005 conductor litmus).

**Tech stack:** Rust 2021 (1.94), `tokio`, `serde_json`, `Supervised` (3a), `bridge-core`/`bridge-registry`/`bridge-acp`. `claude` CLI 2.1.159 (installed, logged in). No new HTTP deps (B1 deferred).

**Spec status:** brainstormed; Sections 1–2 approved live, 3–5 folded here. Tags `[probe]` = pinned by the Task-0 stream-json probe.

---

## 1. Why this shape (and the conductor framing)

ADR-0005 deferred the conductor fork/continue decision to "post-3c, when a second protocol family arrives." 3c delivers a **second `AgentBackend` impl** (non-ACP) and a **second adapter kind**, but `claude-cli` is still a **stdio child process** — so it does NOT by itself exercise the *non-process* dimension of the conductor litmus. What carries that dimension is the **`kind`-seam designed to hold a non-process `claude-api` (B1) arm**: building the factory + the registry's lease/binding/retire machinery so a future HTTP backend (no `Supervised` child) drops in *without domain change* is the test. So 3c's conductor contribution is: (a) prove the ports absorb a non-ACP **process** backend cleanly (claude-cli), and (b) design the non-process seam (claude-api) so the post-3c re-eval can judge whether greenfield holds. The definitive non-process exercise lands when B1 (or a Gemini-API backend) is implemented; the post-3c re-eval weighs all of it.

**Chosen path (B2):** warm Claude Code (Pattern 2) — reuses the subscription login (no API key, unlike A and B1), gives a genuine interactive multi-turn session, and fits our warm-backend model. Rejected: **A** (ACP-Claude adapter — same family AND needs an API key, rejecting subscription OAuth) and **B1** (Anthropic API direct — the strongest non-process test but needs an API key) — both designed-in via the kind-seam for later, neither built now.

## 2. The adapter-`kind` seam

`bridge_core::domain::AgentEntry` gains:
```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AgentKind { #[default] Acp, ClaudeCli /* , ClaudeApi (future B1) */ }
// AgentEntry { ..., kind: AgentKind }
```
TOML `[[agents]]` gains `kind = "acp" | "claude-cli"` (serde `rename_all = "kebab-case"`), **defaulting to `acp`** so every existing entry + config is unchanged (back-compat). `RegistryConfig`/`into_snapshot` (3b's `bin/a2a-bridge/src/config.rs`) parse it; invalid kind → `ConfigError::Registry`.

**The backend factory dispatches on kind.** 3b's `SpawnFn` (built in `bin/a2a-bridge/src/main.rs`, capturing the policy) becomes a kind-dispatcher:
```rust
let spawn: SpawnFn = Arc::new(move |entry: Arc<AgentEntry>| {
    let policy = policy.clone();
    Box::pin(async move {
        match entry.kind {
            AgentKind::Acp       => { let be = AcpBackend::spawn(&entry.cmd, &args, acp_config(&entry)?).await?.with_policy(policy); Ok(Arc::new(be) as Arc<dyn AgentBackend>) }
            AgentKind::ClaudeCli => { let be = ClaudeCliBackend::spawn(&entry, policy).await?;                                   Ok(Arc::new(be) as Arc<dyn AgentBackend>) }
            // AgentKind::ClaudeApi => ClaudeApiBackend::new(&entry, ...)   // B1: NO Supervised child — the conductor litmus
        }
    })
});
```
The `bridge-registry` crate stays **kind-agnostic** (it only calls the `SpawnFn` + holds `Arc<dyn AgentBackend>`); the dispatch lives at the wiring layer. The registry's lazy-spawn / lease / binding / retirement machinery is **transport-agnostic** (it reasons over the backend `Arc` lifetime, not the process) — so a non-process `claude-api` would reuse it unchanged. `allowed_cmds` validation (3b) still applies (`claude-cli` uses `cmd="claude"` → add `"claude"` to `allowed_cmds`).

A `claude-cli` entry:
```toml
[registry]
allowed_cmds = ["kiro-cli", "codex-acp", "claude"]
[[agents]]
id   = "claude"
kind = "claude-cli"
cmd  = "claude"
args = []                       # extra claude flags (perm/tools); see §4 auth/trust
model = "claude-opus-4-8"       # → --model at warm-process spawn (best-effort)
cwd  = "/abs/work/dir"          # absolute (3b resolves relative → current_dir)
model_provider = "anthropic"    # descriptive only
```

## 3. `bridge-claude` — `ClaudeCliBackend` (warm process per session) [§ Section-2 approved]

New crate `crates/bridge-claude`. `ClaudeCliBackend` implements `AgentBackend` (`prompt`/`cancel` + 3b's `configure_session`/`forget_session`/`retire`).

**State:** a per-bridge-`SessionId` map of warm processes:
```
sessions: Arc<Mutex<HashMap<SessionId, Arc<SessionProc>>>>   // lazy, exactly-once spawn per session (OnceCell pattern from 3a)
SessionProc { stdin: <NDJSON writer>, turn_lock: Mutex<()>, updates: <reader→per-turn mpsc registry>, child: Supervised, model/mode: …, claude_session_id: Option<String> }
session_cfg: Arc<Mutex<HashMap<SessionId, EffectiveConfig>>>  // configure_session stash (applied at spawn)
```

**Lifecycle (mirrors `AcpBackend`, keyed per session):**
- **`configure_session(session, eff)`** → stash `eff` (applied at the session's lazy spawn).
- **`prompt(session, parts)`** → `ensure_session_proc(session)`: if absent, spawn `claude --input-format stream-json --output-format stream-json --verbose [--model <eff.model>] [perm flags] [entry.args…]` via `Supervised` (process-group + `SIGTERM→SIGKILL` reap, 3a); start a reader task draining stdout NDJSON; capture `session_id` from the init event. Acquire the **per-session turn lock**; write the user envelope (§3.1); the reader demuxes events to this turn's mpsc; return a `BackendStream` yielding `Update::Text` (assistant text deltas) then `Update::Done{stop_reason}` (the terminal `result` event). The process **stays warm** for the next turn.
- **Follow-up turn (same session)** → reuse the warm `SessionProc` (no respawn, no `--resume`) — the conversation context lives in the process. The 3b `TaskBinding` already routes a follow-up to the same backend `Arc`; this backend then reuses the same `SessionProc`.
- **`forget_session(session)`** → `Supervised::terminate` that session's proc + drop the map entry + the stash.
- **`retire`** → terminate ALL session procs (idempotent, take-once per proc).
- **`cancel(session)`** → send a stream-json **interrupt** control message if the probe (Task 0) confirms support (`{"type":"control_request","request":{"subtype":"interrupt"}}`-style), else `Supervised::terminate` the session's proc (hard cancel, loses warmth). Completion is the turn's terminal event / the process end.

### 3.1 stream-json wire shapes `[probe — pinned by Task 0]`
- **Write (stdin, NDJSON, one per turn):** `{"type":"user","message":{"role":"user","content":[{"type":"text","text":<part text>}]}}`.
- **Read (stdout, NDJSON):** an init event (`{"type":"system","subtype":"init","session_id":…,"model":…}`) at spawn; assistant text events (Claude Code emits `{"type":"assistant","message":{…content…}}`; with `--verbose`/`--include-partial-messages`, incremental `stream_event`/`content_block_delta` deltas) → `Update::Text`; the terminal `{"type":"result","subtype":"success","stop_reason":…,"session_id":…}` → `Update::Done`. **Tolerant reader** (3a): unmodeled event types / non-text content are dropped (no Update, no error). Exact field names/`--verbose` requirement for token-level deltas are pinned by Task 0.

### 3.2 Reuses proven `AcpBackend` patterns
Reader-loop → per-turn mpsc demux; per-session turn lock (one turn at a time; a second prompt waits); lazy exactly-once spawn (`OnceCell`); `Supervised` process hygiene; tolerant NDJSON reader; the streaming `BackendStream` shape (`Update::Text…` then `Update::Done`, `Err` on transport/process failure → the inbound producer maps to `Failed`, 3b).

## 4. Auth, trust, model/mode, errors

- **Auth — reuse the login (no API key):** `ClaudeCliBackend` spawns plain `claude` with NO `ANTHROPIC_API_KEY` env; the CLI's logged-in subscription OAuth is used as-is. If `claude` is not logged in, the spawn/first-turn fails → surface `BridgeError::AgentNotAuthenticated`. (Billing note: `claude -p`-style usage on a subscription may draw from a separate Agent-SDK credit pool from 2026-06-15 — documented in the README, not a code concern.)
- **Workspace trust:** Claude Code may gate on workspace trust for the spawn `cwd` (like Gemini). Handle via the entry's `cwd` (a trusted dir) and/or a permission/trust flag in `args` (e.g. `--permission-mode`); the Task-0 probe + the gated e2e confirm the exact flag needed for a non-interactive session.
- **model/mode:** `model` → `--model` at spawn (best-effort; an invalid model → the CLI errors → surface clearly). Claude CLI has no mid-session model switch, so model is **fixed per warm process** — `configure_session`'s model is applied at the session's spawn (mint-time-only, same semantic as `AcpBackend`; a per-request model override → a fresh session uses it). `mode`/permission → CLI permission flags (best-effort).
- **Errors:** process crash / stdout EOF mid-turn → `Err(AgentCrashed)` on the stream (→ `Failed`, 3b's distinction); an unparseable NDJSON line → tolerant drop; spawn failure → `AgentCrashed`/`AgentNotAuthenticated`; per-session lazy-spawn failure leaves the session unminted so a retry re-attempts (3a semantics).

## 5. Testing

- **Task 0 — stream-json probe (gate, must run first) `[probe]`:** a bounded local run of `claude --input-format stream-json --output-format stream-json --verbose` that (a) confirms the process **stays alive for a SECOND turn** (the make-or-break assumption for warm Pattern 2), and (b) captures the exact init / assistant-delta / `result` NDJSON event shapes + whether `--verbose`/`--include-partial-messages` is needed for token deltas + the interrupt control-message shape for cancel + the workspace-trust flag. **If turn-2 fails**, `ClaudeCliBackend` falls back INTERNALLY to Pattern 1 (`--resume` one-shot, server-side continuity) — an implementation detail of the `claude-cli` kind, NOT a redesign (the kind-seam absorbs it). Report back to the controller (gate), like 3a's SDK-discovery task.
- **Unit (in-process fake `claude`):** a tiny `/bin/sh` (or Rust test-bin) fake that reads stdin NDJSON envelopes and emits canned stdout NDJSON (init + text + result), mirroring `AcpBackend`'s fake-agent tests. Cover: prompt → `Update::Text`×N + `Update::Done{stop_reason}`; **multi-turn reuse** (two prompts on one session → the SAME warm proc handled both, sequentially — turn lock); tolerant reader (unmodeled event dropped); `forget_session` kills the proc; `retire` kills all + is idempotent; `cancel` (interrupt or kill); per-session isolation (two sessions → two procs, no bleed); spawn-failure retry.
- **Wire-golden (no tautology):** the outbound user envelope serializes as `{"type":"user","message":{"role":"user","content":[{"type":"text","text":…}]}}` — hand-authored expected, asserted against the value the backend writes.
- **Config/registry:** `kind = "claude-cli"` parses (+ default `acp` for omitted); the kind-dispatching factory builds the right backend; `allowed_cmds` includes `claude`.
- **Gated real e2e (run it — `claude` is installed + logged in):** register `claude` (`kind=claude-cli`) alongside `kiro`/`codex` (3 agents, 2 kinds) in one registry; route to `claude` by id → a warm multi-turn exchange (turn 1 "Remember the number 7", turn 2 "What number?" → proves the warm session retained context — the genuine interactive proof), reusing the login. Bound by timeout; `#[ignore]`-gated; RUN it once to confirm. Assert route-by-id reaches Claude (not kiro/codex) and the multi-turn continuity holds.
- **Coverage:** workspace ≥85%; `bridge-core` ≥90%; new `bridge-claude` ≥90% — after `cargo llvm-cov clean --workspace`.

## 6. Scope boundary
**3c BUILDS:** the `AgentKind` discriminator + config parse + the kind-dispatching factory; `bridge-claude`/`ClaudeCliBackend` (warm Pattern 2, with the Pattern-1 internal fallback if Task 0 shows per-turn exit); the gated multi-turn e2e. **3c DOCUMENTS / designs-in (not built):** the `claude-api` (B1) HTTP arm (the non-process conductor test) and the ACP-Claude adapter (A) as a `kind="acp"` config entry; the post-3c **conductor re-evaluation** (now better-informed: 2 ACP agents + 1 non-ACP-process agent + a designed non-process seam). **Non-goals:** no Anthropic API/HTTP code, no API key, no fan-out/registry changes beyond the `kind` field.

## 7. Review
After this spec is written + self-reviewed, run the established **dual review via the a2a-local-bridge tooling** before the plan: **Codex (gpt-5.5)** + **Claude (opus-4.8)**, firewalled. Fold findings; re-review if substantive. The plan then gets its own dual review.
