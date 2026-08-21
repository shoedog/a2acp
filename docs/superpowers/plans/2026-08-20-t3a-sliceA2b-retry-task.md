---
task-type: implement
---

# A2b targeted retry — one unreachable assertion, one misclassification

## Description

Your A2b implementation is nearly complete and its structure is accepted. The
public return-type change landed, the report is populated from the production
exact outcome, A1's constructor allowances were consumed, the frozen genuine-red
control exists with its SHA-256 and reproduction command, and the handoff carries
a full evidence table.

Two defects remain. Both are closed and bounded. **Fix only these.** Do not
restructure, do not rewrite production, do not add scenarios.

Your base is your own previous candidate, not `main`.

### Measured starting state

`[MEASURED]` on this task's exact base, pinned 1.94.0 toolchain, run on the host:

| Gate | Result |
|---|---:|
| `cargo fmt --all -- --check` | exit 0 |
| `cargo clippy --workspace --all-targets --locked -- -D warnings` | exit 0 |
| `cargo test -p bridge-worktree --locked` | **exit 101 — 307 passed, 1 failed** |

The single failure is `sweep::tests::exact_absence_sweep_reports_cannot_canonicalize`.

---

## Defect 1 — the test asserts an outcome production cannot produce

`exact_absence_sweep_reports_cannot_canonicalize` builds an **absolute** temp
path, asserts it does not exist, and expects `canonical_root()` to be `None` with
`ExactAbsenceRootRefusalV1::CannotCanonicalize`.

It fails with `left: Some("/private/var/folders/…")`, `right: None`.

**Root cause, measured.** `canonicalize_lenient` walks up to the nearest existing
ancestor, canonicalizes that, and re-appends the missing tail. It returns `Err`
only when it exhausts `file_name()` or `parent()`. The operator transcribed that
loop and ran it:

| Input | Result |
|---|---|
| `/tmp/definitely-does-not-exist-12345` | **Ok** — `/private/tmp/definitely-does-not-exist-12345` |
| `/tmp/a/b/c/d/e/nope` | **Ok** |
| `/definitely-no-such-root-xyz/deep/path` | **Ok** — `/` exists, so the whole tail re-appends |
| `relative-nonexistent` | **Err** |
| `a/b/c-nonexistent` | **Err** |
| `""` | **Err** |

**An absolute path can never produce `CannotCanonicalize`**, however deep or
absent, because `/` always canonicalizes. Only a relative or empty path can.

**The correct pattern already exists in this repository.** The passing test
`exact_route_cannot_canonicalize_without_opening_pin` in
`crates/bridge-worktree/src/sweep/checked_scan.rs` passes `""` as the root for
exactly this reason.

**Fix:** give the test an input that genuinely cannot canonicalize — `""` or a
relative non-existent path — and keep every assertion it already makes. Do not
change production to make the assertion true, and do not weaken the assertion to
match an absolute path's behavior. If you conclude production is wrong here, stop
and report instead of editing it.

Record in the handoff which input the test uses and why an absolute path cannot
exercise this branch. That fact has now cost this lane three separate tests.

---

## Defect 2 — a test that cannot compile on the base is not characterization

The handoff classifies `exact_absence_sweep_reports_cannot_canonicalize` as
**characterization**. It calls `sweep_orphans_with_exact_absence` and reads
`report.requested_root()`, `report.canonical_root()`, `report.scan()`, and
`report.entries()`.

On the untouched base that function returns `()`, so the test **cannot compile**
there. Characterization means the test passes against the untouched base; a test
that cannot even build against it is not characterization, and the handoff's own
definition says so.

**Fix:** re-audit every row of the evidence table against one question — *would
this test compile and run on the untouched base?*

- If it **cannot compile** because it consumes the new return type, classify it
  as genuine runtime red (or compiler-barrier evidence, if you distinguish
  those) and bring it under the frozen base control, extending the control patch
  and its recorded SHA-256 accordingly.
- If it exercises only internal helpers unchanged by A2b — the
  `root_observation_classifier_*` tests appear to be in this group — then
  characterization is correct and it stays out of the control.

The audit matters more than the label. State the compile-or-not answer for every
new test, and make the control cover exactly the set that cannot compile.

If extending the control changes the reproduction command, update it and re-state
the patch's SHA-256.

---

## Everything else is accepted

Do not change: the public signature, report population, the consumed `dead_code`
allowances, the boot callers, the semver record, the F9 possible-versus-
guaranteed distinction, the Unix-only-separator note, or the sizing worksheet
beyond what these two fixes require.

## Handoff and custody

Amend the existing handoff in place; do not start a new one. Update the evidence
table, the control's contents and SHA-256 if they change, and the sizing
worksheet for whatever these fixes add.

**You make the implementation-candidate commit only.** Gate execution and the
handoff-only evidence commit belong to the host operator — this container's
egress cannot fetch the pinned `a2a-lf` dependency, so `cargo` cannot build here.
Do not attempt the gates, do not run `git diff --cached --check`, and do not
fabricate totals. The six `PENDING OPERATOR` lines stay unticked.

## Sizing

| Counted component | Estimate | Cap |
|---|---:|---:|
| Defect 1 — the test's input and any comment it needs | 10 | 25 |
| Defect 2 — control patch extension and classification audit | 45 | 80 |
| Handoff amendments | 25 | 45 |
| **Total** | **80** | **150** |

If a row will exceed its cap, stop and report rather than compressing evidence.

## Acceptance Criteria

1. `exact_absence_sweep_reports_cannot_canonicalize` passes, using an input that
   genuinely cannot canonicalize, with its original assertions intact.
2. No production source is changed to make that test pass.
3. `cargo test -p bridge-worktree` has zero failures at the candidate commit
   (operator-verified).
4. Every new test's evidence classification is re-audited against "would this
   compile on the untouched base," and the answer is stated for each.
5. Every test that cannot compile on the base is covered by the frozen control.
6. The control patch's recorded SHA-256 matches its contents, and the
   reproduction command is accurate.
7. The handoff is amended in place, not replaced.
8. Nothing outside these two defects is changed.
9. The six `PENDING OPERATOR` lines remain unticked and exactly one
   implementation-candidate commit exists.

Do not claim any gate result. Do not tick a pending box.

## Files

- `crates/bridge-worktree/src/sweep.rs` — the failing test's input only.
- `docs/superpowers/reviews/2026-08-20-r2f1b-3d-t3a-inc1-sliceA2b-handoff.md` —
  amend.
- `docs/superpowers/reviews/2026-08-20-r2f1b-3d-t3a-inc1-sliceA2b-genuine-red-control.patch`
  — extend if the audit widens the covered set.
- Everything else — read-only.

## Spec Refs

- `crates/bridge-worktree/src/provider_path.rs` — `canonicalize_lenient`, the
  ancestor-walk that makes absolute paths always canonicalize.
- `crates/bridge-worktree/src/sweep/checked_scan.rs` —
  `exact_route_cannot_canonicalize_without_opening_pin`, the passing test that
  already uses `""`.
- `docs/superpowers/plans/2026-08-20-t3a-sliceA2b-task.md` — the A2b task; its
  scope fences and falsification license still apply.

## Commit Message

fix(worktree): use an uncanonicalizable root and re-audit A2b evidence

Give `exact_absence_sweep_reports_cannot_canonicalize` an input that can actually
refuse: `canonicalize_lenient` walks to the nearest existing ancestor and
re-appends the tail, so an absolute path always canonicalizes and the refusal
branch is unreachable for it.

Re-audit every new test against whether it compiles on the untouched base, and
bring each test that cannot under the frozen genuine-red control.

## Falsification license

The measurements above are operator claims against your base. The repository is
authoritative. If `canonicalize_lenient` does not behave as the table shows, if
the failing test differs, or if the evidence table is already correct, record the
exact repository evidence and stop before editing.
