# ADR-0041 Slice 2B2 implementation handoff — isolated local export and Git-object closure

**Status:** the implementation is complete in the container lane and committed as `87570241`; `91d0bf13` adds the
`#[cfg(unix)]` accessor gates. Every §7 control exists and passes. Every control's guard mutation flipped its control
at turn 3 (66/66, two full rounds), and each repair round re-ran its new and affected rows (§11.3, §12.3). The
controller ran the **macOS host lane** on the turn-3 tree (§10). The **native Linux ext4 lane** (GitHub Actions ubuntu)
and `cargo deny` are not executed here (§6) and remain for CI (§9).

**Repair round 1 (2026-09-26, §11):** the Sol implementation review of `87570241` found three WRONG blockers. All
three were repaired RED-first on top of `91d0bf13`: the non-bare worktree root (W1), the `git init` logical bytes in
the ledger (W2), and the format-aware `verify-pack -v` bound (W3). The matrix then defined 73 rows; the 7 new rows
and the 17 existing rows they affect all FLIPPED. Repair round 1 is committed as `7842f620`.

**Repair round 2 (2026-09-26, §12):** Sol implementation review round 2 of `7842f620` found W1 unresolved for a
stale, renamed worktree (BLOCKER), and found stale commit-status text in this handoff (IMMATERIAL). Both are repaired
on top of `7842f620`. The overlap preflight now compares scratch ancestors with the identities the capability's pinned
descriptors retained, and the capability is rechecked immediately before the first scratch write. The matrix defines
75 rows; the 2 new rows and the 11 existing rows they affect or whose targets lie in a changed function all FLIPPED.
Repair round 2 lands in the commit that contains §12. Figures in §2 and §4 are the round-2 figures. §5's table is
the turn-3 record, and §11.3 and §12.3 extend it.

**Task (authoritative):** `docs/superpowers/plans/2026-09-23-adr0041-slice2b2-isolated-export-task.md`, revision 11,
SHA-256 `b9f1215aaac592d3185e3c315929a6b3a996d387d19a1afd48ac3aeb8ad814e9`, as it stands at the clone's base commit
(re-verified at the start of turn 3).

**Clone:** `/Users/wesleyjinks/code/.a2a-implement/impl-75891-9w4stng1`
**Branch:** `implement/impl-75891-9w4stng1`
**Base HEAD:** `742e0a60521c8ed2a8dd334f90478adafb10f3d2`
**2B2a seam consumed unchanged:** `crates/bridge-core/src/custody_git.rs` and the four `PinnedDirectoryV1` methods in
`crates/bridge-core/src/fs_custody.rs`, both merged at `67f414e7`. Neither file is modified: both equal `HEAD`
(`git diff --quiet HEAD --` on both), SHA-256 `78837e41…5d061` and `142cf271…217a6`. No §10 stop condition fired.

**Handoff path.** Task §8 names `docs/superpowers/reviews/2026-09-23-adr0041-slice2b2-implementation-handoff.md`; the
controller's brief names this file (`2026-09-25-…`). The brief is followed; the §8 name is not created.

**Turn history.**

| Turn | Ended by | Snapshot |
|---|---|---|
| 1 | transient upstream API 500 | `refs/wip/2b2-edit-turn1` (`125c20fb`) |
| 2 | controller stop: the image served Opus 5, not Opus 5.5 | `refs/wip/2b2-edit-turn2` (`b791ddc4`) |
| 3 | this turn (Opus 5.5), completed | continued from `b791ddc4`; the working tree was verified byte-equal to that snapshot before any edit |

Nothing was reset, discarded, or restarted.

**Container lane:** Linux `7.0.14-orbstack`, OrbStack, `/tmp` on **overlayfs**, running as **uid 0**. Git 2.54.0 at
`/opt/git/bin/git` (the first non-symlink `git` on `PATH`; `/usr/local/bin/git`, `/usr/bin/git`, `/bin/git` also
report 2.54.0). Admitted route in the tests: 2B2a's `#[cfg(test)]` `TestSystem` profile (trusted uid 0, mode-bits write
check), pinned by the lane digest helper in `custody_export_tests.rs`; controls 31 and the version test use 2B2a's
`TestFixture` profile with an owner-private anchor. Every cargo command in this document is run with
`CARGO_HOME=/cargo CARGO_NET_OFFLINE=true CARGO_TARGET_DIR=/tmp/target`.

---

## 1. Structural RED on the merge predecessor

Captured by turn 1 before any production code, at base `742e0a60`, and retained at
`.git/a2a-bridge/mutation/structural-red.txt`:

- `crates/bridge-core/src/custody_export.rs` and `crates/bridge-core/src/custody_export_tests.rs` did not exist;
- `crates/bridge-core/src/lib.rs` had no `custody_export` declaration;
- `crates/bridge-core/src/custody_capsule.rs` line 1434 read `mod sealed {` (module-private), so no sibling module
  could implement the sealed envelope traits.

---

## 2. Inventory of the tree against the task

Turn 2's inventory marked every §2–§6 behavior `PRESENT`. Turn 3 re-verified it against the source rather than trusting
it. It built the tree and ran the existing tests: `custody_export` 4/4, doctests 7/7, the `custody_capsule` target
18/18, and the `custody_seal` lib tests 4/4. It then read every production path against the task text. Several
`PRESENT` claims did not hold. **Turn 3 start** is that verified state; **Now** is the delivered state.

Legend: `PRESENT` exists and matches the task; `PARTIAL` exists but deviates; `DEFECT` exists and is wrong; `MISSING`
is absent; `FIXED`/`ADDED` landed this turn, with the named control and mutation as its evidence.

### 2.1 §2–§6 production behaviors

| Area | Behavior | Turn 3 start | Now |
|---|---|---|---|
| §2 | crate-private `export_capsule_v1` in a private `custody_export` module | PRESENT | PRESENT (24a, M24a) |
| §2 | caller budgets validated against the fixed §3 ceilings | PRESENT | PRESENT (11, M11a) |
| §2 | bounded canonical-manifest preflight strictly before `CustodyCapsuleLayoutV1::derive` | PRESENT | PRESENT (11, M11k, M11l) |
| §2 | non-cloneable generation-bound `CustodyCaptureCapabilityV1`, crate-private fixture mint only | PRESENT | PRESENT |
| §2 | capability↔manifest binding: four identities, one object format, exact inventory, via crate-private `custody_seal` accessors | PRESENT | PRESENT (18, M18-*) |
| §2 | caller `GitRouteRequestV1` passed to 2B2a admission unchanged | PRESENT | PRESENT (31, M31) |
| §2 | scratch-root disjointness: canonical paths both directions plus ancestor identity | PARTIAL: the identity comparison ran one direction only | FIXED: identity compared in both directions (21, M21; see §7 on the identity layer). **Repair 1 (W1):** the capability pins the source repository root (a non-bare worktree) apart from the git directory, and it joins the protected paths (21 worktree rows, M21-root). **Repair 2 (W1-stale):** scratch ancestors are compared with each pinned descriptor's retained identity, never one re-derived by reopening its path (21 stale-pin row, M21-retained) |
| §2 | scratch root owner-private and empty | PRESENT | PRESENT |
| §2 | exactly two top-level scratch children, created through `PinnedDirectoryV1` | PRESENT | PRESENT (asserted by `the_capsule_holds_exactly_the_reserved_names_plus_the_seal`) |
| §2 | exporter never deletes anything | PRESENT | PRESENT |
| §3 | scratch-wide ledger, checked arithmetic, 64 KiB per-entry allowance | PARTIAL: the `index-pack` bound used unchecked `+`/`*` | FIXED: the `index-pack` bound (now `index_pack_logical_bound`) is fully checked (11, M11f, M11g). **Repair 1 (W2):** each `git init` reserves a 4 KiB logical bound for `HEAD` and `config` beside its nine allowances, and the ledger equals an independent census of logical bytes plus allowances (11, M11n, M11o) |
| §3 | one pre-reserved pack-output allowance `A`, `stdout_limit = A`, reconciled to `L` | PRESENT | PRESENT (35, M35) |
| §3 | re-measure every file under the git directory after **each** child | PARTIAL: only after `init` and `index-pack`; `verify.git` compared with the `index-pack` reservation alone | FIXED: re-measured after all nine children against a per-directory budget (11, M11h, M11i). **Repair 1 (W2):** the budget keeps logical bytes apart from entries; each re-measure holds logical bytes to their own bound, then reconciles that bound and the ledger down to the measured bytes (11, M11o, M11p) |
| §3 | no `Vec` sized from an unvalidated length | DEFECT: `read_alternates_digest` called `read_to_end` before its size check | FIXED: read through `take(MAX + 1)` (11, M11m) |
| §3 | per-chunk ceilings enforced before a chunk is written or charged | PARTIAL: chunk bytes and chunk count were enforced only by the 2B1 validator *after* `write_all` | FIXED: sink pre-write checks (11, M11c, M11d, M11e) |
| §4 | mixed/foreign object format refused before any spawn | PRESENT | PRESENT (18, M18-format) |
| §4 | shallow/grafted/promisor source state refused before `pack-objects` | PRESENT (fixture-supplied flags) | PRESENT (see §7) |
| §4 | recursive alternate chain pinned by identity and alternates-file content | PRESENT | PRESENT (5a/5b) |
| §4.1 | every Git child through the 2B2a runner, pre-spawn and post-exit callbacks | PRESENT | PRESENT (5a/5b/9a/9b, M05a, M05b). **Repair 1 (W1):** the callbacks also recheck the pinned source repository root (9a/9b worktree rows, M09-root). **Repair 2 (W1-stale):** the capability is also rechecked immediately before the first scratch write (9 pre-write rows, M09-prewrite) |
| §4.1 | `work/source-git/` created by `InitBare`, no source config written | PRESENT | PRESENT (30, M30) |
| §4.2 | `cat-file --batch-check` presence/kind proof | PRESENT | PRESENT (30) |
| §4.2 | one `pack-objects --stdout` run into one create-new `work/objects.pack`, synced, recorded | PRESENT | PRESENT (34, M34) |
| §4.2 | evidence records the exact argv, **Git version**, environment keys, format, status, stream evidence | DEFECT: `git_version` hard-coded to `GitRunnerV1::MINIMUM_VERSION` | FIXED: every run record carries the runner's admitted version (M-version) |
| §5 | step 1 from the **retained descriptor**, `max_stdin_bytes` = **recorded** length | PARTIAL: re-opened by name, bounded by the *current* `fstat` length | FIXED: `VerifiedPackV1` retains the descriptor for §5 and §6 (7, M07b) |
| §5 | step 1 `GitRunEvidenceV1.stdin` equals the verified-pack identity, then `verify-pack -v` | PRESENT | PRESENT (17, M17). **Repair 1 (W3):** `verify-pack -v` output is bounded by the checked, format-aware `verify_pack_stdout_limit`, not the shared 128-byte line estimate (11, M11q, M11r) |
| §5 | a typed strict-**pack** refusal at step 1 | MISSING: surfaced as generic `GitChild` | ADDED: `StrictPack` (7, M07) |
| §5 | steps 2–4: exact inventory, all-object closure, `fsck --no-dangling` output check | PRESENT | PRESENT (1–4, 6, 20b; M01, M03, M06, M20b) |
| §5 | `StrictObjectCheck` with the bounded message id, at step 1 and step 4, as **separable** classifiers | PARTIAL: the generic exit check also classified, so the two guards were not separable | FIXED: `classify_index_pack` (step 1) and `check_fsck_output` (step 4) own their classifiers (20a/20b, M20a, M20b) |
| §6 | per-artifact create-new staging, destination-owned sink validator | PRESENT | PRESENT |
| §6 | exporter-built expected receipt, whole-receipt equality over `ReceiptFieldsV1` | PRESENT | PRESENT (15, M15, M15-1..8) |
| §6 | exact-total plaintext source wrapper for every artifact, with a typed `finish` refusal | PARTIAL: `finish` failure surfaced as a raw `Capsule` error | FIXED: `PlaintextNotConsumed` (16a, M16a); identity (16b, M16b) |
| §6 | content remeasurement after sink finish and staging sync | PRESENT | PRESENT (32, M32) |
| §6 | seal barrier **immediately before** the seal rename, including destination identities | PARTIAL: ran before the seal's own staging write; no destination-directory recheck | FIXED: runs inside `publish_new_regular_child_with_before_rename`, rechecks scratch, `capsule/`, and every capsule directory, then re-hashes every artifact (10b, 33; M10b, M33) |
| §6 | commit-point lattice | DEFECT: `(SealRename, After)` returned `Err` with the seal in place; `is_pre_commit` excluded `(SealRename, Before)` | FIXED: position-aware `is_pre_commit`; every post-rename fault is `PublishedDurabilityUnconfirmed` (12, 13, 14, 14b; M12, M13, **M13b reverts the turn-2 defect and turns 13 red**, M14, M14b) |
| §6 | fault-point enum plus ordinal; chunk sampling first/second/final | PRESENT (final chunk by ordinal only) | PRESENT, with a `Final` chunk selector (12) |

### 2.2 §7 controls

| # | Turn 3 start | Now: test (`custody_export::tests::…` unless noted) | Mutation(s) |
|---|---|---|---|
| 1 | MISSING | `control_01_closure_refuses_a_reflog_only_commit_whose_parent_is_absent` | M01 |
| 2 | MISSING | `control_02_closure_refuses_an_unreachable_tree_whose_blob_child_is_absent` | M01 |
| 3 | MISSING | `control_03_inventory_equality_refuses_an_orphan_dropped_from_the_pack` | M03 |
| 4 | PRESENT | `control_04_a_present_and_valid_orphan_blob_seals` (positive) | none admissible in owned paths (§7) |
| 5a / 5b | MISSING | `control_05a_pre_spawn_callback_refuses_alternate_drift` / `control_05b_post_exit_callback_refuses_alternate_drift` | M05a / M05b |
| 6 | MISSING | `control_06_inventory_equality_refuses_an_extra_packed_object` | M06 |
| 7 | MISSING | `control_07_strict_indexing_refuses_a_truncated_or_corrupt_staged_pack`, plus `step_1_refuses_a_staged_pack_that_grew_past_its_recorded_length` | M07, M07b |
| 9a / 9b | MISSING | `control_09a_pre_spawn_callback_refuses_a_swapped_source` / `control_09b_post_exit_callback_refuses_a_swapped_source` (3 rows each; repair 1 adds the worktree-root row); repair 2 adds `control_09_pre_write_recheck_refuses_a_source_drifted_before_the_export` (5 rows) | M05a / M05b; M09-root; M09-prewrite |
| 10 | MISSING | `control_10a_…symlink_at_the_reserved_name`, `control_10b_…retargeted_capsule_directory`, `control_10c_a_case_fold_alias_is_refused_or_left_untouched`, `control_10d_…pre_planted_component_symlink` | M10a, M10b, M10d (10c: §7) |
| 11 | MISSING | 16 `control_11_*` tests (13 at turn 3, where this row said 14): budget ceilings, plaintext budget, sink chunk bytes / chunk count / per-artifact, ledger, Git-child reservations, post-exit re-measure (unit and read-only child), alternates bound, end-to-end scratch budget against an independent census (repair 1), logical bound apart from allowances (repair 1), artifact count, canonical preflight before derive, and the `verify-pack` bound, unit and exact edge (repair 1) | M11a–M11m; M11n–M11r |
| 12 | MISSING | `control_12_every_pre_commit_fault_leaves_no_seal` (31 fault sides + 6 chunk samples) | M12 |
| 13 | MISSING | `control_13_a_post_seal_fault_is_published_durability_unconfirmed` | M13, M13b |
| 14 | MISSING | `control_14_an_unverifiable_seal_rename_is_seal_publication_unverified` | M14 |
| 14b | MISSING | `control_14b_an_unverified_seal_target_is_seal_publication_target_unverified` | M14b |
| 15 | MISSING | `control_15_whole_receipt_equality_refuses_swapped_receipt_contexts`, `control_15_every_receipt_field_is_compared` | M15, M15-1..8 |
| 16 | MISSING | `control_16a_a_prefix_only_sealer_fails_the_exact_total_wrapper`, `control_16b_substituted_plaintext_fails_the_identity_comparison` (pack, control artifact, payload) | M16a, M16b |
| 17 | MISSING | `control_17_step_1_refuses_a_verification_input_that_is_not_the_recorded_pack` | M17 |
| 18 | MISSING | `control_18_capability_binding_refuses_before_any_write` (6 rows) | M18-unit/-run/-materialization/-generation/-inventory/-format |
| 19 | MISSING | `control_19_the_capability_refuses_a_stream_replayed_under_another_role` | M19 |
| 20a / 20b | MISSING | `control_20a_…strict_object_check` / `control_20b_…when_step_1_is_bypassed` | M20a / M20b |
| 21 | MISSING | `control_21_disjointness_preflight_refuses_every_overlap_before_any_write` (9 relations; repair 1 adds 3 non-bare worktree relations); repair 2 adds `control_21_a_renamed_worktree_is_refused_by_its_retained_identity_before_any_write` | M21; M21-root; M21-retained |
| 24a | PRESENT (untested by mutation) | doctest on `custody_capsule::CustodyCapsuleLayoutV1` | M24a |
| 24b | PRESENT (untested by mutation) | doctest on `custody_capsule::CustodyEnvelopeSealerV1` | M24b |
| 24c | PRESENT at base | doctest on `custody_capsule::CustodyEnvelopeSealReceiptV1` | M24c |
| 24 caller | PRESENT | `control_24_in_crate_caller_reaches_the_crate_private_entry_point` (compile-pass) | — |
| 25 | PRESENT at base | first doctest on `custody_capsule::CustodyEnvelopeStreamReceiptV1` | M25 |
| 30 | MISSING | `control_30_the_synthesized_git_dir_receives_no_source_config` (fresh store per arm from an immutable template) | M30 |
| 31 | MISSING | `control_31_the_caller_route_pin_reaches_admission_unchanged` | M31 |
| 32 | MISSING | `control_32_content_remeasurement_refuses_an_in_place_staging_overwrite` | M32 |
| 33 | MISSING | `control_33_seal_barrier_refuses_an_in_place_overwrite_of_a_published_artifact` | M33 |
| 34 | PRESENT (test only) | `control_34_pack_objects_is_spawned_exactly_once` | M34 |
| 35 | MISSING | `control_35_the_pack_output_allowance_bounds_the_pack_stream` | M35 |
| no-mutation observation | one test | every real-Git success test goes through `HarnessV1::run_sealed`, which compares every byte of every watched source store before and after | observation, not a control |
| four dropped 2B1 public negatives | PRESENT | `tests/custody_capsule.rs::restored_2b1_public_negatives_reject_oversized_seals_duplicate_names_and_an_empty_index` | (existing public negatives; not re-mutated) |
| deferred 2B1: generic-seal substitution compile-fail + signature reversion | MISSING | two doctests on `custody_capsule::CustodyCapsuleSealProofV1` (binding; open request) | M2B1-sig-binding, M2B1-sig-open |
| deferred 2B1: direct empty receipt | MISSING | `custody_capsule::tests::capsule_seal_proof_refuses_an_empty_receipt_population` | M2B1-empty |
| deferred 2B1: max/max+1 allocation boundaries | MISSING | `custody_capsule::tests::v1_envelope_limits_admit_max_and_refuse_max_plus_one` | M2B1-chunk, M2B1-recipients |

Controls 8a–8c, 22, 23, 26, 27a–27d, 28, 28a, 28b, and 29 are retired in this task (§7) and are not reused. The 2B3
opener receipt/metadata binding remains deferred and unreachable.

Additional real-Git tests: `the_evidence_records_the_observed_git_version_and_every_child`,
`the_evidence_records_the_admitted_git_version_not_the_minimum` (M-version),
`the_capsule_holds_exactly_the_reserved_names_plus_the_seal`, `the_sealed_pack_is_the_verified_pack`,
`an_export_through_a_pinned_alternate_store_seals`. Repair 1 adds three positives:
`an_export_from_a_non_bare_source_seals_and_leaves_its_worktree_untouched`,
`a_delta_heavy_sha1_pack_seals_within_the_verify_pack_bound`, and
`a_delta_heavy_sha256_pack_seals_within_the_verify_pack_bound`.

---

## 3. What turn 3 changed

**Production (`custody_export.rs`):** every item in §2.1 marked FIXED or ADDED. The key changes:

- the position-aware commit-point lattice;
- a retained-descriptor `VerifiedPackV1` bounded by the recorded length;
- separable step-1 and step-4 classifiers, plus `StrictPack`;
- `PlaintextNotConsumed`;
- sink pre-write ceilings;
- per-directory Git budgets re-measured after every child;
- the checked `index-pack` bound;
- bounded alternates reads;
- symmetric identity disjointness;
- the seal barrier moved into the rename's last-chance hook, with destination-directory rechecks;
- the admitted Git version in the evidence;
- a non-panicking plaintext slicer.

**`#[cfg(test)]` seams (exporter-local, §7):** three new bypasses, each isolating one control:

- `closure_step` for 3;
- `plaintext_identity_comparison` for 19;
- `seal_barrier` for 32.

The `Final` chunk selector was also added. The control-16 substitution seam now inverts the planned plaintext itself,
including the pack, so it always has the planned length. Every seam is a fixture affordance that bypasses an *outer*
layer. None is a guard.

**`custody_capsule.rs`:** two generic-seal substitution compile-fail doctests on `CustodyCapsuleSealProofV1`; the
empty-receipt and max/max+1 unit tests. No production change beyond turn 2's `pub(crate) mod sealed` and control-24
doctests.

**Unchanged from turn 2 and re-verified:** `custody_seal.rs` (crate-private accessors plus three unit tests), `lib.rs`
(`#[cfg(unix)] #[allow(dead_code)] mod custody_export;`), `tests/custody_capsule.rs` (four restored negatives).

---

## 4. Verification totals

Turn 3's results were on its final bytes, after mutation round 2 had restored and verified them. The rows that
repair round 2 re-ran show its figures, taken on the round-2 bytes after its matrix run had restored and verified
them (§12.4). Each earlier figure is kept in parentheses; the rest are turn 3's.

| Command | Result |
|---|---|
| `cargo fmt --all -- --check` | clean (repair 1 and repair 2: clean) |
| `cargo clippy --locked --offline -p bridge-core --all-targets -- -D warnings` | clean (repair 1 and repair 2: clean) |
| `cargo clippy --locked --offline --workspace --all-targets -- -D warnings` | clean (turn 3; not re-run in either repair round) |
| `git diff --check` / `git diff --cached --check` | clean (see §8) |
| `cargo test --locked --offline -p bridge-core --lib custody_export` | **62 passed**, 0 failed (repair 1: 60; turn 3: 54) |
| `cargo test --locked --offline -p bridge-core --doc` | **9 passed**, 0 failed (unfiltered; 7 compile-fail in `custody_capsule`, 2 elsewhere) |
| `cargo test --locked --offline -p bridge-core --lib custody_seal` | **4 passed** |
| `cargo test --locked --offline -p bridge-core --lib custody_capsule` | **10 passed** |
| `cargo test --locked --offline -p bridge-core --test custody_capsule` | **18 passed** |
| `cargo test --locked --offline -p bridge-core --lib` | **832 passed** (repair 1: 830; turn 3: 824); see §11.4 for a pre-existing intermittent 2B2a fixture failure |
| `cargo test --locked --offline -p bridge-core` | 17 targets, **970 passed**, 0 failed, 0 ignored (repair 1: 968; turn 3: 962) |
| `cargo test --locked --offline --workspace --all-targets --no-fail-fast` (proxy variables unset, §6) | 90 targets, **4566 passed**, 0 failed, 13 ignored (repair 1: 4564; turn 3: 4558) |
| `cargo test --locked --offline --workspace --no-fail-fast` (proxy variables unset, §6) | 106 targets, **4576 passed**, 0 failed, 13 ignored (repair 1: 4574; turn 3: 4568) |
| the two workspace commands **with** the container's proxy variables set | 8 `a2a-bridge` failures and a `bridge-api` lib hang, all reproduced on the exact predecessor (§6) |
| `cargo deny check` | **not run**: `cargo-deny` is not installed in the container (`no such command: deny`) — an unrunnable gate, not green |
| `cargo run --locked --offline -p a2a-bridge -- validate --repo-hygiene` | `repository hygiene validated` |

---

## 5. Mutation matrix

This section is the turn-3 record: 66 rows, and the turn-3 snapshot. Repair round 1 adds 7 rows (73 defined),
re-snapshots the repaired bytes, and re-runs 24 rows; see §11.3. Repair round 2 adds 2 rows (75 defined),
re-snapshots its bytes, and re-runs 13 rows; see §12.3.

**Harness:** `.git/a2a-bridge/mutation/matrix.py` (untracked; survives the container). Log `matrix.log`, per-mutation
records `results.jsonl`, raw cargo output `output-<id>.txt`, pre-matrix `baseline.txt`, snapshot
`snapshot/` with `manifest.json`.

**Rules the harness enforces:**

- a mutation is applied only if each of its target strings occurs exactly once in the snapshot (`matrix.py check`: 66/66);
- a `pending.json` marker is written before the mutation is applied, and removed only after the file has been
  rewritten from the snapshot, `fsync`ed, stamped with a fresh mtime (`os.utime` now), and re-hashed equal to the
  snapshot;
- any later invocation that finds the marker restores first;
- SIGINT/SIGTERM/SIGHUP restore before exit;
- the matrix ran in the **foreground only**, in single `timeout 590` calls with an internal 400–450 s budget and a
  240 s cap per cargo run; no background job was ever started.

**Verdict rule:** `FLIPPED` requires all of the following:

- the crate compiled, and at least one test was selected;
- every expected-red test failed, and every named expected-green test passed;
- for the per-row mutations, the expected row text appeared.

For the doctest mutations, the unfiltered `--doc` run must fail **exactly** the one targeted doctest, identified by
item and position. Anything else is `INADMISSIBLE`.

**Commands:** `python3 .git/a2a-bridge/mutation/matrix.py snapshot && … check && timeout 590 … run --budget 450`,
then `… verify` and `… summary`.

**Rounds:**

- Round 1 (05:34–05:39Z): 66/66 FLIPPED.
- Between rounds, control 10c's case-sensitive arm was switched to `run_sealed`, so it also makes the no-mutation
  observation.
- Round 2 (06:02–06:07Z), against the final bytes after a fresh snapshot: **66 defined, 66 run, 66 FLIPPED,
  0 INADMISSIBLE, 0 not run**.

**Source equals snapshot after the matrix:** `matrix.py verify` → `VERIFY OK: 8 files equal their snapshot`; no
`pending.json` exists. Snapshot SHA-256, which equal the staged bytes:

| File | SHA-256 |
|---|---|
| `crates/bridge-core/src/custody_export.rs` | `8f0383422584d61a57df6ccfc765dd9a540d1e7551d3163e412512de2a8ce753` |
| `crates/bridge-core/src/custody_export_tests.rs` | `d6aa58fd2b79db4f4b4ad9252ab4f5f5a47da0c119f6aea33b37138f6c70a616` |
| `crates/bridge-core/src/custody_capsule.rs` | `68b21d1bbcf7f57cafd2909e47e3a97cf8f0cc115911e78eb925767970811f91` |
| `crates/bridge-core/src/custody_seal.rs` | `b14b8c4d22ee35474c820007bbe050494e2fc73a29d18ac15301d38bbc435e46` |
| `crates/bridge-core/src/lib.rs` | `23539c6dcc373edf88816688672f0b76d6e338742a94735bd0499d3a1befbbcf` |
| `crates/bridge-core/tests/custody_capsule.rs` | `0cc7d659c07b162ab884a43f9716e86e26dc90dfe4043d3058104f522f796d95` |
| `crates/bridge-core/src/custody_git.rs` (read-only; equals `HEAD`) | `78837e41acfc442860eda2c59bbbc5adcf5395259c0e7374b57a6991b31d5061` |
| `crates/bridge-core/src/fs_custody.rs` (read-only; equals `HEAD`) | `142cf2714e0de806fe3a41198353dc1f77b89c85f31d47e3d057e87b27f217a6` |

**Outcome kinds under mutation (round 2):**

- **Wrong success (26):** a capsule sealed where a refusal was required.
- **Doctest compiles (6):** the one targeted compile-fail doctest compiled.
- **Wrong refusal or boundary (34):** a max + 1 input was admitted, a max input was refused, a post-commit fault
  returned `Err` while the seal existed, a pre-commit fault left a seal, or a typed refusal changed type behind a
  layered guard. Each is listed below.

| ID | Control | Guard mutated | Red | Named green | Verdict | Outcome under mutation |
|---|---|---|---|---|---|---|
| M01 | 1, 2 | §5 step 3 closure comparison deleted (`let _ = compare_closure(..)`) | 01, 02 | 03, 04 | FLIPPED | wrong success |
| M03 | 3 | §5 step 2 equality, missing direction | 03 | 06, 04 | FLIPPED | wrong success |
| M06 | 6 | §5 step 2 equality, extra direction | 06 | 03, 04 | FLIPPED | wrong success |
| M07 | 7 | §5 step 1: a failed `index-pack` accepted | 07 | 17, 20a | FLIPPED | `VerificationInputMismatch` instead of `StrictPack` (layered: see below) |
| M07b | 7 | §5 step 1 stdin bound = recorded length (→ `u64::MAX`) | `step_1_…grew…` | 07, 17 | FLIPPED | the grown pack is streamed; caught one layer later as `VerificationInputMismatch` |
| M17 | 17 | stdin evidence vs verified-pack identity | 17 | 07, 34 | FLIPPED | wrong success (pack B verified, pack A sealed) |
| M20a | 20a | step-1 strict-object classifier | 20a | 20b, 07 | FLIPPED | `StrictPack` instead of `StrictObjectCheck` |
| M20b | 20b | step-4 fsck checks deleted | 20b | 20a, 04 | FLIPPED | wrong success |
| M05a | 5a, 9a | pre-spawn identity callback | 05a, 09a | 05b, 09b | FLIPPED | wrong success |
| M05b | 5b, 9b | post-exit identity callback | 05b, 09b | 05a, 09a | FLIPPED | wrong success |
| M10a | 10a | no-replace publication (reserved name clobbered first) | 10a | 10d, 10b | FLIPPED | wrong success |
| M10b | 10b | barrier destination-directory recheck | 10b | 33, 10a | FLIPPED | wrong success (sealed while `control/` names a decoy) |
| M10d | 10d | create-new, no-follow directory (path-following open on `EEXIST`) | 10d | 10a | FLIPPED | wrong success (artifacts written through the symlink) |
| M11a | 11 | budget validation, chunk bytes off by one | `11_budget_ceilings` | — | FLIPPED | max + 1 admitted |
| M11b | 11 | plaintext chunk-count ceiling off by one | `11_plaintext_budget` | — | FLIPPED | max + 1 admitted |
| M11c | 11 | sink chunk-bytes pre-write check | `11_sink_chunk_bytes` | `11_sink_chunk_count` | FLIPPED | over-limit bytes written; the 2B1 validator refuses only after the write |
| M11d | 11 | sink chunk-count pre-write check | `11_sink_chunk_count` | `11_sink_chunk_bytes` | FLIPPED | same, for the chunk count |
| M11e | 11 | sink per-artifact ciphertext ceiling | `11_sink_per_artifact` | — | FLIPPED | same, for the per-artifact ceiling |
| M11f | 11 | ledger limit off by one | `11_scratch_ledger`, `11_git_child_reservations`, `11_scratch_budget_end_to_end` | — | FLIPPED | wrong success at U − 1 |
| M11g | 11 | `index-pack` bound drops the reverse index | `11_git_child_reservations` | — | FLIPPED | exact bound changes |
| M11h | 11 | re-measure byte bound off by one | `11_post_exit_remeasure` | — | FLIPPED | max + 1 admitted |
| M11i | 11 | re-measure after `verify-pack` (read-only child) | `11_a_read_only_child` | — | FLIPPED | stray file not caught before step 2 (the armed step-2 fault fires) |
| M11j | 11 | artifact-count ceiling (`>` → `>=`) | `11_artifact_count` | — | FLIPPED | max refused |
| M11k | 11 | canonical preflight off by one | `11_canonical_manifest` | — | FLIPPED | max + 1 admitted |
| M11l | 11 | derive swapped ahead of preflight | `11_canonical_manifest` | — | FLIPPED | derive entered after an over-limit preflight |
| M11m | 11 | alternates-file bound off by one | `11_alternates_file` | — | FLIPPED | max + 1 admitted |
| M12 | 12 | commit point moved ahead of binding construction | 12 | — | FLIPPED | a pre-commit fault leaves a seal |
| M13 | 13 | post-seal sync fault as `Err` | 13 | 14, 14b | FLIPPED | `Err` with the seal present |
| M13b | 13 | **turn-2 defect reverted:** `(SealRename, After)` as `Err` | 13 | 12 | FLIPPED | `Err` with the seal present |
| M14 | 14 | unverified-rename arm reported as no seal | 14 | 14b, 13 | FLIPPED | `Err` claiming no seal |
| M14b | 14b | target-unverified arm reported as success | 14b | 14, 13 | FLIPPED | wrong success |
| M15 | 15 | whole-receipt comparison skipped | `15_whole_receipt_equality` | `15_every_receipt_field` | FLIPPED | wrong success |
| M15-1…8 | 15 | comparator ignores one field each: `artifact_name`, `manifest_digest`, `capsule_format`, `sealing_tool`, `sealing_tool_version`, `recipients`, `ciphertext_length`, `ciphertext_sha256` | `15_every_receipt_field` (the named row) | — | 8 × FLIPPED | exactly that field's row red (`receipt field row <field>:`) |
| M16a | 16a | exact-total wrapper `finish` | 16a | 16b | FLIPPED | wrong success (a truncated artifact sealed) |
| M16b | 16b | plaintext SHA-256 comparison | 16b | 16a | FLIPPED | wrong success |
| M18-unit/-run/-materialization/-generation/-inventory | 18 | one binding comparison each | 18 (the named row) | — | 5 × FLIPPED | wrong success |
| M18-format | 18 | one-object-format comparison | 18 (`mixed formats` row) | — | FLIPPED | refused only after writes (`ObjectPresence`), not before |
| M19 | 19 | captured-stream SHA-256 binding | 19 | — | FLIPPED | wrong success |
| M21 | 21 | disjointness preflight disabled | 21 | — | FLIPPED | wrong success (writes into a source store) |
| M30 | 30 | source config copied into `work/source-git/` | 30 | — | FLIPPED | wrong success after a lazy fetch into the source store |
| M31 | 31 | trust-on-first-use fallback on `DigestMismatch` | 31 | — | FLIPPED | wrong success; the mismatched route was executed |
| M32 | 32 | content remeasurement | 32 | 33 | FLIPPED | wrong success (stale digest sealed) |
| M33 | 33 | barrier artifact re-hash | 33 | 10b, 32 | FLIPPED | wrong success (stale digest sealed) |
| M34 | 34 | a second, discarded `pack-objects` run | 34 | 17 | FLIPPED | spawn counter 2 while 17 stays green |
| M35 | 35 | `stdout_limit = A + 1` | 35 | — | FLIPPED | byte B + 1 written; ledger refusal instead of `StdoutLimit` |
| M-version | §4.2 evidence | version from the runner replaced by the minimum | `…admitted_git_version_not_the_minimum` | — | FLIPPED | records 2.54.0 instead of the admitted 2.99.1 |
| M2B1-empty | deferred 2B1 | nonempty-population guard | `capsule_seal_proof_refuses_an_empty_receipt_population` | — | FLIPPED | `InvalidInput` from the generic constructor instead of `MissingArtifact` |
| M2B1-chunk | deferred 2B1 | envelope chunk bytes off by one | `v1_envelope_limits_…` | — | FLIPPED | max + 1 admitted |
| M2B1-recipients | deferred 2B1 | recipient count off by one | `v1_envelope_limits_…` | — | FLIPPED | max + 1 admitted |
| M24a | 24a | `mod custody_export` → `pub mod` | `CustodyCapsuleLayoutV1` doctest | all 8 others | FLIPPED | doctest compiles |
| M24b | 24b | `: sealed::Sealed` removed from `CustodyEnvelopeSealerV1` | `CustodyEnvelopeSealerV1` doctest | all 8 others | FLIPPED | doctest compiles |
| M24c | 24c | `CustodyEnvelopeSealReceiptV1::new` made `pub` | `CustodyEnvelopeSealReceiptV1` doctest | all 8 others | FLIPPED | doctest compiles |
| M25 | 25 | source `finish` reconnected to the ciphertext receipt | `CustodyEnvelopeStreamReceiptV1` doctest #1 | all 8 others | FLIPPED | doctest compiles |
| M2B1-sig-binding | deferred 2B1 | binding signature reverted to `&CustodySealV1` (plus `Deref` so crate callers compile) | `CustodyCapsuleSealProofV1` doctest #1 | all 8 others | FLIPPED | doctest compiles |
| M2B1-sig-open | deferred 2B1 | open-request signature reverted to `&CustodySealV1` | `CustodyCapsuleSealProofV1` doctest #2 | all 8 others | FLIPPED | doctest compiles |

**Layered guards whose mutation flips by refusal type rather than by wrong success.** Each is admissible: the control
asserts its own typed refusal, and the mutation removes exactly that guard.

- **7 (M07):** no index can exist for a truncated or corrupt pack. With strict indexing accepted, control 17's
  identity comparison is the next layer, so a wrong success is unreachable by construction.
- **M07b:** likewise, control 17's comparison catches the grown pack one step later.
- **20a (M20a):** the step-1 rejection still refuses, but as a generic `StrictPack`. §5 requires the dedicated class.
- **M18-format:** later Git children still refuse a foreign-format object, but only after writes. Control 18 requires
  refusal before any write.
- **M11c–M11e:** the 2B1 validator still refuses, but only after the over-limit bytes were written and charged. §3
  requires "before allocation or file creation".

---

## 6. Exclusions and their mechanisms

1. **Container HTTP proxy.** The container sets `HTTP_PROXY` and `HTTPS_PROXY`. The `a2a-bridge` integration tests
   and the `bridge-api` unit tests talk to loopback HTTP mocks through `reqwest`, which honors those variables. With
   them set:
   - 8 tests fail: `e2e_registry::api_entry_resolves_and_serves_through_registry`,
     `integration_delegate::delegate_skill_round_trips_through_peer`,
     `integration_fanout::fanout_merges_kiro_and_peer_with_terminal`,
     `mcp_spawn::workflow_stats_get_reads_live_mcp_owner_active_and_terminal_wal_rows`,
     `models_cli::{non_success_api_status_is_provider_error_and_body_is_not_exposed, malformed_api_response_has_response_parse_category}`,
     and `r2f0b_production_wiring::real_api_message_delta_reaches_production_{workflow,direct}_attempt_owner`;
   - the `bridge-api` lib tests fail and hang (`backend::tests::cancellation_between_round_terminal_and_successor_publication_prevents_post`
     runs past 60 s).

   **Attribution:** the same 8 failures and the same `bridge-api` hang reproduce on the **exact predecessor
   `742e0a60`**, extracted with `git archive` into `/tmp/base-742e0a60` and built into `/tmp/target-base`, in this
   same container with the proxy set. With the variables unset (`env -u HTTP_PROXY -u HTTPS_PROXY -u http_proxy -u
   https_proxy`), both workspace gates pass completely (§4). No affected crate depends on `custody_export`, which is
   crate-private and production-unreachable.
2. **`cargo deny check`:** unrunnable. `cargo-deny` is not installed in the image, so this gate is not green here. No
   dependency, feature, or `Cargo.lock` change was made (`git status` shows no `Cargo.*` change).
3. **Host macOS lane:** **not executed in this container**. The controller ran it on the turn-3 tree (§10). It is
   the only lane that exercises control 10c's refusal arm (a case-insensitive filesystem).
4. **Native Linux ext4 lane (GitHub Actions ubuntu):** **not executed here**. This lane is OrbStack **overlayfs**,
   which §9 says does not substitute for the native identity-drift control. Controls 5a/5b/9a/9b/10b ran and flipped
   on overlayfs, and still require the native ext4 run. 2B2a's `#[cfg(test)]` ext4 classifier was not invoked here.
5. **uid 0:** the container runs as root, and 2B2a's production route profile refuses uid 0. Every real-Git test
   therefore admits Git through 2B2a's `#[cfg(test)]` `TestSystem` or `TestFixture` profiles. The production
   profile's non-root admission is 2B2a's and is not re-exercised here.

---

## 7. Honest limits

**§15 honest limits, restated verbatim (owner ruling 2026-09-24: a hostile same-user or privileged check-to-use racer
is out of scope; detection remains mandatory):**

- **HL1:** absolute paths Git requires, and the source `GIT_OBJECT_DIRECTORY`/alternate lookups, are resolved by Git
  by path. A same-user swap between recheck and lookup is detected afterwards, not prevented, and §5's content proof
  keeps a substituted store from yielding a wrong pack.
- **HL2:** an identical substitution (empty, same owner and mode) is undetectable. The substitute is necessarily a
  directory inside the retained parent at open time.
- **HL3:** a privileged replace-and-restore entirely inside one child's window. A persisting privileged replacement is
  detected after the child.

**Production-unreachable by design (§2).** `export_capsule_v1` is `pub(crate)` in a private module (control 24a).
`CustodyCaptureCapabilityV1` has only the crate-private `#[cfg(test)]` fixture mint, `from_fixture_quiescence`, and the
only sealer is the in-crate `#[cfg(test)]` `FixtureSealerV1`. So the entry point has no production caller. The wiring
slice supplies the production quiescence mint, the sealer, and where the route digest comes from. The fixture envelope
makes no confidentiality claim, and `work/` holds plaintext only because 2B2 makes none.

**Further limits of this implementation's evidence:**

- **Control 4** is a positive control. Its guard, `--no-dangling`, lives in 2B2a's closed argv (`custody_git.rs`,
  read-only), so no admissible mutation exists in the owned paths. Every sealing test exercises it, because the fixture
  always carries the orphan blob.
- **Control 10c** (case-fold alias) discriminates only on a case-insensitive filesystem. Here it asserts only that the
  alias is neither followed nor replaced. There is no mutation for it on this lane.
- **Control 21's** nine relation rows are all refused through the canonical-path comparison. The directory-identity
  layer compares in two directions, updated in repair round 2 (§12):
  - The forward comparison checks scratch ancestors against each pinned descriptor's retained identity. It is
    independently discriminated by the stale-pin row: no path comparison can see that the scratch root lies inside a
    renamed worktree (M21-retained).
  - The reverse comparison checks the ancestors of each pinned path against the scratch root. It is reachable only
    through an alias that canonicalization cannot see, such as a bind mount, so it is not independently
    discriminated. It walks a pinned path, because `fs_custody` offers no `..` walk from a retained descriptor. A
    stale path cannot lead it to admit a write: the recheck immediately before the first scratch write refuses a path
    that no longer resolves to its descriptor (M09-prewrite), and an empty scratch root cannot contain a pinned
    directory.

  M21 disables the whole preflight.
- **Controls 5b and 9b** place their "during the child" drift inside the pre-spawn callback with the pre-spawn check
  bypassed, so it is in place for the child's whole lifetime. 2B2a exposes no mid-child hook to the exporter.
- **Control 14b** and control 13's `ParentSyncAmbiguous` row use a `#[cfg(test)]` publication-outcome override:
  `fs_custody` cannot produce `TargetIdentityUnverified` or `ParentSyncAmbiguous` on demand. Control 14 uses
  `fs_custody`'s real `UnlinkSourceOnly` rename fault.
- **Shallow, grafted, and promisor detection** is supplied by the capture capability, whose fixture mint reports
  none. Control 30 bypasses that outer layer and proves the synthesized git directory. The production detector
  belongs to the wiring slice's mint.
- **The no-mutation observation** snapshots every byte of every watched store: objects, config, alternates, and for
  control 30 the promisor. The fixture sources have no refs, so "ref bytes" are vacuously equal. Most are bare, but
  since repair round 1 `an_export_from_a_non_bare_source_seals_and_leaves_its_worktree_untouched` watches a real
  worktree, so "worktree bytes" are observed there.
- **Read-only path walks.** The post-exit git-directory re-measure and the scratch-root emptiness check enumerate by
  path (`std::fs::read_dir`). They only detect; they never write, and they fall within the HL1 class.
- **The seal barrier** runs inside `fs_custody`'s last-chance hook, which can only return `FsCustodyError`. The typed
  `SealBarrier` refusal is carried out through a `RefCell`, the same pattern the runner callbacks use.

---

## 8. Owned paths and staged changes

Only task §8 owned paths changed. No `Cargo.lock`, dependency, feature, CLI, config, store, runtime, container, operator,
or 2B3 path is touched. The mutation harness and its artifacts live under `.git/` and are not part of the diff. Turn 3
changed these paths, committed as `87570241`:

```text
crates/bridge-core/src/custody_capsule.rs           (modified)
crates/bridge-core/src/custody_export.rs            (new)
crates/bridge-core/src/custody_export_tests.rs      (new)
crates/bridge-core/src/custody_seal.rs              (modified)
crates/bridge-core/src/lib.rs                       (modified)
crates/bridge-core/tests/custody_capsule.rs         (modified)
docs/superpowers/reviews/2026-09-25-adr0041-slice2b2-implementation-handoff.md (new)
```

Turn 3's exact `git diff --cached --stat` and `git diff --cached --check` results were reported in that turn's final
message.

Repair round 1 changed only `crates/bridge-core/src/custody_export.rs`, `crates/bridge-core/src/custody_export_tests.rs`,
and this handoff (§11.5), committed as `7842f620`. Repair round 2 changes the same three paths (§12.5), and lands in
the commit that contains §12.

## 9. What remains for the controller

**Committed:**
- the implementation, `87570241`;
- the `#[cfg(unix)]` accessor gates, `91d0bf13`;
- repair round 1, `7842f620`.

Repair round 2 (§12) lands in the commit that contains that section.

**Done:** the macOS host lane (§10), run by the controller on the turn-3 tree: the §9 gate list, control 10c's
refusal arm on APFS, the Git version, and the admitted route.

**Remaining gates:**
1. The native Linux ext4 lane (GitHub Actions ubuntu CI on the PR), for the identity-drift controls.
2. `cargo deny check`, in CI, where `cargo-deny` is installed.
3. The next implementation review round, under the §9 two-admitted-round cap.

## 10. Controller macOS host lane (2026-09-26)

The controller ran the macOS host lane on the turn-3 staged tree (`refs/wip/2b2-edit-turn3`, tree `7b13113d`):
Darwin 25.6.0, APFS (case-insensitive), a non-root user, and Command Line Tools Git.

- **Found:** two tests failed with `RouteRefusal("executing user can write a Git route component")`:
  `control_31_the_caller_route_pin_reaches_admission_unchanged` and
  `the_evidence_records_the_admitted_git_version_not_the_minimum`.
  - Both built a `TestFixture` route whose script (`0755`) and anchor (`0700`) the owner can write.
  - As uid 0 in the container, 2B2a's fixture profile checks only group and other write bits, so the route was
    admitted. As a non-root user, `faccessat(W_OK)` correctly reports the owner-writable component.
  - The production refusal is correct; the fixtures were not portable. The other tests use the root-owned system Git
    through `TestSystem` and passed.
- **Repair (test-only):** `SealedRouteAnchorV1` seals the script and anchor to `0500`, the same idiom as 2B2a's
  `FixtureRoute`, and reopens the anchor on drop so the temporary directory is removed. It is used by the version
  test and by `marker_route`.
  - Both tests still discriminate the same way: the pin mismatch refuses before any marker, and the version comes from
    the admitted route. The only change is that the fixture no longer trips the writability guard.
  - No production file changed.
- **Result on the repaired tree:** `cargo fmt --check` passes, and warnings-denied Clippy for `bridge-core` with all
  targets is clean.
  - `cargo test --workspace --no-fail-fast` with `CARGO_INCREMENTAL=0` exited 0: 4,566 passed, 0 failed, 13 ignored.
  - The `bridge_core` doctests passed 9/9.
  - Control 10c ran its case-insensitive refusal path and passed.
  - No fixture anchor directory leaked.
- **Remaining lanes:**
  - native Linux ext4: the GitHub Actions ubuntu CI on the PR;
  - `cargo deny`: CI.

## 11. Repair round 1 — Sol implementation review (2026-09-26)

**Review:** an independent Sol review of `87570241` (HEAD `91d0bf13` adds only the `#[cfg(unix)]` accessor gates).
It found three WRONG · MATERIAL · BLOCKER findings, which the controller verified and accepted. Its two SMELLs are
DEFERRED and already listed in §7: control 5b/9b mutation timing, and control 21's identity layer. They were not
worked.

**Base and scope:** HEAD `91d0bf13`, clean working tree. Only `custody_export.rs`, `custody_export_tests.rs`, and
this handoff changed. `custody_git.rs` and `fs_custody.rs` equal `HEAD` (`git diff --quiet HEAD --`), with the §5
SHA-256 values. The task is still revision 11, SHA-256 `b9f1215a…d814e9`.

**Method: RED first.** Every failing control was written against the unrepaired API. It ran with
`custody_export.rs` still byte-equal to `91d0bf13`. Raw outputs are in `.git/a2a-bridge/mutation/`:
- RED: `repair1-red.txt`. 58 tests were selected: **51 passed, 7 failed**, exactly the targeted controls.
- GREEN: `repair1-green.txt`, **60 passed**.

Between RED and GREEN the test file changed only in plumbing the repaired API needs:
- the fixture mint also receives the repository root;
- the reservation unit test follows the split constants;
- the logical-bound control gains a `const` fixture-validity assertion;
- two tests are added: the bound's unit test and the non-bare positive;
- rustfmt reflowed the changed lines.

### 11.1 Findings, RED evidence, and fixes

**W1: a scratch root inside a non-bare source worktree was admitted and wrote into the source.** The capability
pinned only the git directory, its object store, and its alternates. So `repo/custody`, beside `repo/.git`, matched
no protected path.

- **RED:** control 21 gained three non-bare relations, and controls 9a and 9b gained a worktree-root row. The RED
  output, verbatim:
  - `ScratchInsideSourceWorktree: wrong success: a capsule sealed; 15 source paths created, first …/repo/custody/capsule/control/capsule-index.json.enc`
  - `ScratchIsSourceWorktree: wrong success: a capsule sealed; 15 source paths created, first …/wt/capsule/control/capsule-index.json.enc`
  - `SourceWorktreeInsideScratch: wrong success: a capsule sealed; 0 source paths created`
  - 9a and 9b: `SwapSourceWorktreeRoot: wrong success: a capsule sealed`

  The first row is the reviewer's scenario. `repo/custody` is empty and owner-private, so that row runs the
  production preflight with no bypass. The other two use a `--separate-git-dir` clone, whose worktree holds no git
  directory to trip over. Without that layout they would not be distinguishable from the git-directory relation.
  The emptiness bypass now applies only to rows whose scratch root already holds source bytes, and control 21
  reports every failing row.
- **Fix:** `CustodyCaptureCapabilityV1` gains a required `source_repository: PinnedDirectoryV1`. It is the worktree
  of a non-bare source, or the git directory of a bare one. The fixture mint takes it. It is the first entry of
  `protected_paths()`, so the existing loop compares it by canonical path and by directory identity in both
  directions. `recheck()` also checks it, so the pre-spawn and post-exit callbacks recheck it. The refusal names a
  "protected source path". The production mint must supply this pinned root; the capability shape now fixes that.
- **GREEN:** all nine control-21 rows are refused with `ScratchPreflight` before any write. The 9a and 9b worktree
  rows refuse with `IdentityDrift`, and no pack is produced. A new positive,
  `an_export_from_a_non_bare_source_seals_and_leaves_its_worktree_untouched`, seals with the scratch root outside the
  worktree, and the worktree bytes are unchanged.
- **Mutations:** M21-root and M09-root (§11.3).

**W2: both `git init` runs left their `HEAD` and `config` bytes out of the ledger.** Each init reserved only
`9 × 64 KiB`. The re-measure compared its logical bytes with that same allowance and never charged them.

- **RED:** `control_11_scratch_budget_end_to_end_admits_max_and_refuses_max_plus_one` now checks the ledger against
  an independent census, `scratch_census`. It walks the scratch root, shares no code with the ledger, counts every
  file and directory, and sums every file length. The RED output, verbatim:
  - `census: the ledger reports 2366451 bytes, but the scratch root holds 7245 logical bytes in 36 entries, 2366541 bytes by §3`
  - `exact-U arm (budget 2366451): wrong success: a capsule sealed; census footprint 2366541`
  - `U - 1 arm (budget 2366540, one byte under the census): wrong success: a capsule sealed`
  - `control_11_a_git_directory_is_held_to_its_logical_bound_not_its_entry_allowances`:
    `wrong success: a capsule sealed`. Here `config` grew by 8 KiB after the source init and was absorbed by the
    allowances.

  The 90-byte gap has two parts:
  - 178 bytes of `HEAD` and `config` were never charged: 2 × (23 + 66) for SHA-1;
  - 88 bytes were over-reserved for the index and never reconciled: 40 + 8·N with N = 6.
- **Fix:**
  - `INIT_BARE_LOGICAL_BYTES_V1` (4 KiB) is reserved beside the nine allowances before each init.
  - `GitDirectoryBudgetV1` keeps `logical_bytes` and `entries` apart.
  - `index-pack` reserves `INDEX_PACK_ENTRIES_V1` (3) plus `index_pack_logical_bound`, the renamed
    `index_pack_reservation` without its entries.
  - `remeasure_git_directory` first holds the measured logical bytes to the logical bound, never to the allowances.
    It then releases the unused part of the bound from the ledger and sets the budget to the measured bytes.
  - The re-measure after `index-pack` is reconciled the same way. The census equality requires it: the §3
    version-2 index bound exceeds the index Git writes by 40 + 8·N bytes when no offset needs the 64-bit table.
  - Every bound is still reserved in full before its child runs, so no Git write goes unreserved.
- **GREEN:** the ledger reports 2,366,541 bytes, equal to the census: 7,245 logical bytes in 36 entries. The exact-U
  arm seals within its census, and the U − 1 arm refuses with `ScratchLedger`. The logical-bound control refuses with
  `ScratchLedger` naming `source-git`.
- **Figures changed:** `INIT_BARE_RESERVATION_V1` (`9 × 64 KiB`) is gone. An init is now 9 entries plus a 4 KiB
  logical bound, reconciled to the measured bytes: 89 for SHA-1 and 125 for SHA-256 on this lane.
- **Mutations:** M11n, M11o, and M11p.

**W3: the shared 128-byte line estimate refused valid SHA-256 packs at `verify-pack -v`.**

- **RED:**
  - `a_delta_heavy_sha256_pack_seals_within_the_verify_pack_bound`: `the fixture export seals: Git(StdoutLimit { limit: 273152 })`.
    The fixture has 2,006 objects: 2,000 related blobs from one `fast-import` stream, plus the standard six.
  - The exact edge, `control_11_verify_pack_output_admits_its_exact_bound_and_refuses_one_byte_more`:
    `exact bound (17698 bytes): refused with Git(StdoutLimit { limit: 17152 })` and
    `bound + 1 (17699 bytes): refused with Git(StdoutLimit { limit: 17152 })`.
  - `a_delta_heavy_sha1_pack_seals_within_the_verify_pack_bound` passed both before and after the repair; it is a
    positive control.
  - A lane probe outside the suite, with Git 2.54.0 and the same 2,000 blobs:
    - SHA-256: 302,565 bytes, 1,960 of them delta rows, against the old 272,384 limit;
    - SHA-1: 207,334 bytes.
- **Fix:** `verify_pack_stdout_limit(objects, format)`, with checked arithmetic; an overflow is a typed `Io`
  refusal. Per object, it allows:
  - `2·W + 83` bytes for the widest delta row. That is two hex object names, the `%-6s` type, three 20-digit
    `uintmax_t` fields, a 10-digit `%u` depth, six separators, and a newline.
  - 56 bytes for the widest `chain length = %d: %lu objects` line, of which there is at most one per object.

  On top of that, a 16 KiB floor covers the `non delta` line and the final `<pack>: ok` line. So SHA-1 is 219 bytes
  per object and SHA-256 is 267. The runner fixes `LC_ALL=C`, so these are Git's untranslated formats. The other
  listings keep the 128-byte estimate: each of their rows carries one object name, at most `W + 29` bytes.
- **GREEN:**
  - Both delta-heavy fixtures seal, and each asserts that at least 90% of its objects are delta rows.
  - The SHA-256 fixture's `verify-pack` output exceeds `128·N + 16 KiB`, so the fixture is valid, and it fits the
    new bound.
  - The exact edge uses a sealed fixture route that prints a prepared file for `verify-pack` and runs the lane Git
    otherwise. 17,698 bytes seals; 17,699 refuses with `StdoutLimit { limit: 17698 }`. The test computes the bound
    itself, so it does not inherit a mutated bound.
  - `control_11_verify_pack_bound_is_format_aware_and_checked` pins the widths against Git's own formats at their
    widest values, and checks the overflow refusal.
- **Mutations:** M11q and M11r.

### 11.2 Controls added or changed

| Control | Test | Kind |
|---|---|---|
| 21 | `control_21_…` gains `ScratchInsideSourceWorktree`, `ScratchIsSourceWorktree`, and `SourceWorktreeInsideScratch`, and now reports every failing row | discriminating (M21, M21-root) |
| 9a / 9b | `control_09a_…` and `control_09b_…` gain `SwapSourceWorktreeRoot`. The worktree is replaced and `.git` is moved across intact, so only the root's identity changes | discriminating (M05a/M05b, M09-root) |
| 11 | `control_11_scratch_budget_end_to_end_…`: census equality, exact-U, and U − 1 | discriminating (M11f, M11n, M11o) |
| 11 | `control_11_a_git_directory_is_held_to_its_logical_bound_not_its_entry_allowances` (new) | discriminating (M11p) |
| 11 | `control_11_git_child_reservations_are_exact_and_bounded`: the split constants | discriminating (M11f, M11g) |
| 11 | `control_11_verify_pack_output_admits_its_exact_bound_and_refuses_one_byte_more` (new) | discriminating (M11q, M11r) |
| 11 | `control_11_verify_pack_bound_is_format_aware_and_checked` (new) | unit |
| positive | `a_delta_heavy_sha1_…`, `a_delta_heavy_sha256_…`, `an_export_from_a_non_bare_source_…` (new) | positive; M11q turns the SHA-256 one red |

### 11.3 Mutation matrix, repair round 1

**Harness changes:**
- `text` may be a list, and every entry must appear;
- the M11i and M30 targets follow the repaired call text: the re-measure call passes the ledger, and the init call
  passes `&mut source_budget`;
- seven rows are added.

73 rows are defined, and `matrix.py check` finds every target unique. The turn-3 results are kept as
`results.round2-turn3.jsonl`.

**Snapshot of the repaired bytes.** These hashes equal the staged bytes.

| File | SHA-256 |
|---|---|
| `crates/bridge-core/src/custody_export.rs` | `0d47d8db5d285bff70bfc3013e200241c4107d31dbc96627241aad734158bb09` |
| `crates/bridge-core/src/custody_export_tests.rs` | `d6748c47591ac5386b65ba6fbfacfdba7540f852c9406c09cc1f414e2b6b2920` |
| `crates/bridge-core/src/custody_seal.rs` (as at `91d0bf13`) | `1c474087735df94f4d8229bd4f1eb279dd7eb23bf7b87c1ca8c75bb571a2b887` |
| `crates/bridge-core/src/custody_capsule.rs`, `lib.rs`, `tests/custody_capsule.rs`, `custody_git.rs`, `fs_custody.rs` | unchanged from the §5 table |

**Run:** two foreground `timeout 590` calls, with an internal budget of 450 s and a 240 s cap on each cargo run. No
background job was started. The run covered the 7 new rows plus the 17 existing rows that are affected or that
touch a changed function: M01, M05a, M05b, M11f, M11g, M11h, M11i, M11j, M11l, M12, M20b, M21, M30, M31, M34, M35,
and M-version. **24 of 24 FLIPPED, 0 INADMISSIBLE.**

Each mutation was restored byte-exactly with a fresh mtime. `matrix.py verify` then reported
`VERIFY OK: 8 files equal their snapshot`, and no `pending.json` exists. The other 42 rows were not re-run. Their
targets and controls are untouched by this repair, and their last verdict is FLIPPED, from turn 3 round 2 (§5).

| ID | Guard mutated | Red | Named green | Outcome under mutation |
|---|---|---|---|---|
| M21-root | the repository root dropped from `protected_paths()` | 21 (all three worktree rows, by text) | non-bare positive | wrong success: two rows write 15 paths into the worktree |
| M09-root | the repository-root recheck dropped from `recheck()` | 9a, 9b (`SwapSourceWorktreeRoot`) | 5a, 5b, 21, non-bare positive | wrong success |
| M11n | the init logical reservation dropped, so only allowances are charged | end-to-end census (census, exact-U, U − 1) | logical-bound control, reservations unit | the ledger is 8,192 bytes under the census; seals at census − 1 |
| M11o | the reconciliation release dropped | end-to-end census (census row) | logical-bound control, reservations unit | the ledger is 8,102 bytes over the census |
| M11p | the logical bound conflated with the entry allowances in the re-measure | logical-bound control | end-to-end census, re-measure unit | wrong success: an 8 KiB `config` growth is absorbed |
| M11q | `verify-pack` bound reverted to `stdout_limit_for_objects` | SHA-256 delta-heavy, exact edge | SHA-1 delta-heavy, bound unit | `StdoutLimit { limit: 273152 }`; the exact bound is refused at 17,152 |
| M11r | the `verify-pack` bound loosened by one byte at the call site | exact edge (the `bound + 1` arm) | SHA-1 and SHA-256 delta-heavy, bound unit | wrong success at 17,699 bytes |

M11f's expected-red end-to-end test is now the census test, and it still turns red. M05a and M05b also turn the
new 9a and 9b worktree rows red with their callbacks. Every other re-run row flips exactly as in §5.

### 11.4 Gates on the repaired bytes

All were run with `CARGO_HOME=/cargo CARGO_NET_OFFLINE=true CARGO_TARGET_DIR=/tmp/target`, after the matrix had
restored and verified the sources.

| Gate | Result |
|---|---|
| `cargo fmt --all -- --check` | clean |
| `cargo clippy --locked --offline -p bridge-core --all-targets -- -D warnings` | clean |
| `cargo test --locked --offline -p bridge-core --lib` | 830 passed, 0 failed, on five further full runs; the first run failed one 2B2a test (below) |
| `cargo test --locked --offline -p bridge-core --doc` | 9 passed, 0 failed |
| `cargo test --locked --offline -p bridge-core` | 17 targets, 968 passed, 0 failed |
| `cargo test --locked --offline --workspace --no-fail-fast`, proxy variables unset (§6) | exit 0; 106 targets, **4574 passed**, 0 failed, 13 ignored |
| `cargo test --locked --offline --workspace --all-targets --no-fail-fast`, proxy variables unset | exit 0; 90 targets, **4564 passed**, 0 failed, 13 ignored |

**Observed intermittent failure: pre-existing and outside the owned paths.** In the first full `bridge-core --lib`
run after the matrix, `custody_git_tests::a5c_a5e_and_a5e_r_recheck_after_spawn_bind_and_audit_ancestors` failed at
`custody_git_tests.rs:114` (`admit fixture`).

- **Mechanism.** Reproduced on this tree in 1 of 12 `--lib custody_` runs:
  `Spawn(Os { code: 26, kind: ExecutableFileBusy, message: "Text file busy" })`. 2B2a's `FixtureRoute` writes a route
  script and then executes it. If another test thread forks while the script's write descriptor is open, the child
  inherits that descriptor until its own `exec`. For that window the kernel refuses to execute the script.
- **Attribution.** The exact predecessor `91d0bf13` was extracted with `git archive` into `/tmp/base-91d0bf13` and
  built into `/tmp/target-base`. In this same container it failed in 1 of 20 `--lib custody_` runs. The failure was
  a different 2B2a test, `w2_stdout_target_cannot_escape_the_pinned_root`, at the same line with the same
  `ETXTBSY`.
- **Scope.** The fixture lives in `custody_git_tests.rs`, which is outside §8, so it is not changed here.
- **Exposure in owned tests.** The new exact-edge route uses the same write-then-execute pattern as control 31 and
  the version test, which already existed. None of them failed in the observed runs.

### 11.5 Staged paths (repair round 1)

```text
crates/bridge-core/src/custody_export.rs            (modified)
crates/bridge-core/src/custody_export_tests.rs      (modified)
docs/superpowers/reviews/2026-09-25-adr0041-slice2b2-implementation-handoff.md (modified)
```

Repair round 1 is committed as `7842f620`. Its two SMELLs remained DEFERRED (§7). What remains for the controller is
listed in §9.

## 12. Repair round 2 — Sol implementation review round 2 (2026-09-26)

**Review:** Sol implementation review round 2 of `7842f620`. It resolved W2 and W3 and left two items, which the
controller verified:
- W1-stale, WRONG · MATERIAL · BLOCKER: repaired;
- the handoff's commit status, WRONG · IMMATERIAL: rewritten.

The SMELL on control 5b/9b timing stays DEFERRED (§7) and was not worked.

**Base and scope:** HEAD `7842f620`, clean working tree. Only `custody_export.rs`, `custody_export_tests.rs`, and this
handoff changed. `custody_git.rs` and `fs_custody.rs` equal `HEAD` (`git diff --quiet HEAD --`), with the §5 SHA-256
values. The task is still revision 11, SHA-256 `b9f1215a…d814e9`.

**Method: RED first.** Both controls in §12.2 were written and run while `custody_export.rs` was still byte-equal to
`7842f620` (SHA-256 `0d47d8db…58bb09`). Raw outputs are in `.git/a2a-bridge/mutation/`:
- RED: `repair2-red.txt`. `cargo test -p bridge-core --lib custody_export` selected 62 tests: **60 passed, 2 failed**,
  exactly the two new controls.
- GREEN: `repair2-green.txt`, **62 passed**, on the final bytes.

Between RED and GREEN the test file changed only by `use std::path::PathBuf;`. The repaired exporter no longer imports
`PathBuf`, which the tests had reached through `use super::*`.

### 12.1 Findings, RED evidence, and fixes

**W1-stale: a scratch root inside a renamed, descriptor-pinned worktree was admitted and written into.**
`protected_paths()` kept only canonical path strings. `refuse_source_overlap` then derived each protected identity by
reopening that path. The review's sequence: mint the capability, rename the worktree, and create a replacement at its
old path. The reopened identity is then the replacement's, so no scratch ancestor matched it. The exporter created
`capsule/`, `work/`, `work/home/`, and `work/xdg/` inside the source. Only then did the first Git pre-spawn callback
refuse with `IdentityDrift`.

- **RED, the review's regression:** `control_21_a_renamed_worktree_is_refused_by_its_retained_identity_before_any_write`
  failed on `7842f620`. Verbatim, with the identity dump elided:
  - `… was not refused by the preflight before any write: refused with IdentityDrift("pinned scan root …/wt now resolves to a different directory …"); worktree entries created: ["custody/capsule/", "custody/work/", "custody/work/home/", "custody/work/xdg/"]`
  - The scratch root is a fresh, empty, owner-private `custody/`, so the production preflight runs with no bypass.
  - The snapshot records entries as well as bytes (`snapshot_entries`). The writes that land before the callback are
    empty directories, which the file-only `snapshot_tree` cannot see.
- **RED, the pre-write recheck:** `control_09_pre_write_recheck_refuses_a_source_drifted_before_the_export` failed on
  `7842f620` in all five rows, each with `scratch entries written: ["capsule/", "work/", "work/home/", "work/xdg/"]`:
  - `RewriteAlternatesToUnboundStore: refused with IdentityDrift("the alternates file of …/src.git/objects changed content")`;
  - `RetargetPinnedAlternate`, `SwapPrimaryObjectStore`, `SwapSourceGitDirectory`, and `SwapSourceWorktreeRoot`:
    `refused with IdentityDrift("pinned scan root … now resolves to a different directory …")`.
- **Fix, part 1: retained identities.**
  - `protected_paths()` becomes `protected_directories()`. It returns the pinned descriptors themselves
    (`&PinnedDirectoryV1`): the repository root, the git directory, the primary store's pin, and each alternate's pin.
  - `refuse_source_overlap` compares every scratch ancestor with the `(dev, ino)` of each descriptor's `identity()`.
    That identity is the `fstat` of the descriptor retained at mint. No protected identity is re-derived from a path.
  - A descriptor with no dev/ino refuses as a typed `ScratchPreflight`.
  - The canonical-path comparison and the reverse walk are unchanged.
  - Whatever a pinned path names now, the forward comparison checks the directory the capability pinned. That closes
    the path-staleness class for this comparison, not just the rename instance.
- **Fix, part 2: a recheck before the first scratch write.** `export_capsule_v1` calls `capability.recheck()` right
  after the preflight, before the ledger exists and before `capsule/` is created. A pinned path that no longer resolves
  to its descriptor refuses with `IdentityDrift` before the exporter writes anything. That covers a renamed or
  retargeted path and a rewritten alternates file, and it includes the paths the reverse walk used.
- **Why the reverse walk stays path-based:** `fs_custody`, read-only under §8, offers no `..` walk from a retained
  descriptor. The walk only has to be right when a write follows. A stale path is refused by the pre-write recheck,
  and an empty scratch root cannot contain a pinned directory (§7).
- **GREEN:**
  - The regression refuses with `ScratchPreflight`. The renamed worktree and the git directory are unchanged, byte for
    byte and entry for entry.
  - Each pre-write row refuses with `IdentityDrift` and leaves the scratch root empty.
  - The nine control-21 relation rows and controls 5a, 5b, 9a, and 9b are unchanged.
- **Mutations:** M21-retained and M09-prewrite (§12.3).

**Handoff status: the committed handoff reported pre-commit state as current.** RED
(`repair2-handoff-red.txt`, grepped from the text committed in `7842f620`; the literal phrases are recorded there
and not repeated here, so a phrase lint over this file stays meaningful):
- lines 3 and 11 called the work staged but uncommitted;
- lines 458, 461, and 712 said that no commit existed;
- lines 465 and 713 listed the completed macOS lane as remaining work, and line 469 listed the commit itself.

- **Fix:** the status paragraphs, §6.3, §8, §9, and §11.5 now describe committed state. Each names the commit that
  holds its round (`87570241`, `91d0bf13`, `7842f620`). §9 lists only the gates that remain. Repair round 2 is
  described as landing in the commit that contains this section, so the text stays true once that commit exists.
- **GREEN:** the same grep finds no match in this handoff. The lint the review proposes would live outside the §8
  owned paths, so it is not added here.
- **Mutation:** none. This is a text fix with no code guard.

### 12.2 Controls added or changed

| Control | Test | Kind |
|---|---|---|
| 21 | `control_21_a_renamed_worktree_is_refused_by_its_retained_identity_before_any_write` (new): the review's regression | discriminating (M21-retained; M21 and M21-root also turn it red) |
| 9 (pre-write) | `control_09_pre_write_recheck_refuses_a_source_drifted_before_the_export` (new): the five control-5/9 drifts, applied between mint and export | discriminating (M09-prewrite) |
| fixture | `snapshot_entries` (new) records directories beside file bytes; `DriftTargetsV1::of` replaces the inline construction in `assert_callback_refuses_drift` | plumbing |

### 12.3 Mutation matrix, repair round 2

**Harness changes:**
- M21's and M21-root's targets follow the renamed `protected_directories()`. M21 still disables the loop with
  `.take(0)`, and M21-root still drops the repository root from the protected set.
- Two rows are added.

75 rows are defined, and `matrix.py check` finds every target unique. Round 1's results are kept as
`results.repair1.jsonl`.

**Snapshot of the round-2 bytes.** These hashes equal the staged bytes.

| File | SHA-256 |
|---|---|
| `crates/bridge-core/src/custody_export.rs` | `3987f39d68b21b8da0ef8c2207f937cb3bdfb420db58eac052e46ffaad2ee988` |
| `crates/bridge-core/src/custody_export_tests.rs` | `47d66a1256a3303b6de0fb323b2af651082d47a66c9940ae7e2a6d933c21f934` |
| the other six snapshot files | unchanged from §11.3 |

**Run:** one foreground `timeout 590` call, with an internal budget of 450 s and a 240 s cap on each cargo run, from
07:58:47Z to 07:59:43Z. No background job was started. It covered 13 rows:
- the 2 new rows;
- the 5 affected rows:
  - M21 and M21-root, whose targets were retargeted;
  - M05a, M05b, and M09-root, whose controls use the refactored drift fixture, and whose guard, `recheck()`, the new
    pre-write guard calls;
- the 6 rows whose targets lie in the changed `export_capsule_v1`: M11j, M11l, M12, M30, M31, and M-version.

**13 of 13 FLIPPED, 0 INADMISSIBLE.**

Each mutation was restored byte-exactly with a fresh mtime. `matrix.py verify` then reported
`VERIFY OK: 8 files equal their snapshot`, an independent re-hash of all eight files against `snapshot/manifest.json`
agreed, and no `pending.json` exists. The other 62 rows were not re-run. Their targets lie outside the changed
functions and fixtures, and their last verdict is FLIPPED (§5, §11.3).

| ID | Guard mutated | Red | Named green | Outcome under mutation |
|---|---|---|---|---|
| M21-retained | the protected identity re-derived from the pinned path (`identity(path)`), not taken from the descriptor | 21 stale-pin row | 21 relation rows, 9 pre-write | refused one layer later: the pre-write recheck returns `IdentityDrift`, not `ScratchPreflight`, and no worktree entry is created (layered, below) |
| M09-prewrite | the pre-write `capability.recheck()` deleted | 9 pre-write (all five rows) | 21 stale-pin row, 21 relation rows, 9a, 9b | every row writes `capsule/`, `work/`, `work/home/`, and `work/xdg/` into the scratch root before the first Git callback refuses |

- **M21 and M21-root** now also turn the stale-pin row red. With the preflight disabled, or with the repository root
  unprotected, the pre-write recheck refuses that row as `IdentityDrift`. Their relation rows show the same wrong
  successes as in §11.3.
- **M09-root**'s named greens now include the stale-pin row, which the retained-identity preflight still refuses.
- Every other re-run row flips exactly as in §5 and §11.3.

**Layered guard.** M21-retained flips by refusal type, like M07 and M20a (§5). The pre-write recheck is a second
layer that stops a stale capability before any write, so once the retained-identity comparison is removed, a wrong
write is unreachable in this scenario. The control therefore requires the §2 preflight's own `ScratchPreflight`
refusal. The RED on `7842f620`, which had neither layer, shows the write that each layer prevents.

### 12.4 Gates on the round-2 bytes

All were run with `CARGO_HOME=/cargo CARGO_NET_OFFLINE=true CARGO_TARGET_DIR=/tmp/target`, after the matrix had
restored and verified the sources.

| Gate | Result |
|---|---|
| `cargo fmt --all -- --check` | clean |
| `cargo clippy --locked --offline -p bridge-core --all-targets -- -D warnings` | clean |
| `cargo test --locked --offline -p bridge-core --lib custody_export` | 62 passed, 0 failed |
| `cargo test --locked --offline -p bridge-core --lib` | 832 passed, 0 failed, on the first run |
| `cargo test --locked --offline -p bridge-core --doc` | 9 passed, 0 failed |
| `cargo test --locked --offline -p bridge-core` | 17 targets, 970 passed, 0 failed |
| `cargo test --locked --offline --workspace --no-fail-fast`, proxy variables unset (§6) | exit 0; 106 targets, **4576 passed**, 0 failed, 13 ignored |
| `cargo test --locked --offline --workspace --all-targets --no-fail-fast`, proxy variables unset | exit 0; 90 targets, **4566 passed**, 0 failed, 13 ignored |

The §11.4 intermittent 2B2a `ETXTBSY` failure did not recur in these runs.

### 12.5 Changed paths (repair round 2)

```text
crates/bridge-core/src/custody_export.rs            (modified)
crates/bridge-core/src/custody_export_tests.rs      (modified)
docs/superpowers/reviews/2026-09-25-adr0041-slice2b2-implementation-handoff.md (modified)
```

The exact `git diff --cached --stat` and `git diff --cached --check` results are reported in the turn's final message.
The remaining gates are listed in §9.
