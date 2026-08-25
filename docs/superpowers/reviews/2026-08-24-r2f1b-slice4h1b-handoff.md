# R2f1b slice 4H-1b handoff

## Outcome

`bridge-core` now mints `UnidentifiableCleanupOwnerProofV1` only when
`transfer_cleanup_deadline` observes, while holding the flight transition lock, that the supplied
same-flight guard token is absent from live state. The proof remains a tuple struct with a private
field. Its mint is `pub(crate)`, and there is no constructor, builder, `Default`, or `From` reachable
outside `bridge-core`. The returned proof names the observed resource flight.

The three conceptual `Unknown` routes make these decisions:

| Route | Mints? | Reason |
| --- | --- | --- |
| Foreign guard | No | A caller-selected guard from another flight is caller error and says nothing about this flight's cleanup ownership. |
| Guard token not held | Yes | This call checks the same-flight token against the flight's own live guard set while holding the transition lock. That is the point at which the ownerless condition is directly observed. |
| Adopted durable terminal | No | The durable row's provenance is unknown. It may be a caller pre-seed, an earlier transfer's settlement, or another writer's terminal, so adoption cannot earn a proof. Every durable-adoption return carries `proof: None`. |

The existing control-flow decisions and action results are unchanged. `CleanupDeadlineTransferV1::Unknown`
only gains an optional minted proof.

## Red-first and focused evidence

The new tests were first run against an isolated copy of the pre-change tree with only the test
sources added:

- The `bridge-core` focused test target failed to compile with `E0026` at the proof-field matches in
  the positive, foreign-guard, public-journal-preseed, and sequential-transfer cases.
- The `bridge-workflow` integration target failed to compile with `E0026` at both proof-field matches,
  making both the minted-proof consumer and refusal-gate tests red.
- The compile-fail harness failed with a fixture mismatch: the candidate-generated diagnostic offers
  the crate-private mint as a possible spelling, while that mint does not exist on the pre-change
  crate.

Initial Cargo probes that lacked the approved offline environment failed while trying to reach the
blocked registry. They were diagnostic-run failures and are excluded from red-first evidence. The
isolated reruns used `CARGO_HOME=/cargo`, `CARGO_NET_OFFLINE=true`, localhost proxy exclusions, and an
explicit `RUSTDOC`.

On the candidate:

- the minting case proves the proof is present and names the correct flight;
- the foreign-guard and public-journal-preseed cases prove `proof: None`;
- two ordinary sequential transfers prove the settled second call cannot mint a second proof;
- the workflow helper can construct `UnsettledUnknownOwnerless` only by consuming
  `Unknown { proof: Some(proof), .. }`;
- the workflow refusal gate proves readiness remains `Disarmed` and production resolves to
  `ManualOnlyR2f1a`; and
- the clean trybuild run passes the no-`Default`, no-tuple-literal, and no-`From` case.

The `.stderr` fixture was generated with `TRYBUILD=overwrite` and then verified by a separate clean
trybuild run; it was not hand-written.

## Frozen mutation control

Control patch:
`docs/superpowers/reviews/2026-08-24-r2f1b-slice4h1b-mutation-control.patch`

SHA-256: `c18ac3de412a90c37e033a5401f8884df824cdbd5e91529a28e6625c0327a5d8`

The patch applies cleanly and changes exactly one production decision: it incorrectly mints on the
first adopted-durable-terminal return. The mutated tree still passes
`cargo clippy --all-targets --all-features --locked -- -D warnings`.

Using the configured full-suite command for both trees, the actual newly-red set difference was:

- `retained_resource_flight::tests::cleanup_deadline_public_journal_preseed_carries_no_ownerless_proof`
- `retained_resource_flight::tests::sequential_cleanup_deadline_transfers_do_not_mint_a_second_proof`

The first candidate comparison run had one unrelated compatibility-process failure,
`compatibility::tests::staged_candidate_nonzero_exit_retains_process_status`; it did not recur on the
mutated run. The control was reversed exactly, and it still applies cleanly to the restored candidate.

## Gates

All Cargo commands below used the approved offline environment; the full-suite runs also used
`CARGO_INCREMENTAL=0`.

| Gate | Result |
| --- | --- |
| `cargo fmt --all -- --check` | Pass |
| `git diff --check` | Pass |
| `cargo clippy --all-targets --all-features --locked -- -D warnings` | Pass |
| `cargo build --locked` | Pass |
| `cargo check --workspace --locked` | Pass |
| `cargo build --release --bin a2a-bridge --locked` | Pass |
| `cargo run -p a2a-bridge --locked -- validate --repo-hygiene` | Pass |
| Focused `bridge-core`, `bridge-workflow`, and clean trybuild tests | Pass |
| Configured workspace test command | Not green: 4,273 passed, 1 failed, 12 ignored, 716 filtered across 99 harnesses |

The final configured test run's sole failure was the intermittent process fixture
`compatibility::tests::staged_candidate_exec_is_bound_to_the_verified_file_object`. An earlier candidate
run had the same totals but failed the different compatibility-process fixture named above. Neither
failure overlaps the changed area or the task's focused tests, and neither was chased into production
changes. No billable smoke or compatibility case was run.

## Frozen invariants and size

`crates/bridge-workflow/src/executor.rs` has SHA-256
`def9c4fc6dc174f7d744ef2554df4f428550a84725ee71129c7ff7127be684d4` on both the task-pinned base
tree and the candidate tree, proving byte identity.

`Cargo.lock` is unchanged (SHA-256
`56a948ba41ca71540c99d38dd2ed9edf1f179b962d96e93f3b9c90554523af86` on both trees). No manifest,
`MAX_WORKTREE_CONFIGURES_IN_FLIGHT`, timer, `select!`, sleep, spawn, token, cancellation, node encoding,
or executor code changed. Readiness remains `Disarmed`.

The final change adds exactly **150 nonblank physical Rust lines**, excluding documentation, within
the 250-line stop boundary.

## Deliberate exclusions

This slice does not wire `UnsettledUnknownOwnerless` into a live cleanup path, arm automatic R2f1b,
change `NodeCleanupObservationV1` or `NodeCleanupV2` encodings, edit the executor or its biased select,
or alter scheduling/concurrency behavior. Those remain later-slice work.
