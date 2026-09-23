# Handoff - ADR-0041 Slice 2B1 pure capsule contracts

**Date:** 2026-09-20
**Rejected predecessor:** `a210d5d5e58717080f4d1e6a1f887a83404042c2`
**Repaired artifact:** verified retained candidate; final amended commit identity is recorded by the planning-lane custody handoff.
**Scope:** provider-free, effect-free `bridge-core` custody capsule contracts only. No export, filesystem materialization, Git subprocess, encryption/decryption provider, restore execution, CLI/config/store wiring, remote custody, cleanup, or operator adoption was added.

**Reading order:** the Tier-3 update and host closure evidence below supersede the historical
pre-addendum implementation and verification narrative later in this file where they conflict.

## Second closure review - 2026-09-23

Exact code candidate `8d9d4c45d93ef6441aef52fea2fc35db4b2316b9` received the sole authorized host
Codex `gpt-5.6-sol`/`xhigh` hard-read-only closure review as
`exec-6fb6dc2200ad44563d487e3e2ee96408` / `attempt-0c3b24562fa60b0e6c9fd6dd1e850641`.
The review returned **REJECT**: R2 and R3 are RESOLVED, while R1 is UNRESOLVED with one BLOCKER WRONG and
four deferred SMELLs. Raw result SHA-256 is
`574c9481eb6a9fe4a862b55d001b5fe57142f472577b38c5a3a044a7fa7c46bb`; the durable disposition is
`docs/superpowers/reviews/2026-09-23-adr0041-slice2b1-second-closure-review.md`.

The remaining R1 mechanism is type-level provenance: source and sink validators both return the same public
`CustodyEnvelopeStreamReceiptV1`, and public `CustodyEnvelopeSealReceiptV1::new` accepts either. Private fields
block raw struct construction but do not stop a source/decoy receipt from being relabeled as ciphertext and
used to assemble an accepted capsule proof/binding. The prior R1 mutation proved faithful copying of the
supplied receipt, not that it came from the ciphertext sink. No repair or rereview is authorized; preserve
reviewed code `8d9d4c45` and this docs-only custody successor.

## Second retained-candidate repair update - 2026-09-23

The owner authorized one write-capable retained-candidate repair round on branch
`implement/impl-16580-ybg5ha5r`, incoming HEAD
`77a6598a1990b03f01f3bf7b0f12df2f1027101f`, base
`80d4586828aca7d5959b3931f3d30d2cd593e662`, and repair contract
`/contract/repair.md` with SHA-256
`0b4d51bacb7a9c5578f5e3a13155501a95c256870cd0a220d0e176e95a283ec9`.
Pre-edit checks matched the exact branch, HEAD, base merge point, digest, and clean status.

Repairs staged in this round:

- R1: `CustodyEnvelopeSealReceiptV1::new` now consumes the private-field
  `CustodyEnvelopeStreamReceiptV1` returned by the stream validators. Artifact name, manifest digest,
  complete envelope format, and canonical recipients are derived from the validated context; ciphertext
  length and SHA-256 are derived from the finalized stream receipt. Zero-length ciphertext still refuses.
  A compile-fail doctest documents that external callers cannot forge a stream receipt by struct literal,
  and the focused integration test proves the genuine finalized digest/length carry into the receipt and
  sealed artifact.
- R2: `CustodyCapsuleSealProofV1` is a private-field capsule proof assembled only from non-empty
  `CustodyEnvelopeSealReceiptV1` collections. It derives the inner generic `CustodySealV1` from those
  receipts, requires shared manifest digest, complete format/tool/version, and canonical recipients,
  and leaves generic duplicate-name rejection to `CustodySealV1::new`.
  `CustodyCapsuleBindingV1::new` and `CustodyEnvelopeOpenRequestV1::from_seal_artifact` now accept this
  proof instead of a generic seal.
- R3: `CustodySealV1::validate_borrowed_canonical` adds a borrowed validation path for schema,
  non-empty strictly ordered unique artifacts, borrowed artifact names, prefix conflicts, non-empty
  strictly ordered unique recipients, and non-empty format/tool/version strings. The capsule open preflight
  performs capsule-v1 seal-wide limits before that borrowed generic validation and before membership or
  selected-artifact checks. The focused integration test uses the allocation counter around an over-limit
  recipient population and asserts the preflight returns `SealExceedsV1Limits` without recipient-proportional
  allocation. An in-crate seal unit test covers malformed generic artifact ordering through the borrowed
  validator.

RED-first evidence limit: temporary focused RED controls were added before the implementation for the R1
forged-digest path and the R2 generic substituted-identity binding path, but the direct RED command
`cargo test --locked --offline -p bridge-core --test custody_capsule red_` failed before compiling because
this retained container's offline registry cannot resolve `arc-swap`, required by `bridge-a2a-inbound v0.3.1`.
No behavioral RED execution count was produced in this container. The final source keeps separately named
R1/R2/R3 regression tests for the controller's host gates.

The one-turn Tier-3 workflow completed as execution
`exec-a79e871ae620b0d4144dc06ce6228f6f`, attempt
`attempt-d6d4f303d2185e22cba22951e2ae0714`; its terminal result SHA-256 was
`e952bb46e8da2cc9f60e795bd203ef378293107e05adb7428830736fea5c6562`.
The controller then made three bounded test-only corrections: imported the stream-receipt type, changed
the R2 mismatch fixtures to a distinct valid artifact name so duplicate-name rejection could not mask the
identity checks, and made the allocation counter thread-local with an exact-zero assertion.

Focused verification run directly in this retained clone:

```text
cargo test --locked --offline -p bridge-core --test custody_capsule
# failed before compilation: no matching package named `arc-swap` found

cargo test --locked --offline -p bridge-core
# failed before compilation: no matching package named `arc-swap` found

cargo test --locked --offline --workspace --all-targets
# failed before compilation: no matching package named `arc-swap` found

cargo test --locked --offline --workspace
# failed before compilation: no matching package named `arc-swap` found

cargo clippy --locked --offline --workspace --all-targets -- -D warnings
# failed before compilation: no matching package named `arc-swap` found

cargo fmt --all -- --check
# passed

cargo deny check --disable-fetch
# failed before policy evaluation: cargo-deny is not installed (`no such command: deny`)

cargo run --locked --offline -p a2a-bridge -- validate --repo-hygiene
# failed before running hygiene: no matching package named `arc-swap` found
```

Controller host mutation evidence on the repaired source was discriminating and was restored after each
probe:

- R1 replaced the receipt ciphertext digest with a digest of `mutated`; the exact receipt-derivation test
  failed 0/1 with the wrong digest.
- R2 removed shared manifest/format/recipient checks; the exact capsule-proof test failed 0/1 because a
  mismatched receipt population was accepted.
- R3 ran allocating generic validation before capsule limits; the exact allocation test failed 0/1 after
  observing 88 allocations instead of zero.
- After restoration, the focused capsule suite passed 25/25.

Final controller host gates:

```text
cargo test --locked --offline -p bridge-core
# passed, including 732 unit tests, 25 capsule integration tests, the new
# compile-fail doctest, and all remaining bridge-core integration/doc tests

cargo test --locked --offline --workspace --all-targets --quiet
# 4,472 passed; 0 failed; 13 ignored; 90 test groups

cargo test --locked --offline --workspace --quiet
# 4,476 passed; 0 failed; 13 ignored; 106 test groups

cargo clippy --locked --offline --workspace --all-targets -- -D warnings
# passed

cargo fmt --all -- --check
# passed

cargo deny check --disable-fetch
# passed: advisories, bans, licenses, and sources ok; allowlisted duplicate warnings remain

cargo run --locked --offline -p a2a-bridge -- validate --repo-hygiene
# passed: 41 tracked artifacts; 9 validated example configs
```

This round did not obtain independent approval. No dependency, persisted wire schema, effect adapter,
provider call, network access, cleanup, merge, publication, or 2B2/2B3 work was performed. The repaired
candidate is host-verified but rejected by the second closure review described above.

## Tier-3 retained-candidate repair update - 2026-09-21

Retained candidate `9e2fd7e1671a01a93ecff346f32024bd11c4a6cb` on branch `implement/impl-16580-ybg5ha5r` was repaired in place under the reviewed addendum mounted at `/contract/addendum.md` (observed SHA-256 `e95df3c5132c0cda055eae63efadcf7b1b3b4d1d238f56b01d20b9d811875de3`). The checkout identity matched the authorized branch and HEAD before editing, and `git status --short` was clean.

2B1a repair summary:

- Public artifact-role row construction, Serde `TryFrom`, index construction, validation, and canonical decode now enforce the closed reserved name-role bijection before returning success. Safe but non-reserved names, wrong reserved role/name pairings, object-database coverage payload rows, and coverage names whose suffix maps to another class refuse as `UnmappedArtifact`.
- Layout derivation now builds private pre-canonical artifact-role candidates, invokes a pure duplicate-ownership helper, then materializes canonical rows. The helper has an in-crate focused test with all three valid control candidates plus two distinct path-safe `worktree` owners; a companion public-boundary test proves that fixture cannot become a canonical public index.
- The capsule-side logical artifact-name validator now applies the 4,096-byte v1 name cap before canonical encoding.

2B1b repair summary:

- The whole-artifact `CustodyEnvelopeOpaqueBytesV1`, sealed-bytes, and opened-bytes port values were replaced by one-chunk-at-a-time stream records, source/sink validators, stream receipts, and sealed sealer/opener traits that exchange source/sink ports rather than complete artifact `Vec<u8>` values.
- Stream limits enforce nonzero caller budgets, 1 MiB chunk cap, 16,384 chunk cap, 10 GiB logical cap, checked chunk-capacity product, declared source totals, budget-only output sinks, contiguous ordinals, finality, zero-byte plaintext representation, post-final rejection, cumulative totals, and incremental SHA-256 receipts.
- Envelope context canonical records now bind the complete validated format record, logical artifact name, manifest digest, and canonical recipients. The new seal-derived open request validates seal-wide capsule limits before membership lookup, then selected-artifact limits, and binds the selected ciphertext length and digest from the seal.
- Seal receipts require authenticated name, manifest digest, complete format, recipients, non-empty ciphertext length, and ciphertext digest to match the sealing context. Metadata and recipient constructors preflight row counts, field bytes, aggregate bytes, canonicalized results, and persisted JSON size where applicable.

Additional tests added or repaired in `crates/bridge-core/tests/custody_capsule.rs` cover the public reserved-row bijection, the unreachable public duplicate-owner fixture, multi-chunk artifacts larger than 1 MiB, oversized chunks, capacity product refusal, source budget/capacity refusal, gapped ordinals, early final, missing final, non-final exact total, chunk after final, valid/invalid zero-byte streams, budget-only sink overrun, multi-chunk SHA-256 receipts, seal membership refusal, 2A-valid seals exceeding capsule-v1 format/recipient/selected-length limits as `SealExceedsV1Limits`, and seal receipt identity/empty-ciphertext substitutions. The private helper mutation seam is covered by an in-crate unit test in `custody_capsule.rs`.

Verification run directly in the retained clone:

```text
cargo test --locked --offline -p bridge-core --test custody_capsule
cargo test --locked --offline -p bridge-core
cargo test --locked --offline --workspace --all-targets
cargo test --locked --offline --workspace
cargo clippy --locked --offline --workspace --all-targets -- -D warnings
cargo run --locked --offline -p a2a-bridge -- validate --repo-hygiene
```

Each command failed before compiling this candidate because the offline resolver could not find the locked dependency `arc-swap`, required by `bridge-a2a-inbound v0.3.1`. No test count was produced for those gates in this container.

```text
error: no matching package named `arc-swap` found
location searched: crates.io index
required by package `bridge-a2a-inbound v0.3.1 (/Users/wesleyjinks/code/.a2a-implement/impl-16580-ybg5ha5r/crates/bridge-a2a-inbound)`
```

`cargo fmt --all -- --check` passed. A narrower `rustfmt crates/bridge-core/src/custody_capsule.rs crates/bridge-core/src/custody_seal.rs crates/bridge-core/tests/custody_capsule.rs` was also run after edits and passed. `cargo deny check` was not run: the unguarded command was rejected by automatic approval review as potentially network-bearing under the no-network contract, and the safer `cargo deny check --disable-fetch` could not run because `cargo-deny` is not installed in this container. A non-offline Cargo compile probe was also rejected by automatic approval review for the same no-network contract reason.

Staging evidence before this handoff note was refreshed:

```text
git diff --cached --stat
 crates/bridge-core/src/custody_capsule.rs          | 791 ++++++++++++++++++---
 crates/bridge-core/src/custody_seal.rs             |  10 +
 crates/bridge-core/tests/custody_capsule.rs        | 354 +++++++--
 docs/reliability-execution-roadmap.md              |   4 +-
 .../2026-09-20-adr0041-slice2b-planning-handoff.md |   4 +-
 ...9-20-adr0041-slice2b1-implementation-handoff.md |  57 ++
 6 files changed, 1038 insertions(+), 182 deletions(-)

git diff --cached --check
# passed with no output
```

## Controller host verification and mutation evidence - 2026-09-21

The host's offline cache resolved `arc-swap`, unlike the repair container. The first focused
compile exposed a missing `CustodySealedArtifactV1` import in the repaired module; the controller
added that import and removed one unused test helper. The focused test then passed **23/23**.
The full `bridge-core` package gate passed, including **731/731** library tests, the **23/23**
capsule integration tests, compile-fail targets, and two doc tests. Both direct workspace commands
(`--all-targets` and the default including doc tests) exited zero; the largest unit targets
reported **1115/1115** and **731/731**, respectively, with no failing groups. A final
all-target workspace rerun after the sink assertion exited zero: **4,469 passed / 0 failed /
13 ignored across 90 target result groups**; its complete output is retained at
`/private/tmp/a2a-bridge-slice2b1-repair-20260920/host-all-targets-final.log`.
Some ignored tests require live providers and were not invoked. `cargo clippy --locked --offline --workspace
--all-targets -- -D warnings`, `cargo fmt --all -- --check`, and `git diff --cached --check`
passed. `cargo deny check --disable-fetch` exited zero with duplicate-dependency warnings and
`advisories ok, bans ok, licenses ok, sources ok`. The hygiene gate initially could not open
Cargo's target lock inside the sandbox; the exact host retry passed with **41 tracked artifacts /
9 example configs**. This environment distinction does not reclassify the container cache miss
as a code failure.

Isolated source mutations, one at a time, with the named branch restored after each run:

| Disabled branch | Exact selected result | Discriminating observation |
|---|---|---|
| private duplicate coverage owner | 0 passed / 1 failed | expected `DuplicateArtifactRole`, got `Ok(())` |
| contiguous ordinal check | 0 passed / 1 failed | gapped ordinal was accepted |
| finish requires final chunk | 0 passed / 1 failed | budget sink accepted a non-final stream; a new sink assertion was added for this proof |
| cumulative caller total budget | 0 passed / 1 failed | over-budget sink chunk was accepted |

An initial `--exact` filter selected zero tests; it is inadmissible and was corrected before
the duplicate-owner mutation result above. Restored focused suite: **23 passed / 0 failed**.
These four controls do not claim to cover every new constructor bound or state-machine branch;
the closure reviewer must independently assess the remaining evidence against the addendum.

The historical sections below record the original rejected candidate and its container gates,
not the final host result or a current approval.

## Implemented

- Added `bridge_core::custody_capsule` with private-field Serde v1 records for `custody-capsule-index.v1`, `custody-restore-policy.v1`, envelope format/context records, pure layout derivation, post-effect binding validation, bounded canonical decoding, and sealed envelope port traits.
- Added closed artifact roles and reserved layout names:
  - `control/manifest.json.enc`
  - `control/capsule-index.json.enc`
  - `control/restore-policy.json.enc`
  - `git/objects.pack.enc`
  - `payload/<coverage-class>.bin.enc`
- Role rows validate only the logical name namespace; layout derivation and binding enforce the reserved-name mapping. This keeps duplicate class ownership reachable as its own typed branch while still refusing non-derived layouts at binding.
- `CustodyEnvelopeSealedBytesV1` now validates ciphertext, metadata, envelope format, and canonical recipients against the authenticated `CustodyEnvelopeContextV1`.
- Added minimal immutable accessors to `custody_seal.rs` for manifest coverage/object rows, seal digest/artifacts/recipient/format identities, and sealed artifact names.
- Added focused integration coverage in `crates/bridge-core/tests/custody_capsule.rs` for canonical layout totals, digest binding, index/seal equality, inert restore policy, envelope context recipient canonicalization, bounded decode/metadata/opaque bytes, direct Serde validation, order-independent index construction, non-UTF-8 logical names, and digest sensitivity.

## Structural RED

Before adding production code, the new focused target contained only the import of `bridge_core::custody_capsule::CustodyCapsuleIndexV1`.

Command:

```text
cargo test --locked --offline -p bridge-core --test custody_capsule
```

Result: failed as expected with unresolved import before the module existed.

```text
error[E0432]: unresolved import `bridge_core::custody_capsule`
 --> crates/bridge-core/tests/custody_capsule.rs:1:18
  |
1 | use bridge_core::custody_capsule::CustodyCapsuleIndexV1;
  |                  ^^^^^^^^^^^^^^^ could not find `custody_capsule` in `bridge_core`
error: could not compile `bridge-core` (test "custody_capsule") due to 1 previous error
```

## Behavioral Mutation Evidence

Every mutation was applied to compiling code, run directly against an exact focused test, and then restored. A final restoration run passed `18 passed / 0 failed / 0 ignored`.

| Mutation | Exact focused test | Mutated result |
|---|---|---|
| 1. Omit `worktree` payload mapping | `layout_derives_exact_control_and_full_artifact_totals` | exit 101, failed with `DerivedLayoutMismatch` |
| 2. Allow a seal artifact to remain unmapped | `binding_reports_missing_extra_unmapped_and_duplicate_artifacts` | exit 101, failed because binding returned `Ok` for the extra seal artifact |
| 3. Disable duplicate class ownership while using distinct logical names | `duplicate_class_ownership_uses_distinct_names_and_reaches_the_exact_branch` | exit 101, assertion flipped away from `DuplicateArtifactRole` |
| 4. Treat unresolved coverage as empty | `layout_refuses_non_sealable_and_object_database_state_mismatches` | exit 101, assertion flipped away from `NonSealableManifest` |
| 5. Allow active restore behavior | `restore_policy_accepts_only_the_closed_inert_values` | exit 101, active hooks constructed `Ok` |
| 6a. Disable three-way digest check, foreign index digest | `binding_refuses_a_foreign_index_manifest_digest` | exit 101, exact test failed |
| 6b. Disable three-way digest check, foreign seal digest | `binding_refuses_a_foreign_seal_manifest_digest` | exit 101, exact test failed |
| 6c. Disable three-way digest check, different valid manifest while index/seal agree | `binding_refuses_a_foreign_manifest_while_index_and_seal_agree` | exit 101, exact test failed |
| 7. Accept `unknown` object kind | `layout_refuses_unknown_object_kind` | exit 101, layout construction returned `Ok` |

Restoration confirmation:

```text
cargo test --locked --offline -p bridge-core --test custody_capsule
running 18 tests
...
test result: ok. 18 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
```

## Verification

Commands run directly unless noted:

```text
cargo test --locked --offline -p bridge-core --test custody_capsule
```

Passed: **18 passed / 0 failed / 0 ignored**.

```text
cargo test --locked --offline -p bridge-core --lib --tests
```

Passed all non-doctest `bridge-core` targets, including **730 passed / 0 failed** in the library target and the focused `custody_capsule` **18 passed / 0 failed** integration target.

```text
cargo test --locked --offline -p bridge-core
```

Rerun after repair confirmed the compiled unit/integration targets pass, then failed before doc tests because this quarantine has no `rustdoc` binary:

```text
Doc-tests bridge_core
error: doctest failed, to rerun pass `-p bridge-core --doc`
Caused by: could not execute process `rustdoc ...` (never executed)
Caused by: No such file or directory (os error 2)
```

```text
cargo test --locked --offline --workspace --all-targets
```

Rerun after repair reproduced the existing `a2a-bridge` e2e registry failure outside this slice, after `a2a-bridge` unit tests passed **1113 passed / 0 failed** and the new `bridge-core` focused tests passed under the package gates above:

```text
bin/a2a-bridge/tests/e2e_registry.rs:621:17
called `Result::unwrap()` on an `Err` value: AgentFailure { diagnostic: FailureDiagnostic { failed_phase: PromptStream, last_completed_phase: Some(PromptStart), class: Transport, disposition: Fatal, code: DiagnosticCode("api.prompt.error_body_read"), prompt_may_have_been_accepted: true } }
```

```text
cargo test --locked --offline --workspace
```

Not rerun after the repair because the stricter all-target workspace gate above reproduced the same out-of-scope e2e failure.

```text
cargo clippy --locked --offline --workspace --all-targets -- -D warnings
```

Passed.

```text
cargo fmt --all -- --check
```

Could not run in this quarantine because `cargo` lacks the `fmt` subcommand:

```text
error: no such command: `fmt`
```

The local substitute passed:

```text
rustfmt --edition 2021 --check crates/bridge-core/src/custody_capsule.rs crates/bridge-core/src/custody_seal.rs crates/bridge-core/src/lib.rs crates/bridge-core/tests/custody_capsule.rs
```

```text
git diff --check
```

Passed.

```text
cargo deny check
```

Could not run in this quarantine because `cargo` lacks the `deny` subcommand:

```text
error: no such command: `deny`
```

```text
cargo run --locked --offline -p a2a-bridge -- validate --repo-hygiene
```

Passed: **41 tracked artifacts / 9 validated example configs**.

## Environment Notes

All shell commands required escalated execution because the sandbox wrapper failed before command execution:

```text
bwrap: No permissions to create a new namespace, likely because the kernel does not allow non-privileged user namespaces.
```

Bare `rustfmt` is used locally because `cargo fmt` is unavailable in this quarantine. The controller should repeat `cargo fmt --all -- --check` on the host toolchain.

## Stop Boundary

2B1 remains pure contracts only. No 2B2/2B3 local export, object closure proof, filesystem materialization, Git subprocess, production envelope implementation, restore engine, remote custody, cleanup, or operator adoption is included.
