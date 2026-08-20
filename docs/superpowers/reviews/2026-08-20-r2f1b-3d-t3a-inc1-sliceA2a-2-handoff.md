# A2a-2 handoff — characterization evidence for the checked-scan engine

## Summary

Ten characterization scenarios deferred from A2a-1 now exist and pass. Tests
only: no production source changed, no dependency added.

**This handoff is operator-authored, and that is a deviation.** The
implementer's own handoff step never ran — the `implement` harness failed at its
commit step and returned before the agent reached it. The work below was
authored by the containerized implementor (`gpt-5.6-terra`, xhigh) and is
applied byte-for-byte from its working tree; everything attested here was
verified by the host operator directly, not relayed from the agent. Where a
claim could only have come from the agent, it is marked as such or omitted.

**Base:** `3963e0560f0ed6310656e36b2ed0438b633176ee` (`main`)
**Implementation commit:** `c682bccf` on `a2a/a2a2-recovered`

## What changed

`crates/bridge-worktree/src/sweep/checked_scan.rs` — 380 insertions, 5
deletions, confined to `#[cfg(test)] mod tests`. The shared `Script` harness
gained a `custody_by_name` map, an `observations` field, and a `script(log,
names)` constructor; ten tests were added.

No other file changed. `Cargo.toml`, `Cargo.lock`, and
`crates/bridge-worktree/Cargo.toml` are untouched — verified by
`git diff --stat` over the base range, which is empty.

## The ten scenarios

| Test | Source | Evidence category |
|---|---|---|
| `checked_scan_classifier_preserves_full_path_precedence_and_boundaries` | scripted | characterization |
| `checked_scan_silently_omits_bad_legacy_and_retains_bad_custody` | scripted | characterization |
| `checked_scan_counts_iterator_errors_and_continues_in_injected_order` | scripted | new-seam mechanism |
| `nondefault_root_observations_survive_exact_without_changing_rows_or_decisions` | scripted | injected-seam mechanism |
| `enumeration_refusal_retains_canonical_root_and_skips_assessment` | scripted | characterization |
| `action_projection_erases_only_action_metadata` | scripted | projection characterization |
| `injected_sources_use_production_action_and_exact_projections` | scripted | routing evidence |
| `injected_sources_prove_action_and_exact_projection_equivalence` | scripted | projection characterization |
| `report_side_pin_failure_uses_post_canonicalization_opener_seam` | real filesystem | seam characterization |
| `compatibility_pin_failure_preserves_legacy_and_refuses_custody` | real filesystem | seam characterization |

None is genuine runtime red, and none is labeled as such. A2a-1 already landed
the production behavior, so these pass on the untouched base by construction.

### Why two tests use a real directory

The two pin-failure tests assert that the opener receives the **canonicalized**
root: each passes a deliberately non-canonical spelling (`root.join(".")`) and
then asserts the opener observed `std::fs::canonicalize(&root)`. Canonicalization
is a filesystem operation, so a scripted source cannot exercise it. Both use
`rows.iter().any(...)` rather than indexing, so neither depends on enumeration
order.

The other eight use the scripted source, which is what makes their order and
iterator-error assertions sound: enumeration order from a real `read_dir` is
unspecified, so an ordering oracle across independent traversals would not be
valid evidence.

### The classifier divergence is characterized, not repaired

`is_custody_record_name` takes a full path and guards the empty-basename case
with `!stem.ends_with('/')` — a Unix-only separator. On a backslash-spelled
path such as `dir\.custody.v1.json` the stem ends in `'\'`, the guard does not
fire, and classification differs from the Unix spelling `dir/.custody.v1.json`.

A2a preserves behavior, so this slice pins the divergence rather than fixing it.
**Open item for a future slice:** decide whether the guard should be
platform-aware. It is a latent platform dependency, not a defect this slice may
silently repair.

## Operator evidence — run on the host

The implement container's egress cannot fetch the pinned `a2a-lf` dependency, so
gates are operator-owned. Measured at `c682bccf`, pinned toolchain 1.94.0
(`rustc 1.94.0 (4a4ef493e 2026-03-02)`, `cargo 1.94.0`, `rustfmt 1.8.0-stable`,
`clippy 0.1.94`):

- [x] `cargo fmt --all -- --check` — **exit 0**
- [x] `CARGO_INCREMENTAL=0 cargo clippy --workspace --all-targets --locked -- -D warnings` — **exit 0**, zero warnings
- [x] `CARGO_INCREMENTAL=0 cargo test --workspace --locked --no-fail-fast` — **exit 0**, 0 failures across **75 test binaries + 16 doc-test suites**
- [x] `CARGO_INCREMENTAL=0 cargo test -p bridge-worktree --locked --no-fail-fast` — **exit 0**; lib **302 passed** / 0 failed, plus 12 / 5 / 2 / 0
- [x] `cargo run -p a2a-bridge -- validate --repo-hygiene` — **exit 0**; 40 tracked artifacts, 8 validated example configs
- [x] `git diff --check origin/main..HEAD` — **exit 0**, no whitespace defects

**Attribution control:** `bridge-worktree` on base `3963e056`, same host and
toolchain, gives 292 passed. This commit gives 302 — **+10, exactly the ten
scenarios**, zero failures either side.

### An inadmissible probe, recorded

The first workspace run reported **23 failures** in
`tests/r3d0_foundation_cli.rs`. That run was performed in a worktree under
`/private/tmp/...`. Those tests validate trusted cwd roots and symlink escapes,
so the location itself caused the failures.

Re-run at the same commit under `/Users/wesleyjinks/code`: **33 passed, 0
failed**, and the full workspace suite exits 0. The 23 failures were an artifact
of where the probe ran, not of this change. The results above are all from the
normal location.

## Limits and disclosures

- **Custody is a single commit, not the two-commit protocol.** The harness
  failure meant the implementer never authored a handoff, so there is no
  implementation-candidate/evidence split to preserve. This document is the
  evidence, committed separately from `c682bccf`.
- **No pre-edit checkpoint exists.** It is an implementer artifact captured
  before the first edit; the agent's record of it was lost with the failed run.
  It cannot be reconstructed honestly after the fact and is not fabricated here.
- **No counted-line worksheet from the implementer.** Measured after the fact:
  380 added lines in one file against a 510 cap. Per-row attribution was the
  implementer's to make and is not invented here.
- These results attest the tree at `c682bccf` only.
- Production root observations remain `Unavailable` and
  `EXACT_ABSENCE_POLICY_READY_V1` remains `false`. This slice changes no
  production behavior.
- The harness defect that stranded this work is recorded at
  `docs/superpowers/reviews/2026-08-20-implement-commit-failed-nothing-staged.md`.

## Sizing

380 added nonblank lines in `checked_scan.rs` against the task's 510 cap. The
handoff row was not spent by the implementer; this operator record replaces it.
