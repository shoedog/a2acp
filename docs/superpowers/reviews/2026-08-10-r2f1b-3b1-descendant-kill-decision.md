# 3b1 M18 diagnostic prerequisite — descendant-kill decision record

Date: 2026-08-10. Author: Fable orchestrator. Subject: the parked
`compatibility_resolution::tests::process_executor_kills_descendants_when_execution_is_cancelled`
residual (flake-fix review `2026-08-08-flake-fix-review.md`, commit `18b0c2c2`
PARKED paragraph) plus the same-family `staged_candidate_*` kevent hang. Slice-3
brief §1.3.10 / §3-3b1 requires this record BEFORE the process-authority
implementation gate: hypotheses, same-environment control, and an explicit
absorbed-by-tree vs re-parked disposition.

## 1. The observed defect (carried evidence, not re-derived)

Under whole-bin load (`--test-threads=32`), cancellation of a resolver command
lets a descendant survive the fail-safe `kill(-pgid, SIGKILL)` at ~2%/run
(4/42 failing runs pre-change AND post-change at ×3 concurrent invocations —
the same-environment control is already on record in `18b0c2c2`). Instrumented
at failure: the abort returned 12–13 ms in, the survivor's 200 ms timer was
~188 ms away, and the marker was unwritten — the descendant genuinely outlived
the group SIGKILL; this is not a test-window race.

## 2. Hypotheses, with expected observations (verified against live main `0a3c2434`)

Source facts established today, fold worktree at `0a3c2434`:

- The cancellation fail-safe is `AnchoredProcessGroup::Drop`
  (`bin/a2a-bridge/src/compatibility_process_group.rs:584-606`): guarded by
  drop-policy + retained-anchor-identity checks, then
  `libc::kill(-pgid, SIGKILL)` with the **return code discarded** (`:599`).
  The direct child additionally gets `start_kill()` (and tokio
  `kill_on_drop(true)`, `compatibility_resolution.rs:2194`), so the shell dies
  by pid regardless; the surviving writer is the backgrounded subshell.
- `signal()` (`:528-553`) DOES observe its rc, but it runs only on the
  non-cancellation terminate path (`terminate_command_process_group`,
  `compatibility_resolution.rs:2138`), which also has the
  identity-verified no-stale-PGID fallback.

**H1 — group-signal/fork non-atomicity (darwin).** `kill(-pgid)` enumerates
current pgrp members; a `fork()` concurrent with delivery can produce a child
that is not yet linked into the pgrp list while the parent's pending SIGKILL
does not propagate to it. Sharp variant matching the evidence: the kill lands
while `sh` is inside `fork()` of the backgrounded subshell (load-delayed past
the test's 10 ms poll); the subshell is born orphaned-alive, `sh` dies, the
subshell touches the survivor marker ~200 ms later.
*Expected observation at a failure:* Drop ran, guard passed, kill rc == 0.
*Falsified by:* kill rc == -1, or the guard short-circuiting.

**H2 — the Drop-time kill did not run or was refused.** Enumerated against the
live source: (a) Drop never executed — refuted structurally (the observed
`JoinError::is_cancelled()` implies the future was dropped); (b) the identity
guard failed — requires a construction-time field pair to diverge, no
mechanism found; (c) `kill` returned -1 (e.g., transient ESRCH on a live
group) — **unobservable today because the rc is discarded**. H2 therefore
reduces in practice to (c).
*Expected observation at a failure:* kill rc == -1 with its errno.
*Falsified by:* rc == 0 at a failure instance.

The prior discriminating probe did not fire in 40 instrumented runs — at
~2%/run that is under-powered (P(zero instances) ≈ 45%), not suppressed. A
probabilistic re-run remains under-powered at any bounded n; the discriminator
that actually closes this is (i) an always-on rc observation at the Drop site
and (ii) a deterministic fork-during-sweep interleaving under an injectable
OS port — which is exactly 3b1's B10 machinery.

## 3. Same-environment control

Primary control (on record, `18b0c2c2`): pre-change 4/42 vs post-change ~2%/run
under identical load shape — the residual is pre-existing and change-independent.
Fresh baseline: N=20 sequential whole-bin runs at `--test-threads=32` on live
main `0a3c2434`, same host, launched with this record (results appended below
before the 3b1 fold gate; at ~2%/run, n=20 bounds the family's presence, it
cannot bound the exact rate — stated honestly). The baseline serves R3-2
fold-gate attribution, not the disposition below, which rests on the recorded
control + mechanism analysis.

## 4. Decision

**(a) The defect CLASS — ABSORBED-BY-TREE.** 3b1's `OwnedProcessTreeV1`
replaces the bare-group-kill shape for every owned tree, and its dispatch spec
carries these constraints as MANDATED acceptance surface:

1. The descendant-containment mechanism must be closed against forks
   concurrent with the containment sweep: the fake-OS (B10 port) ordering
   tests must include a fork-during-sweep interleaving that the mechanism
   provably terminates and contains (rescan-until-stable, stop-then-kill, or
   equivalent — implementer's design; the red test is the constraint). A bare
   `kill(-pgid)` cannot pass this test — that is the point (B9).
2. Every kill/containment syscall outcome is OBSERVED and journaled — a
   refused or failed signal is a typed, journaled event, never a discarded
   rc. This closes H2(c)'s observability hole for all 3b1-owned trees.
3. The V2 drop control (B17) pins that a V2 session's drop still reaps
   exactly as today.

**(b) The resolver INSTANCE — RE-PARKED with named owner + observability
hooks.** `compatibility_process_group` is census-EXCLUDED from the slice-3
raw-signal migration (brief §6.4: the compatibility harness's own process
groups are test/diagnostic infrastructure, not R2f1b-owned resources), so 3b1
does not rebuild this site. Disposition:

- **Observability hook (lands with 3b1, bounded, disclosed):** observe and
  record the `kill` rc + errno at `AnchoredProcessGroup::Drop` (and count via
  the existing `signal_attempts` shape) — diagnostic only, no behavior
  change. This makes the H1-vs-H2 discriminator always-on, so the next
  natural failure instance IS the discriminating observation the 40-run
  probe never caught.
- **Named owner:** the parked-flake family ledger (task #9 lineage: this
  residual + the `staged_candidate_*` kevent hang). Re-investigation triggers
  on the FIRST hook-bearing failure instance; until then no further
  probabilistic probes (they stay under-powered by construction).
- **Fold-gate attribution rule (R3-2):** a failure of
  `process_executor_kills_descendants_when_execution_is_cancelled` (or the
  kevent-hang pair) in a 3b1 gate run is attributed to the parked
  pre-existing family — citing §3's controls — and reported as such, never
  silently re-run to green and never charged to the slice without the hook
  showing a NEW signature. Both families remain hermetic-container-excluded;
  host gates are the evidence.

**(c) The kevent-hang symptom** (1/68 post vs 0/78 pre, Fisher p≈0.47 —
indistinguishable) stays parked under the same family owner; the per-run
timeout on gate runs is the containment.

## 5. Baseline appendix (filled before the 3b1 fold gate)

- [pending] N=20 whole-bin baseline on `0a3c2434`: exit codes + any failure
  signatures, log `scratchpad/3b1-baseline-control.log`.
