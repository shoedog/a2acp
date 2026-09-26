---
task-type: implement
---
# Implement ADR-0041 Slice 2B2b1: the pure coverage-payload frame

**Revision:** 3 (approved at spec review round 2, with its two DEFERs folded; see §12)
**Predecessor (merged code base):** `f16ca474` (`main`, after PR #111 / 2B2 at `90d3a208` and docs PR #112)
**Parent plan:** `docs/superpowers/plans/2026-09-20-adr0041-slice2b-local-capsule-plan.md`, as amended below

## 1. Description

### 1.1 Why this child exists

2B2 exports the non-Git coverage classes as opaque, fixture-bound byte streams (`CustodyCapturedStreamV1`, which has
only a `#[cfg(test)]` constructor). Production custody needs those payloads to carry a worktree, an index, refs,
reflogs, hooks, bridge evidence, and so on, in a form that 2B3 can restore losslessly. 2B2b owns that production
framing (parent-plan amendment of 2026-09-24).

**Split (owner-approved 2026-09-26):** 2B2b is delivered as two serial children:
- **2B2b1 (this task):** the pure, effect-free frame. It covers the byte grammar, canonical order, bounds, path and
  mode policy, a validating streaming encoder, and a validating streaming decoder.
- **2B2b2 (later, separately specified):** the descriptor-relative no-follow tree walker, the per-class entry
  selection, and replacing the exporter's in-memory fixture streams with streaming frame producers. 2B2b2 consumes
  the 2B2b1 API unchanged.

The serial order is 2B1 → 2B2a → 2B2 → 2B2b1 → 2B2b2 → 2B3.

**Goal this child serves:** a coverage payload is a canonical, bounded, self-validating record of a set of
filesystem entries.
- Every accepted frame decodes to exactly the entries that were encoded.
- The same class, generation, and canonical entry sequence always encode to identical bytes. A different class or
  generation always yields different bytes, because the header binds both.
- Every malformed, truncated, reordered, over-budget, or foreign frame is refused with a typed error, without a
  panic and without allocation sized from an unvalidated length.

### 1.2 Scope and non-scope

This child is provider-free and **effect-free**. It adds one crate-private module that works only over caller-supplied
`std::io::Read` and `std::io::Write` values. It must not:
- open, stat, walk, or write any filesystem path;
- spawn processes or run Git;
- add encryption;
- decide which files belong to a coverage class;
- touch the exporter, CLI, config, store, or operator.

A design that seems to need any of these is a stop condition (§9).

## 2. The frame, v1

All integers are unsigned little-endian and fixed width. "Bytes" means raw octets; nothing is NUL-terminated.

### 2.1 Grammar

```text
frame        = header *entry trailer            ; followed immediately by end of stream
header       = magic version class generation
magic        = %s"a2a-cfr1"                      ; 8 bytes: 61 32 61 2d 63 66 72 31
version      = %x01                              ; u8
class        = u8                                ; code from the table in 2.2; never object_database
generation   = u16 length (1..=1024) + UTF-8 bytes ; the manifest generation id, byte-exact

entry        = directory / regular / symlink
directory    = %x01 path mode
regular      = %x02 path mode length content digest
symlink      = %x03 path target
path         = u16 length (1..=4096) + bytes      ; components joined by one "/" byte (2.3)
mode         = u16                               ; permission bits only, <= 0o777 (2.4)
length       = u64                               ; content byte count
content      = <length> bytes
digest       = 32 bytes                          ; SHA-256 of content
target       = u16 length (1..=4095) + bytes     ; symlink target, opaque, no NUL

trailer      = %xFF entries total frame-digest
entries      = u64                               ; number of entry records
total        = u64                               ; sum of all regular-file content lengths
frame-digest = 32 bytes                          ; SHA-256 of every byte from magic through total
```

The frame digest makes the frame self-validating: a single changed byte anywhere refuses by the end of decoding,
even in a field that would otherwise still parse, such as a mode. The seal's ciphertext digest is an independent,
outer check.

Any tag other than `0x01`, `0x02`, `0x03`, and `0xFF` is refused. After the trailer, the decoder requires end of
stream; one extra byte is `TrailingBytes`.

### 2.2 Class codes

The codes are fixed and closed. They follow `CustodyCoverageClassV1::ALL` order, 1-based:

| Code | Class | Code | Class |
|---|---|---|---|
| 1 | refs_and_head | 8 | nested_repositories_and_submodules |
| 2 | object_database (**never framed**; refused) | 9 | lfs_and_external_payloads |
| 3 | index | 10 | alternates_and_shared_stores |
| 4 | worktree | 11 | git_configuration_and_hooks |
| 5 | stash_and_reflogs | 12 | bridge_evidence |
| 6 | in_progress_git_operations | 13 | external_evidence |
| 7 | linked_worktrees | 14 | reproducible_outputs |

- Code 0, code 2, and codes above 14 are refused.
- The header binds the payload to one class and one source generation. A decoder given a different expected class
  or generation refuses, so a payload cannot be replayed under another role (this mirrors 2B2 control 19).

### 2.3 Path policy (lossless)

A path is 1..=4096 bytes, split on `/` into components, and relative to the class root. The class root itself is
implicit and never an entry. Each component must:
- be 1..=255 bytes;
- not be `.` or `..`;
- not contain NUL (`0x00`) or `/`.

Any other byte is allowed, including non-UTF-8 sequences and `\`, because capture must be lossless. Portability,
case-fold collisions, and platform-reserved names are **restore** concerns (2B3), not frame concerns. The frame
preserves `A` and `a` as distinct entries.

The frame never turns a path into a filesystem operation. A path is data.

### 2.4 Mode policy

- For directories and regular files, the caller passes `st_mode & 0o7777`, which is the permission bits **and** the
  setuid, setgid, and sticky bits. It must not pre-mask to `0o777`, because that would erase the evidence the
  encoder needs.
- The encoder refuses any `0o7000` bit as `UnsupportedMode` (reachable callers park the unit, per ADR-0041 §4: an
  unsupported state parks). Otherwise it encodes exactly the `0o777` bits.
- The decoder refuses any mode above `0o777`.
- Symlinks carry no mode field.

### 2.5 Canonical order and structure

Entries are strictly increasing under **component-wise byte order**. Two paths compare component by component, each
by lexicographic byte order; if one path is a proper prefix of the other, the shorter sorts first. This is
exactly depth-first pre-order with siblings sorted by name bytes. For example, `a` < `a/b` < `a.b`, because the
component `a` < `a.b`.

- Strict increase means duplicates are impossible.
- Every entry's parent (every proper prefix path) must already have appeared **as a directory entry**. Root-level
  entries have the implicit root as parent. A regular file or symlink can never be a parent.
- An empty directory is a valid entry. A frame with zero entries is valid.

The encoder enforces the same order and structure as the decoder. The encoder refuses out-of-order input; it does
not sort, because a streaming producer (2B2b2) must emit in canonical order, and sorting would require buffering
the whole set.

### 2.6 Deliberately not represented in v1 (declared limits)

The frame records only type, permission bits, content, and symlink target. It does not represent:
- owner or group;
- timestamps;
- extended attributes, ACLs, resource forks, or BSD file flags;
- sparse-file layout;
- hard-link identity (each link is captured as an independent regular file);
- special files (FIFO, socket, block or character device).

There is no entry type for special files. The walker in 2B2b2 must refuse them, and that refusal parks the unit.
These limits are restated in the handoff and in 2B2b2's spec.

## 3. Bounds

`CustodyFrameBudgetV1 { max_entries, max_frame_bytes }` is caller-supplied. Its constructor refuses values above the
v1 ceilings:
- `max_entries` ≤ **1,048,576**;
- `max_frame_bytes` ≤ **10 GiB** (10 · 2³⁰), the ADR-0041 per-artifact ceiling. Envelope overhead remains the
  sink's concern, as in 2B2 §3.

`max_frame_bytes` counts every byte of the frame: header, records, content, digests, and trailer.

- **Encoder:** before writing any byte of a record, it computes the record's exact size from the declared lengths
  with checked arithmetic, and refuses `ByteBudget` if the record would exceed the remaining budget. A regular file
  reserves its whole record, including its declared content length, before any content byte is written. The
  `max_entries + 1`th record refuses `EntryLimit` before it is written.
- **Decoder:** enforces the same two limits on input, and refuses before reading past them.
- **Allocation:** no allocation is sized from an unvalidated length. Path, target, and generation buffers are
  allocated only after their length field is validated against 4096, 4095, and 1024. Content is never buffered
  whole: both sides stream it through a fixed buffer of at most 64 KiB.
- **Arithmetic:** every sum and every length-to-size conversion is checked. Overflow is `ByteBudget`, never a wrap
  or a panic.
- **Tests:** ceiling boundaries (max and max+1) are tested with small caller budgets, never by materializing large
  inputs, as in 2B2 §3.

## 4. API (crate-private)

New module `crates/bridge-core/src/custody_frame.rs`, declared in `lib.rs` as
`#[allow(dead_code)] mod custody_frame;` because nothing in production uses it until 2B2b2. It is **not**
`#[cfg(unix)]`: it is pure and must **compile** on every CI target. The CI Windows job builds `bridge-core` as a
dependency (non-test), so it proves compilation only. The frame's **tests** run on Linux (CI and the container) and
on macOS (controller lane). Running `bridge-core` tests on Windows is out of scope; custody export and restore are
unix-only.

- **`CustodyFrameHeaderV1::new(class, generation_id)`** validates the §2.2 class (refusing `object_database`) and the
  generation's length and UTF-8.
- **`CustodyFramePathV1::from_components`** and **`CustodyFramePathV1::from_bytes`** apply the §2.3 validation. The
  type provides canonical `Ord` (component-wise) and `as_bytes`.
- **`CustodyFrameBudgetV1::new(max_entries, max_frame_bytes)`** applies the §3 ceilings.
- **`CustodyFrameEncoderV1<W: Write>`**, created by `new(sink, header, budget)`, which writes the header:
  - `directory(path, mode)` and `symlink(path, target)`;
  - `regular_file(path, mode, declared_length, reader: &mut dyn Read) -> Result<CustodyFrameFileReceiptV1>`:
    - streams exactly `declared_length` bytes from `reader` through a bounded buffer and computes the SHA-256
      while streaming;
    - refuses `ContentLengthMismatch { short }` if the reader ends early;
    - after the declared bytes, performs a one-byte probe read and refuses `ContentLengthMismatch { long }` if
      that returns data;
    - returns the length and the digest;
  - `finish() -> Result<CustodyFrameSummaryV1>` writes the trailer and returns `{ entries, content_bytes,
    frame_bytes, frame_sha256 }`, where `frame_sha256` is the SHA-256 of every emitted byte.

  Any error **poisons** the encoder: every later call returns `Poisoned`, and `finish` never succeeds. Bytes already
  written to the sink are the caller's to discard; 2B2b2 routes them into a scratch sink that is never published on
  error.
- **`CustodyFrameDecoderV1<R: Read>`**, created by `new(source, expected_header, budget)`, which reads and validates
  the header:
  - `next_entry() -> Result<Option<CustodyFrameEntryV1<'_>>>` yields directory, symlink, or regular-file entries.
    A regular-file entry exposes a bounded content reader:
    - it reads at most `length` bytes;
    - when the content is exhausted it verifies the digest, and refuses `ContentDigestMismatch` before any later
      call returns;
    - an unread remainder is drained and digest-checked by the next `next_entry()` call, so skipping content
      cannot skip verification.
  - It returns `Ok(None)` exactly at the trailer, after validating the entry count, the content total, the frame
    digest, and end of stream.
  - **Entries are provisional until `Ok(None)`.** A consumer must not treat any yielded entry as verified until the
    decoder reaches the verified trailer. 2B3 restores into a new root and reports success only after that point.
  - Any error poisons the decoder.
- **Refusal codes.** One typed enum, `CustodyFrameErrorV1`, with one variant per distinct refusal, and messages that
  never include content bytes:
  - format: `Io`, `BadMagic`, `UnsupportedVersion`, `UnknownClass`, `ObjectDatabaseNotFramed`, `ClassMismatch`,
    `GenerationMismatch`, `InvalidGeneration`, `UnknownTag`, `Truncated`, `TrailingBytes`;
  - paths and modes: `InvalidComponent { kind }` (kinds: empty, dot, dot-dot, NUL, slash, too long), `PathTooLong`,
    `OutOfOrder`, `MissingParent`, `UnsupportedMode`, `InvalidSymlinkTarget`;
  - content: `ContentLengthMismatch { short | long }`, `ContentDigestMismatch`;
  - trailer: `CountMismatch`, `TotalMismatch`, `FrameDigestMismatch`;
  - budgets: `EntryLimit`, `ByteBudget`;
  - state: `Poisoned`.
- SHA-256 uses `ring::digest` as the other custody modules do. **No new dependency.**

## 5. Acceptance criteria

1. **Round trip.** Decoding an encoded frame yields exactly the encoded entries, in order, with identical paths,
   modes, targets, and content bytes. This holds for a fixture that jointly contains:
   - nested and empty directories;
   - zero-length and multi-chunk (more than 64 KiB) files;
   - executable and non-executable modes;
   - a symlink with an absolute target and one with a `..` target (targets are opaque data);
   - non-UTF-8 and backslash component bytes;
   - names differing only in case;
   - `a` / `a/b` / `a.b` ordering.
2. **Determinism and binding.**
   - Two independent encodes of the same class, generation, and entry sequence produce byte-identical frames and
     equal summaries.
   - Changing only the class, or only the generation, produces different bytes, and the frame is refused by a
     decoder expecting the original header. A golden-bytes test pins the exact encoding of a small frame (header, one of each entry type, and the
   trailer) as a hex literal, so any format change is a deliberate, visible diff.
   - The same test asserts every field of each `CustodyFrameFileReceiptV1` and of `CustodyFrameSummaryV1` (entries,
     content bytes, frame bytes, frame SHA-256) against independently computed expected values. Equality between two
     encodes is not enough.
   - The same test also asserts that `frame_sha256` is the SHA-256 of the complete emitted bytes, trailer digest
     included, and that the trailer digest covers magic through `total`.
3. **Every refusal in §4 has a dedicated test** that triggers exactly that variant.
   - **Class codes:** a table-driven test covers all 13 accepted class/code pairs, encoding and decoding each. Codes
     0, 2, 15, 16, and 255, plus `object_database`, are refused.
   - **Field boundaries:** zero, min, max, and max+1 are covered, on both the raw decoder and the constructor, for
     the generation length (0/1/1024/1025), path length (0/1/4096/4097), component length (0/1/255/256), symlink
     target length (0/1/4095/4096), and mode (`0o777` accepted;
     each of `0o1000`, `0o2000`, and `0o4000` refused by the encoder; a value above `0o777` refused by the decoder).
4. **Truncation sweep.** For a small valid frame containing every entry type, decoding every proper prefix, at every
   byte offset, refuses with `Truncated`. None panics, and none returns `Ok(None)`.
5. **Flip sweep.** For the same small frame, flipping any single bit of any byte never lets decoding complete: every
   flip refuses with a typed error by the time decoding reaches the trailer. The test records which variant each
   flip produced, and fails if any flip completes decoding.
6. **Budgets.** Tests cover:
   - exact-max and max+1 for entries and frame bytes, on both the encoder and the decoder;
   - the encoder refusing a regular file whose declared length would exceed the budget before writing any byte of
     it (the sink length is unchanged);
   - checked-overflow cases, such as a declared length near `u64::MAX`, refusing without a panic;
   - ceiling refusal in `CustodyFrameBudgetV1::new`.
7. **Replay binding.** A frame encoded for class X and generation G is refused when decoded with class Y, or with G′.
   `object_database` is refused on both sides.
8. **Poisoning.** After any encoder or decoder error, every later call returns `Poisoned`, and `finish` never
   succeeds.
9. **Skip safety.** A consumer that ignores a file's content still gets `ContentDigestMismatch` for a corrupted
   content byte on the next call.
10. **Portability.** The module has no `std::os::unix` use. `cargo check -p bridge-core --target
    x86_64-pc-windows-msvc` is not runnable on the controller host (`ring` needs a Windows C toolchain), so the CI
    Windows job is the gate.

## 6. RED-first and mutation evidence

- **Structural RED:** before production code, a test that names `custody_frame` fails to compile on the predecessor.
  Record the exact error.
- **Behavioral RED:** write each §5 control before the guard it tests, and record its failure against a stub or
  against the guard's absence.
- **Mutation matrix:** one row per guard. At minimum:
  - order check off;
  - parent check off;
  - each component rule off;
  - mode ceiling off;
  - symlink NUL check off;
  - length probe off (long reader accepted);
  - digest check off;
  - skip-drain verification off;
  - trailer count, total, or frame-digest check off;
  - each receipt or summary field computed wrongly (content length, content digest, entries, content bytes, frame
    bytes, frame SHA-256);
  - trailing-bytes check off;
  - class or generation comparison off;
  - each budget check off;
  - checked arithmetic replaced by wrapping;
  - poisoning off.

  Each row must turn its dedicated control red. Rows that flip only through a second layer (a different refusal
  type) are allowed but must be named, as in 2B2.
- **Harness rules** (2B2 practice):
  - the harness and its log persist under `.git/a2a-bridge/mutation/`;
  - it runs in the foreground only;
  - every mutation is restored byte-exactly with a fresh mtime;
  - after the matrix, the source is proven equal to its snapshot.

## 7. Verification

Run the parent plan §8 gates that apply to a pure child:

```text
cargo test --locked --offline -p bridge-core --lib custody_frame
cargo test --locked --offline -p bridge-core
cargo test --locked --offline --workspace --all-targets --no-fail-fast
cargo test --locked --offline --workspace --no-fail-fast
cargo clippy --locked --offline --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
git diff --check
cargo deny check          # where cargo-deny is installed; otherwise named as excluded, and CI runs it
cargo run --locked --offline -p a2a-bridge -- validate --repo-hygiene
```

Report exact totals, and use `--no-fail-fast` so one failing binary cannot hide the rest.

- **Implementation container:** unset `HTTP_PROXY` and `HTTPS_PROXY` for workspace runs. With them set, 8 unrelated
  `a2a-bridge` tests fail and the `bridge-api` tests hang; this is pre-existing (2B2 ledger).
- **Controller macOS lane:** the controller runs the same gates on macOS.
- **CI:** covers native Linux, Windows, `cargo deny`, and coverage.
- **Real-Git and native-filesystem lanes:** do not apply, because this child has no effects.

## 8. Files

The implementor owns these paths:
- `crates/bridge-core/src/custody_frame.rs` (new)
- `crates/bridge-core/src/custody_frame_tests.rs` (new; `#[cfg(test)] #[path] mod` from `custody_frame.rs`, as
  `custody_export_tests.rs` is)
- `crates/bridge-core/src/lib.rs` (the module declaration only)
- `docs/superpowers/reviews/2026-09-26-adr0041-slice2b2b1-implementation-handoff.md` (new)

The controller alone updates the parent plan, the roadmap, and the planning handoff.

## 9. Stop conditions

Stop and report rather than broaden scope if:
- any requirement seems to need filesystem, process, Git, or network access, or a new dependency;
- the grammar cannot represent an entry that §2.6 does not already declare out of scope;
- canonical order cannot be validated with memory bounded by the §3 limits;
- a change outside the §8 paths seems necessary;
- a review finding is open-class, or the review cap is exhausted without convergence.

## 10. Review

The spec review and the implementation review each have a **two-round cap**, run by host Sol/xhigh in hard
read-only mode. Every finding is tagged WRONG or SMELL and MATERIAL or IMMATERIAL to the §1.1 goal. Only a WRONG
MATERIAL finding with a constructible input, a wrong result, and a bounded fix is a blocker.

After implementation approval and green CI, the PR is opened and merged under the owner's standing directive.
Implementation and repairs are by Opus 5.5 through the bridge.

## 11. Commit message

```text
feat(bridge-core): ADR-0041 Slice 2B2b1 coverage-payload frame
```

## 12. Review history

**Spec review round 1** (Sol/xhigh, read-only, on revision 1 at `86b51cc6`): REJECT. Dispositions: W1–W3 were
BLOCKERs; W4 and S1–S4 were DEFER. Revision 2 folds all eight, because each fix is small:

- **W1:** the Windows job never runs `bridge-core` tests. §4 now claims Windows compilation only, and names Linux and
  macOS as the test lanes.
- **W2:** the roadmap's main-lineage line was stale. It is fixed to `f16ca474`.
- **W3:** "identical entry sets encode identically" contradicted the header binding. Determinism is now qualified by
  class and generation, and a binding test is added (§1.1, §5.2).
- **W4** (IMMATERIAL): §7 is now the complete parent gate list.
- **S1:** receipts and summaries are asserted against independent expected values, with mutation rows.
- **S2:** a table covers all 13 class codes, plus min/max/max+1 field matrices.
- **S3:** the caller supplies `st_mode & 0o7777`, so special bits are refused rather than masked away.
- **S4:** the parent plan marks the older serial order as superseded.

**Spec review round 2** (final admitted round, on revision 2 at `6c6e4c63`): **APPROVE**. All eight round-1 items
were RESOLVED.

It left two DEFERs, both folded in revision 3 without another review, because they are additive test cases and a
wording fix:
- **a WRONG IMMATERIAL finding:** the history sentence above mixed dispositions and is now corrected;
- **a SMELL MATERIAL finding:** the boundary matrix lacked class codes 16/255 and the zero-length edges, which §5.3
  now includes.
