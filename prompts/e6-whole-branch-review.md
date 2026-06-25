You are doing a rigorous, adversarial WHOLE-BRANCH REVIEW (read-only) of the FULLY-IMPLEMENTED "E6 — Node Retry"
feature for the a2a-bridge (a Rust A2A↔ACP bridge + multi-agent workflow orchestrator). READ-ONLY: read the diff +
the binding spec/plan + the real code; do NOT edit/build/test.

E6 adds OPT-IN per-node retry of TRANSIENT agent failures (crash / `_dyld_start` startup flake / overload / watchdog
timeout) in the COLD workflow executor (`run_node`), default OFF (zero behavior change unless a node sets `retry`).
The reset between attempts = `release_session` (bridge-side, while the lease is live) + `AgentRegistry::invalidate`
(atomic Slot replace + detached lease-drained retirement → the next `resolve` RESPAWNS a fresh process).

- BINDING SPEC: `docs/superpowers/specs/2026-06-24-e6-node-retry.md` (`## v2` SR-FIX-1..6 + `## v3` RR-FIX-1..4).
- BINDING PLAN: `docs/superpowers/plans/2026-06-24-e6-node-retry.md` (`## v2` PR-FIX-1..10 + `## v3` RR2-FIX-1..6).
- The branch is `feat/e6-node-retry`; the impl commits are T1..T6 (`feat(core)`/`feat(workflow)`/`feat(registry)`).

The IMPLEMENTATION surface (read every change):
- `crates/bridge-core/src/error.rs` — `BridgeError::is_transient()` (`AgentCrashed | AgentOverloaded |
  AgentTimedOut`).
- `crates/bridge-workflow/src/graph.rs` — `RetryPolicy { max_attempts, backoff_ms, backoff_cap_ms }` +
  `attempts()` + overflow-safe `backoff_for()` + `WorkflowNode.retry` (rides `encode_workflow_spec`).
- `bin/a2a-bridge/src/config.rs` — `RetryToml` + `WorkflowNodeToml.retry` + the `load_workflows` mapping.
- `crates/bridge-core/src/ports.rs` + `crates/bridge-registry/src/registry.rs` — `AgentRegistry::invalidate`
  (trait default no-op + the real impl: shared `write_lock` with `apply`, atomic Slot replace, `retired=true` +
  `spawn_retirement`).
- `crates/bridge-workflow/src/executor.rs` — `run_node`'s retry loop (the core) + the retry tests + the
  `dropped_mid_retry_emits_no_checkpoint` resume test.
- `crates/bridge-coordinator/src/detached.rs` — `working_task_without_checkpoint_reruns_on_resume` resume test.

Reference code to verify against:
- `run_node` retry loop (`executor.rs`): the `'attempt` labeled block spanning resolve/configure/prompt/drain;
  the cancel-selects; `release_session` on retry vs `forget_session` on non-retry exits; `invalidate` + the
  cancel-abortable backoff; last-attempt usage; `ConfigInvalid`→Fatal.
- `Registry` (`registry.rs`): `Slot { entry: ArcSwap, backend: OnceCell }`, `State { slots, default }` in
  `state: ArcSwap`, `resolve` lazy-spawn + the `retired` re-check, `apply` + the shared `write_lock`,
  `spawn_retirement` (lease-drained, guards `slot.backend.get()`).
- `release_session` (`acp_backend.rs:2705`, bridge-side, process-preserving); the retirement lease (`ports.rs`
  `Resolved`); the watchdog process-kill on `AgentTimedOut`.

{{input}}

GROUND every finding in a real `file:line`. Pressure-test the WHOLE feature for CORRECTNESS + SAFETY + CONCURRENCY:

1. **The retry loop (the core).** Is the `'attempt` block faithful to the ORIGINAL `run_node` (cancellation arms,
   rich-sink flush, `forget_session` discipline, the 3-tuple return, usage preservation)? Is `release_session`
   ALWAYS done WHILE `Resolved` is in scope on the retry path (RR2-FIX-1 lease lifetime — never after the lease
   dropped)? Is `forget_session` (not release) used on Ok/Fatal/Canceled/exhausted exactly as before? Does
   `retry: None` reproduce the EXACT prior single-attempt behavior (zero behavior change)?
2. **Transient classification + fail-fast.** Is every failure site (resolve/configure/prompt/drain) gated by
   `is_transient()`? Does `ConfigInvalid` stay FATAL (no retry — the T6/SR-FIX-3 contract + the regression)? Could a
   NON-transient error ever wrongly retry, or a transient one wrongly fail-fast?
3. **Cancellation.** Does a cancel mid-attempt AND mid-backoff abort PROMPTLY (the biased selects)? Is a canceled
   retry NEVER re-attempted? Does cancellation correctly produce the canceled marker (not a retry marker)?
4. **The `invalidate` seam (concurrency — the riskiest).** Is the shared `write_lock` across `apply` + `invalidate`
   correct + deadlock-free (no `.await` inside the locked load→store besides the lock itself)? Can `apply` and
   `invalidate` still race to RESURRECT a removed slot or clobber `default`? Does `invalidate` no-op correctly on an
   unknown/vanished agent? Does replacing the Slot + `spawn_retirement` ever kill a process a CONCURRENT session
   still needs (or is the lease-drain correct)? Does `invalidate` on a never-spawned slot (resolve-time flake)
   behave (spawn_retirement guards `backend.get()`)?
5. **Respawn correctness.** After `invalidate`, does the next `resolve` actually RESPAWN (fresh `OnceCell`)? For a
   resolve-time `AgentCrashed` (failed spawn, uninitialized `OnceCell`), does the loop recover? Is the
   respawn-every-attempt cost acceptable + bounded by `max_attempts`?
6. **Usage + observability.** Is the reported usage the LAST attempt's (not summed — RR2-FIX-2)? On EXHAUSTION does
   it still return the last-attempt usage? Is the `tracing::warn` per-retry correct (no PII leak, right fields)?
7. **Resume-compat.** Does `dropped_mid_retry_emits_no_checkpoint` actually prove NO `NodeFinished`/checkpoint on a
   dropped (not canceled) future? Does `working_task_without_checkpoint_reruns_on_resume` prove a no-checkpoint
   retry node RE-RUNS? Is the deferral (exhausted `ok=false` checkpoints NOT re-run) intact?
8. **Plumbing + snapshot durability.** Does `WorkflowNode.retry` ride `encode_workflow_spec` resume-safe (additive,
   `skip_serializing_if`)? Is `backoff_for` truly overflow-safe (no panic/wrap for large `attempt`)? Are ALL ~42
   `WorkflowNode` literals correct (`retry: None`)? Does the `load_workflows` mapping carry retry through?
9. **Anything missed / wrong / over-built.** Any SR-FIX/PR-FIX/RR-FIX under-realized. Any wrong `file:line`. Any
   test that passes even if retry is broken (tautology). Any scope creep beyond the documented deferrals
   (respawn-only-on-death, resume-re-run of exhausted, warm-turn retry, write-node reset, rich NodeRetry event).

OUTPUT: a numbered findings list, each `BLOCKER | MAJOR | MINOR | NIT` + a real `file:line` + a concrete fix. End
with `REVIEW VERDICT: ship | fix-then-ship | needs-rework`.
