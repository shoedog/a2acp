# R2f — Phase-aware liveness and safe takeover plan

- **Status:** DEFERRED; incident recorded, investigation not started
- **Prerequisite:** R2b structured diagnostics merged; may proceed independently of R2c–R2e afterward
- **Program source:** [`../../bridge-reliability.md`](../../bridge-reliability.md)
- **Program cursor:** [`../../reliability-execution-roadmap.md`](../../reliability-execution-roadmap.md)
- **Incident ids:** `INC-VERIFY-STALL-2026-07-11`, `INC-POST-WORK-WEDGE-2026-07-12-A/B/C/D`

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

The follow-up [2026-07-12 incident record](../2026-07-12-post-work-wedge-incidents.md) now supplies four
exact failure timelines plus high/xhigh negative controls. One Luna/max and three Sol/max implement runs
parked after useful content; one had already staged its plan and written `.git/A2A_COMMIT_MSG`. All four
stopped bridge stderr at `node edit started` and produced no `--out`. In the operator's 12 completed runs,
max separated 4/4 wedged from high 3/3 clean and xhigh read-only 5/5 clean. That makes requested effort a
high-value reproduction variable and makes a Luna-only or slow-Cargo-only explanation insufficient, but
the sample is non-random and still does not distinguish an agent loop, ACP terminal-delivery loss,
verification behavior, or bridge waiter/finalization leak.

## R2f0 — Reproduction and meaningful-progress vocabulary

- Capture attempt/provider/adapter/runtime provenance and monotonic timestamps for phase entry, agent
  update, tool start/end, child spawn/exit, bounded stdout/stderr activity, file mutation, and test result.
- Capture rollout terminal/update state, ACP `prompt_stream` and `prompt_finish`, workflow node terminal
  persistence, and `--out` finalization as distinct boundaries; a node start without any of those later
  facts must remain diagnosable after manual termination.
- Define `meaningful_progress` by phase. Verification progress includes command start/exit and bounded
  output/heartbeat from an owned child; file edits are evidence but never the sole criterion.
- Reproduce at least: a child blocked forever, an agent waiter parked after a child exits, a silent but
  healthy long-running verification command, a provider turn still emitting non-tool updates, an ACP
  terminal delivered to a bridge waiter that never finalizes, and a terminal withheld from that waiter.
- Run one paired, disposable implement reproduction with identical repo/task/adapter inputs at `high` and
  `max`. Record the resolved effort actually applied, rather than assuming the requested value reached the
  adapter, and treat any high wedge or max clean exit as evidence against perfect effort separation.
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

## R2f3 — Tests and dogfood

- deterministic fake-clock tests for warning/hard thresholds and monotonic rollback immunity;
- silent healthy child versus blocked child versus exited-child/wedged-waiter classification;
- active test output prevents a false stall even when the last file edit is old;
- exact process-tree identity, PID-reuse defense, unrelated-process survival, partial-kill reporting;
- worktree edits survive termination and the takeover artifact names completed/pending gates exactly;
- observer/store failure cannot trigger termination or erase the primary diagnostic;
- no automatic retry, fallback, or second billable attempt;
- paired high/max reproduction preserves all phase, rollout, child, and output-finalization evidence;
- one opt-in dogfood run that intentionally wedges a disposable verifier and proves targeted takeover.

## Completion

R2f is complete only after a reproduced failure has an evidence-backed root cause, the phase-aware
watchdog distinguishes the negative controls, scoped termination preserves useful work, takeover is
exercised end to end, and a fresh adversarial review approves the safety boundary. Until then the operator
runbook permits evidence capture and targeted manual termination only; it must not claim automatic
recovery.
