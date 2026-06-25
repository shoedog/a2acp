# E6 — Node Retry (transient-failure retry/resume) — SPEC

**One-liner:** a workflow node that fails with a TRANSIENT agent error (crash, overload, watchdog timeout, the
`_dyld_start` startup flake) is automatically retried within the run — bounded attempts + backoff — before
degrading to `ok=false`, hardening the self-hosted review/implement loop against the transient failures the project
repeatedly hits.

**Roadmap:** Slice-10+ tail item E6 (the user picked it from {E6 retry · E3 batch · E7 task-spec · E8 prompt-lib}).
Base = `main` `d274177`. Branch `feat/e6-node-retry`.

---

## Goal & value
The dogfooded `code-review`/`design`/`implement` workflows run real ACP agents (codex/claude) that intermittently:
- **crash** (`AgentCrashed` — process death, the `_dyld_start`/rustc-startup sandbox stall, a chatty-stderr deadlock),
- **overload** (`AgentOverloaded` — 429),
- **time out** (`AgentTimedOut` — the E9 watchdog).

Today a single such hiccup makes the node finish `ok=false` and the whole workflow degrades (downstream nodes get the
error marker, the synth is polluted). E6 lets a node RETRY a transient failure a bounded number of times before
giving up. Net: the self-hosted loop survives a flaky turn instead of wasting the whole run.

## Scope (MVP cut-line)
**IN:**
1. A `BridgeError::is_transient()` classifier (bridge-core) — the single source of truth for "retryable."
2. A per-node retry policy (`max_attempts` + `backoff_ms`), config-driven, defaulting to NO retry (1 attempt =
   today's behavior — zero behavior change when unset).
3. An in-run retry loop in the executor's `run_node`: on a transient prompt/drain error, `forget_session` →
   backoff → re-`configure_session` → re-`prompt` → re-collect, bounded by `max_attempts`. Cancellation aborts
   immediately; a NON-transient error fails immediately (no retry); exhausting attempts → `ok=false` (current
   degradation) with a marker that records attempts + the last error.
4. Observability: each retry emits a signal so retries are VISIBLE (not silent) — at minimum `tracing`, ideally a
   rich `OrchEventKind::NodeRetry { attempt, reason }` journaled (S6 substrate) + surfaced in `task watch`.
5. Resume-compatibility (FREE): a node mid-retry is unfinished → not checkpointed → re-runs fresh on a W3b
   crash-resume. No new code; assert it in a test.

**OUT (explicit deferrals):**
- **Resume re-running EXHAUSTED `ok=false` checkpoints** — a node that already exhausted its in-run retries stays
  seeded/done on crash-resume (today's W3b behavior). Re-running it needs a `node_checkpoints` schema change (persist
  "was-transient") + poison-loop handling beyond the existing `resume_attempts` cap → DEFER.
- **Warm-session turn retry** — this slice is the COLD workflow-node path (`executor.rs::run_node`). Retrying a warm
  interactive turn (the SessionManager path) is a separate concern → DEFER.
- **Per-error backoff curves / jitter / circuit-breaking** — start with one simple backoff; richer policy later.
- **Turning retry ON for the shipped workflows** — this slice ships the MECHANISM (default off). Enabling it in
  `examples/*.toml` is a config-only follow-on (may ride this branch as a separate commit if cheap).

## The seam (where each piece lives)
- **`crates/bridge-core/src/error.rs`** — `BridgeError::is_transient(&self) -> bool` beside `is_resumable()` (`:127`).
  Transient = `AgentCrashed | AgentOverloaded | AgentTimedOut`. NON-transient (do NOT retry) = `PermissionDenied |
  PermissionRequired | AuthRequired | AgentNotAuthenticated | ModelNotAvailable | ConfigMismatch |
  ConfigReseedRequired | InvalidRequest | A2aVersionMismatch | MessageTooLarge | CancelTimeout | SessionExpired |
  SessionNotFound | TaskNotFound | HandleBusy | FrameError | StoreFailure | UpstreamA2aError | InvalidStateTransition
  | UnknownAgent | ConfigInvalid`. (Rationale per variant documented in the code; `CancelTimeout` is user-intent →
  never retry; auth/permission → needs human action, not a retry.)
- **`crates/bridge-workflow/src/graph.rs`** — `WorkflowNode` gains `retry: Option<RetryPolicy>` where
  `RetryPolicy { max_attempts: u32, backoff_ms: u64 }`. `#[serde(default, skip_serializing_if=...)]` so it rides the
  durable spec snapshot (`encode_workflow_spec`) → resume restores it (additive-safe, mirrors Slice-10 `panel`).
- **`crates/bridge-workflow/src/executor.rs`** — `run_node` wraps the configure→prompt→collect core in a retry loop
  reading `node.retry`. Emits the retry signal. The 3-tuple return `(String, bool, Option<UsageSnapshot>)` is
  unchanged.
- **`bin/a2a-bridge/src/config.rs`** — `WorkflowNodeToml` gains `retry: Option<RetryToml>` (`{ max_attempts, backoff_ms }`)
  → mapped into `WorkflowNode.retry` at `into_snapshot`/graph build.
- **(observability, optional)** `crates/bridge-core/src/orch.rs` — `OrchEventKind::NodeRetry { node, attempt, reason }`
  (rides the S6 journal + the S7a rich-event path) if the rich-signal route is chosen over plain `tracing`.

## Retry mechanics (the core)
`run_node` today: `configure_session` (T6: fail-node on error) → cancel-check → `prompt`/`prompt_observed` (fail at
`:318`) → drain loop (fail at `:355` on `Some(Err)`) → `forget_session` → return `(text, ok, usage)`.

E6: wrap the configure→prompt→drain core in `for attempt in 1..=max_attempts { ... }`:
- A **transient** failure (configure/prompt/drain returns a `BridgeError` where `is_transient()`) AND `attempt <
  max_attempts` AND NOT cancelled → `forget_session`, emit `NodeRetry{attempt, reason}`, `sleep(backoff)`, `continue`
  (re-configure + re-prompt).
- A **non-transient** failure → break/return `ok=false` immediately (no retry).
- **Cancellation** (the `tokio::select! cancel` arms / `STOP_REASON_CANCELLED` / `canceled_during_drain`) → abort the
  retry loop immediately, return the canceled marker (NEVER retry a cancel).
- **Success** (`ok=true`) → return.
- **Exhausted** (`attempt == max_attempts`, still transient-failing) → return `ok=false` with a marker like
  `[node N failed after K attempts: <last error>]`.
- Backoff: `sleep(backoff_ms)` between attempts (the cancel token must be select-able during the sleep so a cancel
  mid-backoff aborts promptly). Decision D1: fixed vs exponential.
- `max_attempts` semantics: total attempts (1 = no retry). `max_attempts=0` treated as 1 (defensive).

## Resume-compatibility (assert, no new code)
A node that crashes mid-retry is UNFINISHED → no `NodeFinished` → no checkpoint → on serve restart W3b's
`resume_working_tasks` re-runs it fresh (it's a pending node). So in-run retries survive a crash for free. A test
asserts: a node with retry that is interrupted (cancel/crash) before finishing is NOT seeded on resume.

## Open questions (for the dual spec-review)
- **Q1 — backoff shape:** fixed `backoff_ms`, or exponential (`backoff_ms * 2^(attempt-1)`), or +jitter? (D1 below.)
- **Q2 — config granularity:** per-node only, or also a workflow-level/global default that a node overrides? Per-node
  is the minimal cut; a `[workflows].retry` default is a small add.
- **Q3 — observability route:** plain `tracing::warn!` per retry (cheap, no schema), or a rich
  `OrchEventKind::NodeRetry` journaled + watch-visible (richer, touches orch.rs + the S7a unfold + the journal)?
  Recommendation: start with `tracing` + a structured field; promote to a rich event only if review wants it
  watch-visible.
- **Q4 — the transient set:** is `AgentTimedOut` retryable (the watchdog tripped — retrying may just trip again, but a
  transient hang is exactly what retry helps)? Is `FrameError` transient (protocol desync — maybe)? Lock the set.
- **Q5 — does retry re-render the prompt / re-seed?** The node prompt is deterministic (rendered once from inputs);
  re-prompting uses the SAME parts. Confirm no double-side-effect (the agent re-runs the turn from scratch; for
  read-only review agents that's fine; for write agents (implement) a retry re-runs the edit — acceptable? note it).
- **Q6 — interaction with the per-node watchdog (E9):** the watchdog kills a hung turn → `AgentTimedOut` → retry.
  Confirm the watchdog's `biased` select + the retry loop compose (no double-fire, no relabel).
- **Q7 — interaction with usage (Slice-10):** on a retried node, is the reported usage the LAST attempt's, or summed?
  (Each attempt consumes tokens.) Decision needed: sum across attempts, or last-only. Leaning: sum (true cost).

## Decisions (proposed — confirm/revise in review)
- **D1:** backoff = simple exponential with a cap: `min(backoff_ms * 2^(attempt-1), backoff_cap_ms)`; default
  `backoff_ms=500`, implicit cap (e.g. 30s). Cancel-abortable via `tokio::select!` over the sleep. (Q1.)
- **D2:** per-node policy only for MVP; a global default is a cheap follow-on (Q2). Default = `None` ⇒ 1 attempt.
- **D3:** observability via `tracing::warn!(node, attempt, reason)` for MVP; rich `NodeRetry` event DEFERRED unless
  review insists on watch-visibility (Q3).
- **D4:** transient = `AgentCrashed | AgentOverloaded | AgentTimedOut` ONLY for MVP (Q4); everything else fails fast.
- **D5:** usage = SUM across attempts (true consumed cost) — accumulate `last_usage` deltas across attempts (Q7).
- **D6:** write-agent re-run side-effects (Q5) are ACCEPTABLE in v1 (the implement path host-commits the agent index;
  a re-run just re-stages) — documented, not guarded.

## Live-gate shape
With a config whose node has `retry = { max_attempts = 3, backoff_ms = 200 }` and a fake/forced-flaky agent (or a
real agent pointed at a transiently-unavailable endpoint):
1. A node whose first attempt(s) fail with a TRANSIENT error and a later attempt succeeds → node finishes `ok=true`
   (the workflow completes), and the retry signal fired `attempts-1` times.
2. A node that fails transiently on EVERY attempt → finishes `ok=false` after exactly `max_attempts` attempts (marker
   records the count); the workflow degrades (today's behavior), not hangs.
3. A node that fails with a NON-transient error (e.g. config/permission) → fails on attempt 1, NO retry.
4. A cancel mid-retry/mid-backoff → aborts promptly, node canceled, no further attempts.
5. (resume) Kill serve while a retrying node is mid-attempt → on restart the node re-runs fresh (not seeded).
Most can be unit/integration-tested with a scripted flaky `AgentBackend`; the real-agent gate proves the
classifier + the live wiring end-to-end.

## Test strategy
- Unit: `is_transient()` covers every `BridgeError` variant (table test, like the disposition tests).
- Unit/integration: a `FlakyBackend` (configurable: fail-N-then-succeed / always-fail-transient /
  fail-non-transient) drives `run_node` via a one-node graph → assert attempts, ok, marker, the retry signal count,
  and cancel-aborts-retry. Mirror the existing `cold_configure_error_fails_node` test harness.
- Resume: assert an unfinished retrying node is not seeded (extends the W3b resume tests).
- Config: `WorkflowNodeToml.retry` parses; maps to `WorkflowNode.retry`; rides `encode_workflow_spec` round-trip.

## Proven loop + roles (unchanged)
codex gpt-5.5 HIGH implements (no commit / no git-mutating cmd); codex xhigh reviews (read-only); Opus
architects/verifies-in-clean-host-env/commits/live-gates. Stage ONLY each task's files (the worktree has many
pre-existing untracked `examples/*.toml`/`prompts/*.md` + `M examples/a2a-bridge.slicing-analysis.toml` — NEVER fold).
The controller re-runs runtime tests in the clean host env (codex's sandbox `_dyld_start` stall). `cargo test
--workspace --all-targets` is the gate.

---

## v2 — dual spec-review folded (codex xhigh: 2 BLOCKER + 3 MAJOR + 1 MINOR; Opus lens) — BINDING

> Supersedes v1 where it conflicts. The ARCHITECTURE HOLDS (opt-in per-node retry in `run_node`, default off) — but
> the RESET CONTRACT between attempts is the crux and v1 got it wrong (`forget_session` is not a reset). Apply each
> SR-FIX. VERDICT after folding: ready-to-plan (the plan details the reset seam + decides the registry-invalidation size).

### SR-FIX-1 (BLOCKER-1) — the retry loop wraps `resolve()`, not just configure→prompt
The `_dyld_start`/startup flake is an `AgentCrashed` from `registry.resolve()`'s LAZY spawn (`registry.rs:305-312`
`get_or_try_init`), BEFORE the prompt/drain sites (`acp_backend.rs:948/1237`). On a failed spawn the `OnceCell` stays
UNINITIALIZED (`registry.rs:306`) → a later `resolve()` RESPAWNS. **So the retry loop must span
`resolve() → configure_session → prompt → drain`** — then a startup-flake re-resolve respawns the agent FOR FREE. (v1's
hook starting at configure/prompt missed the headline flake.) `run_node`'s `resolve` is at `~:269`.

### SR-FIX-2 (BLOCKER-2) — the RESET CONTRACT: `release_session` + a UNIQUE per-attempt `SessionId` + re-`resolve`
`forget_session` is NOT a clean reset for plain ACP: it clears config/turn metadata only (`acp_backend.rs:2616-2621`),
NOT the agent session map or the process; `ensure_session` REUSES the existing session (`:1604/1635`); only
`release_session` drops it (`:2705`); and the registry reuses one backend `Arc` per agent (`registry.rs:308`). So a
retry that `forget`s + re-prompts the SAME `SessionId` reuses the stale (possibly dead) session. **Reset between
attempts = (a) `release_session(prior_attempt_sid)` (drops the agent session + reaps a `:rw` container), (b) a UNIQUE
per-attempt `SessionId` (e.g. suffix `-a{N}`) so `ensure_session` creates a FRESH session, (c) re-`resolve()`.**
- Process-ALIVE transients (`AgentOverloaded`, `AgentTimedOut`): the fresh session re-prompts the same alive process →
  clean retry. ✓ Works for ALL cold backends.
- STARTUP-failure `AgentCrashed` (resolve-time, `OnceCell` uninitialized): re-`resolve()` respawns. ✓
- MID-TURN `AgentCrashed` (spawned then died, `OnceCell` INITIALIZED): re-`resolve()` returns the cached DEAD `Arc` →
  retry is futile WITHOUT a registry **invalidate-and-respawn** seam. **DECISION FOR THE PLAN:** either (i) add a
  minimal registry "invalidate agent backend" seam (drop the `OnceCell` so re-resolve respawns) — preferred, delivers
  full crash-recovery — or (ii) scope the MVP transient set to `AgentOverloaded | AgentTimedOut | resolve-time
  AgentCrashed` and document mid-turn-crash retry as best-effort/deferred. The plan reads the registry seam to size (i).

### SR-FIX-3 (MAJOR-3) — resolve the `configure_session` inconsistency (preserve the T6 fix)
v1 was inconsistent (binding context said prompt/drain; mechanics said configure). RESOLUTION: the retry loop gates
EVERY site (`resolve`/`configure`/`prompt`/`drain`) by `is_transient()`. The E1/T6 fail-node-on-configure-error
behavior (`executor.rs:275-291`, regression test `:1519`) is PRESERVED because that error is `ConfigInvalid`
(NON-transient → no retry → fail-fast). A genuinely TRANSIENT configure error (e.g. a worktree-add git lock that
escaped the T3 bounded retry) would now retry — acceptable + strictly better. Keep/extend the T6 test (ConfigInvalid
still fails on attempt 1, no retry).

### SR-FIX-4 (MAJOR-4 + Opus-1) — usage = LAST observed attempt, do NOT sum the carrier
`UsageSnapshot { used, size, cost, at_ms }` (`orch.rs:37`) has NO additive semantics: `size` is the context-window
size (summing corrupts `windowFraction = used/size`, the Slice-10 panel signal, `executor.rs:69`), `cost` is
mixed-currency, `at_ms` is a timestamp. **D5 REVISED: report the LAST observed attempt's `UsageSnapshot`** (matches
existing downstream meaning). The failed attempts' tokens are uncounted in the report (a minor, documented cost on the
rare retry). A true cross-attempt cost carrier is DEFERRED (needs explicit per-field aggregation).

### SR-FIX-5 (MAJOR-5 + D6) — write side-effects: retry is RECOMMENDED for read-only nodes; write-node reset deferred
`container_rw` per-turn retries reuse the same writable cwd + open a fresh container against it with NO reset
(`container/lib.rs:463-524`), unlike the warm-respawn path which resets first (`resilient.rs:97`). So retrying a
WRITE-capable node may re-apply edits. **D6 REVISED:** retry is opt-in per node; v1 RECOMMENDS enabling it only on
READ-ONLY nodes (review/design — the bulk of the dogfooded loop + the stated value). A pre-retry reset hook for
write-capable nodes (clean the worktree/clone before re-attempt) is DEFERRED + documented. The spec/plan do NOT
auto-guard; the operator scopes retry to read-only nodes.

### SR-FIX-6 (MINOR-6 + D3) — observability = `tracing` for MVP
`OrchEventKind` has no retry variant and `frame_from_orch` `unreachable!`s on unmapped kinds
(`detached.rs:98`). **D3 CONFIRMED:** MVP emits `tracing::warn!(node, attempt, reason)` per retry. A rich
`OrchEventKind::NodeRetry` (journal + watch-visible) is DEFERRED (full DTO/journal/frame/watch wiring).

### Q/D answers — LOCKED (both lenses)
- **Q1/D1 backoff:** simple exponential with a CAP — `min(backoff_ms * 2^(attempt-1), backoff_cap_ms)`. `RetryPolicy`
  gains `backoff_cap_ms: Option<u64>` (default ~30_000). The backoff sleep MUST be `tokio::select!`-able against the
  cancel token (abort mid-backoff). Default `backoff_ms=500`.
- **Q2/D2:** per-node policy ONLY for MVP; global default deferred. `retry: None` ⇒ 1 attempt (zero behavior change).
- **Q3/D3:** `tracing` only (SR-FIX-6).
- **Q4/D4 transient set:** `AgentCrashed | AgentOverloaded | AgentTimedOut`. Everything else fails fast — auth/
  permission/config/model/request/state/not-found/message-size/store/upstream/`FrameError`/session/cancel all need
  human/config action, indicate protocol/state/persistence bugs, or are user intent. **NOTE the `AgentCrashed` split**
  (resolve-time respawns free; mid-turn needs the SR-FIX-2 invalidation decision). `is_transient()` is COLD-only —
  do NOT reuse the warm-respawn classifier (`resilient.rs`, which treats `AgentTimedOut` fatal + `FrameError`/
  `SessionNotFound` transient — deliberately different).
- **Q5/D6:** read-only-recommended (SR-FIX-5).
- **Q6 watchdog:** COMPOSES — ACP makes fresh per-turn watchdog state in `prompt_inner` (`acp_backend.rs:2248`) with
  biased arbitration (`:2357`); each retry = a new prompt = a fresh watchdog. Retrying `AgentTimedOut` is sound,
  bounded by attempts. (Opus note: with `AgentTimedOut` retryable, worst-case wall = `max_attempts × watchdog_window` —
  document the time-budget implication; operators sizing `max_attempts` for timeout-prone nodes should account for it.)
- **Q7/D5:** last-attempt usage (SR-FIX-4).
- **Resume claim CONFIRMED:** mid-retry unfinished nodes are NOT checkpointed until `NodeFinished`
  (`executor.rs:615` → `detached.rs:310`) → re-run free on resume; exhausted `ok=false` checkpoints are seeded+skipped
  (`detached.rs:1492`, `executor.rs:534`), terminal-failure short-circuited (`detached.rs:1546`). The deferral (don't
  re-run exhausted failures) is REAL + acceptable-if-documented. **Opus-3 gap to name:** a LONG transient outage
  (longer than `max_attempts × backoff`) exhausts in-run retries → node fails permanently even though a restart later
  would succeed; resume does NOT recover it (documented MVP limitation; operator re-submits).

### Revised scope summary (net of the folds)
`is_transient()` (bridge-core) → `RetryPolicy { max_attempts, backoff_ms, backoff_cap_ms }` on `WorkflowNode` (rides
`encode_workflow_spec`) ← `WorkflowNodeToml.retry` → the retry loop in `run_node` spanning resolve→configure→prompt→
drain with the release+fresh-SessionId+re-resolve RESET, `is_transient`-gated, cancel-abortable (incl. backoff),
last-attempt usage, `tracing` observability. PLAN decides the registry invalidate-and-respawn seam (full mid-turn-crash
recovery) vs the scoped-transient-set fallback. Read-only-recommended; resume-compat asserted (no new code).

---

## v3 — spec v2 RE-REVIEW folded (codex xhigh: 1 BLOCKER + 1 MAJOR + 1 MINOR + 1 NIT; Opus re-lens) — BINDING

> Supersedes v2 where it conflicts. The re-review CONFIRMED SR-FIX-1/3/4/5/6 RESOLVED and SR-FIX-2
> PARTIALLY-RESOLVED — the reset's BRIDGE-side piece is right, but **backend invalidation for a dead cached process**
> was left too loose, and that gap hits `AgentTimedOut` too (the watchdog can KILL the process past grace,
> `acp_backend.rs:2376/2435`, yet still report `AgentTimedOut`). v3 promotes the invalidation seam INTO SCOPE so the
> headline crash+timeout recovery actually works. VERDICT after folding: ready-to-plan (the plan designs the seam).

### RR-FIX-1 (BLOCKER) — adopt the registry INVALIDATE-AND-RESPAWN seam; respawn every retry
The registry caches ONE backend `Arc` per slot in a `OnceCell` (`registry.rs:32-34`); after a mid-turn process death
(`AgentCrashed`) OR a watchdog process-kill (`AgentTimedOut` past grace) the cached `Arc` is DEAD and `resolve()`
returns it again → retry is futile. The `AgentRegistry` trait has no invalidate (`ports.rs:197`). **DECISION: add a
minimal `AgentRegistry::invalidate(&self, agent: &AgentId)` seam** (default no-op for mocked registries) that
ATOMICALLY replaces that agent's `Slot` with a fresh one (new `OnceCell`) via an `ArcSwap` state store — mirroring
`apply`'s atomic slot swap (`registry.rs:366-412`). Concurrent resolvers keep their already-held `Arc` clones
(unaffected); the NEXT `resolve()` respawns. The retry RESET between attempts is then:
`release_session(node_sid)` → `registry.invalidate(node.agent)` → re-`resolve()` (respawn) → re-`configure_session`
→ re-`prompt`. **Respawn EVERY attempt** is the simplest correct reset (a fresh process per retry — recovers
`AgentCrashed` startup+mid-turn AND `AgentTimedOut` killed-or-not AND `AgentOverloaded`). Optimizing to
respawn-only-on-death-signals is a documented follow-on (spawn+handshake latency per retry is acceptable for the rare
retry). The plan designs the `invalidate` impl + the `apply`-vs-`invalidate` ArcSwap concurrency.

### RR-FIX-2 (MAJOR) — `release_session` is BRIDGE-side; respawn handles the agent side (no `session/close` needed)
`release_session` cancels + drops the bridge `sessions` map + config stash (`acp_backend.rs:2705-2715`) but does NOT
send ACP `session/close` (capability-advertised, unimplemented). With RR-FIX-1's respawn-every-attempt, the OLD
process (and all its agent-side sessions) DIES when invalidated → no stale agent-side session, no `session/close`
required. Spec wording corrected: the reset "drops bridge-side session state + replaces the process," not "re-mints a
fresh `session/new` on the same process." (Opus-R1: with respawn, the SAME node `SessionId` is reused — the FRESH
process mints a new session for it; the v2 unique-per-attempt `-a{N}` `SessionId` is DROPPED as redundant. The
re-review confirmed `-a{N}` would be valid (`ids.rs:10` rejects only empty) and downstream keys on `NodeId` not
`SessionId`, so either works — same-id is simpler.)

### RR-FIX-3 (MINOR) — backoff arithmetic must be overflow-safe
`min(backoff_ms * 2^(attempt-1), backoff_cap_ms)` can overflow `u64` before the `min`. Implementation: compute with
`checked_shl`/`saturating_mul` (saturate to `u64::MAX` on overflow) then `min(.., backoff_cap_ms)` BEFORE
`Duration::from_millis`. `backoff_cap_ms` defaults to 30_000. (A high `attempt` thus clamps to the cap, never panics.)

### RR-FIX-4 (NIT) — drop the misleading "transient configure error" example
The v2 "worktree-add git lock" example for a retryable configure error is wrong: host worktree-add failures return
`ConfigInvalid` (NON-transient, `host_git.rs:90/102`) → SR-FIX-3 fails them fast. There is NO transient configure
error today; the retry loop's configure-site gating by `is_transient` is FUTURE-PROOFING (if a transient configure
error variant is ever added). Reword — no example claimed.

### Fold status (re-review, for the record)
- SR-FIX-1 RESOLVED · SR-FIX-2 → now COMPLETED by RR-FIX-1+2 · SR-FIX-3 RESOLVED (T6 `ConfigInvalid` fail-fast,
  test `:1549`) · SR-FIX-4 RESOLVED (last-attempt usage; `size`/`cost` non-additive) · SR-FIX-5 RESOLVED (read-only
  recommended) · SR-FIX-6 RESOLVED (`tracing`; `frame_from_orch` can't map a new kind).
- Downstream stable: checkpoints/watch/rich-sink key on `NodeId` (`detached.rs`/`executor.rs`), not the cold
  `SessionId`. `backoff_cap_ms: Option<u64>` with serde default round-trips `encode_workflow_spec` cleanly.

### Revised scope summary (net of v2 + v3)
`is_transient()` (bridge-core) · `RetryPolicy { max_attempts, backoff_ms, backoff_cap_ms }` on `WorkflowNode` (rides
`encode_workflow_spec`) ← `WorkflowNodeToml.retry` · NEW `AgentRegistry::invalidate(agent)` seam (atomic Slot
replacement) · the retry loop in `run_node` spanning resolve→configure→prompt→drain, `is_transient`-gated,
cancel-abortable (incl. overflow-safe backoff sleep), with the RESET = release_session + invalidate + re-resolve
(respawn) + re-configure + re-prompt (same node `SessionId`), last-attempt usage, `tracing` observability,
read-only-recommended. Resume-compat asserted (no new code). DEFERRED: respawn-only-on-death optimization,
resume-re-run of exhausted failures, warm-turn retry, write-node pre-retry reset, rich `NodeRetry` event, global
retry default.
