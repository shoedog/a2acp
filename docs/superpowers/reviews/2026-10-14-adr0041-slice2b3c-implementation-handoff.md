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
