---
task-type: design
---

# R2f1b 3d T3a rebuild — what is actually left, and how to wire it

## Description

Design the remaining work for slice 3d task **T3a**, on a tree that has moved
substantially since T3a was written. Produce a plan an implementer can execute.
Do not write code.

**Read the repository.** You have the working tree at the session cwd, checked
out at `main` = `9aedf175`. Every anchor below was measured on that commit and is
a claim you may disprove — see the falsification license.

### The one-paragraph history you need

The worktree lane had a defect class it got wrong five times: a path compared by
**spelling** where **identity** was required, mostly failing OPEN in proofs whose
contract is fail-closed. That class was escalated into a shared tri-state
path-identity primitive (`Same` / `Different` / `CannotProve`, where only a proven
`Different` lets a caller skip or remove and `CannotProve` always refuses). It
just landed on `main`. T3a was written **before** it and was rejected by a counted
closure with three blockers; two of those three were defects the primitive has
since fixed. **T3a is therefore not a rebuild of the original task — it is a
much smaller residual, and your first job is to establish exactly what that
residual is.**

### What T3a was chartered to deliver

Slice 3d's third task, split into a deciding half (T3a) and an acting half (T3b),
because the combined task was too large to converge in a review cap.

- **T3a DECIDES**: builds the exact-absence proof, the seam it needs, and a
  tri-state refusal, and performs **no record mutation whatsoever**. Its output is
  a typed decision value.
- **T3b ACTS**: consumes that decision under a refusing lock window spanning
  proof→transition→unlink, publishes the `UnusedSettled` transition, performs a
  descriptor-safe removal, and serves the marker population. **T3b is out of scope
  here — do not design it.**

T3a's five chartered deliverables were: (1) a **state-agnostic** exact-absence
proof, one definition serving both the 2b2 marker population and the candidate
population, adding **no edge** to the frozen custody transition table; (2) the
"B18" async/trait recovery seam, with a written justification — the sweep is
**sync** while the registration probe was **async and private**; (3) tri-state
refusal, fail-closed, where a probe `Err` is never read as absence; (4)
**recovery-inventory coupling** — a candidate whose preparation was transferred to
recovery is still owned by a live recovery flight and is **not** provably unused
however absent its target looks, so it must classify cannot-prove → refuse; and
(5) boot-caller wiring for the decision path, still effect-free.

### What is ALREADY on main (measured at `9aedf175`)

The path-identity slice absorbed most of T3a's substrate. In
`crates/bridge-worktree/src/sweep.rs`:

- `ExactAbsenceCandidateV1` with `from_legacy` and `from_claim`, binding **both**
  the source and the `common_dir` identity (this closed one of T3a's blockers).
- `ExactAbsenceObservationV1 { TargetPresent, RegisteredButAbsent, BothAbsent }`
  (~line 159).
- `pub trait ExactAbsenceProbeV1 { fn observe_exact_absence(&self, candidate)
  -> Result<ExactAbsenceObservationV1, BridgeError> }` (~line 164) — a **sync**
  trait over the host capability. This appears to be the B18 seam, already chosen.
- `UnusedCandidateDecisionV1 { Authorized, Refused }` and
  `decide_unused_candidate(candidate, recovery_owned: bool, probe)` (~line 176),
  which refuses on `recovery_owned`, authorizes only on `Ok(BothAbsent)`, and
  refuses on every other `Ok` variant **and on `Err`**.
- Two population entry points: `decide_unused_legacy_sidecar` (~line 488) and
  `decide_unused_custody_record` (~line 505), both funnelling into the one
  `decide_unused_candidate`.
- `sweep_orphans_with_exact_absence(root, probe)` (~line 529), called at ~line 550
  with `HostGitWorktree::new()`.
- `scan_worktree_records` yields `ScannedWorktreeRecordV1 { Legacy(sidecar),
  Custody(record), UnreadableCustody(refusal) }` (~line 318).

Underneath it, `compare_path_identities` in `crates/bridge-core/src/fs_custody.rs`
is the landed tri-state primitive, and `host_git.rs`'s registration probes now
carry three states with `CannotProve` reaching the durable record as
`RegistrationUnproven`.

### The gaps you are designing around (measured, and the reason for this task)

**G1 — the recovery coupling is a parameter that is never populated.**
`decide_unused_candidate` takes `recovery_owned: bool`, and **both** call sites
pass a hardcoded `false`. T2's recovery inventory, `preparation_recovery_flights`,
lives on the backend (`crates/bridge-worktree/src/backend.rs` ~2203, 2272, 2459,
2474, 3820) and is never consulted by the sweep. So deliverable (4) — the reason
T3 was blocked on T2 at all — is structurally absent. It is currently harmless
only because the decision is effect-free; it becomes a fail-open authorization the
moment T3b acts on it.

This is the design's hard part. The inventory is backend state behind a
`StdMutex`; the sweep is a free function that also runs from
`WorktreeRunEndGuard::drop` and at boot. A `Drop` impl cannot await, and the file
notes `run_git_sync` "stays sync (not de-blocked like host_git.rs's run_git)" for
exactly that reason. Whatever seam you choose must not block a runtime worker
thread in a way that can deadlock, must not make the sweep's sync contract a lie,
and must not widen a lock's scope such that the sweep can deadlock against a live
configure/release path holding the same inventory.

**G2 — the 2b2 V3 marker population never reaches the proof.** This was T3a's
surviving closure blocker: the specified marker population is not among the
populations the sweep scans. `ScannedWorktreeRecordV1` has Legacy, Custody, and
UnreadableCustody — no marker variant. Establish from the code what the marker
population actually is, whether it reaches `decide_unused_candidate` by any route,
and what it would take for **one** proof definition to serve it — deliverable (1)
is explicitly that the proof be state-agnostic across both populations, not that
two proofs exist. Note the acting half of marker authority belongs to T3b; T3a
owns only the proof reaching it.

**G3 — the named exit-gate test does not exist.**
`unused_candidate_settles_only_after_exact_absence` is absent from `main`. It is
the slice's named exit gate and must exist in its refusing arms (present-target
refuses; registered-but-absent refuses; probe-failure refuses) plus an
authorized arm asserting an authorized decision **and zero mutation**.

**G4 — is the decision path actually reachable, and is it effect-free?** Confirm
whether `sweep_orphans_with_exact_absence` is on a live path or is currently
unreachable production code, and confirm that no T3a-owned path writes a custody
record. State what you measured.

### Constraints that are not negotiable

- **No record mutation on any T3a path.** T3a decides. If your plan requires a
  write to demonstrate anything, the plan is wrong.
- **No new edge** in the frozen custody transition table
  (`crates/bridge-worktree/src/custody.rs`). `UnusedSettled {}` already exists
  (~line 132) and the table already records that it precedes `git worktree add`
  entirely and may therefore carry a degraded identity (~lines 209-219). Do not
  propose changing those rulings.
- **Fail-closed asymmetry.** A wrong `Authorized` is fail-open and becomes
  destructive under T3b; a wrong `Refused` merely declines. Never propose closing
  a refusal gap by widening authorization without an explicit soundness argument.
- **Out of scope:** T3b's lock window, the `UnusedSettled` publication,
  descriptor-safe removal, marker-removal authority; any change to T1's or T2's
  landed mechanisms; the T2 control-root identity defect (it has its own
  sub-slice); the path-identity primitive itself (it just landed, approved).
- `bridge-core` compiles for Windows in CI while `liveness` and
  `namespace_transaction` are `#[cfg(unix)]`. This lane has lost five landing
  rounds to that boundary. Any item you propose that touches it must say how it is
  gated.

### Sizing is part of the design

This lane's failures have been sizing and contract failures, not coding failures.
T2 took 682 delivered lines, four review rounds, a park, an extension and two
operator completions. The original T3a was 1,106 lines against a 750-line cap. The
path-identity slice was 852 against 700, then took two further repair rounds.

So: **decompose the residual into slices that can converge inside a two-round
review cap**, give each a line budget you believe, and order them so each is
independently landable and independently green. If your honest assessment is that
the residual is one small slice, say that — do not manufacture phases.

### Falsification license

Every anchor, line number and claim above was measured by the operator at
`9aedf175` and may be wrong. The repository is the authority. If
`recovery_owned` is in fact populated somewhere, if the marker population does
reach the proof, if the seam is already resolved, or if a gap listed here does not
exist — **say so plainly with the evidence and drop it from the plan.** Finding
that the residual is smaller than described is a good outcome, not a failure to
deliver. Equally, if you find a gap not listed here, add it.

The one thing not open for redesign is the split itself: T3a decides, T3b acts.

## Acceptance Criteria

A satisfactory design must:

1. **State what is actually left**, gap by gap, each marked confirmed or refuted
   against the code with a file and symbol cited. Refuted gaps are dropped with
   the evidence shown.
2. **Answer G1 concretely**: name the seam that lets the sweep consult
   `preparation_recovery_flights`, and give the deadlock argument — which lock is
   held, by whom, across what call, and why the sweep cannot block a runtime
   worker or contend with a live configure/release path. Name the options rejected
   and why. This is the design's core content; a plan that hand-waves it is not
   acceptable.
3. **Answer G2**: say what the 2b2 marker population is in code terms, whether it
   reaches the proof today, and how one state-agnostic definition serves both
   populations — or argue with evidence that the blocker was mis-stated.
4. **Order the work into independently landable slices**, each with a line budget
   and a stated exit gate, sized to converge within two counted review rounds.
5. **Specify the red-first tests per slice**, including the named exit gate
   `unused_candidate_settles_only_after_exact_absence` with all three refusing arms
   and a zero-mutation authorized arm. For each test say what state it constructs
   and what mutation of the production code it would catch. A test that cannot
   fail on the pre-change tree is not a test.
6. **Identify what only a specific environment can verify.** This lane has shipped
   three tests that looked like evidence and were not; one passed on macOS/APFS and
   on the container's overlayfs and failed only on ubuntu/ext4. Say which proposed
   checks are filesystem- or platform-sensitive and where each must run.
7. **State the fail-open risk of each proposed change** and confirm none widens
   authorization without a soundness argument.
8. **Call out anything that should NOT be built**, including work the primitive
   already did that a naive reading of the original T3a spec would duplicate.

## Spec Refs

Present in the session cwd and worth reading:

- `docs/superpowers/plans/2026-08-17-r2f1b-3d-t3a-task.md` — T3a's original
  charter. **Written against an older tree; treat its "what to build" as
  historical**, since the primitive has since landed much of it.
- `docs/superpowers/reviews/2026-08-17-r2f1b-3d-t3a-sol-closure.md` — the counted
  closure that rejected T3a with three blockers. Two are believed fixed by the
  primitive; the marker-population blocker is G2.
- `crates/bridge-worktree/src/sweep.rs`, `.../host_git.rs`, `.../backend.rs`,
  `.../custody.rs`, `crates/bridge-core/src/fs_custody.rs` — the live code.

Parked T3a artifacts exist as `salvage/r2f1b-3d-t3a-complete` (`b255cba5`, last
host-green) and `salvage/r2f1b-3d-t3a-repair3` (`ad60db53`, parked). They were
built on a pre-primitive tree and carry a divergent copy of the sweep substrate.
**Treat them as reference only** — do not plan to merge or rebase them; a
reconciliation debt is expected and saying so is part of the answer.
