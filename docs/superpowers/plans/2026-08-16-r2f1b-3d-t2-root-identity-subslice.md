# R2f1b 3d — control-root identity binding: design sub-slice (2026-08-16)

Status: DESIGN BRIEF, not dispatched. Raised by the counted Sol re-look on
`435257ce..85658e01` (finding 1, WRONG-BLOCKER). Escalated to design rather
than repaired in-round, per the owner's decision to split the re-look's two
findings.

## Why this is a design sub-slice and not another targeted repair

Finding 1 is the **third distinct defect in control-root / journal-root
handling** across three consecutive review rounds:

| Round | Finding | Shape |
|-------|---------|-------|
| Closure on `c5d9390c` | W3 | the nonreturning INITIAL journal op has no journal to transfer into |
| Re-look on `435257ce` | E2 | the root OPEN is itself the first stall-capable op, stalling with no owner published |
| Re-look on `85658e01` | finding 1 | the pinned root can diverge from the frozen target |

Each round closed its predecessor and surfaced a new instance of the same
kind. That is the open-class signature the convergence discipline names, and
the response it prescribes is escalation to design — not a fourth repair on
the same subsystem inside a consumed cap.

Note for the design pass: the review lens scoped the *fix* as "moderate and
localized to control-root/journal construction." Designed does not mean large.
It means this one gets a written spec and its own review round instead of being
folded into a repair tail.

## The defect

The backend builds its shared control root from the **raw configured spelling**:

- `backend.rs:2194` — `PreparationControlRootV1::new(PathBuf::from(&cfg.root), …)`
- `provider_path.rs:102` — bound validation instead does
  `canonicalize_lenient(&cfg.root)` and freezes a target under that canonical root.

Constructible state (from the re-look, operator-verified at source):

1. Configure `root = /state/current`, where `current -> /vol/a`.
2. Bound validation resolves target `/vol/a/<name>`.
3. Before the lazy open at `backend.rs:574`, retarget `current -> /vol/b`.
4. The control root pins `/vol/b`. The journal keeps only the record basename
   and writes through the pinned root.
5. Custody and provider materialization still derive from the frozen target
   `/vol/a/<name>`, and the served map projects that same path.

Observable result: configure can succeed with the worktree and custody state
under `/vol/a` while `Open` / `Settled` are durably published under `/vol/b`.
A colliding record in `/vol/b` can instead refuse an unrelated `/vol/a` flight.

Trigger requires a symlinked worktree root retargeted during admission —
**rare**, but it does not depend on transfer arming.

## Questions the design pass must settle

1. **What is the authoritative root identity?** The frozen canonical root from
   bound validation is the obvious candidate, since `provider_path.rs:102`
   already forces every bound session to agree with it. Confirm that no
   admitted configuration can legitimately produce two different canonical
   roots for one backend.
2. **When is it pinned?** The pin is deliberately lazy — eager pinning at
   construction would block startup on a stalled filesystem, and the whole E2
   fix depends on publishing the active owner *before* the blocking open. Any
   design that re-eagerises the pin must not reintroduce E2.
3. **How is divergence detected and refused?** `pinned_root()` already calls
   `pinned_root_unchanged(root)`, so identity-stability machinery exists; what
   is missing is binding the *original* pin to the frozen canonical root rather
   than the raw spelling. Decide between (a) pinning the canonical path
   directly, (b) verifying the pinned descriptor's identity against the frozen
   expectation and refusing before first publication, or (c) both.
4. **Ownership ripple.** `PreparationFlightJournalV1` currently holds only
   `control_root` + `record_name`. Carrying an expected root identity changes
   what the journal knows and what it may refuse. Enumerate every caller before
   choosing.
5. **Refusal semantics.** A mismatch must be a typed, effect-free refusal
   before any publication — not a partial write. Confirm it composes with the
   phase CAS (a refusing flight still has to reach a terminal and release its
   reservation).

## Red-first battery the slice must land

- Retarget an admitted root alias from A to B between validation and pinning;
  require either publication beside target A or a typed no-effect refusal, with
  **zero bytes written under B**.
- A stable-alias positive case that still publishes normally (guards against a
  fix that refuses everything).
- A colliding-record case: a record of the same basename already under B must
  not refuse an unrelated flight bound to A.
- Refusal path releases its reservation and reaches a terminal, composed with
  the phase CAS (the finding-2 class: never publish a result before releasing).

## Non-scope

T2's landed mechanisms (E1 phase CAS, E2 owner-before-blocking-op, E3
exact-child lease); s1 abort residue; the slice-4 binding observer obligation;
the per-flight blocking-wait SMELL; the test-harness hang amplifier. Each stays
on its existing ledger line.

## Does this block T2 landing? — ANSWERED: no, the defect is latent

The re-look called finding 1 "production-reachable now." The operator resolved
that against the current tree rather than leaving it open, and the reviewer's
reachability claim does **not** hold at the deployment level:

- `materialize_under_custody` — the preparation flight, its control root, and
  the journal — runs only when a `BoundWorktreeCustodyV1` reaches the backend.
  Absent one, `configure_bound_resolved` returns `WtCustodyV1::Legacy` before
  any flight is claimed (`backend.rs:3686`).
- A `BoundWorktreeCustodyV1` requires an admitted `FrozenR2f1bContractV1`.
- The **only** constructor of `FrozenR2f1bContractV1` is
  `execution_policy.rs:2621`, inside the `#[cfg(test)]` module that begins at
  `execution_policy.rs:2580`. No production path constructs one — matching the
  in-tree statement at `custody_writer.rs:20-27` that the V3 writer is
  "production-unreachable by construction."

So finding 1 cannot execute in production today. What the reviewer established
is that it does not depend on *transfer arming* — correct, and it means the
defect activates the moment the V3 path becomes reachable, not later.

**Consequence for scheduling: this sub-slice is a hard prerequisite of the
slice that makes V3 reachable, not of T2 landing.** T2 may land with finding 1
ledgered against this brief. That is the operator's reading; the owner decides.
