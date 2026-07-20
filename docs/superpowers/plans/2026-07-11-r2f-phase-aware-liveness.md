# R2f — Phase-aware liveness and safe takeover plan

- **Status:** DEFERRED; four incidents recorded, investigation not started
- **Prerequisite:** R2b structured diagnostics merged; may proceed independently of R2c–R2e afterward
- **Program source:** [`../../bridge-reliability.md`](../../bridge-reliability.md)
- **Program cursor:** [`../../reliability-execution-roadmap.md`](../../reliability-execution-roadmap.md)
- **Incident ids:** `INC-VERIFY-STALL-2026-07-11`, `INC-SHARED-WARM-CRASH-2026-07-16`,
  `INC-SHARED-SESSION-CAPACITY-2026-07-17`, `INC-SHARED-RESTART-RECOVERY-2026-07-19`

## Incident evidence and limits

The operator reported a Luna run in `~/code/stockTrading` with **2h54m total elapsed time**. Useful file
edits completed in about the first **25 minutes**; the last file edit was at **17:22**. The run then made
no observed editing progress for nearly three hours while parked in verification. The operator killed
only that run's process tree, took over the remaining verification, and found the retained work clean.

This is an operator report, not a bridge reproduction. The current record does not prove whether the
stall was in the provider, ACP adapter, agent runtime, verification command, child-process wait, or
orchestration waiter. File modification time alone is not a safe liveness signal: a legitimate long test
can make no edits, while a wedged verifier can keep a process alive. R2f must collect the observations that
separate those alternatives before changing timeout behavior.

## Shared-operator evidence and limits

The long-lived production operator has repeatedly returned immediate Codex `AgentCrashed` before observable prompt
start while its bridge, ACP adapter, and Codex app-server processes remained alive. Fresh isolated one-shot
operators completed the same package/model/effort/mode and review shape. The old app server had observed 15
distinct session thread ids and no close notifications; bridge release removed its local session entry
without sending a capability-gated ACP close, while codex-acp retained sessions until close. On 2026-07-19 the same
boundary recurred against R3d2 exact `3e4508a`: no task/session/turn row or prompt/usage evidence was created, the
roughly two-day-old warm process tree remained alive, and the same release binary completed the review through one
fresh one-shot bridge without touching the production generation.

The incident stream later recurred against operator release `983398427c9f0486`: card/catalog and Codex
doctor/provenance checks were healthy and there were zero unfinished tasks and zero durable sessions, yet two
explicit unary raw-`gpt-5.6-sol`/xhigh/read-only submits failed before task, turn-log, prompt-start, or usage
creation. The operator reports that stopping and restarting the served bridge ultimately restored the affected
path, while one controlled exact unary reproduction after an earlier restart still failed pre-prompt. That makes
pre/post-restart process, transport, ACP-child, and session state required evidence; it does not establish a
session-count threshold, poisoned transport, or restart as the root cause or durable remedy.

This rules out a general package/model/auth/cwd incompatibility for those incidents, but does not distinguish
a capacity ceiling/session leak from a poisoned long-lived transport. Fifteen is evidence, not a threshold.
The earlier isolated comparison stopped no running turn, warm session, backend, image, or production operator and
replayed no failed request. The later stop/start was an independent operator recovery action, not an R3d gate.
R2f owns this investigation and every lifecycle remedy. R3d only records that its fresh one-shot executions did
not evaluate shared-operator health.

## R2f0 — Reproduction and meaningful-progress vocabulary

- Capture attempt/provider/adapter/runtime provenance and monotonic timestamps for phase entry, agent
  update, tool start/end, child spawn/exit, bounded stdout/stderr activity, file mutation, and test result.
- Define `meaningful_progress` by phase. Verification progress includes command start/exit and bounded
  output/heartbeat from an owned child; file edits are evidence but never the sole criterion.
- Reproduce at least: a child blocked forever, an agent waiter parked after a child exits, a silent but
  healthy long-running verification command, and a provider turn still emitting non-tool updates.
- Preserve a hypothesis/probe/result log. Do not label the incident root cause until the observations
  distinguish provider silence, adapter loss, child-process deadlock, and orchestration wait leakage.

## R2f1 — Phase-aware watchdog and stagnation snapshot

- Add an append-only phase/progress state machine with separate warning and hard-stagnation thresholds;
  use monotonic time and bounded low-cardinality reason codes.
- A warning snapshots the exact owned process tree, current phase/command category, last meaningful
  progress by category, elapsed phase time, worktree status/diff hash, and completed/pending verification
  gates. It does not cancel, retry, resume, or start another billable attempt.
- The hard threshold requires both phase stagnation and absence of a live owned child making bounded
  progress. A quiet process, old file mtime, or large total wall time alone is insufficient.
- Provider/adapter watchdogs and verification watchdogs remain distinct evidence; neither rewrites the
  other's diagnosis.

## R2f2 — Scoped termination and takeover artifact

- Termination is explicit operator action by default and targets one recorded attempt/process tree. Never
  use a broad name-based kill, kill unrelated repository tests, or discard the working tree.
- Stop children before the owned root using recorded identity plus start-time/generation checks so PID
  reuse cannot target an unrelated process. Record survivors and return partial failure rather than
  claiming success.
- Emit a bounded sanitized takeover artifact containing provenance, phase, last progress, exact scoped
  termination result, worktree diff/hash, gates completed, gates pending, and the command/result needed to
  resume verification. It contains no credential values or unbounded process output.
- A takeover reuses the preserved repository state only after an operator selects it. It is a new attempt;
  no automatic duplicate reviewer/model turn is started, and possibly accepted prompt work is never
  replayed silently.

## R2f3 — Shared backend/session health and non-disruptive rotation

- Preserve structured pre-turn ACP errors instead of collapsing session creation, configuration, transport,
  and capacity failures into generic `AgentCrashed`.
- Track created, active, warm, released, close-attempted, closed, retained, and unknown session state per
  exact backend generation. A local release cannot erase unresolved remote capacity debt.
- Negotiate and use `session/close` only when the exact adapter capability/protocol proves it exists. If it
  is absent or close fails, retain typed debt and use backend-generation ownership rather than inventing a
  successful close.
- Separate a poisoned transport from a capacity threshold with deterministic fake backends and bounded
  health evidence. Never hard-code the observed count 15 as a limit.
- Design rotation as side-by-side generations: new sessions select the new generation while every running
  turn and warm session remains on its owning generation. No automatic replay, forced warm-session close,
  broad process kill, or production restart is allowed.
- Before implementation, run a focused owner design increment for warm-session definition/expiry,
  generation retirement, health thresholds, operator authorization, and the exact non-disruptive swap
  protocol. An indefinitely retained warm session must remain visible rather than being killed to make a
  rotation appear complete.

## R2f4 — Tests and dogfood

- deterministic fake-clock tests for warning/hard thresholds and monotonic rollback immunity;
- silent healthy child versus blocked child versus exited-child/wedged-waiter classification;
- active test output prevents a false stall even when the last file edit is old;
- exact process-tree identity, PID-reuse defense, unrelated-process survival, partial-kill reporting;
- worktree edits survive termination and the takeover artifact names completed/pending gates exactly;
- observer/store failure cannot trigger termination or erase the primary diagnostic;
- no automatic retry, fallback, or second billable attempt;
- structured session-new/configure/transport/capacity failures retain their exact phase and cause;
- capability-present close success, capability-absent retention, close failure, duplicate close, and
  concurrent release preserve exact session/generation ownership;
- poisoned transport versus bounded fake capacity exhaustion are distinguishable without assuming a count;
- generation rotation routes new sessions to the successor while running and warm sessions continue on the
  predecessor, and an incomplete drain remains visible without interruption;
- one opt-in dogfood run that intentionally wedges a disposable verifier and proves targeted takeover.

## Completion

R2f is complete only after the verification stall and shared-operator alternatives have evidence-backed
dispositions; the phase-aware watchdog distinguishes the negative controls; scoped termination preserves
useful work; session/close and generation ownership are capability- and concurrency-safe; non-disruptive
rotation preserves running turns and warm sessions; takeover is exercised end to end; and a fresh
adversarial review approves the safety boundary. Until then the operator runbook permits evidence capture
and targeted manual termination only; it must not claim automatic recovery, session-capacity repair, or
safe rotation.
