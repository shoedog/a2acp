# ADR-0041 Slice 2B2b1 implementation handoff — the pure coverage-payload frame

**Status:** implemented in the container lane and **finalized in a continuation turn**. The first implementation
turn wrote the module, the controls, and the harness, and ran matrix round 1. It then edited the tests file after
round 1 and ended before re-running the matrix, running the gates, or staging. The continuation turn did the rest:
- reviewed those test edits (§6);
- refreshed the snapshot to the final bytes;
- ran matrix round 2 and re-recorded `red-all`;
- ran every §7 gate;
- completed this handoff and staged the §8 paths.

Every §5 criterion has a control, and every control passes (34/34). Every guard's mutation turned its control red:
the matrix defines 68 rows, and round 2 ran all 68 on the final bytes (snapshot `3312111c77b1a8bb`: 68 FLIPPED,
0 NOT-FLIPPED, 0 INADMISSIBLE, 0 not run). The source was then proved equal to that snapshot. The workspace gates
pass with the proxy variables unset (§5). `cargo deny` and the Windows compile check are left to CI (§7). This
handoff records evidence only; it claims no review approval.

**Task (authoritative):** `docs/superpowers/plans/2026-09-26-adr0041-slice2b2b1-coverage-frame-task.md`, revision 3,
SHA-256 `7467a2934dcbd0535c5d89d49dcd35e243f4103264a0af98fed272c0e34c42f3` at the base commit. The bridge copy
`.git/A2A_TASK.md` differs from it only in the gate-renamed headings and the §13 controller notes.

**Clone:** `/Users/wesleyjinks/code/.a2a-implement/impl-69337-fdp3jame`
**Branch:** `implement/impl-69337-fdp3jame`
**Base HEAD:** `f3a4c0a8319e0c9bbd86def509707a7cf9f014a2` (`main`, the merge of the spec in PR #113)

**Container lane:** Linux `7.0.14-orbstack` on **aarch64**, `/tmp` and `/` on overlayfs, running as uid 0, rustc and
cargo 1.94.0. Every cargo command in this document runs with `CARGO_HOME=/cargo CARGO_NET_OFFLINE=true
CARGO_TARGET_DIR=/tmp/target`. No §9 stop condition fired: the frame needed no filesystem, process, Git, or network
access, no new dependency, and no path outside §8.

---

## 1. Structural RED on the predecessor

Before any production code, a scratch test was appended to `lib.rs` at the base commit. It named
`crate::custody_frame::CustodyFrameBudgetV1::new(1, 1)`, and
`cargo test --locked --offline -p bridge-core --lib custody_frame` failed to compile with:

```text
error[E0433]: failed to resolve: could not find `custody_frame` in the crate root
  --> crates/bridge-core/src/lib.rs:74:24
   |
74 |         let _ = crate::custody_frame::CustodyFrameBudgetV1::new(1, 1);
   |                        ^^^^^^^^^^^^^ could not find `custody_frame` in the crate root
error: could not compile `bridge-core` (lib test) due to 1 previous error
```

The scratch lines were then removed, restoring `lib.rs` to the base bytes. The raw output is retained at
`.git/a2a-bridge/mutation/structural-red.txt`.

## 2. Behavioral RED

**Order of work.** `custody_frame_tests.rs` was written first, with every control, against the §4 API as designed.
`custody_frame.rs` was written after it. The first build compiled the pair together, so no control ever ran against a
guard that existed before the control did.

**RED against the guards' absence (§6 allows a stub or the guard's absence).** The harness command `red-all`
removes every `guard` row of the matrix at once: 55 guards, leaving the byte codec intact. It then runs the whole
`custody_frame` suite. Record: `.git/a2a-bridge/mutation/red-all.json`, raw output `output-red-all.txt`.

| Outcome with every guard removed | Controls |
|---|---|
| **FAILED (28)** | 02 class/generation binding; every 03 refusal control (16, the class-code refusals included); 04; 05; the five 06 budget controls; 07; both 08 controls; 09 |
| **passed (6)** | the five positive controls: 01 round trip, 01 canonical order, 02 determinism, 02 golden bytes, 03 class-code table; and 10 (portability), which is structural and has no guard to remove |

The attempts before that record are kept, not discarded:

- **Attempt 1** (`red-all-attempt1-bad-skip-mutation.json`) had a wrong `M-skip-drain` mutation. It read the 32
  digest bytes even when no content was pending, so every decode broke, positives included. The mutation was fixed to
  skip only a pending digest.
- **Attempt 2** (`red-all-attempt2-53-guards.json`) showed that `control_03_io_failures_are_refused_as_io` stayed green:
  I/O propagation had no guard row. Two rows were added, `M-io-read` (a read error taken as end of stream) and
  `M-io-write` (a sink error taken as written).
- **Round 1's record** (`red-all-round1.json`, raw output `output-red-all-round1.txt`, snapshot `1d373045c9d63994`)
  is the 55-guard run described above. It was taken on the tests file as it stood before the post-round-1 edits
  (§6).
- **The record** (`red-all.json`) is the continuation turn's re-run of the same 55-guard build on the final bytes
  (snapshot `3312111c77b1a8bb`, 18:15:22Z): 28 failed and 6 passed. Every test's outcome is identical to round 1's.

**GREEN:** the same suite on the real module passes **34/34** on the final bytes.

## 3. What was built

`crates/bridge-core/src/custody_frame.rs` is new and crate-private. It is declared in `lib.rs` as
`#[allow(dead_code)] mod custody_frame;`, with no `cfg(unix)`. Its only imports are `std::{cmp, fmt, io}`,
`ring::digest`, and `custody_seal::CustodyCoverageClassV1`. No dependency, feature, or `Cargo.lock` change was made.

| §4 item | Implementation |
|---|---|
| `CustodyFrameHeaderV1::new(class, generation_id)` | refuses `object_database` (`ObjectDatabaseNotFramed`) and a generation outside 1..=1024 bytes (`InvalidGeneration`). The id is `&str`, so it is UTF-8 by construction; the decoder checks UTF-8 on input |
| class codes (§2.2) | two explicit `match` tables (`class_code`, `class_from_code`); code 2 → `ObjectDatabaseNotFramed`; 0 and 15..=255 → `UnknownClass` |
| `CustodyFramePathV1::{from_bytes, from_components, as_bytes}`, `Ord` | §2.3 component rules; `Ord` compares component iterators, so `a` < `a/b` < `a.b`; zero components refused as `InvalidComponent { Empty }` (the implicit root is never an entry) |
| `CustodyFrameBudgetV1::new` | refuses `max_entries` > 1,048,576 (`EntryLimit`) and `max_frame_bytes` > 10 GiB (`ByteBudget`) |
| `CustodyFrameEncoderV1::{new, directory, symlink, regular_file, finish}` | order, structure, mode, target, entry-limit, and byte-budget checks before any byte of a record; content streamed through a 64 KiB buffer with SHA-256 on the way, then a one-byte probe; `finish(self)` writes the trailer and returns the summary |
| `CustodyFrameDecoderV1::{new, next_entry}` | header read and compared in `new`; `next_entry` drains and verifies unread content, then yields `CustodyFrameEntryV1::{Directory, Regular, Symlink}`, or `Ok(None)` after count, total, frame digest, and end of stream |
| bounded content reader | `CustodyFrameContentReaderV1: Read`; reads at most the declared length; the read that exhausts content verifies its digest and returns the refusal instead of data; its `io::Error` carries the typed refusal (`CustodyFrameErrorV1::from_io_error`) |
| `CustodyFrameErrorV1` | exactly the §4 variants. `Io` carries only the `io::ErrorKind`. `InvalidComponent { kind }` and `ContentLengthMismatch { kind }` carry a small enum. No message holds a frame byte |
| poisoning | encoder: a `poisoned` flag set by any refusal; decoder: a phase set by any `next_entry` or content-reader refusal |

**Budget accounting.** Both sides reserve the 49-byte trailer up front: records may use at most
`max_frame_bytes − 49`.

- The **encoder** admits the header before writing it. It admits each record's exact size, computed from its
  declared lengths with checked arithmetic, before writing any byte of it. A regular file's size includes its
  declared content and digest.
- The **decoder** admits every field before reading it. It admits a regular file's declared content plus digest as
  soon as the length is read, before any content byte. It validates each path, target, and generation length
  against 4096, 4095, and 1024 before allocating its buffer.
- **Canonical order** is checked with one retained path and a stack of prefix lengths. Its memory is bounded by the
  4096-byte path ceiling, not by the entry count (§2.5, §9).

## 4. The §5 controls

All are in `custody_frame::tests` (`crates/bridge-core/src/custody_frame_tests.rs`). Refusal controls build their
frames with `RawFrame`, an independent test-side writer of the grammar that re-seals the trailer. So removing a guard
gives a wrong success, not a later `FrameDigestMismatch`.

| §5 | Control(s) | What it proves |
|---|---|---|
| 1 | `control_01_round_trip_yields_exactly_the_encoded_entries`, `control_01_the_canonical_order_is_component_wise` | a 15-entry fixture round-trips. It holds nested and empty directories, a zero-length file and a 135,171-byte (three-chunk) file, modes 0o755/0o700/0o644/0o600/0o444, absolute and `..` symlink targets, non-UTF-8 and `\` bytes, `A`/`a`, and `a`/`a/b`/`a.b`. It is decoded through a slice and through a 7-byte, `Interrupted`-injecting source, with content read and with content skipped. Encoder bytes equal `RawFrame`'s bytes. Every receipt equals an independent length and SHA-256. A trickled content reader gives identical bytes and summary |
| 2 | `control_02_the_same_class_generation_and_entries_encode_identically`, `control_02_changing_only_the_class_or_the_generation_changes_the_bytes`, `control_02_golden_bytes_receipts_and_summary` | determinism; class-only and generation-only changes change the bytes and `frame_sha256`, and are refused (`ClassMismatch`, `GenerationMismatch`). The golden frame is a 128-byte hex literal computed by an independent Python script (§4.1). The receipt (length 2, SHA-256 of `hi`) and summary (entries 3, content bytes 2, frame bytes 128, `frame_sha256`) are asserted against independent values. `frame_sha256` is the SHA-256 of all 128 bytes. The trailer digest is the SHA-256 of bytes 0..96, magic through `total` |
| 3 | `control_03_the_class_code_table_frames_all_thirteen_classes`; `control_03_codes_0_2_15_16_255_and_object_database_are_refused`; `control_03_generation_length_boundaries_…` (0/1/1024/1025 on the constructor and the raw decoder, byte-counted with `é`, plus non-UTF-8); `control_03_path_length_boundaries_…` (0/1/4096/4097 on both constructors and the decoder); `control_03_component_length_boundaries_…` (0/1/255/256); `control_03_every_component_fault_…` (each `InvalidComponent` kind); `control_03_symlink_target_boundaries_and_nul` (0/1/4095/4096, NUL); `control_03_mode_boundaries_…` (0o777 and 0 accepted; 0o1000, 0o2000, 0o4000, 0o1777, 0o4755, 0o7777, 0o100644 refused by the encoder; 0o1000, 0o4755, 0o7777, 0xFFFF refused by the decoder); `control_03_bad_magic_and_unsupported_version_are_refused`; `control_03_unknown_tags_are_refused`; `control_03_bytes_after_the_trailer_are_refused`; `control_03_out_of_order_entries_…`; `control_03_an_entry_without_a_directory_parent_…`; `control_03_a_reader_that_disagrees_with_the_declared_length_is_refused`; `control_03_the_read_that_exhausts_corrupted_content_refuses`; `control_03_trailer_count_total_and_frame_digest_mismatches_are_refused`; `control_03_io_failures_are_refused_as_io` | one dedicated trigger per §4 refusal. Max+1 length fields are placed where the frame ends at the length field, so a guard that reads before refusing shows up as `Truncated` |
| 4 | `control_04_every_proper_prefix_is_truncated` | all 128 proper prefixes of the golden frame (0..=127 bytes), content read and skipped: each is exactly `Truncated`; none panics or returns `Ok(None)` |
| 5 | `control_05_every_single_bit_flip_is_refused` | all 1,024 single-bit flips, content read and skipped (2,048 decodes): none completes. The per-offset variant table is recorded in `.git/a2a-bridge/mutation/flip-sweep.txt`; the continuation turn's run on the final bytes printed identical lines. The totals were BadMagic 128, ByteBudget 76, ClassMismatch 6, ContentDigestMismatch 556, CountMismatch 128, FrameDigestMismatch 600, GenerationMismatch 32, InvalidComponent 32, InvalidGeneration 26, InvalidSymlinkTarget 18, MissingParent 26, OutOfOrder 54, PathTooLong 24, TotalMismatch 128, Truncated 96, UnknownClass 10, UnknownTag 60, UnsupportedMode 32, UnsupportedVersion 16 |
| 6 | `control_06_the_entry_limit_admits_max_and_refuses_max_plus_one_on_both_sides`; `control_06_frame_bytes_admit_the_exact_size_and_refuse_one_byte_less_on_both_sides`; `control_06_a_regular_file_reserves_its_declared_length_before_writing_any_byte`; `control_06_declared_lengths_near_u64_max_refuse_without_panic`; `control_06_the_budget_constructor_refuses_values_above_the_v1_ceilings` | max and max+1 for entries (including a limit of 0) and frame bytes (including header-and-trailer-only budgets), on both sides. A refused record leaves the shared sink's length unchanged and the content reader untouched, although that reader holds all 101 declared bytes. Declared lengths `u64::MAX`, `MAX−31`, `MAX−50` (encoder) and `MAX`, `MAX−31`, `MAX−40` (decoder) exercise each checked-overflow site. The ceilings are pinned as literals |
| 7 | `control_07_a_frame_is_refused_under_another_class_or_generation` | the frame is refused under each of the 12 other framed classes and 5 other generations (prefix, extension, case); `object_database` is refused by the constructor and as code 2 |
| 8 | `control_08_an_encoder_refusal_poisons_every_later_call`, `control_08_a_decoder_refusal_poisons_every_later_call` | after each of five encoder refusals, `directory`, `regular_file`, `symlink`, and `finish` all return `Poisoned`. On the decoder, after `UnknownTag`, a trailer refusal, and a content-reader refusal, later `next_entry` calls and reads return `Poisoned`. Every row is built so that without poisoning the later call would succeed |
| 9 | `control_09_skipped_content_is_still_digest_checked_by_the_next_call` | five rows: content skipped entirely, partly read, zero length, a corrupted byte at offset 70,000 of 100 KiB, and the last entry before the trailer. Each refuses `ContentDigestMismatch` on the next `next_entry` |
| 10 | `control_10_the_frame_module_is_portable_and_effect_free` | the module source names none of `std::os::`, `cfg(unix)`, `cfg(windows)`, `cfg(target_os`, `libc::`, `std::fs`, `std::process`, `std::net`, `std::env`. The Windows compile itself is CI's gate (§7) |

### 4.1 The golden frame's independent source

`.git/a2a-bridge/mutation/golden.py` builds the frame from the §2.1 grammar with Python's `struct` and `hashlib`,
without reference to the Rust code:

```python
b  = b'a2a-cfr1' + bytes([1]) + bytes([4]) + u16(2) + b'g1'                    # header: worktree, "g1"
b += bytes([1]) + u16(1) + b'd' + u16(0o755)                                  # directory d
b += bytes([2]) + u16(3) + b'd/f' + u16(0o644) + u64(2) + b'hi' + sha256(b'hi')  # regular d/f
b += bytes([3]) + u16(3) + b'd/l' + u16(1) + b'f'                             # symlink d/l -> f
b += bytes([0xff]) + u64(3) + u64(2)                                          # trailer
b += sha256(b)                                                                # frame digest
```

Output: 128 bytes; frame digest `15a417a8…d5c9c7`; SHA-256 of the whole frame `87e05bcd…a462a8`; SHA-256 of `hi`
`8f434346…327aa4`. These are the literals in the test.

## 5. Verification totals

**When and how the gates ran.** The continuation turn ran every gate on the final bytes (snapshot
`3312111c77b1a8bb`), after matrix round 2 and the `red-all` re-run.
- **Environment:** `CARGO_HOME=/cargo CARGO_NET_OFFLINE=true CARGO_TARGET_DIR=/tmp/target`, under
  `env -u HTTP_PROXY -u HTTPS_PROXY -u http_proxy -u https_proxy`.
- **Registry:** `/cargo` is the warm registry volume, mounted read-only. Before any gate,
  `cargo metadata --locked --offline` resolved the lockfile from it.
- **Target directory:** `/tmp/target` started empty, so every gate built from scratch.
- **Raw output:** `.git/a2a-bridge/gates/`.

| Gate | Exit | Totals |
|---|---|---|
| `cargo test --locked --offline -p bridge-core --lib custody_frame` | 0 | 34 passed, 0 failed, 0 ignored; 832 filtered out |
| `cargo test --locked --offline -p bridge-core` | 0 | **1,004 passed**, 0 failed, 0 ignored: lib 866, 15 integration binaries 129, doctests 9 |
| `cargo test --locked --offline --workspace --all-targets --no-fail-fast` | 0 | 90 test binaries: **4,601 passed**, 0 failed, 13 ignored |
| `cargo test --locked --offline --workspace --no-fail-fast` | 0 | 90 test binaries and 16 doctest runs: **4,611 passed** (4,601 + 10 doctests), 0 failed, 13 ignored |
| `cargo clippy --locked --offline --workspace --all-targets -- -D warnings` | 0 | no warning or error |
| `cargo fmt --all -- --check` | 0 | no diff |
| `git diff --check` | 0 | no output. Before staging, it sees only `lib.rs`, the one tracked change; `git diff --cached --check` over all four staged paths is in §9 |
| `cargo deny check` | — | **excluded**: not installed (`error: no such command: deny`, exit 101); CI runs it (§7) |
| `cargo run --locked --offline -p a2a-bridge -- validate --repo-hygiene` | 0 | `repository hygiene validated`; `tracked_artifacts: 41`, `validated_example_configs: 9` |

The 13 ignored tests are the pre-existing live and e2e tests outside `bridge-core` (for example `e2e_registry`,
`e2e_acp_codex`, and `live_ollama`). None is in `bridge-core`.

## 6. Mutation matrix

**Harness:** `.git/a2a-bridge/mutation/matrix.py` (untracked; it survives the container). It keeps the log in
`matrix.log`, one JSON record per row in `results.jsonl` (keyed by snapshot), raw cargo output in `output-<id>.txt`,
and the snapshot in `snapshot/` with `manifest.json`. The rules it enforces are 2B2's:

- A row applies only if each of its target strings occurs exactly once in the snapshot. `matrix.py check` reports
  68 rows, 55 guard rows, and 0 problems, including the composition of all guard rows for `red-all`.
- A `pending.json` marker is written before a mutation is applied. It is removed only after the source has been
  rewritten from the snapshot, `fsync`ed, stamped with a fresh mtime (`os.utime` now), and re-hashed equal to the
  snapshot. Any later invocation that finds the marker restores first. SIGINT, SIGTERM, and SIGHUP restore before
  exiting.
- Each row runs `cargo test --locked --offline -p bridge-core --lib custody_frame --no-fail-fast` with a 240 s cap.
- The matrix ran in the **foreground only**, inside `timeout 590` calls with an internal budget. Round 2 was one call,
  `timeout 590 python3 matrix.py run --budget 540`, and never reached its budget. Neither turn ran a background job.

**Verdict rule.** `FLIPPED` requires all of these:

- the crate compiled, with no timeout;
- every named test ran;
- every expected-red control failed;
- every named expected-green control passed.

Anything else is `INADMISSIBLE` or `NOT-FLIPPED`.

**Rounds:**

- **Round 1** (first turn, 17:49:48–17:54:03Z, snapshot `1d373045c9d63994`) ran on the first complete bytes:
  **68/68 FLIPPED**, then `VERIFY OK`.
  - Three rows flipped only through a second layer that the matrix did not name:
    - `M-gen-utf8` gave `GenerationMismatch`;
    - `M-tag` gave `CountMismatch`;
    - `M-enc-reserve` gave `ContentLengthMismatch { Short }`.
  - Its records remain in `results.jsonl` under that snapshot id. The snapshot itself is kept in
    `snapshot-round1/`.
- **Test refinements** (first turn, 17:54:51Z, *after* round 1's verify). There were four test-only edits to
  `custody_frame_tests.rs`, and `custody_frame.rs` and `lib.rs` did not change:
  1. **UTF-8 row.** The UTF-8 control runs the lossy-equal header (`"\u{FFFD}\u{FFFD}"`) first and `header()`
     second. Both still expect `InvalidGeneration`.
  2. **Unknown tags.** The unknown-tag control runs the trailer position first and the entry position second.
     Both still expect `UnknownTag` for each of the four tags.
  3. **Reserve reader.** `CountingReader` gains `available` bytes; the default is 0, so the other call site at
     end of stream is unchanged. The reserve control's reader holds all 101 declared bytes, and the control still
     asserts `ByteBudget`, an unchanged sink length, and `reads == 0`.
  4. **Flip sweep.** The flip sweep also tallies variants per byte offset and prints them. Its three assertions
     are unchanged.
- **Continuation-turn review of those edits.** Each edit is intended: edits 1 to 3 remove the three unnamed layered
  flips above, and edit 4 produces the §5.5 per-flip record. None removes or relaxes an assertion. On the final
  bytes, all 34 controls pass.
- **Round 2** (continuation turn, 18:09:17–18:13:56Z) ran on the final bytes. First, `matrix.py snapshot` refreshed
  the snapshot to them (id `3312111c77b1a8bb`): `custody_frame.rs` and `lib.rs` hash as in round 1, and the tests
  file is `80aee73b…`. Then `matrix.py check` reported 68 rows, 55 guard rows, and 0 problems. One foreground call
  ran all 68 rows: **68 FLIPPED, 0 NOT-FLIPPED, 0 INADMISSIBLE, 0 not run**.
  - Every row's set of failing controls is identical to round 1's.
  - The three refined rows now fail through a wrong success, from the round-2 output:
    - `M-gen-utf8`: "not UTF-8 (Read): wrong success: decoded 0 entries, expected InvalidGeneration";
    - `M-tag`: "trailer 0x0 (Read): wrong success: decoded 1 entries, expected UnknownTag";
    - `M-enc-reserve`: `left: Ok(CustodyFrameFileReceiptV1 { length: 101, … })`, against `Err(ByteBudget)`.
  - The table below is rendered by `table.py` from the round-2 records only. The outcome text is
    `outcomes.json`, checked row by row against the round-2 output. One entry was corrected: `M-dec-reserve` also
    shows a wrong success in `06 bytes`. The prior text is kept in `outcomes-round1.json`.

**Source equals snapshot after the matrix:** yes.
- **Harness verify:** after round 2 the harness logged `VERIFY OK: 3 files equal their snapshot; no pending marker`
  at 18:13:56Z, and again at 18:15:22Z after the `red-all` re-run.
- **Restores:** round 2 logged 68 `APPLIED` and 68 `RESTORED` lines, and no `pending.json` remains. Each restore
  rewrites `custody_frame.rs` from the snapshot with a fresh mtime. Its mtime is that of the last restore, the
  `red-all` re-run's (18:15:22Z).
- **Independent check:** outside the harness, each file's SHA-256 equals the manifest:
  - `custody_frame.rs`: `7cea5dac…62ed3bf`;
  - `custody_frame_tests.rs`: `80aee73b…e998dcbd`;
  - `lib.rs`: `a5f56024…c665be`.

  The same check was repeated after the gates, immediately before staging (§9).

| ID | §5 | Guard mutated | Red | Outcome under mutation |
|---|---|---|---|---|
| M-comp-empty | 3 | component rule: empty | 03 component length, 03 component faults, 03 path length | FLIPPED: wrong success: `""` and `a//b` admitted as paths |
| M-comp-toolong | 3 | component rule: longer than 255 (off by one) | 03 component length, 03 component faults | FLIPPED: wrong success: a 256-byte component admitted |
| M-comp-dot | 3 | component rule: `.` | 03 component faults | FLIPPED: wrong success: `.` admitted |
| M-comp-dotdot | 3 | component rule: `..` | 03 component faults | FLIPPED: wrong success: `..` admitted |
| M-comp-nul | 3 | component rule: NUL | 03 component faults | FLIPPED: wrong success: `a\0b` admitted |
| M-comp-slash | 3 | component rule: `/` inside a component | 03 component faults | FLIPPED: wrong success: the one component `a/b` admitted as two |
| M-root-empty | 3 | zero components (the implicit root) refused | 03 path length | FLIPPED: wrong success: zero components admitted as the empty path |
| M-path-bytes | 3 | path length: `from_bytes` | 03 path length | FLIPPED: wrong success: a 4097-byte path admitted by `from_bytes` |
| M-path-components | 3 | path length: `from_components` | 03 path length | FLIPPED: wrong success: a 4097-byte path admitted by `from_components` |
| M-dec-path-prealloc | 3,6 | path length checked before the path buffer is allocated (decoder) | 03 path length | FLIPPED: layered: the 4097-byte path is allocated and read before `from_bytes` refuses it; the fixture ends, so `Truncated` |
| M-order | 3 | strict component-wise increase | 03 order, 08 encoder | FLIPPED: wrong success: reversed siblings encoded |
| M-parent | 3 | parent is an earlier directory entry | 03 parent | FLIPPED: wrong success: `a/b` encoded without `a` |
| M-mode-enc | 3 | encoder refuses bits outside 0o777 (special bits) | 03 mode, 08 encoder | FLIPPED: wrong success: 0o1000 encoded |
| M-mode-dec | 3 | decoder mode ceiling 0o777 | 03 mode | FLIPPED: wrong success: a raw 0o1000 directory decoded |
| M-link-nul | 3 | symlink target NUL | 03 target | FLIPPED: wrong success: target `a\0b` admitted |
| M-link-min | 3 | symlink target length minimum (1) | 03 target, 08 encoder | FLIPPED: wrong success: an empty target admitted |
| M-link-max | 3 | symlink target length maximum (4095, off by one) | 03 target | FLIPPED: wrong success: a 4096-byte target admitted |
| M-dec-target-prealloc | 3,6 | target length checked before the target buffer is allocated (decoder) | 03 target | FLIPPED: layered: the 4096-byte target is allocated and read before refusal; the fixture ends, so `Truncated` |
| M-gen-ctor | 3 | generation length 1..=1024 (constructor) | 03 generation | FLIPPED: wrong success: an empty generation admitted |
| M-dec-gen-prealloc | 3,6 | generation length 1..=1024 checked before allocation (decoder) | 03 generation | FLIPPED: layered: length 0 reaches `GenerationMismatch`; length 1025 is allocated and read |
| M-gen-utf8 | 3 | generation UTF-8 (decoder) | 03 generation | FLIPPED: wrong success: `ff fe` accepted as `"\u{FFFD}\u{FFFD}"` |
| M-class-unknown | 3 | unknown class codes refused | 03 codes | FLIPPED: wrong success: code 0 decoded as worktree |
| M-objdb-enc | 3,7 | object_database refused by the header constructor | 03 codes, 07 replay | FLIPPED: wrong success: an object_database header constructed |
| M-objdb-dec | 3,7 | code 2 refused by the decoder | 03 codes, 07 replay | FLIPPED: layered: `ClassMismatch` instead of `ObjectDatabaseNotFramed` |
| M-class-cmp | 2,7 | class comparison with the expected header | 02 binding, 07 replay | FLIPPED: wrong success: an index frame decoded under worktree |
| M-gen-cmp | 2,7 | generation comparison with the expected header | 02 binding, 07 replay | FLIPPED: wrong success: a `g2` frame decoded under `g1` |
| M-magic | 3 | magic | 03 magic/version | FLIPPED: wrong success: `a2a-cfr2` decoded |
| M-version | 3 | version | 03 magic/version | FLIPPED: wrong success: version 0 decoded |
| M-tag | 3 | unknown tags refused (not read as a trailer) | 03 tags, 08 decoder | FLIPPED: wrong success: a trailer tagged 0x00 accepted |
| M-truncated | 4 | end of stream inside a field is `Truncated` | 04 truncation | FLIPPED: `Io(UnexpectedEof)` instead of `Truncated` (the empty prefix) |
| M-truncated-content | 4 | end of stream inside content is `Truncated` | 04 truncation | FLIPPED: `Io(UnexpectedEof)` instead of `Truncated` (a prefix ending in content) |
| M-trailing | 3 | end of stream after the trailer | 03 trailing | FLIPPED: wrong success: a trailing byte accepted |
| M-io-read | 3 | a read error is surfaced, never taken as end of stream | 03 io | FLIPPED: a failing reader taken as end of stream: `ContentLengthMismatch { Short }` instead of `Io` |
| M-io-write | 3 | a sink error is surfaced, never taken as written | 03 io | FLIPPED: wrong success: the header reported written to a failed sink |
| M-short | 3 | short reader refused | 03 length, 08 encoder | FLIPPED: wrong success: a 9-byte reader receipted as 10 bytes |
| M-long | 3 | one-byte probe after the declared length | 03 length | FLIPPED: wrong success: an 11-byte reader receipted as 10 bytes |
| M-digest | 3,9 | content digest comparison | 03 exhausting read, 08 decoder, 09 skip | FLIPPED: wrong success: corrupted content read to its end, and skipped, without refusal |
| M-exhaust | 3 | the exhausting read verifies before returning | 03 exhausting read, 08 decoder | FLIPPED: the exhausting read returns data; the refusal waits for the next call |
| M-skip-drain | 9 | skip-drain verification | 03 exhausting read, 09 skip | FLIPPED: wrong success: skipped corrupted content accepted |
| M-count | 3 | trailer entry count | 03 trailer, 08 decoder | FLIPPED: wrong success: a count of 1 for 2 entries accepted |
| M-total | 3 | trailer content total | 03 trailer | FLIPPED: wrong success: a total of 1 for 2 content bytes accepted |
| M-frame-digest | 3,5 | trailer frame digest | 03 trailer, 05 flips | FLIPPED: wrong success: an all-zero frame digest accepted; flips complete |
| M-receipt-length | 2 | receipt content length | 01 round trip, 02 golden, 03 length | FLIPPED: the golden receipt length reads 3 |
| M-receipt-digest | 2 | receipt content SHA-256 | 01 round trip, 02 golden | FLIPPED: the golden receipt digest is SHA-256 of the empty string |
| M-summary-entries | 2 | summary entries | 02 golden | FLIPPED: the golden summary reports 4 entries |
| M-summary-content | 2 | summary content bytes | 02 golden | FLIPPED: the golden summary reports 3 content bytes |
| M-summary-frame-bytes | 2 | summary frame bytes | 02 golden, 06 reserve, 06 bytes | FLIPPED: the golden summary reports 96 frame bytes |
| M-summary-sha256 | 2 | summary frame SHA-256 (the trailer digest instead) | 02 golden | FLIPPED: the golden summary's frame SHA-256 is the trailer digest |
| M-code | 2,3 | class code table (worktree 4 -> 5) | 01 round trip, 02 binding, 02 golden, 03 generation, 03 mode, 03 path length, 03 target, 03 class table, 06 bytes, 06 entries, 07 replay | FLIPPED: the golden hex differs at the class byte; the table reads 5 |
| M-ceiling-entries | 6 | budget ceiling: entries (off by one) | 06 ceilings | FLIPPED: wrong success: 1,048,577 entries admitted |
| M-ceiling-bytes | 6 | budget ceiling: frame bytes (off by one) | 06 ceilings | FLIPPED: wrong success: 10 GiB + 1 admitted |
| M-enc-entries | 6 | encoder entry limit | 06 entries, 08 encoder | FLIPPED: wrong success: a third record encoded under a limit of 2 |
| M-dec-entries | 6 | decoder entry limit | 06 entries | FLIPPED: wrong success: 3 entries decoded under a limit of 2 |
| M-enc-bytes | 6 | encoder byte budget | 06 reserve, 06 overflow, 06 bytes | FLIPPED: wrong success: the last record encoded one byte over budget |
| M-dec-bytes | 6 | decoder byte budget | 06 overflow, 06 bytes | FLIPPED: wrong success: a frame decoded one byte over budget |
| M-enc-header-budget | 6 | encoder admits the header before writing it | 06 bytes | FLIPPED: wrong success: a 24-byte header written under a 72-byte budget |
| M-dec-header-budget | 6 | decoder admits the fixed header | 06 bytes | FLIPPED: wrong success: a frame decoded one byte over budget |
| M-trailer-reserve | 6 | the trailer is reserved against the budget | 06 reserve, 06 bytes | FLIPPED: wrong success: records admitted into the trailer's reserve |
| M-enc-reserve | 6 | a regular file reserves its declared length | 06 reserve, 06 overflow | FLIPPED: wrong success: the 101-byte file written past the budget |
| M-dec-reserve | 6 | the decoder admits declared content before reading it | 06 overflow, 06 bytes | FLIPPED: layered in 06 overflow (content is read past the budget; the fixture ends, so `Truncated`); wrong success in 06 bytes: a frame decoded one byte over budget |
| M-wrap-enc-size | 6 | checked record size (encoder) -> wrapping | 06 overflow | FLIPPED: layered: the size wraps, the prefix is written, and the finite reader ends: `ContentLengthMismatch { Short }` |
| M-wrap-enc-admit | 6 | checked running size (encoder) -> wrapping | 06 overflow | FLIPPED: layered: the running size wraps, the prefix is written, and the finite reader ends: `ContentLengthMismatch { Short }` |
| M-wrap-dec-reserve | 6 | checked content reservation (decoder) -> wrapping | 06 overflow | FLIPPED: layered: the reservation wraps and content is read; the fixture ends, so `Truncated` |
| M-wrap-dec-admit | 6 | checked admitted total (decoder) -> wrapping | 06 overflow | FLIPPED: layered: the admitted total wraps and content is read; the fixture ends, so `Truncated` |
| M-poison-enc | 8 | encoder poisoning | 08 encoder | FLIPPED: wrong success: a later `directory` succeeds after a refusal |
| M-poison-dec | 8 | decoder poisoning (next_entry) | 08 decoder | FLIPPED: wrong success: `next_entry` yields the entry after an `UnknownTag` |
| M-poison-reader | 8 | decoder poisoning (content reader) | 08 decoder | FLIPPED: the second read returns `Ok(0)` instead of `Poisoned` |
| M-portable | 10 | no platform-specific code | 10 portability | FLIPPED: control 10 finds `std::os::` |

**Rows that flip through a second layer (§6).** In round 2, ten rows are layered. In each, the named control asserts
its own typed refusal, and the mutation removes exactly that guard. The control fails because a later layer refuses
with a different variant. Each entry says whether that input or another could turn the removed guard into a wrong
success.

1. **`M-dec-path-prealloc`:** `Truncated` instead of `PathTooLong`. The 4097-byte path is allocated and read before
   any check, and the fixture ends at the length field. When the bytes are present, `from_bytes` re-checks the length
   and refuses `PathTooLong`. So the defect is only the order: a buffer of at most 65,535 bytes sized from an
   unvalidated `u16`. No wrong success.
2. **`M-dec-target-prealloc`:** `Truncated` instead of `InvalidSymlinkTarget`, on the same pattern.
   `validate_symlink_target` re-checks the length after the read. No wrong success.
3. **`M-dec-gen-prealloc`:** `GenerationMismatch` instead of `InvalidGeneration` for length 0. Length 1025 is read
   and ends `Truncated`. The constructor refuses both lengths, so no expected header can hold them, and the
   comparison always refuses. No wrong success.
4. **`M-objdb-dec`:** `ClassMismatch` instead of `ObjectDatabaseNotFramed`. No expected header can name
   `object_database`, so the class comparison always refuses. No wrong success.
5. **`M-dec-reserve`:** layered only in its named control, `06 overflow`, which sees `Truncated`. The same mutation
   is a **wrong success** in `06 bytes`, where a frame one byte over budget decodes. That control shows it directly,
   so this row does not rely on the layer.
6. **`M-wrap-enc-size` and `M-wrap-enc-admit`:** `ContentLengthMismatch { Short }` instead of `ByteBudget`. The
   wrapped size admits the record, its prefix is written, and the finite reader ends. A wrong success would need a
   reader that supplies about 2⁶⁴ bytes.
7. **`M-wrap-dec-reserve` and `M-wrap-dec-admit`:** `Truncated` instead of `ByteBudget`. The wrapped reservation
   admits the content, and the fixture ends. A wrong success would need a frame that carries about 2⁶⁴ content
   bytes.
8. **`M-io-read`:** `ContentLengthMismatch { Short }` instead of `Io`. The control's reader fails mid-content. The
   mutation reports that failure as end of stream, and the length check refuses.
   - The mutated helper, `read_some`, serves every read, including the encoder's one-byte probe and the decoder's
     end-of-stream probe. A source that fails exactly at either probe would be a wrong success under this
     mutation.
   - The control does not construct that input. This is a coverage note for the reviewer. The row still proves the
     guard live.

**Not layered, for contrast:**
- **`M-truncated` and `M-truncated-content`:** the mutation substitutes the variant (`Io(UnexpectedEof)` for
  `Truncated`) at the guard itself, and control 4 sees the substitution directly.
- **`M-exhaust`:** control 3 directly observes that the exhausting read returns without its refusal. The drain layer
  still refuses on the next call, which is why control 9 (a named green for this row) stays green.
- **The three rows that round 1 left layered and unnamed** (`M-gen-utf8`, `M-tag`, `M-enc-reserve`) now fail
  through wrong successes. See Round 2 above.

## 7. Exclusions and their mechanisms

1. **Container HTTP proxy.** The container sets `HTTP_PROXY` and `HTTPS_PROXY`. Per the 2B2 ledger (§6.1 of its
   handoff), 8 `a2a-bridge` tests fail and the `bridge-api` lib tests hang with them set; the cause is `reqwest`
   honoring the proxy for loopback mocks. The workspace gates here ran with `env -u HTTP_PROXY -u HTTPS_PROXY -u
   http_proxy -u https_proxy`, as §7 and §13 direct. Nothing outside `bridge-core` depends on `custody_frame`, which is
   crate-private and unused in production.
2. **`cargo deny check`:** not run. `cargo-deny` is not installed in the image (`error: no such command: deny`), so
   this gate is not green here; CI runs it. No dependency, feature, or `Cargo.lock` change was made.
3. **`cargo check -p bridge-core --target x86_64-pc-windows-msvc`:** not run. Only `aarch64-unknown-linux-gnu` is
   installed, the lane is offline, and `ring` needs a Windows C toolchain (§5.10). The CI Windows job, which builds
   `bridge-core` as a dependency, is the gate. Control 10 proves only the source-level precondition.
4. **Controller macOS lane:** not run here. The controller runs the same gates on macOS (§7).
5. **Real-Git and native-filesystem lanes:** not applicable. This child has no effects (§7).

## 8. Declared limits and interpretations

**§2.6 declared limits, restated.** A v1 frame records only the entry type, the permission bits, the content, and
the symlink target. It does **not** represent:

- owner or group;
- timestamps;
- extended attributes, ACLs, resource forks, or BSD file flags;
- sparse-file layout;
- hard-link identity (each link is captured as an independent regular file);
- special files (FIFO, socket, block or character device).

There is no entry type for a special file. The 2B2b2 walker must refuse one, and that refusal parks the unit. The
module documentation states these limits, and 2B2b2's spec must restate them.

**Interpretations the reviewer should check:**

- **Modes above 0o7777.** §2.4 has the caller pass `st_mode & 0o7777`, and the encoder refuse any `0o7000` bit.
  The encoder refuses **any** bit outside 0o777 as `UnsupportedMode`, so an unmasked file-type bit (for example
  0o100644) is refused rather than masked. For a conforming caller the two readings coincide. For a nonconforming
  one, this reading fails closed. Control 3 pins 0o100644.
- **The end-of-stream probe.** "Refuses before reading past them" holds for every frame byte: the trailer is
  reserved, so even an entry tag read at the reservation edge lies within `max_frame_bytes`. Proving end of stream
  still takes a one-byte read after the trailer (§2.1), and for a frame of exactly `max_frame_bytes` that byte lies
  past the budget. It goes into a fixed one-byte buffer; if a byte is present, the frame is refused `TrailingBytes`.
- **Positions of the content-length probe and digest check.** The encoder's probe consumes at most one extra byte from
  a long reader, then refuses. The decoder's exhausting read returns the digest refusal instead of its final bytes.
  Those bytes are already in the caller's buffer, but the call reports an error.
- **Checked arithmetic.** Every sum over a frame-supplied or caller-supplied value is checked. These are the record
  size, the running frame size, the decoder's admitted total, the content reservation, and both content totals.
  Overflow maps to `ByteBudget`.
  - The remaining unchecked sums are bounded by validated constants: a prefix of at most 4,109 bytes plus 32; a path
    of at most 4,096 + 1 + 255 bytes while it is built; and entry counters bounded by `max_entries` ≤ 2²⁰.
  - The `usize as u64` conversions are lossless on every target of at most 64 bits. Every `u64` to `usize`
    conversion is checked (`chunk_length`).
  - The content-total overflow checks cannot be reached, because the byte budget (≤ 10 GiB) bounds every total. So no
    control or matrix row exists for them.
- **Hostile readers and writers.** A reader that returns more bytes than it was given, or a writer that claims more
  than it was handed, is refused as `Io(InvalidData)` rather than trusted. So no caller-supplied `Read` or `Write`
  can make the codec index out of bounds. `Interrupted` is retried without a cap, as std's `read_exact` and
  `write_all` do, so a reader or sink that returns `Interrupted` forever hangs the call; it cannot corrupt state.
- **The empty path.** A zero path length, `from_bytes(b"")`, or zero components is refused as
  `InvalidComponent { Empty }` (an empty sole component), not `PathTooLong`. §2.3 gives the range 1..=4096 but names
  no variant for 0.

**In-lane self-check.** Before matrix round 2, a read-only subagent reviewed `custody_frame.rs` against §2–§4. It
found no WRONG finding. It also re-derived the golden digests in Python, and checked a Python transcription of the
order validator against a brute-force oracle over about 780,000 sequences, with no disagreement. Its five SMELL
IMMATERIAL notes are the mode reading, the empty-path variant, the end-of-stream probe, the uncapped `Interrupted`
retry, and the lossless `usize as u64` casts. All five are recorded in this section, and none changed code. This
check is not the §10 review.
- **Entries are provisional until `Ok(None)`** (§4). This is stated on `CustodyFrameDecoderV1`. The type system
  cannot enforce it; 2B3's restore-into-a-new-root must honor it.

## 9. Owned paths and staged changes

Only §8 paths changed. The mutation harness and its artifacts live under `.git/` and are not part of the diff.

```text
crates/bridge-core/src/custody_frame.rs          (new)
crates/bridge-core/src/custody_frame_tests.rs    (new)
crates/bridge-core/src/lib.rs                    (modified: the module declaration only)
docs/superpowers/reviews/2026-09-26-adr0041-slice2b2b1-implementation-handoff.md (new)
```

**Staged** by the continuation turn with `git add` of exactly these four paths. `git diff --cached --stat`:

```text
 crates/bridge-core/src/custody_frame.rs            | 1298 +++++++++++++
 crates/bridge-core/src/custody_frame_tests.rs      | 1996 ++++++++++++++++++++
 crates/bridge-core/src/lib.rs                      |    4 +
 ...26-adr0041-slice2b2b1-implementation-handoff.md |  462 +++++
 4 files changed, 3760 insertions(+)
```

- **Checks:** `git diff --cached --check` exits 0 with no output. `git status --short` shows only these four paths.
- **`lib.rs`:** its change is the `#[allow(dead_code)] mod custody_frame;` declaration and its two-line comment.
- **Sources:** immediately before staging, each source file's SHA-256 still equaled the round-2 snapshot manifest
  (§6), and no `pending.json` existed.
- **Not committed:** nothing is committed. `.git/A2A_COMMIT_MSG` holds the §11 subject and a short body.

## 10. What remains for the controller

1. The implementation review, under the §10 two-round cap.
2. The controller macOS lane: the §7 gates on macOS.
3. CI: native Linux, the Windows compile of `bridge-core`, `cargo deny`, and coverage.
4. The parent plan, the roadmap, and the planning handoff, which the controller alone updates (§8).
