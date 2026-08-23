---
task-type: implement
---

# T3b slice 5B repair — re-cut the frozen control against production

## Description

The 5B candidate is otherwise correct: verify PASS on all four, both reviewers APPROVE, the readiness flip
is one line, and `readiness_true_still_refuses_a_stale_entry` is sound — it takes an authoritative report,
changes the world by creating the target, calls the real production entrypoint
`WorktreeCustodianV1::replace_unused_settled_with_probe`, and asserts `Refused`.

**One thing must change: the frozen control.** Repair only that. Do not alter the readiness flip, the test,
or any other production code.

Base: `refs/t3b/slice5b-candidate` = `5eb50005`, whose parent is `origin/main` = `d6b3bb4d`.

### Falsification license — scoped to load-bearing anchors

Stop and report if a load-bearing anchor is false. Do not stop for immaterial measurement differences; only
the cap binds.

## The defect

`docs/superpowers/reviews/2026-08-23-r2f1b-t3b-slice5b-readiness-control.patch` mutates a **test-only**
helper: inside `mod tests`, `CurrentAbsenceProbeV1::observe_exact_absence` changes `.exists()` to
`.is_file()`.

That reddens the test, but it proves only that the test notices a **misbehaving fixture**. It does not
prove the **production re-prove obligation** is enforced. Both reviewers raised this as a MAJOR
methodology concern, and the operator agrees: it fails this slice's own acceptance criterion, which
required a mutation "chosen so that removing it defeats the re-prove obligation — for example, accepting
the report entry's authority without re-proving under the window."

This matters more here than in any previous slice. **This commit is the arming point.** Its control is the
evidence that arming did not silently convert the report from historical evidence into authority.

## The fix

Re-cut the frozen control so its single mutation lands in **production** code on the settlement path, such
that removing it defeats the re-prove obligation. Choose the mutation yourself from what the code actually
supports — candidates include skipping the byte-identity comparison between the held record and the
re-read, or accepting the report entry's decision without re-proving under the held window.

Requirements:

- The mutation must be in production code, not in `mod tests` and not in a test helper.
- It must redden **exactly one** test, and that test must be `readiness_true_still_refuses_a_stale_entry`.
- Record the new SHA-256 in the handoff, replacing the old one.
- Verify the patch applies cleanly to this candidate's head before recording it.

If no production mutation can redden exactly that test — for example because the obligation is enforced in
more than one place and removing any single one is caught by a different test — **stop and report** which
mutations you tried and what each reddened. That is a real finding about where the obligation lives, and it
is more valuable than a control that technically satisfies the letter of the criterion.

## Size

A control re-cut plus a handoff edit. Expect well under **20** added nonblank Rust lines; the production
diff should be zero. Cumulative cap for the slice is 200.

## Handoff

Update `docs/superpowers/reviews/2026-08-23-r2f1b-t3b-slice5b-handoff.md` in place: the new control path
disposition, the new SHA-256, the named single reddening test, and one sentence recording that the previous
control mutated a test fixture and was replaced because it did not exercise the production obligation.

**Do not record this candidate's own head commit or tree sha.**

Keep the six operator gate lines unticked and exactly as they are:

- [ ] `cargo fmt --all -- --check` — **PENDING OPERATOR**
- [ ] `CARGO_INCREMENTAL=0 cargo clippy --workspace --all-targets --locked -- -D warnings` — **PENDING OPERATOR**
- [ ] `CARGO_INCREMENTAL=0 cargo test --workspace --locked --no-fail-fast` — **PENDING OPERATOR**
- [ ] `CARGO_INCREMENTAL=0 cargo test -p bridge-worktree --locked --no-fail-fast` — **PENDING OPERATOR**
- [ ] `cargo run -p a2a-bridge -- validate --repo-hygiene` at the implementation point — **PENDING OPERATOR**
- [ ] `cargo run -p a2a-bridge -- validate --repo-hygiene` at the handoff point — **PENDING OPERATOR**

## Acceptance criteria

- [ ] The frozen control's mutation is in production code, not a test or test helper.
- [ ] Applied to this candidate's head it reddens exactly one test, and that test is
      `readiness_true_still_refuses_a_stale_entry`.
- [ ] The handoff records the new SHA-256 and why the previous control was replaced.
- [ ] `EXACT_ABSENCE_POLICY_READY_V1` is still `true` and the readiness flip is otherwise unchanged.
- [ ] `readiness_true_still_refuses_a_stale_entry` is unchanged.
- [ ] `LEGAL_CUSTODY_TRANSITIONS_V1` is still ten rows, unchanged.
- [ ] `Cargo.lock` and every manifest remain untouched.
