[implement] review Reject:
I’ll perform one bounded, hard read-only closure pass. I’m using the prism navigation skill because the review explicitly requires structural blast-radius tracing; I’ll read its instructions, then the supplied review slice first, and verify every lead against the committed code and production callers. I will not run builds or tests.The supplied prism slice is empty—no precomputed findings or paths—so it provides no substantive evidence. I’m treating it only as a map and will derive the review from the exact commit diff, then use structural navigation for the changed production methods and read each returned caller before relying on it.The first production-risk lead is in reopen’s two-step orphan repair: it durably advances the checkpoint before replacing the child status. I’m tracing Task A’s replacement/recovery outcomes now to determine whether every failure of the second step leaves recoverable residue or whether a reachable pre-mutation failure can strand an unissued child as apparently issued. I’m also checking whether the two claimed mid-retire cuts are genuinely distinct tests rather than duplicate labels.The code confirms two blocker classes beyond the supplied gate result: a crash/fault during the second half of orphan healing can permanently reclassify an unissued child as an issued active child, and the “real-adapter” repair still bypasses the actual stage/replace adapters in its existing fault cases. I’ve completed the caller/persistence trace: this module remains exported but has no production callers yet, so current live exposure is unarmed; the defects become reachable at the intended Task C activation boundary.## WRONG findings

1. **WRONG — The committed change fails the configured clippy gate. `BLOCKER`.**

   The supplied verification reaches compilation but exits 101 on three change-local diagnostics: deprecated `std::io::Error::new(ErrorKind::Other, ...)` calls at [remote_request_flight.rs:841](/Users/wesleyjinks/code/.a2a-implement/impl-9079-5czgier2/crates/bridge-core/src/remote_request_flight.rs:841) and [remote_request_flight.rs:865](/Users/wesleyjinks/code/.a2a-implement/impl-9079-5czgier2/crates/bridge-core/src/remote_request_flight.rs:865), plus the single-element loop at [remote_request_flight.rs:1382](/Users/wesleyjinks/code/.a2a-implement/impl-9079-5czgier2/crates/bridge-core/src/remote_request_flight.rs:1382).

   - Trigger: every all-target clippy run with `-D warnings`.
   - Likelihood: **common**; deterministic on the configured Rust toolchain.
   - Exposure/impact: all integrators and CI; the artifact cannot satisfy the repository’s green-tree acceptance gate.
   - Fix: use `std::io::Error::other("injected")` twice and replace the loop with `let boundary = 4;`. Approximately three test-only lines.
   - Red regression: rerun the exact configured clippy command; it is already red on this commit.
   - Rationale: **BLOCKER** because verification is deterministically red and the repair is trivial.

2. **WRONG — The real-adapter fault-injection repair remains partial. `BLOCKER`.**

   Publishing and retirement now execute their real adapters, but stage and acknowledgement replacement still return from a pre-adapter seam. Admission calls `task_a_boundary(Stage)` before `op.stage` at [remote_request_flight.rs:618](/Users/wesleyjinks/code/.a2a-implement/impl-9079-5czgier2/crates/bridge-core/src/remote_request_flight.rs:618), and `replace_child` calls `task_a_boundary(Replace)` before `NamespaceTransactionV2::replace` at [remote_request_flight.rs:411](/Users/wesleyjinks/code/.a2a-implement/impl-9079-5czgier2/crates/bridge-core/src/remote_request_flight.rs:411). Worse, `task_a_boundary` feeds synthetic `Ok(())` through the journal mapper at [remote_request_flight.rs:880](/Users/wesleyjinks/code/.a2a-implement/impl-9079-5czgier2/crates/bridge-core/src/remote_request_flight.rs:880), so the replacement case does not exercise the production namespace-transaction mapper.

   The concrete incorrect test result is visible in the regressions: injected stage asserts the root remains unchanged, and injected acknowledgement replacement also asserts unchanged bytes at [remote_request_flight.rs:1604](/Users/wesleyjinks/code/.a2a-implement/impl-9079-5czgier2/crates/bridge-core/src/remote_request_flight.rs:1604) and [remote_request_flight.rs:1628](/Users/wesleyjinks/code/.a2a-implement/impl-9079-5czgier2/crates/bridge-core/src/remote_request_flight.rs:1628). A regression in either actual adapter therefore remains green—the exact repeat class repair 4 required eliminating.

   - Trigger: any stage or replacement adapter regression while these protective-outcome cases run.
   - Likelihood: **plausible**; the bypass occurs on every such test execution.
   - Exposure/impact: maintainers receive false-green custody evidence; once Tasks C–G arm this module, an adapter regression could affect request durability.
   - Fix: inject raw faults at or beneath each actual adapter, then consume its raw result through `mutation` or `transaction` as appropriate. Add side-effect/call assertions proving stage and replacement executed. Small-to-medium test-seam change.
   - Red regression: require observable stage creation and replacement execution under injected outcomes; both assertions fail on the current pre-call seams.
   - Rationale: **BLOCKER** because one of the four explicitly required repairs is not fully delivered.

## SMELL findings

1. **SMELL — Below-checkpoint active children remain intrinsically ambiguous. `DEFER`.**

   The new boundary-5 test constructs an admission that returned an error without returning authority, yet leaves checkpoint 1 plus active child 0 and deliberately preserves it on reopen at [remote_request_flight.rs:1411](/Users/wesleyjinks/code/.a2a-implement/impl-9079-5czgier2/crates/bridge-core/src/remote_request_flight.rs:1411). The same durable shape can also result from a successful admission, so B2 cannot safely distinguish the cases. A failure after checkpoint healing but before child closure at [remote_request_flight.rs:337](/Users/wesleyjinks/code/.a2a-implement/impl-9079-5czgier2/crates/bridge-core/src/remote_request_flight.rs:337)–[345](/Users/wesleyjinks/code/.a2a-implement/impl-9079-5czgier2/crates/bridge-core/src/remote_request_flight.rs:345) has the same ambiguity.

   - Trigger: crash or transient filesystem failure after checkpoint replacement but before authority return or child closure.
   - Likelihood: **rare**, and there are currently no production callers.
   - Exposure/impact: future Task-C request runs could retain an unowned active row indefinitely, consuming capacity.
   - Fix: Task C should durably distinguish admitted/issued/send states and add paired crash regressions for successful issuance versus pre-return failure. This is cross-cutting rather than a safe B2 heuristic.
   - Rationale: **DEFER** because the task explicitly reserves durable send-state discrimination for Task C, and guessing in B2 could corrupt genuinely issued requests.

## Evidence assessment

I read the complete changed module and handoff, the entire diff, and the relevant Task A transaction/recovery paths. The supplied prism slice was empty; repository search found only the `lib.rs` export and no production caller, persistence consumer, route, or served projection. Thus production V3 remains unarmed as claimed.

The diff is confined to the two authorized files and totals 397 changed lines; the documented 99 production-line churn is below the cap. `git diff --check` is green. Supplied fmt and build are green, but clippy is red. The supplied full test run also exits 101 in the `a2a-bridge` binary target; its excerpt lacks the named failing test and a same-environment base control, so it is not admissible for regression attribution, but full-suite green evidence is absent. I did not rerun builds or tests under the read-only contract.

Confidence: **96/100**. Exact source, persistence mechanics, and deterministic clippy diagnostics raise confidence; the truncated full-test failure lowers it slightly. A same-environment base/control log could classify that separate test failure but would not collapse either blocker above.

VERDICT: REJECT
SUMMARY: The change is blocked by deterministic clippy failures and an incomplete real-adapter fault-injection repair; the below-checkpoint ambiguity remains deferred to Task C.
implement: committed 09a19025194e239166548e75fce088cff7ea000f "fix(r2f1b): scope reopen healing and authorize before recovery" on implement/impl-9079-5czgier2
clone: /Users/wesleyjinks/code/.a2a-implement/impl-9079-5czgier2
After an Approved result, use the guarded operator-authored hand-off (add --config when non-default):
  a2a-bridge merge impl-9079-5czgier2 --onto <target>
For an inspected parallel sibling whose target advanced from the shared base:
  a2a-bridge merge impl-9079-5czgier2 --onto <target> --integrate-current

verify: FAIL at clippy  (fmt reached exit=0 ✓ · clippy reached exit=101 ✗ command · build reached exit=0 ✓ · test reached exit=101 ✗ command)
review: REJECT  (The change is blocked by deterministic clippy failures and an incomplete real-adapter fault-injection repair; the below-checkpoint ambiguity remains deferred to Task C.)
loop: 1 attempt(s) — bound reached
e1aa575b437f3a30c73f49369307fb38a329dbdb6ba700a2f51438de365e995f
