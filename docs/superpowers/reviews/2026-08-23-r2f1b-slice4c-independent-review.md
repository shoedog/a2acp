## Merged Review — R2f1b slice 4C

**BLOCKER (WRONG) — Configured test command is not green.**
Location: `docs/superpowers/reviews/2026-08-23-r2f1b-slice4c-handoff.md:90-98`
The handoff itself records the configured test command exiting 101, with `bridge-api backend::tests::settlement_refusal_does_not_mask_the_provider_failure` failing. Acceptance criteria explicitly require fmt/clippy/build/test all green; the nondeterminism discussion in the handoff is useful attribution evidence but does not satisfy the stated green gate. Fix: stabilize or isolate the failing test and rerun to green, or obtain an explicit acceptance waiver from the operator before claiming completion.

**MAJOR — Required test #5 (preservation-before-disposition unrepresentability) does not exist in the delivered form.**
Location: `crates/bridge-workflow/tests/r2f1b_slice4c_preservation_ownership.rs:34-52`
The delivered test (`disposition_requires_typed_and_coherent_preservation`) always calls `.after_preservation()` before `.into_disposition()`; it proves a different, pre-existing coherence invariant (`validate_coherence()` rejecting `NotNeeded`+`Preserved`), not that obtaining a disposition before preservation is typed fails or is unrepresentable. The type-state itself does correctly gate `.into_disposition()` behind `PreservationTypedV1`, so the underlying guarantee holds — this is a missing-proof gap, not a broken invariant. The codebase has a precedented convention for exactly this class of claim (trybuild + `compile_fail.rs`, used in `bridge-core`) that was never applied to `bridge-workflow`. Fix: add a trybuild harness to `bridge-workflow` proving `.into_disposition()` without `.after_preservation()` fails to compile.

**MAJOR (disputed, downgraded from Reviewer A's BLOCKER) — Ownerless-unknown cleanup has no public constructor outside `bridge-core`.**
Location: `crates/bridge-core/src/execution_policy.rs:1003-1007`
`UnidentifiableCleanupOwnerProofV1` has a private field and no public constructor; only `bridge-core`'s own module test can build the ownerless-`Unknown` observation. Reviewer A calls this a BLOCKER on the theory that `bridge-workflow`/the multiplexer can't construct it later. Resolution: downgraded to MAJOR — required test #3 only demands the state be *representable*, which it is (proven inside `bridge-core`), and the task explicitly places "wiring the settlement decision into the live cleanup path" out of scope for 4C, deferring integration to 4D–4H. A sealed type with no public constructor is consistent with 4C's "build the seam, test it in isolation" intent, but it's a real forward-looking integration risk worth flagging now. Fix: note for 4D/4H, or add a constrained constructor now if the team wants to de-risk early.

**MINOR — New unit test breaks the file's established test-organization convention.**
Location: `crates/bridge-core/src/execution_policy.rs:1092-1108`
`ownerless_unknown_requires_sealed_unidentifiable_proof` is a bare top-level test; every other test in the file is grouped in a named `mod ..._tests` block. Cosmetic — fold into a module.

Both reviewers produced full, independent reviews (no missing lens). No disagreement on the deadline-cap logic, sole-owner-guard consumption, encoding-stability literals, or untouched-invariant checks (executor.rs, Cargo.lock, refusal gate) — both independently verified these as correct.

VERDICT: REJECT
SUMMARY: Configured test command is not green per the handoff's own record, and required test #5 (preservation-before-disposition unrepresentability) wasn't actually built — both are explicit, unmet acceptance criteria, not style nits.