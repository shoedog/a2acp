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

**Revision:** 3 (approved in spec round 2, with its deferrals folded; see the Revision history at the end).

## Description

2B3a is the first of four serial children (2B3a reader → 2B3b Git plane → 2B3c payload plane → 2B3d record and
proofs). It implements design §3.1 **phase V only**. It writes only under `<dest>/.restore-work/`, never under
`repository/` or `evidence/`. It runs no Git command and decodes nothing into the destination tree.

**Design refinement (stated for review):** design §5 says tampering "refuses before any plaintext is staged". The
manifest, index, and restore policy are themselves encrypted artifacts, and the seal has no independently supplied
expected digest in 2B, so a canonical seal that merely disagrees with the controls can only be detected after the
controls are opened. The plan therefore fixes a three-tier timing contract. Each tier has named tests.

| Tamper class | Example | Refuses | Staged when refused |
|---|---|---|---|
| **T1: malformed or out-of-bounds seal** | non-canonical or invalid JSON, oversize, zero-length artifact | task 3, `SealUnreadable` / `SealInvalid` | nothing; no `.restore-work/` exists |
| **T2: seal/ciphertext disagreement** | a ciphertext byte, length, or a canonical edit of a seal artifact digest | task 3, `CiphertextLength` / `CiphertextDigest` | nothing; no `.restore-work/` exists |
| **T3: seal/control semantic disagreement** | a canonical edit of the seal's `manifest_digest`; a swapped manifest; an index mapping or policy change | task 5, `Binding` / `ControlDecode` | only the three control plaintexts; **no pack or payload plaintext** |

The order is:
1. verify every ciphertext against the seal;
2. stage **only the three control artifacts**;
3. bind them through 2B1;
4. only then stage the pack and the coverage payloads.

## Global Constraints

- Rust 1.94.0 is pinned by `rust-toolchain.toml`. No new crate dependency.
- **Where code goes:**
  - `custody_restore.rs` is `#[cfg(unix)]` and crate-private, with `#[allow(dead_code)]` until 2B3b uses it. It is
    declared in `lib.rs` next to `custody_export`.
  - The new constructor in `custody_capsule.rs` is **additive** and **`pub(crate)`**, so the receipt-derived proof
    boundary of the public API is unchanged. No existing validation is weakened.
  - The shared envelope fixture is `#[cfg(test)]` only.
  - No change to `custody_frame.rs`, `custody_walk.rs`, `custody_coverage.rs`, `custody_git.rs`, or `custody_mounts.rs`.
- **Filesystem access:**
  - Every filesystem read goes through retained, no-follow descriptors (`PinnedDirectoryV1`,
    `open_existing_child_directory`, `open_regular_file`).
  - Every write is create-new beneath the retained destination descriptor.
  - Never overwrite, never follow a symlink, never delete, not even on failure.
- **Budget:** one restore-wide `ScratchLedgerV1` reserves every staged byte and entry before writing it.
  - The ceiling is 10 GiB (`RESTORE_BYTES_CEILING_V1 = 10 * 1024 * 1024 * 1024`), the same as 2B2 §3.
  - It is enforced by a validating constructor, not by a comment.
- **Device containment (design §4):**
  - Every directory the restore creates must have the destination root's `dev`.
  - A mount census (`custody_mounts::mount_points_v1` / `mount_point_within`, consumed unchanged) must show no mount
    point strictly within the destination. It runs at preflight and again before `VerifiedCapsuleV1` is returned.
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
6. **A retained ciphertext modified in place after verification.** The bytes the opener consumes are re-verified
   against the seal (task 5: `ciphertext_changed_after_verification_refuses`).
7. **A capsule whose file chunking differs from the sealer's.** The fixture opener parses the magic across arbitrary
   chunk boundaries (task 1: `fixture_opener_accepts_split_magic`, `fixture_opener_accepts_coalesced_magic`).

---

### Task 1: the shared test envelope fixture (sealer moved, opener added)

**Files:**
- Create: `crates/bridge-core/src/custody_envelope_fixture.rs` (`#[cfg(test)]`, declared in `lib.rs` as
  `#[cfg(test)] pub(crate) mod custody_envelope_fixture;`).
- Modify: `crates/bridge-core/src/custody_export_tests.rs`. **Move** `FixtureSealerV1`, `SealerFaultV1`, and
  `FIXTURE_ENVELOPE_MAGIC_V1` out, and import them. Their behavior is byte-identical.
  - `SealerFaultV1`'s two existing fields (`stop_after_chunks`, `swap_receipt_contexts`) become `pub(crate)`, so the
    existing field-literal constructions in export controls 15 and 16 compile unchanged.
  - The existing `envelope_format()` helper moves too, as `pub(crate) fn fixture_envelope_format_v1()`.

**Interfaces:**
- Produces:

```rust
pub(crate) const FIXTURE_ENVELOPE_MAGIC_V1: &[u8; 8] = b"A2AFIX1\n";
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct SealerFaultV1 { pub(crate) stop_after_chunks: Option<usize>, pub(crate) swap_receipt_contexts: bool }
pub(crate) fn fixture_envelope_format_v1() -> CustodyEnvelopeFormatV1; // ("capsule-v1", "a2a-bridge-2b2-fixture", "0.1.0")
pub(crate) struct FixtureSealerV1 { /* unchanged */ }
impl FixtureSealerV1 { pub(crate) fn honest() -> Self; pub(crate) fn faulted(fault: SealerFaultV1, target: Option<&[u8]>) -> Self; }
pub(crate) struct FixtureOpenerV1 { fault: OpenerFaultV1, calls: Cell<usize> }
#[derive(Default, Clone, Copy)]
pub(crate) struct OpenerFaultV1 { pub(crate) lie_about_receipt_length: bool, pub(crate) refuse: bool, pub(crate) stop_after_chunks: Option<usize> }
impl FixtureOpenerV1 { pub(crate) fn honest() -> Self; pub(crate) fn faulted(fault: OpenerFaultV1) -> Self; pub(crate) fn calls(&self) -> usize; }
impl sealed::Sealed for FixtureOpenerV1 {}
impl CustodyEnvelopeOpenerV1 for FixtureOpenerV1 { /* see Step 3 */ }
```

- [ ] **Step 1: write the failing tests.** Add `#[cfg(test)] mod tests` in the new module:
  - `fixture_opener_round_trips_the_fixture_sealer`: seal a 3-chunk plaintext with `FixtureSealerV1::honest()` into
    an in-memory ciphertext sink, then open it with `FixtureOpenerV1::honest()` into an in-memory plaintext sink.
    The bytes, the receipt length, and the SHA-256 are all equal.
  - `fixture_opener_accepts_split_magic`: the same envelope re-chunked as `b"A2A"`, `b"FIX1\n" ‖ body[..3]`, and then
    the rest. The plaintext is byte-equal.
  - `fixture_opener_accepts_coalesced_magic`: the whole envelope as one chunk. The plaintext is byte-equal.
  - `fixture_opener_refuses_a_wrong_magic`: `b"NOTMAGIC"` followed by a body gives `Err(InvalidInput)`, with nothing
    written to the sink.
  - `fixture_opener_refuses_a_short_envelope`: no chunks, or only `b"A2AFIX"`, gives `Err(InvalidInput)`.
  - `fixture_opener_refuses_an_unsupported_format`: an open request whose context format differs from
    `fixture_envelope_format_v1()` gives `Err(InvalidInput)` before any chunk is read.
- [ ] **Step 2: run and verify RED.** Run `cargo test -p bridge-core --lib custody_envelope_fixture`. Expected: a
  compile error (the module does not exist yet).
- [ ] **Step 3: implement.** Move the sealer verbatim, and implement `FixtureOpenerV1::open`:
  - refuse unless `request.context().format() == &fixture_envelope_format_v1()`;
  - read `ciphertext.next_chunk()` until `None`, treating the source as one byte stream: the chunk boundaries are
    transport, not framing;
  - accumulate the first 8 bytes across as many chunks as needed, and require them to equal
    `FIXTURE_ENVELOPE_MAGIC_V1`. Nothing is written to the sink before the magic is proven;
  - forward the remainder of the chunk that completes the magic, and every later chunk, to `plaintext`. Ordinals are
    renumbered from 0, and the last-chunk flag is set on the final one (an empty plaintext is one empty last chunk);
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
    /// A seal read back from a published capsule, for the crate-private restore only. It is validated as
    /// open requests validate a seal, and additionally every artifact as `from_seal_artifact` validates one.
    pub(crate) fn from_published_seal_v1(seal: CustodySealV1) -> Result<Self, CustodyCapsuleErrorV1> {
        validate_capsule_seal_for_open_request(&seal)?;
        for artifact in seal.artifacts() {
            validate_selected_seal_artifact_limits(artifact)?;
            if artifact.byte_length() == 0 {
                return Err(CustodyCapsuleErrorV1::InvalidInput);
            }
        }
        Ok(Self { seal })
    }
}
```

- [ ] **Step 1: failing tests.**
  - `published_seal_round_trips_through_a_seal_proof`: build a proof with `from_receipts` (the existing test helper),
    encode `proof.seal()` canonically, decode it with `CustodySealV1::decode_canonical`, and pass it to
    `from_published_seal_v1`. It returns `Ok`, and `seal()` is equal.
  - `published_seal_refuses_a_zero_length_artifact`: a canonical seal with one zero-length artifact gives `Err`.
    `validate_capsule_seal_for_open_request` alone accepts it, so this test is RED against the one-line body.
  - `published_seal_refuses_an_over_limit_artifact`: an artifact whose length exceeds the selected-artifact limit
    gives `Err`.
  - `from_published_seal_is_crate_private`: a `compile_fail` doctest on `CustodyCapsuleSealProofV1` calls
    `bridge_core::custody_capsule::CustodyCapsuleSealProofV1::from_published_seal_v1(seal)` from outside the crate.
    Making it `pub` turns this control red.
  - `published_seal_refuses_what_open_requests_refuse`: each seal the existing seal-wide negative fixtures refuse
    also gives `Err`.
  - The format check belongs to the opener (task 1), not the seal. The open-request validator accepts any bounded,
    non-empty format.
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
pub(crate) enum CustodyRestoreErrorV1 {
    // task 3
    SealUnreadable(String), SealInvalid(String), CapsuleEntryUnexpected { name: String }, CapsuleEntryMissing { name: String },
    CiphertextLength { name: String }, CiphertextDigest { name: String }, SymlinkRefused { name: String },
    // task 4
    DestinationInvalid(String), IdentityChanged(String), Budget(String), DeviceCrossing { name: String }, MountBoundary(String),
    // task 5
    Open(CustodyCapsuleErrorV1), OpenerReceiptMismatch { name: String }, CiphertextChanged { name: String },
    StagingCollision { name: String }, ControlOversize { name: String }, ControlDecode { name: String }, Binding(CustodyCapsuleErrorV1),
    // task 6
    FrameInvalid { class: CustodyCoverageClassV1 },
    Io(String),
}
```

**Behavior:**
1. `PinnedDirectoryV1::open(capsule_root, "custody restore capsule root")`. Every listing uses
   `list_child_names(&mut remaining, label)`, with one shared entry budget per capsule
   (`remaining = RESTORE_CAPSULE_ENTRY_BUDGET_V1 = 4096`).
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
    /// envelope is `A2AFIX1\n` followed by the plaintext, so plaintext = ciphertext[8..]. Every ciphertext is
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
  - `malformed_seal_refuses` (T1): three variants each give `SealInvalid`, and no `.restore-work/` exists:
    - a seal that is not valid JSON;
    - valid JSON that is not canonical (an added space);
    - a canonical seal with a zero-length artifact.
  - `seal_artifact_digest_edit_refuses_before_staging` (T2): a canonical re-encoding of the seal with one
    artifact's `sha256` changed gives `CiphertextDigest`, and no `.restore-work/` exists.
  - `seal_manifest_digest_edit_passes_task_3` (T3 boundary): a canonical re-encoding with one hex character of
    `manifest_digest` changed passes `pin_and_verify_capsule_v1`. Task 5 owns its refusal.
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
pub(crate) const RESTORE_BYTES_CEILING_V1: u64 = 10 * 1024 * 1024 * 1024;
pub(crate) struct CustodyRestoreBudgetV1 { max_restore_bytes: u64 }
impl CustodyRestoreBudgetV1 {
    /// Refuses 0 and anything above RESTORE_BYTES_CEILING_V1.
    pub(crate) fn new(max_restore_bytes: u64) -> Result<Self, CustodyRestoreErrorV1>;
    pub(crate) const fn ceiling() -> Self; // exactly RESTORE_BYTES_CEILING_V1
    pub(crate) const fn max_restore_bytes(&self) -> u64;
}
pub(crate) struct RestoreDestinationV1 { root: PinnedDirectoryV1, work: PinnedDirectoryV1, plain: PinnedDirectoryV1, ledger: RefCell<ScratchLedgerV1> }
pub(crate) fn prepare_destination_v1(destination_root: &Path, capsule: &PinnedCapsuleV1, budget: CustodyRestoreBudgetV1) -> Result<RestoreDestinationV1, CustodyRestoreErrorV1>;
```

**Behavior:**
- Preflight the destination as 2B2 preflights a scratch root: owner-private and empty.
- The capsule/destination overlap refuses in both directions, by retained identity (`protected = [capsule root
  pin]`). The reverse check, a destination inside the capsule, also goes through the same predicate.
- Create `.restore-work/` and `.restore-work/plain/` create-new, charging the ledger.
- Recheck the destination pin before the first create.
- **Device containment:**
  - Record the destination root's `dev` at preflight.
  - After each create-new directory, compare the created directory's `dev`, from its retained descriptor, with the
    root's. A mismatch is `DeviceCrossing{name}`.
  - Take the mount census over the destination's canonical path at preflight. Any mount point
    `mount_point_within(mount_point, dest)` gives `MountBoundary`.
  - A census error (unsupported, oversize, malformed, or empty) also refuses, as `MountBoundary`.
  - `RestoreDestinationV1::recheck_containment(&self)` re-runs the census and is called again by task 6 before
    returning.
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
  - `budget_ceiling_and_ceiling_plus_one`: `CustodyRestoreBudgetV1::new(RESTORE_BYTES_CEILING_V1)` is `Ok`;
    `new(RESTORE_BYTES_CEILING_V1 + 1)` and `new(0)` give `Budget`.
  - `mount_inside_destination_refuses`: `custody_mounts::seam::install_list` with a mount point one level below the
    destination's canonical path gives `MountBoundary`, and no `.restore-work/` exists. A mount point equal to the
    destination, or above it, is accepted.
  - `census_failure_refuses`: an installed census that returns `Err(CustodyMountErrorV1::MalformedLine { line: 1 })` gives `MountBoundary`.
  - `created_directory_on_another_device_refuses`: a test seam in `custody_restore.rs`,
    `override_created_dev_for_test`, reports a different `dev` for the created `.restore-work/`. The result is
    `DeviceCrossing{".restore-work"}`.
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
- **Verified ciphertext source (the consumed bytes are bound to the seal):**
  - The ciphertext chunk source, `VerifiedCiphertextSourceV1`, reads the retained ciphertext descriptor from offset 0
    in `max_chunk_bytes` chunks.
  - It feeds every chunk to a `CustodyEnvelopeSourceValidatorV1`, built from
    `CustodyEnvelopeSourceDescriptorV1::new(request.ciphertext_length(), limits)`.
  - After the opener returns, the stager calls the validator's `finish()`, which requires full consumption to the
    declared total. It then requires `receipt.total_bytes() == request.ciphertext_length()` and
    `receipt.sha256() == request.ciphertext_sha256()`.
  - A mismatch, or an opener that stops early, gives `CiphertextChanged{name}`. The staged file stays, and nothing is
    deleted.
  - This is the exporter's `ExactTotalPlaintextSourceV1` pattern (`custody_export.rs`) applied to the ciphertext.
- The plaintext sink writes create-new, reserving each chunk in the ledger before writing it, and **measures its own
  length and SHA-256**. If the opener's receipt disagrees, it refuses with `OpenerReceiptMismatch` (review focus 3).

**Behavior of `bind_control_v1`:** stages exactly the three control names, reads each back through a bounded read
(1 MiB), decodes them, and calls `CustodyCapsuleBindingV1::new`.

**The bounded reader** is one function over any reader, used for both the seal (task 3) and the controls:
`fn read_bounded_v1(reader: &mut dyn Read, bound: u64) -> Result<Vec<u8>, BoundedReadErrorV1>`.
- It never requests more than `bound + 1` bytes in total.
- It allocates at most `bound + 1`.
- Reading `bound + 1` bytes is `Oversize`.

**Create-new staging:**
- Every staged directory and file is created with `create_new_child_directory` / `create_new_regular_child`,
  which refuse an existing entry or a planted symlink.
- A collision is `StagingCollision{name}`, and the pre-existing object is left untouched.
- **Shared staging directories** (for example `plain/control/`, used by all three controls):
  - `RestoreDestinationV1` retains the pin of every staging directory it created, keyed by relative path.
  - A later artifact reuses only that retained pin, and never re-opens the directory by name.
  - Before each child create, it rechecks the pin's identity and `dev` against the parent entry.
  - A pre-existing, planted, or replaced intermediate directory is `StagingCollision{name}` or `IdentityChanged`.

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
    `ControlOversize`.
  - `bounded_reader_never_requests_past_the_probe`: a counting reader over 4 MiB with `bound = 1 MiB` gives
    `Oversize`, with at most `bound + 1` bytes requested. A reader that fails any read past `bound + 1` also gives
    `Oversize`, not an I/O error. A `read_to_end` implementation turns this red.
  - `seal_manifest_digest_edit_refuses_at_binding` (T3): the task 3 canonical `manifest_digest` edit gives
    `Binding(..)`. Only the three control plaintexts exist under `plain/`; `plain/payload/` and `plain/git/` do not.
  - `ciphertext_changed_after_verification_refuses` (review focus 6): a test seam
    `after_ciphertext_verification_for_test` rewrites one control ciphertext in place, through the same inode, with a
    same-length, internally valid fixture envelope of different plaintext. The result is `CiphertextChanged{name}`.
  - `opener_stopping_early_refuses`: an opener fault `stop_after_chunks: Some(1)`, added to `OpenerFaultV1`, gives
    `CiphertextChanged{name}`.
  - `shared_staging_directory_custody`: three variants, each giving `StagingCollision` or `IdentityChanged` with no
    leaf created beneath the planted object:
    - plant a real `plain/control/` directory before the first control is staged;
    - plant it as a symlink to an outside directory;
    - after the first control is staged, rename the created `plain/control/` away and put a fresh directory in its
      place.
  - `staging_collision_refuses`: a hook plants a regular file at the first staged leaf, then separately a symlink to
    a file outside the destination. Each gives `StagingCollision`, the planted object's bytes and target are
    unchanged, and the outside file is unchanged.
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
0. The order is: `pin_and_verify_capsule_v1` (task 3), then `prepare_destination_v1` (task 4), then
   `bind_control_v1` (task 5). A T1 or T2 refusal therefore happens before `.restore-work/` exists.
1. After `bind_control_v1`, stage every remaining index row in index order.
2. For each `CoveragePayload(class)`, decode the staged plaintext with the expected header `(class,
   manifest.generation_id())` and the frame budget. Drain every entry's content, and require `Ok(None)`, so the
   trailer digest, count, and total are verified. Rewind afterwards.
3. The pack is staged only; 2B3b verifies it with `index-pack`.
4. Call `destination.recheck_containment()` before returning.
5. `staged` is in exactly `control.index`'s artifact order, the index's canonical sorted order, including the three
   controls. Consumers select descriptors by name, and the order is a checked invariant.

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
  - `staged_order_equals_index_order`: the staged names equal the index's artifact names, in order.
  - `every_ciphertext_verified_before_the_first_open`: in a plan-backed capsule, corrupt the lexically **last**
    artifact's ciphertext. A counting opener (`FixtureOpenerV1::calls()`) records 0 calls, the result is
    `CiphertextDigest`, and no `.restore-work/` exists.
  - `capsule_is_never_written`: snapshot every capsule file (bytes, mode, and mtime) and its directory listings
    before `verify_and_stage_v1`, on success and on each of three failure paths (T1, T2, T3). They are equal after.
  - `mount_appearing_during_restore_refuses`: an installed census whose second call lists a mount point inside the
    destination gives `MountBoundary` from the final recheck.
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
  - the class or generation expectation dropped;
  - the fixture opener requiring the magic in the first chunk (the revision 1 bug);
  - the per-artifact zero-length check in `from_published_seal_v1` off;
  - the consumed-ciphertext validator off;
  - the device comparison off;
  - the preflight census off, and the final census recheck off;
  - the budget ceiling check off;
  - create-new replaced by create-or-truncate;
  - the bounded reader replaced by `read_to_end`;
  - the staged-order sort off;
  - `from_published_seal_v1` visibility changed to `pub` (the compile-fail doctest);
  - a shared staging directory re-opened by name instead of by its retained pin.

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

## Revision history

The reviewed SHAs `e230efd3` (revision 1) and `173eb88e` (revision 2) were rebased onto `main` after #123 merged, as `24d2bda1` and `6c155852`. Their document bytes are unchanged.

**Revision 1** (`e230efd3`), spec review round 1: REJECT, with 8 WRONG MATERIAL, 1 WRONG IMMATERIAL, and 4 SMELL
findings. All are folded in revision 2:

| # | Finding | Fold |
|---|---|---|
| W1 | Public constructor weakens the receipt-derived proof boundary | `pub(crate)` (Global Constraints, task 2) |
| W2 | One-line constructor accepts a zero-length artifact | Per-artifact limit and nonzero checks, a RED test for each; the format check moved to the opener |
| W3 | "Before any plaintext" is false for canonical seal/control disagreement | The T1/T2/T3 timing contract, a named test per tier |
| W4 | Opener treats a chunk boundary as envelope framing | Stream parse of the magic; split and coalesced tests |
| W5 | Consumed ciphertext not bound to the seal | `VerifiedCiphertextSourceV1` with the envelope source validator; in-place rewrite and early-stop tests |
| W6 | Destination device-crossing invariant omitted | Per-directory `dev` check plus the census at preflight and before return; seam tests |
| W7 | 10 GiB ceiling only a comment | Validating `CustodyRestoreBudgetV1::new`; ceiling and ceiling+1 tests |
| W8 | Moved `SealerFaultV1` fields private | `pub(crate)` fields; the export controls compile unchanged |
| W9 | `PinnedDirectoryV1::open` / `list_child_names` signatures | Corrected, with a label and a shared entry budget |
| S1 | No proof all ciphertexts are verified before the first open | `every_ciphertext_verified_before_the_first_open` with a counting opener |
| S2 | Bounded-read tests don't prove bounded reading | `read_bounded_v1` over `Read`, with a counting and failing reader test |
| S3 | Create-new and capsule read-only claims lack controls | Collision and planted-symlink tests; capsule snapshot on success and failure; matrix row |
| S4 | Staged index order not asserted | `staged_order_equals_index_order` |

**Revision 2** (`173eb88e`), spec review round 2 (final admitted round): **APPROVE**, with W1–W9 and S1, S2, S4
RESOLVED and 0 blockers. Its three DEFER items are folded into revision 3 as text-only fixes, and no further review
round was run:
- the census error variant's name (`MalformedLine`);
- shared staging-directory custody with retained pins, and its test (round-1 S3, which round 2 marked UNRESOLVED);
- a compile-fail doctest plus a mutation row for the constructor's visibility.

## Commit Message

feat(bridge-core): ADR-0041 Slice 2B3a capsule reader, verify and stage
