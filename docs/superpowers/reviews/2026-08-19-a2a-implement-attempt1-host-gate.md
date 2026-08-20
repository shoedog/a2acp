# A2a implement attempt 1 — REJECT, host gate confirms

**Candidate:** `2e4bba41` on `implement/impl-59639-11akzp7i`
**Clone:** `/Users/wesleyjinks/code/.a2a-implement/impl-59639-11akzp7i`
**Base:** `c637e493` · **Loop:** 3 attempts, bound reached · **Container verify:** FAIL at test

## Host gate results

| Gate | Result |
|---|---:|
| `cargo fmt --all -- --check` | **exit 0** |
| `cargo clippy --workspace --all-targets --locked -- -D warnings` | **exit 0**, 0 warnings |
| `cargo test -p bridge-worktree --locked` | **exit 101** — 3 failed |
| `cargo test --workspace --locked --no-fail-fast` | **exit 101** — same 3, no others |

Workspace scale: 75 `Running` binaries + 16 `Doc-tests` suites. Counted that way
deliberately — summing `test result:` lines over-counts here.

**The three failures are the only failures in the entire workspace.** No
collateral breakage.

## Attribution control — and a correction

`[MEASURED]` Same host, same pinned 1.94.0 toolchain, same command, base tree at
`c637e493` (the `fold` worktree): **exit 0**, 284 + 12 + 5 + 1 + 0 passed.

The candidate's lib binary reports 285 passed / 3 failed against the base's 284
passed, so A2a added 4 lib tests, 3 of which fail.

**My first control attempt was invalid and I am recording that.** I ran the base
suite in the main checkout at `ad7aa23a`, the planning branch — which differs
from `c637e493` by 76 files (6,518 insertions, 77,399 deletions) because it
branched from an older point. It reported `exit 0, 61 passed`, and the binary
count did not match the candidate's, which is what exposed it. Green on a
different tree is not a control; the discrepancy in binary counts was the tell.
The valid control above replaces it.

## The three failures — all self-inflicted test bugs, not production defects

1. `exact_route_cannot_canonicalize_without_opening_pin` — `checked_scan.rs:486`
   asserts `Err((None, CannotCanonicalize))`. The chosen path evidently has an
   existing ancestor, so `canonicalize_lenient` succeeds. Retarget at a path with
   no existing ancestor, or assert `CannotEnumerate`.
2. `exact_route_pin_failure_preserves_legacy_and_refuses_custody` —
   `checked_scan.rs:465` indexes `rows[0]` and expects `Legacy`. Row order is not
   pinned. Route through the injected `Script` source, or sort before indexing.
3. `checked_scan_reads_each_selected_name_before_next_and_finishes_once` —
   `checked_scan.rs:282` unwraps `Err(NonCanonical)`. Hand-written custody JSON;
   regenerate via `encode_canonical()`.

Each names its input, its incorrect result, and a bounded fix — **closed and
enumerable**, three distinct single-test bugs.

## What the gate independently validates

- **G5/G6 held.** `clippy -D warnings` is green, which is the exact gate my
  compile probes predicted would red if the discarded-outcome design had shipped
  with `pub(super)` fields. The opaque-accessor resolution works in the real
  crate, not just in my minimal harness.
- **Formatting held.** `cargo fmt --check` exit 0 on landed code — the first
  round in this lane where no hand-split declaration had to be repaired. The
  spec carrying rustfmt's exact bytes appears to have transferred to the code.
- Reviewers confirm the production routing is faithful to the pinned seam: the
  types, `pub(super)` visibility, exactly six field-scoped `dead_code`
  allowances, the action/exact split, the single row-bound tracing call site,
  and `sweep_orphans` untouched.

## Why it is still REJECT — the two non-test blockers

**Missing two-commit custody.** `git log c637e493..2e4bba41` shows exactly one
commit, and no `*sliceA2a*` handoff file exists anywhere in the tree. The entire
17-step interim-handoff protocol — pre-edit checkpoint, source audit, gate
totals, both hygiene-guard runs, provisional/final staged-check disclosure,
worksheet — is absent. That protocol is what rounds 3 and 4 of review were spent
constructing.

**Conformance matrix substantially unwritten.** Roughly 5 tests exist against
~20 named in the spec's evidence table; `sweep.rs` received zero new tests
despite its own worksheet row. Absent: classifier boundary characterization
(including the backslash case), malformed-legacy omission, custody-refusal
variety, non-UTF-8 name retention, non-default root observations,
enumeration-refusal canonical-root retention, action-metadata erasure. And the
decision matrix that finding G3 was specifically added to force is unverified —
no test asserts `TargetPresent`, `RegisteredButAbsent`, or probe `Err` for
either record kind.

## Assessment

The refactor's **structure** landed and is independently gate-validated. What
did not land is the **evidence**: three-quarters of the required matrix, the
decision assertions, and the entire custody protocol.

Diff is 651 insertions across 3 files against a 704-line pre-edit estimate — so
the production work is roughly on-estimate while the test and handoff rows are
largely unspent. That is consistent with a run that built the thing and ran out
of attempts before proving it.

Retry is warranted and should be **targeted**, not a restart: the spec is
converged, the production routing is accepted, and the three test bugs each have
a named fix. The gap is bounded and enumerable.
