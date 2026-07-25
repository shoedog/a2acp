# R2f0a final native macOS verification

Date: 2026-07-24

## Frozen boundary

- checkout: `/Users/wesleyjinks/code/.a2a-implement/impl-6025-b0q56l47`
- branch: `implement/impl-6025-b0q56l47`
- HEAD: `d7f20d37a9fda493c0b8dc18339489bfe1a059a3`
- tree: `1803a888cf77fdee378367404179cc9ba4085ee6`
- direct parent: `e6276fbc59d37b25ba4315f5e14f260310d71d2b`
- correction base and merge base: `4cc71f4a7e7e82500041d117ed5484f305ed6f13`
- range: 36 commits, 9 changed paths, 23,921 insertions, 8,908 deletions
- host: operator-owned macOS host, effective UID 501
- worktree/index: clean before and after verification

## Fail-first evidence and correction

At clean predecessor `e6276fbc59d37b25ba4315f5e14f260310d71d2b`, the exact native
`foreign_owned_canonical_wal_and_shm_are_refused_before_sqlite_inspection` command failed 0/1 in
its owner-owned platform writable negative control. The genuinely root-required foreign-UID arm
correctly skipped for the non-root operator. The public opener reached the after-initial-probe hook,
so the production effective-owner gate accepted the owner-owned WAL/SHM. The post-transition test
oracle then retained the database and bridge-lock entries in its unrelated-entry vectors.

The cause was test-only path spelling: `TMPDIR` is `/var/folders/...`, its real path is
`/private/var/folders/...`, and the oracle stripped the caller-spelled protected root from an
already-canonical database path. Commit `d7f20d37` canonicalizes the protected root only for deriving
the existing exact database/WAL/SHM/journal/attempt-lock match paths. It also forces the same
counterexample on Linux and macOS with an existing child followed by `..`. The caller-spelled
snapshot, exact root equality, unrelated sentinel, bridge-lock comparison, namespace identity,
semantic WAL recovery, owner/link checks, `.attempt-locks` mode/owner/emptiness, and parent-proof
checks remain intact. The delta from `e6276fbc` is test code only in
`crates/bridge-store/src/sqlite.rs`: 28 insertions and 9 deletions.

## Tier-3 Linux evidence

The isolated implementation controller passed all 27 configured commands:

- format, workspace all-target/all-feature check, warnings-denied Clippy, debug build, and release
  build;
- all named R2f0a owner, sidecar, alias-root, crash-residue, bounded-child, authority, migration,
  platform-root, public-route, comparator, and corruption regressions;
- the full R2f0a bridge-store family;
- accurately labeled hermetic and container-complete workspace lanes (each excluding only the three
  host-PID-1 tests named below);
- repository hygiene and diff check.

Built-in review returned APPROVE on attempt 1. Implementation checkpoint SHA-256:
`7779636f8327c951fbcd6bd1f515c4dc0f848f47b1045f5d599a86708f5e7fb9`.

## Native focused gates

All commands used `CARGO_TARGET_DIR=/private/tmp/a2a-bridge-r2f0a-native-target-e967b62`,
`CARGO_PROFILE_DEV_DEBUG=0`, `--locked`, `--exact`, and `--test-threads=1` where applicable.

- `cargo fmt --all -- --check`: PASS, no diagnostics.
- `foreign_owned_canonical_wal_and_shm_are_refused_before_sqlite_inspection`: PASS,
  1 passed / 0 failed / 0 ignored / 0 measured / 200 filtered. On the non-root host only the actual
  foreign-UID construction arm skipped; all owner-owned controls executed and passed.
- `owner_owned_canonical_wal_and_shm_remain_accepted`: PASS,
  1 passed / 0 failed / 0 ignored / 0 measured / 200 filtered.
- `bounded_crash_fixture_timeout_kills_and_reaps`: PASS,
  1 passed / 0 failed / 0 ignored / 0 measured / 200 filtered.
- `public_platform_writer_recovers_untagged_crash_residue_wal_and_shm`: PASS,
  1 passed / 0 failed / 0 ignored / 0 measured / 200 filtered.
- `process::tests::drop_group_kills_descendants`: PASS,
  1 passed / 0 failed / 0 ignored / 0 measured / 314 filtered across 5 result groups.
- `process::tests::terminate_reaps_child_no_zombie`: PASS,
  1 passed / 0 failed / 0 ignored / 0 measured / 314 filtered across 5 result groups.
- `process::tests::term_ignoring_loop_forces_group_sigkill`: PASS,
  1 passed / 0 failed / 0 ignored / 0 measured / 314 filtered across 5 result groups.
- `cargo deny check`: PASS; `advisories ok`, `bans ok`, `licenses ok`, and `sources ok`. Existing
  duplicate-version findings remained warnings.
- `git diff --check 4cc71f4a..d7f20d37`: PASS, no diagnostics.

## Full native workspace suite

`cargo test --workspace --locked -- --test-threads=1` ran without skips and passed:

- 2,769 passed
- 0 failed
- 12 ignored
- 0 measured
- 0 filtered out
- 73 result groups
- 56 nonempty result groups

This suite includes all three native PID-topology tests above.

## Unverified boundaries

No live or billable provider compatibility canary, host fold, documentation update, GitHub CI, PR,
merge, release, deployment, production-server replacement, or served-version update is claimed by
this evidence.
