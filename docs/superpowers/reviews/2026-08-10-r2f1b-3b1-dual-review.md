# R2f1b 3b1 (process authority I) — run ledger, dual review, rulings

Slice: 3b1 per `docs/superpowers/plans/2026-08-09-r2f1b-slice3-brief.md` §3 —
Supervised internal to `OwnedProcessTreeV1`, all signal paths join-or-refuse,
verified-stop containment closure, V2 legacy arms byte-identical with controls,
flight-before-spawn, one registry per attempt. M18 prerequisite discharged by
`2026-08-10-r2f1b-3b1-descendant-kill-decision.md` (planning branch; appendix
filled with the 19/20 baseline control on `0a3c2434`).

## Run ledger (all via bridge implement, gpt-5.6-sol/xhigh; branch `feat/r2f1b-3b1-process-authority`)

1. **Base run** `bec2e01b` (clone `impl-10675-5dkhnukw`; +2,690/−251) — hit the
   3-attempt bound, internal REJECT. Operator verified all four internal
   findings REAL at source (census-stable-without-verified-stop; four
   `.is_err()`-only teardown sites; discarded attach/detach Results; zero-caller
   V3 constructor) plus two operator finds (out-of-scope fs_custody tripwire
   weakening — reverted; darwin lane did not compile under the pinned libc —
   the Linux-only container verify could never see it).
2. **Continuation** `45a72f01` (clone `impl-26449-n7y4cb58`; D1–D6) — bound
   reached CONVERGING; all six directives verified discharged; riders accepted
   (reaper release-gate test strengthening; container initializer ripple).
3. **Dual lens** over `0a3c2434..45a72f01`: opus senior-lead **REVISE**
   (2 WRONG / 8 SMELL — `…-3b1-opus-lens.md`) vs sol/max **REJECT**
   (6 WRONG-BLOCKER / 1 SMELL — `…-3b1-sol-lens.md`).
4. **Adjudication** (`…-3b1-dual-adjudication.md`): every WRONG re-verified at
   source — ALL REAL, zero literalism rejections; eight distinct defect
   classes; repair directives R1–R7; declared cap ONE round.
5. **Repair round** `495d7474` (clone `impl-31902-w2ubb7wj`; +1,407/−242 across
   exactly the three named files) — converged at attempt 2 (attempt 1 internal
   REJECT: red clippy `let_and_return` + missing R7-item-5 note; both fixed).
   Final container verify PASS (fmt/clippy/build/test), internal review
   APPROVE. Operator inspection confirmed every R-directive mechanism at
   source; guarded-merge hand-off commit tree byte-identical to the reviewed
   commit; fast-forwarded onto the branch.

## What the repair landed (R1–R7)

- **R1** anchor-based census admission (live immutable anchor admits every
  kernel-confirmed same-PGID member, including reparented/double-forked
  descendants); empty census while the root identity is live = census ERROR;
  anchor-loss with remaining same-PGID members = `ContainmentUnstable`, never
  vacuous `Complete`. darwin census: absolute `/bin/ps` as enumerator only —
  deadline-bounded, byte-bounded, every row's pgid re-validated via
  `libc::getpgid` before use; no PATH resolution; no libc bump.
- **R2** full protective-lifecycle capacity (`PROCESS_LIFECYCLE_SLOTS = 7`)
  reserved BEFORE `Command::spawn`; insufficient cap refuses with zero spawn
  calls (cap-2/cap-7 tests). V2 owner attach/detach evidence non-failing
  (`attach/detach_owner_legacy_v2`): journal `Full` can no longer kill warm
  sessions or retirement (512-cap churn test) — closes the production
  ~254-session-cycle permanent-failure regression.
- **R3** exited/zombie members count as contained (they cannot fork); every
  `close_and_kill` refusal exit SIGCONTs the members its volleys stopped —
  the frozen-SIGSTOPped-tree refusal state is gone.
- **R4** injectable direct-child-wait seam; reap-deadline expiry with the
  child unreaped records a typed timeout and settles `Failed` — never
  `Complete` — in both V2 and V3 arms (signal shapes unchanged).
- **R5** NotFound-while-Driving/Finished JOINS the in-flight result (recycled
  different-live-identity still refuses); `settle` returns the authoritative
  adopted terminal and `settle_dispatch` projects it to every caller; ACP
  consumers classify any non-`Complete` disposition as failed.
- **R6** the darwin host red was the TEST spawning `/bin/true` (absent on
  macOS) — per-platform binary; the ImmutableStart leg now genuinely
  exercises; production was correct all along (ledger description superseded).
- **R7** `finish_action` unwind guard (driver panic wakes joiners with a typed
  error); poison-tolerant V2 drop group SIGKILL; exact serialization goldens +
  strict-reader negatives for the new wire events; dead `read_only_controller`
  deleted; kill-switch notify ordering kept with an in-code cancellation-safety
  justification (the reorder could drop the escalation task mid-terminate).

## Operator host gates (darwin, s3b1 worktree, evidence = real exit codes)

- `cargo fmt --all --check` OK; `cargo clippy --workspace --all-targets -- -D
  warnings` exit 0.
- bridge-core host: **540/0** lib (was 524/1 — the darwin red is green; net
  +16 tests), all package harnesses green.
- Full workspace: **3,904 passed / 0 failed / 12 ignored across 90 harnesses**,
  exit 0 (3,866 → +38). Not run on host: container-hermetic verify (ran
  in-pipeline, PASS), gated e2e (kiro-cli), container tests — same exclusions
  as every prior fold.

## Rulings and ledger

- Darwin-red root cause CORRECTED by the opus lens (test binary path, not a
  missing pid row) — prior ledger description superseded.
- D3 residual (owner rows carry fingerprint, not raw owner id) stays DEFERRED:
  consistent with leak-guard redaction posture.
- V3 remains route-unarmed; arming is slice 4. Opus DEFER "V3 unarmed backend
  drop orphans the tree" is the stated capability-gating trade — slice 4 must
  confirm the contract when it arms the route.
- `RESOURCE_FLIGHT_JOURNAL_SCHEMA_V1` stays 1 while the event set grew —
  single-binary repo; noted for any future mixed-version reader.
- **3b2 obligations**: real-host V3 containment-closure coverage (opus S1 —
  the SIGSTOP-verify → child-first-SIGKILL sweep has never run against real
  syscalls with real descendants; lands with 3b2's wrapper/trait pass);
  `LIFECYCLE_SLOTS`/`PROCESS_LIFECYCLE_SLOTS` values now golden-pinned — keep
  them pinned through the trait sweep.
- Slice-5 carries (unchanged from 3a): per-owner outbox, lease-proven
  liveness, transfer sink, journal serialization.

Fold-gate totals + coverage delta in the addendum below.
