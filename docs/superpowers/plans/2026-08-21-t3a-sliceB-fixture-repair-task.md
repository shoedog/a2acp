---
task-type: implement
---

# Slice B repair — one non-portable test fixture

## Description

Slice B is otherwise complete and approved. Its production code is correct and
its design was confirmed by both reviewers. One test fails on the operator's
host, and it fails for a reason that has nothing to do with the code under test.

**Fix only that fixture.** Do not change production, do not restructure, do not
add or remove tests, and do not touch anything else in the slice.

Your base is the approved slice B candidate.

### Measured starting state

`[MEASURED]` on this task's exact base, pinned 1.94.0 toolchain, run on the
operator's macOS/APFS host:

| Gate | Result |
|---|---:|
| `cargo fmt --all -- --check` | exit 0 |
| `cargo clippy --workspace --all-targets --locked -- -D warnings` | exit 0, zero warnings |
| `cargo test -p bridge-worktree --locked` | **exit 0 — 312 passed** |
| `cargo test --workspace --locked --no-fail-fast` | **exit 101 — exactly 1 failed** |

The single failure is
`fs_custody::tests::retained_enumeration_matches_read_dir_selection_and_order`.

Formatting and lint are green. Do not reformat existing Rust; if your change
makes `fmt --check` red, you introduced it.

---

## The defect

The test builds its fixture from four names, one deliberately invalid UTF-8:

```rust
let directory = tempfile::tempdir().unwrap();
for name in [
    OsStr::new("beta"),
    OsStr::new(".hidden"),
    OsStr::new("alpha"),
    OsStr::from_bytes(b"non-utf8-\xff"),
] {
    fs::write(directory.path().join(name), b"entry").unwrap();
}
```

That `fs::write` panics with:

```
Os { code: 92, kind: Uncategorized, message: "Illegal byte sequence" }
```

**Root cause, measured independently of the test.** Creating a file named
`non-utf8-\xff` on the operator host is refused outright:

```
open(b"non-utf8-\xff", "wb")  ->  errno=92 (EILSEQ): Illegal byte sequence
```

APFS enforces UTF-8 filenames. The fixture assumes a filesystem that accepts
arbitrary bytes. The panic happens during fixture **creation**, before
`RetainedDirectoryEnumerationV1` is ever called, so it proves nothing about the
enumerator.

**Why nothing upstream caught it.** The implement container runs Linux over
overlayfs and accepts the name; CI runs its full workspace suite only on
`ubuntu-latest`, which also accepts it; CI's `macos-14` job runs
`cargo test -p bridge-store` only. The operator's host is the sole lane that
executes this test on APFS.

---

## The repair

**Keep the non-UTF-8 coverage.** On ext4 it is real, CI runs there, and
`OsString` round-tripping is exactly what a `read_dir`-equivalence test should
prove. Deleting it would trade a portability bug for a coverage hole.

Make that one entry conditional on the filesystem accepting it:

- attempt to create the non-UTF-8 name and **do not unwrap** the result;
- on success, include it in the fixture and in the comparison exactly as today;
- on **any** error, omit it from both the expected and the actual sets and
  continue with the three portable names;
- when it is omitted, emit one clearly-prefixed line naming the errno or error
  so an operator reading `--nocapture` output can tell coverage was reduced
  rather than silently assuming it ran;
- the three portable names — `beta`, `.hidden`, `alpha` — must still be created
  unconditionally and compared, and `.hidden` must still be present in both
  sets, since dotfile handling is part of what this test proves.

Do not gate on `cfg(target_os = ...)`. Filesystem behavior is not a property of
the operating system: a case-sensitive ext4 volume can be mounted on macOS and
APFS-like restrictions can appear elsewhere. **Probe the filesystem's actual
behavior at runtime** by attempting the write and reacting to the result. That
is the same discipline this lane applied when it refused to infer filesystem
capability from a platform label.

The comparison itself — that the retained enumerator yields the same names in
the same order as `std::fs::read_dir` — must be unchanged for whatever set of
names ends up being created.

---

## Everything else is accepted

Do not modify: `RetainedDirectoryEnumerationV1`, `RetainedObjectIdentityV1`,
either `cfg` arm, `CompatibilityCheckedScanRootSessionV1::finish`,
`classify_root_observations`, any other test in either crate, the handoff, or
the frozen genuine-red control.

If the repair requires touching anything beyond the one fixture, stop and report
under the falsification license rather than widening scope.

## Handoff

Amend the existing slice B handoff in place — do not start a new one. Add a
short note recording: that this fixture failed on APFS with EILSEQ, that the
cause was fixture creation rather than the enumerator, that the non-UTF-8 case
is now runtime-conditional, and that on a filesystem which rejects such names the
test proves the portable subset only.

That last point is a real coverage disclosure and must not be softened: on the
operator's macOS host, this test no longer proves non-UTF-8 round-tripping at
all. Only the ext4 lanes do.

**You make the implementation-candidate commit only.** Gate execution and the
handoff-only evidence commit belong to the host operator; this container's egress
cannot fetch the pinned `a2a-lf` dependency, so `cargo` cannot build here. Do not
attempt the gates, do not run `git diff --cached --check`, and do not fabricate
totals. The six `PENDING OPERATOR` lines stay unticked.

## Sizing

| Counted component | Estimate | Cap |
|---|---:|---:|
| The conditional fixture and its skip diagnostic | 25 | 45 |
| Handoff amendment | 15 | 30 |
| **Total** | **40** | **75** |

If a row will exceed its cap, stop and report rather than compressing evidence.

## Acceptance Criteria

1. `retained_enumeration_matches_read_dir_selection_and_order` passes on a
   filesystem that rejects non-UTF-8 filenames, and still passes on one that
   accepts them.
2. The non-UTF-8 case is retained and attempted at runtime; it is not deleted
   and not gated on `cfg(target_os = ...)`.
3. When the non-UTF-8 entry is omitted, the test emits one clearly-prefixed line
   naming the error, so reduced coverage is visible rather than silent.
4. `beta`, `.hidden`, and `alpha` are created unconditionally and compared in
   every run.
5. The retained-vs-`read_dir` equivalence assertion is unchanged in substance.
6. No production source is modified anywhere in either crate.
7. No other test is modified, added, or removed.
8. The handoff is amended in place and records the APFS behavior, the cause, the
   runtime-conditional design, and the coverage disclosure.
9. The six `PENDING OPERATOR` lines remain unticked and exactly one
   implementation-candidate commit exists.

Do not claim any gate result. Do not tick a pending box.

## Files

- `crates/bridge-core/src/fs_custody.rs` — the one test fixture only.
- `docs/superpowers/reviews/2026-08-21-r2f1b-3d-t3a-inc1-sliceB-handoff.md` —
  amend.
- Everything else — read-only.

## Spec Refs

- `crates/bridge-core/src/fs_custody.rs` —
  `retained_enumeration_matches_read_dir_selection_and_order` and
  `RetainedDirectoryEnumerationV1`.
- `docs/superpowers/plans/2026-08-21-t3a-sliceB-task.md` — the slice B spec; its
  scope fences and falsification license still apply.
- `docs/superpowers/reviews/2026-08-21-sliceB-apfs-fixture-defect.md` — the
  operator's diagnosis, including the independent errno probe.

## Commit Message

test(fs-custody): make the non-UTF-8 enumeration fixture runtime-conditional

`retained_enumeration_matches_read_dir_selection_and_order` created a file named
`non-utf8-\xff` and unwrapped the result. APFS enforces UTF-8 filenames and
refuses it with EILSEQ, so the test panicked during fixture creation — before the
enumerator under test ran — on any such filesystem.

Attempt the name at runtime instead of assuming it, drop it from both sides of
the comparison when the filesystem refuses it, and say so visibly. The three
portable names are unconditional, and the retained-vs-`read_dir` equivalence
assertion is unchanged.

On a filesystem that rejects non-UTF-8 names this test now proves the portable
subset only; the ext4 lanes retain the full coverage.

## Falsification license

The measurements above are operator claims against your base. The repository is
authoritative. If the failing test differs from the code quoted here, if the
fixture does not unwrap its write, or if production code would have to change to
satisfy this repair, record the exact repository evidence and stop before
editing.
