# ADR-0041 Slice 2B2b2a implementation handoff — the descriptor-relative no-follow walker

**Status:** implemented in the container lane over two bridge turns. **This revision is the repair turn's.**
- **First turn.** It implemented the slice. It stopped under §9 on 2B2a's A14 inventory, which pins all of
  `fs_custody.rs` (§0), and staged a change that failed that one test.
- **Verify and code review then rejected it**, raw result not in this clone:
  - the workspace suite was red on the stale A14 inventory;
  - "an unresolved symlink-target identity gap".
- **The repair turn fixed both:**
  - it applied the A14 amendment, which the repair turn's issue list required (§0);
  - it bracketed the symlink target read with a length binding and a re-stat (§0.1).
- **Now:**
  - every §5 criterion has a control, and all **48** walker controls pass;
  - every guard's mutation turned its control red: the matrix defines **36** rows and ran all 36 on the final bytes
    (snapshot `d9db3aab198c9b9e`), **36 FLIPPED, 0 NOT-FLIPPED, 0 INADMISSIBLE**, and the source was then proved
    equal to that snapshot;
  - **every workspace gate is green** (§5).
- **Left to CI and the controller:** `cargo deny`, the native ext4 lane, and the macOS lane (§7).

This handoff records evidence only; it claims no review approval.

**Task (authoritative):** `docs/superpowers/plans/2026-09-26-adr0041-slice2b2b2a-walker-task.md`, revision 4, SHA-256
`1184623a658a646574f7e86ae12a5cab979a711a2f8828b5b0075c74183bd9d1` at the base commit. The bridge copy
`.git/A2A_TASK.md` differs from it only in the pinned base, the gate-renamed headings, and the §13 controller notes.

**Clone:** `/Users/wesleyjinks/code/.a2a-implement/impl-7091-pt1wty0u`
**Branch:** `implement/impl-7091-pt1wty0u`
**Base HEAD:** `980eb52a96373fb396e42023e8757fbab0256f4d` (`main`, the merge of the spec in PR #116)

**Container lane:** Linux `7.0.14-orbstack` on **aarch64**, `/` and `/tmp` on overlayfs, running as uid 0, rustc and
cargo 1.94.0. Every cargo command in this document runs with `CARGO_HOME=/cargo CARGO_NET_OFFLINE=true
CARGO_TARGET_DIR=/tmp/target`, and every workspace run also unsets `HTTP_PROXY`, `HTTPS_PROXY`, `http_proxy`, and
`https_proxy`. No symlink is followed, every read is descriptor-relative, and no dependency was added.

---

## 0. The A14 inventory: stopped in the first turn, amended in the repair turn

**The conflict.** §2 authorizes new `unsafe` inside `fs_custody`'s boundary, and asks for a control that pins the added
sites. There are five new blocks in two functions:
- `list_child_names` has 4: `fdopendir`, `rewinddir`, the errno-cleared `readdir`, and the `d_name` borrow;
- `read_child_symlink` has 1: `readlinkat`.

The other two primitives add none. `child_metadata_no_follow` reuses the existing `stat_child_no_follow`, and
`current_metadata` uses `File::metadata`. No safe alternative exists for `readlinkat`:
- std has no descriptor-relative `readlink`;
- a `/proc/self/fd` path is path-addressed, which §9 forbids;
- `rustix` is only a transitive dependency, and making it direct would be a new dependency.

2B2a's control `custody_git_tests::a14_ast_inventory_keeps_new_unsafe_boundaries_exact` asserts that `fs_custody.rs`'s
whole inventory equals its base map plus 2B2a's two owned functions. So any new `fs_custody.rs` site fails it. It failed
with exactly the two new keys (`list_child_names: 4`, `read_child_symlink: 1`).

**First turn.** The fix is a pinned-map edit in `custody_git_tests.rs`, which §8 does not list. The turn stopped and
reported under §9, and left the file untouched. It saved the amendment as `.git/a2a-bridge/a14-amendment.patch` and
proved it in scratch: the patch was applied, the test passed, and the file was restored byte-exactly.

**Repair turn.** The verify step and the code review rejected the red suite. The repair turn's issue list requires the
suite to be green, so it applied that patch **verbatim** (`patch -p1`). It is the only change to
`custody_git_tests.rs`, and it is staged:

```diff
@@ -2223,6 +2223,10 @@
     let owned: BTreeMap<String, usize> = BTreeMap::from([
         ("create_new_child_directory".into(), 2),
         ("root_command".into(), 1),
+        // ADR-0041 2B2b2a: the walker's listing and `readlinkat` primitives, pinned by
+        // `custody_walk::tests::unsafe_inventory_pins_the_new_2b2b2a_sites_and_their_files`.
+        ("list_child_names".into(), 4),
+        ("read_child_symlink".into(), 1),
     ]);
```

A14 now passes, and this slice's own inventory control pins the same sites independently (§4, `unsafe_*`). No
`unsafe` was added in the repair turn: the symlink fix below reuses the existing primitives.

## 0.1 The symlink-target identity gap (repair turn)

**The gap.** In the first turn, a symlink was stated at listing and its target was then read with `readlinkat` by
name. Nothing tied the bytes read to the listed entry:
- a regular file has its opened-descriptor check (§4.2);
- a directory has its descent check (§4.5);
- a symlink had neither, and the final pass is stat-only and never re-reads a target.

So a symlink replaced for the read and restored before any later observation was emitted with the impostor's target.
On a coarse-timestamp filesystem, no check saw it.

**The fix.** `WalkV1::read_symlink` brackets the read. A symlink has no descriptor to pin: an `O_PATH` descriptor is
Linux-only and would need new `unsafe`.
1. **Length binding.** After `readlinkat`, the target's length must equal the listed `size`. A symlink's `st_size` is
   its target's length on Linux and macOS. An impostor target of another length is refused however coarse the clock
   is.
2. **Re-stat.** A no-follow re-stat after the read (the new `ObservationV1::TargetRead`) must equal the listing stat in
   every field. A replacement that is still in place after the read is refused at once, rather than later by the
   parent's `ChildIdentity` check or the final pass.

Either failure is the new `SourceDrift(SymlinkTarget)`. An entry absent at the re-stat is `Vanished`, and a failed
`readlinkat` still goes through `explain`.

**Controls.** Two new `control_06_*` controls and two matrix rows cover it:
- `control_06_a_symlink_swapped_only_while_its_target_is_read_refuses_on_a_coarse_timestamp_filesystem`. An impostor
  with a 2-byte target (the original's is 4 bytes) is in place only for the read, the original is restored before the
  re-stat, and timestamps read as zero. `M-link-length` (length check off) turns it into a **wrong success**: the frame
  carries the impostor's target.
- `control_06_a_symlink_replaced_before_its_target_is_read_refuses`. A same-length replacement is in place at the
  re-stat. `M-link-restat` (re-stat off) is **layered**: the parent's post-subtree check refuses `ChildIdentity`
  instead.

**The residual, stated.** A same-length impostor swapped in and back out within one timestamp granule leaves the length,
inode, and times equal at every observation. That is the §1.1 hostile same-user racer, out of scope by the owner's
ruling. On any filesystem whose timestamps separate the swap, the renames move the original's ctime, and the re-stat or
the final pass refuses. Closing it for a hostile racer would need a descriptor to the symlink itself: `O_PATH` on Linux,
and a different mechanism on macOS. That is a spec amendment, with new `unsafe`, and is not taken here.

## 1. Structural RED on the predecessor

Before any production code, a scratch test naming `crate::custody_walk::walk_tree_v1::<&mut Vec<u8>>` was appended to
`lib.rs` at the base commit. `cargo test --locked --offline -p bridge-core --lib custody_walk` failed to compile:

```text
error[E0433]: failed to resolve: could not find `custody_walk` in the crate root
  --> crates/bridge-core/src/lib.rs:77:24
77 |         let _ = crate::custody_walk::walk_tree_v1::<&mut Vec<u8>>;
error: could not compile `bridge-core` (lib test) due to 1 previous error
```

The scratch lines were removed, restoring `lib.rs` to the base bytes. Raw output:
`.git/a2a-bridge/mutation/structural-red.txt`.

## 2. Behavioral RED

**Order of work, stated plainly.** The four primitives and `custody_walk.rs` were written first, then
`custody_walk_tests.rs`, and the first test build compiled them together. So no control was observed red against a
predecessor before its guard existed. The behavioral RED per control comes from removing guards, which §6 allows:

- **`red-all`** (`.git/a2a-bridge/mutation/red-all.json`, raw `output-red-all.txt`) applies the 30 composable guard
  rows at once. **47 of 48 controls fail.** The one that passes is `control_09` (case), whose row `M-fold` cannot compose
  with `M-lossy`, since both rewrite the same line.
- **The per-row matrix (§6)** turns every control red in at least one row. `control_09` turns red under `M-fold` alone.
  The positive controls turn red too: `01` and `10` under `M-sort`, `02` under `M-nofollow`, `11` under
  `M-nofollow`, `M-fold`, and `M-rewind`, and the golden digest under `M-digest-ctime`.

One control was corrected after its first run, before the matrix. `control_06_a_transient_chmod_at_descent…` first
restored the mode at the `Opened` point, which is before the walker reads the opened descriptor's mode, so it did not
exercise the guard. It failed as a wrong success. The restore moved to the child's `Listed` point, which comes after
the descent check. No assertion was relaxed.

The repair turn wrote its two symlink controls after the fix, in the same edit, so their RED is also by guard removal:
`M-link-length` and `M-link-restat` (§0.1, §6).

**GREEN:** the walker suite passes **48/48** on the final bytes. Repeated 60 times with 16 test threads, it had 0
failures (`.git/a2a-bridge/gates/repeat-60.txt`).

## 3. What was built

### 3.1 `fs_custody.rs`: the four §2 primitives (`#[cfg(unix)]`, `pub(crate)`)

| §2 item | Implementation |
|---|---|
| `list_child_names(&mut u64, label) -> Vec<ChildBytesV1>` | `File::try_clone` (an `F_DUPFD_CLOEXEC` duplicate), then `fdopendir`, `rewinddir`, and `readdir` to the end. The existing `DirectoryStreamV1` `closedir`s it on drop. It skips `.` and `..`. Each name is charged before it is stored; at zero it refuses `EnumerationLimitExceeded` before storing. Linux and macOS only; any other unix refuses `Unsupported` |
| `child_metadata_no_follow(name, label) -> Option<ChildStatV1>` | the existing `stat_child_no_follow` (`fstatat(AT_SYMLINK_NOFOLLOW)`); `None` means the name is absent |
| `read_child_symlink(name, max_bytes, label) -> Vec<u8>` | `readlinkat` into `max_bytes + 1`; a result over `max_bytes` refuses `Unsupported` and is never truncated |
| `current_metadata(label) -> ChildStatV1` | `File::metadata` (`fstat`) of the retained descriptor |
| types | `ChildBytesV1` holds exact bytes, and its `Ord` is byte order. `ChildKindV1` is `Directory`, `Regular`, `Symlink`, or `Special(S_IFMT bits)`. `ChildStatV1` holds `kind`, `mode` (`& 0o7777`), `size`, `dev`, `ino`, `mtime_ns`, and `ctime_ns`, built by one `from_fields` for both the raw `fstatat` and the `fstat` paths |
| test seam | a `#[cfg(test)]` thread-local count of stored names (`listed_child_names_for_test`) |

`open_existing_child_directory` and `open_regular_file` are reused unchanged. `fs_custody.rs` gains 265 lines and no
other change.

### 3.2 `custody_walk.rs` (new, crate-private, `#[cfg(unix)]`)

It is declared in `lib.rs` as `#[cfg(unix)] #[allow(dead_code)] mod custody_walk;` with a one-line comment. Its imports
are `custody_frame`, `custody_inventory::CustodyReasonCodeV1`, the `fs_custody` primitives, `ring::digest`, and std. It
holds no `unsafe`.

- **API (§3).** It exposes `walk_tree_v1<W: Write>(root, &dyn WalkSelectionV1, entry_budget, CustodyFrameEncoderV1<W>)
  -> Result<WalkReceiptV1, CustodyWalkErrorV1>`. The other public items are:
  - `WalkSelectionV1::decide(&path, &stat) -> WalkDecisionV1`, where the decision is `Include`, `IncludeEntryOnly`,
    `Skip`, or `Park(WalkParkV1)`;
  - `WalkParkV1 { reason: CustodyReasonCodeV1, path }`;
  - `WalkReceiptV1 { summary, skipped_entries, inventory_sha256 }`.
- **Traversal (§4.1).** The traversal is iterative, with an explicit stack of directory frames: one retained
  descriptor and one sorted, decided child list per depth level. Frames share one path buffer, so an entry stores a
  name, not a full path. Each directory is:
  1. `fstat`ed;
  2. listed;
  3. sorted;
  4. stated and decided in that sorted order, where the first `Park` refuses at once;
  5. processed in that order, each subtree before its next sibling.
- **Entry kinds (§4.2).**
  - A **directory** is opened no-follow, and the descent checks run on the opened descriptor. It is emitted with the
    opened descriptor's mode, then entered. An `IncludeEntryOnly` directory is emitted with its listing mode and never
    opened.
  - A **regular file** is opened no-follow, and its opened `fstat` must equal the listing stat in every field. It is
    then streamed through the encoder at the listing size. After the encoder returns, successfully or not, a second
    `fstat` must equal the opened one; drift outranks a length refusal.
  - A **symlink** is read with `readlinkat(…, 4095)`. A target that is too long refuses as
    `Frame(InvalidSymlinkTarget)`. The read is bracketed (§0.1): the target's length must equal the listed `size`, and
    a no-follow re-stat after the read must equal the listing. Otherwise it refuses `SourceDrift(SymlinkTarget)`.
  - A **special file** refuses `UnsupportedEntry`.
- **Device (§4.3).** Every listed entry's `dev` must equal the root's, and this is checked before the selection
  decides, so it covers skipped entries too (§8). A regular file's opened `dev` is checked again by the full-stat
  equality of rule 2.
- **Post-subtree checks (§4.4).** A directory is re-listed through a new duplicate, charged to a separate counter
  capped at the budget. The checks run in this order:
  1. the sorted name set must be equal;
  2. each kept child's `(kind, dev, ino)` must be unchanged;
  3. the directory's `(dev, ino, mtime, ctime)` must be unchanged.
- **Descent (§4.5).** The mode is compared first, then `(kind, dev, ino, mtime, ctime)` (§8).
- **Final pass (§4.6).** `InventoryV1` folds each emitted entry's listing stat, and each skipped entry with tag 4. The
  encoding is exactly the §4.6 grammar. After the emitting pass, a second `WalkV1` in `Verify` mode re-walks the tree
  with no encoder: it reads no content, reads no symlink, and runs no re-listing. It recomputes the digest under a fresh
  counter capped at the budget and requires equality. Only then is `encoder.finish()` called.
- **Root (§4.7).** The root descriptor's `(dev, ino)` is observed at the start and again after the final pass, and the
  two must be equal.
- **Open failures.** When an open or `readlinkat` fails, the entry is re-stated. It is `SourceDrift` (`Vanished` or
  `Replaced`) when the entry is gone or differs from its listing, and `Io` otherwise.
- **Verification counters.** Exhausting the re-listing counter or the final-pass counter is `SourceDrift`, not
  `EntryLimit`, because those counters re-list what the emitting pass already listed within the same budget (§8).
- **Errors.** Exactly the §3 variants. `SourceDrift` carries `path: Option<…>`, where `None` means the root, plus a
  typed `WalkDriftV1` naming the check that fired. `Io` carries the `io::ErrorKind`.
- **Test seam** (`#[cfg(test)] mod seam`). It provides:
  - a stat hook applied to every observation, keyed by pass, observation point, and path;
  - a point hook between steps: `Listed`, `BeforeOpen` (also before a symlink's target read), `Opened`, `TargetRead`,
    `Reading` (after the first content read), `Subtree`, and `Verifying`;
  - `readdir` reversal;
  - an `openat` log;
  - a peak count of live listed names.

  Outside tests, observation is the identity function and the hooks are compiled out.

## 4. The §5 controls

All are in `custody_walk::tests` (`crates/bridge-core/src/custody_walk_tests.rs`).
- **Fixtures.** Every control walks a real temporary tree. A fixture ages each directory and regular file to a fixed
  mtime in 2001 before pinning, so any later change moves it however coarse the kernel clock is.
- **Racing hooks.** Controls that race the walker change the tree from a point hook. Where a later layer would also see
  the change, the control uses the stat seam to stand in for a **coarse-timestamp filesystem**, which reads mtime and
  ctime as zero, and puts the tree back before the later layer looks. Removing the guard is then a wrong success
  (§6).

| §5 | Control(s) | What it proves |
|---|---|---|
| 1 | `control_01_the_frame_holds_exactly_the_tree_in_canonical_order` | The fixture holds `B`, `a`, `a/b`, `a/b/c`, `a/b.x` (a symlink), `a.b`, `z` (a symlink), `é`, `é/ü`, and `日本`, with modes 0o755, 0o700, 0o644, 0o600, and 0o444. It is walked in `readdir` order and with the order reversed. Each frame, decoded by the 2B2b1 decoder, equals the hand-written expected list, and that list is checked against an independent component-wise comparator. The receipt equals the decoded frame |
| 2 | `control_02_every_entry_kind_is_captured_and_nothing_outside_the_root_is_opened` | captures an empty directory, a zero-length file, a 200,003-byte file (four 64 KiB chunks), an executable (0o755), and three symlinks: to an absolute path in a sibling `aside` directory, to `..`, and a dangling one, each exactly. The `openat` log is exactly `big`, `empty`, `exec`, and `zero` in the emitting pass, and `empty` in the final pass. No symlink is opened |
| 3 | `control_03_a_fifo_and_a_socket_refuse_as_unsupported_entries`; `control_03_a_setuid_file_and_a_sticky_directory_refuse_through_the_encoder` | A FIFO refuses `UnsupportedEntry { Special(0o010000) }` and a Unix socket `{ Special(0o140000) }`. A 0o4755 file and a 0o1777 directory each refuse `Frame(UnsupportedMode)` |
| 4 | `control_04_skip_omits_and_counts_and_never_descends`, `control_04_include_entry_only_emits_a_childless_directory_it_never_opens`, `control_04_park_refuses_with_the_callers_reason` | `Skip` omits a directory and a file, counts 2, and opens neither; a skipped socket does not refuse. A `.git` directory under `IncludeEntryOnly` is emitted as 0o750 with no children, and neither pass opens it. `Park` returns the caller's `WalkParkV1` |
| 5 | `control_05_a_directory_…`, `control_05_a_regular_file_…`, `control_05_a_symlink_on_another_device_refuses_mount_boundary`, `control_05_every_entry_is_checked_even_a_skipped_one`, `control_05_a_file_whose_opened_device_differs_from_its_listing_refuses` | The seam reports another `dev` for one entry at every observation of it, as a real mount point would. On a subdirectory, a regular file, a symlink, and a skipped subdirectory, each refuses `MountBoundary`. An `Opened`-only `dev` refuses `SourceDrift(OpenedIdentity)` |
| 6 | 21 `control_06_*` controls | the lineage round 2 #1 regression (an earlier sibling rewritten in place during a later read → `FinalVerification`); a chmod between listing and descent → `DescentMode`, and its transient coarse-timestamp twin; a file created after its directory's listing (the review's #5 regression, same inode) → `ChildNames`, and its transient coarse twin; a file deleted after listing (coarse) → `Vanished`, between stat and open → `Vanished`, and after emission → `ChildNames`; a directory replaced before descent → `DescentIdentity`, or replaced by a file → `Replaced`; a file grown while it is read → `DuringRead`; a file replaced before open → `OpenedIdentity`, swapped only while opened (coarse) → `OpenedIdentity`, or replaced by a symlink → `Replaced`; a symlink replaced by a same-length one before its target read → `SymlinkTarget` (re-stat), and an impostor symlink swapped in only for the read (coarse) → `SymlinkTarget` (length); a kept child replaced during the subtree and restored (coarse) → `ChildIdentity`; a transient entry → `DirectoryMetadata`; a changed root inode → `RootIdentity`; a re-listing and a final pass that outgrow an exactly spent budget → `ChildNames` and `FinalVerification` |
| 7 | `control_07_the_global_entry_budget_admits_max_and_refuses_max_plus_one`, `control_07_the_peak_live_name_count_is_within_two_budgets`, `control_07_a_wide_child_refuses_before_its_list_reaches_full_size`, `control_07_frame_budget_refusals_propagate` | Five names in three lists of at most three: a budget of 5 succeeds, and 4 and 0 refuse `EntryLimit`. With a budget of 6 over one directory of five files, the peak live name count is exactly 11 = 2·6 − 1, and a wider tree stays within two budgets. With a wide parent and a wide child (20 files each) and a budget of 25, the name-allocation seam counts exactly 25 stored names (the parent's 21 and the child's 4, never all 20 of the child's) before `EntryLimit`. A 200-byte frame budget refuses a 1,000-byte file as `Frame(ByteBudget)` |
| 8 | `control_08_a_non_utf8_name_round_trips` (`cfg(target_os = "linux")`) | `d\xff/\xfe\x80` round-trips byte-exact. On macOS it is a named exclusion, since APFS refuses such names |
| 9 | `control_09_the_on_disk_spelling_is_emitted_and_never_folded` | `CaSe`, `CaSe/InNeR`, and `MiXeD` are emitted exactly as spelled. The control records whether another spelling resolves; on macOS that is the controller lane's case-insensitive run (§7) |
| 10 | `control_10_walks_are_deterministic_and_independent_of_readdir_order`, `control_10_the_first_park_is_independent_of_readdir_order` | Two walks, and a walk with reversed listings, give identical receipts and frame bytes (with one skip). With `p1`, `p2`, and `p3` all parkable, the first `Park` is `p1` in both orders |
| 11 | `control_11_the_receipt_summary_equals_the_decoded_frame` | For an empty tree and for the 10-entry tree, `entries`, `content_bytes`, `frame_bytes`, and `frame_sha256` all equal the decoded sink's. The empty tree's inventory digest is the SHA-256 of the domain prefix alone |
| §2 | `primitive_list_child_names_returns_exact_bytes_and_charges_one_shared_budget`, `primitive_list_child_names_rewinds_the_shared_directory_offset`, `primitive_child_metadata_is_no_follow_with_the_full_mode`, `primitive_read_child_symlink_never_truncates_or_follows` | `list_child_names`: one budget is charged across two directories, and at zero it refuses with no name stored (by the allocation seam), while an empty directory still lists; a second listing through the same pin is equal. `child_metadata_no_follow`: a symlink is reported as the entry itself, with `size` equal to the target's length; the full mode (0o4755, 0o1777) is kept; an absent name is `None`; and the `fstat` and `fstatat` observations of one object are equal. `read_child_symlink`: exact at `max`, refuses at `max − 1`, and refuses on a regular file |
| §2 | `unsafe_inventory_pins_the_new_2b2b2a_sites_and_their_files` | In A14's style, a `syn` visitor over the committed sources checks three inventories: `fs_custody.rs` is exactly A14's base map, plus 2B2a's two owned functions, plus `list_child_names: 4` and `read_child_symlink: 1`; `custody_walk.rs` has none; and `custody_walk_tests.rs` has one site, the FIFO's `mkfifo` |
| §4.6 | `inventory_digest_of_a_small_tree_is_pinned` | the golden inventory digest (§4.1) |

### 4.1 The golden inventory digest's independent source

`.git/a2a-bridge/mutation/inventory_golden.py` builds the digest input from the §4.6 byte encoding with Python's `struct`
and `hashlib`, without reference to the Rust code.
- **Tree.** `d` (0o755), `d/f` (0o644, `hi`), `d/l` → `f`, and `s` (0o600, `xyz`, skipped).
- **Normalized fields.** The control's stat seam sets them so the digest is reproducible on any filesystem:
  - `dev` 7;
  - inodes 11 to 14 by path, and 1 for the root;
  - mtime `1_700_000_000_123_456_789` ns;
  - ctime `-1_500_000_000` ns, which pins the floor split to `(-2 s, 500_000_000 ns)`;
  - directory size 0;
  - symlink mode 0o777.
- **Output.** 250 input bytes, SHA-256 `abfc2a54da1adedc2870fe53cc72a09f5ee54a3b87742fb3e0362deec56e1a67`, which is the
  test's literal.

## 5. Verification totals

**When and how the gates ran.** The repair turn ran every gate on the final bytes (snapshot `d9db3aab198c9b9e`), after
its matrix round and `red-all`. The environment is the one in the header. Raw output is in `.git/a2a-bridge/gates/`.

**Counting.** Totals count every `test result` line, as the 2B2b1 handoff did. That includes one self-re-executed
1-test run inside the `bridge-core` lib binary, which 2B2b1 counted as its fifteenth integration binary.

| Gate | Exit | Totals |
|---|---|---|
| `cargo test --locked --offline -p bridge-core --lib custody_walk` | 0 | **48 passed**, 0 failed, 0 ignored; 866 filtered out |
| `cargo test --locked --offline -p bridge-core --no-fail-fast` | 0 | **1,052 passed**, 0 failed, 0 ignored. The lib runs 914 tests (866 at 2B2b1 plus these 48), then the re-executed run adds 1, 14 integration binaries add 128, and doctests add 9 |
| `cargo test --locked --offline --workspace --all-targets --no-fail-fast` | 0 | 90 test binaries: **4,649 passed**, 0 failed, 13 ignored. 2B2b1's 4,601 plus 48 |
| `cargo test --locked --offline --workspace --no-fail-fast` | 0 | 90 test binaries and 16 doctest runs: **4,659 passed**, 0 failed, 13 ignored. 2B2b1's 4,611 plus 48 |
| `cargo clippy --locked --offline --workspace --all-targets -- -D warnings` | 0 | no warning or error |
| `cargo fmt --all -- --check` | 0 | no diff |
| `git diff --check` | 0 | no output; `git diff --cached --check` over the staged paths is in §9 |
| `cargo deny check` | — | **excluded**: not installed (`error: no such command: deny`, exit 101); CI runs it (§7) |
| `cargo run --locked --offline -p a2a-bridge -- validate --repo-hygiene` | 0 | `repository hygiene validated`; `tracked_artifacts: 41`, `validated_example_configs: 9` |

- **The first turn's gates** failed exactly A14 (§0) in every test gate. No other test failed in any gate run of
  either turn.
- **A known pre-existing flake.** One exploratory lib run in the first turn, before its gates, also failed 2B2a's
  `w2_stdout_target_cannot_escape_the_pinned_root` with `Spawn(… ExecutableFileBusy, "Text file busy")`. That is the
  pre-existing 2B2a fixture `ETXTBSY` race recorded in the 2B2 handoff §11.4 and the roadmap, at about 1 in 20 runs. It
  did not recur in three immediate reruns or in any gate run. The walker spawns no process.
- **The 13 ignored tests** are the pre-existing live and e2e tests outside `bridge-core`.

## 6. Mutation matrix

**Harness:** `.git/a2a-bridge/mutation/matrix.py` (untracked; it survives the container). It keeps:
- the log in `matrix.log`;
- one JSON record per row in `results.jsonl`, keyed by snapshot;
- raw cargo output in `output-<id>.txt`;
- the snapshot in `snapshot/`, with `manifest.json`;
- the table below in `table.md`, rendered from the records.

**Rules** (2B2 and 2B2b1 style):
- A row applies only if each of its targets occurs exactly once, applied in order, in the snapshot. `matrix.py check`
  reports 36 rows, 30 composable, and 0 problems.
- The repair turn added `custody_git_tests.rs` to the snapshot. No row mutates it, but the snapshot proof now covers
  every changed file.
- A `pending.json` marker is written and `fsync`ed before a mutation is applied. It is removed only after:
  1. the source is rewritten from the snapshot and `fsync`ed;
  2. its mtime is refreshed with `os.utime` (now);
  3. it re-hashes equal to the snapshot.

  Any later invocation that finds the marker restores first. SIGINT, SIGTERM, and SIGHUP restore before exiting.
- Each row runs `cargo test --locked --offline -p bridge-core --lib custody_walk --no-fail-fast` with a 240 s cap.
- **Foreground only.** Every call ran in the foreground inside `timeout 590`. No background job was started at any
  point in either turn.

**Verdict rule.** `FLIPPED` requires all of these:
- the crate compiled, with no timeout;
- every named test ran;
- every expected-red control failed;
- every named expected-green control passed.

Anything else is `INADMISSIBLE` or `NOT-FLIPPED`.

**Round 1** (first turn, 02:03:20–02:06:41Z; snapshot `be75aa735062fa7f`), 34 rows on the first turn's bytes:
- **First pass: 32 FLIPPED.** The other two rows were mislabeled, and the source did not change:
  - **`M-sort`: NOT-FLIPPED.** It named `control_11` as red, but that control stayed green: this lane's filesystem
    lists in creation order, and that fixture is created in sorted order. `control_01` and `control_10` still failed
    through their reversed-listing runs, which exist for exactly this. The row's red list dropped `11`.
  - **`M-fold`: INADMISSIBLE.** It named `control_11` as green, but that tree has an uppercase `B`, which the fold
    breaks. Its greens became `04-park` and `07-frame`, whose trees are lowercase.
- **Both rows were rerun:** FLIPPED, so the round ended at 34 FLIPPED.
- **`red-all`:** 45 of 46 fail.
- **Records:** remain in `results.jsonl` under that snapshot id.

**Round 2** (repair turn, 02:30:33–02:33:08Z) ran on the final bytes.
- **Rows.** It added `M-link-length` and `M-link-restat`.
- **Snapshot.** `matrix.py snapshot` took `d9db3aab198c9b9e`, whose SHA-256 prefixes are `custody_walk.rs` `41db10fd…`,
  `custody_walk_tests.rs` `db843a61…`, `fs_custody.rs` `3845f382…` (unchanged), `lib.rs` `844733bd…` (unchanged), and
  `custody_git_tests.rs` `2b9333b2…`.
- **Result.** One foreground call ran all 36 rows: **36 FLIPPED, 0 NOT-FLIPPED, 0 INADMISSIBLE, 0 not run**, each row
  on its first try.
- **Unchanged rows.** Every one of the 34 has the same failing set as in round 1, except `M-nofollow`, which now also
  fails the two new symlink controls.
- **`red-all`:** 47 of 48 fail (§2).

**Source equals snapshot after the matrix:** yes.
- **Harness:** every round 2 call ended with `VERIFY OK: 5 files equal their snapshot; no pending marker`, the last at
  02:33:08Z. `matrix.log` has 74 `APPLIED` and 74 `RESTORED` lines, 37 per round: in round 1, 34 rows, 2 reruns, and
  `red-all`; in round 2, 36 rows and `red-all`.
- **Independent check:** outside the harness, each file's SHA-256 equals the manifest, and no `pending.json` exists.
  The last restore stamped `custody_walk.rs` and `fs_custody.rs` at 02:33:08Z. No row mutates the other three files.
- **After the gates:** the check was repeated immediately before staging (§9). No snapshot file was edited after round
  2; only this handoff changed.

In the table, control aliases are the `T` map in `matrix.py`: `06-chmod-coarse` is the transient chmod control,
`p-list` the listing primitive, and so on.

| ID | § | Guard mutated | Named red | Verdict | Every failing control |
|---|---|---|---|---|---|
| M-sort | 4.1 | names sorted by byte order before stating and deciding | 01, 10-receipts | FLIPPED | 01, 02, 10-park, 10-receipts |
| M-decide-before-sort | 4.1,3 | decisions made only after sorting | 10-park | FLIPPED | 10-park |
| M-nofollow | 2.2,1.1 | no-follow stat and open (AT_SYMLINK_NOFOLLOW, O_NOFOLLOW) | 02, p-stat | FLIPPED | 01, 02, 06-file-to-link, 06-link-replaced, 06-link-swapped, 07-peak, 11, digest, p-stat |
| M-special | 4.2 | a kept FIFO, socket, or device refuses UnsupportedEntry | 03-special | FLIPPED | 03-special |
| M-dev-dir | 4.3 | device check: directories | 05-dir, 05-skipped | FLIPPED | 05-dir, 05-skipped |
| M-dev-file | 4.3 | device check: regular files | 05-file | FLIPPED | 05-file |
| M-dev-link | 4.3 | device check: symlinks | 05-link | FLIPPED | 05-link |
| M-dev-all | 4.3 | device check: every entry (for red-all) | 05-dir, 05-file, 05-link, 05-skipped | FLIPPED | 05-dir, 05-file, 05-link, 05-skipped |
| M-opened | 4.2 | opened descriptor equals the pre-open listing stat | 06-file-swapped | FLIPPED | 05-opened, 06-file-replaced, 06-file-swapped |
| M-during-read | 4.2 | no metadata change while the content is read | 06-size | FLIPPED | 06-size |
| M-link-length | 4.2 | a symlink target read must be as long as the listed size | 06-link-swapped | FLIPPED | 06-link-swapped |
| M-link-restat | 4.2 | a symlink re-stat after its target read equals the listing | 06-link-replaced | FLIPPED | 06-link-replaced |
| M-relist-names | 4.4 | post-subtree re-listing: exact same name set | 06-created-coarse | FLIPPED | 06-created, 06-created-coarse, 06-deleted-emitted |
| M-relist-child | 4.4 | post-subtree: each kept child keeps kind, dev, ino | 06-child | FLIPPED | 06-child |
| M-dir-metadata | 4.4 | post-subtree: the directory's own dev, ino, mtime, ctime | 06-dir-metadata | FLIPPED | 06-dir-metadata |
| M-relist-shared | 4.4 | re-listing charges its own counter, not the emit budget | 07-global, 07-peak | FLIPPED | 06-final-exhaust, 07-global, 07-peak |
| M-descent | 4.5 | descent identity (kind, dev, ino, mtime, ctime) | 06-dir-replaced | FLIPPED | 06-dir-replaced |
| M-descent-mode | 4.5 | descent mode comparison | 06-chmod-coarse, 06-chmod | FLIPPED | 06-chmod, 06-chmod-coarse |
| M-final | 4.6 | final verification pass reproduces the inventory digest | 06-lineage | FLIPPED | 06-lineage |
| M-digest-ctime | 4.6 | inventory encoding folds ctime nanoseconds | digest | FLIPPED | digest |
| M-root | 4.7 | root (dev, ino) unchanged at start and end | 06-root | FLIPPED | 06-root |
| M-budget-global | 2.1,4.1 | one global entry budget (mutated: a per-list budget) | 07-global, 07-wide | FLIPPED | 07-global, 07-wide |
| M-budget-charge | 2.1 | each name charged before it is stored; zero refuses | p-list, 07-global, 07-wide | FLIPPED | 07-global, 07-wide, p-list |
| M-rewind | 2.1 | the listing rewinds the shared directory offset | p-rewind, 01 | FLIPPED | 01, 02, 04-entry-only, 04-skip, 06-child, 06-dir-metadata, 06-final-exhaust, 06-lineage, 06-root, 07-global, 07-peak, 08, 09, 10-park, 10-receipts, 11, digest, p-list, p-rewind, unsafe |
| M-lossy | 2.1,5.8 | names are kept as exact bytes | 08 | FLIPPED | 08 |
| M-fold | 2.1,5.9 | names are never case-folded | 09 | FLIPPED | 01, 07-peak, 09, 10-receipts, 11 |
| M-symlink-max | 2.3 | a symlink target that does not fit refuses; never truncated | p-link | FLIPPED | p-link |
| M-mode-mask | 2.2,4.2 | mode is the full st_mode & 0o7777 | 03-mode, p-stat | FLIPPED | 03-mode, p-stat |
| M-entry-only | 3 | IncludeEntryOnly never opens or descends | 04-entry-only | FLIPPED | 04-entry-only |
| M-skip-count | 3 | skipped entries are counted in the receipt | 04-skip, 10-receipts, digest | FLIPPED | 04-skip, 10-receipts, digest |
| M-park | 3 | Park refuses the walk with the caller's reason | 04-park, 10-park | FLIPPED | 04-park, 10-park |
| M-vanished | 4.1 | a listed name absent at its stat is drift | 06-deleted | FLIPPED | 06-deleted |
| M-explain | 4.2,4.5 | an open or readlink refusal of a changed entry is drift | 06-file-to-link, 06-deleted-open, 06-dir-to-file | FLIPPED | 06-deleted-open, 06-dir-to-file, 06-file-to-link |
| M-relist-exhaust | 4.4 | re-listing counter exhaustion is the directory's drift | 06-relist-exhaust | FLIPPED | 06-relist-exhaust |
| M-final-exhaust | 4.6 | final-pass counter exhaustion is drift | 06-final-exhaust | FLIPPED | 06-final-exhaust |
| M-streamed | 5.7 | a frame refusal while streaming propagates | 07-frame | FLIPPED | 03-mode, 07-frame |

Every §6 minimum row is present:
- sort: `M-sort`;
- no-follow: `M-nofollow`;
- special: `M-special`;
- device ×3: `M-dev-dir`, `M-dev-file`, `M-dev-link`;
- pre-open versus opened: `M-opened`, and for symlinks `M-link-length` and `M-link-restat`;
- during-read: `M-during-read`;
- post-subtree re-listing: `M-relist-names`, plus `M-relist-child` and `M-dir-metadata` for its other two checks;
- descent identity: `M-descent`;
- global budget, as a per-list budget: `M-budget-global`;
- `IncludeEntryOnly` descending: `M-entry-only`;
- `Skip` counter: `M-skip-count`;
- final pass: `M-final`;
- descent mode: `M-descent-mode`;
- decision before sorting: `M-decide-before-sort`.

**How each row flips.** Per-control failure text was classified from the raw outputs.

- **Wrong success** (the walk returned a receipt where a refusal was required). These rows are `M-special`, the four
  device rows, `M-opened` (`06-file-swapped`), `M-relist-names` (`06-created-coarse`), `M-relist-child`,
  `M-dir-metadata`, `M-descent-mode` (`06-chmod-coarse`), `M-final`, `M-root`, `M-budget-charge` (`07-*`),
  `M-mode-mask` (`03-mode`), `M-park`, `M-vanished`, and `M-link-length` (`06-link-swapped`). In the coarse-timestamp
  controls, the wrong success is the frame capturing the impostor's bytes or symlink target, the transient mode, or a
  transient name.
- **A direct wrong value** (the named control observes the removed guard's own output). `M-decide-before-sort` gives
  first `Park` `p3` in reversed order. `M-skip-count` gives `skipped_entries` 0. `M-entry-only` emits `.git`'s
  children. `M-digest-ctime` gives another digest. `M-symlink-max` returns `Ok`. The exhaustion rows and `M-explain`
  give the misclassified `EntryLimit` and `Io`. `M-rewind` gives an empty second listing. `M-relist-shared` gives an
  emit-budget `EntryLimit` at the exact budget. `M-budget-charge` refuses nothing at zero, in `p-list`.

**Layered rows.** In these, a later layer refuses with a different variant. Each entry says whether a wrong success
could exist.
1. **`M-sort`:** 2B2b1's encoder refuses `Frame(OutOfOrder)`. The encoder never accepts unsorted input, so the sort is
   what lets a walk succeed at all. No wrong success.
2. **`M-nofollow`:** in `02`, the dangling link is stated through (`ENOENT`) while the root's names are stated and
   decided, before any entry is processed, so it refuses `Vanished`. Without the dangling link, `absolute` would be
   stated as the outside regular file, opened through the link, and captured as that file's bytes: a wrong success
   that reads outside the root. `p-stat` observes `Regular` directly.
3. **`M-during-read`:** a file grown mid-read is refused by the encoder's probe, `ContentLengthMismatch { Long }`. A
   same-size rewrite during the read moves mtime, which the final pass catches (`FinalVerification`). A wrong success
   needs forged timestamps, which §1.1 rules out of scope.
4. **`M-descent`:** the parent's post-subtree check refuses `ChildIdentity`, and the final pass would also see the
   replacement's children. A wrong success needs the impostor directory's whole subtree to equal the original's in
   every folded field, inode numbers included.
5. **`M-descent-mode`** in the real-timestamp `06-chmod`: `DescentIdentity`, because the chmod moved ctime. Its coarse
   twin `06-chmod-coarse` is a wrong success, so the row does not rely on the layer.
6. **`M-budget-global`:** the per-list budget lets the emitting pass list more than the budget, and the still-global
   re-listing counter then runs out, which is reported as `ChildNames`. The emitting pass had already held more than
   one budget of names before that refusal, which is the memory the guard exists to prevent.
7. **`M-lossy` and `M-fold`:** a mangled name no longer resolves, so it refuses `Vanished`. On a case-insensitive
   filesystem (the macOS lane), `M-fold`'s lower-cased name would resolve and be emitted: a wrong success.
8. **`M-streamed`:** the swallowed `ByteBudget` leaves the encoder poisoned, so `finish()` refuses `Frame(Poisoned)`
   (2B2b1's poisoning layer). No wrong success.
9. **`M-link-restat`:** the replacement is still in place at the parent's post-subtree check, which refuses
   `ChildIdentity`. If the original were restored before that check, the re-stat would already see the original too.
   So the re-stat's unique value is earlier, precise attribution. The length binding (`M-link-length`, a wrong success)
   is what defeats a restored impostor.

## 7. Exclusions and their mechanisms

1. **Container HTTP proxy:** workspace runs unset the proxy variables, as §7 and §13 direct. The known `reqwest`
   loopback-mock failures under the proxy are pre-existing (2B2 ledger).
2. **`cargo deny check`:** not run; `cargo-deny` is not installed (`error: no such command: deny`). CI runs it. No
   dependency, feature, or `Cargo.lock` change was made.
3. **Native ext4 lane:** not available in this container, whose `/` and `/tmp` are overlayfs. §7 requires the drift and
   identity controls on native ext4 in CI, and overlayfs does not substitute. Every `control_06_*` passed here, 60 of 60
   repeated runs included.
4. **macOS lane:** not run here. It is the controller's host lane and includes `control_09` on a case-insensitive
   APFS volume. `control_08` is compiled out on macOS (§5.8). The code is written for macOS too: `list_child_names`,
   the `mode_t` and `dev_t` widths, and the `libc::stat` time fields are handled per target. That is untested here.
5. **Windows:** the walker and the primitives are `#[cfg(unix)]`, as §1.2 requires. Nothing changes on Windows.

## 8. Interpretations the reviewer should check

- **`child_metadata_no_follow` returns `Option<ChildStatV1>`.** §2 names the return `ChildStatV1`. The walker needs a
  typed absence, a name that vanished after listing (§5.6), so `ENOENT` is `Ok(None)`, as in `stat_child_no_follow`.
- **`SourceDrift.path` is `Option`.** The root directory is never a frame entry, so the root's own drift carries
  `None`. `detail` is a closed `WalkDriftV1` naming the check that fired.
- **The device check covers skipped entries.** §4.3 says "every entry", and the check runs on every listed entry before
  the selection decides. So a policy-excluded subtree on another device (for example a tmpfs `target/`) refuses
  `MountBoundary` rather than being skipped. That fails closed; 2B2b2b should know it.
- **Skipped and entry-only directories are still stat-verified.** Each one's own listing stat is folded into the
  inventory (§4.6), so activity inside that moves its mtime, such as a build writing `target/`, is drift at the final
  pass. That is intended under the quiescence model, and 2B2b2b should know it.
- **Stricter equalities than §4 lists.**
  - A regular file's opened and post-read `fstat` are compared on every `ChildStatV1` field, where §4.2 lists
    `(dev, ino, size, mtime, ctime)`.
  - The post-read `fstat` is compared with the opened `fstat`. That window is exactly the read; the opened `fstat`
    already equals the listing.
  - Descent also compares `kind`.
- **Descent compares the mode first.** A chmod also moves ctime, so with the identity check first, the variant reported
  for a chmod would depend on the clock's granularity. Mode-first makes it deterministic.
- **The verification counters.** Running out of the re-listing counter or the final pass's counter is `SourceDrift`
  (`ChildNames` or `FinalVerification`), not `EntryLimit`. Both re-list only what the emitting pass listed within the
  same budget, so exhaustion proves growth. The re-listing proof is by induction over the directories already
  re-listed. The final pass has its own counter, and does no §4.4 re-listing, because its digest already covers every
  name.
- **Peak live names.** A directory's own list is live during its re-listing, so the bound is two budgets, stated as
  2·budget − 1 in the tight control. The final pass runs after the emitting pass has released every list.
- **Shared directory offset.** A `dup` shares its open file description, so `list_child_names` rewinds the pin's
  directory offset. Two listings of one pin must not run concurrently. The walker lists sequentially, and this is
  documented on the method.
- **Depth.** Traversal is iterative, so depth costs heap, not stack. It still holds one directory descriptor per level.
  A tree deeper than the process's descriptor limit refuses as `Io`, never partially.
- **A symlink's size is its target's length.** The length binding (§0.1) relies on `st_size` being the target's length,
  which POSIX specifies and Linux and APFS honor. A filesystem that reports another size for a symlink, such as some
  FUSE filesystems, refuses `SymlinkTarget`. That fails closed.
- **`read_child_symlink`'s refusal.** A target that is too long is `FsCustodyError::Unsupported`, which the walker maps
  to `Frame(InvalidSymlinkTarget)`. On Linux and macOS `symlink(2)` cannot create a target over 4,095 bytes, so only the
  primitive's own control reaches it.
- **Selector purity is a documented contract.** The final pass calls `decide` again. An impure selection makes the
  digests differ, so it refuses as `FinalVerification`; it never succeeds silently.

**In-lane self-check.** Before the matrix, the author re-read `custody_walk.rs` against §3 and §4. No separate
reviewer ran in this turn, so this is not the §10 review.

## 9. Owned paths and staged changes

The §8 paths changed, plus `custody_git_tests.rs`. That file carries only the A14 amendment, which the repair turn's
issue list required (§0). The harness, the gates, and the A14 patch live under `.git/a2a-bridge/` and are not part of
the diff.

```text
crates/bridge-core/src/custody_walk.rs          (new)
crates/bridge-core/src/custody_walk_tests.rs    (new)
crates/bridge-core/src/fs_custody.rs            (modified: the four §2 primitives, their types, and a test-only name counter)
crates/bridge-core/src/lib.rs                   (modified: the module declaration only)
crates/bridge-core/src/custody_git_tests.rs     (modified: the A14 amendment only, repair turn)
docs/superpowers/reviews/2026-09-27-adr0041-slice2b2b2a-implementation-handoff.md (new)
```

The folded commit touches these six paths. The first turn's commit already holds all of them except the
`custody_git_tests.rs` amendment. The repair turn changed and staged four of them: `custody_walk.rs`,
`custody_walk_tests.rs`, `custody_git_tests.rs`, and this handoff. `fs_custody.rs` and `lib.rs` are unchanged since
the first turn.
- `git diff --cached --check` exits 0, and `git status --short` shows only these paths.
- Immediately before staging, each source file's SHA-256 still equaled the snapshot manifest (§6), and no
  `pending.json` existed.
- Nothing is committed. The bridge folds the repair into the first turn's commit and keeps its message.

## 10. What remains for the controller

1. The implementation review's second round, under the §10 two-round cap. It should check that the A14 amendment
   (§0) and the symlink bracket and its stated residual (§0.1) close the round-1 REJECT.
2. The controller's macOS lane: the §7 gates on macOS, including `control_09` on case-insensitive APFS.
3. CI: native ext4 for the drift and identity controls, the Windows compile, `cargo deny`, and coverage.
4. The parent plan, the roadmap, and the planning handoff, which the controller alone updates.
