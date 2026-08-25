# R2f1b slice 4H-1 handoff — wiring residuals without executor changes

Date: 2026-08-25

## What changed

- Widened the seven mechanical-impossibility observation/classifier items required by
  bridge-workflow from crate-public to public. ProducerFinalRouteObservationV1 fields are public as
  well so an external workflow caller can actually supply the route observation. The returned
  MechanicalImpossibilityProofV1 remains sealed.
- Item 5 was withdrawn by the operator. The attempted UnidentifiableCleanupOwnerProofV1 constructor,
  its proof-only CleanupDeadlineUnknownEvidenceV1 plumbing, and its dedicated tests were removed.
  The proof type again has no public constructor.
- Made post-cutoff completion treatment explicit and auditable in the pure arbitration result.
- Made delayed no-progress polling explicit by recording how many due warning ordinals the emitted
  highest ordinal supersedes.
- Added integration and compile-fail coverage for the surviving items 1–4 and reasserted that
  production readiness remains Disarmed and production activation remains ManualOnlyR2f1a.

## Boundary decisions

### Completion after the exact cutoff

A completion at the exact cutoff remains inclusive and drains. A completion observed strictly after
the cutoff is excluded from ready_node_completions, recorded in the deterministically sorted
post_cutoff_completions audit field, and its node remains in nodes_to_cancel_after_winner. The node
was unfinished at the authoritative cutoff; admitting its later completion would make the boundary
dependent on polling delay, while silently dropping it would lose useful attribution evidence.

### First warning poll at 65 minutes

The caller-clocked warning interval remains 30 minutes. A first poll at 65 minutes emits one warning
at ordinal 2 and records superseded_ordinal_count = 1. It does not emit an ordinal-1 catch-up burst.
One warning per poll keeps the warning path bounded and avoids turning a delayed observer into
backpressure; the ordinal and explicit superseded count preserve the elapsed-cadence evidence.

### Withdrawn ownerless cleanup proof

The operator withdrew item 5 because a validate-a-caller-supplied-value shape cannot establish
provenance: any value a caller can obtain can be replayed or fabricated through another public path.
The attempted constructor and CleanupDeadlineUnknownEvidenceV1 existed only for that requirement, so
both were removed. The follow-up will define a minting contract instead of inspecting a supplied
CleanupDeadlineTransferV1 value.

UnidentifiableCleanupOwnerProofV1 therefore still has no public constructor, exactly as on the base
tree. bridge-workflow cannot yet construct the ownerless-Unknown observation. That is a known
sequencing constraint for 4H-2, not an oversight.

## Trybuild sealing regeneration

The mechanical-impossibility sealing stderr was regenerated with TRYBUILD=overwrite and then checked
again in ordinary compile-fail mode. All seven former E0603 visibility errors disappeared. The three
intended sealing diagnostics survived exactly:

- E0599: MechanicalImpossibilityProofV1 has no default constructor.
- Plain compiler error: struct-literal construction is refused because kind is private.
- E0277: From<bool> is not implemented.

The item-5-only cleanup-owner compile-fail fixture and runtime test were deleted with the withdrawn
constructor work.

## Red-first evidence

The required targets were run against an isolated archive of the pre-change tree with their own
Cargo target directories.

- The bridge-workflow wiring target failed with the seven E0603 visibility errors.
- The cutoff test failed with E0609 because post_cutoff_completions did not exist.
- The delayed-warning test failed with E0609 because superseded_ordinal_count did not exist.
- The regenerated sealing fixture mismatched because the pre-change compiler still emitted the
  seven E0603 errors in addition to the three sealing errors.

The item-5-specific constructor and forgery evidence is withdrawn with the requirement and is not
used as acceptance evidence. No production refusal was relaxed: the surviving wiring test
independently reasserts Disarmed and ManualOnlyR2f1a.

## Frozen mutation control

- Patch:
  docs/superpowers/reviews/2026-08-24-r2f1b-slice4h1-mutation-control.patch
- SHA-256: 932d35749f4babf6bfa632115891dd4deeaed6b3ef5414e10adfad3132cebae5
- Production mutation: widen the inclusive completion boundary by 1 ms with saturating_add(1), so
  a completion strictly after cutoff is incorrectly drained instead of audited and cancelled.
- Applicability: git apply --check passed before the control. After the run the patch was reversed,
  and git apply --check passed again on the restored candidate.
- The exact production scheduler source was compiled in a dependency-free harness with the focused
  completion_strictly_after_cutoff_is_dropped_and_its_node_is_cancelled assertion: candidate 1/0,
  mutant 0/1. The mutant admitted ("after", 51) into ready_node_completions instead of retaining it
  in post_cutoff_completions and nodes_to_cancel_after_winner.
- The patch is hand-authored and frozen against the surviving item-3 decision.

## Verification

Before item 5 was withdrawn, the original 4H-1 candidate used the prescribed populated offline Cargo
environment with CARGO_INCREMENTAL=0 and localhost excluded from the injected proxy for aggregate
verification. Those exact-tree results were:

- cargo fmt --all -- --check — green.
- git diff --check — green.
- cargo check --workspace --locked — green.
- cargo clippy --all-targets --all-features --locked -- -D warnings — green.
- cargo build --locked — green.
- The exact configured workspace command from examples/a2a-bridge.containerized.toml
  (workspace, locked, no-fail-fast, excluding bridge-container, with the three configured process
  test skips) — green on the pre-withdrawal candidate when serialized with CARGO_BUILD_JOBS=1,
  including doc tests.
- Focused results: progress epochs 10/10; scheduler arbitration 9/9; workflow wiring 1/1; bridge-core
  and bridge-workflow compile-fail harnesses 1/1 each.
- cargo run -p a2a-bridge --locked -- validate --repo-hygiene — green
  (40 tracked artifacts and 8 example configs).

Post-withdrawal repair evidence:

- cargo fmt --all -- --check and git diff --check — green.
- Every item-5-only source/test path is byte-identical to the declared base; the two dedicated new
  tests are absent on both trees.
- Exact reference search finds no constructor, evidence type, or item-5-only test outside this
  withdrawal record.
- The retained exact-branch scheduler and wiring binaries pass 9/0 and 1/0 respectively.
- The replacement dependency-free mutation harness is candidate 1/0 and mutant 0/1.
- The unchanged built hygiene validator passes with 40 tracked artifacts and 8 example configs.
- executor.rs remains byte-identical to the base at the SHA-256 recorded below.

### Diagnostic exclusions

- A preliminary focused command omitted the required offline Cargo environment and stopped before
  compilation at the blocked crates.io proxy (CONNECT 403). It is not test evidence.
- The first configured candidate aggregate attempt reported one failing a2a-bridge binary target
  after console truncation lost the individual test name. An immediate isolated rerun passed all
  1,097 tests, and the final exact configured workspace rerun was green. The first attempt is
  retained only as intermittent diagnostic evidence.
- The first candidate clippy attempt encountered unrelated E0463 dependency-artifact lookup errors
  in bridge-a2a-inbound/tests/golden_wire.rs after dependencies had checked. The identical command
  immediately passed on the pre-withdrawal candidate. The failed invocation is excluded as
  compiler-cache diagnostics rather than a lint result.
- The first repaired-candidate aggregate attempt failed while linking bridge-core because ld could
  not allocate memory. The bridge-core target then passed alone, and the final serialized full
  population was green; the linker failure is excluded as infrastructure diagnostics.
- In this post-withdrawal repair shell, the mandated locked Clippy and configured workspace test
  commands refused before compilation because the local Cargo registry lacks arc-swap and no
  configured verifier cache or container runtime is present. The checked-in proxy returned CONNECT
  403 and direct access had no DNS. These are dependency-resolution failures, not green gate results;
  the bridge verifier must rerun both commands in its populated verification environment before
  approval.

No live provider turn, host smoke, or fallback-plan action was run.

## Frozen invariants and exclusions

- crates/bridge-workflow/src/executor.rs base SHA-256:
  def9c4fc6dc174f7d744ef2554df4f428550a84725ee71129c7ff7127be684d4.
- crates/bridge-workflow/src/executor.rs candidate SHA-256:
  def9c4fc6dc174f7d744ef2554df4f428550a84725ee71129c7ff7127be684d4.
- No executor loop, timer, wait, select, sleep, spawn, cancellation token/effect, persistence effect,
  or terminal route was added.
- Scheduler readiness remains Disarmed; AutomaticR2f1b remains unavailable in production.
- MechanicalImpossibilityProofV1 remains sealed against direct construction.
- UnidentifiableCleanupOwnerProofV1 has no public constructor. bridge-workflow cannot yet construct
  the ownerless-Unknown observation; that remains the known 4H-2 sequencing constraint.
- MAX_WORKTREE_CONFIGURES_IN_FLIGHT, all manifests, Cargo.lock, and fixed policy constants are
  unchanged.

## Size

Added nonblank physical Rust lines after formatting, measured from the task's declared base:
**108 / 300**. Documentation, generated stderr, and the frozen control patch are excluded from this
cap.
