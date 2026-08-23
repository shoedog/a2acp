# T3b slice 5A handoff — boot wiring and legacy settlement

## Scope

- Base: `3d654a0eaa2edcce06515158c526870273a3461f`.
- Changed implementation files: `bin/a2a-bridge/src/main.rs`, `crates/bridge-worktree/src/custody_writer.rs`, `crates/bridge-worktree/src/settle.rs`, `crates/bridge-worktree/src/sweep.rs`, and `crates/bridge-worktree/src/sweep/report.rs`.
- Review artifacts: this handoff and `2026-08-22-r2f1b-t3b-slice5a-wiring-control.patch`.
- Repaired files: `crates/bridge-worktree/src/sweep.rs` (test-only) and this handoff.
- Rust added-line count: 547 nonblank lines, below the 790-line cap (measured against the stated base).
- Cargo manifests and `Cargo.lock` are unchanged.

## Boot wiring and gate

`sweep_orphans` now retains its exact-absence report and drives the policy-selected entries through one bounded settlement driver. `sweep_orphans_async(root: String, my_host: String, probe: &'static dyn LeaseProbe)` runs the synchronous sweep inside `tokio::task::spawn_blocking`; all five async boot callers now await that wrapper.

`EXACT_ABSENCE_POLICY_READY_V1` remains `false`, unchanged and the sole production readiness gate. Consequently this slice records and re-proves candidates but has no production settlement effect until a separately reviewed 5B gate flip. The report’s future-ready filter now admits authorized legacy entries under the same authoritative-scan condition as V3, so either population receives the same settlement policy when that gate opens. With that gate false, the driver returns before its second record scan, so the offloaded boot paths do not pay a future-settlement scan today.

Part C is a separate commit and independently revertable; it is not included in this slice.

## Proof and coexistence boundary

The shared action-time re-proof accepts an explicitly held V3 custody record or legacy sidecar, requires the authoritative report to contain the selected entry, rescans the root, and compares the exact serialized held record. Legacy reports retain serialized sidecar evidence for that comparison.

Legacy settlement reuses the existing two forgery guards before re-proof and removal: the marker must name its expected sibling and its canonical target must remain under the canonical sweep root. Its existing custody-control check prevents the legacy arm from acting when V3 custody is present. The V3 arm now independently refuses when the corresponding legacy marker exists. Thus coexistence preserves both markers; neither arm can retire the other generation’s guard. The V3 presence probe is descriptor-relative under the settlement window’s pinned root, so a dangling legacy symlink still occupies its name and refuses the irreversible V3 transition.

No `UnusedSettled` schema, residual handling, terminal-state semantics, or frozen custody-transition-table entry changed. Settlement still only retires a proven-unused marker; it never changes the checkout directory.

## Tests and bounded audits

- The report-policy test now covers a future-ready authorized legacy entry.
- `policy_selected_settlement_retires_v3_and_legacy_unused_markers` proves a valid selected report retires both marker generations after re-proof.
- Its expected legacy-marker path and action-time scan lookup are canonicalized before comparison, so a macOS symlinked temp directory cannot change the path spelling; the other `record_path()` tests already construct canonical expected paths.
- `legacy_settlement_refuses_a_forged_non_sibling_marker` and `legacy_settlement_refuses_a_sibling_marker_pointing_outside_the_root` exercise the two legacy guards.
- `policy_selected_settlement_preserves_coexisting_legacy_and_v3_markers` proves bidirectional coexistence protection.
- `a_dangling_legacy_sidecar_refuses_v3_settlement` proves the V3 descriptor-relative guard treats a dangling legacy symlink as an occupied name and preserves the `ProtectionPrepared` marker.
- `boot_sweep_drives_the_policy_selected_settlement_entries` source-pins report retention and driver invocation. `boot_sweep_call_sites_are_async_and_the_wrapper_has_no_process_or_mutation_origin` names its missing anchors, counts the five wrapper calls, proves each is awaited, rejects a direct synchronous token without whitespace dependence, requires JoinError logging, and rejects subprocess, deletion, rename, or Git-mutation origins in the async wrapper.
- The existing `settlement_probe_git_verbs_are_query_only` remains and passes.

Focused verification passed:

- `cargo fmt --all -- --check`
- `CARGO_HOME=/cargo CARGO_NET_OFFLINE=true CARGO_INCREMENTAL=0 cargo clippy -p bridge-worktree --all-targets --locked -- -D warnings`
- `CARGO_HOME=/cargo CARGO_NET_OFFLINE=true CARGO_INCREMENTAL=0 cargo test -p bridge-worktree --lib --locked --no-fail-fast` (363 passed)
- `CARGO_HOME=/cargo CARGO_NET_OFFLINE=true CARGO_INCREMENTAL=0 cargo test -p a2a-bridge --bin a2a-bridge --no-run --locked`
- `policy_selected_settlement_retires_v3_and_legacy_unused_markers` and `settlement_probe_git_verbs_are_query_only` after control reversal.

## Frozen controls

- Carried slice-4 control: `docs/superpowers/reviews/2026-08-22-r2f1b-t3b-slice4-mutation-control.patch`
- Carried slice-4 SHA-256: `cb7667c947558e2d6fb041c565a9aa419ac0be8392db107e0e3226d817aeac3f`
- New slice-5A control: `docs/superpowers/reviews/2026-08-22-r2f1b-t3b-slice5a-wiring-control.patch`
- New slice-5A SHA-256: `03a5632bcd44f9bf1931b19521027a27061754ec01bb9aa1f54dbf939be0ccd4`

The new control changes the legacy policy match from `Authorized` to `Refused`. It applies cleanly to the restored implementation and produces the expected red test `sweep::tests::policy_selected_settlement_retires_v3_and_legacy_unused_markers`, whose legacy-marker retirement assertion fails. The control was then reversed and the focused positive test passed.

The test-only path-shape repair changes no line the slice-5A control depends on, so the control is carried unchanged and its SHA-256 remains `03a5632bcd44f9bf1931b19521027a27061754ec01bb9aa1f54dbf939be0ccd4`.

- [ ] `cargo fmt --all -- --check` — **PENDING OPERATOR**
- [ ] `CARGO_INCREMENTAL=0 cargo clippy --workspace --all-targets --locked -- -D warnings` — **PENDING OPERATOR**
- [ ] `CARGO_INCREMENTAL=0 cargo test --workspace --locked --no-fail-fast` — **PENDING OPERATOR**
- [ ] `CARGO_INCREMENTAL=0 cargo test -p bridge-worktree --locked --no-fail-fast` — **PENDING OPERATOR**
- [ ] `cargo run -p a2a-bridge -- validate --repo-hygiene` at the implementation point — **PENDING OPERATOR**
- [ ] `cargo run -p a2a-bridge -- validate --repo-hygiene` at the handoff point — **PENDING OPERATOR**
