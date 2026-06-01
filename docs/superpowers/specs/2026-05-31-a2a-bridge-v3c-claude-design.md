# A2A Bridge Increment 3c — Claude adapter (warm Claude Code, kind-seam) Design

> **Revision 2** — folds the dual spec review (Codex gpt-5.5 + Claude opus-4.8). The reviews caught a blocker: a warm process keyed/torn-down per-turn is destroyed by 3b's per-turn `forget_session`/binding eviction (and an in-flight-turn race). **Fix (this rev): mirror `AcpBackend` exactly — `forget_session` drops only the config stash and never kills the process; the warm Claude process is keyed by `SessionId` and survives across same-TaskId turns; it is reaped only by a bounded warm-pool (idle-TTL ~55 min ≈ the Max prompt-cache window, LRU cap, reap on retire/shutdown), turn-lock-aware so a reap never fires mid-turn.** Plus: `kind` joins the slot-reuse identity; bounded spawn/init/turn/cancel timeouts; cancel→`Canceled` (not `Failed`); result-error-subtype→`Failed`; not-logged-in + workspace-trust as Task-0 detection targets; honest conductor framing (`cmd`/`allowed_cmds` is exec-residue a non-process backend will force a change to); an **inbound-level** sequential multi-turn e2e (direct-`prompt` tests would hide the blocker); Pattern-1 (`--resume`) is a co-equal documented fallback.

**Goal:** Add Claude as a registered agent via a new non-ACP `AgentBackend` (`bridge-claude` / `ClaudeCliBackend`) that drives a **warm interactive Claude Code process per bridge `SessionId`** (`claude --input-format stream-json --output-format stream-json`, reusing the logged-in subscription — no API key), behind an **adapter-`kind` seam** on the registry so the same factory can later flex to an HTTP Anthropic-API backend (B1) or the ACP-Claude adapter (A) with minimal change.

**Architecture:** `AgentEntry` gains a `kind` discriminator (`acp` default | `claude-cli` new | `claude-api` future). The registry's backend factory dispatches on `kind`. `ClaudeCliBackend` mirrors `AcpBackend`'s warm-process machinery (reader-task NDJSON demux, per-session turn lock, lazy exactly-once spawn via `OnceCell`, `Supervised` hygiene, bounded handshake/cancel timeouts, `forget_session`-drops-only-stash) — but is **one warm process per `SessionId`** (stream-json is one conversation per process) instead of `AcpBackend`'s one-process-many-sessions, so it adds a **bounded warm-pool** (idle-TTL / LRU / retire) to bound process count.

**Tech stack:** Rust 2021 (1.94), `tokio`, `serde_json`, `Supervised` (3a), `bridge-core`/`bridge-registry`. `claude` CLI 2.1.159 (installed, logged in). No HTTP deps (B1 deferred).

**Spec status:** brainstormed + dual-review folded. Tags `[probe]` = pinned by the Task-0 stream-json probe.

---

## 1. Why this shape (and an honest conductor framing)

ADR-0005 deferred the conductor fork/continue decision to "post-3c, second protocol family." 3c delivers a **second `AgentBackend` impl** (non-ACP) and a **second adapter kind** (`claude-cli`), but it is still a **stdio child process** — so it does NOT exercise the *non-process* dimension by itself. The `kind`-seam designs in a `claude-api` (B1) arm to carry that dimension. **Honest caveat (review):** `AgentEntry` currently *requires* `cmd` and `allowed_cmds` validation is exec-launch vocabulary — so a non-process `claude-api` (no `cmd`/`args`/`cwd`) **will** force making `cmd` kind-conditional/optional and reworking the `allowed_cmds` gate. That forced change is NOT "no domain change"; **that forced change IS the conductor signal** the post-3c re-eval weighs. The designed-in `claude-api` dispatch arm is commented (reasoned, not compiled) — weaker evidence than a built one. So 3c's conductor contribution is: (a) prove the ports absorb a non-ACP **process** backend cleanly (claude-cli), and (b) surface the exec-centric `cmd`/`allowed_cmds` residue a non-process backend must change — *named explicitly* for the re-eval.

**Chosen path (B2):** warm Claude Code — reuses the subscription login (no API key, unlike A and B1), genuine interactive multi-turn, fits the warm-backend model. Rejected/deferred: **A** (ACP-Claude adapter — same family AND needs an API key, rejecting subscription OAuth) and **B1** (Anthropic API direct — strongest non-process test but needs an API key) — both designed-in via the kind-seam for later.

## 2. The adapter-`kind` seam

`bridge_core::domain::AgentEntry` gains `kind: AgentKind`:
```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AgentKind { #[default] Acp, ClaudeCli /* , ClaudeApi (future B1) */ }
```
TOML `[[agents]]` gains `kind = "acp" | "claude-cli"`, **defaulting to `acp`** (back-compat). **Parse as `Option<String>` on `AgentEntryToml` and map in `into_snapshot`** (like `effort`, `config.rs`) so an unknown value yields `ConfigError::Registry` (not a raw TOML deserialize error) `[review]`.

**Backend factory dispatches on kind** (3b's `SpawnFn` in `bin/a2a-bridge/src/main.rs` becomes a dispatcher):
```rust
match entry.kind {
    AgentKind::Acp       => AcpBackend::spawn(&entry.cmd, &args, acp_config(&entry)?).await?.with_policy(policy),
    AgentKind::ClaudeCli => ClaudeCliBackend::spawn(&entry, policy).await?,   // warm pool inside
    // AgentKind::ClaudeApi => …                                            // B1: NO Supervised child (forces cmd-optional — §1)
}
```
The `bridge-registry` crate stays kind-agnostic (it calls the `SpawnFn`, holds `Arc<dyn AgentBackend>`; lease/binding/retire machinery is transport-agnostic). **`kind` MUST join the slot-reuse identity** `[review]`: 3b's `apply()` keeps a slot warm when `cmd/args/cwd/auth_method` are unchanged — add `kind` to that tuple, so an `acp`↔`claude-cli` edit (both `cmd="claude"`) forces a fresh cold slot, not a same-backend config-edit. `allowed_cmds` still gates `cmd` (`claude-cli` uses `cmd="claude"` → add `"claude"`).

```toml
[registry]
allowed_cmds = ["kiro-cli", "codex-acp", "claude"]
[[agents]]
id = "claude"
kind = "claude-cli"
cmd  = "claude"
args = []                       # extra flags (perm/trust); §4
model = "claude-opus-4-8"       # → --model at warm spawn (best-effort)
cwd  = "/abs/trusted/dir"       # absolute; trust gate §4
model_provider = "anthropic"    # descriptive
[agents.extensions]
  idle_ttl_secs = 3300          # warm-pool idle reap (default 3300 = 55 min; §3.3)
  max_warm = 16                 # warm-pool LRU cap (default 16; §3.3)
```

## 3. `bridge-claude` — `ClaudeCliBackend` (warm process per `SessionId`, mirroring `AcpBackend`)

New crate `crates/bridge-claude`. `ClaudeCliBackend` implements `AgentBackend`. The design **mirrors `AcpBackend` semantics so the same per-turn `forget_session`/binding eviction is harmless**, and adds a bounded warm-pool because it is process-per-session.

### 3.1 State + the critical `forget_session` semantics `[review BLOCKER fix]`
```
sessions: Arc<Mutex<HashMap<SessionId, Arc<SessionSlot>>>>   // get-or-insert a handle; proc minted in a OnceCell
SessionSlot { proc: OnceCell<Arc<SessionProc>>, last_used: AtomicInstant }
SessionProc { stdin: <writer>, turn_lock: Mutex<()>, reader→per-turn mpsc registry, child: Supervised, claude_session_id: Option<String> }
session_cfg: Arc<Mutex<HashMap<SessionId, EffectiveConfig>>>   // configure_session stash (applied at spawn)
```
- **`forget_session(session)` DROPS ONLY THE CONFIG STASH — it does NOT kill the process** (exactly like `AcpBackend`, acp_backend.rs:1370). The 3b `BindingGuard` calls `forget_session` on every per-turn producer exit; because it only drops the stash, the warm process **survives across same-TaskId turns**, and the **in-flight-turn race is gone** (turn 1's guard-drop no longer tears down the process turn 2 uses). The process is reaped ONLY by the pool (§3.3) — never by `forget_session`.

### 3.2 Lifecycle (a turn)
- **`configure_session(session, eff)`** → stash `eff` (applied at the session's lazy spawn; model fixed per process — §4).
- **`prompt(session, parts)`** → get-or-insert the `SessionSlot`; `proc.get_or_try_init(spawn)` lazily spawns (exactly-once; a spawn failure leaves it unminted for retry — 3a) `claude --input-format stream-json --output-format stream-json --verbose [--model …] [perm/trust flags] [args…]` via `Supervised` (process-group + reap, 3a), bounded by a **spawn/init timeout** (§4); start the reader task; capture the init `session_id`. Acquire the **per-session turn lock**; write the user envelope (§3.4); the reader demuxes events to this turn's mpsc → `Update::Text` (deltas) then `Update::Done{stop_reason}` (terminal `result`), bounded by a **per-turn timeout** (§4). Update `last_used`. **The process stays warm.** A **follow-up with the same TaskId** reuses the warm `SessionProc` (no respawn) — multi-turn continuity, same as `AcpBackend` reuses its ACP session.
- **`cancel(session)`** → §4 (interrupt or kill+invalidate). **`retire`** → reap ALL warm procs (idempotent, take-once per proc). **`forget_session`** → §3.1 (stash only).

### 3.3 The bounded warm-pool (the process-per-session difference) `[review]`
Because each session is a heavyweight process (unlike `AcpBackend`'s one-process-many-sessions), bound the pool:
- **Idle-TTL reap:** a background reaper (interval ~60 s) terminates a session's proc when `now - last_used > idle_ttl` AND its turn lock is free (never mid-turn). **Default `idle_ttl = 55 min`** — just under the ~1 h Max prompt-cache window: held that long the session stays cache-warm; past it, a warm process ≈ `--resume` in cost (the cached prefix has expired), so reaping there is the natural break. Configurable via `extensions.idle_ttl_secs`.
- **LRU cap:** at most `max_warm` concurrent warm procs (default **16**, configurable `extensions.max_warm`). On exceeding, evict the **least-recently-used idle** session (turn lock free); a later follow-up to an evicted session re-spawns cold (continuity is best-effort over the cap). If all are mid-turn, do not evict (the cap is soft under load).
- **Retire/shutdown:** `retire()` (registry remove/edit/3b retirement) + process shutdown reap all procs via `Supervised::terminate` (lease-drained by 3b's retirement task).
- All reaping is **turn-lock-aware** — a proc is never terminated while a turn holds its lock.

### 3.4 stream-json wire shapes `[probe — Task 0]`
- **Write (stdin NDJSON, per turn):** `{"type":"user","message":{"role":"user","content":[{"type":"text","text":<part text>}]}}`.
- **Read (stdout NDJSON):** init (`{"type":"system","subtype":"init","session_id":…}`); assistant text (`{"type":"assistant","message":{…}}` or, with `--verbose`/`--include-partial-messages`, incremental deltas) → `Update::Text`; terminal `{"type":"result","subtype":…,"stop_reason":…}` → `Update::Done` **on `subtype:"success"`**, or **`Err(AgentCrashed)` → `Failed` on an error subtype** (e.g. `error_max_turns`/`error_during_execution`) `[review]`. **Tolerant reader** (3a): unmodeled events / non-text content dropped. The reader, between serialized turns, ignores late/init events so they aren't misrouted into the next turn. Exact field names + whether `--verbose` is needed for deltas are pinned by Task 0.

### 3.5 Reuses proven `AcpBackend` patterns
Reader-loop → per-turn mpsc demux; per-session turn lock (one turn at a time; a second prompt for an active session waits); lazy exactly-once `OnceCell` spawn + spawn-failure retry; `Supervised` hygiene; tolerant NDJSON reader; the `BackendStream` shape (`Update::Text…` then `Update::Done`, `Err` on transport/process failure → 3b producer maps to `Failed`).

## 4. Timeouts, cancel, auth/trust, model, errors

- **Bounded timeouts (mirror `AcpBackend`) `[review]`:** a **spawn/init timeout** (the first turn's init must arrive within a bound, else `AgentNotAuthenticated`/`AgentCrashed` — a `claude` blocked on a no-TTY workspace-trust or tool-permission prompt must NOT hang `ensure` forever); a **per-turn timeout** (a turn's terminal `result` must arrive within a bound, else terminate+`Err`, so a hung turn can't hold the turn lock forever); a **cancel grace** (interrupt → wait → escalate to terminate). Constants with `AcpConfig`-style defaults; the trust/permission flags (e.g. `--permission-mode`, a trust flag) needed for a non-interactive session are pinned by Task 0.
- **Cancel `[review]`:** prefer a stream-json **interrupt control message** (if Task 0 confirms support + a terminal cancel-`result` event) → map to `Update::Done{cancelled}` → A2A `Canceled`. Fallback: `Supervised::terminate` the session's proc → reader EOF. **A hard kill must map to `Canceled`, not `Failed`** (do not let EOF-after-cancel become `AgentCrashed`) — track a per-session "cancel requested" latch (like 3a) so the EOF is interpreted as cancellation; and **invalidate the session slot** so the next prompt respawns rather than using a dead handle.
- **Auth/trust `[review]`:** spawn plain `claude` (no `ANTHROPIC_API_KEY`) → reuse the subscription login. **Not-logged-in + workspace-trust must be DETECTABLE** (Task 0 captures the signal — stderr text? a `result` error subtype? non-zero exit?) and mapped to `BridgeError::AgentNotAuthenticated` (→ A2A `AuthRequired`), NOT a generic crash. Document the billing note (subscription Agent-SDK credit pool from 2026-06-15).
- **Model/mode:** `model` → `--model` at spawn (fixed per warm process — Claude has no mid-session switch). `configure_session`'s model applies at spawn; **a per-request model override on a follow-up to a live session is a no-op** (same mint-time-only semantic as `AcpBackend`) — documented. `mode`/permission → CLI flags (best-effort).
- **Errors:** process crash / stdout EOF mid-turn (not after a cancel) → `Err(AgentCrashed)` → `Failed`; spawn failure → `AgentCrashed`/`AgentNotAuthenticated`; unparseable NDJSON → tolerant drop; result error-subtype → `Failed` (§3.4).

## 5. Testing

- **Task 0 — stream-json probe (GATE, run first) `[probe]`:** a bounded local run that pins: (a) **does the process accept a SECOND turn** (warm Pattern 2's make-or-break); (b) the init/assistant-delta/`result` NDJSON shapes + whether `--verbose`/`--include-partial-messages` is needed for token deltas; (c) the **interrupt** control-message shape + its terminal cancel-`result` event; (d) the **not-logged-in + workspace-trust** failure signals (for the auth mapping); (e) the non-interactive trust/permission flag. Report back (controller gate). **If turn-2 fails**, build **Pattern 1 instead** (below) — same `claude-cli` kind, different internals.
- **Pattern-1 fallback (co-equal, documented — not a footnote) `[review]`:** if warm-multi-turn is impossible, `ClaudeCliBackend` is a `--resume` one-shot per turn: spawn `claude -p --output-format stream-json [--resume <claude_session_id>] -- <prompt>` per turn, capture/store `session_id`, continuity server-side, slot-invalidation on resume failure (agent-knowledge's proven pattern). This **avoids the warm-pool entirely** (no process survives between turns → no idle-TTL/LRU/forget-session concern) and is a *different lifecycle* (process-per-turn, no turn-spanning process). Cancel = kill the per-turn process. The kind-seam absorbs the swap with no registry/factory change. The plan picks Pattern 1 or 2 from the Task-0 result.
- **Unit (in-process fake `claude`):** a `/bin/sh`/test-bin fake reading stdin NDJSON + emitting canned stdout NDJSON. Cover (warm): prompt → `Update::Text`×N + `Done`; `forget_session` drops the stash but **does NOT kill the proc** (assert the proc still serves a follow-up); per-session isolation; tolerant reader; result error-subtype → `Err`; idle-TTL reap (with a short TTL override) terminates an idle proc but **not one mid-turn**; LRU eviction over a small cap; cancel→`Canceled` + slot invalidation; spawn-failure retry; spawn/turn timeout bounds.
- **Inbound-level sequential multi-turn e2e (CRITICAL — direct-`prompt` tests hide the blocker) `[review]`:** drive **two sequential `message/send` to the SAME TaskId THROUGH `InboundServer`** (wait for turn 1's terminal first), against a fake-claude backend, and assert **turn 2 reaches the SAME warm process with prior context** (i.e. `forget_session` between turns did NOT destroy continuity). This is the test that actually exercises the blocker fix.
- **Wire-golden:** the outbound user envelope `{"type":"user","message":{"role":"user","content":[{"type":"text","text":…}]}}` — hand-authored expected.
- **Config/registry:** `kind="claude-cli"` parses (+ default `acp`); `kind` in the reuse-identity tuple (an `acp`↔`claude-cli` edit forces a fresh slot); invalid kind → `ConfigError::Registry`; the kind-dispatching factory builds the right backend.
- **Gated real e2e (run it — `claude` installed + logged in):** register `claude` (`kind=claude-cli`) alongside `kiro`/`codex` (3 agents, 2 kinds); route to `claude` by id; **two sequential turns (turn 1 "Remember the number 7", turn 2 "What number?") → turn 2 answers 7** (the genuine warm-continuity proof), reusing the login. `#[ignore]`-gated; RUN once to confirm warm-multi-turn works against real `claude` (and thus which pattern Task 0 mandated).
- **Coverage:** workspace ≥85%; `bridge-core` ≥90%; new `bridge-claude` ≥90% — after `cargo llvm-cov clean --workspace`.

## 6. Scope boundary
**3c BUILDS:** the `AgentKind` discriminator (+ config parse + `kind` in reuse-identity) + the kind-dispatching factory; `bridge-claude`/`ClaudeCliBackend` (warm Pattern 2 with the bounded warm-pool — `forget_session`-drops-stash, idle-TTL/LRU/retire, bounded timeouts, cancel→Canceled, auth/trust detection; OR Pattern 1 `--resume` if Task 0 mandates); the inbound-level multi-turn e2e + the gated real e2e. **3c DOCUMENTS / designs-in (not built):** the `claude-api` (B1) HTTP arm + the exec-residue (`cmd`/`allowed_cmds`) it will force changing — named for the **post-3c conductor re-evaluation**; the ACP-Claude adapter (A) as a `kind="acp"` config entry. **Non-goals:** no Anthropic API/HTTP code, no API key, no fan-out/registry changes beyond the `kind` field + reuse-identity.

## 7. Review
Spec **Revision 2** has folded the dual review (Codex gpt-5.5 + Claude opus-4.8). Re-review this revision (the warm-pool lifecycle, the `forget_session`-drops-stash blocker fix + the inbound-level multi-turn test, the timeouts/cancel/auth, and the honest conductor framing) before the plan; fold findings; the plan then gets its own dual review.
