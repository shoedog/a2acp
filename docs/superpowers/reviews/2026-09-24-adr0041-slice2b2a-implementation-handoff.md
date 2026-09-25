# Handoff — ADR-0041 Slice 2B2a descriptor seam and Git runner

**Implementation base:** `90c5c247d96861f932a043f3b5e0ce7b119c6ac6` (task base
`5e431f4f2dd6f77c66d64fa28dc48054f396edf9`). The delivered diff is restricted to the five
paths named by the task. The runner remains crate-private and production-unwired.

## Scope delivered

`PinnedDirectoryV1` now creates/open children through its retained descriptor, checks new-directory
emptiness, effective owner/mode, and parent-entry identity, and roots commands using an owned
duplicate descriptor. `custody_git` admits a caller-pinned binary, records descriptor facts in a
digest-mismatch refusal, rechecks every route component before and after execution, uses a closed
Git command set and environment allowlist, terminates a process group on cap/deadline, and supports
file-descriptor stdin and stdout streaming. Captured stdout remains a separately selected bounded
mode for version parsing and small controls.

No exporter, capsule, ledger, seal, restore, CLI, configuration, store, scheduler, dependency, or
lockfile change is included.

## Pre-code lane inventory

| Lane | Route | device/inode | SHA-256 | Version | Status |
|---|---|---:|---|---|---|
| Implement container (uid 0) | `/opt/git/bin/git` | `77` / `82093288` | `d0f9299bdb98a5f3229f6ca1a545f1dc1abeffb2f7156d0550b58a79d7efe3c4` | `git version 2.54.0` | measured; non-native ext4 (`fuseblk`) |
| macOS Command Line Tools | `/Library/Developer/CommandLineTools/usr/bin/git` | — | — | — | not measured |
| GitHub Actions Ubuntu | `/usr/bin/git` | — | — | — | not measured |

The container uses the test-system profile because it runs as uid 0; production admission refuses
uid 0. The macOS and native-ext4 lanes are named exclusions, not green evidence.

## Structural RED and mutation evidence

The structural RED on `5e431f4f` was missing `custody_git` (`E0433`). The controls below execute
compiling `#[cfg(test)]` hooks or bypasses at the named guard. Each row asserts the mutated wrong
outcome first, drops the guard or uses its default-restored mode, and asserts the normal outcome in
the same test. This is source-level mutation through deterministic test seams, not a claim that an
untracked edit was left in the working tree.

| Controls | Targeted mutation/fixture | Mutated result asserted | Restored result asserted |
|---|---|---|---|
| A1 | Rename the pinned root pathname before child creation | neither child appears in replacement pathname | directory and regular child appear under retained descriptor |
| A2a | Post-`mkdirat` hook substitutes a non-empty directory | typed refusal/no returned pin | normal created directory pins successfully |
| A2b/A2d | Post-`mkdirat` hook substitutes mode `0755`; injected metadata gives link count one | mode guard refuses | owner/mode `0700` accepts both link counts one and two |
| A2c | Post-open hook renames opened child aside and replaces parent entry | `IdentityChanged` | normal parent entry matches opened descriptor |
| A3/A4 | Swap root after `root_command`; then drop original pin and pressure descriptor reuse | marker and cwd do not use replacement/reused fd | marker/cwd use retained directory; caller observes root drift |
| A5 | Route table uses `0777`, untrusted uid, and final symlink; `FINAL_SYMLINK_REFUSAL_BYPASS` skips pre-canonicalization lstat | insecure route refuses; bypass admits final symlink | sealed trusted route admits |
| A5a/A5b | Wrong digest and post-admission replacement before spawn | `DigestMismatch` or identity refusal; marker absent | own digest admits and marker appears |
| A5c | Callback replaces route after pre-spawn recheck | `BinaryDrift` after child | unchanged route succeeds |
| A5e/A5e-t | Audit hook installs byte-identical new inode; each route-fact field is changed singly | `RouteIdentityChanged`; every one-field row compares unequal | unchanged descriptor facts compare equal |
| A5e-r | After admission add root group-write or ordinary-user owner-write to anchor | shared route predicate reports write permitted and runner returns `RouteRefusal` | sealed anchor admits |
| A5f/A5g | Hold fact observation constant while rewriting same-length route before spawn or after helper `exec` | pre-spawn `DigestMismatch`; post-exit `BinaryDrift` | original hash returns success |
| A6 | `add_environment_variable` adds one non-allowlisted key | fixture sees `A2A_GIT_TEST_INHERITED=1` | exact allowlist and retained cwd match golden |
| A7 | Exact argv golden covers every enum variant; invalid `g`/`z` IDs are submitted | malformed hashes are `InvalidCommand` | every fixed argv matches golden |
| A8/A8b | cap+1 stdout/stderr; `skip_stdout_limit`/`skip_stderr_limit` compile mutations | normal guard returns named cap; bypass accepts 17 bytes | exact 16-byte stream succeeds |
| A9 | sleeping wrapper forks a pipe-holding grandchild; separate parent-exits-immediately fixture retains pipe | typed `Timeout`, bounded return, portable `kill -0` proves descendant disappears | drain watchdog returns before deadline even when only descendant holds pipes |
| A10 | 1 MiB file stdin and file stdout; each serialization switch delays readers | each of stdin-before-readers, stdout-after-stdin, stderr-after-exit returns `Timeout` | descriptor streaming completes 1 MiB stdout, stderr, and input before deadline |
| A11a–A11c | Real local promisor remote with exactly one no-lazy guard left; all three guards removed | one active guard prints `<oid> missing` and leaves copied store empty; all removed fetches blob | each fresh copied store proves independent guard behavior; immutable template digest is unchanged |
| A11d | `init_sets_git_dir` and `init_uses_template` | evidence contains `GIT_DIR`; planted template leaks a file | normal `InitBare` has exact four-entry shape and no `GIT_DIR`/remote config |
| A12 | ext4 classifier rows for ext2/ext3/wrong device/duplicate/malformed mountinfo | named non-admit classification | matching `0xEF53`/device/ext4 admits |
| A13 | parser rows minimum-1, malformed, and nonzero fixture | unsupported-version refusal | minimum and Apple-suffix version admit |
| A14 | AST inventory parses a synthetic owned `unsafe { libc::getpid() }` mutation | extra method-level unsafe site is observed | base’s 35 sites and the exact three new fs sites plus two runner sites match frozen maps |
| A15 | Object-store primary/alternate paths contain each prohibited byte, empty, relative, or `..` | typed object-route refusal | two absolute entries round-trip |
| A16 | Real Git creates a pack, runner streams it to `index-pack`, then parses `pack\t<hash>` | missing fixed `objects/pack/` namespace would fail verify argv/test | `verify-pack` succeeds at exact fixed path |
| A17/A18 | Production/test profile and anchor table; alternate order and primary/alternate role swaps | production uid-0/unanchored routes refuse; evidence differs | sealed anchored fixture and ordered evidence admit/match |

The actual focused commands and totals were:

```text
cargo test --locked --offline -p bridge-core --lib fs_custody --quiet
# 117 passed; 0 failed; 639 filtered

cargo test --locked --offline -p bridge-core --lib custody_git --quiet
# 16 passed; 0 failed; 740 filtered

cargo test --locked --offline -p bridge-core --lib
# 756 passed; 0 failed; 0 filtered

git diff --check
# exit 0
```

Additional invocation results:

```text
CARGO_INCREMENTAL=0 cargo test --locked --offline --workspace --all-targets --quiet
# passed (exit 0)

CARGO_INCREMENTAL=0 cargo test --locked --offline --workspace --quiet
# blocked in bin/a2a-bridge/tests/e2e_registry.rs:
# api_entry_resolves_and_serves_through_registry
# PromptStream transport failed reading the authenticated API response after prompt start.
# This live/authenticated end-to-end failure is an exclusion, not a green result.

cargo clippy --locked --offline --workspace --all-targets -- -D warnings
# passed (exit 0)

cargo run --locked --offline -p a2a-bridge -- validate --repo-hygiene
# repository hygiene validated; tracked_artifacts: 41; validated_example_configs: 9

cargo fmt --all -- --check
# unavailable: this image has rustfmt but no cargo-fmt subcommand

cargo deny check
# unavailable: cargo-deny is not installed
```

## Verification and lane limits

The required macOS route and native Linux ext4 route were not executed in this container. The ext4
classifier is reusable by 2B2 and rejects overlayfs/fuse/nonmatching mountinfo. The process cleanup
control uses a POSIX-shell `kill -0` probe rather than `/proc`, so it executes on macOS too.

## Honest limits

- **HL1:** object-store route paths (`GIT_OBJECT_DIRECTORY`, `GIT_ALTERNATE_OBJECT_DIRECTORIES`) and any variable Git
  requires to be absolute are resolved by Git by path. A same-user swap between the recheck and the lookup is detected
  by the caller's post-exit check, not prevented. For 2B2, the content-addressed closure proof keeps a substituted
  store from yielding a wrong pack.
- **HL2:** an identical substitution of a newly created directory (empty, same owner and mode) between `mkdirat` and
  `openat` is undetectable. The substitute is necessarily a directory inside the retained parent at open time.
- **HL3:** root or another privileged user replacing and restoring the admitted route entirely within one child's
  window. A persisting replacement is detected after the child by the identity and digest recheck.
