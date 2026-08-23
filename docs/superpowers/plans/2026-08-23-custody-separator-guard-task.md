---
task-type: implement
---

# Separator-neutral custody record classification

## Description

Repair the separator assumptions in `bridge_worktree::custody::is_custody_record_name`. This item has been
carried on the R2f1b ledger since T3a and is now due; T3b slice 3 compounded it by adding a second
forward-slash-only assumption inside the retirement-residue recognizer.

Base: `origin/main` = `5e3d70b2`.

**Provenance:** folded by the operator from two independently authored specs — `gpt-5.6-sol` (effort xhigh)
and `opencode-go/ox-alpha-free` — given byte-identical input. Both verified their anchors; every factual
claim below was additionally re-checked by the operator against the tree.

### Falsification licence — load-bearing anchors only

**Stop and report before editing** if any of these fails:

- `is_custody_record_name` is absent from `crates/bridge-worktree/src/custody.rs`, or its signature is not
  `pub fn is_custody_record_name(path: &str) -> bool`.
- `CUSTODY_RECORD_SUFFIX` is not `".custody.v1.json"`.
- `ChildNameV2::parse_reserved`, `ReservedNameNamespaceV2::RetirementCapture`, or the `.a2a-v2-rtc-` prefix
  is absent from `crates/bridge-core/src/fs_custody.rs`, or their semantics are not
  prefix-strip-plus-single-component-validation.
- The existing test `custody_record_path_is_invisible_to_the_legacy_sidecar_scanner` is absent, or asserts
  classifications other than those described below.
- Any executed behaviour claim in this spec does not hold on the base tree.

**Do NOT stop for immaterial measurement differences** — line numbers, exact diff line counts, or
formatting-only deltas. Only the size cap binds those. If your count differs from the operator's, record
both and continue.

### Verified anchors

`is_custody_record_name` strips `CUSTODY_RECORD_SUFFIX` and then applies **two** forward-slash-only
assumptions:

1. `!stem.ends_with('/')` — guards the empty-basename case (the original carried defect).
2. `stem.rsplit('/').next()` — isolates the terminal segment before testing it against the
   `.a2a-v2-rtc-` retirement-capture namespace (added by T3b slice 3).

The classifier's input is the full lossy joined display path produced by `Path::join(..).to_string_lossy()`
in `sweep/checked_scan.rs` — `classify_record_display`, reached from `scan_checked_rows_with_source`. That
join uses the **platform** separator, so the separator in the input is an encoding artifact of the caller's
platform, not a property of any filesystem object. This is why correct behaviour must be defined per
*basename*, independent of how the directory part was spelled.

`ChildNameV2::from_bytes` deliberately rejects **both** `/` and `\` as `NotOneComponent`, because it
validates a single component; `is_custody_record_name` deliberately receives a full display path.

### Measured ground truth on `5e3d70b2`

| input | `/` spelling | `\` spelling |
|---|---|---|
| `…/rec.custody.v1.json` (ordinary record) | `true` | `true` |
| `…/.a2a-v2-rtc-rec.custody.v1.json` (retirement residue) | `false` | **`true`** |
| `…/.custody.v1.json` (empty stem) | `false` | **`true`** |
| `…/.a2a-v2-stg-rec.custody.v1.json` (staging namespace) | `true` | `true` |
| `…/..custody.v1.json` (dot stem) | `true` | `true` |

The residue row is the consequential one. T3b slice 3 shipped the residue recognizer so that a crash
between capture and unlink cannot leave something a later sweep reads as a live custody record. **Under
backslash spelling that protection does not hold.**

Reachability, stated honestly: CI runs `bridge-store` on Windows only and no supported lane exercises a
backslash sweep root today, so this is **latent**, not live. It remains a correctness gap, and because the
repair changes classification it needs its own review — which is this slice.

## Required behaviour

Treat `/` and `\` as equivalent **lexical** separators for this classifier. These are string-classification
rules and must be testable on every host; they are not conditional on the host's native separator.

| Class | required result, BOTH spellings |
|---|---|
| Ordinary record | `true` |
| Retirement residue | `false` |
| Empty terminal stem | `false` |
| Staging namespace record | `true` |
| Dot stem | `true` |
| Legacy `.meta.json` suffix | `false` |

The forward-slash answer is correct for all three divergent rows, and the backslash answer is the defect:

- an ordinary record has the exact suffix and a non-empty, non-retirement basename, so it stays discoverable;
- retirement capture is recovery-owned intermediate state, not a live record — a protection whose validity
  depends on separator spelling is no protection at all;
- an empty terminal stem names no checkout target and cannot be emitted by `record_file_name`.

**Do NOT over-correct.** The rule is not "reject anything containing a backslash". Forward-slash-spelled
input must keep classifying exactly as it does today; no currently-`false` row may become `true`, and no
currently-`true` row may become `false`.

## Implementation decision

Repair locally in `is_custody_record_name`, via a small **private** helper in the same `custody.rs` module.

After stripping `CUSTODY_RECORD_SUFFIX`, derive **one** terminal segment using both `/` and `\`, and use
that same segment for both the non-empty-target check and the `ChildNameV2` retirement-namespace parsing.
Remove the independent forward-slash-only trailing-separator guard — do not retain two separately
maintained separator decisions, because they express one conceptual rule ("the last path component").

**Do not** use `Path::file_name`, `MAIN_SEPARATOR`, or platform-conditional code. Those preserve
host-dependent behaviour and would prevent the backslash contract from being exercised on non-Windows CI,
which is the only place this repair can actually be regression-tested.

**Do not** move this into `bridge-core`. `ChildNameV2` owns portable single-component validation and
already refuses both separators; giving it full-path parsing semantics would change a cross-crate public
contract for zero additional callers.

## Invariants that must not change

- `LEGAL_CUSTODY_TRANSITIONS_V1` — ten rows, unchanged.
- The stranded-marker rule: no `source` field on the record, no claim on `UnusedSettled`, no transition out
  of it.
- The public signature `pub fn is_custody_record_name(path: &str) -> bool`.
- Nothing that relaxes the retirement-residue protection this slice strengthens.
- Every existing assertion in `custody_record_path_is_invisible_to_the_legacy_sidecar_scanner` keeps its
  current expectation.

## Required tests

Items 2–4 must **fail on the pre-change tree**; verify that item 1's extension does too.

1. **Extend** `custody_record_path_is_invisible_to_the_legacy_sidecar_scanner` with the backslash rows,
   constructing the residue name through `ChildNameV2::from_bytes` + `ChildNameV2::reserved(
   ReservedNameNamespaceV2::RetirementCapture, &target)` rather than hardcoding the prefix literal.
2. `custody_record_name_rejects_retirement_residue_across_separator_spellings`.
3. `custody_record_name_rejects_empty_stem_across_separator_spellings`.
4. A non-divergence guard: ordinary, staging, dot-stem and legacy rows classify identically under both
   spellings, and every existing expectation is unchanged.

## Size

Projection **110** counted lines against a cap of **260**. Counted lines are added nonblank physical Rust
lines after `cargo fmt`; a grep for added nonblank lines already excludes blanks — do not subtract them
again. If the projection will exceed the cap, stop before editing and report a revised estimate.

## Frozen single-mutation control

Freeze it at
`docs/superpowers/reviews/2026-08-23-r2f1b-custody-separator-mutation-control.patch`.

One **production** mutation inside `is_custody_record_name`: revert the dual-separator terminal-segment
extraction to slash-only. It must not alter tests, fixtures or documentation.

Applied to the candidate head it must redden **all three** of tests 1, 2 and 3 simultaneously. **That
multi-test red is expected and required** — one load-bearing production extraction enforces several
independent classification rows, and a control that reddens only one would be evidence of a *weaker*
defence, not a better control. Do not merge, weaken or delete any test to manufacture a single red.

If any of the three stays green, the obligation is not enforced — **stop and report**. If tests beyond
those three redden, **stop and report the actual population** rather than adjusting the control.

Verify the patch applies cleanly, record its SHA-256 in the handoff, run the named tests, inspect the
output, and restore the candidate tree exactly afterwards.

## Handoff

Create `docs/superpowers/reviews/2026-08-23-r2f1b-custody-separator-handoff.md` with the base, the
changed-file list, the counted total against the 260 cap, the control's path and SHA-256, and the named
tests it reddens.

**Do not record this candidate's own head commit or tree sha.** The review loop amends, so any head sha
written inside the handoff is rewritten by the next amend and becomes unreachable. That binding is the
operator's, made in the evidence commit after the candidate is final.

End the handoff with exactly these six unticked lines:

- [ ] `cargo fmt --all -- --check` — **PENDING OPERATOR**
- [ ] `CARGO_INCREMENTAL=0 cargo clippy --workspace --all-targets --locked -- -D warnings` — **PENDING OPERATOR**
- [ ] `CARGO_INCREMENTAL=0 cargo test --workspace --locked --no-fail-fast` — **PENDING OPERATOR**
- [ ] `CARGO_INCREMENTAL=0 cargo test -p bridge-worktree --locked --no-fail-fast` — **PENDING OPERATOR**
- [ ] `cargo run -p a2a-bridge -- validate --repo-hygiene` at the implementation point — **PENDING OPERATOR**
- [ ] `cargo run -p a2a-bridge -- validate --repo-hygiene` at the handoff point — **PENDING OPERATOR**

Operator note, not a defect: `cargo test --workspace` is red at base on the operator's host with 11
pre-existing `bin/a2a-bridge` failures, and that population inflates under parallel load. The operator
compares populations against a same-environment base on an idle machine.

## Acceptance criteria

- [ ] `/` and `\` are treated as equivalent lexical separators, with one shared terminal-segment derivation
      used by both the empty-target check and the retirement-namespace parse.
- [ ] The independent forward-slash-only trailing-separator guard is gone; there is exactly one separator
      decision.
- [ ] No `Path::file_name`, no `MAIN_SEPARATOR`, no platform-conditional code in the classifier.
- [ ] No change to `bridge-core`; the helper is private to `bridge-worktree`.
- [ ] All six behaviour rows classify identically under both spellings, per the required-behaviour table.
- [ ] No currently-`false` row becomes `true` and no currently-`true` row becomes `false`.
- [ ] The public signature is unchanged.
- [ ] Tests 1–4 exist; 2–4 and the item-1 extension fail on the pre-change tree.
- [ ] `LEGAL_CUSTODY_TRANSITIONS_V1` is still ten rows, unchanged.
- [ ] Counted lines stay at or under 260.
- [ ] The frozen control exists at the named path, is SHA-256-recorded, mutates production only, and
      reddens exactly tests 1, 2 and 3.
- [ ] The handoff records no head commit or tree sha for this candidate.
- [ ] `Cargo.lock` and every manifest are untouched.
