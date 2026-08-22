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

- [ ] `cargo fmt --all -- --check` — **PENDING OPERATOR**
- [ ] `CARGO_INCREMENTAL=0 cargo clippy --workspace --all-targets --locked -- -D warnings` — **PENDING OPERATOR**
- [ ] `CARGO_INCREMENTAL=0 cargo test --workspace --locked --no-fail-fast` — **PENDING OPERATOR**
- [ ] `CARGO_INCREMENTAL=0 cargo test -p bridge-worktree --locked --no-fail-fast` — **PENDING OPERATOR**
- [ ] `cargo run -p a2a-bridge -- validate --repo-hygiene` at the implementation point — **PENDING OPERATOR**
- [ ] `cargo run -p a2a-bridge -- validate --repo-hygiene` at the handoff point — **PENDING OPERATOR**
