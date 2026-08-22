---
task-type: implement
---

# T3b slice 4 repair — one real defect, one spec clarification, two disclosures

## Description

The slice 4 candidate reached **verify PASS on all four commands** and is substantially correct. It is
rejected on one real test-isolation defect plus one finding that is an operator spec-ambiguity rather than an
artifact defect. Repair on the existing artifact. **Do not redesign, do not restart, do not re-scope.**

Base: `refs/t3b/slice4-candidate` = `e9c0d4c5`, whose parent is `origin/main` = `c343e563`.

Confirmed correct by operator probe and not to be disturbed: 748 counted lines under the 790 cap;
`replace_unused_settled` calls `reprove_under_window` and is therefore the first production caller of the
re-prove gate; the `CustodyRetirementResidue` operator-visible category exists in `storage_report.rs` and
`MarkerRetirementOutcomeV1::CapturedRetained` is handled; the frozen transition table is untouched.

### Falsification license — scoped to load-bearing anchors

**Stop and report** if a *load-bearing* anchor is false: a named symbol is absent, a visibility or signature
differs from what is stated, a described behaviour does not hold, or a requirement cannot be satisfied as
written. That behaviour is wanted, not penalised — an earlier dispatch of this slice correctly refused when
the task demanded something impossible.

**Do not stop for immaterial measurement differences.** Counts, sizes and totals in this task are the
operator's advisory measurements, not contracts. Only the **cap** is binding. If your measurement of a count
differs from the operator's, record both numbers in the handoff and continue; a difference that does not
cross the cap is not a falsified anchor.

This scoping is itself a correction: a previous dispatch stopped because it measured 747 added lines where
the operator stated 748. Both are far under the cap and the difference changes nothing.

## Defect 1 — the test-isolation race (the only code defect)

`crates/bridge-worktree/src/custody_writer.rs` uses a **process-global** static for crash-injection:

```rust
#[cfg(test)]
static INTERRUPT_UNUSED_SETTLEMENT_AFTER_TRANSITION: AtomicBool = AtomicBool::new(false);
```

It is `#[cfg(test)]` with a `#[cfg(not(test))]` counterpart returning `false`, so **production is unaffected**
— this is a test-isolation defect only. But Rust's harness runs tests in parallel threads on one process, so:

- a concurrently running test can **consume another test's arming**, making the armed test miss its interrupt
  path and the unarmed test take one it never asked for; and
- `arm_unused_settlement_interruption_for_test` asserts `!swap(true, ..)`, so two tests arming concurrently
  **panics outright**.

This makes the crash-ordering test nondeterministic — and that test is the guard on the stranded-marker
property, which is the one guarantee in this slice that must not be flaky.

**Fix it with a thread-local, following this repository's own precedent.** `bin/a2a-bridge/src/compatibility_schedule_state.rs`
uses a thread-local for exactly this purpose and its comment states the reasoning: the guards' drops always
run on the arming test's own thread. Match that pattern. Keep the "already armed" assertion, which is correct
once the state is per-thread.

## Not a defect — driving integration IS correctly deferred

Review raised an unmet-intent BLOCKER: that this slice's acceptance criteria require wiring the first
production caller of the settlement path while the handoff says integration is deferred.

**The handoff is right and the finding is an operator spec-ambiguity.** The task said this slice "introduces
the first production caller", meaning the first caller of `reprove_under_window` — which
`replace_unused_settled` is, verified by the operator. It did **not** mean boot integration. The lane plan
assigns that to slice 5 explicitly: *"`sweep_orphans` stops discarding the report and drives settlement"*,
together with `sweep_orphans_async` and the five call sites.

**Do not wire `sweep_orphans` or any boot caller in this slice.** Doing so would pull slice 5's scope into the
destructive slice. Keep the deferral sentence in the handoff and make it unambiguous by naming slice 5 as the
owner of driving integration, so a later reader does not re-raise this.

## Disclose, do not remove — two out-of-scope test-only changes

The candidate touches two files outside the stated boundary. The operator inspected both, judges both
legitimate, and wants them **kept and disclosed** rather than reverted:

1. `bin/a2a-bridge/src/compatibility_schedule_state.rs` — releases the real descriptor before reporting the
   synthetic failure, so a test panic cannot retain the process-local lock. This is a **root-cause fix for the
   `force_next_release_failure_for` flake** that this lane has carried with an unproven mechanism for months.
   The change is inside the test-armed branch, so the production path is unaffected.
2. `bin/a2a-bridge/src/compatibility_resolution.rs` — extends a wide stress fixture's deadline because it
   competes with the suite's CPU-bound tests. Test-only, and the same load-margin class this repository has
   hardened before.

Add a short **"Out-of-scope changes"** section to the handoff naming both files, why each was needed, and the
evidence that each is test-only. An undisclosed out-of-scope edit in the slice that deletes things is the
problem; a disclosed and justified one is not.

## Size

A repair. Expect well under **30** added nonblank Rust lines on top of `e9c0d4c5`.

**The binding constraint is the cumulative cap of 790.** The operator measured the pre-repair candidate at
**748** added nonblank Rust lines against `c343e563`, by two independent methods, with zero whitespace-only
added lines; an implementer measurement of 747 was also reported. Treat any figure in this range as
equivalent — roughly **40 lines** of headroom remain. Report your own measured total in the handoff. Stop
only if the repair would cross **790**.

## Frozen control

If the repair changes any line the control at
`docs/superpowers/reviews/2026-08-22-r2f1b-t3b-slice4-mutation-control.patch` depends on, re-cut it and record
the new SHA-256; otherwise carry it unchanged and say so explicitly.

## Handoff

Update `docs/superpowers/reviews/2026-08-22-r2f1b-t3b-slice4-handoff.md` in place: the repaired file list, the
new counted total against 790, the thread-local change, the clarified slice-5 deferral, the new
"Out-of-scope changes" section, and the control's disposition.

**Do not record this candidate's own head commit or tree sha.** The review loop amends, so any head sha
written inside the handoff is rewritten by the next amend and becomes unreachable. That binding is the
operator's, made in the evidence commit after the candidate is final.

Keep the six operator gate lines unticked and exactly as they are:

- [ ] `cargo fmt --all -- --check` — **PENDING OPERATOR**
- [ ] `CARGO_INCREMENTAL=0 cargo clippy --workspace --all-targets --locked -- -D warnings` — **PENDING OPERATOR**
- [ ] `CARGO_INCREMENTAL=0 cargo test --workspace --locked --no-fail-fast` — **PENDING OPERATOR**
- [ ] `CARGO_INCREMENTAL=0 cargo test -p bridge-worktree --locked --no-fail-fast` — **PENDING OPERATOR**
- [ ] `cargo run -p a2a-bridge -- validate --repo-hygiene` at the implementation point — **PENDING OPERATOR**
- [ ] `cargo run -p a2a-bridge -- validate --repo-hygiene` at the handoff point — **PENDING OPERATOR**

Operator note, not a defect: `cargo test --workspace` is red at base on the operator's host with 11
pre-existing `bin/a2a-bridge` failures. The operator compares populations rather than attributing them.

## Acceptance criteria

- [ ] The crash-injection state is **thread-local**, not a process-global static, and the "already armed"
      assertion is retained.
- [ ] Both required tests that use it pass, and neither can be perturbed by a concurrently running test.
- [ ] No boot caller or `sweep_orphans` wiring is added; the handoff names slice 5 as the owner of driving
      integration.
- [ ] The handoff has an "Out-of-scope changes" section covering both `bin/` files with test-only evidence.
- [ ] `LEGAL_CUSTODY_TRANSITIONS_V1` is still ten rows and unchanged.
- [ ] Cumulative counted lines stay at or under 790.
- [ ] The handoff records no head commit or tree sha for this candidate.
- [ ] `Cargo.lock` and every manifest remain untouched.
