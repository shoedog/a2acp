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
