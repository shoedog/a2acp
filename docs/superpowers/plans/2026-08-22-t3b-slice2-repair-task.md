---
task-type: implement
---

# T3b slice 2 repair — fix the no-effect audit and amend its contract

## Description

The slice 2 candidate is sound in design and reached exit 0 on fmt, clippy and build. It is rejected on one
real test failure plus one contract defect that belongs to the operator, not to the implementation. Repair
both on the existing artifact. **Do not redesign, do not restart, do not re-litigate the accepted design.**

Base: `refs/t3b/slice2-candidate` = `37931561`, whose parent is `origin/main` = `c65c8eca`.

Confirmed clean by review and not to be disturbed: counted lines 697 under the 770 cap; the single
`pub(crate)` seam is the only widened visibility and routes through the existing scan and projection;
`ProvenSettlementV1` has no public constructor and owns its window; `LEGAL_CUSTODY_TRANSITIONS_V1`,
`Cargo.lock` and every manifest untouched; the handoff correctly omits the head and tree sha.

### Falsification license

Every claim below is a tripwire. If an anchor is false, **stop and report** rather than adapting around it.

## Repair 1 — the red test (this is the gate blocker)

`settle::tests::the_reproof_mints_no_effect` panics at the `source_slice` end-anchor `unwrap`.

Cause, verified by the operator: `checked_scan_source` is built by splitting `include_str!("sweep/checked_scan.rs")`
on `#[cfg(test)]` and taking the first part, so the resulting string **contains no `#[cfg(test)]`**. It is then
passed to `source_slice(..., "fn scan_checked_rows_with_source", "#[cfg(test)]")`, whose end anchor can never
match, so `split_once` returns `None` and the unwrap panics.

The operator enumerated the complete anchor population for this audit — all six start/end pairs — and **exactly
one is broken**: this one. The other five resolve. Fix only this pair; do not rewrite the audit's other slices.

Fix it so the slice runs to the end of the already-truncated source rather than searching for a marker that was
removed. Prefer a form that cannot panic: this gate must **fail with a message naming the missing anchor**, never
unwrap. Convert the other `unwrap` sites in `source_slice` to the same non-panicking, message-bearing failure so a
future anchor drift reports which anchor moved instead of a bare `Option::unwrap()` panic.

## Repair 2 — the contract amendment (operator defect, fix the audit's claim)

Review raised that the re-prove seam's effect-free guarantee is bypassable through the production
`ExactAbsenceProbeV1` implementation, `HostGitWorktree`, which spawns `git rev-parse` and which the text-slice
audit does not cover. **The mechanism is correct and the operator accepts it.** The root cause is a contradiction
in the slice 2 task, which required routing through the existing scan and projection — machinery that takes a
probe — while also forbidding any process-spawn edge. With the production probe those cannot both hold.

The design is not at fault. Taking `probe: &dyn ExactAbsenceProbeV1` as a parameter is correct: it keeps the
acting path identical to the reporting path, which is the whole point of this slice. Forcing a divergent
probe-free path would reintroduce exactly the drift this boundary exists to prevent.

Amend the audit's **claim**, not the design:

- The forbidden-edge list this slice enforces covers **mutating** effects: rename, unlink, publication,
  transition, settlement, provider removal and prune. Keep every one of those.
- Replace the blanket process-spawn ban with the accurate, checkable claim: **the added code originates no
  process spawn**, and any spawn reachable from this path can only arrive through a caller-supplied probe.
- Add an assertion that `settle.rs` outside its test module **constructs no `ExactAbsenceProbeV1`
  implementation** — the probe is always caller-supplied, never minted here.
- State in the audit's doc comment that effect-freedom is **conditional on the probe the caller supplies**, and
  that no production caller exists at this slice.

## Repair 3 — the missing doc comment

Required test #4, `a_non_authoritative_scan_refuses`, has no `///` comment naming the production mutation it
catches, unlike its siblings. Add one.

## Carried forward — do not attempt here

Record in the handoff, under a heading `Carried to slice 5`, that when `sweep_orphans` is wired to drive
settlement, the caller **must** supply a read-only probe, and that wiring a spawning probe into a settlement
path is a slice-5 blocker. This slice adds no caller and must not add one.

## Size

This is a repair. Expect well under **80** added nonblank Rust lines on top of `37931561`. The 770 cap applies
to the cumulative slice; the candidate is at 697, so the repair has **73 lines** of headroom. If the repair
cannot fit, stop and report before editing.

## Frozen control

The existing control at `docs/superpowers/reviews/2026-08-22-r2f1b-t3b-slice2-mutation-control.patch` was
verified by review to hash correctly and redden a single test. If the repair changes any line it depends on,
re-cut it and record the new SHA-256; otherwise carry it unchanged and say so explicitly.

## Handoff

Update `docs/superpowers/reviews/2026-08-22-r2f1b-t3b-slice2-handoff.md` in place: the repaired file list, the
new counted total against 770, the amended effect-audit claim, the `Carried to slice 5` note, and the control's
disposition.

**Do not record this candidate's own head commit or tree sha.** The review loop amends, so any head sha written
inside the handoff is rewritten by the next amend and becomes unreachable. That binding is the operator's, made
in the evidence commit after the candidate is final.

Keep the six operator gate lines unticked and exactly as they are:

- [ ] `cargo fmt --all -- --check` — **PENDING OPERATOR**
- [ ] `CARGO_INCREMENTAL=0 cargo clippy --workspace --all-targets --locked -- -D warnings` — **PENDING OPERATOR**
- [ ] `CARGO_INCREMENTAL=0 cargo test --workspace --locked --no-fail-fast` — **PENDING OPERATOR**
- [ ] `CARGO_INCREMENTAL=0 cargo test -p bridge-worktree --locked --no-fail-fast` — **PENDING OPERATOR**
- [ ] `cargo run -p a2a-bridge -- validate --repo-hygiene` at the implementation point — **PENDING OPERATOR**
- [ ] `cargo run -p a2a-bridge -- validate --repo-hygiene` at the handoff point — **PENDING OPERATOR**

The base control is `bridge-worktree` **338 passed** at `c65c8eca`; the candidate before repair is **346 passed,
1 failed**. After repair the expectation is **347 passed, 0 failed** — 338 plus this slice's eight tests, with the
previously-red audit passing. Report the actual numbers rather than these if they differ.

## Acceptance criteria

- [ ] `the_reproof_mints_no_effect` passes, and `source_slice` reports a named missing anchor instead of panicking.
- [ ] The audit still forbids rename, unlink, publication, transition, settlement, provider removal and prune.
- [ ] The audit asserts that `settle.rs` outside tests constructs no `ExactAbsenceProbeV1` implementation.
- [ ] The audit's doc comment states that effect-freedom is conditional on the caller-supplied probe.
- [ ] `a_non_authoritative_scan_refuses` has a `///` comment naming the mutation it catches.
- [ ] The handoff carries the `Carried to slice 5` read-only-probe obligation.
- [ ] No production caller of `reprove_under_window` is added.
- [ ] Cumulative counted lines stay at or under 770.
- [ ] `LEGAL_CUSTODY_TRANSITIONS_V1`, `Cargo.lock` and all manifests remain untouched.
