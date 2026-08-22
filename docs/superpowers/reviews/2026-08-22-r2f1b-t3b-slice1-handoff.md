# T3b slice 1 handoff — refusing settlement window

## Candidate checkpoint

- Implementation base: `cafeae13a67b621f194e4cb82b2fc6765f4e8b4a`; its exact pre-edit tree is `d7ec7095f4d0cd835fdc4ef8d572ab527b74ebeb`.
- This implementation candidate changes only `crates/bridge-worktree/src/settle.rs`, `crates/bridge-worktree/src/lib.rs`, the module documentation in `crates/bridge-worktree/src/custody_lock.rs`, this handoff, and its frozen mutation control. Manifests and `Cargo.lock` are untouched.
- The pre-edit added-Rust count was 0. The formatted candidate adds 415 nonblank physical Rust lines (411 in `settle.rs`, 3 in `custody_lock.rs`, and 1 in `lib.rs`), against the 790-line cap. This leaves 375 lines of headroom.
- No additional verification result is claimed in this handoff. The base control is `bridge-worktree` 331 passed at `cafeae13`; the six operator gates below remain pending.

## Implementation

- `SettlementWindowV1::open` first checks that the root exists, then refuses on the publication cell, pins the root, derives the one-component custody record child, reads it, refuses on the record-derived custody cell, re-reads it, and compares the canonical bytes before accepting the subject path.
- Guard fields are retained in publication-then-custody acquisition order so declaration-order drop releases the custody cell first and the publication cell last.
- The six required focused tests cover both held cells, a writer waiting behind an open window, publication-before-custody order, a changed record, and the bounded no-effect audit. An additional test covers the typed `SubjectNotConstructible` refusal when a decodable custody record's worktree path does not match the requested subject.

## Bounded effect audit

The added production path may reach refusing lock acquisition, directory pinning, descriptor-relative regular-file reads, canonical decoding, allocation, and tracing. It has no edge to rename, unlink, publication, transition, settlement, provider removal, prune, or process spawn. The no-effect test also freezes the production source audit against those forbidden primitive and writer/provider edges.

## Frozen single-mutation control

- Exact reviewed candidate head: commit `1efb3154b63bba538539b61db0723540db489b5e`, tree `f53dc01549b13eb0e35960a326ed330570d186cb`.
  <!-- Operator correction. The candidate commit recorded `efe19ac3`/`99a5ea7b`; both are unreachable.
       Root cause is this lane's spec, not the implementer: the review->tweak loop AMENDS the candidate
       each attempt, so any head sha written inside the handoff is rewritten by the next amend. The
       implementer was asked to record its own final commit from inside that commit, which is impossible.
       The binding belongs in the operator's evidence commit, which is written after the candidate is
       final. Slices 2-5 must assign it there. -->
- Control: `docs/superpowers/reviews/2026-08-22-r2f1b-t3b-slice1-mutation-control.patch`.
- SHA-256 (recomputed and patch-checked against the reviewed head): `97f0f3cecf0a7ede147276f76d931948d983a0add2a8b920f9b0243e1601b1c6`.
- One logical mutation: delete the step-7 byte-equality comparison so the second read is accepted unconditionally.
- The sole test that must redden is `the_window_refuses_a_record_that_changed_between_its_two_reads`.
- The mutation control was not run in this container; its result is recorded under Operator gates below.

```text
git apply docs/superpowers/reviews/2026-08-22-r2f1b-t3b-slice1-mutation-control.patch
CARGO_INCREMENTAL=0 cargo test -p bridge-worktree --locked settle:: -- --nocapture
```

## Operator gates

All run by the operator on the host at `1efb3154`, from a checkout under the owner-approved trusted cwd
root (`/Users/wesleyjinks/code`). Exit status and FAILED counts are authoritative; per-binary `test result:`
lines are not summed, because nested filtered subprocesses double-count them.

- [x] `cargo fmt --all -- --check` — **exit 0**
- [x] `CARGO_INCREMENTAL=0 cargo clippy --workspace --all-targets --locked -- -D warnings` — **exit 0**
- [x] `CARGO_INCREMENTAL=0 cargo test --workspace --locked --no-fail-fast` — **exit 0, 0 FAILED across 91 binaries**
- [x] `CARGO_INCREMENTAL=0 cargo test -p bridge-worktree --locked --no-fail-fast` — **exit 0, 338 passed / 0 failed**
- [x] `cargo run -p a2a-bridge -- validate --repo-hygiene` at the implementation point — **exit 0**
- [x] `cargo run -p a2a-bridge -- validate --repo-hygiene` at the handoff point — **exit 0** (recorded in the evidence commit)

### Same-environment base control

`bridge-worktree` at `cafeae13`, same host, same command: **331 passed / 0 failed**. Candidate: **338 passed / 0 failed**.
Delta **+7**, matching the six required tests plus the `SubjectNotConstructible` test.

### Frozen mutation control — RUN

- SHA-256 recomputed by the operator: `97f0f3cecf0a7ede147276f76d931948d983a0add2a8b920f9b0243e1601b1c6` — **matches the recorded value**.
- Applied to the actual head `1efb3154`: **applies cleanly**.
- Result: **337 passed / 1 failed**. The single reddened test is
  `settle::tests::the_window_refuses_a_record_that_changed_between_its_two_reads` — exactly the named test, and no other.
- Tree restored to `1efb3154` after the run.

### Counted lines — operator recount

Added nonblank physical Rust lines, post-fmt: `settle.rs` 411 + `custody_lock.rs` 3 + `lib.rs` 1 = **415**, against
the **790** cap; **375 lines of headroom**. This confirms the candidate's own figure. (A review round recounted
401; the operator count agrees with the handoff, not that recount. Both are far inside the cap.)

## Adjudication of the round-3 review findings

The run ended REJECT at the review bound of 3. Each finding was probed rather than accepted or waved off.

| Finding | Verdict | Evidence |
|---|---|---|
| **BLOCKER** — frozen-control section names a stale, unreachable commit | **CONFIRMED** | `git cat-file -t` on both `efe19ac3` and `99a5ea7b` returns unreachable. Corrected above. Narrower than reported: the patch *itself* is sound — it applies to the real head and reddens exactly its named test. The defect was the recorded provenance, not the control. |
| **MAJOR** — the `SubjectNotConstructible` check in step 4 is dead | **REFUTED** | Mutation proof: deleting the check reddens exactly one test (`the_window_refuses_a_record_with_a_mismatched_worktree_path`), 337 passed / 1 failed. A dead check cannot redden a test. The check is live. |
| **MAJOR** — red test `bridge-api::backend::tests::settlement_refusal_does_not_mask_the_provider_failure` | **REFUTED — pre-existing flake, not attributable** | Four independent probes: (1) `bridge-api/Cargo.toml` has no dependency edge to `bridge-worktree`; (2) `backend.rs` is byte-identical between `cafeae13` and `1efb3154`; (3) the failure is `Elapsed(())`, a timeout, not an assertion; (4) **decisive** — the candidate tree re-run in the same `a2a-toolchain:latest` image with the same verify command is green on every test, this one included. Same tree, same environment, passes on re-run. |

### Convergence disclosure

Round 1 rejected on two `E0277` build errors plus a fmt failure; round 3 has fmt, clippy and build all at exit 0
and one docs-provenance defect. Findings are fewer and smaller, so this is a converging loop, not an open class.
The one surviving finding is closed and enumerable — it names the input (the recorded sha), the incorrect result
(unreachable object), and a bounded fix (record the real head) — and its root cause is a defect in this lane's
spec rather than in the artifact. It is therefore folded here as a targeted operator repair on the existing
artifact, with no restart and no re-dispatch.
