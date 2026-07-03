You are doing a focused, adversarial RE-REVIEW (read-only) of the REVISED implementation plan "E6 — Node Retry" for
the a2a-bridge (a Rust A2A↔ACP bridge + multi-agent workflow orchestrator). A first plan-review found 4 BLOCKER + 6
MAJOR; all were folded into the plan's `## v2` section (PR-FIX-1..10). YOUR JOB: verify each fold actually RESOLVES
its finding + is IMPLEMENTABLE against the real code, and hunt for NEW issues the v2 mechanics introduce — especially
in the T4 registry seam (the shared write-lock + the reused detached retirement) and the T5 attempt model. READ-ONLY.

- PLAN: `docs/superpowers/plans/2026-06-24-e6-node-retry.md` — read the **`## v2` section (BINDING)** first, then the
  v1 task bodies it supersedes.
- BINDING SPEC: `docs/superpowers/specs/2026-06-24-e6-node-retry.md` (`## v2` SR-FIX-1..6 + `## v3` RR-FIX-1..4).

The v2 folds to validate against the real code:
- **PR-FIX-1 (T2):** `retry: None` at ALL ~32 workspace `WorkflowNode {` literals — spot-check the cited sites
  (`bin/a2a-bridge/src/main.rs:4729`, `crates/bridge-a2a-inbound/src/server.rs:6296`,
  `crates/bridge-coordinator/src/coordinator.rs:886`, `detached.rs:1080`, `crates/bridge-mcp/tests/mcp_client.rs:222`)
  + grep for any MISSED literal (incl. macro/builder-generated). Is the count right? Any `WorkflowNode` built via a
  helper/Default that the plan didn't account for?
- **PR-FIX-2 (T4):** a shared `write_lock: Mutex<()>` across `apply` + `invalidate`'s load→modify→store. Does adding
  this lock to the EXISTING `apply` (`crates/bridge-registry/src/registry.rs:366-414`) change its semantics or
  deadlock (apply currently holds NO such lock; does anything call apply re-entrantly or hold state across an await)?
  Is "no-op if the agent vanished in the current state" correctly expressed (re-load UNDER the lock)?
- **PR-FIX-3 (T4):** reuse `apply`'s detached lease-drained retirement (`registry.rs:416-425`) — is that retirement
  logic EXTRACTABLE into a shared helper without changing apply's behavior? Does it depend on the `old.slots` iteration
  / `Arc::ptr_eq` "kept" check that won't translate cleanly to a single-slot invalidate? Does `invalidate` correctly
  set `retired=true` SYNCHRONOUSLY before spawning (the resolve race at `:322`)? Is `self.grace` the right field?
- **PR-FIX-4 (T4):** `Slot::new((*entry).clone())` returns `Arc<Slot>` — does `entry` come from
  `old_slot.entry.load_full()` (an `Arc<AgentEntry>`), so `(*entry).clone()` is an `AgentEntry`? Verify the exact deref.
- **PR-FIX-5 (T5):** the `Attempt { Transient { err, backend: Option<Arc<dyn AgentBackend>> } ... }` model — does
  threading `Option<backend>` out of the per-attempt block compose with the existing cancel-selects + rich-sink +
  `forget_session` discipline in `run_node` (`executor.rs:158-388`)? Where exactly does the resolve-error branch
  (`:266-273`) produce `backend: None`?
- **PR-FIX-6/8/9/10 (tests):** are the proposed test shapes actually realizable against the existing harness
  (`Rec`/`FakeBackend` `:646`, `one_node_graph` `:761`, the detached/checkpoint recorder)? Does the
  resolve+invalidate-counting fake registry (PR-FIX-6) need to wrap the real `Registry` or a fresh fake `impl
  AgentRegistry`?
- **PR-FIX-7 (T6):** is "drop the runner future mid-retry → no NodeFinished" actually testable + true? Trace where
  `NodeFinished` is emitted (`executor.rs:615`) relative to the run-future being dropped; does the detached harness
  let a test drop/abort the run task mid-backoff?

{{input}}

GROUND every finding in real `file:line`. Pressure-test the V2 DECISIONS specifically:
1. **T4 write-lock + retirement (the riskiest).** Is the shared-lock + detached-retirement design correct, deadlock-
   free, and faithfully reusing `apply`'s mechanism? Any NEW race (lock held across an await that blocks resolve;
   the detached retirement referencing a moved/cloned slot)? Is the `invalidate` no-op-on-vanished-agent correct?
2. **T5 attempt model.** Does the `Attempt` enum + `Option<backend>` cleanly replace the v1 pseudo-code without
   breaking cancellation/usage/forget? Is the reset order (release-if-backend → invalidate → backoff → re-resolve)
   correct, and does re-resolve after invalidate actually get the fresh process?
3. **Compile-green completeness.** With the v2 folds, will each task still be compile-green? Any remaining literal,
   trait impl, or type the v2 text still misses?
4. **Test honesty.** Do PR-FIX-6/7/8/10's tests actually PROVE their property (resolve-in-loop, crash-not-cancel,
   ConfigInvalid-fatal-with-retry, last-attempt-usage), or can they still pass with a broken impl?
5. **New issues / residue.** Anything the v2 folds introduced or left ambiguous enough to block implementation. Any
   wrong `file:line`. For each v1 finding state RESOLVED / PARTIALLY / NOT.

OUTPUT: a numbered findings list, each `BLOCKER | MAJOR | MINOR | NIT` + a real `file:line` + a concrete fix. End
with `RE-REVIEW VERDICT: ready-to-implement | needs-revision`.
