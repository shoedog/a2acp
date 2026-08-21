# Slice B — one host-gate failure: a non-portable test fixture, not a defect in the code

**Candidate:** `750cd8f3` on `a2a/sliceB-candidate` · **Base:** `9ce2074e`
**Container verify:** PASS (all four) · **Review:** APPROVE, converged at 3 attempts

## Host gate

| Gate | Result |
|---|---:|
| `cargo fmt --all -- --check` | exit 0 |
| `cargo clippy --workspace --all-targets --locked -- -D warnings` | exit 0, zero warnings |
| `cargo test -p bridge-worktree --locked` | **exit 0 — 312 passed** |
| `cargo test --workspace --locked --no-fail-fast` | **exit 101 — 1 failed** |

The single failure is `fs_custody::tests::retained_enumeration_matches_read_dir_selection_and_order`.

## Root cause — measured, not inferred

The test builds its fixture with four names, one of them deliberately invalid
UTF-8:

```rust
OsStr::from_bytes(b"non-utf8-\xff"),
…
fs::write(directory.path().join(name), b"entry").unwrap();
```

That `fs::write` panics at `fs_custody.rs:3298` with
`Os { code: 92, kind: Uncategorized, message: "Illegal byte sequence" }`.

Probed directly on this host, independent of the test:

```
open("/tmp/…/non-utf8-\xff", "wb")
  -> REFUSED, errno=92 (EILSEQ): Illegal byte sequence
```

**APFS enforces UTF-8 filenames.** The fixture assumes a filesystem that permits
arbitrary bytes. The failure is in fixture *creation*, before the enumerator
under test is ever called — so it says nothing about
`RetainedDirectoryEnumerationV1`'s correctness.

## Why nothing upstream caught it

- The implement container runs Linux over overlayfs, which permits the name.
  Container `verify: PASS`.
- CI's full workspace suite runs only on `ubuntu-latest` (ext4), which also
  permits it. CI would be green.
- `macos-14` in CI runs `cargo test -p bridge-store` only — no bridge-core, no
  bridge-worktree.

So the **operator host is the only lane that executes this test on APFS**, and it
is the only thing that would ever have caught it.

This is the exact mirror image of the defect this lane already paid for, where a
fixture passed on macOS/APFS and on container overlayfs and failed only on
ubuntu/ext4. Same lesson, opposite direction: **three environments, and any one
of them alone is an incomplete gate.**

## The test's intent is right; only its portability is wrong

Verifying that a non-UTF-8 name survives enumeration unmangled is exactly what a
`read_dir`-equivalence test should check, and `OsString` is the right vehicle.
The fix is to make the fixture's non-UTF-8 entry *conditional on the filesystem
accepting it*: attempt the write, and on `EILSEQ` (or any error) drop that name
from both the expected and actual sets and record the skip visibly, rather than
unwrapping. The portable names must still be compared.

Do not delete the non-UTF-8 case — on ext4 it is real coverage, and CI runs
there.

## Not yet done

The F8 birthtime-capability probe has not been run, so the classifier-policy
stop condition is **undecided**. It must be run after the fixture is repaired,
because a red workspace suite is not a state in which to read an observation.
