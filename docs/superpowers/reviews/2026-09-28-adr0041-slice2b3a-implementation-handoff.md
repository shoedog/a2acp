# ADR-0041 Slice 2B3a implementation handoff — capsule reader (phase V: verify and stage)

**Status:** implemented in the container lane in one bridge turn.
- A crate-private, `#[cfg(unix)]` module, `custody_restore.rs`, implements design §3.1 phase V as
  `verify_and_stage_v1`. It pins a sealed capsule and a new, empty destination, verifies every ciphertext against the
  seal, stages only the three controls and binds them through 2B1, then stages the pack and the payloads and fully
  decode-verifies every coverage frame. It writes only beneath `<dest>/.restore-work/`.
- The test-only envelope fixture (the moved 2B2 sealer plus a new stream-parsing opener) is one shared module, and
  `custody_capsule.rs` gains one additive `pub(crate)` constructor, `from_published_seal_v1`.
- **Controls:** every test the task names in tasks 1–6 exists and passes, plus four extra controls (§3). That is
  **58** new lib tests (6 fixture, 4 capsule, 48 restore) and **1** new compile-fail doctest.
- **Mutation matrix:** **34** rows. In one foreground pass on the final bytes (snapshot `412f94d481773a0b`), all 34
  **FLIPPED**, with 0 NOT-FLIPPED and 0 INADMISSIBLE. The source was then proved equal to that snapshot (§5).
- **Gates:** green on the final bytes: fmt, workspace clippy with `-D warnings`, the diff check, and hygiene.
  - `bridge-core` lib: **1,091** passed, which is the base's 1,033 plus exactly the 58 new tests.
  - Workspace `--all-targets`: **4,833** passed, 0 failed, 13 ignored.
  - Workspace default (with doctests): **4,844** passed, 0 failed, 13 ignored. The new visibility doctest passes.
- **Excluded here:** `cargo deny` (not installed), the controller's macOS lane, and CI's native ext4 lane (§6).
- **Repair round 1 (§10)** closes blocker B1 of the Sol implementation review, round 1. Its snapshot, matrix rows, and
  totals supersede the ones above for the final bytes.
- **Repair round 2 (§11)** closes B1 as the Sol implementation review, round 2, re-raised it: the final gate's order.
  It also declares the final gate's guarantee boundary (§11.6). Its snapshot, matrix rows, and totals supersede §10's
  for the final bytes.

This handoff records evidence only; it claims no review approval.

**Task (authoritative):** `docs/superpowers/plans/2026-09-27-adr0041-slice2b3a-capsule-reader-task.md`, revision 3,
SHA-256 `1e3bd7a70b9658cbefb6f75c10cb51ca32a9e64c2cdbc342aac83f8a702d5e1a` at the base commit. The bridge copy
`.git/A2A_TASK.md` (SHA-256 `443cfacbb07904bd00a759aa05bc21f4123e756028a88ed8d5bf9e7c849490cc`) differs from it only
in the front matter, the pinned base, and the controller notes.

**Clone:** `/Users/wesleyjinks/code/.a2a-implement/impl-98351-dh9jlwz2`
**Branch:** `implement/impl-98351-dh9jlwz2`
**Base HEAD:** `b567d2d6af86c50b3c59cd3f71c55ed7b07e429d` (`main`, the merge of the spec in PR #124)

**Container lane:**
- Linux `7.0.14-orbstack` on **aarch64**, with `/` on overlayfs, running as uid 0.
- rustc and cargo 1.94.0, and Git 2.54.0 at `/opt/git/bin/git`.
- Every cargo command ran with `CARGO_HOME=/cargo CARGO_NET_OFFLINE=true CARGO_TARGET_DIR=/tmp/target
  CARGO_INCREMENTAL=0`. Every workspace run also unset `HTTP_PROXY`, `HTTPS_PROXY`, `http_proxy`, and `https_proxy`.
- **Stop conditions:** none was reached.
  - `custody_frame.rs`, `custody_walk.rs`, `custody_coverage.rs`, `custody_git.rs`, `custody_mounts.rs`, and
    `fs_custody.rs` are unchanged.
  - No existing validation is weakened, no dependency is added, and no Git command runs in the reader.
  - No symlink is followed, and no capsule content is read by path. Every write is create-new under `.restore-work/`.
  - The reader adds no `unsafe`.

---

## 1. RED evidence per task

Raw output is in `.git/a2a-bridge/red/`. The tests of each task were written before its code.

| Task | Structural RED (tests first, code absent) | Behavioral RED |
|---|---|---|
| 1 | `task1-structural.txt`: 30 errors (`FixtureOpenerV1`, `fixture_envelope_format_v1`, `FIXTURE_ENVELOPE_MAGIC_V1`, … not found) | from the matrix: `M-opener-first-chunk-magic` and `M-opener-format` |
| 2 | `task2-structural.txt`: 6 × E0599, no `from_published_seal_v1` | `task2-stub.txt`: against the one-line body the task names (open-request validation only), the zero-length and over-limit controls fail (2 of 4). `task2-novalidation-stub.txt`: against a body that validates nothing, 3 of 4 fail; the round trip is the positive control. `task2-doctest-pub.txt`: with the constructor made `pub`, the new doctest fails with "Test compiled successfully, but it's marked `compile_fail`" |
| 3 | `task3-structural.txt`: 47 errors (`pin_and_verify_capsule_v1`, `PinnedCapsuleV1`, `CustodyRestoreErrorV1`, … not found) | `task3-stub.txt`: against a stub that always returns `Ok`, 11 of 12 fail. The 12th is `reseal_with_no_replacement_is_byte_identical`, the fixture helper's own self-test, which does not call the reader |
| 4 | `task4-structural.txt`: 24 errors (`prepare_destination_v1`, `CustodyRestoreBudgetV1`, `override_created_dev_for_test`, …) | `task4-stub.txt`: against a stub that pins and creates the work tree with no check, 9 of the 10 task 4 controls fail. The 10th, `budget_ceiling_and_ceiling_plus_one`, tests the real budget constructor, which the stub keeps; its RED is the `M-budget-ceiling` and `M-budget-zero` rows |
| 5 | `task5-structural.txt`: 16 errors (`bind_control_v1`, `stage_one_v1`, `override_chunk_bytes_for_test`, …) | from the matrix, one or more rows per control (§5) |
| 6 | `task6-structural.txt`: 7 errors (`verify_and_stage_v1`, `VerifiedCapsuleV1`, …) | from the matrix, one or more rows per control (§5) |

**Task 1's moved sealer is behavior-identical.** Right after the move, `cargo test -p bridge-core --lib custody_export`
passed **88 of 88** (`task1-export-green.txt`). On the final bytes the default run again has 88 `custody_export::tests`
passes, the base's count exactly.

**Two test corrections, stated plainly.** No assertion was relaxed to make a guard pass.
- `published_seal_refuses_what_open_requests_refuse` first had a "recipient bytes" row of 64 recipients of 4,096 bytes
  each. That is exactly the 262,144-byte ceiling, so the row was admitted, correctly: the aggregate limit is
  unreachable within the per-field and count limits. The row was removed; the count, per-recipient, format, tool, and
  version rows remain.
- `verifies_and_stages_a_plan_backed_capsule` first compared the staged payload classes, which are in index (name)
  order, with the plan's receipts, which are in class order. It now sorts before comparing and requires exactly 7.

## 2. What was built

### 2.1 `custody_envelope_fixture.rs` (new, `#[cfg(test)] pub(crate) mod`)

- **Moved verbatim** from `custody_export_tests.rs`: `FIXTURE_ENVELOPE_MAGIC_V1`, `SealerFaultV1` (whose two fields are
  now `pub(crate)`, so export controls 15 and 16 compile unchanged), and `FixtureSealerV1`. The old `envelope_format()`
  helper is `fixture_envelope_format_v1()`, and its three call sites in the export tests now use it.
- **`FixtureOpenerV1`**, with `OpenerFaultV1 { lie_about_receipt_length, refuse, stop_after_chunks }` and `calls()`:
  - it counts every call, then refuses (`InvalidInput`) on the `refuse` fault or on any format other than
    `fixture_envelope_format_v1()`, before reading a chunk;
  - it reads the ciphertext as one byte stream, accumulating the 8-byte magic across as many chunks as it takes, and
    refuses a wrong or short magic. Nothing reaches the sink before the magic is proven;
  - it forwards the rest of the chunk that completes the magic, and every later chunk, holding back the latest
    non-empty piece so the final one carries the last-chunk flag. Ordinals are renumbered from 0, and an empty
    plaintext is one empty final chunk;
  - it hashes while writing and returns `CustodyEnvelopeOpenReceiptV1::new(len, sha)`, with `len + 1` under
    `lie_about_receipt_length`. `stop_after_chunks` stops reading and finishes the plaintext anyway.

### 2.2 `custody_capsule.rs` (additive)

- `CustodyCapsuleSealProofV1::from_published_seal_v1`, exactly the task's body: the open-request seal validator,
  then per artifact `validate_selected_seal_artifact_limits` and a nonzero length. It is `pub(crate)`, with
  `#[cfg_attr(not(unix), allow(dead_code))]` because its only caller is unix-only.
- The `from_published_seal_is_crate_private` compile-fail doctest on `CustodyCapsuleSealProofV1`. It builds a valid
  `CustodySealV1` through the public API and calls the constructor, so its only error is the visibility.
- The four `published_seal_*` unit tests.

### 2.3 `custody_restore.rs` (new, `#[cfg(unix)] #[allow(dead_code)] mod`, declared next to the other custody modules)

**Task 3: `pin_and_verify_capsule_v1(capsule_root)`.**
1. `PinnedDirectoryV1::open` of the root. One entry budget, `RESTORE_CAPSULE_ENTRY_BUDGET_V1 = 4096`, is shared by
   every `list_child_names` call.
2. The seal: a no-follow stat (absent is `CapsuleEntryMissing`, a link is `SymlinkRefused`), `open_regular_file`,
   `read_bounded_v1(.., 1 MiB)` (oversize is `SealUnreadable`), `CustodySealV1::decode_canonical`, then
   `from_published_seal_v1` (either failing is `SealInvalid`).
3. The exterior: the expected set is built from the seal (`custody-seal.v1` plus every artifact's components; a sealed
   name that collides with the seal or with another entry's kind is `SealInvalid`). Each directory, parents first, is
   listed and must equal exactly its expected children: an extra name is `CapsuleEntryUnexpected`, a missing one
   `CapsuleEntryMissing`. Each child is stat'ed no-follow: a symlink is `SymlinkRefused`, and a kind other than the
   expected one is `CapsuleEntryUnexpected`. Directories are opened with `open_existing_child_directory` and files
   with `open_regular_file`; an `ELOOP` from either no-follow open is also `SymlinkRefused`.
4. Every ciphertext, in seal order, is hashed from offset 0 through its retained descriptor, reading at most
   `byte_length + 1` bytes: a length mismatch is `CiphertextLength`, a digest mismatch `CiphertextDigest`. Each
   descriptor is rewound and retained.

**Task 4: `CustodyRestoreBudgetV1` and `prepare_destination_v1(destination, capsule, budget)`.**
- `CustodyRestoreBudgetV1::new` refuses 0 and anything above `RESTORE_BYTES_CEILING_V1` (10 GiB) as `Budget`;
  `ceiling()` is exactly the ceiling.
- The destination goes through 2B2's `preflight_scratch_root(destination, &[&capsule.root])`: owner-private, empty,
  and disjoint from the pinned capsule in both directions, by canonical path and by retained identity. Its refusal is
  `DestinationInvalid`.
- The root's `dev` is recorded, and the mount census runs over the destination's canonical path: a mount point
  strictly within it, or any census error, is `MountBoundary`.
- `pinned_root_unchanged` rechecks the destination pin before the first create (`IdentityChanged`).
- One `ScratchLedgerV1` of the budget's limit. `.restore-work/` and `.restore-work/plain/` are each created
  create-new after reserving one 64 KiB entry allowance, and each created directory's `dev`, from its retained
  descriptor, must equal the root's (`DeviceCrossing{name}`).
- `RestoreDestinationV1::recheck_containment()` re-runs the census.

**Task 5: `stage_one_v1` and `bind_control_v1`.**
- **Staging directories.** `RestoreDestinationV1` retains, in `staging: RefCell<BTreeMap<key, Rc<PinnedDirectoryV1>>>`,
  the pin of every directory it creates beneath `plain/`. A later artifact reuses only that pin and never re-opens a
  directory by name. Before each create, the chain `.restore-work/` → `plain/` → every retained directory down to the
  parent is rechecked through the parent's no-follow stat: the entry must be a directory with the retained `dev` and
  `ino`, else `IdentityChanged`.
- **The staged file.** Its entry allowance is reserved, then it is created with `create_new_regular_child`
  (`O_CREAT | O_EXCL | O_NOFOLLOW`, mode 0600). An existing entry, a planted symlink included, is `StagingCollision`,
  and the planted object is untouched.
- **`VerifiedCiphertextSourceV1`.** It reads the retained ciphertext with positioned reads (`read_exact_at`) from
  offset 0, in chunks of `RESTORE_CHUNK_BYTES_V1` (1 MiB; a test seam lowers it), and feeds every chunk to a
  `CustodyEnvelopeSourceValidatorV1` declared at the sealed length. After the opener returns, the validator's `finish`
  must succeed, with the sealed length and digest: otherwise, and on a short read, `CiphertextChanged{name}`.
- **`StagingPlaintextSinkV1`.** It reserves each chunk's bytes in the ledger before writing, admits the chunk to its
  own `CustodyEnvelopeSinkValidatorV1`, writes it, and measures its own length and SHA-256 over what it wrote. A receipt
  that disagrees with that measurement, or an unfinished plaintext stream, is `OpenerReceiptMismatch`.
- **Error order after `open`.** A recorded source or sink failure (budget, write, short read), else `Open(error)` if
  the opener refused; then the consumed-ciphertext check; then the receipt-versus-measurement check. The file is then
  synced and rewound.
- **The role** of a staged artifact is the one the reserved 2B1 name admits (`CustodyCapsuleArtifactRoleRowV1::new`
  accepts exactly one role per reserved name).
- **`bind_control_v1`** stages the manifest, the index, and the policy, in that order. It reads each back through
  `read_bounded_v1(.., 1 MiB)` (oversize is `ControlOversize`), decodes it canonically (a failure is `ControlDecode`),
  and calls `CustodyCapsuleBindingV1::new` (a refusal is `Binding`). A seal without a control name is
  `Binding(MissingArtifact)`.
- **`read_bounded_v1(reader, bound)`** reads into one `bound + 1` buffer with `read(&mut buffer[filled..])`, so it never
  requests a byte past `bound + 1` and never allocates more. Reading `bound + 1` bytes is `Oversize`.

**Task 6: `verify_and_stage_v1(capsule, destination, budget, frame_budget, opener)`.**
1. Task 3, then task 4, then task 5, so a T1 or T2 refusal precedes `.restore-work/`.
2. Every remaining index row is staged in index order, and its staged role must equal the row's.
3. `staged` is sorted into the index's order, and the order is then checked: the names must equal the index's
   artifact names exactly.
4. Each coverage payload is decoded with the header `(class, manifest.generation_id())` and the frame budget, through
   a 64 KiB `BufReader` over the retained descriptor. Every entry's content is drained to a null sink until `Ok(None)`,
   so the trailer's digest, count, and total are verified. Any refusal is `FrameInvalid{class}`, and the descriptor is
   rewound. The pack is staged only.
5. `recheck_containment()` runs before `VerifiedCapsuleV1` is returned.

**Test seams (`#[cfg(test)]`):** hook points `AfterCiphertextVerification` (the task's
`after_ciphertext_verification_for_test`), `BeforeFirstCreate`, and `BeforeLeafCreate`;
`override_created_dev_for_test(name, dev)`; and `override_chunk_bytes_for_test(bytes)`. Each guard uninstalls only
its own seam.

### 2.4 The 2B2 test harness (visibility only) and `custody_export.rs`

- `custody_export.rs`: `mod tests;` became `pub(crate) mod tests;`. Nothing else changed.
- `custody_export_tests.rs`, besides the fixture move: `pub(crate)` on `HarnessV1`, `HarnessV1::new`, and
  `HarnessV1::run_sealed`; on `PlanFixtureV1` and its `rich_clone`, `inventory`, `plan`, `planned`, and `export`; and
  one new `capsule_dir()` accessor on each fixture (`scratch.join(CAPSULE_DIR_NAME)`).
- **Beyond the task's list, also `pub(crate)`** (§7, item 1): `HarnessV1`'s `manifest` and `streams` fields,
  `IDENTITY_V1`, `manifest_for`, `default_streams`, `expect_sealed`, `manifest_from_plan`, and `planned_capability`.

## 3. The controls

All are in `custody_restore::tests` unless named otherwise. "Pipeline" means `verify_and_stage_v1`.

| Task | Control | What it proves |
|---|---|---|
| 1 | `fixture_opener_round_trips_the_fixture_sealer` | a 3-chunk plaintext sealed by the fixture sealer opens byte-equal, with the receipt length and SHA-256, renumbered ordinals, and one final flag |
| 1 | `fixture_opener_accepts_split_magic`, `…_coalesced_magic` | `A2A` / `FIX1\n‖body[..3]` / rest, and the whole envelope as one chunk, both open byte-equal (review focus 7) |
| 1 | `fixture_opener_refuses_a_wrong_magic` | `NOTMAGIC` split and coalesced, and a near miss, refuse with nothing written |
| 1 | `fixture_opener_refuses_a_short_envelope` | no chunks, `A2AFIX`, and `A2A`+`FIX` refuse with nothing written |
| 1 | `fixture_opener_refuses_an_unsupported_format` | another format refuses before any chunk is read |
| 2 | `custody_capsule::tests::published_seal_*` (4) | the round trip; a zero-length artifact the open-request validator admits is refused; 10 GiB + 1 is refused and 10 GiB admitted; each seal-wide over-limit row (recipient count and length, format, tool, version) refuses as open requests do |
| 2 | doctest `from_published_seal_is_crate_private` | the constructor cannot be named outside the crate |
| 3 | `reseal_with_no_replacement_is_byte_identical` | the `reseal_for_test` helper reproduces all 7 files byte for byte |
| 3 | `pins_and_verifies_an_exported_capsule` | 6 retained, rewound ciphertexts, each with its sealed length |
| 3 | `oversize_seal_refuses` | 1 MiB + 1 is `SealUnreadable`; exactly 1 MiB is read (and is `SealInvalid` JSON) |
| 3 | `malformed_seal_refuses` (T1) | not JSON, non-canonical, and a canonical zero-length artifact are each `SealInvalid`, directly and through the pipeline with no `.restore-work/` and 0 opener calls |
| 3 | `seal_artifact_digest_edit_refuses_before_staging` (T2) | `CiphertextDigest`, directly and through the pipeline with nothing written and nothing opened |
| 3 | `seal_manifest_digest_edit_passes_task_3` | the T3 edit passes task 3 |
| 3 | `ciphertext_length_mismatch_refuses`, `ciphertext_digest_mismatch_refuses`, `missing_artifact_refuses` | `CiphertextLength`, `CiphertextDigest`, and `CapsuleEntryMissing` (a payload, and the pack) |
| 3 | `capsule_extra_entry_refuses` | `stray`, `.a2a-staging-seal`, and `control/.a2a-staging-x` are each `CapsuleEntryUnexpected` (review focus 2) |
| 3 | `capsule_symlinked_artifact_refuses`, `capsule_symlinked_component_refuses` | `SymlinkRefused`, never followed (review focus 1) |
| 4 | `prepares_a_new_empty_destination` (extra) | exactly `.restore-work/` and `plain/`, and 128 KiB charged |
| 4 | `destination_must_be_empty` | `DestinationInvalid("… not empty")`, nothing created |
| 4 | `destination_inside_capsule_refuses`, `capsule_inside_destination_refuses` | the overlap predicate refuses each direction, with no entry created; for the second, 2B2's emptiness seam is bypassed so the overlap is the only guard |
| 4 | `destination_swapped_before_first_write_refuses` | `IdentityChanged`, and both directories stay empty (review focus 5) |
| 4 | `ledger_max_and_max_plus_one` | exactly the 128 KiB charge admits; one byte less is `Budget` |
| 4 | `budget_ceiling_and_ceiling_plus_one` | the ceiling admits; ceiling + 1 and 0 are `Budget` |
| 4 | `mount_inside_destination_refuses` | a mount one level down is `MountBoundary` with no `.restore-work/`; a mount at or above the destination is admitted |
| 4 | `census_failure_refuses` | `MalformedLine { line: 1 }` is `MountBoundary` |
| 4 | `created_directory_on_another_device_refuses` | `DeviceCrossing{".restore-work"}` |
| 5 | `binds_an_exported_capsule` | exactly the 3 control plaintexts, byte-equal and rewound; the manifest, digest, index, and inert policy |
| 5 | `staging_ledger_max_and_max_plus_one` (extra) | 6 entry allowances plus the control plaintext bytes admit exactly; one byte less is `Budget` |
| 5 | `tampered_manifest_refuses_before_payload_staging` | another generation's canonical manifest, resealed: `Binding(ThreeWayDigestMismatch)`, no `plain/payload` or `plain/git` |
| 5 | `index_mapping_mismatch_refuses` | the index minus the worktree row: `Binding(MissingArtifact)`, no payload |
| 5 | `non_inert_policy_refuses` | `"hooks":"enabled"`: `ControlDecode`, no payload |
| 5 | `opener_lying_receipt_refuses`, `opener_refusal_propagates` | `OpenerReceiptMismatch` (review focus 3); `Open(InvalidInput)` after exactly one call |
| 5 | `oversize_control_plaintext_refuses` | a 1 MiB + 1 manifest: `ControlOversize` (review focus 4) |
| 5 | `bounded_reader_never_requests_past_the_probe` | over 4 MiB, at most `bound + 1` bytes requested; a reader failing past `bound + 1` still gives `Oversize`; 0, 1, and `bound` bytes read whole |
| 5 | `seal_manifest_digest_edit_refuses_at_binding` (T3) | `Binding(ThreeWayDigestMismatch)`, and `plain/` holds exactly `control/` and the three controls |
| 5 | `ciphertext_changed_after_verification_refuses` | the policy rewritten in place, same inode and length, valid envelope: `CiphertextChanged` (review focus 6) |
| 5 | `ciphertext_truncated_after_verification_refuses` (extra) | the manifest truncated in place after verification: `CiphertextChanged` |
| 5 | `opener_stopping_early_refuses` | with 64-byte chunks, `stop_after_chunks: Some(1)`: `CiphertextChanged` |
| 5 | `binds_across_small_chunks` (extra) | with 3-byte chunks (the magic split), the controls bind |
| 5 | `shared_staging_directory_custody` | a planted `plain/control/` directory, and a planted symlink to an outside directory, are each `StagingCollision` with no leaf beneath; a retained `plain/control/` renamed away and replaced is `IdentityChanged`, and the replacement stays empty |
| 5 | `staging_collision_refuses` | a planted file, then a planted symlink to an outside file, at the first leaf: `StagingCollision`, with the planted bytes and link target and the outside file unchanged |
| 6 | `verifies_and_stages_a_fixture_stream_capsule` | the 2B2 harness's fixture streams are not frames: `FrameInvalid{Index}` |
| 6 | `verifies_and_stages_a_plan_backed_capsule` | a rich clone exported plan-backed: `Ok`; each staged plaintext equals its envelope body, and each payload equals its plan receipt (length and SHA-256); 7 payloads; one open per artifact |
| 6 | `corrupt_frame_trailer_refuses`, `wrong_class_frame_refuses`, `wrong_generation_frame_refuses` | the worktree payload resealed with its last trailer byte flipped, with the index payload, and with a second export's worktree payload under `generation-2b3a-other`: each `FrameInvalid{Worktree}` |
| 6 | `nothing_outside_restore_work` | the destination holds exactly `.restore-work/`, `plain/`, its three directories, and one file per index row |
| 6 | `staged_order_equals_index_order` | names and roles in the index's order |
| 6 | `every_ciphertext_verified_before_the_first_open` | the lexically last artifact corrupted: `CiphertextDigest`, 0 opener calls, nothing written |
| 6 | `capsule_is_never_written` | bytes, mode, mtime, and listings of every capsule entry, unchanged on success and on T1, T2, and T3 refusals |
| 6 | `mount_appearing_during_restore_refuses` | a mount only on the second census call: `MountBoundary` from the final recheck, after staging |

**Review focus coverage:** 1 → `capsule_symlinked_*`; 2 → `capsule_extra_entry_refuses`; 3 →
`opener_lying_receipt_refuses`; 4 → `oversize_seal_refuses`, `oversize_control_plaintext_refuses`; 5 →
`destination_swapped_before_first_write_refuses`; 6 → `ciphertext_changed_after_verification_refuses`; 7 →
`fixture_opener_accepts_split_magic` and `…_coalesced_magic`.

## 4. Verification totals

All gates ran on the final source bytes (snapshot `412f94d481773a0b`), before the final matrix pass, which then
restored exactly those bytes. Raw output is in `.git/a2a-bridge/gates/`; `totals.py` sums every `test result` line.

| Gate | Exit | Totals |
|---|---|---|
| `cargo fmt --all -- --check` | 0 | no diff |
| `cargo clippy --locked --offline --workspace --all-targets -- -D warnings` | 0 | no warning or error |
| `git diff --check`, then `git diff --cached --check` after staging | 0 | no output |
| `cargo test --locked --offline --workspace --all-targets --no-fail-fast` | 0 | 90 `test result` lines: **4,833 passed**, 0 failed, 13 ignored |
| `cargo test --locked --offline --workspace --no-fail-fast` | 0 | 106 lines, 16 doctest runs: **4,844 passed**, 0 failed, 13 ignored; the `CustodyCapsuleSealProofV1` doctest at line 541 (the new one) passes |
| `bridge-core` lib, its own line in the default run | 0 | **1,091 passed**, 0 failed |
| `cargo run --locked --offline -p a2a-bridge -- validate --repo-hygiene` | 0 | `repository hygiene validated`; `tracked_artifacts: 41`, `validated_example_configs: 9` |
| `cargo deny check` | 101 | **excluded**: not installed (`error: no such command: deny`) |

**The base totals were measured, not inferred.** The base commit was extracted with `git archive` to `/tmp` and both
workspace suites ran there with a separate target directory.
- All-targets: 4,751 passed and 24 failed, so 4,775 executed. Default: 4,761 passed and 24 failed, so 4,785.
- **The 24 are location artifacts of the extraction, not base defects.** Every one is an `r3d0_*` CLI test that
  requires the checkout to lie under the owner-approved root `/Users/wesleyjinks/code`, or
  `config::tests::worktrees_config_parses_and_preflight`, which requires a root inside the a2a-bridge repository. All
  24 pass in this clone.
- **So the deltas are exact:** all-targets 4,775 → 4,833 (+58), default 4,785 → 4,844 (+59: the 58 lib tests and the
  one doctest), `bridge-core` lib 1,033 → 1,091 (+58), `bridge-core` doctests 9 → 10, and `custody_export::tests` 88 →
  88.
- The 13 ignored tests are the pre-existing live and e2e tests outside `bridge-core`. No flake occurred in either
  workspace run.

## 5. Mutation matrix

**Harness:** `.git/a2a-bridge/mutation/matrix.py` (untracked; it survives the container). It keeps:
- the log in `matrix.log`, and one JSON record per row run in `results.jsonl`, keyed by snapshot;
- raw cargo output per row in `output-<id>.txt`;
- the snapshot of all seven changed source files in `snapshot/`, with `manifest.json`;
- the table below in `table.md`.

**Rules:**
- A row applies only if each of its targets occurs exactly once in the snapshot. `matrix.py check` reports 34 rows and
  0 problems on the final bytes, and every named control exists.
- A `pending.json` marker is written and fsynced before a mutation is applied. It is removed only after every file is
  rewritten from the snapshot, fsynced, its mtime refreshed, and re-hashed equal to the snapshot. Any later invocation
  that finds it restores first, and SIGINT, SIGTERM, and SIGHUP restore before exiting.
- A lib row runs `cargo test --locked --offline -p bridge-core --lib --no-fail-fast -- custody_restore::tests::
  custody_envelope_fixture::tests:: custody_capsule::tests:: custody_export::tests::` with a 420 s cap: every new
  control, and every 2B2 export control, so a stray red in 2B2 is recorded. The visibility row runs
  `cargo test -p bridge-core --doc CustodyCapsuleSealProofV1`, and identifies the new doctest by its fence line.
- **Foreground only.** Every batch ran in the foreground under `timeout 590`. No matrix job ran in the background, and
  none ran while a gate was building.

**Verdict rule.** `FLIPPED` requires that the crate compiled with no timeout, and that every named red control ran
exactly once and failed. Anything else is `NOT-FLIPPED` or `INADMISSIBLE`.

**Rounds.**
1. **Trial** (snapshot `b1ad77c9c7590af8`, 05:38Z–05:53Z): 34 of 34 FLIPPED.
2. **Final** (snapshot `412f94d481773a0b`, 06:07Z–06:23Z, four batches, after the gates): **34 FLIPPED, 0 NOT-FLIPPED,
   0 INADMISSIBLE, 0 not run**. The only source change between the two snapshots is two lines of the test module's
   doc comment.

**Source equals the snapshot:** yes.
- **Harness:** every batch ended `VERIFY OK: 7 files equal their snapshot 412f94d481773a0b; no pending marker`, the last
  at 06:22:58Z. `matrix.log` has 68 `APPLIED` and 68 `RESTORED` lines: 34 in each round.
- **Independent check:** outside the harness, each live file's SHA-256 prefix equals its snapshot copy's: `lib.rs`
  `d11dd9db`, `custody_capsule.rs` `3c43dedd`, `custody_envelope_fixture.rs` `40030a73`, `custody_export.rs`
  `cb9e7d3e`, `custody_export_tests.rs` `b1c7bc0c`, `custody_restore.rs` `4cebbe9b`, and `custody_restore_tests.rs`
  `24d24617`. No `pending.json` exists.
- **After the final pass,** no source file was edited. Only this handoff and the commit message were written.

**Every task 7 row is present:** the seal bound (`M-seal-bound`); the exterior exact-set check (`M-exterior-exact`);
no-follow (`M-no-follow`); the ciphertext length and digest checks (`M-cipher-length`, `M-cipher-digest`); the
destination-empty check (`M-dest-empty`); each overlap direction (`M-overlap-dest-in-capsule`,
`M-overlap-capsule-in-dest`); the pre-write recheck (`M-prewrite-recheck`); the ledger reservation (`M-ledger-dirs`,
`M-ledger-leaf`, `M-ledger-plaintext`); receipt versus measurement (`M-receipt-measure`); staging the payloads before
binding (`M-payloads-before-binding`); the frame drain (`M-frame-drain`); the class and generation expectations
(`M-frame-class`, `M-frame-generation`); the opener requiring the magic in the first chunk
(`M-opener-first-chunk-magic`); the per-artifact zero-length check (`M-published-zero-length`); the consumed-ciphertext
validator (`M-consumed-validator`); the device comparison (`M-device`); the preflight and final census
(`M-census-preflight`, `M-census-final`); the budget ceiling (`M-budget-ceiling`); create-new replaced by
create-or-truncate (`M-create-or-truncate`); the bounded reader replaced by `read_to_end` (`M-bounded-read-to-end`);
the staged-order sort (`M-staged-order-sort`); `pub` visibility (`M-visibility-pub`); and a shared staging directory
re-opened by name (`M-staging-reopen-by-name`). The extras are `M-ledger-leaf`, `M-ledger-plaintext`,
`M-opener-format`, `M-published-limits`, `M-census-fail-closed`, `M-budget-zero`, and `M-staging-recheck`.

**How the less obvious rows mutate:**
- `M-no-follow` stats with `std::fs::metadata` (following) and opens by joined path with `PinnedDirectoryV1::open` and
  `File::open`, both of which follow.
- `M-frame-class` and `M-frame-generation` build the expected header from the frame's own header bytes (class code, or
  generation), as an implementation that trusts the frame would.
- `M-payloads-before-binding` stages every non-control sealed artifact before `bind_control_v1`.
- `M-staging-reopen-by-name` re-opens a retained staging directory by name and replaces the retained pin with it.
- `M-overlap-*` removes one direction from both of 2B2's arms: the canonical-path comparison and the identity walk.

**The strays are expected, not flakes.**
- `M-overlap-dest-in-capsule` also fails 2B2's `control_21_*` (both) and `planned_binding_07`, and
  `M-overlap-capsule-in-dest` fails `control_21_disjointness_preflight…`: the rows remove 2B2's own shared overlap
  predicate, which those controls guard for the exporter.
- `M-staged-order-sort` fails every control that runs the full pipeline to success or to a frame refusal, because the
  checked order invariant then refuses.
- `M-payloads-before-binding` also fails every task 5 refusal control, because each asserts that no payload was staged.

Across all 68 row runs, no `ETXTBSY` or other unrelated failure occurred.

| ID | Task | Guard mutated | Named red | Verdict | Every failing control |
|---|---|---|---|---|---|
| M-seal-bound | T3.2 | the seal is read through the 1 MiB bounded reader | oversize_seal_refuses | FLIPPED | oversize_seal_refuses |
| M-exterior-exact | T3.3 | the capsule exterior is exactly the sealed names plus the seal | capsule_extra_entry_refuses | FLIPPED | capsule_extra_entry_refuses |
| M-no-follow | T3.3 | every capsule entry is stat'ed and opened no-follow | capsule_symlinked_artifact_refuses, capsule_symlinked_component_refuses | FLIPPED | capsule_symlinked_artifact_refuses, capsule_symlinked_component_refuses |
| M-cipher-length | T3.4 | each ciphertext's length equals its sealed length | ciphertext_length_mismatch_refuses | FLIPPED | ciphertext_length_mismatch_refuses |
| M-cipher-digest | T3.4 | each ciphertext's SHA-256 equals its sealed digest | ciphertext_digest_mismatch_refuses, seal_artifact_digest_edit_refuses_before_staging, every_ciphertext_verified_before_the_first_open | FLIPPED | ciphertext_digest_mismatch_refuses, every_ciphertext_verified_before_the_first_open, seal_artifact_digest_edit_refuses_before_staging |
| M-dest-empty | T4 | the destination is empty (2B2 scratch preflight) | destination_must_be_empty | FLIPPED | destination_must_be_empty |
| M-overlap-dest-in-capsule | T4 | overlap: a destination inside the capsule refuses (path and identity) | destination_inside_capsule_refuses | FLIPPED | control_21_a_renamed_worktree_is_refused_by_its_retained_identity_before_any_write, control_21_disjointness_preflight_refuses_every_overlap_before_any_write, destination_inside_capsule_refuses, planned_binding_07_a_scratch_root_aliasing_a_protected_pin_refuses |
| M-overlap-capsule-in-dest | T4 | overlap: a capsule inside the destination refuses (path and identity) | capsule_inside_destination_refuses | FLIPPED | capsule_inside_destination_refuses, control_21_disjointness_preflight_refuses_every_overlap_before_any_write |
| M-prewrite-recheck | T4 | the destination pin is rechecked before the first create | destination_swapped_before_first_write_refuses | FLIPPED | destination_swapped_before_first_write_refuses |
| M-ledger-dirs | T4/T5 | the ledger reserves each created directory's entry before it is created | ledger_max_and_max_plus_one, staging_ledger_max_and_max_plus_one | FLIPPED | ledger_max_and_max_plus_one, prepares_a_new_empty_destination, staging_ledger_max_and_max_plus_one |
| M-ledger-leaf | T5 | the ledger reserves each staged file's entry before it is created | staging_ledger_max_and_max_plus_one | FLIPPED | staging_ledger_max_and_max_plus_one |
| M-ledger-plaintext | T5 | the sink reserves each plaintext chunk before writing it | staging_ledger_max_and_max_plus_one | FLIPPED | staging_ledger_max_and_max_plus_one |
| M-receipt-measure | T5 | the opener's receipt equals the sink's own measurement | opener_lying_receipt_refuses | FLIPPED | opener_lying_receipt_refuses |
| M-payloads-before-binding | T6.0 | no pack or payload is staged before the controls bind | tampered_manifest_refuses_before_payload_staging, seal_manifest_digest_edit_refuses_at_binding, index_mapping_mismatch_refuses, non_inert_policy_refuses | FLIPPED | ciphertext_changed_after_verification_refuses, ciphertext_truncated_after_verification_refuses, index_mapping_mismatch_refuses, non_inert_policy_refuses, opener_lying_receipt_refuses, opener_refusal_propagates, opener_stopping_early_refuses, oversize_control_plaintext_refuses, seal_manifest_digest_edit_refuses_at_binding, staging_collision_refuses, tampered_manifest_refuses_before_payload_staging |
| M-frame-drain | T6.2 | every frame is decoded to its verified trailer (Ok(None)) | corrupt_frame_trailer_refuses | FLIPPED | corrupt_frame_trailer_refuses |
| M-frame-class | T6.2 | the expected header carries the payload's class | wrong_class_frame_refuses | FLIPPED | wrong_class_frame_refuses |
| M-frame-generation | T6.2 | the expected header carries the manifest's generation | wrong_generation_frame_refuses | FLIPPED | wrong_generation_frame_refuses |
| M-opener-first-chunk-magic | T1 | the fixture opener accumulates the magic across chunks (revision 1 bug) | fixture_opener_accepts_split_magic, binds_across_small_chunks | FLIPPED | binds_across_small_chunks, fixture_opener_accepts_split_magic |
| M-opener-format | T1 | the fixture opener refuses an unsupported envelope format | fixture_opener_refuses_an_unsupported_format | FLIPPED | fixture_opener_refuses_an_unsupported_format |
| M-published-zero-length | T2 | from_published_seal_v1 refuses a zero-length artifact | published_seal_refuses_a_zero_length_artifact, malformed_seal_refuses | FLIPPED | malformed_seal_refuses, published_seal_refuses_a_zero_length_artifact |
| M-published-limits | T2 | from_published_seal_v1 applies the selected-artifact limits | published_seal_refuses_an_over_limit_artifact | FLIPPED | published_seal_refuses_an_over_limit_artifact |
| M-consumed-validator | T5 | the consumed ciphertext is the sealed length and digest | ciphertext_changed_after_verification_refuses, opener_stopping_early_refuses | FLIPPED | ciphertext_changed_after_verification_refuses, opener_stopping_early_refuses |
| M-device | T4 | each created directory is on the destination root's device | created_directory_on_another_device_refuses | FLIPPED | created_directory_on_another_device_refuses |
| M-census-preflight | T4 | the mount census runs at preflight | mount_inside_destination_refuses, census_failure_refuses | FLIPPED | census_failure_refuses, mount_appearing_during_restore_refuses, mount_inside_destination_refuses |
| M-census-final | T6.4 | the mount census runs again before VerifiedCapsuleV1 is returned | mount_appearing_during_restore_refuses | FLIPPED | mount_appearing_during_restore_refuses |
| M-census-fail-closed | T4 | a census error refuses (fails closed) | census_failure_refuses | FLIPPED | census_failure_refuses |
| M-budget-ceiling | T4 | CustodyRestoreBudgetV1::new refuses above the 10 GiB ceiling | budget_ceiling_and_ceiling_plus_one | FLIPPED | budget_ceiling_and_ceiling_plus_one |
| M-budget-zero | T4 | CustodyRestoreBudgetV1::new refuses 0 | budget_ceiling_and_ceiling_plus_one | FLIPPED | budget_ceiling_and_ceiling_plus_one |
| M-create-or-truncate | T5 | every staged file is created new (never truncated, never through a link) | staging_collision_refuses | FLIPPED | staging_collision_refuses |
| M-bounded-read-to-end | T5 | the bounded reader never requests or allocates past bound + 1 | bounded_reader_never_requests_past_the_probe | FLIPPED | bounded_reader_never_requests_past_the_probe |
| M-staged-order-sort | T6.5 | staged plaintexts are in the index's canonical order | staged_order_equals_index_order, verifies_and_stages_a_plan_backed_capsule | FLIPPED | capsule_is_never_written, corrupt_frame_trailer_refuses, mount_appearing_during_restore_refuses, nothing_outside_restore_work, staged_order_equals_index_order, verifies_and_stages_a_fixture_stream_capsule, verifies_and_stages_a_plan_backed_capsule, wrong_class_frame_refuses, wrong_generation_frame_refuses |
| M-visibility-pub | T2 | from_published_seal_v1 is crate-private | from_published_seal_is_crate_private | FLIPPED | from_published_seal_is_crate_private |
| M-staging-reopen-by-name | T5 | a shared staging directory is reused by its retained pin, never re-opened by name | shared_staging_directory_custody | FLIPPED | shared_staging_directory_custody |
| M-staging-recheck | T5 | the retained staging chain is rechecked before each staged file's create | shared_staging_directory_custody | FLIPPED | shared_staging_directory_custody |

## 6. Exclusions

1. **`cargo deny check`:** not run, because `cargo-deny` is not installed here (exit 101). CI runs it. No dependency,
   feature, or `Cargo.lock` change was made.
2. **The controller's macOS lane:** not available in this container. The reader uses only `fs_custody` primitives that
   already run on macOS, and the census's `getfsstat` path is unchanged and unexercised here.
3. **The native ext4 lane:** CI.
4. **Non-unix targets:** `custody_restore` is `#[cfg(unix)]`; the fixture module is `#[cfg(test)]`, and the Windows CI
   job builds `bridge-core` only as a non-test dependency; `from_published_seal_v1` carries
   `#[cfg_attr(not(unix), allow(dead_code))]`.
5. **Container proxy:** every workspace run unset the proxy variables.

## 7. The design §5 refinement, and interpretations the reviewer should check

**The refinement (task Description).** Design §5 says tampering refuses "before any plaintext is staged". The seal has
no independently supplied expected digest in 2B, and the controls are encrypted artifacts, so a canonical seal that
only disagrees with the controls is detectable only after they are opened. The implementation holds the task's
three-tier contract, each tier with named controls:

| Tier | Refuses | Staged when refused | Controls |
|---|---|---|---|
| T1: malformed or out-of-bounds seal | task 3, `SealUnreadable` / `SealInvalid` | nothing; no `.restore-work/` | `oversize_seal_refuses`, `malformed_seal_refuses` |
| T2: seal/ciphertext disagreement | task 3, `CiphertextLength` / `CiphertextDigest` | nothing; no `.restore-work/` | `seal_artifact_digest_edit_refuses_before_staging`, `ciphertext_*_mismatch_refuses`, `every_ciphertext_verified_before_the_first_open` |
| T3: seal/control semantic disagreement | task 5, `Binding` / `ControlDecode` | only the three control plaintexts | `seal_manifest_digest_edit_refuses_at_binding`, `tampered_manifest_…`, `index_mapping_…`, `non_inert_policy_refuses` |

**Interpretations:**
1. **Test-harness visibility beyond the task's list.** The task names `HarnessV1`, `new`, `run_sealed`,
   `PlanFixtureV1`, "the `PlanFixtureV1` methods these tests call", and the `capsule_dir()` accessors. Two task
   fixtures, "a second harness export whose generation differs" and "a second plan-backed export with a different
   generation", also need `HarnessV1`'s `manifest` and `streams` fields, `IDENTITY_V1`, `manifest_for`,
   `default_streams`, `manifest_from_plan`, `planned_capability`, and `expect_sealed`. These were made `pub(crate)`,
   exactly as 2B2's own control 18 uses them. Every one is `#[cfg(test)]`, no function body changed, and no method was
   added other than the two accessors.
2. **The T1/T2 controls run twice.** Each asserts its error from `pin_and_verify_capsule_v1` and again through
   `verify_and_stage_v1`, where it also asserts no destination entry and 0 opener calls. The task 5 refusal controls
   run the whole pipeline too, and each asserts no `plain/payload` or `plain/git`. The pipeline arms were added with
   task 6.
3. **Control staging order.** The controls are staged manifest, index, policy (decode order). `staged` is then sorted
   into the index's order, and that order is checked. This makes the task's "staged-order sort" row discriminating.
4. **Error names.** Capsule refusals name the logical capsule path (`payload/worktree.bin.enc`). Destination refusals
   name the destination-relative path (`.restore-work`, `.restore-work/plain/control/manifest.json.enc`), matching the
   task's `DeviceCrossing{".restore-work"}`.
5. **Collision versus replacement.** A planted entry where the restore creates one is `StagingCollision`. A retained
   staging directory whose entry no longer matches its pin is `IdentityChanged`. A planted symlink at a directory name
   fails `mkdirat` with `EEXIST`, so it is a collision too.
6. **What the ledger charges.** One 64 KiB entry allowance (2B2's) per created directory and file, and each plaintext
   chunk's bytes before it is written. Ciphertext reads charge nothing.
7. **`CiphertextChanged` covers a short read.** A retained ciphertext truncated after verification cannot serve its
   sealed length; that is `CiphertextChanged`, the same as a consumed-digest mismatch.
8. **`OpenerReceiptMismatch` covers an unfinished stream.** An opener that returns `Ok` without a final plaintext chunk
   fails the sink validator's `finish`; that is reported as a receipt that disagrees with the bytes written.
9. **`recheck_containment` is the census only,** as the task states. The staging chain is rechecked by identity before
   every create.
10. **The overlap refusal's text is 2B2's** ("the scratch root … the protected source path …"), wrapped in
    `DestinationInvalid`, because the task mandates reusing `preflight_scratch_root` unchanged.
11. **The destination root is followed once, at preflight,** by 2B2's `canonicalize`, as for a scratch root; so is the
    capsule root by `PinnedDirectoryV1::open`. Nothing beneath either root is followed.
12. **Chunking.** The reader opens in 1 MiB chunks (the 2B1 ceiling) with at most 16,384 chunks. The
    `override_chunk_bytes_for_test` seam makes a small capsule span many chunks; without it, `opener_stopping_early`
    could not stop early on a one-chunk control.

**In-lane self-check.** Before the final matrix, the author re-read `custody_restore.rs` against design §3.1 and §4
and the task's tasks 3–6. That pass removed an unreachable device re-comparison from the staging recheck (the
retained `dev` is compared when each directory is created and is part of the identity check) and added
`ciphertext_truncated_after_verification_refuses`. No separate reviewer ran in this turn, so this is not the
implementation review.

## 8. Owned paths and staged changes

Exactly the task's Files paths changed. The harness, the gates, and the RED output live under `.git/a2a-bridge/` and
are not part of the diff.

```text
crates/bridge-core/src/custody_envelope_fixture.rs   (new: the moved sealer, the opener, 6 controls)
crates/bridge-core/src/custody_restore.rs            (new: phase V)
crates/bridge-core/src/custody_restore_tests.rs      (new: 48 controls and the reseal fixture)
crates/bridge-core/src/custody_capsule.rs            (modified: the additive constructor, its doctest, 4 controls)
crates/bridge-core/src/custody_export_tests.rs       (modified: the fixture moved out; test-only visibility; 2 accessors)
crates/bridge-core/src/custody_export.rs             (modified: `pub(crate) mod tests;` only)
crates/bridge-core/src/lib.rs                        (modified: the two module declarations)
docs/superpowers/reviews/2026-09-28-adr0041-slice2b3a-implementation-handoff.md (new)
```

Immediately before staging, every source file's SHA-256 still equaled the final snapshot, and
`git diff --cached --check` exits 0.

## 9. What remains for the controller

1. The Sol implementation review, under its two-round cap. It should check §7, especially items 1 (harness
   visibility), 2 (which controls run the pipeline), and 3 (the staging order).
2. The controller's macOS lane: the restore controls against APFS and the real `getfsstat` census, and the gates.
3. CI: native ext4, the Windows build, `cargo deny`, and coverage.
4. 2B3b (the Git plane), which consumes `VerifiedCapsuleV1`: `IndexPackStrictStdin` on the staged pack and 2B2's
   closure proof.
5. The parent plan, the roadmap, and the planning handoff, which the controller alone updates.

## 10. Repair round 1 (Sol implementation review, round 1)

The review's verdict was **REJECT**, with one blocker. This round fixes that blocker only, inside the task's Files
paths. Every result below is on the final bytes, snapshot `3e60cf312cf06e7c`. Raw output is in
`.git/a2a-bridge/red/repair1-*` and `.git/a2a-bridge/gates/repair1-*`.

### 10.1 Finding B1 (WRONG MATERIAL): the final gate did not revalidate the destination's identity

- The only check before `Ok(VerifiedCapsuleV1)` was `destination.recheck_containment()`, which is the mount census
  alone.
- `stage_one_v1` rechecks the staging chain before `BeforeLeafCreate`. It then creates the leaf through the retained
  parent descriptor, and the frame is verified through retained descriptors.
- **The failure case.** During the last indexed artifact's `BeforeLeafCreate`, a racer renames either the destination
  root or that artifact's retained parent staging directory out of the destination, and puts an empty directory at the
  name. The restore still returned `Ok`, although the named destination was incomplete.
- The existing `shared_staging_directory_custody` swap is caught only because a later staging call rechecks. No check
  follows the last one.
- §7 item 9 described this gap as the intended design. §10.3 supersedes it.

### 10.2 RED

| Stage | Reader under test | Controls run | Result |
|---|---|---|---|
| A (`repair1-red-a.txt`) | exactly `7e63ed8d`'s `custody_restore.rs` (SHA-256 `4cebbe9b…`); only the tests changed | the root swap, the staging-parent swap, and the no-swap control | root swap FAILED and parent swap FAILED, each `observed Ok("VerifiedCapsuleV1")`; no-swap ok. 1 passed, 2 failed |
| B (`repair1-red-b.txt`) | `7e63ed8d` plus only the `#[cfg(test)]` hook point `BeforeFinalGate` (`repair1-red-b-seam.diff`, 7 added lines) | adds the `.restore-work` swap | all three swap controls FAILED with `observed Ok("VerifiedCapsuleV1")`; no-swap ok. 1 passed, 3 failed |
| Final tests (`repair1-red-final-tests.txt`) | the stage B reader (`c73dadb9…`), swapped in temporarily, then restored to the fixed reader (`320c9065…`) and checked by hash | the final test bytes | the same: 3 FAILED with `observed Ok("VerifiedCapsuleV1")`, no-swap ok |

- **Why the final-tests row exists.** After stage B, each refusal assertion was tightened to name the check that must
  fire. On the RED reader, each still observes `Ok`.
- **Why the third control needed a new hook point.** The existing hook points (`AfterCiphertextVerification`,
  `BeforeFirstCreate`, and `BeforeLeafCreate`) all run before the last frame verification.
  - The census seam's second call does run later, but it is inside the gate. The brief's order runs it after the
    identity checks, so a swap there would come after them.
  - So one test-only hook point, `BeforeFinalGate`, was added. It runs after the frame loop and immediately before the
    gate. It is compiled only under `#[cfg(test)]` and has no effect outside tests.

### 10.3 Fix (`custody_restore.rs`, private)

A new method, `RestoreDestinationV1::final_gate(&self)`, is called only from `verify_and_stage_v1`, in place of the
bare `recheck_containment()` call. It runs three checks in order:
1. **The root pin.** `pinned_root_unchanged(&self.root)` requires the destination's name to still resolve to its
   pinned identity; otherwise `IdentityChanged`.
2. **The staging chains.**
   - First, `recheck_staging_chain(b"")` checks `.restore-work/` against the root's entry, and `plain/` against
     `.restore-work/`'s entry.
   - Then `recheck_staging_chain(key)` runs for every retained staging key. It checks each component against its
     parent's no-follow entry: it must be a directory with the retained `dev` and `ino`, otherwise `IdentityChanged`.
3. **The census.** `recheck_containment()` runs last, unchanged. So "`recheck_containment()` runs before
   `VerifiedCapsuleV1` is returned" (task 6 step 4) still holds.

Notes on the fix:
- A swap during frame verification is closed by the same gate, because the gate runs only after every frame is
  verified. The `.restore-work` control swaps at the latest point of that window.
- No existing check changed, and no error variant was added.
- **The empty-key call.** In every reachable state at least `control/` is retained, and every key's chain starts at
  `.restore-work/` and `plain/`. So the empty-key call is redundant there. It guarantees that those two are rechecked
  even if no staging directory were retained. The matrix row mutates the call and the loop as one unit.

### 10.4 Controls (4 new, in `custody_restore::tests`)

The helper `restore_with_last_leaf_swap` does the following:
- It exports a fresh plan-backed capsule.
- It takes the seal's last artifact (`payload/worktree.bin.enc`) as the last one staged. The seal and the index are in
  canonical name order, and the controls stage first.
- It counts every `BeforeLeafCreate`. It asserts that the swapped leaf is the last create, and that every artifact
  reached its leaf create.

`swap_out` renames an object out of the destination and puts an empty 0700 directory at its name.

| Control | Swap | Asserts after the fix |
|---|---|---|
| `final_gate_refuses_a_destination_root_swapped_at_the_last_leaf` | at the last artifact's `BeforeLeafCreate`, `dest` is renamed to the sibling `dest-moved`, and `dest` is recreated empty | `IdentityChanged` from the root pin ("now resolves to a different directory"); the last leaf is in `dest-moved`; `dest` is empty |
| `final_gate_refuses_a_staging_parent_swapped_at_the_last_leaf` | at the same point, `.restore-work/plain/payload/` is renamed to `parent-moved`, outside the destination, with an empty replacement | `IdentityChanged` naming `.restore-work/plain/payload`; the leaf is in `parent-moved`; the replacement is empty |
| `final_gate_refuses_restore_work_swapped_after_the_last_frame` | at `BeforeFinalGate`, `.restore-work/` is renamed to `restore-work-moved`, with an empty replacement | `IdentityChanged` naming `.restore-work`; the moved tree holds `plain/payload/`; the replacement is empty |
| `final_gate_admits_an_unswapped_restore` | both hooks are installed and change nothing | `Ok`; the leaf hook ran; the gate hook ran exactly once; every index row is staged; the last leaf is at its named path |

### 10.5 Mutation rows

**Harness changes (`matrix.py`):**
- Two new rows: `M-final-root-identity` and `M-final-staging-chain`.
- `M-census-final` is retargeted. Its target line, `destination.recheck_containment()?;`, moved into `final_gate` as
  `self.recheck_containment()`. The mutation is the same in kind: the final census is removed.
- `matrix.py check` reports **36 rows, 0 problems** on the snapshot.

**The run.** One foreground run on the final bytes, snapshot `3e60cf312cf06e7c`, from 06:54:52Z to 06:58:11Z, under
`timeout 590`. **8 of 8 FLIPPED**, with 0 NOT-FLIPPED and 0 INADMISSIBLE.

| ID | Task | Guard mutated | Named red | Verdict | Every failing control |
|---|---|---|---|---|---|
| M-final-root-identity | R1/B1 | the final gate proves the destination root pin unchanged | final_gate_refuses_a_destination_root_swapped_at_the_last_leaf | FLIPPED | final_gate_refuses_a_destination_root_swapped_at_the_last_leaf |
| M-final-staging-chain | R1/B1 | the final gate rechecks .restore-work/, plain/, and every retained staging chain | final_gate_refuses_a_staging_parent_swapped_at_the_last_leaf, final_gate_refuses_restore_work_swapped_after_the_last_frame | FLIPPED | final_gate_refuses_a_staging_parent_swapped_at_the_last_leaf, final_gate_refuses_restore_work_swapped_after_the_last_frame |
| M-census-final | T6.4 | the mount census runs again before VerifiedCapsuleV1 is returned | mount_appearing_during_restore_refuses | FLIPPED | mount_appearing_during_restore_refuses |
| M-staged-order-sort | T6.5 | staged plaintexts are in the index's canonical order | staged_order_equals_index_order, verifies_and_stages_a_plan_backed_capsule | FLIPPED | capsule_is_never_written, corrupt_frame_trailer_refuses, final_gate_admits_an_unswapped_restore, final_gate_refuses_a_destination_root_swapped_at_the_last_leaf, final_gate_refuses_a_staging_parent_swapped_at_the_last_leaf, final_gate_refuses_restore_work_swapped_after_the_last_frame, mount_appearing_during_restore_refuses, nothing_outside_restore_work, staged_order_equals_index_order, verifies_and_stages_a_fixture_stream_capsule, verifies_and_stages_a_plan_backed_capsule, wrong_class_frame_refuses, wrong_generation_frame_refuses |
| M-payloads-before-binding | T6.0 | no pack or payload is staged before the controls bind | tampered_manifest_refuses_before_payload_staging, seal_manifest_digest_edit_refuses_at_binding, index_mapping_mismatch_refuses, non_inert_policy_refuses | FLIPPED | ciphertext_changed_after_verification_refuses, ciphertext_truncated_after_verification_refuses, final_gate_admits_an_unswapped_restore, final_gate_refuses_a_destination_root_swapped_at_the_last_leaf, final_gate_refuses_a_staging_parent_swapped_at_the_last_leaf, index_mapping_mismatch_refuses, non_inert_policy_refuses, opener_lying_receipt_refuses, opener_refusal_propagates, opener_stopping_early_refuses, oversize_control_plaintext_refuses, seal_manifest_digest_edit_refuses_at_binding, staging_collision_refuses, tampered_manifest_refuses_before_payload_staging |
| M-staging-recheck | T5 | the retained staging chain is rechecked before each staged file's create | shared_staging_directory_custody | FLIPPED | shared_staging_directory_custody |
| M-prewrite-recheck | T4 | the destination pin is rechecked before the first create | destination_swapped_before_first_write_refuses | FLIPPED | destination_swapped_before_first_write_refuses |
| M-staging-reopen-by-name | T5 | a shared staging directory is reused by its retained pin, never re-opened by name | shared_staging_directory_custody | FLIPPED | shared_staging_directory_custody |

- **Which rows were run.**
  - The two new rows.
  - The three whose targets are in the changed function `verify_and_stage_v1`, or were moved out of it:
    `M-census-final`, `M-staged-order-sort`, and `M-payloads-before-binding`.
  - The three that guard the pre-create identity checks and retained pins the gate re-walks: `M-staging-recheck`,
    `M-prewrite-recheck`, and `M-staging-reopen-by-name`.
- **The 28 rows not re-run** target functions this round did not change. Their FLIPPED verdicts (§5) are on snapshot
  `412f94d481773a0b`. The five files this round did not touch are byte-identical in both snapshots.
- **Each new row flips only its own controls.**
  - Removing the root check leaves the root-swap control `Ok`: the chain recheck walks the retained descriptors inside
    the renamed tree, and they all agree.
  - Removing the chain recheck leaves the two directory-swap controls `Ok`: the root's name still resolves to its pin.
- **The strays are expected.**
  - `M-staged-order-sort` refuses every pipeline run at the order check, before the gate.
  - Under `M-payloads-before-binding` the last leaf create is a control, so the helper's last-leaf assertion fails.
    The `.restore-work` control does not use the helper, so it still refuses at the gate.

**The source equals the snapshot.**
- The harness logged `VERIFY OK: 7 files equal their snapshot 3e60cf312cf06e7c; no pending marker` at 06:58:11Z.
- Outside the harness, each live file's SHA-256 prefix equals its snapshot copy's:

  | File | SHA-256 prefix |
  |---|---|
  | `lib.rs` | `d11dd9db` |
  | `custody_capsule.rs` | `3c43dedd` |
  | `custody_envelope_fixture.rs` | `40030a73` |
  | `custody_export.rs` | `cb9e7d3e` |
  | `custody_export_tests.rs` | `b1c7bc0c` |
  | `custody_restore.rs` | `320c9065` |
  | `custody_restore_tests.rs` | `0786d21d` |

- `cmp` finds the two changed files byte-identical to their snapshot copies. Each of the seven has the restore's
  fresh mtime, 06:58:11Z. No `pending.json` exists.
- `matrix.log` has 76 `APPLIED` and 76 `RESTORED` lines: 68 from round 0 and 8 here.
- After this run, no source file was edited. The gates below then ran, and only this handoff was written.

### 10.6 Gates, on the final bytes after the matrix run

Every cargo command ran with `CARGO_HOME=/cargo CARGO_NET_OFFLINE=true CARGO_TARGET_DIR=/tmp/target
CARGO_INCREMENTAL=0`, with `HTTP_PROXY`, `HTTPS_PROXY`, `http_proxy`, and `https_proxy` unset.

| Gate | Exit | Totals |
|---|---|---|
| `cargo fmt --all -- --check` | 0 | no diff |
| `cargo clippy --locked --offline --workspace --all-targets -- -D warnings` | 0 | no warning or error |
| `cargo test --locked --offline -p bridge-core --lib --no-fail-fast` | 0 | **1,095 passed**, 0 failed (1,091 + the 4 new controls) |
| `cargo test --locked --offline --workspace --all-targets --no-fail-fast` | 0 | 90 `test result` lines: **4,837 passed**, 0 failed, 13 ignored (4,833 + 4) |
| `cargo test --locked --offline --workspace --no-fail-fast` | 0 | 106 lines, 16 doctest runs: **4,848 passed**, 0 failed, 13 ignored (4,844 + 4); the visibility doctest at line 541 passes |
| `git diff --cached --check` after staging | 0 | no output |

- **The lib count.** The `--lib` run also prints one nested child-process line (1 passed, the rest filtered out), as
  round 0's did. 1,095 is the full lib line.
- **Excluded:** as in §6.

### 10.7 Staged paths

```text
crates/bridge-core/src/custody_restore.rs        (the final gate; one #[cfg(test)] hook point)
crates/bridge-core/src/custody_restore_tests.rs  (4 controls and 2 helpers)
docs/superpowers/reviews/2026-09-28-adr0041-slice2b3a-implementation-handoff.md (§10 and a status pointer)
```

Nothing is committed in this round, and `.git/A2A_COMMIT_MSG` is not written.

## 11. Repair round 2 (Sol implementation review, round 2)

The review's verdict was **REJECT**, with one blocker: B1, still unresolved. This round fixes that blocker only, inside
the task's Files paths. Every result below is on the final bytes, snapshot `ab2972aa675e4e3f`. Raw output is in
`.git/a2a-bridge/red/repair2-*` and `.git/a2a-bridge/gates/repair2-*`.

### 11.1 Finding B1, round 2 (WRONG MATERIAL): the final gate checked identity before the census

- Round 1's `final_gate` ran the root pin first, then the staging-chain sweep, and the mount census last. The census
  may be slow. `verify_and_stage_v1` then returned `Ok` with no further identity check.
- **The failure case.** Census call 1 (preflight) returns the ordinary list. During census call 2 (the final gate), a
  racer renames `dest` to `dest-moved`, creates an empty `dest`, and the census returns only `/`. Every identity check
  had already passed, so the restore returned `Ok(VerifiedCapsuleV1)` while the named destination was empty.
- §10.2 noted that a swap inside the census would come after the identity checks, but took that as the brief's order.
  §11.3 supersedes §10.3's order.

### 11.2 RED

| Stage | Reader under test | Controls run | Result |
|---|---|---|---|
| RED (`repair2-red.txt`) | exactly `59b252c7`'s `custody_restore.rs` (SHA-256 `320c9065…`, equal to `git show HEAD:`); only the new control was added to the tests | the new control, the four round-1 controls, and `mount_appearing_during_restore_refuses` | the new control FAILED at `custody_restore_tests.rs:1981` with `observed Ok("VerifiedCapsuleV1")`; the other five ok. 5 passed, 1 failed |
| GREEN (`repair2-green.txt`) | the fixed reader (`1b5dd96d…`), with the same test bytes (`4efae0bb…`) | the same six | 6 passed, 0 failed |

- The test bytes did not change between RED and GREEN, or afterwards.
- No new hook point was needed. The census seam's second call is the final gate's census.

### 11.3 Fix (`custody_restore.rs`, `final_gate` only)

The gate's three checks are unchanged. Only their order changed:
1. **The census.** `recheck_containment()` runs first, because it is the check that may be slow.
2. **The staging chains.** `recheck_staging_chain(b"")` runs, then the check for every retained key, as in §10.3.
3. **The root pin.** `pinned_root_unchanged(&self.root)` runs last. It is the last fallible check before
   `Ok(VerifiedCapsuleV1)`.

- **Why the root pin is last.**
  - The chain sweep reads each entry through retained descriptors. A root rename carries those descriptors along
    intact, so the sweep still agrees after a root swap (§10.5). Only the root pin sees a root swap.
  - Because it runs last, the root pin sees a root swap made during the census or during the sweep.
- **What else changed.**
  - The doc comment now states the new order and the boundary in §11.6.
  - No other code changed. No error variant or hook point was added, and `verify_and_stage_v1` is byte-identical.
- **The root pin's form.** The root pin is written as `…?;` followed by `Ok(())`, not as a tail expression. So the
  persisted `M-final-root-identity` target applies unchanged.

### 11.4 Control (1 new, in `custody_restore::tests`)

| Control | Swap | Asserts after the fix |
|---|---|---|
| `final_gate_refuses_destination_root_swapped_during_final_census` | The census seam returns `["/"]` on call 1 (preflight). On call 2 (the final gate), it renames `dest` to the sibling `dest-moved`, recreates `dest` empty with mode 0700, and returns `["/"]` | `IdentityChanged` from the root pin ("now resolves to a different directory"); exactly 2 census calls; `dest` is empty; every sealed artifact is a file under `dest-moved/.restore-work/plain/` |

The four round-1 controls and `mount_appearing_during_restore_refuses` pass unchanged: see the GREEN row in §11.2 and
the gates in §11.7.

### 11.5 Mutation rows

**Harness changes (`matrix.py`):**
- **A new row, `M-final-census-first`.** It replaces the fixed gate body with round 1's body, which moves the census
  back after the identity sweep. Before the run, the mutated `final_gate` was checked to be byte-identical to
  `59b252c7`'s `final_gate`, from its signature to its closing brace.
- **`M-census-final` is retargeted.** Its round-1 target, the tail `self.recheck_containment()`, is now the first line,
  `self.recheck_containment()?;`. The mutation is the same in kind: the final census is removed.
- **Unchanged rows.** `M-final-root-identity` and `M-final-staging-chain` are unchanged, because their target bytes
  are unchanged.
- `matrix.py check` reports **37 rows, 0 problems** on the snapshot.

**The run.** There was one foreground run on the final bytes, snapshot `ab2972aa675e4e3f`, from 07:19:58Z to
07:21:37Z, under `timeout 590`. **4 of 4 FLIPPED**, with 0 NOT-FLIPPED and 0 INADMISSIBLE.

| ID | Task | Guard mutated | Named red | Verdict | Every failing control |
|---|---|---|---|---|---|
| M-final-census-first | R2/B1 | the final gate censuses first and ends with the root pin (mutated: the census moved back after the identity sweep) | final_gate_refuses_destination_root_swapped_during_final_census | FLIPPED | final_gate_refuses_destination_root_swapped_during_final_census |
| M-final-root-identity | R1/B1 | the final gate proves the destination root pin unchanged | final_gate_refuses_a_destination_root_swapped_at_the_last_leaf | FLIPPED | final_gate_refuses_a_destination_root_swapped_at_the_last_leaf, final_gate_refuses_destination_root_swapped_during_final_census |
| M-final-staging-chain | R1/B1 | the final gate rechecks .restore-work/, plain/, and every retained staging chain | final_gate_refuses_a_staging_parent_swapped_at_the_last_leaf, final_gate_refuses_restore_work_swapped_after_the_last_frame | FLIPPED | final_gate_refuses_a_staging_parent_swapped_at_the_last_leaf, final_gate_refuses_restore_work_swapped_after_the_last_frame |
| M-census-final | T6.4 | the mount census runs again before VerifiedCapsuleV1 is returned | mount_appearing_during_restore_refuses | FLIPPED | final_gate_refuses_destination_root_swapped_during_final_census, mount_appearing_during_restore_refuses |

- **Which rows were run.** The new row ran, and so did the three rows whose targets are in `final_gate`, the only
  changed function.
- **The 33 rows not re-run** target functions this round did not change.
  - 28 have FLIPPED verdicts on snapshot `412f94d481773a0b` (§5), and 5 on snapshot `3e60cf312cf06e7c` (§10.5).
  - The five files this round did not touch are byte-identical across all three snapshots.
  - `table.md` lists those 33 rows as "not run", because it counts only rows run on the current snapshot.
- **`M-final-census-first` flips only the new control.** The round-1 controls swap before the gate, so the gate catches
  them in either order.
- **The strays are expected.**
  - Under `M-final-root-identity`, the new control also fails. Without the root pin, nothing sees a root swap.
  - Under `M-census-final`, the new control also fails. With no second census the swap never happens, the result is
    `Ok`, and the seam counts one call.

**The source equals the snapshot.**
- The harness logged `VERIFY OK: 7 files equal their snapshot ab2972aa675e4e3f; no pending marker` at 07:21:37Z.
- Outside the harness, each live file's SHA-256 prefix equals its snapshot copy's, and all seven equal the manifest:

  | File | SHA-256 prefix |
  |---|---|
  | `lib.rs` | `d11dd9db` |
  | `custody_capsule.rs` | `3c43dedd` |
  | `custody_envelope_fixture.rs` | `40030a73` |
  | `custody_export.rs` | `cb9e7d3e` |
  | `custody_export_tests.rs` | `b1c7bc0c` |
  | `custody_restore.rs` | `1b5dd96d` |
  | `custody_restore_tests.rs` | `4efae0bb` |

- `cmp` finds all seven byte-identical to their snapshot copies. Each has the restore's fresh mtime, 07:21:37Z. No
  `pending.json` exists.
- `matrix.log` has 80 `APPLIED` and 80 `RESTORED` lines: 76 from earlier rounds and 4 here.
- A `/proc` scan after the run found no cargo, rustc, or matrix process running.
- **After this run, no snapshot file was edited.** `table.md` was rendered, and the gates below ran on those bytes.
  Then only this handoff was written; it is outside the snapshot.

### 11.6 The declared guarantee boundary of the final gate

- **What the gate proves.** At its last check, the gate proves three things:
  - the final census saw no mount point inside the destination;
  - `.restore-work/`, `plain/`, and every retained staging directory are still the entries their parents hold;
  - the destination's name still resolves to the root pin.
- **The residual.** No gate can detect a change made after the last check instruction. There is always a last
  instruction. This round documents that residual and does not try to fix it.
- **Why it is out of scope.** Under the owner ruling, a hostile same-user racer is out of scope. The restore detects
  changes and refuses at its gate points. It does not claim atomicity against a concurrent writer that runs as the
  same user.
- **What consumers rely on.** 2B3b and later consumers operate through the retained pins in `VerifiedCapsuleV1`, not by
  path:
  - the destination's root, `.restore-work/`, `plain/`, and staging-directory descriptors;
  - each staged plaintext's open descriptor.

  So a rename after the gate cannot redirect what they read. It can only mean that the named path no longer shows the
  staged tree.

### 11.7 Gates, on the final bytes after the matrix run

Every cargo command ran with `CARGO_HOME=/cargo CARGO_NET_OFFLINE=true CARGO_TARGET_DIR=/tmp/target
CARGO_INCREMENTAL=0`, with `HTTP_PROXY`, `HTTPS_PROXY`, `http_proxy`, and `https_proxy` unset.

| Gate | Exit | Totals |
|---|---|---|
| `cargo fmt --all -- --check` | 0 | no diff |
| `cargo clippy --locked --offline --workspace --all-targets -- -D warnings` | 0 | no warning or error |
| `cargo test --locked --offline -p bridge-core --lib --no-fail-fast` | 0 | **1,096 passed**, 0 failed (1,095 + the 1 new control) |
| `cargo test --locked --offline --workspace --all-targets --no-fail-fast` | 0 | 90 `test result` lines: **4,838 passed**, 0 failed, 13 ignored (4,837 + 1) |
| `cargo test --locked --offline --workspace --no-fail-fast` | 0 | 106 lines, 16 doctest runs: **4,849 passed**, 0 failed, 13 ignored (4,848 + 1); the visibility doctest at line 541 passes |
| `git diff --cached --check` after staging | 0 | no output |

- **The lib count.** As in §10.6, the `--lib` run also prints one nested child-process line (1 passed). 1,096 is the
  full lib line.
- **Excluded:** as in §6.

### 11.8 Staged paths

```text
crates/bridge-core/src/custody_restore.rs        (final_gate's order and its doc comment)
crates/bridge-core/src/custody_restore_tests.rs  (1 control)
docs/superpowers/reviews/2026-09-28-adr0041-slice2b3a-implementation-handoff.md (§11 and a status pointer)
```

Nothing is committed in this round, and `.git/A2A_COMMIT_MSG` is not written.
