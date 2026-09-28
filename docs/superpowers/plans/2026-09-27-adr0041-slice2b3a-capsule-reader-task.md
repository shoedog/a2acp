---
task-type: implement
---
# ADR-0041 Slice 2B3a — capsule reader (phase V: verify and stage) Implementation Plan

> **For agentic workers:** this plan is executed by Opus 5.5 through `a2a-bridge implement` (the owner's standing
> directive), after a Sol spec review. Work task by task: RED first, then GREEN, then the next task. Steps use
> checkbox (`- [ ]`) syntax.

**Goal:** A restore can pin a sealed capsule and a new, empty destination, verify every ciphertext against the seal,
open every artifact through a fixture opener into a bounded staging area, bind the manifest, index, and restore
policy through 2B1, and fully decode-verify every coverage frame. It does all of this before anything is
materialized.

**Architecture:** A new crate-private, `#[cfg(unix)]` module, `custody_restore.rs`, exposes `verify_and_stage_v1`,
which returns a `VerifiedCapsuleV1`. It reuses:
- 2B2's scratch preflight and `ScratchLedgerV1`;
- `fs_custody` retained-descriptor primitives;
- 2B1's `CustodyCapsuleBindingV1`;
- the 2B2b1 frame decoder.

The test-only envelope fixture (sealer plus a new opener) moves into one shared module. `custody_capsule.rs` gains
one validating constructor for a seal read from disk.

**Tech stack:** Rust 1.94.0 (pinned), `bridge-core`, `ring` (SHA-256), and the existing `fs_custody` unsafe
boundary. No new dependency.

**Spec:** `docs/superpowers/specs/2026-09-27-adr0041-slice2b3-restore-design.md` (§3.1 phase V, §4 invariants, and
the §5 row for 2B3a).

**Implementation base:** current `main`; bind the exact SHA at dispatch.

## Description

2B3a is the first of four serial children (2B3a reader → 2B3b Git plane → 2B3c payload plane → 2B3d record and
proofs). It implements design §3.1 **phase V only**. It writes only under `<dest>/.restore-work/`, never under
`repository/` or `evidence/`. It runs no Git command and decodes nothing into the destination tree.

**Design refinement (stated for review):** design §5 says tampering "refuses before any plaintext is staged". That
holds literally for seal-level and ciphertext-level tampering. The manifest, index, and restore policy are themselves
encrypted artifacts, though, so they must be opened before they can be checked. This plan therefore:
1. verifies every ciphertext against the seal;
2. stages **only the three control artifacts**;
3. binds them through 2B1;
4. only then stages the pack and the coverage payloads.

Control-artifact tampering therefore refuses before **any pack or payload plaintext** is staged.

## Global Constraints

- Rust 1.94.0 is pinned by `rust-toolchain.toml`. No new crate dependency.
- **Where code goes:**
  - `custody_restore.rs` is `#[cfg(unix)]` and crate-private, with `#[allow(dead_code)]` until 2B3b uses it. It is
    declared in `lib.rs` next to `custody_export`.
  - The new constructor in `custody_capsule.rs` is **additive**; no existing validation is weakened.
  - The shared envelope fixture is `#[cfg(test)]` only.
  - No change to `custody_frame.rs`, `custody_walk.rs`, `custody_coverage.rs`, `custody_git.rs`, or `custody_mounts.rs`.
- **Filesystem access:**
  - Every filesystem read goes through retained, no-follow descriptors (`PinnedDirectoryV1`,
    `open_existing_child_directory`, `open_regular_file`).
  - Every write is create-new beneath the retained destination descriptor.
  - Never overwrite, never follow a symlink, never delete, not even on failure.
- **Budget:** one restore-wide `ScratchLedgerV1` reserves every staged byte and entry before writing it. The ceiling
  is 10 GiB, the same as 2B2 §3.
- **Bounded control reads:** the seal file and each control plaintext are read through a bounded read (1 MiB, the
  2B1 canonical metadata ceiling); oversize refuses.
- **Capsule exterior:** exactly the seal's artifact files plus `custody-seal.v1`. Any other entry refuses (review
  focus 2).
- `cargo` gates as in design §7: `--no-fail-fast`, `CARGO_INCREMENTAL=0`, and the proxy variables unset in the
  container.

## Review Focus

These are inputs the design implies but does not spell out. Each has a test in the owning task.

1. **An artifact file, or an intermediate capsule directory, replaced by a symlink.** Refuse, and never follow it
   (task 3: `capsule_symlinked_artifact_refuses`, `capsule_symlinked_component_refuses`).
2. **Extra entries in the capsule** (a stray file, or a leftover `.a2a-staging-*`). Refuse rather than ignore
   (task 3: `capsule_extra_entry_refuses`).
3. **An opener whose receipt disagrees with the bytes it actually wrote.** Refuse, because the sink measures the
   bytes independently (task 5: `opener_lying_receipt_refuses`).
4. **An oversize seal file, or an oversize control plaintext.** Refuse at the bound, without allocating to the file's
   size (task 3: `oversize_seal_refuses`; task 5: `oversize_control_plaintext_refuses`).
5. **Destination replaced between preflight and the first staged write.** Refuse, with no write outside the
   original destination (task 4: `destination_swapped_before_first_write_refuses`).

---

### Task 1: the shared test envelope fixture (sealer moved, opener added)

**Files:**
- Create: `crates/bridge-core/src/custody_envelope_fixture.rs` (`#[cfg(test)]`, declared in `lib.rs` as
  `#[cfg(test)] pub(crate) mod custody_envelope_fixture;`).
- Modify: `crates/bridge-core/src/custody_export_tests.rs`. **Move** `FixtureSealerV1`, `SealerFaultV1`, and
  `FIXTURE_ENVELOPE_MAGIC_V1` out, and import them. Their behavior is byte-identical.

**Interfaces:**
- Produces:

```rust
pub(crate) const FIXTURE_ENVELOPE_MAGIC_V1: &[u8; 8] = b"A2AFIX1\n";
pub(crate) struct FixtureSealerV1 { /* unchanged */ }
impl FixtureSealerV1 { pub(crate) fn honest() -> Self; pub(crate) fn faulted(fault: SealerFaultV1, target: Option<&[u8]>) -> Self; }
pub(crate) struct FixtureOpenerV1 { fault: OpenerFaultV1 }
#[derive(Default, Clone, Copy)]
pub(crate) struct OpenerFaultV1 { pub(crate) lie_about_receipt_length: bool, pub(crate) refuse: bool }
impl FixtureOpenerV1 { pub(crate) fn honest() -> Self; pub(crate) fn faulted(fault: OpenerFaultV1) -> Self; }
impl sealed::Sealed for FixtureOpenerV1 {}
impl CustodyEnvelopeOpenerV1 for FixtureOpenerV1 { /* first chunk must equal the magic; every later chunk is written verbatim; the receipt carries the plaintext length and SHA-256 */ }
```

- [ ] **Step 1: write the failing tests.** Add `#[cfg(test)] mod tests` in the new module:
  - `fixture_opener_round_trips_the_fixture_sealer`: seal a 3-chunk plaintext with `FixtureSealerV1::honest()` into
    an in-memory ciphertext sink, then open it with `FixtureOpenerV1::honest()` into an in-memory plaintext sink.
    The bytes, the receipt length, and the SHA-256 are all equal.
  - `fixture_opener_refuses_a_wrong_magic`: the first ciphertext chunk `b"NOTMAGIC"` gives `Err(InvalidInput)`.
  - `fixture_opener_refuses_an_empty_envelope`: no chunks gives `Err`.
- [ ] **Step 2: run and verify RED.** Run `cargo test -p bridge-core --lib custody_envelope_fixture`. Expected: a
  compile error (the module does not exist yet).
- [ ] **Step 3: implement.** Move the sealer verbatim, and implement `FixtureOpenerV1::open`:
  - read `ciphertext.next_chunk()` until `None`;
  - require the first chunk's bytes to equal `FIXTURE_ENVELOPE_MAGIC_V1`;
  - write every later chunk to `plaintext` with ordinals renumbered from 0, and the last-chunk flag set on the final
    one (an empty plaintext is one empty last chunk);
  - hash while writing, and return `CustodyEnvelopeOpenReceiptV1::new(len, sha)`;
  - `refuse` returns `Err(InvalidInput)`, and `lie_about_receipt_length` returns `len + 1`.
- [ ] **Step 4: run and verify GREEN.** Run the new tests plus `cargo test -p bridge-core --lib custody_export`.
  Every 2B2 export test still passes, so the moved sealer's behavior is unchanged.
- [ ] **Step 5: commit.** Commit with the message `test(bridge-core): shared fixture envelope with opener (2B3a task 1)`.

### Task 2: a seal proof from a published seal (`custody_capsule.rs`, additive)

**Files:**
- Modify: `crates/bridge-core/src/custody_capsule.rs`. Add one constructor to `impl CustodyCapsuleSealProofV1`.
- Test: the same file's tests module.

**Interfaces:**
- Produces:

```rust
impl CustodyCapsuleSealProofV1 {
    /// A seal read back from a published capsule. Validated exactly as open requests validate a seal.
    pub fn from_published_seal_v1(seal: CustodySealV1) -> Result<Self, CustodyCapsuleErrorV1> {
        validate_capsule_seal_for_open_request(&seal)?;
        Ok(Self { seal })
    }
}
```

- [ ] **Step 1: failing tests.**
  - `published_seal_round_trips_through_a_seal_proof`: build a proof with `from_receipts` (the existing test helper),
    encode `proof.seal()` canonically, decode it with `CustodySealV1::decode_canonical`, and pass it to
    `from_published_seal_v1`. It returns `Ok`, and `seal()` is equal.
  - `published_seal_refuses_what_open_requests_refuse`: for each seal that
    `preflight_generic_seal_for_capsule_v1` refuses (the existing negative fixtures: a zero-length artifact, a wrong
    format), `from_published_seal_v1` also returns `Err`.
- [ ] **Step 2: RED.** It does not compile, because the function is missing.
- [ ] **Step 3: implement.** Add exactly the constructor above.
- [ ] **Step 4: GREEN.** Run the `custody_capsule` tests.
- [ ] **Step 5: commit.** Commit with the message `feat(bridge-core): seal proof from a published seal (2B3a task 2)`.

### Task 3: pin the capsule, read the seal, and verify every ciphertext (nothing opened)

**Files:**
- Create: `crates/bridge-core/src/custody_restore.rs` and `crates/bridge-core/src/custody_restore_tests.rs`
  (`#[cfg(test)] #[path]`, as `custody_export_tests.rs` is).
- Modify: `crates/bridge-core/src/lib.rs`, adding `#[cfg(unix)] #[allow(dead_code)] mod custody_restore;`.

**Interfaces:**
- Consumes: `CustodyCapsuleSealProofV1::from_published_seal_v1` (task 2), and `PinnedDirectoryV1`
  (`open`, `open_existing_child_directory`, `open_regular_file`, `list_child_names`, `child_metadata_no_follow`).
- Produces:

```rust
pub(crate) const RESTORE_SEAL_NAME_V1: &str = "custody-seal.v1";
pub(crate) const RESTORE_CONTROL_READ_BOUND_V1: u64 = 1024 * 1024;
pub(crate) struct PinnedCapsuleV1 { root: PinnedDirectoryV1, seal: CustodyCapsuleSealProofV1, ciphertexts: Vec<VerifiedCiphertextV1> }
pub(crate) struct VerifiedCiphertextV1 { pub(crate) name: LosslessPathV1, pub(crate) file: File /* retained, no-follow, rewound */, pub(crate) length: u64 }
pub(crate) fn pin_and_verify_capsule_v1(capsule_root: &Path) -> Result<PinnedCapsuleV1, CustodyRestoreErrorV1>;
#[derive(Debug, thiserror::Error)]
pub(crate) enum CustodyRestoreErrorV1 { /* SealUnreadable, SealInvalid, CapsuleEntryUnexpected, CapsuleEntryMissing, CiphertextLength{name}, CiphertextDigest{name}, SymlinkRefused{name}, Io(String), … later tasks add variants */ }
```

**Behavior:**
1. `PinnedDirectoryV1::open(capsule_root)`.
2. Open `custody-seal.v1` with `open_regular_file`. A bounded read (at most `RESTORE_CONTROL_READ_BOUND_V1`, with the
   `+1` probe) refuses oversize input. Then `CustodySealV1::decode_canonical`, then `from_published_seal_v1`.
3. List the capsule exterior. It must equal exactly `{"custody-seal.v1"} ∪ {first components of every seal artifact
   name}`; anything else is `CapsuleEntryUnexpected`. Each artifact is reached component by component through
   `open_existing_child_directory` and then `open_regular_file` at the leaf, with the no-follow opens refusing links.
   Every intermediate directory's listing must equal exactly the expected children.
4. For each artifact, stream-hash the file through its retained descriptor. It must match the seal's `byte_length`
   and ciphertext SHA-256, else `CiphertextLength` or `CiphertextDigest`. Rewind and retain the descriptor.

- **Test fixtures (test-only, visibility changes only in 2B2 code):**
  - In `custody_export.rs`, change `mod tests;` to `pub(crate) mod tests;`.
  - In `custody_export_tests.rs`, make `HarnessV1`, `HarnessV1::new`, `HarnessV1::run_sealed`, `PlanFixtureV1`, and
    the `PlanFixtureV1` methods these tests call `pub(crate)`. Add one accessor,
    `pub(crate) fn capsule_dir(&self) -> PathBuf` (`scratch.join(CAPSULE_DIR_NAME)`), to each fixture.
  - In `custody_restore_tests.rs`, add a fixture reseal helper:

    ```rust
    /// Copies `capsule` to `out` (owner-private, new), replacing the plaintext of each named artifact. The fixture
    /// envelope is `A2AFIX1
` followed by the plaintext, so plaintext = ciphertext[8..]. Every ciphertext is
    /// re-wrapped, and the seal is rebuilt with `CustodySealV1::new` from the original seal's recipients, format,
    /// tool, and version, with fresh `CustodySealedArtifactV1::new(name, len, sha256)` rows. The seal's manifest
    /// digest is `manifest_digest.unwrap_or(original)`, and the seal is written via `encode_canonical`.
    fn reseal_for_test(capsule: &Path, out: &Path, replace: &[(&str, Vec<u8>)], manifest_digest: Option<Sha256HexV1>);
    ```

    The helper is self-tested: `reseal_with_no_replacement_is_byte_identical` means that resealing an exported
    capsule with no replacement reproduces every file byte-for-byte.
- [ ] **Step 1: failing tests.** Fixture: a capsule exported by `HarnessV1::new().run_sealed()`, at `capsule_dir()`.
  - `pins_and_verifies_an_exported_capsule`: `Ok`, the ciphertext count equals the seal's artifact count, and every
    length equals the seal's.
  - `oversize_seal_refuses`: replace the seal with 1 MiB + 1 bytes, giving `SealUnreadable`.
  - `tampered_seal_refuses`: flip one byte of the seal JSON, giving `SealInvalid`.
  - `ciphertext_length_mismatch_refuses`: append one byte to an artifact, giving `CiphertextLength`.
  - `ciphertext_digest_mismatch_refuses`: flip one byte of an artifact at the same length, giving `CiphertextDigest`.
  - `missing_artifact_refuses`: remove one artifact, giving `CapsuleEntryMissing`.
  - `capsule_extra_entry_refuses`: add `stray`, or `.a2a-staging-x`, giving `CapsuleEntryUnexpected` (review focus
    2).
  - `capsule_symlinked_artifact_refuses`: replace an artifact with a symlink to an identical copy, giving
    `SymlinkRefused` (review focus 1).
  - `capsule_symlinked_component_refuses`: replace `payload/` with a symlink to an identical directory, giving
    `SymlinkRefused`.
- [ ] **Step 2: RED.** It does not compile (the module is missing), then tests fail against a stub that returns `Ok`.
- [ ] **Step 3: implement** `pin_and_verify_capsule_v1` exactly as described above.
- [ ] **Step 4: GREEN.** Run `cargo test -p bridge-core --lib custody_restore`.
- [ ] **Step 5: commit.** Commit with the message `feat(bridge-core): capsule pin and ciphertext verification (2B3a task 3)`.

### Task 4: destination preflight, the work tree, and the ledger

**Files:** modify `custody_restore.rs` and `custody_restore_tests.rs`.

**Interfaces:**
- Consumes: `preflight_scratch_root(root, protected: &[&PinnedDirectoryV1]) -> Result<PinnedDirectoryV1,
  CustodyExportErrorV1>` and `ScratchLedgerV1::{new, reserve, reserve_entries}` from `custody_export.rs`. All are
  already `pub(crate)`.
- Produces:

```rust
pub(crate) struct CustodyRestoreBudgetV1 { pub(crate) max_restore_bytes: u64 /* ≤ 10 GiB */ }
pub(crate) struct RestoreDestinationV1 { root: PinnedDirectoryV1, work: PinnedDirectoryV1, plain: PinnedDirectoryV1, ledger: RefCell<ScratchLedgerV1> }
pub(crate) fn prepare_destination_v1(destination_root: &Path, capsule: &PinnedCapsuleV1, budget: CustodyRestoreBudgetV1) -> Result<RestoreDestinationV1, CustodyRestoreErrorV1>;
```

**Behavior:**
- Preflight the destination as 2B2 preflights a scratch root: owner-private and empty.
- The capsule/destination overlap refuses in both directions, by retained identity (`protected = [capsule root
  pin]`). The reverse check, a destination inside the capsule, also goes through the same predicate.
- Create `.restore-work/` and `.restore-work/plain/` create-new, charging the ledger.
- Recheck the destination pin before the first create.
- Map the ledger's and preflight's `CustodyExportErrorV1` into `CustodyRestoreErrorV1::Budget(String)` and
  `DestinationInvalid(String)` respectively. Both constructors are already `pub(crate)`:
  `ScratchLedgerV1::new(limit)`, `reserve`, `reserve_entries`, and `preflight_scratch_root`.

- [ ] **Step 1: failing tests.**
  - `destination_must_be_empty`: a destination containing one file gives `DestinationInvalid`.
  - `destination_inside_capsule_refuses`, and `capsule_inside_destination_refuses`: `DestinationInvalid`, with no
    entry created.
  - `destination_swapped_before_first_write_refuses`: a hook between preflight and the first create renames the
    destination and puts an empty directory in its place. The result is `IdentityChanged`, and nothing is written in
    either directory (review focus 5).
  - `ledger_max_and_max_plus_one`: with `max_restore_bytes` exactly equal to the bytes this task charges, it
    succeeds; with one byte less, it gives `Budget`.
- [ ] **Step 2: RED**, then **Step 3: implement**, then **Step 4: GREEN.**
- [ ] **Step 5: commit.** Commit with the message `feat(bridge-core): restore destination preflight and ledger (2B3a task 4)`.

### Task 5: open the control artifacts and bind through 2B1

**Files:** modify `custody_restore.rs` and `custody_restore_tests.rs`.

**Interfaces:**
- Consumes: `CustodyEnvelopeOpenRequestV1::from_seal_artifact(&seal_proof, name)`, `CustodyEnvelopeOpenerV1`, and
  `CustodyCapsuleBindingV1::new(&manifest, &index, &policy, &seal_proof)`. It also uses the manifest, index, and
  policy `decode_canonical` functions.
- Produces:

```rust
pub(crate) struct StagedPlaintextV1 { pub(crate) name: LosslessPathV1, pub(crate) role: CustodyCapsuleArtifactRoleV1, pub(crate) file: File /* retained, rewound */, pub(crate) length: u64, pub(crate) sha256: [u8; 32] }
pub(crate) struct BoundControlV1 { pub(crate) manifest: CustodyManifestV1, pub(crate) index: CustodyCapsuleIndexV1, pub(crate) policy: CustodyRestorePolicyV1, pub(crate) binding: CustodyCapsuleBindingV1 }
fn stage_one_v1(dest: &RestoreDestinationV1, capsule: &PinnedCapsuleV1, name: &LosslessPathV1, opener: &dyn CustodyEnvelopeOpenerV1) -> Result<StagedPlaintextV1, CustodyRestoreErrorV1>;
fn bind_control_v1(dest: &RestoreDestinationV1, capsule: &PinnedCapsuleV1, opener: &dyn CustodyEnvelopeOpenerV1) -> Result<(BoundControlV1, Vec<StagedPlaintextV1>), CustodyRestoreErrorV1>;
```

**Behavior of `stage_one_v1`:**
- The staged file mirrors the logical name under `plain/`, creating component directories new and charging the
  ledger.
- The ciphertext chunk source reads the retained ciphertext descriptor in `max_chunk_bytes` chunks.
- The plaintext sink writes create-new, reserving each chunk in the ledger before writing it, and **measures its own
  length and SHA-256**. If the opener's receipt disagrees, it refuses with `OpenerReceiptMismatch` (review focus 3).

**Behavior of `bind_control_v1`:** stages exactly the three control names, reads each back through a bounded read
(1 MiB), decodes them, and calls `CustodyCapsuleBindingV1::new`.

- [ ] **Step 1: failing tests.** Build capsules with the fixture sealer.
  - `binds_an_exported_capsule`: `Ok`, with only the 3 control plaintexts staged.
  - `tampered_manifest_refuses_before_payload_staging`: `reseal_for_test` replaces the manifest plaintext with the
    canonical manifest of a second harness export whose generation differs, keeping the original seal manifest
    digest. The ciphertexts are self-consistent, so only the three-way digest disagreement remains, and it gives
    `Binding(..)`. Assert that `plain/payload/` and `plain/git/` do not exist.
  - `index_mapping_mismatch_refuses`: decode the staged index, rebuild it with `CustodyCapsuleIndexV1::new` minus one
    artifact row, `encode_canonical`, and reseal. The result is `Binding(..)`, with no payload staged.
  - `non_inert_policy_refuses`: the policy's `RestoreDisabledV1` fields are closed enums, so a policy JSON with
    `"hooks":"enabled"` gives `ControlDecode`, with no payload staged.
  - `opener_lying_receipt_refuses`: `FixtureOpenerV1::faulted(lie_about_receipt_length)` gives
    `OpenerReceiptMismatch`.
  - `opener_refusal_propagates`: `refuse` gives `Open(..)`.
  - `oversize_control_plaintext_refuses`: a sealed manifest artifact of 1 MiB + 1 plaintext bytes gives
    `ControlOversize`, without reading past the bound.
- [ ] **Step 2: RED**, then **Step 3: implement**, then **Step 4: GREEN.**
- [ ] **Step 5: commit.** Commit with the message `feat(bridge-core): stage and bind capsule control artifacts (2B3a task 5)`.

### Task 6: stage the pack and the payloads, then decode-verify every frame

**Files:** modify `custody_restore.rs` and `custody_restore_tests.rs`.

**Interfaces:**
- Consumes: `stage_one_v1` (task 5), `CustodyFrameDecoderV1::new(source, expected_header, budget)` and `next_entry`
  (2B2b1), and `CustodyFrameHeaderV1::new(class, generation_id)`.
- Produces:

```rust
pub(crate) struct VerifiedCapsuleV1 { pub(crate) destination: RestoreDestinationV1, pub(crate) control: BoundControlV1, pub(crate) staged: Vec<StagedPlaintextV1> /* control + pack + payloads, in index order */ }
pub(crate) fn verify_and_stage_v1(capsule_root: &Path, destination_root: &Path, budget: CustodyRestoreBudgetV1, frame_budget: CustodyFrameBudgetV1, opener: &dyn CustodyEnvelopeOpenerV1) -> Result<VerifiedCapsuleV1, CustodyRestoreErrorV1>;
```

**Behavior:**
1. After `bind_control_v1`, stage every remaining index row in index order.
2. For each `CoveragePayload(class)`, decode the staged plaintext with the expected header `(class,
   manifest.generation_id())` and the frame budget. Drain every entry's content, and require `Ok(None)`, so the
   trailer digest, count, and total are verified. Rewind afterwards.
3. The pack is staged only; 2B3b verifies it with `index-pack`.

- [ ] **Step 1: failing tests.**
  - `verifies_and_stages_a_fixture_stream_capsule`: the 2B2 harness capsule, whose fixture streams are **not**
    frames. This must **refuse** with `FrameInvalid{class}`, which documents that fixture-stream capsules cannot be
    restored.
  - `verifies_and_stages_a_plan_backed_capsule`: a capsule exported through the 2B2b2b2 plan-backed path, with real
    frames, from a rich clone fixture. The result is `Ok`, every staged length equals its receipt, and every frame
    decodes.
  - `corrupt_frame_trailer_refuses`, `wrong_class_frame_refuses`, `wrong_generation_frame_refuses`: use
    `reseal_for_test` on a plan-backed capsule to replace one payload plaintext. The replacement is, in turn:
    - that frame with its last trailer byte flipped;
    - the plaintext of another class's payload from the same capsule;
    - the same class's payload from a second plan-backed export with a different generation.

    Each gives `FrameInvalid{class}`.
  - `nothing_outside_restore_work`: after success, the destination contains exactly `.restore-work/**`.
- [ ] **Step 2: RED**, then **Step 3: implement**, then **Step 4: GREEN.**
- [ ] **Step 5: commit.** Commit with the message `feat(bridge-core): stage payloads and decode-verify frames (2B3a task 6)`.

### Task 7: mutation matrix, gates, and handoff

- [ ] **Step 1: mutation matrix.** Build a persisted, foreground harness under `.git/a2a-bridge/mutation/` with one
  row per guard:
  - the seal bound off;
  - the exterior exact-set check off;
  - no-follow off;
  - the ciphertext length check off;
  - the ciphertext digest check off;
  - the destination-empty check off;
  - each overlap direction off;
  - the pre-write recheck off;
  - the ledger reservation off;
  - receipt versus measurement off;
  - staging the payloads before binding;
  - the frame drain skipped;
  - the class or generation expectation dropped.

  Each row must turn its own control red. Restore byte-exactly, and prove the source equals its snapshot.
- [ ] **Step 2: gates.**
  - `cargo fmt --all -- --check`;
  - `cargo clippy --workspace --all-targets -- -D warnings`;
  - `cargo test --workspace --all-targets --no-fail-fast`;
  - `cargo test --workspace --no-fail-fast`;
  - `cargo run -p a2a-bridge -- validate --repo-hygiene`;
  - `cargo deny check`, or record it as a named exclusion if it is not installed.
- [ ] **Step 3: handoff.** Write `docs/superpowers/reviews/<date>-adr0041-slice2b3a-implementation-handoff.md`. It
  records the RED evidence per task, the matrix table, exact totals, exclusions, and the design §5 refinement stated
  in the Description. It must have no placeholders.
- [ ] **Step 4: stage.** Stage exactly the files below, run `git diff --cached --check`, and write the plain Commit
  Message subject below, with no code fence, to `.git/A2A_COMMIT_MSG`.

## Acceptance Criteria

- Every test named in tasks 1–6 exists, fails on the predecessor or on a stub for the stated reason, and passes.
- Every review-focus line has its named test.
- Nothing is written outside `<dest>/.restore-work/`, and the capsule is never written.
- The export tests pass unchanged after the fixture move.
- The mutation matrix flips every row on the final bytes.
- The workspace gates are green. The controller's macOS lane and CI (including native ext4) are green.

## Files

- `crates/bridge-core/src/custody_envelope_fixture.rs` (new, `#[cfg(test)]`)
- `crates/bridge-core/src/custody_restore.rs` (new) and `crates/bridge-core/src/custody_restore_tests.rs` (new)
- `crates/bridge-core/src/custody_capsule.rs` (additive constructor only)
- `crates/bridge-core/src/custody_export_tests.rs` (fixture moved out; `pub(crate)` visibility on the two harnesses
  plus the `capsule_dir()` accessors, all test-only)
- `crates/bridge-core/src/custody_export.rs` (`pub(crate) mod tests;` only; the ledger and preflight are already
  `pub(crate)`)
- `crates/bridge-core/src/lib.rs` (module declarations)
- `docs/superpowers/reviews/<date>-adr0041-slice2b3a-implementation-handoff.md`

## Stop conditions

Stop and report if any of the following is needed:
- a change to `custody_frame.rs`, `custody_walk.rs`, `custody_coverage.rs`, `custody_git.rs`, `custody_mounts.rs`, or
  `fs_custody.rs`;
- weakening any existing validation;
- following a symlink;
- a path-addressed read;
- any write outside `.restore-work/`;
- any Git command;
- a new dependency.

Also stop on an open-class review population, or a review cap exhausted without convergence.

## Commit Message

feat(bridge-core): ADR-0041 Slice 2B3a capsule reader, verify and stage
