# 3b2 dual-lens adjudication — repair round R1–R4 (declared cap: ONE round)

Artifact: `feat/r2f1b-3b2-wrapper-authority` @ `90359127` (base `b8471c24`).
Lenses: opus senior-lead SHIP (0 WRONG, 9 SMELL — `2026-08-10-r2f1b-3b2-opus-lens.md`)
vs sol/max REJECT (2 WRONG-BLOCKER, 2 SMELL-DEFER — `2026-08-10-r2f1b-3b2-sol-lens.md`).
Adjudicator: orchestrator, at source on the s3b2 worktree (HEAD verified `90359127`).
Evidence context: darwin host gates all exit 0 (diff-check / fmt / clippy -D warnings /
full workspace test **3,932/0/12 across 90** / release build / repo hygiene); in-container
Linux verify PASS 3,858/0/12 across 88 with hermetic skips. The bridge-internal
implement-review NEVER inspected the diff (reviewer Authenticate-phase timeout ×3;
synth refused fail-open — correct evidence-admissibility posture; new degradation class,
distinct from the two registry-egress events).

## Verdicts (every mechanism re-verified at source; anchors on `90359127`)

| Finding | Verdict | Source evidence |
|---|---|---|
| sol-1 detached protective cleanup cannot settle durably | **REAL-WRONG (introduced by this slice)** | `MemoryWorkflowHistoryStore::settle_cleanup` (`workflow_history.rs:1838`) and SQLite `settle_cleanup` (`sqlite.rs:14117`) both hard-reject anything but `"complete" \| "failed"` with `LedgerUnavailableReason::Schema`; `finish_with_detached_cleanup` (`coordinator.rs:326-356`) writes initial terminal `"pending"` then settles detached with `disposition.as_str()` — the NEW values `retained`/`preserved`/`unknown` became producible by this diff; on `Err` the spawned task only `tracing::warn!`s → durable terminal stuck at `pending`. Production-reachable: inbound direct-unary failure path `server.rs:~4139` calls `finish_with_detached_cleanup("failed", …)`; a worktree gate refusal returns `Retained`. The green test `detached_cleanup_projects_each_protective_disposition_exactly` writes the disposition in the INITIAL terminal (sync prompt-barrier path) and never exercises `settle_cleanup` — a non-discriminating probe for this defect. |
| sol-2 OwnerHeld unsettled / sync no-claim collapses to Complete | **PRE-EXISTING — PARK with ledger** | Mechanism-level control against base `b8471c24`: (a) base `DetachedCleanupDisposition` was `Complete\|Failed\|OwnerHeld` and the base coordinator arm was `OwnerHeld => return` — identical skip; base wrote the same initial `"pending"` (`coordinator.rs:333` at base). (b) base sync `complete()` returned `Ok(())` on the Expire-no-claim arm, projected `"complete"` — the new `Ok(BackendCleanupDispositionV1::Complete)` (`dispatch.rs:368`) is the same durable outcome on the same inputs. The slice neither created nor widened the race (sol's own text: "the race predates this commit"). Downgrade is mechanism-proven (byte-equivalent durable outcomes vs base), not absence-of-counterexample. Owner: the slice-4/5 settlement-design ledger (owner-held receipt / two-phase settlement); rider R2 closes the adjacent sibling-projection trap now. |
| opus S1 lattice order (`Preserved > Retained`) vs sol invariant-3 PASS (order "matches the 2c durable-preservation labeling") | **LENS CONFLICT — adjudicated: keep shipped order; BINDING 3c1 row** | Primary evidence: the 2c2 record's durable-disposition monotonicity ranks Preserve as the top protective state (`max(cell, record)`; DeleteAuthorized between Reclaim and Preserve). The shipped `Unknown > Preserved > Retained > Complete` matches that precedent; opus's operational-visibility argument (a leaked container hidden behind `preserved`) is real but fires only when an inner backend can produce `Retained` — none can until 3c1. BINDING 3c1 dispatch row: before `ContainerRwBackend` may return `Retained`, either split inner/outer into two carried fields or re-argue the order with the container-leak scenario on the table. Also carries opus S2 (cell-evicted inner accumulator restarts at `Complete` — needs the durable re-derivation treatment 2c2 gave the checkout half). |
| opus S3 Cancel-arm discards disposition | **LEDGER (cfg-test-only producer)** | Sole non-test `workflow_owner` call site sits inside a `#[cfg(test)]` region (`server.rs:867-896`); becomes real when `workflow_owner` is promoted — same settlement-design ledger as sol-2. |
| opus S4 sibling projection hardcoded (`server.rs:4000-4004`) | **RIDER R2** | Provably safe today (Finish arm returns `Ok(Complete)` unconditionally) but the correct pattern is 285 lines away at `:4285`; one-line consistency fix in-round. |
| opus S5 + sol SMELL-1 (independent convergence): registry funnel discards refusals | **RIDER R4 (observability only)** | Four `let _ = Self::retire_join_or_refuse(…)` sites; the discard's MEANING changed (was: error from work done; now: possibly a decision that work would NOT happen). `tracing::warn!` with static fields; zero behavior change. Whether an unexposed backend on a drain is an invariant violation worth journaling → 3c1/slice-4 ledger. |
| opus S6 ResilientWarm deltas | **ACCEPTED (fail-closed trades)** | (a) unexposed-rebuilt not retired: unreachable (original must have exposed; rebuild returns same type); (b) refusal disables respawn: correct fail-closed. V2 LegacyV2 byte-identity confirmed by both lenses. |
| opus S7 Replay-in-integration-tests trap | **NOTE** | Replay never wrapped in production (factory wraps only the Acp arm); trap documented for the next test author. |
| opus S8 `Class::Config` classification + reader-compat one-way door | **NOTE (compat ledger)** | Static category correct for the wire; an older binary validating a row containing `retained` would reject — single-binary repo, same class as the 3b1 schema note. |
| opus S9 attach self-consistent by accident | **RIDER R3** | `attach_process_flight_owner` returns `Ok(())` when `supervised` is None; the public method is saved only by the re-read of `resource_flight_v1()`. Pin the ordering (comment, or explicit else-refusal if provably safe against 3b1's V2 universal-attach contract). |
| sol SMELL-2 legacy release entrances outside funnel | **LEDGER (theoretical-only under current factory)** | `expire_retired_idle_child` + configure/reset/compact rollbacks call unit `release_session` directly; sol's own likelihood: theoretical-only. Funnel consolidation → slice-4 ledger. |

Dedup: sol-1 is the durable half of the M11 chain both lenses traced — opus verified the
chain to `AttemptTerminal` validation (`workflow_history.rs:527-536`) and stopped one layer
short of the store `settle_cleanup` closed set; complementary-lens value demonstrated again.

## Repair directives (dispatched to sol/xhigh via bridge implement; base = branch tip `90359127`)

- **R1 (the blocker):** widen `settle_cleanup` accepted vocabulary to
  `complete | retained | preserved | unknown | failed` in BOTH stores
  (`MemoryWorkflowHistoryStore`, SQLite) and the trait contract's doc;
  `pending` stays initial-only (never a settlement value); CAS / replay /
  conflict / accounting semantics unchanged. Red-first: drive
  `finish_with_detached_cleanup` through each protective value against BOTH
  stores and assert the exact value lands durably (the existing green
  coordinator test is non-discriminating — the new tests must fail on
  `90359127`); negatives: invalid vocabulary (incl. `pending`) still Schema;
  conflicting second settlement still Conflict.
- **R2:** `server.rs:4000-4004` projects the typed disposition
  (`disposition.as_str()`) like its `:4285` sibling; control if cheap.
- **R3:** pin the attach ordering in `AcpBackend`
  (`attach_process_flight_owner` None-arm): comment pinning why the
  `resource_flight_v1()` re-read is load-bearing, or an explicit else-refusal
  if provably compatible with 3b1's V2 universal-attach contract.
- **R4:** `tracing::warn!` (bounded, static fields: refusal-vs-error category)
  on `retire_join_or_refuse` failure at the funnel; zero behavior change.

## Cap and convergence

Declared cap: ONE targeted repair round on the existing artifact (this document is the
declaration). Findings are closed-enumerable (each names state, wrong result, bounded fix).
If the round's review surfaces open-class findings, park and escalate per steering.
V3 remains route-unarmed; arming stays slice 4. The internal-reviewer auth-timeout
degradation is a pipeline item, not an artifact item; recurrence at the repair round
triggers the reviewer-auth investigation.
