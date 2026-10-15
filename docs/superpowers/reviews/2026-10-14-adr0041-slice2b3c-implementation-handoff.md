# ADR-0041 Slice 2B3c implementation handoff — increment C1 (`fs_custody` primitives and unsafe inventory)

**Written:** 2026-10-14 · **By:** Opus 5.5 (`claude-opus-5-5`) through `a2a-bridge implement` · **Base:**
`95fb79dc` (spec revision 4) · **Scope:** C1 only; C2a, C2b and C3 are not started.
**Source snapshot:** `9ed3f3f8e58dbbb1` (the first 16 hex digits of the SHA-256 of the four changed `.rs` files
concatenated in the §7 order; per-file hashes are in the receipts).
**Receipts:** `.git/a2a-evidence/2b3c-c1/` (listed in §3.4).

## 1. Context

Increment C1 of the 2B3c payload plane (`docs/superpowers/plans/2026-10-14-adr0041-slice2b3c-payload-plane-task.md`,
revision 4, "Primitives (increment C1)" and §C1; review ledger
`2026-10-14-adr0041-slice2b3c-spec-review.md`). It adds the three file-system primitives that C2b and C3 will call.
It also updates both existing unsafe-inventory controls and adds a 2B3c control. The primitives are crate-private and
production-unwired: only tests call them.

Lane: the bridge's Linux container (OrbStack kernel `7.0.14`, `/tmp` on overlayfs, the clone on virtiofs), running as
root. Every new control was also run as an unprivileged user (`setpriv --reuid=65534 --regid=65534 --clear-groups`,
`TMPDIR` in a world-writable scratch directory). **The native non-root macOS run was not performed here**; the spec
gives that run to the controller (see §6).

## 2. What changed

### 2.1 `crates/bridge-core/src/fs_custody.rs`

There is one new section directly after the 2B2b2a block that holds `read_child_symlink`, plus the test-module mount at
the end of the file.

| Item | Contract |
|---|---|
| `PinnedDirectoryV1::create_new_symlink_child(name, target, label) -> Result<ChildStatV1, SymlinkChildRefusalV1>` | Validates the name (`child_name_cstring`). Refuses an empty or NUL-holding target as `Unsupported`, before any syscall. Runs `symlinkat` relative to the retained descriptor. `EEXIST` → `Custody(TargetExists)`. `ENAMETOOLONG` → `DestinationUnsupported { label, target_len, source }`. Any other errno → `Custody(Io)`. Then a no-follow `fstatat` (`stat_child_no_follow`) must show a symlink owned by the effective uid, on the pin's recorded device, whose size equals `target.len()`. Next, `readlinkat` (`read_child_symlink` with `max_bytes = target.len()`) must equal `target`; a longer target counts as a different link and is never truncated into a match. Last, a closing no-follow `fstatat` must equal the first. Every mismatch → `Custody(IdentityChanged)`. Returns the verified observation (kind, mode, size, `dev`, `ino`, mtime, ctime) so C2b can record the creation identity that C3 requires. Nothing follows the link. The parent is not synced, as with `create_new_regular_child`. |
| `SymlinkChildRefusalV1` (new, `pub(crate)`) | `DestinationUnsupported { label, target_len, source: io::Error }` or `Custody(FsCustodyError)`. |
| `created_symlink_is_valid(observed, owner, effective_uid, parent_dev, target_len)` (private) | The pure kind/owner/device/size predicate. Like 2B2a's `created_child_owner_and_mode_is_valid`, it is pure so that the owner and device arms can be tested with injected values. |
| `set_exact_file_mode(&File, mode, label)` (`pub(crate)`) | Refuses `mode & !0o777 != 0` as `Unsupported` before any change. Calls `File::set_permissions`, which is `fchmod` on the descriptor (no `unsafe`, no path, and a mode change is not subject to the umask). Then confirms with `fstat` of the same descriptor; any other observed `mode & 0o7777` → `IdentityChanged`. |
| `PinnedDirectoryV1::set_exact_mode(mode, label)` (`pub(crate)`) | `set_exact_file_mode(&self.file, …)` through the retained directory descriptor. |
| Test seam (`#[cfg(all(test, unix))]`, private) | A thread-local `PayloadPrimitivePointV1` hook with three points: `SymlinkCreated(path)` (after `symlinkat`, before the verifying `fstatat`), `TargetRead(path)` (after `readlinkat`, before the closing `fstatat`) and `ModeSet(&File)` (after `fchmod`, before the confirming `fstat`). Installed with a drop guard that restores the previous hook, including on panic. |
| Mount | `#[cfg(all(test, unix))] #[path = "fs_custody_payload_tests.rs"] mod payload_primitive_tests;` |

The only new `unsafe` block is in `create_new_symlink_child`. It calls `geteuid` and then `symlinkat` (see §5.1).
Each production item carries `#[allow(dead_code)]` with its landing increment, following the 2B2a convention.

### 2.2 Inventory controls

- `custody_git_tests.rs::a14_ast_inventory_keeps_new_unsafe_boundaries_exact`: the `owned` map gains
  `("create_new_symlink_child", 1)`.
- `custody_walk_tests.rs::unsafe_inventory_pins_the_new_2b2b2a_sites_and_their_files`: removes exactly
  `create_new_symlink_child → 1` before comparing the unchanged predecessor map.
- New `fs_custody_payload_tests.rs::unsafe_inventory_pins_the_2b3c_symlink_site`, written in the
  `custody_mounts_tests.rs` style. It pins `create_new_symlink_child → 1`. It pins no site in `set_exact_file_mode`,
  `set_exact_mode`, the predicate or the seam functions. It pins the `fs_custody.rs` total at `35 + 3 + 5 + 1 = 44`, so
  a second site anywhere in that file turns this control red too. It also pins the control file itself as holding no
  `unsafe`.

### 2.3 `crates/bridge-core/src/fs_custody_payload_tests.rs` (new; 14 tests)

| Test | Proves |
|---|---|
| `symlink_01_absolute_dotdot_and_non_utf8_targets_round_trip_byte_exact` | An absolute, a `..` and a non-UTF-8 target are each written and read back byte-exact, through the descriptor and through an independent path `read_link`. The returned observation equals the entry now at the name. |
| `symlink_02_a_long_target_follows_the_destinations_runtime_capability` | For 4095 and 4096 bytes, the capability is probed at run time with std's path `symlink` on the same file system. Where the probe holds the target, the primitive holds it byte-exact. Where the probe gives `ENAMETOOLONG`, the primitive refuses `DestinationUnsupported` (with `target_len` and the errno) and leaves no entry. The test asserts that the refusal arm ran at least once (4096 exceeds the kernel path limit on Linux and macOS). Prints `PAYLOAD-SYMLINK-CAPABILITY` lines. |
| `symlink_03_an_existing_name_refuses_typed_and_is_left_untouched` | A regular file, a directory with a child, a dangling symlink and a live symlink each refuse `TargetExists`. Every occupant's observation, bytes and target are unchanged, and the dangling link's target is not created. |
| `symlink_04_a_replaced_parent_refuses_and_a_renamed_one_never_reaches_the_decoy` | A pinned directory removed and replaced at its name refuses (`Io`), and the replacement stays empty. A pinned directory renamed away stays descriptor-relative: the link lands in the retained directory, the decoy stays empty, and `pinned_root_unchanged` reports the re-pointed name (see §5.2). |
| `symlink_05_the_link_is_never_followed` | Relative and absolute links to an existing `0640` file leave its full observation and bytes unchanged. |
| `symlink_06_an_entry_substituted_after_symlinkat_refuses` | At `SymlinkCreated`, three substitutes each refuse `IdentityChanged`. A regular file of the target's length isolates the kind arm. A same-length symlink to another target isolates the byte comparison. An absent entry refuses at the stat. |
| `symlink_07_an_entry_substituted_after_readlinkat_refuses` | At `TargetRead`, a same-target symlink is renamed over the entry. It is created before the original is released, so its inode number cannot be a reuse. The closing `fstatat` refuses `IdentityChanged`. |
| `symlink_08_each_arm_of_the_verifying_predicate_refuses_an_injected_observation` | The owner (another uid, root), device (other, none), size (±1) and kind (regular, directory, special) arms. |
| `symlink_09_an_unexpressible_target_or_an_unsafe_name_refuses_before_any_entry` | Empty and NUL-holding targets refuse `Unsupported`; `""`, `.`, `..`, `a/b` and `a\0b` names refuse `InvalidChildName`. The directory stays empty. |
| `mode_01_exact_modes_hold_under_any_umask_in_an_isolated_child` | For umasks `077` and `000`, `/bin/sh` installs the umask and `exec`s this test binary filtered to this one test. The child proves its umask with a `0666` file probe and a `0777` directory probe. It then sets `0`, `0200`, `0400`, `0555`, `0755` and `0777` on a created file and a created directory pin, and checks each by `fstat` and by the parent's no-follow stat. The child prints a marker the parent requires. The test process's own umask is never touched. |
| `mode_02_a_renamed_away_object_gets_the_mode_and_the_decoy_is_untouched` | A directory pin and an open file are each renamed away and replaced by a decoy. The retained inode gets the mode; the decoy's full observation is unchanged. |
| `mode_03_a_mode_changed_before_the_confirming_fstat_refuses` | At `ModeSet`, the mode is changed through the same descriptor, and the file and directory both refuse `IdentityChanged`. |
| `mode_04_a_mode_beyond_the_permission_bits_refuses_before_any_change` | `01777`, `02755`, `04755`, `07777` and `010644` refuse `Unsupported` on a file and a directory, and both modes are unchanged. |
| `unsafe_inventory_pins_the_2b3c_symlink_site` | §2.2. |

## 3. Verification

All runs were in the foreground with `CARGO_INCREMENTAL=0` and the proxies unset. The gates were re-run on the final
bytes (§3.3).

### 3.1 Targeted controls (16 = 14 new + A14 + 2B2b2a)

- As root: `16 passed; 0 failed; 0 ignored; 1471 filtered out`.
- As `nobody` (uid 65534): `16 passed; 0 failed; 0 ignored; 1471 filtered out`. The scratch `TMPDIR` was empty
  afterwards, so non-root cleanup works (directories are made searchable again before drop).
- Capability on this lane: `target_len=4095 holds=true`, `target_len=4096 holds=false`. Both umask children printed
  `PAYLOAD-UMASK-CHILD-OK` (`077` and `000`).

### 3.2 Required local gates

| Gate | Result |
|---|---|
| `cargo fmt --all -- --check` | clean |
| `cargo clippy --workspace --all-targets --all-features -- -D warnings` | clean (0 warnings). The first run flagged two lints: `enum_variant_names` on the seam (the hook variants were renamed) and `type_complexity` in `symlink_06` (a local type alias was added). |
| `cargo test -p bridge-core` | lib unittests **1487 passed, 0 failed, 0 ignored** (the 14 new tests plus a derived base of 1473). `compile_fail` (trybuild) 1 passed. 13 integration binaries, 128 passed. Doc-tests 10 passed. **Total 1626 passed, 0 failed, 0 ignored; nothing excluded.** |
| `cargo run -p a2a-bridge -- validate --repo-hygiene` | `repository hygiene validated` (41 tracked artifacts, 9 example configs) |
| `git diff --check` | clean |

The base total of 1473 lib tests is derived as 1487 − 14; no failures occurred, so no base-control run was needed. No
failure outside scope was seen. The full workspace suite and the macOS native run are CI's and the controller's.

### 3.3 Final-bytes run

The gates in §3.2 were re-run after the mutation matrix and after this handoff was written, on snapshot
`9ed3f3f8e58dbbb1`; receipts `08-*`. The source hashes matched (`sha256sum -c`). fmt, clippy (0 warnings), repo
hygiene and `git diff --check` were clean, and `cargo test -p bridge-core` again gave lib 1487, trybuild 1,
integration 128 and doc-tests 10: **1626 passed, 0 failed, 0 ignored**.

Both logs also contain one extra `1 passed; … 1486 filtered out` line. It comes from the pre-existing
`fs_custody::tests::path_identity_refuses_an_unreadable_ancestor`, whose nested child process inherits stdout. It is
not a test binary and is excluded from the totals.

### 3.4 Receipts (`.git/a2a-evidence/2b3c-c1/`)

| File | Content |
|---|---|
| `base-commit.txt` | `95fb79dc…` |
| `01-red-missing-symbol.log` | Structural RED (§4.1). |
| `02-green-targeted-root-linux.log`, `03-green-targeted-nonroot-linux.log` | §3.1. |
| `04-clippy-workspace.log`, `05-test-bridge-core-root-linux.log`, `05-totals.txt`, `06-repo-hygiene.log` | §3.2. |
| `07-mutation-matrix-{a,b,c}.json`, `07-run_mutations.py` | §4.2: the script and its machine-readable output. |
| `08-final-*` | §3.3. |
| `source-sha256.txt` | Per-file SHA-256 of the four changed `.rs` files. |

## 4. RED evidence

### 4.1 Structural (missing-symbol) RED

The tests and both inventory updates were written before any production code. `cargo test -p bridge-core --lib
--no-run` then failed with 35 errors, all missing symbols: `create_new_symlink_child`, `set_exact_mode`,
`set_exact_file_mode`, `SymlinkChildRefusalV1`, `created_symlink_is_valid`, `PayloadPrimitivePointV1` and
`install_payload_primitive_hook_for_test`.

### 4.2 Behavioral RED: the single-guard mutation matrix

`07-run_mutations.py` applies each row as an exact, unique string replacement in one file. For each row it rebuilds,
runs the 16 targeted controls as root and as `nobody`, records the failing tests, and restores the file; it then
asserts the tree is unchanged. All 18 rows flipped the same controls as root and as `nobody`.

| Row | Guard removed (mutation) | Controls turned red |
|---|---|---|
| M01 | `EEXIST` → `TargetExists` mapping (reported as `Io`) | `symlink_03` |
| M02 | `ENAMETOOLONG` → `DestinationUnsupported` mapping | `symlink_02` |
| M03 | empty-target refusal | `symlink_09` |
| M04 | predicate kind arm | `symlink_06`, `symlink_08` |
| M05 | predicate owner arm | `symlink_08` |
| M06 | predicate device arm | `symlink_08` |
| M07 | predicate size arm | `symlink_08` |
| M08 | `readlinkat` byte comparison | `symlink_06` |
| M09 | closing no-follow `fstatat` comparison | `symlink_07` |
| M10 | verifying stat follows the link (path `metadata`) | `symlink_01`, `_02`, `_04`, `_05`, `_06`, `_07` |
| M11 | `symlinkat` replaced by path `symlink` at the pin's canonical path | `symlink_04` |
| M12 | the `fchmod` | `mode_01`, `mode_02`, `mode_04` |
| M13 | the confirming `fstat` comparison | `mode_03` |
| M14 | the permission-bits-only range guard | `mode_04` |
| M15 | `set_exact_mode` chmods the canonical path instead of the descriptor | `mode_02`, `mode_03`, `mode_04` |
| M16 | a second `unsafe { libc::getpid(); }` in `create_new_symlink_child` | A14, 2B2b2a, 2B3c inventory controls |
| M17 | an `unsafe` site in `set_exact_file_mode` | A14, 2B2b2a, 2B3c inventory controls |
| M18 | an `unsafe` site in `fs_custody_payload_tests.rs` | 2B3c inventory control |

Rows M16–M18 are the recorded mutation check that "the inventory controls fail if a second unsafe site is added".

Coverage notes:
- The NUL half of `symlink_09` rests on `CString::new`, which cannot be removed independently of building the C
  string.
- The name half of `symlink_09` rests on the pre-existing `child_name_cstring`, which is not a new guard.

## 5. Open items: interpretations for the reviewer and controller

1. **`geteuid` shares the one new `unsafe` block.** The spec requires both an effective-user owner check and exactly
   one new `unsafe` site. `std` has no safe `geteuid`. The crate's existing wrappers are private to `custody_git.rs` or
   inline in other functions, and changing them would touch files outside this increment or move existing pinned
   sites. So one block calls `geteuid` first (no preconditions, no errno) and then `symlinkat`, and the `SAFETY`
   comment covers both. The inventories count one site. If the controller prefers two separately pinned blocks
   (`create_new_symlink_child → 2`), the change is mechanical.
2. **"Refuse when the parent pin's directory was replaced."** Every create-new primitive is descriptor-relative (2B2a's
   A1 control). So the primitive refuses when the pinned directory itself is gone, meaning removed and replaced at its
   name: the create fails and the replacement receives nothing. A pin whose directory was only renamed away still
   names a live directory. The link lands there, never in the decoy, which matches the `set_exact_mode` decoy
   semantics in the same spec. Refusing the re-pointed name belongs to the caller's chain gate (phase G's `recheck`
   pattern; C2b's per-step gates). A path-based parent check inside the primitive was rejected for two reasons: it
   would reintroduce path resolution, and it would fail on deep trees beyond `PATH_MAX` (1024 bytes on macOS).
   `symlink_04` pins both halves.
3. **Typed refusal vocabulary.** `FsCustodyError` is a `pub` enum in the `pub` module `fs_custody`, and it is not
   `#[non_exhaustive]`. Adding `DestinationUnsupported` to it would change the public API, so the refusal lives in the
   new crate-private `SymlinkChildRefusalV1`. A collision reuses `TargetExists`, matching `mkdirat` and
   `create_new_regular_child_at`. A verification mismatch, including a device mismatch, is `IdentityChanged`. If C2b
   needs a typed device crossing for symlinks, it can add a variant. Only `ENAMETOOLONG` (from the target or the name)
   maps to `DestinationUnsupported`. Other errnos with which a file system might decline a symlink (for example
   `EPERM`/`EOPNOTSUPP` on a file system without symlinks) stay `Io` refusals, which is still fail-closed.
4. **Signatures.** Every primitive takes a trailing `label: &str` (the module convention).
   `create_new_symlink_child` returns the verified `ChildStatV1`, the creation identity that C3's terminal gate
   re-requires.
5. **Two fail-closed refusals beyond the spec's text.**
   - `set_exact_file_mode` refuses any bit outside `0o777`. The frame decoder already limits modes to `0o777` (D4), so
     a valid frame never reaches it, and a restore can never set setuid, setgid or sticky.
   - `create_new_symlink_child` refuses an empty or NUL-holding target before `symlinkat`. The frame already forbids
     both, and an empty target behaves differently across kernels.
6. **The mount is `#[cfg(all(test, unix))]`, not `#[cfg(test)]`.** `fs_custody` also compiles on non-unix (Windows CI
   builds `bridge-store`, which depends on `bridge-core`), and the primitives are unix-only.
7. **A confirmation mismatch is `IdentityChanged`.** The observed and requested modes are in its label. The error type
   for the mode primitives is unchanged (`FsCustodyError`).

## 6. Risks

- **The macOS native non-root run has not happened.** These macOS facts are assumed, and the native run will confirm
  or refute them:
  - creating an entry beneath a removed directory's retained descriptor fails (expected `ENOENT`) in `symlink_04`;
  - APFS stores a non-UTF-8 symlink target byte-exact in `symlink_01`;
  - a 4095-byte target gives `ENAMETOOLONG`; the test probes this rather than assuming it, and its refusal branch is
    already exercised here by the 4096-byte case;
  - `/bin/sh -c 'umask …; exec …'` launches the test binary. SIP strips `DYLD_*` through `/bin/sh`, which matters only
    if the test binary needs a non-system dylib.

  If any of these fails, the right response is to report it, not to weaken the test.
- **No descriptor pins the link.** A symlink has no descriptor, so creation is verified by name and bracketed. A
  replacement after the closing `fstatat` is outside the primitive. It is the gates' concern, and C3 re-requires the
  returned identity.
- **Two shared assumptions.** The primitive relies on a symlink's `st_size` being its target length, an assumption the
  2B2b2a walker already makes. It also relies on uniform `st_dev` between a directory and its entries, as phase G's
  device check does. A file system that breaks either (for example, overlayfs across different file systems without
  `xino`) refuses as `IdentityChanged`, which is fail-closed.
- **The owner check is exact.** A volume that maps or ignores ownership (some FUSE, network or "ignore ownership"
  volumes) refuses every symlink. This is fail-closed and the spec intends it.

## 7. Files

Changed and staged, and nothing else:
- `crates/bridge-core/src/fs_custody.rs` — the new section and the test mount (§2.1)
- `crates/bridge-core/src/fs_custody_payload_tests.rs` — new (§2.3)
- `crates/bridge-core/src/custody_git_tests.rs` — A14 inventory, one row
- `crates/bridge-core/src/custody_walk_tests.rs` — 2B2b2a inventory, one removal
- `docs/superpowers/reviews/2026-10-14-adr0041-slice2b3c-implementation-handoff.md` — this handoff

There are no dependency, `Cargo.lock`, public-API, wire or runner changes. The commit message is in
`.git/A2A_COMMIT_MSG`; the bridge commits. There is no push or merge.

## 8. Next

1. The controller runs `cargo test -p bridge-core` natively on macOS as a non-root user, on this candidate, before the
   C1 source review (§6 lists what that run decides).
2. C1 source review: Sol/xhigh, cap two rounds. §5 lists the interpretations to rule on, especially §5.1
   (shared `unsafe` block) and §5.2 (parent-replacement semantics).
3. On approval, C2a is dispatched on the approved commit: admission, prevalidation and plan, with no writes. C2b
   calls `create_new_symlink_child` (mapping `TargetExists` to `DestinationCollision` and keeping the returned identity)
   and maps `DestinationUnsupported` into `PayloadPlaneRestoreFailureV1`. C3's terminal gate calls
   `set_exact_file_mode` and `PinnedDirectoryV1::set_exact_mode`.

---

# Increment C2a — phase-P admission, prevalidation and plan (no writes)

**Written:** 2026-10-14 · **By:** Opus 5.5 (`claude-opus-5-5`) through `a2a-bridge implement` · **Base:**
`c09e54d5` (C1 merged, PR #142) · **Scope:** C2a only; C2b (materialization) and C3 (terminal gate) are not started.
**Source snapshot:** `50544e77a985850b` (the first 16 hex digits of the SHA-256 of the five changed `.rs` files
concatenated in the C2a §7 order; per-file hashes are in the receipts).
**Receipts:** `.git/a2a-evidence/2b3c-c2a/` (listed in C2a §3.5).

## C2a 1. Context

Increment C2a of the 2B3c payload plane (task spec revision 4, "C2a — admission, prevalidation and plan"; decisions
D3, D5, D6, D7; the global input, consumed-byte, deadline and gate rules). It adds phase P's admission and its complete
prevalidation of the worktree, index and evidence frames into one bounded, pure plan. **Nothing is created, charged or
executed**: the only entry point returns the phase-G value unchanged beside the plan, or a typed failure that keeps it.

Lane: the bridge's Linux container (OrbStack kernel `7.0.14`), running as root, with every C2a control also run as an
unprivileged user (`setpriv --reuid=65534 --regid=65534 --clear-groups`, `TMPDIR` in a world-writable scratch
directory). **The native non-root macOS run was not performed here**; the spec gives it to the controller (C2a §6).

## C2a 2. What changed

### 2.1 `crates/bridge-core/src/custody_restore_payload.rs` (new; mounted as `custody_restore::payload`)

| Item | Contract |
|---|---|
| `plan_payload_plane_v1(plane, frame_budget, deadline)` and `plan_payload_plane_bounded_v1(…, table_bound)` | The only entry points. Consume a `RestoredGitPlaneV1`; return `PlannedPayloadPlaneV1 { step: Prevalidation, plane, plan }` or `Box<PayloadPlaneRestoreFailureV1>`. The bounded form is the small-bound test seam (phase G's `_bounded_v1` pattern); the production bound is 64 MiB. |
| Admission | Entry guard, then D5: any Captured class outside {RefsAndHead, ObjectDatabase, StashAndReflogs, Worktree, Index, InProgressGitOperations, GitConfigurationAndHooks, BridgeEvidence} → `UnsupportedCapturedClass { class }`. Then each of the five phase-P classes must be Captured with exactly one staged payload (`Captured { staged }`) or Empty with none; every other state or shape → `UnsupportedCoverage { class, detail }`, never downgraded. Then `reverify_staged_v1` on each phase-P payload, then the first gate. |
| Prevalidation | In step order (worktree, index, then the three evidence classes): a per-frame guard, `reverify_staged_v1` again, then a complete decode through the verified trailer into a null sink via phase G's `PacedFrameReaderV1` (deadline before every read, bounded to length + 1, every consumed byte counted and hashed), a per-entry `before_deadline`, and each entry fed to the table. The first entry refusal is **held**; after the trailer, count and SHA-256 must equal the staged length and digest (`StagedInputChanged`), and only then is a held refusal returned. Then a decoded-frame guard and the rewind. After every frame, the gate again. |
| `PayloadPlanTableV1` | Pure. Keys are `(plane, exact path bytes)`. Rules, in order: D6 components (empty, `.`, `..` → `InvalidComponent`); D3 for the worktree root `.git` (childless directory → `SkipConnector`; regular file → `RootGitfileToEvidence`; symlink → `GitConnectorSymlink`; any path beneath `.git/` → `BeneathGitConnector`); index names (top-level `index` or `sharedindex.*` only → `IndexName`; regular files only → `IndexKind`); D6 duplicates (`Duplicate`, any kinds); D6 parent (a `Directory` entry of the same frame, else `ParentNotDirectory`); D7 (absolute, or lexically leaving the plane root from the link's parent → `external`); the D6 charge `2 × path + target + 64`, checked (`checked_mul`/`checked_add`), `≤ bound` else `PlanTableBound`. A refused entry is not admitted and charges nothing. Each frame's entries are moved out once it is whole, so the table never holds a third copy of a path. |
| `PayloadPlanV1` | `classes` (the five, in step order, with their state), `frames: Vec<PlannedFrameV1>` (class, plane, staged index, staged length and SHA-256, every entry in frame order with exact path bytes, kind with frame mode / length / verbatim target / `external` / `childless`, and action), `root_git: RootGitDispositionV1` (`ConnectorDirectorySkipped`, `RootGitfileToEvidence`, `Absent`), `table_bytes`. Derived views: `childless_directories()` (worktree directories with no entry beneath them, connector excluded; no claim which was a Git directory) and `declared_external_references()` (`ExternalSymlinkV1 { plane, path, target }`). |
| `PayloadPlaneStepV1` | All six steps listed; C2a reaches `Admission` and `Prevalidation`. |
| `PayloadPlaneErrorV1` | `UnsupportedCapturedClass`, `UnsupportedCoverage`, `StagedInputChanged`, `PayloadFrameInvalid { class, fault: PayloadFrameFaultV1 }`, `Timeout`, `Restore(CustodyRestoreErrorV1)` (frame decode, identity, mount, I/O), `GitPlane(Box<GitBootstrapErrorV1>)` (a shared phase-G check's refusal no variant above names; unreachable for a consistent plane). `From<GitBootstrapErrorV1>` keeps the meaning of phase G's `StagedInputChanged`, `Git(Timeout)` and `Restore`. |
| `PayloadPlaneRestoreFailureV1` | `{ cause, last_completed, payload_materialization_began (always false in C2a), ledger_used, partial: PayloadPlanePartialV1 { plane } }`. |
| Gate (private) | `pace("payload-gate")` → `refuse_mounts_within` → phase G's `output_chains` (repository, `.git`, objects, pack, info and each copied object file) → phase G's metadata directories and files → phase G's `source_files_unchanged` (the bootstrap's admitted files) → `retained_sweep` (every staging chain, HOME, XDG, each bootstrap directory, then `pace("payload-root")` and the destination root's identity LAST). Run at the end of Admission and at the end of Prevalidation. |
| Test seam (`#[cfg(test)]`) | `PayloadHookPointV1::{AdmissionEntry, PrevalidationRehashed, PrevalidationEntry, BeforePrevalidationGate}`, thread-local, guard-removed. `PrevalidationRehashed` is the prevalidation pass's own replacement hook (C2b will have its own). |

### 2.2 `crates/bridge-core/src/custody_restore_git.rs` (visibility only)

`pub(super)` on: `HOME_NAME_V1`, `XDG_NAME_V1`, `BOOTSTRAP_NAME_V1`, `before_deadline`, `observe_deadline`, `PaceV1`,
`coverage_state`, `PacedFrameReaderV1` and its five fields, `PinViewV1` and `PinViewV1::of_final`,
`source_files_unchanged`, `output_chains`, `split_relative`, `metadata_name`, `pin_beneath`, `recheck_regular_entry`.
rustfmt rewrapped `observe_deadline`'s signature. No body changed; every 2B3b control is unchanged and green.

`sweep_chains` was **not** widened: the 2B3b structural control `round6_the_late_validation_order_is_fixed_in_the_source`
reads it with `body_of(source, "fn sweep_chains(")`, which derives the closing-brace indentation from the column of
`fn`, so a `pub(super) ` prefix would break that unchanged test. Phase P's `retained_sweep` therefore restates the same
edges in the same order (C2a §5.2).

### 2.3 `crates/bridge-core/src/custody_restore.rs`

The `#[path = "custody_restore_payload.rs"] mod payload;` mount beside phase G's, with a two-line comment.

### 2.4 `crates/bridge-core/src/custody_restore_git_tests.rs` (one mount; outside the task's file list, C2a §5.1)

Five appended lines mount `custody_restore_payload_tests.rs` as `git_tests::payload_tests`, so the C2a controls reuse
the private 2B3b fixtures (`CustomCapsuleV1`, `phase_v`, `PlaneInV1`, `stage_frame`, `manifest_with_row`, `snapshot`,
the lane route) without widening any of them. No 2B3b test is changed.

### 2.5 `crates/bridge-core/src/custody_restore_payload_tests.rs` (new; 18 controls)

Every behavioural row runs over a real phase-G value: a real 2B2 export → real phase V → the real `restore_git_plane_v1`
with real Git, then phase P. Frames are either real (the rich clone; the planned separate-Git-directory source) or hand
encoded with the real encoder and staged by `stage_frame` **before** phase G runs. `PCaseV1::run` snapshots the whole
destination and the one ledger before phase P and requires both unchanged after (or equal to what a drifting hook left),
`repository/` still holding only `.git`, and a refusal's `ledger_used`/`payload_materialization_began` shape.

| Control | Proves |
|---|---|
| `c2a_the_rich_clones_real_frames_plan_whole_and_change_nothing` | The rich clone (all eight union classes Captured) plans; frames in step order with their exact planes; each frame bound to its staged payload and equal to the test's own decode (paths, kinds, modes, lengths, targets); content bytes; table charge = the test's own arithmetic; root `.git` = skipped connector; `target/` absent; hook mode `0755` kept; bridge evidence present; nothing external; offsets 0; a second plan is equal. |
| `c2a_each_phase_p_class_is_captured_with_one_payload_or_empty_and_nothing_else` | For all five classes: Captured+payload plans; Empty plans nothing; ExcludedReproducible, Captured without a payload, Unresolved, Empty with a payload, and Captured with two payloads refuse `UnsupportedCoverage` with `last_completed = None`. All Empty → no frames, `Absent`, charge 0. |
| `c2a_a_captured_class_outside_the_union_refuses_at_admission` | Each of the six non-union classes Captured (with and without its frame) refuses `UnsupportedCapturedClass`, ahead of a broken phase-P class beside it. |
| `c2a_a_root_gitfile_goes_to_evidence_and_every_childless_directory_is_disclosed` | D3 opposite branch on a real planned capsule (`git init --separate-git-dir=repo/.gd` + `repo/empty`): `RootGitfileToEvidence`; childless = [`.gd`, `empty`], both `Create`; nothing beneath `.gd/`. |
| `c2a_the_git_connector_is_skipped_or_sent_to_evidence_and_anything_else_refuses` | Childless `.git` → skipped; gitfile → evidence; none → `Absent`; childless `sub/.git` → ordinary empty directory; `.git` symlink, `.git/config`, `.git/hooks` refuse (held until the frame is whole). |
| `c2a_external_symlink_targets_are_declared_with_their_exact_plane` | D7 in the worktree and in `GitConfigurationAndHooks`: `dir/link -> ../inside`, `..`, `./x/../x`, `../dir/inside` internal; absolute, `../outside`, `../../x`, leave-and-re-enter, non-UTF-8 absolute external; exact plane/class, path and target bytes, in plan order. |
| `c2a_byte_distinct_names_that_render_alike_stay_distinct` | `n\xff`, `n\xfe`, `n\u{FFFD}` and two `d\xff/x`, `d\xfe/x` subtrees: seven exact entries, each child finding its own parent. |
| `c2a_the_index_frame_holds_only_index_and_shared_index_files` | `index` + `sharedindex.0123abcd` plan with modes and lengths; `other`, `INDEX`, `sub/index`, a directory `index`, a symlink `sharedindex.x` refuse. |
| `c2a_the_plan_table_refuses_every_path_rule_and_never_wraps` | Pure table: empty/`.`/`..` components, missing/regular/symlink parents, duplicates in every kind, `.git/x` without a connector, a nested index name; refused entries charge nothing; one path in two planes is two entries; checked arithmetic refuses near `u64::MAX`. |
| `c2a_the_plan_table_bound_is_exact_for_paths_and_for_targets` | Path-dominated (8 × 203-byte names) and target-dominated (4000- and 3000-byte targets) frames: bound = charge plans with `table_bytes = charge`; bound = charge − 1 refuses `PlanTableBound`. |
| `c2a_a_frame_replaced_during_the_prevalidation_pass_refuses_as_changed` | The `PrevalidationRehashed` hook writes a valid same-length frame in place → `StagedInputChanged` (`last_completed = Admission`); that frame alone plans; a frame grown after admission is refused by prevalidation's own rehash; a changed byte, an appended byte and a truncation after phase G are refused by admission's rehash (`None`). |
| `c2a_a_held_entry_refusal_yields_to_the_byte_binding` | A same-length replacement whose entry would be refused (`indez`) is refused as `StagedInputChanged`, never `IndexName`. |
| `c2a_a_frame_that_does_not_decode_whole_refuses` | Another generation, a truncated trailer, another class's header → `FrameInvalid { Worktree }`. |
| `c2a_the_first_gate_refuses_a_changed_retained_chain` | At `AdmissionEntry`: metadata `refs`, `objects/info`, `HEAD` as another inode, a copied pack, an admitted bootstrap pack, `git-bootstrap/objects`, `plain`, `home`, the destination root → `IdentityChanged`, `None`. |
| `c2a_a_mount_inside_the_destination_or_a_failing_census_refuses_at_its_gate` | Exactly two censuses per clean run; a mount at census 1 → `None`, at census 2 → `Admission`; a failing census fails closed. |
| `c2a_prevalidations_gate_refuses_a_chain_changed_after_the_plan` | At `BeforePrevalidationGate`: metadata `refs` and the root → `IdentityChanged`, `Admission`. |
| `c2a_the_one_deadline_is_kept_before_every_step_and_entry` | Spent at entry (only `payload-entry` observed, offsets unmoved); spent at entry `b` (no later entry decoded); the clean run's guard sequence equals one computed from the plane's own chains; nine guards each halt alone with `Timeout` and the right `last_completed` (scoped threads; each row asserts its hook fired before the deadline); brief delays at every guard succeed. |
| `c2a_the_planner_holds_no_write_vocabulary_and_no_unsafe` | Structural only: the production half of `custody_restore_payload.rs` mentions no write/create/link/chmod/rename/ledger-charge/Git-runner/`unsafe` vocabulary; `syn` finds no `unsafe` site in either new file. |

## C2a 3. Verification

All runs were in the foreground with `CARGO_INCREMENTAL=0` and the proxies unset (one exception is disclosed in §3.3).

### 3.1 Targeted controls (final bytes)

- As root: `18 passed; 0 failed; 0 ignored; 1487 filtered out` (~19 s).
- As `nobody`: `18 passed; 0 failed; 0 ignored; 1487 filtered out` (~27 s); the scratch `TMPDIR` was empty afterwards.

### 3.2 Required local gates

| Gate | Result |
|---|---|
| `cargo fmt --all -- --check` | clean |
| `cargo clippy --workspace --all-targets --all-features -- -D warnings` | clean on final bytes. The first run flagged one `type_complexity` in a test (a local alias fixed it). |
| `cargo test -p bridge-core` | **1644 passed, 0 failed, 0 ignored; nothing excluded**: lib 1505 (C1's 1487 + 18), trybuild 1, 13 integration binaries 128, doc-tests 10. Run as an exact partition (§3.3). |
| `cargo run -p a2a-bridge -- validate --repo-hygiene` | `repository hygiene validated` (41 tracked artifacts, 9 example configs) |
| `git diff --check` | clean (new files added intent-to-add for the check) |

### 3.3 The full suite in this lane: load-sensitive deadline controls (outside scope; reported, not fixed)

1. One monolithic `cargo test -p bridge-core` (9 m 17 s) gave lib `1504 passed; 1 failed`: the 2B3b control
   `round6_each_late_validation_guard_halts_the_run_alone` ("guard #59 `rewind`: the fixture found None", with
   `guards_run=0`: its 6 s deadline was spent before the bootstrap reached its first guard). Cargo stopped there. It
   passed alone on the candidate (`05b`).
2. **Base control, same environment**: the base commit `c09e54d5` exported with `git archive` (no checkout) to
   `/tmp/c2a-base` and built with its own target dir. Its full lib suite under the same load showed three other 2B3b
   deadline controls failing (`b2a_an_interrupted_read_or_write_is_never_retried_after_the_deadline`,
   `b2a_a_slow_short_write_keeps_exactly_the_accepted_prefix_and_makes_no_second_write`,
   `b2c_every_final_guard_halts_the_run_alone_and_no_success_follows_expiry`) and did not finish within 590 s (`05c`).
   Each passed alone on the base (`05d`). The load average during these runs was ~13–14 on 15 cores.
3. A filtered run of all 365 `custody_restore::git_tests` controls (default threads) exceeded the 600 s foreground limit
   and **the harness moved it to the background**; I stopped it at once. It had already completed: `363 passed; 2 failed`
   (the same two `b2a` controls as the base control). Its log is kept as `05e`.
4. The receipt run: the lib binary run directly in eight filtered partitions at `--test-threads 6`, each under a 580 s
   foreground timeout: `b2c` 66, `b2a` 40, `b2b` 49, `b1a` 31, `b1b` 49, `round` 26, `corrective` 25, the rest of
   `git_tests` (with the 18 C2a controls) 79, and the rest of the lib 1140 = **1505, all passed**; also one 2B3b control,
   `b2c_lossless_raw_name_aliases_…`, whose own fixture reported its row "INVALID (not behavioral evidence)" in a first
   default-thread `b2c` partition, passed alone and in the 6-thread rerun. Then `cargo test -p bridge-core --test '*'`
   (14 targets: trybuild 1 + 128) and `--doc` (10).

Conclusion: the 2B3b deadline controls are load-sensitive in this container on the base commit too; nothing C2a changes
is in their path (C2a touches phase G's file only by visibility). The full workspace suite and the macOS run are CI's and
the controller's.

### 3.4 Final-bytes run

After the last edit (a test-only validity assertion in `halting_row`), on snapshot `50544e77a985850b`: fmt, clippy,
repo hygiene and `git diff --check` clean; the 18 controls as root and as `nobody` (§3.1); the `git_tests` remainder
partition again `79 passed`.

### 3.5 Receipts (`.git/a2a-evidence/2b3c-c2a/`)

| File | Content |
|---|---|
| `base-commit.txt` | `c09e54d5…` |
| `01-red-missing-symbol.log` | Structural RED (§4.1). |
| `04-clippy-workspace.log` | §3.2 |
| `05-test-bridge-core-root-linux.log` | §3.3 item 1 |
| `05b-round6-alone-candidate.log`, `05c-base-control-lib-root-linux.log`, `05d-base-flakes-alone.log`, `05e-test-lib-part1-git-tests.log` | §3.3 items 1–3 |
| `05f-binary.txt`, `05f-part-*.log`, `05f-b2c-lossless-alone.log`, `05g-integration.log`, `05h-doc.log` | §3.3 item 4 |
| `06-repo-hygiene.log` | §3.2 |
| `07-run_mutations.py`, `07-mutation-matrix-{a..i}.json`, `07-summary.md` | §4.2 |
| `08-final-*` | §3.4 |
| `source-sha256.txt` | Per-file SHA-256 of the five changed `.rs` files. |

## C2a 4. RED evidence

### 4.1 Structural RED

The production module was drafted before the controls, so structural RED was taken by compiling the finished controls
with the `mod payload;` mount removed: `cargo test -p bridge-core --lib --no-run` failed with
`E0432 unresolved import super::super::payload` (and its derived `E0282`s). The behavioural evidence is §4.2.

### 4.2 Behavioural RED: the single-guard mutation matrix

`07-run_mutations.py` applies each row as exact, unique string replacements to `custody_restore_payload.rs`, rebuilds,
runs the 18 controls as root and as `nobody`, records the failing controls, restores the file and checks the tree hash.
**56 rows: 54 turned controls red, every row identically as root and as `nobody`; 2 survived (below).** The full
row → control table is `07-summary.md`; condensed:

| Rows | Guard removed | Controls turned red |
|---|---|---|
| M01–M03 | D5 check removed; union widened (LinkedWorktrees); union narrowed (StashAndReflogs) | union control; rich clone / separate-Git-dir / first gate |
| M04–M07 | Captured with ≥1 payload; Captured-without → Empty; Empty-with accepted; other states → Empty | class-state control |
| M08, M10 | admission's / prevalidation's rehash removed | replacement control (+ deadline control) |
| M09, M32 | admission's / prevalidation's gate removed | first-gate, mount, deadline / prevalidation-gate, mount, deadline |
| M11, M13 | binding SHA-256 removed; held refusal returned before the binding | replacement + held-refusal / held-refusal |
| M14, M18–M23 | per-entry guard; entry, frame, decoded-frame, pre-census, pre-root, first-chain guards | deadline control (exact guard sequence or halting rows) |
| M15, M17 | decode error ends quietly; closing rewind removed | decode control; ten controls (offset 0) |
| M24–M31 | census; object chains; metadata chains; admitted files; staging; HOME/XDG; bootstrap dirs; root identity | mount / first-gate / prevalidation-gate controls |
| M33–M39 | D3 symlink, beneath-`.git`, connector created, gitfile created, disposition dropped, childless never cleared, connector listed | connector, rich clone, separate-Git-dir, lossless, table controls |
| M40–M43 | D6 components; duplicates; parent rule; lossy keys | table control; lossless control |
| M44–M46 | D7 absolute; root escape; base depth from the plane root | external-target control |
| M47–M49 | index name rule; `sharedindex.*` refused; regular-file rule | index control (+ held-refusal, table) |
| M50–M54 | 1 × path; target dropped; 63-byte allowance; saturating sum; `<` bound | bound control (+ rich clone); table control (overflow) |
| M55, M56 | step order; every evidence class in one plane | rich clone |

Survivors, both predicted before the run and kept deliberately (phase G has the same two):
- **M12** (the consumed **count** comparison removed): any count difference also changes the consumed SHA-256, so the
  digest comparison alone refuses every observable case; the count is defence in depth.
- **M16** (the explicit content drain removed): the decoder's next `next_entry` drains and digest-verifies unread
  content itself, through the same paced, hashed reader.

Rows M01–M17, M22 and M24–M28 ran before the exact clean-run guard sequence was added to the deadline control;
M18–M21 and M23 were re-run after it (M23 had survived its first run, which is why that assertion was added), and
M29–M56 ran after it. Later edits only add assertions.

## C2a 5. Open items: interpretations for the reviewer and controller

1. **A file outside the task's list.** `custody_restore_git_tests.rs` gains the five-line `payload_tests` mount (§2.4).
   The acceptance criteria require the 2B3b fixtures, which are private to that module; the alternatives were widening
   about twenty fixtures there or copying them. No test there changed. If the controller prefers widening, it is
   mechanical.
2. **`retained_sweep` restates `sweep_chains`** (§2.2): same edges, same order, same refusals, with phase-P guard
   labels. The other gate parts call phase G's own helpers.
3. **Gate scope.** "Staging and `.git` chains" is read inclusively: phase G's object store and metadata, the bootstrap's
   admitted files, and the staging, HOME, XDG and bootstrap directories, i.e. everything phase G's own gates recheck.
   The gate runs at the end of Admission (the "first gate") and at the end of Prevalidation ("at the end of each step";
   it is also the gate before C2b's first write), so a clean plan takes two censuses.
4. **Admission rehashes only the five phase-P payloads**; phase G already rehashed and consumed the rest.
5. **D3 is applied to the worktree root `.git` only**, as written. A childless `sub/.git` is planned as an ordinary empty
   directory (coverage's connector is matched by identity, so a separate Git directory named `.git` below the root is a
   real case). A nested `.git` *with content* is not refused by phase P; coverage parks it (Worktree becomes
   Unresolved, which admission refuses), so it cannot arrive in a sealed capsule. If the reviewer wants defence in
   depth here, it is one more table rule.
6. **Index entries must be regular files** (`IndexKind`), as a consequence of "`index` and `sharedindex.*`, byte-exact"
   and D4. The `sharedindex.` prefix rule is exactly coverage's routing predicate (so an empty suffix is admitted).
7. **Error shape.** The table bound is a `PayloadFrameFaultV1::PlanTableBound` inside `PayloadFrameInvalid`, as phase G
   nests `PathTableBound` inside `ActiveFrame`. Phase-G helper refusals are mapped by `From<GitBootstrapErrorV1>`.
8. **API shape.** The entry point consumes the plane and returns it beside the plan, so C2b's
   `restore_payload_plane_v1` can be `plan_payload_plane_v1` followed by materialization, and its failure partial can
   grow pins and inventory rows on top of `PayloadPlanePartialV1 { plane }`.
9. **Charge accounting.** Every admitted entry is charged, the skipped connector and the root gitfile included. Derived
   lists (childless directories, external references) are computed on demand rather than stored, so they add no
   uncharged copies.
10. **D1's evidence directory names** are not needed until C2b and are not added here.
11. **The recording Git route is not used by the C2a controls**: they run the same wrapper over the lane route (no child
    is tampered with), because `RouteV1` leaks its sealed anchor under a non-root run (C2a §6).

## C2a 6. Risks

- **The macOS native non-root run has not happened.** Facts it decides: `git init --separate-git-dir` and the planner on
  APFS (the separate-Git-directory control); the macOS mount census at both gates; the 3 s deadline rows on that host
  (each starts only after its plane exists; a row whose deadline is spent before its guard reports itself INVALID rather
  than as a behavioural failure). C2a writes nothing, so case folding and symlink limits are not exercised yet (C2b).
- **Pre-existing, outside scope (reported, not fixed):**
  - the 2B3b deadline controls are load-sensitive in this container, on the base commit too (§3.3);
  - the 2B3b `RouteV1` fixture drops its `TempDir` before its `SealedAnchorV1` restores the anchor's `0700`, so every
    phase-G control that uses it leaves its area behind in `TMPDIR` under a non-root run (measured: 2 directories for
    one `b2c` control as `nobody`). This will show on the controller's macOS run as leftovers, not failures.
- **The suite's wall time.** The C2a controls add ~10–20 s of wall time to `cargo test -p bridge-core` (the deadline
  control runs its nine halting rows in parallel).
- **D3 nested `.git` with content** (§5.5).

## C2a 7. Files

Changed and staged, and nothing else:
- `crates/bridge-core/src/custody_restore.rs` — the `payload` mount (§2.3)
- `crates/bridge-core/src/custody_restore_git.rs` — visibility only (§2.2)
- `crates/bridge-core/src/custody_restore_git_tests.rs` — the `payload_tests` mount only (§2.4, §5.1)
- `crates/bridge-core/src/custody_restore_payload.rs` — new (§2.1)
- `crates/bridge-core/src/custody_restore_payload_tests.rs` — new (§2.5)
- `docs/superpowers/reviews/2026-10-14-adr0041-slice2b3c-implementation-handoff.md` — this section

There are no dependency, `Cargo.lock`, public-API, wire, runner, `fs_custody`, frame-codec or coverage changes, and no
new `unsafe`. The commit message is in `.git/A2A_COMMIT_MSG`; the bridge commits. There is no push or merge.

## C2a 8. Next

1. The controller runs `cargo test -p bridge-core` natively on macOS as a non-root user on this candidate, before the
   C2a source review (§6).
2. C2a source review: Sol/xhigh, cap two rounds; §5 lists the interpretations to rule on, especially §5.1 (the test-file
   mount), §5.3 (gate scope and the two gates) and §5.5 (nested `.git`).
3. On approval, C2b on the approved commit: decode a third time against `PayloadPlanV1` (entry by entry, with the
   materialization pass's own consumed-byte binding and replacement hook), create at `0600`/`0700` through
   `create_new_regular_child` / `create_new_child_directory` / `create_new_symlink_child`, with the one ledger, the
   deadline, the per-step gates and `PayloadPlaneRestoreFailureV1` partial progress.

---

# Increment C2b — phase-P materialization (worktree, index, evidence)

**Written:** 2026-10-14 · **By:** Opus 5.5 (`claude-opus-5-5`) through `a2a-bridge implement` · **Base:**
`c86d392a` (C2a merged, PR #143) · **Scope:** C2b only; C3 (the terminal gate, final modes, the complete
`RestoredPayloadPlaneV1`) is not started.
**Source snapshot:** `e8a6f077fea26e39` after source-review round 1 (round 0 was `16ecb2344bdbfef0`); the first 16
hex digits of the SHA-256 of the three changed `.rs` files concatenated in the C2b §7 order; per-file hashes are in the
receipts.
**Receipts:** `.git/a2a-evidence/2b3c-c2b/` (listed in C2b §3.5; round 0's are kept in its `round0/`).

**Source review round 1/2 (Sol/xhigh): REJECT, two WRONG·BLOCKER findings, both folded here.**

| # | Finding | Disposition (round 1 bytes) |
|---|---|---|
| W1 | A create could follow an expired deadline: one check before an entry, then `evidence/`, its slot and the entry created with no check between (a slow `evidence/` create in its directory sync, then two more creates). | Fixed. The one deadline is now read immediately after every create returns, once what it made is kept: after `evidence/` (its pin kept, charged), after a slot, after a directory or symlink entry is recorded, right after a file's create-new (in flight, before its first byte), and after a written file is verified and recorded (guards `payload-created`, `payload-verified`). A directory entry is now recorded from its pin, with no further filesystem call between its create and that reading. New control `c2b_a_create_returning_after_the_deadline_is_never_followed_by_another` (7 rows, the reviewer's `evidence/` row first: `Timeout`, `evidence/`'s pin and its one allowance in the partial, no slot and no file on disk); mutation rows M43–M46 (§4.2). |
| W2 | The gates did not enforce the interim modes: a same-inode `chmod` of a created file or directory between its creation and a gate passed every gate, and the created file's mode was recorded but not required. | Fixed. A file's mode must be `0600` as its create returns, after its write (its own `fstat`), at its by-name recheck, and at every gate; every created directory (entries, `evidence/`, slots) must still be `0700` at every gate (`recheck_directory`, one no-follow `fstatat`: kind, the pin's device and inode, mode). New control `c2b_the_created_modes_are_required_at_creation_and_at_every_gate` (mode-only changes from `EntryCreated` and before each step gate → `IdentityChanged` with the prior step as `last_completed`; a file not `0600` as its create returns → refused at its create, in flight, nothing after); mutation rows M47–M50 (§4.2). §5.4 is replaced accordingly. |

## C2b 1. Context

Increment C2b of the 2B3c payload plane (task spec revision 4, "C2b — materialization"; D1–D4, D6, D7; the global
input, consumed-byte, write, budget, deadline and gate rules). It consumes C2a's plan and writes the worktree, the
index and the three evidence classes at `0600`/`0700`, each step ending in a gate, and returns a crate-private value
carrying everything C3's terminal gate needs. No final mode is applied and no terminal gate is built.

Lane: the bridge's Linux container (OrbStack kernel `7.0.14`), running as root, with every payload control (C2a's 18
and C2b's 19) also run as an unprivileged user (`setpriv --reuid=65534 --regid=65534 --clear-groups`, `TMPDIR` in a
world-writable scratch directory, empty afterwards). **The native non-root macOS run was not performed here**; the
spec gives it to the controller (C2b §6).

## C2b 2. What changed

### 2.1 `crates/bridge-core/src/custody_restore_payload.rs`

C2a's planner half is unchanged except the gate (below), the partial value, the new error variants and the test seams.
All C2b code is in one new section after the entry point, headed
`// Materialization (increment C2b): the worktree, the index, and the evidence`.

| Item | Contract |
|---|---|
| `materialize_payload_plane_v1(plane, frame_budget, deadline)` and `materialize_payload_plane_bounded_v1(…, table_bound)` | Plan exactly as C2a (`plan_payload_plane_bounded_v1`: admission, prevalidation, their two gates; a refusal there keeps C2a's shape), then three steps on the ONE deadline and the one ledger: **Worktree** (the worktree frame; D3's root gitfile to `evidence/worktree_root_gitfile/gitfile`), **Index** (`index`, `sharedindex.*` beneath `repository/.git`), **Evidence** (`InProgressGitOperations`, `GitConfigurationAndHooks`, `BridgeEvidence`, in step order, beneath `evidence/<serde wire name>/`). Each step writes its Captured frames and then runs the gate; `last_completed` advances only after the gate. Returns `MaterializedPayloadPlaneV1` or `Box<PayloadPlaneRestoreFailureV1>` (§5.1 on the name). |
| Third decode (`materialize_frame`) | Guard `payload-write-frame`; the plan must bind the staged payload it names (role, plane, length, SHA-256), else `PlanMismatch` before any create; a `try_clone` of the retained descriptor (no staged name reopened); `reverify_staged_v1`; the pass's own hook `MaterializationRehashed`; the decode through phase G's `PacedFrameReaderV1` (deadline before every read, bounded to length + 1, every consumed byte counted and hashed). Each decoded entry must equal the planned entry at its position (path bytes, kind, mode, length, target), else a `PlanMismatch` is **held** and nothing after it is created. A create's refusal is also held; the deadline and the decoder's own failure end the decode at once. After the verified trailer: count + SHA-256 = staged (`StagedInputChanged`), then decoded count = planned count (`PlanMismatch`), then the held refusal; guard `payload-write-decoded`; rewind. |
| Entries | Before each create: guard `payload-write-item`. **Directory**: phase V's `create_staging_directory` (entry reserved first, `mkdirat 0700` + the primitive's owner/`0700`/identity verification, given back only on `EEXIST`, device check incl. the `CREATED_DEV` seam); `StagingCollision` → `DestinationCollision`; recorded from its pin, then guard `payload-created`. **File**: entry reserved, `create_new_regular_child` (`O_CREAT\|O_EXCL\|O_NOFOLLOW`, `0600`), `EEXIST` → given back + `DestinationCollision`; guard `payload-created` as the create returns (the file in flight); device and `0600` checked before the first byte; phase G's `stream_into` (each chunk reserved before it is written, only the unwritten part given back on a short/failed/zero write or the deadline; the deadline before every chunk); then the descriptor's `fstat` (regular, frame length, root's device, same inode, `0600`) and a by-name no-follow recheck (the same, `0600`); the descriptor is dropped; recorded, then guard `payload-verified`. **Symlink**: entry reserved, C1's `create_new_symlink_child`; `TargetExists` → `DestinationCollision` and `DestinationUnsupported` → typed refusal, both given back; any other refusal keeps the charge; device check; recorded, then guard `payload-created`. **Connector** (childless root `.git`): skipped. `evidence/` and each slot directory are created only when an entry needs them, each followed by guard `payload-created` once its pin is kept. So no create follows a create, write, or verification that returned after the deadline. |
| Recorded identity | `CreatedEntryV1 { path, kind: Directory \| Regular { length, sha256 } \| Symlink { target }, frame_mode, created_mode, final_mode, dev, ino }`. `created_mode` is `0600` for a file (required at its creation, after its write, and at every gate) and `0700` for a directory (required by the primitive and at every gate) (§5.4). `final_mode` is planned, never applied: the frame's mode for the worktree (D4), `0600` for the index (D4), `0400`/`0500` for evidence (D2), none for a symlink. `CreatedTreeV1 { entries (creation order), directories (pin per created directory) }`; `CreatedEvidenceV1 { root, slots: [EvidenceDirectoryV1 { slot, pin, tree }] }` (`evidence/` and slots end `0500`, constant); `PayloadCreatedV1 { worktree, index, evidence, in_flight: Option<InFlightPayloadFileV1 { name, expected_length, written }>, charged }`. |
| `MaterializedPayloadPlaneV1` | `{ step: Evidence, plane, plan, created }`: the phase-G value unchanged (descriptors at offset 0), the plan (classes, `root_git`, `childless_directories()`, `declared_external_references()`), and every created entry's identity, modes and directory pin. |
| Errors | New `PayloadPlaneErrorV1::{DestinationCollision { name }, DestinationUnsupported { name, target_len }, PlanMismatch { class, detail }}`. Budget, device crossing, identity, I/O and mount refusals stay `Restore(…)`; phase G's helpers keep their `From` mapping. |
| `PayloadPlanePartialV1` | Gains `plan: Option<PayloadPlanV1>` and `created: PayloadCreatedV1` (every created row and pin, the file in flight, the charge). Nothing is removed. |
| Gate | C2a's `retained_sweep` is split: `retained_chains` (the same staging/HOME/XDG/bootstrap edges, same order), then `created_chains` (the worktree tree beneath `repository/`, the index beneath `.git`, `evidence/`, each slot and each slot's tree, parents first: a directory by no-follow `fstatat` against its pin's device and inode and `0700`, a file/link against its recorded kind/device/inode/length and, for a file, `0600`; guard `payload-output-chain` per row), then guard `payload-root` and the root's identity LAST. `gate(plane, created: Option<&PayloadCreatedV1>, deadline)`; planning passes `None`, so its guard sequence and census count are unchanged (the C2a controls are unchanged and green). |
| Test seams | Hook points `MaterializationRehashed`, `MaterializationEntry`, `BeforeCreate`, `EntryCreated`, `BeforeStepGate`; the event gains `step`; `edit_plan_for_test` (a plan that does not describe its frames); the seam guard is now an enum. |

### 2.2 `crates/bridge-core/src/custody_restore_git.rs` (visibility only)

`pub(super)` on `stream_into`, `SinkV1` and its five fields. No body changed; every 2B3b control is unchanged and green.

### 2.3 `crates/bridge-core/src/custody_restore_payload_tests.rs` (19 new controls; 2 C2a edits)

C2a edits: the structural control `c2a_the_planner_holds_no_write_vocabulary_and_no_unsafe` now scans the planner half
only (it cuts at the C2b header, which must occur exactly once); `expected_guards` is refactored into `gate_guards`
plus the same sequence. The C2b runner (`PCaseV1::write_within`) holds on **every** outcome that the one ledger moved by
exactly what phase P reports it charged, that a failure reports that ledger's figure, that every entry phase V/G made is
unchanged (unless the row's own hook drifted the tree), and that every added entry lies in a phase-P plane (beneath
`repository/` but outside `.git`, `repository/.git/{index,sharedindex.*}`, or `evidence/`).
`assert_created_is_the_disk` holds that the recorded rows (identity, pin, bytes, SHA-256, target, modes, in-flight bytes)
are exactly what was added, and that the disk's own charge equals the reported one.

| Control | Proves |
|---|---|
| `c2b_the_rich_clone_restores_byte_exact_with_a_split_index` | The rich clone with a REAL split index (`git update-index --split-index`: `index` + one `sharedindex.<hash>`) and a worktree symlink, V→G→P: added = the test's own decode of every frame, byte-exact, nothing else; source bytes agree (shared index, README, hook, bridge evidence); `a2a-bridge/evidence.json` and `A2A_TASK.md` only beneath `evidence/bridge_evidence/`; no `target/`, no gitfile slot; slot names = serde wire names; frame/created/final modes (hook `0755`/`0600`/`0400`); descriptors at offset 0. |
| `c2b_a_root_gitfile_lands_in_evidence_and_nothing_at_repository_git` | Real separate-Git-dir capsule: the gitfile's source bytes at `evidence/worktree_root_gitfile/gitfile`, `repository/.git` still phase G's inode, `.gd`/`empty` empty `0700`; on hand frames the gitfile exists before the worktree gate and its slot precedes a later class's. |
| `c2b_symlinks_are_written_verbatim_and_never_followed` | Absolute, `..`, dangling, escaping, non-UTF-8 and to-directory targets in the worktree and an evidence class: verbatim; the outside file's bytes/inode/mode/times unchanged, the outside directory empty, no target created; no modes recorded; exactly the external links declared. |
| `c2b_a_symlink_the_destination_cannot_hold_refuses_typed` | 4095-byte target, probed at run time: held byte-exact (this lane) or `DestinationUnsupported` with nothing at its name and its allowance given back (macOS). |
| `c2b_the_ledger_exactly_fits_and_one_byte_over_refuses` | Content-dominated and entry-dominated frames: exactly-fits succeeds with the ledger full; one byte under refuses `Budget` at the last chunk (file in flight, 0 bytes) / the last entry (nothing at its name), figure = ceiling + 1 − last; no room refuses before anything exists (`began` false). |
| `c2b_a_short_write_then_an_error_charges_exactly_the_accepted_bytes` | 4-byte chunks; `WriteAfter(6)` and `WriteZeroAfter(6)`: 6 bytes on disk, charged `(1 entry, 6 bytes)`, in flight `written = 6`; `ShortWrites(3)` alone writes and charges all 10. |
| `c2b_the_deadline_expiring_mid_frame_or_mid_step_halts_with_the_partial_kept` | Six rows (an entry mid-frame, between two chunks, before the index frame's decode, before an evidence frame's decode, in the worktree and the evidence gates): `Timeout`, the right `last_completed`, exactly the entries created before; brief delays at every guard succeed. 10 s deadlines, started after the plane exists; each row asserts its hook fired before the deadline (INVALID otherwise). |
| `c2b_every_materialization_guard_is_in_its_place_and_halts_the_run_alone` | The clean run's guard sequence equals one computed from the plane's chains and the frames (with `payload-created`/`payload-verified` after every create); eleven halting rows (each materialization guard label, `evidence/`'s create included) → `Timeout` with the right `last_completed`. |
| `c2b_a_create_returning_after_the_deadline_is_never_followed_by_another` | (Review round 1, W1.) The guard after a create made slow: `evidence/` (pin and one allowance kept, no slot, no file), a slot (no entry), a directory (no child), a file's create (in flight, 0 bytes, nothing after; also the frame's last entry), a symlink, a written file's verification → `Timeout`, the right `last_completed`, exactly what was made before, recorded as on disk. |
| `c2b_the_created_modes_are_required_at_creation_and_at_every_gate` | (Review round 1, W2.) Mode-only changes (same inode, kind, length): a file `0777` and a directory `0755` right after creation, a file `0400`, a directory `0750`, the index `0644`, an evidence file `0400`, `evidence/` `0755` and a slot `0711` before their step gates → `IdentityChanged` with the prior step as `last_completed`; a file made `0644` as its create returns → refused at its create, in flight with no byte, nothing after. |
| `c2b_a_frame_replaced_during_the_materialization_pass_refuses_as_changed` | The pass's own hook writes a valid same-length frame: `StagedInputChanged` for the worktree/index/evidence with `last_completed` Prevalidation/Worktree/Index, the written bytes stay; prevalidation's hook fired once (its own pass); a change before this pass's rehash refuses before any create; a held collision yields to the binding; an unplanned name is never created. |
| `c2b_a_plan_that_does_not_describe_its_frames_refuses` | Plan edits (digest, length, an entry the plan lacks, an entry the frame lacks) → `PlanMismatch`, with exactly the earlier entries kept. |
| `c2b_an_entry_replaced_after_creation_refuses_at_its_step_gate` | Nine rows at the step gates (file/dir/link/file→link/removed, index file, evidence file, `evidence/`, a slot) → `IdentityChanged` with the right `last_completed`; replaced during its own write → refused at its create, nothing after; replaced right after creation → later entries created, the gate refuses. |
| `c2b_a_device_crossing_refuses_and_keeps_the_charge` | Directory, file, link, index file, `evidence/`, a slot, an evidence file on another device → `DeviceCrossing` at its create, kept and charged, the right `last_completed`. |
| `c2b_every_step_names_its_last_completed_step` | Collisions planted just before the create of a worktree file and link, the index, `evidence/`, a slot → `DestinationCollision`, planted entry untouched, allowance given back, right `last_completed`/`began`; a clean run takes five censuses; a mount at census 3/4/5 → Prevalidation/Worktree/Index. |
| `c2b_case_and_normalization_variants_restore_or_collide_as_the_destination_decides` | `A`/`a` and NFD/NFC `é`: folding detected at run time; distinct on this lane (both restored), `DestinationCollision` on the second name with the first kept on a folding one. |
| `c2b_an_empty_class_creates_nothing` | All Empty: nothing added; one evidence class: only `evidence/`, its slot, its entry; an index alone: `repository/` holds only `.git`; a Captured evidence frame with no entry: nothing at all. |
| `c2b_a_second_invocation_refuses_and_changes_nothing` | Over a written destination: `DestinationCollision` at the first create (`repository/a`, or `evidence` after two empty steps), `began` false, the ledger and the tree unchanged. |
| `c2b_the_materialization_section_is_create_new_only` | Structural only: no rename/remove/truncate/chmod/path-symlink/sync/Git/`unsafe` vocabulary and no Git metadata literal in the C2b section; it uses the create-new primitives, the ledger, the retained descriptors and the deadline. |

## C2b 3. Verification

All runs were in the foreground with `CARGO_INCREMENTAL=0` and the proxies unset.

### 3.1 Targeted controls (final bytes)

- As root: `37 passed; 0 failed; 0 ignored; 1487 filtered out` (~31 s).
- As `nobody`: `37 passed; 0 failed; 0 ignored; 1487 filtered out` (~33 s); the scratch `TMPDIR` was empty afterwards.
- Runtime probes on this lane: `PAYLOAD-LONG-SYMLINK target_len=4095 holds=true`, `PAYLOAD-FOLDING case folds=false`,
  `PAYLOAD-FOLDING normalization folds=false`.

### 3.2 Required local gates

| Gate | Result |
|---|---|
| `cargo fmt --all -- --check` | clean |
| `cargo clippy --workspace --all-targets --all-features -- -D warnings` | clean (0 warnings) on final bytes. Earlier runs flagged `bool_assert_comparison` in a test and `unused_assignments` on a test-only path variable; both fixed. |
| `cargo test -p bridge-core` | **1663 passed, 0 failed, 0 ignored; nothing excluded**: lib 1524 (C2a's 1505 + 19), trybuild 1, 13 integration binaries 128, doc-tests 10. Run as an exact partition (§3.3). Round 0: 1661 (lib 1522). |
| `cargo run -p a2a-bridge -- validate --repo-hygiene` | `repository hygiene validated` (41 tracked artifacts, 9 example configs) |
| `git diff --check` | clean |

### 3.3 The full suite in partitions

C2a measured the monolithic `cargo test -p bridge-core` at over nine minutes in this container, beyond the 600 s
foreground limit, so the lib binary was run directly in nine filtered partitions at `--test-threads 6`, each under its
own foreground timeout: `b2c` 66 (143 s), `b2a` 40 (197 s), `b2b` 49, `b1a` 31, `b1b` 49, `round` 26 (137 s),
`corrective` 25, the rest of `git_tests` with the 37 payload controls 98, and the rest of the lib 1140 = **1524, all
passed** (round 1 bytes; round 0's 1522 are in `round0/`). Then `cargo test -p bridge-core --test '*'` (trybuild 1 + 128) and `--doc` (10). No failure occurred in any
partition, so no base-control run was needed; C2a's load-sensitive 2B3b deadline controls did not fail this time. The
rest-of-lib log also holds the known extra `1 passed` line of `path_identity_refuses_an_unreadable_ancestor`'s nested
child (C1 §3.3), excluded from the totals.

### 3.4 Final-bytes run

After the last source edit of round 1 (formatting, before its mutation matrix), on snapshot `e8a6f077fea26e39`: fmt,
workspace clippy, repo hygiene and `git diff --check` clean; the partitioned suite (§3.3); the 37 payload controls as
root and as `nobody` (`08-*`). The matrix restored the source byte-exact after every row (checked by SHA-256 in the script), and
`sha256sum -c source-sha256.txt` passed after the structural-RED build.

### 3.5 Receipts (`.git/a2a-evidence/2b3c-c2b/`)

| File | Content |
|---|---|
| `base-commit.txt` | `c86d392a…` |
| `01-red-missing-symbol.log` | Structural RED (§4.1). |
| `04-clippy-workspace.log` | §3.2 |
| `05-binary.txt`, `05-part-{1..9}-*.log`, `05-totals.txt`, `05g-integration.log`, `05h-doc.log` | §3.3 |
| `06-repo-hygiene.log` | §3.2 |
| `07-run_mutations.py`, `07-mutation-matrix-{a..k}.json`, `07-summary.md` | §4.2 |
| `08-final-targeted-{root,nonroot}-linux.log` | §3.1, §3.4 |
| `source-sha256.txt` | Per-file SHA-256 of the three changed `.rs` files. |
| `round0/` | Round 0's receipts (its 42-row matrix, partitions, structural RED, targeted runs, hashes). |

## C2b 4. RED evidence

### 4.1 Structural RED

The production section was drafted before the controls, so structural RED was taken by compiling the finished
controls with the C2b section (everything from its header) removed: `cargo test -p bridge-core --lib --no-run` failed
with 62 errors on round 1 bytes (59 on round 0), all unresolved C2b symbols (`EvidenceSlotV1`, `TreeAtV1`, `CreatedKindV1`, `PayloadCreatedV1`,
`CreatedTreeV1`, `InFlightPayloadFileV1`, `MaterializedPayloadPlaneV1`, `materialize_payload_plane_v1`, `split_path`,
`EVIDENCE_NAME_V1`) and their derived `E0282`s. The source was then restored and its hash checked.

### 4.2 Behavioural RED: the single-guard mutation matrix

`07-run_mutations.py` applies each row as exact, unique replacements to `custody_restore_payload.rs`, rebuilds, runs
all 37 payload controls as root and as `nobody`, records the failing controls, restores the file and checks its
SHA-256. On round 1 bytes, **50 rows (round 0's 42, re-pointed where their code changed, and M43–M50 for the review
fixes): 45 turned controls red, every row identically as root and as `nobody`; 5 survived (below).** The full row →
control table is `07-summary.md`; condensed:

| Rows | Guard removed | Controls turned red |
|---|---|---|
| M01, M02, M04, M05 | this pass's rehash; the SHA-256 binding; a create refusal or an unplanned entry returned before the binding | replacement control (+ guard, structural) |
| M06–M08 | decoded entry ≠ plan; decoded count ≠ plan; the plan's staged binding | plan control (+ replacement) |
| M09–M11, M19 | item, frame, decoded guards; the root's identity before the created chains | guard control (+ deadline, replacement) |
| M12, M13, M35 | the step gate; the gate without the created rows; a step completed before its gate | step-gate, guard, step, deadline controls |
| M14–M18, M38 | `evidence/`, slot, directory, file/link rechecks; inode/device dropped from the recheck; the by-name recheck after a write | step-gate control |
| M20–M22, M42 | a file's/link's entry not reserved; a file/link collision keeps its charge | ledger, step, second-invocation and most byte-exact controls |
| M24–M27 | a file's/link's device unchecked; `began` not set; the in-flight bytes not recorded | device, short-write, ledger, deadline controls |
| M28–M33, M36, M39–M41 | final modes; `evidence/` for an Empty class; the connector created; the gitfile into the worktree; a non-wire slot name; no rewind; an untyped directory collision; `evidence/` or the index beneath `repository/` | rich-clone, gitfile, empty-class, step, second-invocation and most byte-exact controls |
| M43–M46 | (W1) the reading after `evidence/`'s create, after a slot's, after a recorded entry, after a file's create-new | slow-create control, guard control (M46 also the mode control) |
| M47, M49, M50 | (W2) a file's `0600` at its creation; at the gates; a directory's `0700` at the gates | mode control |

Survivors, each predicted before its run and kept deliberately:
- **M03** (the consumed **count** comparison removed): any count difference also changes the consumed SHA-256, so the
  digest refuses every observable case (C2a's M12 and phase G have the same survivor).
- **M23** (a link the destination cannot hold keeps its charge): this lane holds 4095-byte targets, so the refusal
  branch never runs here; on macOS, `c2b_a_symlink_the_destination_cannot_hold_refuses_typed`'s charge check
  (`assert_created_is_the_disk`) decides it. The controller's macOS run is the evidence for this row.
- **M34** (the decode-ending refusals held instead): a held `Timeout` is re-raised by the loop's own `before_deadline`
  at the next iteration, and a held `FrameInvalid` by the poisoned decoder's next call, so the outcome is the same;
  `ends_the_decode` documents the intent (phase G keeps the same helper).
- **M37** (the written file's own `fstat` comparison removed): the by-name recheck that follows compares the same
  inode, device, frame length and mode, so every change the descriptor check can see is also refused there; it is
  defence in depth, as in phase G's `make_file`.
- **M48** (the `0600` dropped from that descriptor comparison only): the by-name recheck immediately after requires
  `0600` too, for the same reason as M37.

## C2b 5. Open items: interpretations for the reviewer and controller

1. **Entry-point name.** The spec's `restore_payload_plane_v1 → RestoredPayloadPlaneV1` includes the terminal gate,
   which is C3's. C2b's entry point is therefore `materialize_payload_plane_v1 → MaterializedPayloadPlaneV1` (step
   `Evidence`), and C3 can add `restore_payload_plane_v1` as materialization followed by the terminal gate.
2. **The root gitfile is written in the Worktree step.** Its bytes stream from the worktree frame, so writing it there
   needs neither buffering nor a fourth decode. `evidence/` and `evidence/worktree_root_gitfile/` can therefore first
   appear in the Worktree step; that step's gate covers them.
3. **A symlink charges one entry allowance only.** A target is at most 4095 bytes, within the 64 KiB allowance; the
   controls' disk measure counts a link as one entry.
4. **Interim modes are enforced (round 1, W2).** A file must be `0600` as its create returns (its `fstat`), after its
   write, at its by-name recheck, and at every gate; a directory is `0700` by its primitive and at every gate. A wrong
   mode is `IdentityChanged`, like any other change to a created entry. The creation-time arm is provoked through the
   guard that follows the create (a hook changes the mode there); the test process's umask is never changed. A
   symlink's mode is not compared (nothing reads it, and Linux cannot change it).
5. **Entries refused after their create.** A directory or link refused by a post-create guard (device, identity) stays
   on disk and charged and is named by the failure, but it is not an inventory row, because its identity was refused;
   a file stays as `in_flight`. This is phase G's B2b shape.
6. **No per-entry chain recheck.** Unlike phase G's B2b (a bounded recheck before each create), phase P relies on
   descriptor-relative creation, the by-name verification of each created entry, and the gates the spec places (before
   the first write, i.e. prevalidation's gate, and at the end of each step). A parent renamed away keeps receiving its
   entries through its retained descriptor, and the step gate then refuses (the "a swapped directory" row).
7. **Every step runs its gate, even an Empty one**, and each gate rechecks everything created so far (five censuses
   in a clean run; O(entries) per gate).
8. **`stream_into` is phase G's, unchanged**, so its guard label (`metadata-read`) and its seams (`BeforeObjectChunk`,
   `ObjectIo`, the object fault and chunk seams) apply to payload files too.
9. **No per-file fsync**, as in phase G's B2b; each directory create syncs its parent inside the primitive.
10. **The rich clone's split index.** `PlanFixtureV1`'s fields are private to another module, so the control derives the
    source (`<area>/scratch/capsule` → `<area>/clone`) from `capsule_dir()` and asserts its layout first.
11. **The plan-edit seam** (`edit_plan_for_test`) is the only way to exercise `PlanMismatch` after a verified binding;
    with an unmodified plan it is unreachable (the same bytes decode to the same entries).

## C2b 6. Risks

- **The macOS native non-root run has not happened.** Facts it decides:
  - the folding branch (APFS folds case by default, and normalization in both variants): expected
    `DestinationCollision` on the second name in both rows of the folding control;
  - the 4095-byte symlink refusal (expected `DestinationUnsupported`; it also decides survivor M23);
  - `git update-index --split-index` with the lane Git, and the planner's capture of a worktree symlink, on APFS;
  - the 10 s deadline rows (each starts after its plane exists and reports INVALID, never a false pass or fail, if a hook
    is reached late).

  If any of these fails, the right response is to report it, not to weaken the control.
- **Descriptor use.** Each created directory keeps its pin's descriptor until the value is dropped, as the spec's
  result requires. A worktree with more directories than the soft `RLIMIT_NOFILE` (256 by default on macOS) would fail at
  a create with a typed I/O refusal and its partial kept; files keep no descriptor. C3 and 2B3d may need to bound this.
- **Pre-existing, outside scope:** C2a §3.3's load-sensitive 2B3b deadline controls (not reproduced in this run) and
  C2a §6's `RouteV1` non-root leftovers (no C2b control uses the recording route).

## C2b 7. Files

Changed and staged, and nothing else:
- `crates/bridge-core/src/custody_restore_git.rs` — visibility only (§2.2)
- `crates/bridge-core/src/custody_restore_payload.rs` — the C2b section, the gate split, the partial value, the error
  variants and the seams (§2.1)
- `crates/bridge-core/src/custody_restore_payload_tests.rs` — 19 controls and two C2a edits (§2.3)
- `docs/superpowers/reviews/2026-10-14-adr0041-slice2b3c-implementation-handoff.md` — this section

There are no dependency, `Cargo.lock`, public-API, wire, runner, `fs_custody`, frame-codec or coverage changes, and no
new `unsafe`. The commit message is in `.git/A2A_COMMIT_MSG`; the bridge commits. There is no push or merge.

## C2b 8. Next

1. The controller runs `cargo test -p bridge-core` natively on macOS as a non-root user on this candidate, before the
   C2b source review (§6 lists what it decides).
2. C2b source review round 2/2: Sol/xhigh, on the round 1 bytes; the round 1 dispositions are at the head of this
   section. §5 lists the interpretations still to rule on, especially §5.1 (the entry point's name) and §5.6 (no
   per-entry chain recheck).
3. On approval, C3 on the approved commit: the terminal gate over `MaterializedPayloadPlaneV1` (census; the post-order
   walk of the worktree, index and evidence trees using `CreatedTreeV1`'s rows and pins, `set_exact_file_mode` /
   `PinnedDirectoryV1::set_exact_mode` with each row's `final_mode`; the retained chains; the root's identity last) and
   the complete `RestoredPayloadPlaneV1`.

---

# Increment C3 — the terminal gate, final modes, and the complete result

**Written:** 2026-10-15 · **By:** Opus 5.5 (`claude-opus-5-5`) through `a2a-bridge implement` · **Base:**
`0235b1d3` (C2b merged, PR #148) · **Scope:** C3 only; the `custody-restore.v1` record, the joint fixture and the
cross-phase wrapper are 2B3d's.
**Source snapshot:** `ca852ccd21844867` after review fix 2 (§3.8); `636fbb70fe70f227` after review fix 1 (§3.7,
§4.3; commit `0dfbcc06`); `19dacdaffffa441f` before it (the first 16 hex digits of the SHA-256 of the three changed
`.rs` files concatenated in the C3 §7 order; per-file hashes are in the receipts).
**Receipts:** `.git/a2a-evidence/2b3c-c3/` (listed in C3 §3.6): the first round's at the top level and under `round0/`,
review fix 1's under `fix1/`, review fix 2's under `fix2/`.

**Review fix 1 (REJECT, one blocker).** The first source review found directory edges checked pre-order: a directory
was observed by name before its children, so renaming it away and putting a decoy at its name while a child was being
processed went unnoticed (its children and its seal go through the retained pin, which follows the renamed original),
and the restore succeeded with the destination path naming the decoy. Fixed: each directory's one by-name observation
now happens when it is popped, after its whole subtree is sealed and immediately before its own seal, while its parent
is still `0700`; a slot's tree is walked before the slot is observed and sealed, and `evidence/` is observed and sealed
after every slot. The walk still makes exactly one terminal observation per entry. The sections below are updated in
place.

**Review fix 2 (round 2: two WRONG, folded by the controller as one disclosed extension).** (1) Material: the gate
observed the deadline once per entry and before each seal, so a file's or link's later calls, a mode primitive's
confirming `fstat`, and the record of a seal could follow a call that returned after the deadline. Fixed: the deadline
is observed after EVERY filesystem call the gate makes, before it makes another or records a seal; the enumeration of
every call and the check that follows it is §3.8. Two chain steps of the gate made several calls behind one guard
(phase G's admitted-file check, and C2a's staging chains), so the gate now runs C3-local copies of them with one guard
per call; the step gates keep the shared ones (C2b is unchanged). An entry whose mode primitive or confirmation
returned late is not recorded as sealed (§5.8). (2) Immaterial: §3.6 named files that did not exist at the stated
paths; it now lists the real `round0/`, `fix1/` and `fix2/` paths, and a one-shot manifest check proves every listed
path exists and the final hash file matches the sources (§3.8). Sections 2.1, 2.4, 5.6, 5.8, 6, 7 and 8 are updated in
place where fix 2 changed what they say.

## C3 1. Context

Increment C3 of the 2B3c payload plane (task spec revision 4, "Terminal gate", D2, D4, the API section and §C3; the
C2b section's §5 and §8 above). It adds the terminal gate after C2b's materialization and the complete crate-private
`RestoredPayloadPlaneV1`. The pin-retention model is unchanged; the descriptor count it implies is measured in §3.4 as a
2B3d design input.

Lane: the bridge's Linux container (OrbStack kernel `7.0.14`, aarch64), running as root, with every payload control
(C2a's 18, C2b's 19 and C3's 12) also run as an unprivileged user (`setpriv --reuid=65534 --regid=65534
--clear-groups`, `TMPDIR` in a world-writable scratch directory, empty afterwards). **The native non-root macOS run was
not performed here**; the spec gives it to the controller (C3 §6).

## C3 2. What changed

### 2.1 `crates/bridge-core/src/custody_restore_payload.rs`

The C2a and C2b halves are unchanged except: the module and `PayloadPlaneStepV1` docs, the new error variant, the
partial value's `sealed` list (both existing constructions pass `Vec::new()`), and five new test hook points. All C3
code is in one new section at the end of the file, headed
`// The terminal gate (increment C3): every created entry verified and sealed, and the result`.

| Item | Contract |
|---|---|
| `restore_payload_plane_v1(plane, frame_budget, deadline)` and `restore_payload_plane_bounded_v1(…, table_bound)` | C2b's `materialize_payload_plane_bounded_v1` (its failures returned unchanged), then the test hook `BeforeStepGate` with step `TerminalGate`, then `terminal_gate`. Success → `RestoredPayloadPlaneV1` (step `TerminalGate`), built by the infallible `restored`. Refusal → `PayloadPlaneRestoreFailureV1 { cause, last_completed: Some(Evidence), payload_materialization_began, ledger_used, partial }` with the phase-G value, the plan, every created row and pin, and `sealed`. The C2b entry point stays (§5.1). |
| `terminal_gate` (order) | Guard `payload-terminal-gate`; `refuse_mounts_within`; the walk of the worktree (beneath phase G's `repository/`), the index (beneath `.git`) and the evidence (`evidence/`, each slot, each slot's tree); then phase G's `output_chains`, C2a's `metadata_chains`, and (fix 2) `terminal_source_files_unchanged` and `terminal_retained_chains`, the C3-local copies of phase G's `source_files_unchanged` and C2a's `retained_chains` with one guard per call (§3.8); guard `payload-root`; `pinned_root_unchanged` as the tail expression. No `created_chains`: the walk is each created entry's sole terminal check. |
| The walk (`SealRunV1::seal_tree`, `seal_evidence`) | Iterative post-order over `CreatedTreeV1` (children grouped by exact parent path, creation order): each entry is visited from its retained parent pin while that parent is still `0700`. **Directory**: descended into through its retained pin with no by-name call; once every child is sealed (on the pop), guard `payload-terminal-entry`, `recheck_directory` by name from its parent (kind, the pin's `dev`/`ino`, `0700`), guard `payload-terminal-seal`, `PinnedDirectoryV1::set_exact_mode(final)`, then phase P's own `fstat` of the pin must read the final mode. Its own name is therefore checked after its subtree, never before (review fix 1). **Evidence**: each slot's tree through the slot's pin, then the slot observed by name from `evidence/` and sealed `0500`; after every slot, `evidence/` observed by name from the root and sealed `0500`. **File** (`seal_file`): guard; `recheck_created` by name (`Regular`, `dev`, `ino`, length, `0600`); reopened by name no-follow (`open_regular_file`; a link, `ENOENT` or a non-regular → `IdentityChanged`); the descriptor's `fstat` must be (`Regular`, `dev`, `ino`, length, `0600`); phase G's `stream_digest` (deadline before every read, at most length + 1 bytes) must give the recorded length and SHA-256, else **`ContentChanged { name }`**; the descriptor's closing `fstat` must equal its opening one ("changed while it was read"); guard `payload-terminal-seal`; `set_exact_file_mode(&file, final)`; phase P's own `fstat` of that descriptor must read the final mode; the descriptor is dropped. **Symlink** (`verify_symlink`): guard; `recheck_created` by name (kind, `dev`, `ino`, target length); `read_child_symlink` must equal the recorded target (a longer target is another link); a closing `recheck_created`. Never followed, never given a mode. Every recorded entry must be reached (else `IdentityChanged`). **Fix 2:** a guard after every call before the next one or the record of a seal: a file's stat (`payload-terminal-statted`), reopen (`payload-terminal-reopened`), digest (`payload-terminal-digested`), mode primitive (`payload-terminal-mode-set`) and confirming `fstat` (`payload-terminal-confirmed`, inside `confirm_sealed`, before the push); a directory's mode primitive and confirming `fstat` the same; a link's stat and `readlinkat` (`payload-terminal-link-read`). The full enumeration is §3.8. |
| Final modes | Each row's C2b `final_mode`: the frame's exact mode for the worktree (D4), `0600` for `index`/`sharedindex.*` (D4: set again, a no-op change, so every file takes one path), `0400`/`0500` for evidence entries (D2); `evidence/` and every slot `0500`. Sealing is through descriptors only, never a path. |
| `SealedEntryV1 { name, mode }`, `PayloadPlanePartialV1::sealed` | Every entry the gate sealed, destination-relative, in seal order; pushed only after phase P's confirming `fstat` and (fix 2) the guard after it. Nothing is unsealed on failure. |
| `RestoredPayloadPlaneV1` | `{ step, plane, worktree: WorktreeInventoryV1 { state, entries, root_git, childless_directories }, index: IndexInventoryV1 { state, files }, evidence: EvidenceInventoryV1 { classes: [(class, state); 3], root: Option<(mode, dev, ino)>, slots: [EvidenceSlotInventoryV1 { slot, mode, dev, ino, entries }] }, declared_external_references, pins: PayloadPinsV1 { worktree, evidence, evidence_slots: [EvidenceSlotPinsV1 { slot, pin, directories }] } }`. Every row is `RestoredEntryV1 { path, kind (length + SHA-256, or target), frame_mode, mode (written), dev, ino }`. |
| Test seams (`#[cfg(test)]`) | Hook points `TerminalEntry` (before an entry's first by-name call; a directory's comes once its subtree is sealed), `TerminalLookup` (before every by-name call of the walk), `TerminalDescriptor` (fix 2: before every call through an entry's own descriptor: a file's opening and closing `fstat`, a mode primitive, the confirming `fstat`), `TerminalDigested` (after the read, before the closing `fstat`), `TerminalModeSet` (after the primitive's `fchmod` + confirmation, before phase P's own `fstat`), `TerminalSealed`; `BeforeStepGate` gains the `TerminalGate` step. |

### 2.2 `crates/bridge-core/src/custody_restore_git.rs` (visibility only)

`pub(super)` on `stream_digest`. No body changed; every 2B3b control is unchanged and green (its structural controls
only search other bodies for `stream_digest(`).

### 2.3 `crates/bridge-core/src/custody_restore.rs`

Unchanged: C2a's `payload` mount already covers the new items, and nothing is re-exported.

### 2.4 `crates/bridge-core/src/custody_restore_payload_tests.rs` (12 new controls; C2b fixture edits)

C2b edits, behaviour-preserving: `PCaseV1` carries phase V's ledger figure (`phase_v`, from `PlaneInV1`); `disk_tree` is
`disk_tree_where(root, |_| true)`; `expected_added` is `expected_tree(plane, false)` beside `expected_final`; the C2b
structural control now cuts its section at the C3 header (which must occur once); the second-invocation row passes
`phase_v` on. All 37 C2a/C2b controls are green as root and as `nobody`.

C3 fixtures: `moded_frame` (the real encoder, each entry's own mode); `HeldTreeV1`, which opens every entry phase P added
at the `BeforeStepGate(TerminalGate)` hook, while every file is `0600` and every directory `0700`, so a sealed entry's
final mode and bytes are read afterwards through a descriptor opened before its seal, **never by changing a mode
back**; `UnsealV1`, which re-opens the destination area's directories only when a run is dropped, for removal as a
non-root owner (cleanup only; nothing is inspected through it); `PCaseV1::restore_with`, which holds on every outcome
phase G's accounting as phase P starts (`ledger == phase_v + phase G's on-disk charge`), that a failure reports the one
ledger and that it moved by exactly what phase P charged, that nothing outside phase P's planes changed (its own walk
never reads or descends into a phase-P entry), and on success **the one-ledger accounting: phase V's charge + phase G's
on-disk charge + phase P's on-disk charge (from the held descriptors)**; `assert_inventory_is_the_disk` (every row's
identity, mode written, length, SHA-256 and target is the held entry's; nothing added is missing; every pin is the
directory its row names); `assert_seals_are_the_disk` (on a refusal, exactly the `sealed` entries have their final mode,
every other added entry its created mode); `rich_case` (C2b's rich clone with a real split index and a worktree symlink,
with an optional plant before export).

| Control | Proves |
|---|---|
| `c3_every_frame_mode_restores_and_is_sealed_exactly` | Files `0000`/`0200`/`0755`; `0000` directories nested in a `0000` and in a `0750` directory, each holding a file; a `0555` directory with a file, a link and a `0700` directory; a childless `0711` directory; an escaping link; an evidence class with a `0777` hook and a `0000` directory: all restore and end with exactly their frame modes (evidence `0400`/`0500`, index `0600`), byte-exact through the held descriptors; the inventory shows each frame mode and mode written; `childless_directories = [empty]`; the escaping link declared. As non-root, a lookup beneath the sealed `0000` directories now fails `PermissionDenied`. |
| `c3_the_root_gitfile_ends_inert_and_every_childless_directory_is_reported` | D3's opposite branch on the REAL separate-Git-dir capsule: `RootGitfileToEvidence`, `childless_directories = [.gd, empty]`, the gitfile `0400` with the source's bytes and its frame mode inventoried, its slot `0500`, both childless directories at their frame modes, `repository/.git` still phase G's inode. |
| `c3_no_name_is_looked_up_beneath_a_sealed_directory` | The instrumented seam (`TerminalEntry`, `TerminalLookup`, `TerminalSealed`, in order): no event at or beneath an already-sealed entry; every added entry observed exactly once; every file and directory sealed once and no link; every directory's observation (worktree, evidence, slot, `evidence/`) after every event beneath it, with only its own lookup between it and its seal (fix 1); no retained phase-V/G pin (root, work, plain, staging, HOME, XDG, bootstrap, object store, metadata) lies at or beneath a sealed entry, so the chains after the walk cannot look a name up beneath one. |
| `c3_a_replacement_before_an_entrys_terminal_observation_refuses_typed` | 17 rows, each `IdentityChanged`, `last_completed = Evidence`, exactly the earlier entries sealed and nothing after: at `TerminalEntry`, a file as another inode, a swapped directory (its subtree already sealed), a file beneath it, a link as another same-target link, the index, `evidence/`, a slot, an evidence file, a file made `0000` in place, a link replaced by a regular file of its target's length; **the review's defect (fix 1)**: at a file's `TerminalEntry`, its directory renamed away (still walked through its pin) and a new directory put at its name, for `repository/d` (at `d/x`), the slot and `evidence/` (at the evidence file): the file is sealed, the restore refuses at that directory's own observation, and the directory stays `0700`; inside one observation (`TerminalLookup` by count), a file replaced before its reopen (another inode; a link), a link retargeted before its `readlinkat`, a link replaced before its closing stat. |
| `c3_a_file_rewritten_in_place_refuses_as_content_changed` | The same inode and length with one byte flipped, before the terminal gate (a worktree file, the index, an evidence file) and before the worktree's own step gate (which compares identity and mode, so passes it): `ContentChanged { name }` naming the file, `Evidence`, the rewritten bytes kept and the file still `0600`. A file rewritten with its own bytes after its read (`TerminalDigested`) refuses as changed while it was read. |
| `c3_a_mode_changed_after_its_fchmod_refuses` | At `TerminalModeSet` (after the primitive's `fchmod` and confirmation, before phase P's own `fstat`): a worktree file, a worktree directory, the index, an evidence file, `evidence/` → `IdentityChanged`, `Evidence`, the entry not in `sealed`, the changed mode what refused (§5.2). |
| `c3_a_refused_terminal_gate_names_evidence_and_keeps_everything` | A clean restore takes six censuses (two planning, three step gates, the terminal gate); a mount at the sixth refuses with nothing sealed; ten retained chains changed just before the gate (metadata `refs`, `objects/info`, `HEAD` and a copied pack as other inodes, an admitted bootstrap file and a bootstrap directory on a Captured database, plain staging, HOME, XDG, the root) refuse `IdentityChanged`, `Evidence`, after every entry is sealed (the walk precedes the chains; the root is last); `payload_materialization_began` is false where nothing was created. |
| `c3_the_one_deadline_is_kept_before_every_entry_and_every_seal` | (Fix 2: the slow rows run first.) Thirteen slow rows, one per call of the walk (a file's stat, reopen, opening `fstat`, digest, closing `fstat`, mode primitive, confirming `fstat`; a link's stat, `readlinkat`, closing stat; a directory's stat, mode primitive, confirming `fstat`): the deadline is spent at the seam just before the call (or just after it, for the digest and a mode primitive), and the run halts `Timeout`, `Evidence`, with NO later terminal event (the named next call's seam never runs), exactly the earlier entries sealed, and an entry whose mode was set late not recorded though it has its final mode. A clean restore's guards = the materialization's (recorded from a `materialize` run) + exactly the terminal gate's (computed from the frames and the plane's chains, one guard per call). Nine halting rows (the gate's guard, the first and last entry, the first and last seal, the first object chain, the first `payload-chain-step`, the last staging chain, the root) → `Timeout`, `Evidence`, exactly the earlier entries sealed; on a Captured database, `payload-source-statted` halts with nothing created. 10 s deadlines started after the plane exists; each row asserts its hook fired before the deadline (INVALID otherwise). |
| `c3_the_rich_clone_restores_end_to_end_on_the_one_ledger` | V→G→P on the rich clone (real split index, worktree symlink): byte-exact at every final mode; inventories = disk; skipped connector, the computed childless list, no external reference, every class Captured, every frame mode the frame's; source bytes agree; one ledger = phase V + phase G + phase P on disk (`PAYLOAD-ONE-LEDGER`); `PAYLOAD-DESCRIPTORS` (§3.4). |
| `c3_evidence_ends_inert_and_the_rich_clones_hook_is_never_executable` | Every evidence file `0400` and directory `0500`; the configuration class's frame modes inventoried beside the modes written; the hook (frame `0755`) observed `0600` at creation, at the Evidence and terminal step gates, `0400` once sealed and after: never an execute bit. |
| `c3_planted_config_hook_filter_and_fsmonitor_markers_never_run` | The rich clone's source gains `core.fsmonitor`, `core.pager`, an alias, a required `filter.marker` with `info/attributes`, an `[include]` setting `core.editor`, and executable `post-checkout`/`post-merge`/`reference-transaction` hooks, each running a marker script proven to record; nothing ran during planning and export (`PAYLOAD-MARKERS … []`) or the restore; the planted files are byte-exact `0400` evidence; nothing beneath `repository/` holds a planted byte or hook name; no `repository/.git/hooks` or `info/`. |
| `c3_the_terminal_gate_section_seals_through_descriptors_in_its_order` | Structural only: no path-based chmod, create, write, rename, remove, ledger, Git or `unsafe` vocabulary in the C3 section, and no `created_chains`; the gate's order (guard, census, three walks, the four chains, root guard, root identity as the tail); a file's order (observation, reopen, digest, `ContentChanged`, guard, seal, confirmation) with no lookup after its seal; a directory sealed through its pin after its guard; `seal_tree`'s one `observe_directory` on the pop, before the directory's seal, and `seal_evidence`'s order (slot tree, slot observation, slot seal, `evidence/` observation, `evidence/` seal) (fix 1); the success arm only builds the result, which is infallible. Fix 2: each new guard in its place in a file's, a link's and a directory's order and before `confirm_sealed`'s push; the gate calls the paced chain copies, never the shared sweeps; every `recheck_entry` of `terminal_retained_chains` has its own guard; an admitted file's two calls are separated by `payload-source-statted`. |

## C3 3. Verification

All runs were in the foreground with `CARGO_INCREMENTAL=0`, a target directory in `/tmp`, and the proxies unset.

### 3.1 Targeted controls (final bytes)

- The 49 payload controls as root: `49 passed; 0 failed; 0 ignored; 1503 filtered out` (33 s).
- As `nobody`: `49 passed; 0 failed; 0 ignored; 1503 filtered out` (34 s); the scratch `TMPDIR` was empty afterwards, so
  the sealed `0500`/`0555`/`0000` trees are removable after `UnsealV1`.
- Probes: `PAYLOAD-SEAL-MODES as_root=true|false`, `PAYLOAD-MARKERS during_planning_and_export=[]`, and C2b's
  `PAYLOAD-LONG-SYMLINK target_len=4095 holds=true`, `PAYLOAD-FOLDING case/normalization folds=false`.

### 3.2 Required local gates

| Gate | Result |
|---|---|
| `cargo fmt --all -- --check` | clean |
| `cargo clippy --workspace --all-targets --all-features -- -D warnings` | clean (0 warnings) on final bytes, a forced re-check of `bridge-core` and its dependents |
| `cargo test -p bridge-core` | **1697 passed, 0 failed, 0 ignored; nothing excluded**: lib 1552 (the base's 1540 + 12), 14 integration binaries 135 (trybuild `compile_fail` 1 included), doc-tests 10. Run as an exact partition (§3.3). The base figure 1540 is derived (1552 − 12); no failure occurred, so no base-control run was needed. |
| `cargo run -p a2a-bridge -- validate --repo-hygiene` | `repository hygiene validated` (41 tracked artifacts, 9 example configs) |
| `git diff --check` | clean |

### 3.3 The full suite in partitions (final bytes)

The lib binary was run directly in six filtered partitions at `--test-threads 6`, each under its own foreground timeout:
`b2c` 66 (132 s), `b2a` 40 (175 s), `b2b` + `b1a` 80 (69 s), `b1b` + `round` + `corrective` 100 (163 s), the rest of
`git_tests` with the 49 payload controls 138 (48 s), and the rest of the lib 1128 (39 s) = **1552, all passed**; then
`cargo test -p bridge-core --test '*'` (14 binaries, 135) and `--doc` (10). The same partitions also passed once before
the e2e control's baseline reading and two comment-only edits (`round0/05-*`, `05g-*`, `05h-*`). The rest-of-lib log holds the known extra `1 passed` line of
`path_identity_refuses_an_unreadable_ancestor`'s nested child (C1 §3.3), excluded from the totals. C2a §3.3's
load-sensitive 2B3b deadline controls did not fail in this run.

### 3.4 Descriptors (measured; a 2B3d design input, not a C3 change)

`c3_the_rich_clone_restores_end_to_end_on_the_one_ledger` run alone (`--exact --test-threads 1`, so the process-wide
`/dev/fd` count is exact), as root and as `nobody`, identically (`09-*`):

| Point | Open descriptors |
|---|---|
| Baseline (stdio, harness, the listing's own) | 4 |
| The phase-V/G value alive | 64 → **phase V and G retain 60** (staged plaintexts, phase V's and phase G's pins, the bootstrap's admitted files, phase G's retained metadata and object descriptors) |
| The phase-P value alive (the test's held descriptors dropped) | 73 → **phase P retains 9**, exactly its 9 directory pins (`evidence/`, three slots, five evidence directories; the rich clone's worktree has no directory) |
| Everything dropped | 4 |

Transient use on top: the terminal gate holds at most one file descriptor (each reopened file is dropped before the next
entry); C2b's materialization holds one output file at a time, and a directory create one extra duplicate while it checks
emptiness.

**The bound.** The production process inherits its shell's soft `RLIMIT_NOFILE`: 256 by default on macOS (1024 is common
on Linux; this container's is 1048576). `cargo test` raises its own limit and its test binaries inherit that, so the
suite does not exhibit the bound; the real binary would. For a restore whose phase-V/G value holds what the rich clone's
holds, phase P can retain about 256 − 4 − 60 − 2 ≈ **190 directory pins in total** on macOS before a directory create
fails with a typed I/O refusal (`EMFILE`) and its partial kept: about **180 worktree directories** beside the rich clone's
9 evidence directories (about 950 at a 1024 soft limit). The phase-V/G share grows with the capsule (more staged
artifacts, refs, reflogs and metadata directories), so the real headroom is smaller for a larger repository. 2B3d's
options include raising the soft limit to the hard limit at startup, releasing pins once the terminal gate has sealed
(nothing after it needs them except the record), or re-deriving identity per entry without retaining descriptors.

### 3.5 Final-bytes run

On snapshot `19dacdaffffa441f`, after the mutation matrix: fmt, workspace clippy, the partitioned suite (§3.3), and the 49
payload controls as root and as `nobody` (`round0/08-*`). Repo hygiene and `git diff --check` were run again after this
handoff was written. `sha256sum -c round0/source-sha256.txt` held on those bytes.

### 3.6 Receipts (`.git/a2a-evidence/2b3c-c3/`)

Corrected in review fix 2: the earlier table placed the first round's partitioned run, its matrix summary, its final
runs and its hash file at the top level, where they never were; they are under `round0/`. Every path below exists
(`fix2/12-receipt-manifest.log`, §3.8).

| File | Content |
|---|---|
| `base-commit.txt` | `0235b1d3…` |
| `01-red-missing-symbol.log` | Structural RED (§4.1). |
| `02-green-payload-root-linux.log`, `03-green-payload-nonroot-linux.log` | The first all-payload green runs (48 controls: before the root-gitfile control was added). |
| `04-clippy-workspace.log` | §3.2 |
| `round0/05-binary.txt`, `round0/05-part-{1..6}-*.log`, `05g-integration.log`, `05h-doc.log` | The first partitioned suite run (the lib partitions under `round0/`, integration and doc-tests at the top level). |
| `06-repo-hygiene.log` | §3.2 |
| `round0/07-mutation-matrix-{a..j}.json`, `round0/07-summary.md` | §4.2: rows M01–M47 in chunks a–i and the chunk-j re-run of M16, M25, M26 and M28; every row restored the file to SHA-256 `ff4bac6f…`. |
| `07-run_mutations.py` | The only retained copy of the mutation runner. It was written (01:32 UTC) after the `round0/` matrix and defines rows M48–M50 that the matrix does not have, so it is a later version of the runner that produced §4.2. |
| `07-mutation-matrix-{a..i}.json` | Not cited: a later 50-row run (M01–M50, 01:32–01:36 UTC) on a production file with SHA-256 `f1c8578b…`, which is neither the pre-fix file (`7c3b2e5d…`) nor fix 1's (`e0ce8812…`). |
| `round0/08-final-part-{1..6}-*.log`, `round0/08-final-integration.log`, `round0/08-final-doc.log`, `round0/08-final-payload-{root,nonroot}-linux.log` | §3.3, §3.5 |
| `09-descriptors-{root,nonroot}-linux.log` | §3.4 |
| `10-red-review-w1-committed-production.log`, `10-red-review-w1-swap-after-last-child.log` | Not cited: an earlier draft of review fix 1's RED (01:31 UTC; its row names predate the final rows), superseded by `fix1/03`–`fix1/06`. |
| `round0/source-sha256.txt` | Per-file SHA-256 of the three changed `.rs` files before review fix 1 (snapshot `19dacdaffffa441f`). |
| `fix1/01-green-payload-root-linux.log`, `fix1/02-green-payload-nonroot-linux.log` | Review fix 1: the 49 payload controls on the fixed bytes (§3.7). |
| `fix1/03-red-new-tests-committed-production.log`, `fix1/04-*`, `fix1/05-*`, `fix1/06-red-each-edge-swap-row-committed-production.log` | Review fix 1 RED and its isolated rows (§4.3). |
| `fix1/07-run_mutations.py`, `fix1/07-mutation-matrix.json` | Review fix 1 mutations F1–F5 (§4.3). |
| `fix1/08-clippy-workspace.log`, `fix1/09-part-{1..6}.log`, `fix1/10-integration.log`, `fix1/11-doc.log`, `fix1/12-repo-hygiene.log` | Review fix 1 gates and the full suite on the final bytes (§3.7). |
| `fix1/source-sha256.txt` | Per-file SHA-256 of the three `.rs` files at review fix 1, which round 2 reviewed: equal to commit `0dfbcc06`'s blobs (checked by `fix2/12`). |
| `fix2/base-commit.txt` | `0dfbcc06…` |
| `fix2/01-make_red_seams_only.py`, `fix2/01-red-seams-only-vs-0dfbcc06.diff` | Review fix 2: how the RED production file was made, and its whole difference from `0dfbcc06` (§3.8). |
| `fix2/02-red-c3-on-0dfbcc06-plus-seams.log` | Review fix 2 RED (§3.8). |
| `fix2/03-green-payload-root-linux.log`, `fix2/03-green-payload-nonroot-linux.log` | Review fix 2: the 49 payload controls on the final bytes. |
| `fix2/04-run_mutations.py`, `fix2/04-mutation-matrix.json` | Review fix 2 mutations (§3.8). |
| `fix2/05-fmt-check.log`, `fix2/06-clippy-workspace.log` | Review fix 2 gates. |
| `fix2/07-binary.txt`, `fix2/07-part-{1..6}-*.log`, `fix2/08-integration.log`, `fix2/09-doc.log` | `cargo test -p bridge-core` on the final bytes (the binary and source hashes in `07-binary.txt`). |
| `fix2/10-repo-hygiene.log`, `fix2/11-diff-check.log` | Review fix 2 gates. |
| `fix2/12-check_receipt_manifest.py`, `fix2/12-receipt-manifest.log` | The receipt-manifest check over this table (§3.8). |
| `fix2/source-sha256.txt` | Per-file SHA-256 of the three `.rs` files at review fix 2 (snapshot `ca852ccd21844867`). |

### 3.7 Review fix 1 (final bytes, snapshot `636fbb70fe70f227`)

Same lane and conditions as above (foreground, `CARGO_INCREMENTAL=0`, target in `/tmp`, proxies unset).
- The 49 payload controls: `49 passed; 0 failed` as root (39 s) and as `nobody` (39 s); the scratch `TMPDIR` was empty
  afterwards.
- `cargo fmt --all -- --check` clean; `cargo clippy --workspace --all-targets --all-features -- -D warnings` clean (0
  warnings; `bridge-core` re-checked); `git diff --check` clean (worktree and index);
  `cargo run -p a2a-bridge -- validate --repo-hygiene` → `repository hygiene validated`.
- `cargo test -p bridge-core`: **1697 passed, 0 failed**: lib **1552** in six partitions proven disjoint and exact
  against `--list` (`b2c_` 66; `b2a_` 41; `b2b_`/`b1a_` 80; `b1b_`/`round`/`corrective` 145; the rest of `git_tests` 138;
  the rest of the lib 1082; each partition skips the earlier markers), integration 135 (14 binaries), doc-tests 10. The
  rest-of-lib log again holds the known nested child's extra `1 passed` line, excluded. The partition boundaries differ
  from §3.3's (whose `b2a_`, `round` and `corrective` filters overlapped other partitions); the total is the same.
- Not re-run on these bytes: the full M01–M47 matrix (§4.3 says why) and the descriptor measurement (the fix changes
  no pin or descriptor).

### 3.8 Review fix 2 (final bytes, snapshot `ca852ccd21844867`)

**Finding 1: the deadline after every call.** The rule is: once a filesystem call of the terminal gate returns, the
deadline is observed before the gate makes another call or records a seal; on expiry the gate returns the typed
`Timeout` with `last_completed = Some(Evidence)`, the partial kept, and the entry not recorded as sealed. The table
lists every call the gate makes, in order, with the observation that follows it. A call inside `fs_custody` or
`custody_mounts` (out of scope) is one primitive here, so the rule applies at its boundary: the census, a reopen
(`openat` + `fs_custody`'s own `fstat`), and a mode primitive (`fchmod` + its confirming `fstat`, with C1's own seam
between them).

| # | Call site | Call (system calls) | The observation that follows it | Fix 2 | Regression |
|---|---|---|---|---|---|
| 1 | `terminal_gate` | `refuse_mounts_within`: the mount census (`/proc/self/mountinfo` on Linux, `getmntinfo` on macOS) | the first entry's `payload-terminal-entry`, or with nothing created the first `objects-chain` | existing | — |
| 2 | `seal_file` | `recheck_created`: `fstatat(AT_SYMLINK_NOFOLLOW)` by name | `payload-terminal-statted` | **new** (W1) | a slow file lookup |
| 3 | `seal_file` | `open_regular_file`: `openat(O_NOFOLLOW)` + `fstat` | `payload-terminal-reopened` | **new** (W2) | a slow lookup |
| 4 | `seal_file` | the descriptor's opening `fstat` | `stream_digest`'s `before_deadline` before its first read | existing (phase G) | a slow opening fstat |
| 5 | `seal_file` → `stream_digest` | each `read` (at most length + 1 bytes) | the next read's `before_deadline`; after the last read, `payload-terminal-digested` | last: **new** (W3) | a slow digest |
| 6 | `seal_file` | the closing `fstat` | `payload-terminal-seal` | existing | a slow closing fstat |
| 7 | `seal_file` | `set_exact_file_mode`: `fchmod` + `fstat` | `payload-terminal-mode-set` | **new** (W4) | a slow file seal |
| 8 | `seal_file` → `confirm_sealed` | phase P's confirming `fstat` | `payload-terminal-confirmed`, before the record | **new** (W5) | a slow file confirmation |
| 9 | `verify_symlink` | `recheck_created`: `fstatat` | `payload-terminal-statted` | **new** (W7) | a slow link lookup |
| 10 | `verify_symlink` | `read_child_symlink`: `readlinkat` | `payload-terminal-link-read` | **new** (W8) | a slow readlinkat |
| 11 | `verify_symlink` | the closing `recheck_created`: `fstatat` | the next entry's `payload-terminal-entry` (a sibling's, a popped directory's, the next tree's, a slot's), or the first `objects-chain` | existing | a slow closing link stat |
| 12 | `observe_directory` | `recheck_directory`: `fstatat` | `payload-terminal-seal` | existing | a slow directory lookup |
| 13 | `seal_directory` | `PinnedDirectoryV1::set_exact_mode`: `fchmod` + `fstat` | `payload-terminal-mode-set` | **new** (W6) | a slow directory seal |
| 14 | `seal_directory` → `confirm_sealed` | `current_metadata`: `fstat` | `payload-terminal-confirmed`, before the record | **new** (W5, the same site as #8) | a slow directory confirmation |
| 15 | `output_chains` (phase G, shared) | each object directory's `recheck_entry` and each copied file's `fstatat` | the next `objects-chain`, else the first `payload-metadata-chain` | existing: one call per guard | guard sequence; halting rows |
| 16 | `metadata_chains` (C2a, shared) | each directory's `recheck_entry` and each file's `recheck_regular_entry` (`fstatat`) | the next `payload-metadata-chain`, else `objects-source` or the first `payload-chain` | existing: one call per guard | guard sequence |
| 17 | `terminal_source_files_unchanged` (C3 copy of phase G's `source_files_unchanged`) | an admitted file by name: `fstatat` | `payload-source-statted` | **new** (C1); the shared sweep made this call and #18 behind one guard | the Captured halting row |
| 18 | `terminal_source_files_unchanged` | the admitted file's retained descriptor: `fstat` | the next `objects-source`, else the first `payload-chain` | existing label | guard sequence |
| 19 | `terminal_retained_chains` (C3 copy of C2a's `retained_chains`) | a staging chain's first edge, the work directory beneath the root (`fstatat`) | `payload-chain-step` | **new** (C2); the shared sweep made a whole chain behind one guard | guard sequence; halting row |
| 20 | `terminal_retained_chains` | the plain directory's edge, then each staging component's (`fstatat`) | the next component's `payload-chain-step` (**new**, C3), else the next chain's or HOME's `payload-chain` | **new** | guard sequence |
| 21 | `terminal_retained_chains` | HOME, XDG, each bootstrap directory (`fstatat`) | the next `payload-chain`, else `payload-root` | existing: one call per guard | guard sequence; halting rows |
| 22 | `terminal_gate` | `pinned_root_unchanged`: `lstat` of the root's canonical path | none: it is the last call, and the result is built infallibly (§5.6) | — | — |

The copies in #17–#21 make the same calls in the same order with the same refusals as the shared sweeps (the staging
chains still recheck the work and plain edges for each key, as C2a's does); the step gates keep the shared sweeps, so
C2a's and C2b's code and guard sequences are byte-identical. The copies exist because the shared sweeps live in
`custody_restore_git.rs` and `custody_restore.rs` (outside this repair's files) and change C2b's guards if changed;
phase G's own final gates (`final_source_files_unchanged`, `final_sweep_chains`) already split the same calls one per
guard, but are private to `custody_restore_git.rs`. Reusing them is a visibility change for the controller to choose
(§5.13).

**Regressions.** The deadline control's slow rows (13, run first in the control, so a late call is reported before the
guard sequence): each spends the deadline at the seam immediately before one call (or, for the digest and a mode
primitive, immediately after it returns), and requires `Timeout`, `Evidence`, exactly the earlier entries sealed, the
named next call's seam absent, and NO terminal event of any kind after the spend; where a mode primitive ran, the entry
has its final mode and is not in `sealed`. The new hook point `TerminalDescriptor` announces each call through an
entry's own descriptor, as `TerminalLookup` announces each by-name call. The finding's rows: (a) **a slow file lookup**
(`repository/a`'s first lookup; next: its second `TerminalLookup`), (b) **a slow link lookup** (`repository/l`'s first;
next: its second `TerminalLookup`), (c) **a slow file seal** and **a slow directory seal** (at `TerminalModeSet`; next:
the confirming `fstat`'s `TerminalDescriptor`, and the entry not in `sealed`); and the strengthened **a slow lookup**
(before `repository/d/x`'s reopen; next: its opening `fstat`'s `TerminalDescriptor`). Halting rows add the first
`payload-chain-step` and, on a Captured database, `payload-source-statted`. The clean-run guard sequence (computed, one
guard per call) pins every chain guard in place.

**RED** (`fix2/01`, `fix2/02`). The new rows use the new `#[cfg(test)]` hook point, so they cannot compile against
`0dfbcc06`'s bytes. The RED production file is `0dfbcc06`'s `custody_restore_payload.rs` plus only the seam: 28 added
lines (the `TerminalDescriptor` variant, the `descriptor` announcer and its six call sites, at the fix's positions), no
line removed and no guard; `fix2/01-red-seams-only-vs-0dfbcc06.diff` is its whole difference. Against it, the final
tests fail two controls: the deadline control, on **nine** slow rows, each with its next call's seam present (a slow
file lookup, a slow lookup, a slow digest, a slow file seal, a slow file confirmation, a slow link lookup, a slow
readlinkat, a slow directory seal, a slow directory confirmation); and the structural control (the gate calls
`source_files_unchanged` itself). The four rows that pass there cover observations that already existed (#4, #6, #11,
#12). The fixed file was restored and its SHA-256 checked.

**Mutations** (`fix2/04`, root): each new observation deleted alone, plus the gate calling either shared sweep again.
All 13 turn controls red:

| Row | Deleted | Red (beyond the structural control, red in every row) |
|---|---|---|
| W1 | a file's `payload-terminal-statted` | a slow file lookup |
| W2 | `payload-terminal-reopened` | a slow lookup |
| W3 | `payload-terminal-digested` | a slow digest |
| W4 | a file's `payload-terminal-mode-set` | a slow file seal |
| W5 | `payload-terminal-confirmed` | a slow file confirmation, a slow directory confirmation |
| W6 | a directory's `payload-terminal-mode-set` | a slow directory seal |
| W7 | a link's `payload-terminal-statted` | a slow link lookup |
| W8 | `payload-terminal-link-read` | a slow readlinkat |
| C1 | `payload-source-statted` | the Captured halting row (INVALID: the guard is never reached) |
| C2, C3 | a staging chain's `payload-chain-step` (its plain edge; a component edge) | the guard sequence |
| G1 | the gate calls `source_files_unchanged` | the Captured halting row |
| G2 | the gate calls `retained_chains` | the guard sequence |

Each walk row is caught by its own slow row alone. The chain rows have no call seam (the shared `recheck_entry` has
none), so their regression is the clean run's observed guard sequence and the halting rows.

**Gates on the final bytes.** Same lane as §3.7 (foreground, `CARGO_INCREMENTAL=0`, target in `/tmp`). This lane has no
route to crates.io (the egress proxy refuses it), so every cargo command ran with `CARGO_HOME=/cargo` (the container's
read-only cargo cache) and `--offline --locked`.
- The 49 payload controls: `49 passed; 0 failed` as root (53 s) and as `nobody` (52 s); the scratch `TMPDIR` was empty
  afterwards (`fix2/03`).
- `cargo fmt --all -- --check` clean; `cargo clippy --workspace --all-targets --all-features -- -D warnings` clean (0
  warnings; all 17 workspace crates checked, the first clippy run in that target directory); `git diff --check` and
  `git diff --cached --check` clean; `cargo run -p a2a-bridge -- validate --repo-hygiene` → `repository hygiene
  validated`.
- `cargo test -p bridge-core`: **1697 passed, 0 failed**: lib **1552** in the six partitions of §3.7 (66, 41, 80, 145,
  138, 1082; proven disjoint and exact against `--list`), integration 135 (14 binaries), doc-tests 10. The rest-of-lib
  log again holds the nested child's extra `1 passed` line, excluded.
- Process note: the mutation runner restores the source but not the build, so a first partitioned run used the last
  mutant's binary (G2) and failed exactly G2's assertions in the rest of `git_tests`. Its logs were discarded, the binary
  was rebuilt from the restored, hash-checked source, and all six partitions were run again (`fix2/07-binary.txt`
  records the binary's and the sources' SHA-256).
- Not re-run: the M01–M47 and F1–F5 matrices (fix 2 adds guards and the chain copies, and removes nothing they mutate)
  and the descriptor measurement (no pin or descriptor changes).

**Finding 2: the receipt inventory.** §3.6 now gives the real paths. `fix2/12-check_receipt_manifest.py`, a one-shot
script, reads §3.6's table from this file, expands each listed name's braces and wildcards beneath the evidence
directory, and requires every one to name at least one existing file; it then runs `sha256sum -c` on
`fix2/source-sha256.txt` against the working tree and checks `fix1/source-sha256.txt` against commit `0dfbcc06`'s blobs.
Its output is `fix2/12-receipt-manifest.log`.

## C3 4. RED evidence

### 4.1 Structural RED

The production section was drafted before the controls, so structural RED was taken by compiling the finished controls
with the C3 section (everything from its header) removed: `cargo test -p bridge-core --lib --no-run` failed with 15
errors, all unresolved C3 symbols (`RestoredEntryV1`, `RestoredPayloadPlaneV1`, `SealedEntryV1`,
`restore_payload_plane_v1`) and their derived `E0282`s. The source was then restored and its hash checked.

### 4.2 Behavioural RED: the single-guard mutation matrix

`07-run_mutations.py` applies each row as exact, unique replacements to `custody_restore_payload.rs`, rebuilds, runs all
C3 controls as root and as `nobody`, records the failing controls, restores the file and checks its SHA-256. **47 rows:
45 turned controls red; 2 survived (below).** Every row turned the same controls red as root and as `nobody` except M16
and M24, where `nobody` adds one (by design: those are the guards a root run hides). The full row → control table is
`07-summary.md`; condensed:

| Rows | Guard removed | Controls turned red |
|---|---|---|
| M01 | the terminal census | refused-gate (census count, mount row), structural |
| M02–M04 | the worktree / index / evidence walk | seam, replacement, content, mode, refused-gate, deadline (+ modes, rich clone, gitfile; evidence and markers for M04) |
| M05–M08 | the object-store, metadata, admitted-file, staging/HOME/XDG/bootstrap chains after the walk | refused-gate (+ deadline sequence, structural) |
| M09, M10 | the root's identity last; its guard | refused-gate; deadline (+ structural) |
| M11–M14 | the gate's, each entry's, a file seal's, a directory seal's guard | deadline (+ structural) |
| M15, M17, M47 | a directory's by-name observation; the reopened descriptor's identity; a link at the reopen typed | replacement |
| M16 | a file's by-name stat | structural as root; replacement as `nobody` (the `0000` row: the reopen alone gives an untyped `EACCES`) |
| M18, M19 | the content comparison; the closing `fstat` | content |
| M20, M21, M23, M35 | a file's or a directory's `fchmod`; a file sealed before its digest; the gate not run | nearly every control |
| M22 | phase P's confirming `fstat` | mode |
| M24 | post-order (a directory sealed before its children) | seam, replacement, content, mode, refused-gate, deadline; **+ the modes control as `nobody`** (a lookup beneath a sealed `0000` directory) |
| M25, M27 | a link's by-name stat; its closing stat | replacement |
| M29–M34 | a slot's / `evidence/`'s seal; `0700` for them; D2 file/directory and D4 index final modes taken from the frame | modes, evidence, markers, rich clone, gitfile, mode, refused-gate, deadline (row-dependent) |
| M36–M39 | `last_completed`; `began`; `sealed` kept in the partial; each seal recorded | content, mode, refused-gate, replacement, deadline |
| M40–M46 | the inventory's mode written, frame mode, childless list, root `.git`, external references, `evidence/` pin, index rows | modes, rich clone, gitfile, evidence (row-dependent) |

Survivors, each predicted and kept deliberately:
- **M26** (the symlink's target comparison removed): every replacement of a link is a new inode, which the closing stat
  refuses; the comparison is defence in depth for the recorded target and cannot be isolated without inode reuse.
- **M28** (every recorded entry reached): unreachable on a created tree whose every parent is a recorded directory (C2b
  guarantees it); defence in depth.

M25 survived its first run (chunk e) and M16 was then structural-only; the two rows added to the replacement control
(a file made `0000`, a link replaced by a regular file) were written for them, and chunk j re-ran M16, M25, M26 and M28
on those bytes. Rows M01–M47 in chunks a–i ran on test bytes without those two rows (later edits only add assertions and
the e2e control's baseline reading); the pre-fix production bytes differ from the matrix's by one comment line in
`restored`.

### 4.3 Review fix 1: RED for the pre-order defect, and the fixed lines' mutations

**Behavioural RED.** The fixed tests were run against the committed (pre-fix) production file, then the fixed file was
restored and its SHA-256 checked (`fix1/03`): four controls fail, the seam (`evidence/bridge_evidence` sealed at event
87, beneath `evidence/`, which was observed at event 61), the replacement control, the deadline control (the guard
sequence) and the structural control (`seal_tree`'s observation is not on the pop). Because the replacement control's first failing row is an existing row whose expected seal count
moved (`a directory swapped`), the three new rows were then run alone against the pre-fix file through a temporary,
unstaged row filter (`fix1/04`, `fix1/06`): **each of the three succeeds on the pre-fix code** (`…: must refuse`):
`repository/d`, the slot, and `evidence/` renamed away with a decoy at their name while a file beneath is observed. On the
fixed file the same rows refuse `IdentityChanged` (`fix1/05`). This is the review's reproducer, executable.

**Mutations** (`fix1/07`, root, all five red): F1 a worktree directory's post-order observation removed; F2 a slot's;
F3 `evidence/`'s; F4 a slot observed before its tree; F5 `evidence/` observed before its slots. Each turns the seam,
replacement, deadline and structural controls red. The pre-fix matrix's M15 (a directory's observation) and M24
(post-order seal) concern the same lines and were not re-run; the M01–M47 rows outside `seal_tree` and `seal_evidence`
apply to code that the fix does not change.

## C3 5. Open items: interpretations for the reviewer and controller

1. **Entry points.** `restore_payload_plane_v1` is C2b's `materialize_payload_plane_v1` followed by the terminal gate,
   as the C2b ruling says; both stay.
2. **"A mode changed between `fchmod` and the confirming `fstat` (test seam)."** C1's seam (`ModeSet`) is private to
   `fs_custody`, and this increment may not change `fs_custody`. Phase P therefore confirms again through the same
   descriptor (its own `fstat`, which is also what the inventory's mode written rests on), and its seam
   (`TerminalModeSet`) sits between the primitive's return and that `fstat`. The primitive's own window is C1's
   `mode_03`. If the controller wants C1's seam driven through phase P, that needs a test-only widening in `fs_custody`
   (a contract point, not done).
3. **Checks beyond the spec's text, all fail-closed.** A file's descriptor `fstat` must not change while it is read (a
   write during the digest); a symlink is bracketed by a closing no-follow stat after `readlinkat` (as C1's primitive
   is); every recorded entry must be reached by the walk. The first two have rows; the third is survivor M28.
4. **The index is sealed too**: `set_exact_file_mode(0600)` changes nothing but keeps one path (and one digest) for every
   file.
5. **Step 3's scope** is C2a's gate scope (§C2a 5.3): the object store, metadata, admitted bootstrap files, and the
   staging, HOME, XDG and bootstrap chains, all through phase V/G pins that phase P never seals (the seam control proves
   no retained pin lies beneath a sealed entry).
6. **Deadline placement** (rewritten by review fix 2; the round-2 review showed the earlier placement let a file's or
   link's later calls and a seal's confirmation follow a late call). Guards before the census, before every entry's
   observation, immediately before every seal, and after EVERY filesystem call before the next call or the record of a
   seal (§3.8's table), with one guard per call in the chains. The one call nothing follows is the root's identity,
   the gate's last: if it returns after the deadline, the gate still succeeds, because nothing is done after it but
   building the result, as at phase G's final gates (whose `final_sweep_chains` also ends with it). Making a late last
   call a `Timeout` would refuse a fully verified and sealed restore; that is the controller's call, not made here.
7. **`payload_materialization_began`** on a terminal-gate failure is derived from what materialization recorded (on its
   success every create is recorded): false when every class was Empty.
8. **A refused terminal gate leaves a partially sealed destination** (`sealed` lists every entry whose seal was confirmed
   with the deadline unspent, possibly a `0000` directory). Since review fix 2, an entry whose mode primitive or
   confirming `fstat` returned after the deadline is not recorded although it carries its final mode, as an entry whose
   mode was changed after its `fchmod` was already not recorded (§5.2); `sealed` is what the gate confirmed in time, and
   at most one entry (the one in flight) can carry its final mode outside it. Nothing is unsealed or removed, per the
   spec. A second invocation refuses at its first create, as in C2b; C3 adds no new row for it.
9. **Inventory shape.** One row type (`RestoredEntryV1`) for the worktree, the index and evidence; evidence per slot
   with the slot's own mode and identity; `evidence/` as `(mode, dev, ino)`; each inventory carries its class state(s).
   The mode written is each row's final mode, which the gate confirmed by `fstat` before recording the seal.
10. **Pins** move out of `PayloadCreatedV1` into `PayloadPinsV1`; the index tree never holds a directory, so it has no pins.
11. **`fs_custody`'s `#[allow(dead_code)] // … lands in C3` notes** on `set_exact_file_mode` and `set_exact_mode` remain:
    removing them is an `fs_custody` change, outside this increment.
12. **Post-order directory observation (review fix 1).** A directory is descended into through its retained pin (the
    created object, which the inventory and pins record) without a by-name call; its one by-name observation follows
    its sealed subtree and immediately precedes its seal, while its parent is still `0700`. A directory renamed or
    replaced at any time before that observation refuses there as `IdentityChanged`. One consequence on error typing,
    with no row (read from the code, not run): a directory with children merely made `0000` in place before the gate
    (not replaced) is now met first by a child's by-name stat beneath it, which a non-root owner gets as a typed `Io`
    refusal (`EACCES`) rather than the
    directory's own `IdentityChanged`; as root, the directory's observation refuses it as `IdentityChanged`. Both
    refuse, keep everything, and name `Evidence`.
13. **The terminal gate's chain copies (review fix 2).** Phase G's `source_files_unchanged` makes an admitted file's
    two calls behind one guard and C2a's `retained_chains` a whole staging chain, so the gate runs C3-local copies with
    one guard per call (§3.8 #17–#21), leaving the step gates' sweeps and C2b byte-identical. The copies keep C2a's
    edges and order (each staging key's chain from the root, so the work and plain edges are rechecked per key). The
    alternative is phase G's final-gate helpers (`final_source_files_unchanged`, `final_sweep_chains`), which already
    split these calls one per guard (and check each staging edge once) but are private to `custody_restore_git.rs`;
    using them is a visibility change there and a different guard vocabulary (`final-source`, `chain-step`, `root`).
    The duplication is the cost: a later change to the shared sweeps must be mirrored, and the structural control (§2.4)
    pins only that the gate calls the copies and that each copy's call has its guard.

## C3 6. Risks

- **The macOS native non-root run has not happened.** Facts it decides: `File::open` of a directory to hold it, and
  reading a sealed `0000`/`0200` file through a descriptor opened before its seal, on APFS; `PermissionDenied` for a
  lookup beneath a sealed `0000` directory; `/dev/fd` listing; the swap and rename rows; the closing-`fstat` row (a write
  is expected to move APFS's nanosecond timestamps; the row sleeps 50 ms first); the planted-marker fixture with the
  macOS `/bin/sh`; `UnsealV1`'s cleanup (a failure would show as `TMPDIR` leftovers, not test failures). If any of these
  fails, the right response is to report it, not to weaken the control.
- **Descriptor bound** (§3.4): a real worktree with more than about 180 directories would hit macOS's default soft
  limit in the production binary; 2B3d must decide.
- **Residual boundaries** (unchanged from 2B3a/2B3b): a change to an entry or its name after its own terminal check is
  outside the gate (since fix 1, a directory's check follows its whole subtree, so this is only the window after its
  own observation); a same-owner process can change a mode back and rewrite after sealing.
- **Pre-existing, outside scope:** C2a §3.3's load-sensitive 2B3b deadline controls (not reproduced here) and C2a §6's
  `RouteV1` non-root leftovers (no payload control uses the recording route).
- **Review fix 2's boundaries.** A call cannot be preempted, so a call that hangs is not bounded by the deadline; the
  gate only refuses to make another call or record a seal after it returns late. The C2a/C2b step gates still check an
  admitted file's two calls, and a staging chain, behind one guard each (out of scope: C2b is approved). The chain
  copies (§5.13) can drift from the shared sweeps, and their guards' regression is the clean run's guard sequence and
  the halting rows, not a per-call seam.
- **Wall time.** The 12 C3 controls add about 30 s to the parallel payload run; fix 2 adds about 13 s more (the 49
  controls took 39 s at fix 1 and 53 s at fix 2), mostly its Captured halting row, which spends a 10 s deadline alone.

## C3 7. Files

Changed and staged, and nothing else:
- `crates/bridge-core/src/custody_restore_git.rs` — visibility only (§2.2)
- `crates/bridge-core/src/custody_restore_payload.rs` — the C3 section, the error variant, the partial's `sealed`, the
  hook points and docs (§2.1)
- `crates/bridge-core/src/custody_restore_payload_tests.rs` — 12 controls, the C3 fixtures, and the C2b fixture edits
  (§2.4)
- `docs/superpowers/reviews/2026-10-14-adr0041-slice2b3c-implementation-handoff.md` — this section

There are no dependency, `Cargo.lock`, public-API, wire, runner, `fs_custody`, frame-codec or coverage changes, no new
`unsafe`, and no operator wiring. The commit message is in `.git/A2A_COMMIT_MSG`; the bridge commits. There is no push or
merge.

Review fix 2 (a follow-up commit on `0dfbcc06`) changes only `custody_restore_payload.rs` (the C3 section's guards and
chain copies, the `TerminalDescriptor` hook point, and the docs of the module and of `sealed`),
`custody_restore_payload_tests.rs` (the deadline and structural controls) and this file. `custody_restore_git.rs` is
unchanged from `0dfbcc06`.

## C3 8. Next

1. The controller runs `cargo test -p bridge-core` natively on macOS as a non-root user on this candidate, before the C3
   source review (§6 lists what it decides).
2. C3 source review: Sol/xhigh, cap two rounds; the spec's terminal-gate text is re-examined against code here (the
   spec ledger's round-3 disposition). §5 lists the interpretations to rule on, especially §5.2 (the confirming-`fstat`
   seam), §5.6 (deadline placement) and §5.12 (post-order observation, fix 1). Round 1's blocker (pre-order directory
   edges) is fixed with executable RED (§4.3). Round 2's two findings are fixed as one disclosed extension (§3.8): the
   controller rules on §5.6's last-call interpretation (the root's identity), §5.8's `sealed` semantics and §5.13's
   chain copies (versus phase G's final-gate helpers, a visibility change).
3. On approval, 2B3d: the `custody-restore.v1` record from `RestoredPayloadPlaneV1`'s inventories, the joint fixture,
   the negative fixtures, the cross-phase wrapper and composed failure type, and a decision on the descriptor bound
   (§3.4).
