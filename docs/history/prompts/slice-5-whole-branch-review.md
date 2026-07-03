You are an expert Rust + concurrency reviewer giving an INDEPENDENT, adversarial WHOLE-BRANCH review of the
COMPLETE Slice 5 implementation on `a2a-bridge` (an ACP↔A2A bridge + workflow orchestrator). Your session cwd
IS the a2a-bridge repo, on branch `feat/slice-5-serve-cli`. The whole feature is below the marker.

READ-ONLY + reason from code (do NOT run `cargo`, edit, or commit — the controller runs the build/test gate
separately). Inspect the FULL branch diff: `git diff main...HEAD` (8 implementation commits T1–T8). The
per-increment reviews looked at each task IN ISOLATION; YOUR job is to find the bugs that only appear ACROSS
tasks — cross-task lifecycle races, seam mismatches, leaks, and back-compat breaks the per-commit reviews can't
see. This is the review that historically catches the real bugs.

## The feature (what to verify end-to-end)
`run-workflow --serve --context C <wf>` makes the CLI a streaming serve client so a workflow's per-node agent
sessions stay WARM + reuse across runs. Design-of-record: `docs/superpowers/specs/2026-06-19-slice-5-serve-cli.md`
(FIX-1..11 binding) + `docs/superpowers/plans/2026-06-19-slice-5-serve-cli.md` (binding `## v2…v13`/`PFIX-*`).
KEYSTONE: the executor (`bridge-workflow`) must NOT import `SessionManager` (`bridge-a2a-inbound`) — the seam is
a dependency-inversion trait (`WorkflowNodeDispatcher`).

## Trace these END-TO-END across the commits (cite file:line)
1. **The warm child lifecycle, full circle.** `stream_message` guard → `spawn_workflow_producer` (warm branch) →
   `executor.run_node` (warm) → `WarmWorkflowNodeDispatcher::checkout` → `SessionManager::checkout_child_turn`
   (registers `parent→child`) → node runs → `NodeTurnCleanup::on_exit` → `finish_turn`/`cancel`/`expire_turn`.
   Does a node session correctly go Running→Idle (kept warm) on Normal, and is it REUSED on the next run? Does
   `finish_turn`'s gen+op no-op guard actually match what the cleanup passes (else the child strands `Running`)?
2. **Cancel/cleanup races (the highest-risk area).** SessionCancel C cancels the run token (NOT removing the
   guard) + `cancel_with_children`. The executor's drain-on-cancel fires `on_exit(Canceled)→sm.cancel(child)`.
   Is there EXACTLY ONE `backend.cancel` per child (idempotent cancel)? Can a 2nd same-context run re-claim a
   child mid-teardown? Does the producer's `catch_unwind` cleanup free children + remove `workflow_runs[C]` on
   EVERY exit (normal, panic, absent-executor early-return, abort-Drop) with NO leak and NO double-free?
3. **Back-compat (BLOCKER if broken).** The cold executor path (`run_node` `None` branch) and the non-`--serve`
   CLI path must be BYTE-IDENTICAL to pre-Slice-5. Confirm via the diff that the `None`/non-serve branches are
   pure additive inserts and nothing in the shared path changed observably.
4. **The guard before state mutation.** Both the streaming workflow guard and the unary workflow+context reject
   sit BEFORE their `store.put`. A rejected request must not mutate `SessionStore`.
5. **Spec faithfulness.** Each FIX-1..11 actually implemented? Any gap or scope-creep across the branch?
6. **Lock ordering / deadlock.** `children`→`by_context` everywhere; no path takes them in the other order;
   no lock held across an `.await` that could deadlock.

## Output (plain text, no fence)
- **VERDICT:** APPROVE / APPROVE-WITH-NITS / CHANGES-REQUESTED.
- **FINDINGS:** numbered, tagged [BLOCKER]/[MAJOR]/[MINOR], each citing `file:line` + the FIX/PFIX/v-note, with a
  concrete fix. Prioritize cross-task / lifecycle / back-compat issues.
- End with the VERDICT line exactly. If clean, say so explicitly.

THE FEATURE:

{{input}}
