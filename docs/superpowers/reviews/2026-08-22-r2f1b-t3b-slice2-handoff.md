# T3b slice 2 handoff — re-prove gate

## Candidate scope

- Base: `c65c8eca`.
- Changed files: `crates/bridge-worktree/src/settle.rs`, `crates/bridge-worktree/src/sweep.rs`, `crates/bridge-worktree/src/sweep/checked_scan.rs`, `crates/bridge-worktree/src/sweep/report.rs`, this handoff, and the frozen mutation control.
- This repair changes `crates/bridge-worktree/src/settle.rs` and this handoff only; the frozen mutation control is carried unchanged.
- `Cargo.lock` and every manifest are untouched.
- Post-format, this slice adds 711 nonblank physical Rust lines against the 770-line cap.
- No full verification result is claimed here. The local focused build could not resolve the workspace's uncached registry packages, so the required gates remain operator-owned.

## Re-prove boundary

`settle::reprove_under_window` is public, consumes `SettlementWindowV1`, rejects a non-authoritative, non-member, legacy, or non-authorized entry, and requires the held record to be `ProtectionPrepared` with a claim. It then calls the only new `pub(crate)` sweep seam, which reopens and reprojects only the selected enumerated record through the report scan path. The seam returns `Authorized`, `Refused`, or `CannotProve`, never a report.

`ExactAbsenceSweepEntryV1` retains canonical custody bytes privately. Scan projection retains private authority/observation provenance so a fresh claim-authority or exact-absence probe failure returns `CannotProve` before report/held byte comparison; otherwise the seam compares the report entry, newly scanned entry, and held record, refusing on any byte mismatch. `ProvenSettlementV1` has no public constructor and owns the window, retaining both refusing cells until dropped. `SettlementProofRefusalV1` keeps proved refusal distinct from `CannotProve`.

## Focused coverage and effect audit

The colocated tests cover a stale report with a recreated target, a decodable replacement record, root-level unavailable current scan evidence, row-level claim-authority unavailability, non-authoritative scans, legacy entries, wrong state/bare claim population, proof-held lock ownership, and a bounded no-effect audit.

The added production route may acquire the existing refusing locks, pin/reopen directories, make descriptor-relative regular-file reads, canonically decode, run the existing scan/projection seam, allocate, and trace. The audit freezes the reproof seam and each transitive projection/assessment/checked-scan helper against rename, unlink, publication, transition, settlement effect, provider removal, and prune. The added code originates no process spawn; `settle.rs` outside tests takes a caller-supplied `&dyn ExactAbsenceProbeV1` and constructs no implementation, so any reachable spawn can arrive only through that supplied probe. Effect-freedom is conditional on that probe, and this slice adds no production caller.

## Carried to slice 5

When `sweep_orphans` is wired to drive settlement, its caller must supply a read-only probe. Wiring a spawning probe into a settlement path is a slice-5 blocker; this slice adds no caller.

## Frozen single-mutation control

- Path: `docs/superpowers/reviews/2026-08-22-r2f1b-t3b-slice2-mutation-control.patch`.
- SHA-256: `2fcc242ca810f5a5ba43965abd81720c1a19e6384219a2455ab33dbdc702be5b`.
- Logical mutation: remove the comparison that binds the fresh scan entry to the report entry's retained canonical bytes.
- Sole expected reddening test: `settle::tests::a_record_replaced_between_report_and_window_refuses`.
- The control is carried unchanged: the repair alters none of its dependent lines, it still applies cleanly, and its red run is reserved for the operator command in the task brief.

- [x] `cargo fmt --all -- --check` — **see operator evidence below**
- [x] `CARGO_INCREMENTAL=0 cargo clippy --workspace --all-targets --locked -- -D warnings` — **see operator evidence below**
- [x] `CARGO_INCREMENTAL=0 cargo test --workspace --locked --no-fail-fast` — **see operator evidence below**
- [x] `CARGO_INCREMENTAL=0 cargo test -p bridge-worktree --locked --no-fail-fast` — **see operator evidence below**
- [x] `cargo run -p a2a-bridge -- validate --repo-hygiene` at the implementation point — **see operator evidence below**
- [x] `cargo run -p a2a-bridge -- validate --repo-hygiene` at the handoff point — **see operator evidence below**


---

# Operator evidence

Recorded by the operator at candidate `ca481820` (repair) over `37931561` (slice 2), parent `c65c8eca`.
Run from a checkout under the owner-approved trusted cwd root. Exit status and FAILED counts are
authoritative; per-binary `test result:` lines are not summed, because nested filtered subprocesses
double-count them.

## Gates — all green

| Gate | Result |
|---|---|
| `cargo fmt --all -- --check` | **exit 0** |
| `CARGO_INCREMENTAL=0 cargo clippy --workspace --all-targets --locked -- -D warnings` | **exit 0** |
| `CARGO_INCREMENTAL=0 cargo test --workspace --locked --no-fail-fast` | **exit 0, 0 FAILED** |
| `CARGO_INCREMENTAL=0 cargo test -p bridge-worktree --locked --no-fail-fast` | **exit 0, 347 passed / 0 failed** |
| `cargo run -p a2a-bridge -- validate --repo-hygiene` (both points) | **exit 0** |

## Same-environment base control

`bridge-worktree` at `c65c8eca`, same host, same command: **338 passed / 0 failed**.
Candidate: **347 passed / 0 failed**. Delta **+9**.

## Frozen mutation control — RUN

- SHA-256 recomputed by the operator: `2fcc242ca810f5a5ba43965abd81720c1a19e6384219a2455ab33dbdc702be5b` — **matches the recorded value**.
- Applied to the actual head `ca481820`: **applies cleanly**.
- Result: **346 passed / 1 failed**. The single reddened test is
  `settle::tests::a_record_replaced_between_report_and_window_refuses` — one of the two mandated
  report-versus-window boundary tests, and no other test moved.
- Tree restored after the run.

## Counted lines

Added nonblank physical Rust lines against `origin/main`, post-fmt: **711**, against the **770** cap.
59 lines of headroom.

## Repair disposition

The slice-2 run ended REJECT at the review bound with two blockers. Both were repaired on the existing
artifact; nothing was restarted.

| Blocker | Disposition |
|---|---|
| Red `the_reproof_mints_no_effect` | **Real defect, fixed.** The audit pre-truncated `checked_scan.rs` at `#[cfg(test)]` and then sliced the result using `#[cfg(test)]` as its end anchor, which can never match, so `split_once` returned `None` and the unwrap panicked. The operator enumerated all six anchor pairs before the repair: the population was **exactly one**. `source_slice` now reports the missing anchor by name instead of panicking. |
| Effect-freedom bypassable via the production probe | **Mechanism accepted; root cause was the operator's contract.** The slice-2 task required routing through the existing scan and projection — machinery that takes a probe — while also forbidding any process-spawn edge; `HostGitWorktree::observe_source_common_dir_identity` shells out to `git rev-parse`, so both could not hold. Classified SMELL rather than WRONG for this artifact: `reprove_under_window` has no production caller and the only in-crate probe implementation is test-only, so no realized effect edge exists. The design stands — taking the probe as a parameter is what keeps the acting path identical to the reporting path. The audit's **claim** was amended instead. |

The repair converged in **one** review round with verify PASS on all four commands.

## Carried to slice 5

When `sweep_orphans` is wired to drive settlement, the caller **must** supply a read-only probe. Wiring a
spawning probe into a settlement path is a slice-5 blocker. This slice adds no caller.
