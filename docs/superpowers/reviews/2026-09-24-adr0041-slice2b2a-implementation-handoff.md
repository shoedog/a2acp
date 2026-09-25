# Handoff — ADR-0041 Slice 2B2a descriptor seam and Git runner

**Implementation base:** `90c5c247d96861f932a043f3b5e0ce7b119c6ac6` (task base
`5e431f4f2dd6f77c66d64fa28dc48054f396edf9`). **Repair round 1** works from the retained candidate
`535cfb82e1f65481788b033ed3196ca58a21b70e` and closes the five BLOCKER WRONGs of the attempt-3
implementation review. **Repair round 2** closes the controller's two host-verification findings on
that round-1 tree: H1, the macOS portability errors, and H2, the incomplete mutation matrix.
**Repair round 3** is the controller's macOS host lane, which found and fixed two further production
defects. **Repair round 4** works from `14578f3b` and closes W1, W2, W3, and S1 of Sol implementation
review round 1, deferring W4, S2, and S3 to the ledger in that section. The delivered diff is
restricted to the paths named by the task §6. The runner remains crate-private and production-unwired.
This handoff records evidence only; it claims no review approval.

## Scope delivered

`PinnedDirectoryV1` now creates/opens children through its retained descriptor, checks new-directory
emptiness, effective owner/mode, and parent-entry identity, and roots commands using an owned
duplicate descriptor. `custody_git` admits a caller-pinned binary, records descriptor facts in a
digest-mismatch refusal, rechecks every route component before and after execution, uses a closed
Git command set and environment allowlist, terminates a process group on cap/deadline, and supports
file-descriptor stdin and root-confined stdout streaming. File-backed stdin is constructed through a
fallible `from_file` that refuses a non-regular descriptor and enforces an explicit byte bound at both
construction and read time (round 4, W3). Captured stdout remains a separately selected bounded mode
for version parsing and small controls.

No exporter, capsule, ledger, seal, restore, CLI, configuration, store, scheduler, dependency, or
lockfile change is included. No `fs_custody` method beyond the four in task §3 was added: the
root-confined stdout target reuses `create_new_regular_child`.

## Repair round 1 — the five reported WRONGs

Each row was first reproduced as a failing test on the unmodified candidate, then fixed. The
"observed RED" column is the actual failure on `535cfb82`.

| # | Finding | Fix | Regression | Observed RED on the candidate |
|---|---|---|---|---|
| 1 | post-exit caller check skipped on binary drift | `run` now evaluates the binary recheck **and** `after_exit()` before reporting either; documented precedence keeps drift dominant | `w1_post_exit_caller_check_runs_even_when_the_binary_drifted` | `the caller's post-exit custody check must run even when the binary drifted` |
| 2 | root confinement unenforceable from an arbitrary `File`; any command could take an absolute object store | `stream_stdout_to_file(File)` is replaced by `stream_stdout_to_new_child(&PinnedDirectoryV1, &str)`, which validates a single component, creates the target through the retained descriptor, and binds the run to that root's identity; an object-store route is permitted only for the read-only source-reading subcommands (`ObjectStoreRouteRefused`) | `w2_stdout_target_cannot_escape_the_pinned_root`, `w2_mutating_commands_refuse_a_caller_object_store_route` | stdout wrote **32 bytes** into a sibling directory outside the pin (`left: 32, right: 0`); `index-pack --strict --stdin` accepted an external store and created `["pack"]` inside it |
| 3 | caller components could become Git options | `validate_component` refuses a leading `-` for every command operand and relative name | `w3_option_shaped_path_operands_are_refused_before_any_effect` | `InitBare { dir: "-q" }` was accepted, so `git init` ran with no directory operand |
| 4 | mutation evidence missing, several controls masked | isolation seams, fixture repairs, and the missing rows below; 56 targeted mutations executed | the §5 controls themselves | with the admission digest comparison deleted, A5a stayed green (`2 passed`); with the step-3 identity binding deleted, the A5e arm stayed green (`1 passed`) |
| 5 | format gate failed | `cargo fmt --all` applied | `cargo fmt --all -- --check` | `exit 1`, 11 diffs in `custody_git_tests.rs` starting at line 3 |

Finding 4's masked-control claims were verified before repair by deleting each guard and observing
that the control stayed green, which is reproduced verbatim above. The earlier attempts' fixes are
retained: non-hex object-id refusal in `VerifyPack`, and descendant-safe deadline enforcement through
`process_group(0)` plus group termination.

### Control-discrimination repairs made for finding 4

- **A5a** — a `#[cfg(test)]` `bypass_recheck_digest_for_test()` seam holds the pre-spawn recheck's
  digest comparison open, so admission step 4 is the only guard; the control now runs the fixture on
  an unexpected admission so the mutation is visible as an executed marker.
- **A5e** — the arm now uses a marker-writing fixture, asserts the replacement really is a new inode,
  and holds the recheck's route facts constant, so the step-3 binding is the only guard.
- **A10** — the two serialization mutations no longer collapse into one branch. `GitIoScheduleV1` has
  four values, `StdinBeforeReaders` joins the writer before any reader exists, `StdoutAfterStdin`
  keeps stderr concurrent and defers stdout until the writer finishes, and the control asserts the
  schedule each run actually took.
- **A14** — the synthetic parsed string is removed. Both inventories come from the committed sources
  via `include_str!`, and the required mutation is a real `unsafe { libc::getpid(); }` compiled into
  `create_new_child_directory` (row A14 below).
- **A2b** — the owner arm is added as an injected-metadata negative (`owner 43` and `owner 0` against
  effective uid 42, same mode, same link count).
- **A2a** — fixture repaired. The substitute is now `chmod 0700`, because a `0755` substitute was
  refused by the owner/mode guard first, which masked the emptiness guard; the assertion is tightened
  to the typed `EnumerationLimitExceeded`. The first run of this mutation was **INADMISSIBLE** and is
  reported as such here.
- **A17 production row** — fixture repaired. It now asserts the refusal *reason* names uid 0 when
  running as root, because a temp-directory ancestor would otherwise keep the row green with the
  uid-0 refusal deleted.
- **A5 ACL-granted-write row** — added, with a capability probe; it is a named exclusion on this lane
  (see *Named exclusions*).
- **A5g** — the in-place rewrite now restores the modification time with `File::set_modified` and
  asserts both the restored mtime and that the rewrite succeeded before a result is accepted.

## Repair round 2 — the controller's H1 and H2

### H1 — macOS portability (pre-existing in `535cfb82`, not introduced by round 1)

Two sites used a `libc` type whose width differs between macOS and Linux. The container builds Linux
only, so neither was visible here; both are fixed portably rather than conditionally compiled.

| Site | Host error | Fix |
|---|---|---|
| `custody_git.rs` `RouteFactsV1::from_metadata` | `metadata.mode() & libc::S_IFMT` is `u32 & u16` on macOS, where `mode_t` is `u16` | mask with the POSIX file-type literal `0o170000`, which is the same value on both lanes and needs no cast |
| `fs_custody.rs` `create_new_child_directory` | `entry.st_dev == identity.dev.unwrap_or_default()` compares `i32` with `u64` on macOS, where `dev_t` is `i32` | widen with `entry.st_dev as u64`, matching what `MetadataExt::dev` recorded, under a one-line `#[allow(clippy::unnecessary_cast)]` because the cast is a no-op on Linux |

A cast or `u32::from` at the first site would trip `unnecessary_cast` or `useless_conversion` on
Linux, which is why the literal is used there and the narrow allow is used only at the second.

The rest of the round-1 diff was searched for the same class: it contains no other `libc::stat` field
access and no other `mode_t` constant. Two pre-existing sites outside the diff were checked and need
no change — `fs_custody.rs` `link.st_mode & libc::S_IFMT` is `mode_t & mode_t` on both lanes, and
`classify_ext4_admission_for_test` takes a `libc::c_long` that every caller supplies as a literal.

Linux remains clean: `cargo clippy --locked --offline -p bridge-core --all-targets -- -D warnings`
exits 0 (see *Verification commands and results*). At the time of round 2 the macOS compile itself
was a named exclusion, because this container has no macOS toolchain. **Round 3 superseded that**: the
controller compiled and executed the full macOS host lane, found and fixed two further production
defects there, and reported green host gates.

### H2 — the complete matrix, re-run in the foreground

Round 1 ended its turn with the authoritative re-run still in the background at roughly 9 of 56 rows;
the container was then reaped with one mutation still applied (the A5 pre-canonicalization
final-symlink refusal, row 12 below), which the controller restored. The 56-row table in this
handoff is now **the new foreground run**, not the earlier one.

The harness and its full log are committed to the clone's untracked bridge directory, which survives
the container:

- `.git/a2a-bridge/mutation/mutate.py` — the driver, with all 56 rows as data;
- `.git/a2a-bridge/mutation/mutation-log.txt` — the complete log of the authoritative run;
- `.git/a2a-bridge/mutation/snapshot/` — the pre-matrix snapshot of the three owned sources.

Two mechanical corrections to round 1's method are worth recording:

- Restoration now copies content and stamps the modification time to now. Round 1's restore carried
  the snapshot's older mtime back, which cargo reads as "fresh"; the restored run then re-executed
  the *mutated* test binary. The first foreground attempt caught this on row 1 and aborted.
- Rows 30 and 31 were one shared edit in round 1 — the digest comparison inside `recheck`, which both
  arms call — so they could not discriminate. Each row now inlines the recheck at one call site
  without its digest comparison, so row 30 turns A5f red while A5g stays green, and row 31 turns A5g
  red while A5f stays green. The log records both.

## Repair round 3 — macOS host lane (controller, 2026-09-25)

**Who and where.** The controller ran this round on the macOS host: Claude Opus 5.5, the owner-directed implementor
model, working directly on this clone. The containerized implementor builds Linux only and cannot compile or run the
macOS lane, so this round could not go through it.

**Inherited state.** Round 2's macOS portability edits compiled here, but on first host execution 13 tests failed:
12 in `custody_git` and 1 in `fs_custody`. The earlier leftover mutation (`if false` in `admit_route`) had already
been restored byte-exact by the controller.

**Production defects, each with a regression proven fail-first on this host:**

| Defect | Evidence | Fix | Regression and mutation proof |
|---|---|---|---|
| `faccessat(W_OK, AT_EACCESS)` on `/` returns `EROFS` on the sealed macOS system volume, so no real Git route could ever be admitted | a probe of every Command Line Tools ancestor: `/` gave `EROFS`, the others `EACCES`; `/usr/bin/git` gives `EPERM` (SIP) | `classify_write_denial`: `EACCES`, `EROFS`, and `EPERM` prove denial; any other errno refuses | `a5_write_probe_errno_classification_admits_only_proven_denials`. Mutated to `EACCES`-only it went red (`errno 30 proves ...`); restored byte-exact it went green |
| group signalling returned `EPERM` spuriously on macOS, which surfaced as `Stream(EPERM)` from the A8 overflow path. It failed 4 of 5 full-parallel lib runs | two mechanisms were measured. `killpg` against a zombie-only group returns `EPERM` (probe: zombie `EPERM`, after reap `ESRCH`). Under instrumentation, SIGTERM got `EPERM` while `try_wait` still returned `None`, i.e. the leader was mid-exit, and a 1 ms retry then saw the leader reapable | `signal_process_group`: on `EPERM`, treat the signal as moot once the leader has exited; otherwise retry within a bounded 100 ms window; a persistent `EPERM` is still an error | `terminating_a_group_whose_leader_already_exited_returns_its_status`. With the exited-leader check removed it went red with the observed `EPERM`; restored byte-exact it went green. The mid-exit window cannot be reproduced deterministically; it is covered by stress evidence (below) |

**Test-only macOS corrections.** None of these weakens a control:

- **Canonical paths:** assertions compare against canonical paths, because macOS `/var` is `/private/var`
  (`a1_a4`, the `fs_custody` A4 control, and the A5e audit hook).
- **Fixture rewrites:** they now unseal the route file before writing, because a non-root user cannot write a mode-`0500`
  file; the root container lane never saw this.
- **A6 environment comparison:** it excludes the shell-maintained `PWD`, `SHLVL`, `_`, and `OLDPWD`, because macOS
  `/bin/sh` sets `SHLVL=1`. The cwd stays asserted through its marker.
- **Production-profile row:** the expectation depends on the effective uid. Run as root it refuses, as the spec
  requires. Run as an ordinary user it is the lane's production-profile positive: the real Command Line Tools route,
  digest-pinned, admitted.
- **Success-path deadline:** raised from 10 s to a named 60 s constant, because a real `file://` lazy fetch under full
  parallel load exceeded 10 s. Every `Timeout` control keeps its own millisecond deadline.

**Stress evidence for the signal fix:**

| Run set | Result |
|---|---|
| before the fix | 1 of 5 full-parallel `bridge-core` lib runs passed |
| after the fix, host gates | 8 of 8 passed |
| after the fix, with instrumentation and `--nocapture` | 3 of 3 passed; 7 `EPERM` events across them, every one resolved within one retry |

The instrumentation was removed, and the source was restored from the pre-instrument snapshot before the clean fix
was re-applied.

**Final host gates (macOS aarch64, Rust 1.94.0, `CARGO_INCREMENTAL=0`):**

| Gate | Result |
|---|---|
| `cargo fmt --all -- --check` | exit 0 |
| warnings-denied workspace clippy, all targets | exit 0 |
| `cargo test -p bridge-core --lib`, 8 runs | exit 0 each; 763 passed, 0 failed |
| `cargo test --workspace --all-targets --no-fail-fast` | exit 0; 4,494 passed / 0 failed / 13 ignored across 90 result groups |
| `cargo test --workspace --no-fail-fast` | exit 0; 4,500 passed / 0 failed / 13 ignored across 106 groups |
| `git diff --check` | clean |

Totals are summed from `test result:` lines; exit status and the zero failure count are the load-bearing evidence.

**Mutation matrix.** Round 2's 56-row matrix ran in the Linux container before this round's macOS changes, and
was not re-run on this host. The two new production fixes each carry the single-row fail-first proofs above.
**Round 4 re-ran the complete matrix in the container against the delivered tree, which includes these macOS
changes**, at 65 rows; that run is the authoritative one recorded under *Mutation evidence*.

## Repair round 4 — implementation review round 1 (container, 2026-09-25)

Round 4 triages Sol implementation review round 1 (REJECT, four MATERIAL blocker WRONGs and three
DEFER SMELLs) under the controller contract `/contract/repair3.md`. It ran in the Linux container on
`HEAD 14578f3b`, which already carries rounds 1–3. W1, W2, W3, and S1 are fixed here; W4, S2, and S3
are deferred by controller ruling and are recorded in the ledger below rather than implemented.

### W1 — native-ext4 lane evidence (BLOCKER: PR CI could not establish the required lane)

A12's table is synthetic, so a green PR run on overlayfs or XFS was indistinguishable from the
required native ext4 lane. `custody_git_tests.rs` now carries a Linux-only probe,
`live_ext4_lane_probe_records_admission`, which:

1. creates a temp fixture directory and opens it as a retained descriptor, proving by the
   `PinnedDirectoryV1` pin's own recorded `st_dev`/`st_ino` that the descriptor it measures is that
   pinned directory (the pin keeps its descriptor private and 2B2a may not widen that seam);
2. measures `fstatfs` superblock magic on that descriptor and its `st_dev` major:minor;
3. reads the live `/proc/self/mountinfo`;
4. feeds all three into the **production** classifier and prints one greppable line.

The observed line in this container is:

```text
A12-LIVE-LANE: Excluded(not-ext4-superblock) magic=0x794C7630 dev=0:134 fstype=overlay
```

which is the expected `Excluded` for an overlayfs container. The probe asserts only that the
classifier's outcome agrees with the measured inputs, so it does not fail on a non-ext4 developer
machine; **the CI log line decides admission**. Run the lane with `--nocapture` so the line reaches
the log. Its `fstatfs` call is the one `unsafe` it needs — there is no safe std or `libc` path to the
superblock magic — and it is named in the A14 inventory, which now covers `custody_git_tests.rs` as a
third part alongside `fs_custody.rs` and `custody_git.rs`. No `.github` change is needed. After this
round the controller made the probe **assert `Admitted` when `GITHUB_ACTIONS=true`**, because a passing
test's output is captured and never reaches the CI log. A green PR CI run therefore proves the ubuntu
runner's fixture was measured and admitted as native ext4, and a non-ext4 runner fails loudly. Checked
in the container: plain, it passes `Excluded(overlay)`; with `GITHUB_ACTIONS=true` on the same overlay
it fails with `must be native ext4`.

Two matrix rows discriminate it: row 57 feeds the probe a synthetic admitting triple instead of its
measured facts (red — the lane cannot report admission while its live facts disagree), and row 58
removes the superblock magic comparison from the classifier (red in both the probe and the A12
table).

### W2 — mountinfo parser (BLOCKER: A12 admitted syntactically malformed evidence)

`classify_ext4_admission_for_test` now validates strictly: numeric mount and parent IDs, numeric
`major:minor`, the mandatory root/mount-point/options fields, exactly one `" - "` separator, and
exactly the three right-hand fields (fstype, source, super-options). Optional propagation fields
between the options and the separator keep being admitted, because they are part of the format.
Anything else is `MalformedMountInfo`.

**RED first.** The new A12 rows were added before the parser change and each shape was measured
against the old parser. 8 of the 10 shapes were wrongly `Admitted`:

```text
W2-RED-ROW: "36 25 8:1 - ext4"                                   => Admitted (required MalformedMountInfo)
W2-RED-ROW: "x 25 8:1 / /fixture rw - ext4 /dev/sda1 rw"         => Admitted (required MalformedMountInfo)
W2-RED-ROW: "36 y 8:1 / /fixture rw - ext4 /dev/sda1 rw"         => Admitted (required MalformedMountInfo)
W2-RED-ROW: "36 25 x:1 / /fixture rw - ext4 /dev/sda1 rw"        => MalformedMountInfo (already excluded)
W2-RED-ROW: "36 25 8:y / /fixture rw - ext4 /dev/sda1 rw"        => MalformedMountInfo (already excluded)
W2-RED-ROW: "36 25 8:1 / - ext4 /dev/sda1 rw"                    => Admitted (required MalformedMountInfo)
W2-RED-ROW: "36 25 8:1 / /fixture - ext4 /dev/sda1 rw"           => Admitted (required MalformedMountInfo)
W2-RED-ROW: "36 25 8:1 / /fixture rw - ext4 /dev/sda1"           => Admitted (required MalformedMountInfo)
W2-RED-ROW: "36 25 8:1 / /fixture rw - ext4"                     => Admitted (required MalformedMountInfo)
W2-RED-ROW: "36 25 8:1 / /fixture rw - ext4 /dev/sda1 rw - ext4" => Admitted (required MalformedMountInfo)
```

The two nonnumeric `major:minor` rows were **already** excluded by the pre-existing parse and are
kept as coverage, not claimed as new red. Every malformed shape is additionally asserted alongside a
well-formed matching `ext4` line, so partial evidence can never be salvaged into an admission.

**One fixture was repaired rather than its result claimed.** The first two-separator shape
(`… rw - ext4 /dev/sda1 rw - ext4`) is also caught by the right-hand field count, so the
single-separator requirement's own mutation did not flip on the first matrix run. A second shape,
`36 25 8:1 / /fixture rw - ext4 - /dev/sda1`, keeps exactly three fields after the *first* separator,
leaving that requirement as the only guard; the matrix was then re-run from a fresh snapshot and
row 59 flips. Rows 59–62 discriminate the four validations one at a time.

### W3 — `from_file` is fallible and bounded (BLOCKER: a blocking descriptor defeated the deadline)

`GitRunRequestV1::from_file` now returns `Result`, takes an explicit `max_stdin_bytes`, `fstat`s the
caller's descriptor, and refuses anything that is not a regular file (`CustodyGitError::StdinNotRegular`,
naming the observed type) or a regular file longer than the bound (`CustodyGitError::StdinLimit`).
Both refusals are returned by the constructor, so no `run` call — and hence no child — can exist for a
refused descriptor. The bound also holds at read time: the writer reads through `take(max_bytes)` and
re-`fstat`s afterwards, so a descriptor that grew after its measurement is refused rather than
silently truncated. Every call site is updated, including the A16 `index-pack --stdin` and object-store
tests, which now state an explicit `PACK_STDIN_BOUND`.

**RED first.** The fallible signature and the bound parameter were introduced *without* the guards,
the control was added, and every arm was measured against that guardless build:

```text
IR1W3-RED-ARM: fifo descriptor            => accepted (required StdinNotRegular)
IR1W3-RED-ARM: directory descriptor       => accepted (required StdinNotRegular)
IR1W3-RED-ARM: regular file of bound+1    => accepted (required StdinLimit)
IR1W3-RED-ARM: descriptor grown past bound => child received 8192 bytes (required refusal at 4096)
IR1W3-RED-ARM: regular file of exactly the bound => child received 4096 bytes (required 4096, positive arm)
```

The control is `ir1w3_file_stdin_must_be_regular_and_within_its_bound` — prefixed `ir1` because
rounds 1–3 already used `w1`/`w2`/`w3` for the controller review's findings. The FIFO is opened
`O_RDWR` so the open itself does not block, which is exactly the reviewed constructible input. The
fixture records the byte count it received both on stdout and in a rooted file, so the bound is
observable when the run refuses as well as when it succeeds. Rows 63–65 discriminate the three
guards: the regular-file refusal, the construction-time bound, and the read-time bound.

### S1 — the handoff's macOS custody record

The lane inventory and named exclusions below are reconciled to round 3's macOS measurements: the
route is `/Library/Developer/CommandLineTools/usr/bin/git` at `git version 2.54.0 (Apple Git-157)`,
admitted through the production profile as an ordinary user. No section now says macOS was not
measured or executed. The controller filled in the inode and SHA-256 cells from a host measurement
after this round.

### Deferred ledger (controller ruling — recorded, not implemented)

| Item | Source | Why deferred | What a follow-up must do |
|---|---|---|---|
| **W4** — process-group-ID reuse after the leader is reaped | implementation review round 1, WRONG 4 (BLOCKER as reviewed) | Theoretical-only without a deliberate racer. The fix — an unreaped leader anchor via `waitid(WNOWAIT)` — adds an `unsafe` boundary and is a **spec amendment**, because 2B2a's authorized `unsafe` scope is closed and frozen by A14 | Retain an identity-bearing process authority until final teardown and signal only generation-verified members, or hold an unreaped group anchor until draining completes. RED: an injectable authority that replaces the leader generation after reap must observe zero signals to the replacement. Goes to a follow-up slice with its own A14 amendment |
| **S2** — deterministic negative coverage for the signal branches | implementation review round 1, SMELL 2 (DEFER) | The current branches read fail-closed; only the transient live-leader retry rests on stress evidence rather than a regression | Extract an injectable signal-outcome state machine and test `EPERM→success`, `EPERM→exited`, and persistent `EPERM` deterministically |
| **S3** — A18 exact-digest domain-tag fixtures | implementation review round 1, SMELL 3 (DEFER) | Immaterial: positional roles still distinguish primary from alternate, so a tag regression cannot silently equalize evidence | Add exact digest fixtures and a same-tag mutation, so reversing or equalizing `git-object-dir` / `git-alternate-dir` turns A18 red |

### Round 4 scope and gates

Changed paths are the three owned sources plus this handoff. No dependency, feature, or `Cargo.lock`
change; the seam stays `pub(crate)` and production-unwired. Gates run with
`CARGO_HOME=/cargo CARGO_NET_OFFLINE=true CARGO_TARGET_DIR=/tmp/target`:

| Gate | Result |
|---|---|
| `cargo fmt --all -- --check` | exit 0 |
| `cargo clippy --locked --offline -p bridge-core --all-targets -- -D warnings` | exit 0 |
| `cargo test --locked --offline -p bridge-core --lib` | ok, 764 passed / 0 failed |
| `cargo test --locked --offline -p bridge-core --lib custody_git` | ok, 24 passed / 0 failed |
| `cargo test --locked --offline -p bridge-core --lib fs_custody` | ok, 117 + 1 passed / 0 failed (two targets) |
| `git diff --cached --check` | clean |

`custody_git` is 22 → 24: the live ext4 lane probe and the W3 stdin control. The whole lib is
760 → 764 across rounds 3 and 4.

## Pre-code lane inventory

| Lane | Route | inode / links | SHA-256 | Version | Status |
|---|---|---:|---|---|---|
| Implement container (uid 0) | `/opt/git/bin/git` | `82093288` / 148 | `d0f9299bdb98a5f3229f6ca1a545f1dc1abeffb2f7156d0550b58a79d7efe3c4` | `git version 2.54.0` | **measured and executed** (rounds 2 and 4); root-owned 0755 with root-owned 0755 ancestors |
| macOS Command Line Tools (aarch64, ordinary user) | `/Library/Developer/CommandLineTools/usr/bin/git` | `221490032` / 1 | `a73bf622a2e470d5d57a4b1d5aef1e8680e67278018d4858a2f93825b7d595c7` | `git version 2.54.0 (Apple Git-157)` | **measured and executed** in repair round 3; admitted through the **production** profile, digest-pinned, with all host gates green |
| GitHub Actions Ubuntu (native ext4) | `/usr/bin/git` | — | — | — | not executed; the `A12-LIVE-LANE` probe that decides it exists as of round 4, the workflow that runs it does not |

The container uses the test-system profile because it runs as uid 0; production admission refuses
uid 0. The route's `st_dev` is the container's overlay device (`0:134` in `/proc/self/mountinfo`) and
is instance-specific, so it is not recorded as a stable fact.

The macOS lane's inode and SHA-256 were measured by the controller on the host on 2026-09-25
(root-owned, mode 0755, one link) and are now filled in above. The **native Linux ext4 lane
remains a named exclusion**: it is the one required lane that no round has executed.

## Verification commands and results

Round 1 reported every `cargo`-driven gate as unrunnable because `CARGO_HOME=/cargo` did not exist in
that container and the image registry lacked the workspace's dependencies. **In repair round 2 the
dependencies are present read-only at `/cargo`, so cargo runs.** The round-1 hand-reconstructed
`rustc` harness is retired; nothing below depends on it. Every command was run with
`CARGO_HOME=/cargo CARGO_NET_OFFLINE=true CARGO_TARGET_DIR=/tmp/target`, on `rustc 1.94.0
(4a4ef493e 2026-03-02)` / `cargo 1.94.0 (85eff7c80 2026-01-15)`.

```text
cargo fmt --all -- --check                                                   # exit 0
cargo clippy --locked --offline -p bridge-core --all-targets -- -D warnings   # exit 0
git diff --check                                                             # exit 0

cargo test --locked --offline -p bridge-core --lib custody_git
# test result: ok. 24 passed; 0 failed; 0 ignored; 740 filtered out

cargo test --locked --offline -p bridge-core --lib fs_custody
# test result: ok. 1 passed; 0 failed; 0 ignored; 763 filtered out
# test result: ok. 117 passed; 0 failed; 0 ignored; 647 filtered out

cargo test --locked --offline -p bridge-core --lib
# test result: ok. 764 passed; 0 failed; 0 ignored; 0 filtered out
```

The totals above are round 4's, re-measured on the delivered tree. `custody_git` is 16 → 24: four
round-1 regressions for WRONG 1–3, two round-3 macOS regressions, and round 4's live ext4 lane probe
and W3 stdin control. The `fs_custody` selection matches two targets and is unchanged at 117 + 1; the
whole lib is 756 → 764. The clippy run covers `bridge-core`'s lib target (production `cfg`) and its
lib-test target, which is where the H1 `#[allow]` lives.

**The whole-lib selection is flaky, and it is reported rather than rounded up.** Running the whole
lib in parallel intermittently fails one `custody_git` control with `CustodyGitError::Timeout`.
Measured on the delivered tree with no mutation applied: 6 of 10 runs failed in one sample and 1 of 5
in another; the failure is almost always
`custody_git_tests::a11a_a11b_a11c_each_runner_owned_lazy_fetch_guard_is_independent` at
`custody_git_tests.rs:1592`, the unguarded lazy-fetch row, where Git performs a real `file://` fetch
from the promisor remote. `w3_option_shaped_path_operands_are_refused_before_any_effect` and
`a5_route_binding_rechecks_and_digest_refusals_carry_descriptor_facts` were each seen once.

The mechanism is a **control-fixture** limitation, not a production guard: the controls build their
requests through a shared helper that pins a fixed 10-second wall-clock deadline, and under the load
of 15 parallel test threads spawning Git subprocesses that budget is not always enough for a row that
does real Git work. The runner's deadline enforcement is itself exercised and discriminated by rows 36
and 37–39 below. Fixing the fixtures' deadline budget is outside the controller's H1/H2 scope and is
**not** done here; it is recorded as a known defect for the next round.

The two selections that hold every named §5 control are stable and are what the matrix judges rows on:
`custody_git` and `fs_custody` were each green on 12 consecutive unmutated runs.

**Not run, and not claimed as green:** `cargo test --workspace` (and `--workspace --all-targets`),
`cargo clippy --workspace --all-targets`, `cargo deny check`, and
`cargo run -p a2a-bridge -- validate --repo-hygiene`. These were outside the repair turn's stated
command set; the controller's host gates remain the authority for them.

## Mutation evidence

65 targeted mutations were executed against the delivered tree, **in one complete foreground run**.
Each row applies one mutation to one named guard, rebuilds the `bridge-core` lib test harness, runs
the row's control selection (mutated result), restores the source byte-exactly from a pre-matrix
snapshot, proves the restoration with `git diff --no-index` and a SHA-256 comparison against that
snapshot, rebuilds, and runs the control again (restored result). A row counts only if the **named**
control turns red and the restoration turns it green; a mutation that does not compile is recorded as
inadmissible evidence and never as a flip.

- driver: `python3 .git/a2a-bridge/mutation/mutate.py`
- full log: `.git/a2a-bridge/mutation/mutation-log.txt`
- pre-matrix snapshot: `.git/a2a-bridge/mutation/snapshot/`

The run is `HEAD 14578f3b` plus the unstaged round-4 repair, started `2026-09-25T17:21:18+0000`,
finished `2026-09-25T17:44:16+0000`, 1378.6s wall clock. Every verdict below is copied from that log,
which supersedes round 2's 56-row log: rounds 3 and 4 both changed the sources, so the earlier log is
no longer evidence for the delivered tree and is not quoted here.

The matrix ran as a sequence of **foreground** phases — `--phase header`, then `--phase rows --range`,
then `--phase summary` — because the container caps how long one foreground command may run. They
share one log, one pre-matrix snapshot, and one `state.json`, so the totals, the table, and the final
snapshot proof are computed over the whole matrix exactly as a single-process run would. Nothing was
backgrounded, and no turn ended with a job running.

Result: **rows executed 65; flipped their named control 64; did not flip 1 (row 13); inadmissible 0.**

Rows are judged on the two control selections that hold every named §5 control, `custody_git` and
`fs_custody`, each measured green on 12 consecutive unmutated runs. No verdict comes from a single
run: a "no flip" required three independent confirmations, and a red restored tree was retried three
times before the matrix would abort. The whole-lib selection is recorded in the log at both ends of
the matrix but is not gated, for the flake documented above.

| # | Control | Targeted mutation (single guard) | Mutated | Restored |
|---|---|---|---|---|
| 1 | A1 | `mkdirat` on the retained descriptor → `mkdirat(AT_FDCWD, <joined pathname>)` | red | green |
| 2 | A1 | `create_new_regular_child` → `File::create(<joined pathname>)` | red | green |
| 3 | A2a | emptiness enumeration replaced by `Ok(())` | red | green |
| 4 | A2b | owner half of the owner/mode predicate removed | red | green |
| 5 | A2b | mode half of the owner/mode predicate removed | red | green |
| 6 | A2c | parent-entry `st_dev`/`st_ino` comparison → `true` | red | green |
| 7 | A2d | literal `link_count == 2` requirement added | red | green |
| 8 | A3 | `pre_exec` `fchdir` on the owned duplicate → `chdir(<pin pathname>)` | red | green |
| 9 | A4 | owned duplicate descriptor → borrowed raw descriptor number | red | green |
| 10 | A5 | trusted-owner comparison → `true` | red | green |
| 11 | A5 | root-profile group/other write-bit refusal → `false` | red | green |
| 12 | A5 | pre-canonicalization final-symlink refusal → `false` | red | green |
| 13 | A5 | `faccessat(W_OK, AT_EACCESS)` refusal removed | **no flip** | green |
| 14 | A5a | admission step-4 digest comparison → `false` | red | green |
| 15 | A5b | pre-spawn recheck call removed | red | green |
| 16 | A5c | post-exit recheck call removed | red | green |
| 17 | A5e | step-3 identity binding removed | red | green |
| 18 | A5e-t | `dev` comparison removed from `same_route_facts` | red | green |
| 19 | A5e-t | `ino` comparison removed from `same_route_facts` | red | green |
| 20 | A5e-t | `file_type` comparison removed from `same_route_facts` | red | green |
| 21 | A5e-t | `uid` comparison removed from `same_route_facts` | red | green |
| 22 | A5e-t | `gid` comparison removed from `same_route_facts` | red | green |
| 23 | A5e-t | `mode` comparison removed from `same_route_facts` | red | green |
| 24 | A5e-t | `size` comparison removed from `same_route_facts` | red | green |
| 25 | A5e-t | `mtime_secs` comparison removed from `same_route_facts` | red | green |
| 26 | A5e-t | `mtime_nanos` comparison removed from `same_route_facts` | red | green |
| 27 | A5e-t | `ctime_secs` comparison removed from `same_route_facts` | red | green |
| 28 | A5e-t | `ctime_nanos` comparison removed from `same_route_facts` | red | green |
| 29 | A5e-r | pre-spawn route-rule re-run (`audit_and_bind` inside `recheck`) removed | red | green |
| 30 | A5f | pre-spawn recheck arm loses only its digest comparison | red | green |
| 31 | A5g | post-exit recheck arm loses only its digest comparison | red | green |
| 32 | A6 | one non-allowlisted environment variable set unconditionally | red | green |
| 33 | A7 | `--no-optional-locks` removed from the fixed argv | red | green |
| 34 | A8 | stdout reader cap → `usize::MAX` | red | green |
| 35 | A8b | stderr reader cap → `usize::MAX` | red | green |
| 36 | A9 | caller wall-clock deadline replaced by a fixed 10-second deadline for the drain watchdog and the wait loop | red | green |
| 37 | A10 | `StdinBeforeReaders` schedule neutralized to `Concurrent` | red | green |
| 38 | A10 | `StdoutAfterStdin` schedule neutralized to `Concurrent` | red | green |
| 39 | A10 | `StderrAfterExit` schedule neutralized to `Concurrent` | red | green |
| 40 | A11a | `--no-lazy-fetch` flag never emitted | red | green |
| 41 | A11b | `GIT_NO_LAZY_FETCH` never set | red | green |
| 42 | A11c | `protocol.allow=never` never emitted | red | green |
| 43 | A11d | `GIT_DIR` omission for `InitBare` removed | red | green |
| 44 | A12 | ext4 filesystem-type comparison removed | red | green |
| 45 | A13 | minimum-version comparison → `false` | red | green |
| 46 | A14 | a compiling `unsafe { libc::getpid(); }` added to `create_new_child_directory` | red | green |
| 47 | A15 | prohibited alternate-list byte refusal → `false` | red | green |
| 48 | A16 | fixed `objects/pack/` namespace removed from the built path | red | green |
| 49 | A17 | fixture anchor stop removed from the ancestor audit | red | green |
| 50 | A17 | production uid-0 refusal → `false` | red | green |
| 51 | A18 | ordered alternate-path evidence dropped | red | green |
| 52 | W1 | post-exit caller check restored to the reported defect (`?` before `after_exit`) | red | green |
| 53 | W2 | stdout-target component validation removed | red | green |
| 54 | W2 | stdout-target root-identity binding → `false` | red | green |
| 55 | W2 | object-store route restriction → `false` | red | green |
| 56 | W3 | leading-dash operand refusal → `false` | red | green |
| 57 | W1 | live lane probe fed a synthetic admitting triple instead of its measured magic, device, and mountinfo | red | green |
| 58 | W1 | ext4 superblock magic comparison removed from the classifier | red | green |
| 59 | W2 | single `" - "` separator requirement → first separator wins | red | green |
| 60 | W2 | mandatory root/mount-point/options fields no longer required | red | green |
| 61 | W2 | numeric mount-ID and parent-ID validation removed | red | green |
| 62 | W2 | mandatory source and super-options fields no longer required | red | green |
| 63 | W3 | `from_file` regular-file (`fstat` type) refusal removed | red | green |
| 64 | W3 | `from_file` maximum-stdin-bytes comparison removed | red | green |
| 65 | W3 | read-time `take(max_bytes)` stdin bound → unbounded read | red | green |

**Row 13 is the single non-flipping row and is inadmissible on this lane, not claimed as evidence.**
Its mutation deletes the `faccessat(W_OK, AT_EACCESS)` write-denial refusal in `deny_effective_write`.
The mechanism is that this container runs as uid 0, and `deny_effective_write` branches on
`effective_uid == 0` *before* `faccessat` is reached, returning on the mode-bit comparison
(`WriteCheckV1::ModeBitsAsRoot`). The deleted code is therefore unreachable on this lane, so its
named control `a5_route_rule_table_and_final_symlink_guard_are_discriminating` stayed green — as did
every test in both control selections, on three independent confirming runs. The row is admissible
only on a lane whose executing user is not root; the mode-bit branch that *is* reachable here is
separately discriminated by row 11.

After the matrix, all three owned sources equal the pre-matrix snapshot byte for byte, and the whole
working-tree diff is unchanged (`git diff` SHA-256
`e8d4de77cbfe95c0f13b68a62e91fc14049c93ae00db7f0b61c1ce3e7964febb` before and after). The post-matrix
source digests, equal to the pre-matrix snapshot's:

```text
6d0560823e4de0ad364cce180492429c169d3ba2948f686eac7aecfe9b2364f1  crates/bridge-core/src/custody_git.rs
98b487861b812d4c2b64fe7afe9b071f02d645f7351361b51a289224e7f3b615  crates/bridge-core/src/fs_custody.rs
0787e1f6945db470bdef50fca109c40e4d06116c98152e5cadebf4ef672038e9  crates/bridge-core/src/custody_git_tests.rs
```

Three mutations have been **inadmissible on a first run and had their fixtures repaired** rather than
their results claimed: A2a (masked by the owner/mode guard) and the A17 production row (masked by the
temp-directory ancestor audit) in round 1, and row 59's single-separator requirement (masked by the
right-hand field count) in round 4. All three flip in the run above, as rows 3, 50, and 59 record.
No row was inadmissible in this run: every anchor was validated to occur exactly once before the
authoritative run, and all 65 mutations compiled.

## Named exclusions

- **Native Linux ext4 lane:** not executed locally; decided by PR CI (see below). The container's
  fixture filesystem is `overlayfs`, so the task §7 ext4 admission rule (`0xEF53` plus a matching
  `ext4` `mountinfo` entry) is not satisfied; overlayfs does not substitute. As of round 4 the probe
  that decides the lane exists and runs on every Linux build — `live_ext4_lane_probe_records_admission`
  measures `fstatfs` and `st_dev` on a retained fixture descriptor, reads the live
  `/proc/self/mountinfo`, and prints `A12-LIVE-LANE:` — but the `.github` workflow that would run it
  on the ubuntu runner is not needed: the probe asserts admission under `GITHUB_ACTIONS=true`, so the
  lane is decided by the PR's CI result. Observed in the container:
  `A12-LIVE-LANE: Excluded(not-ext4-superblock) magic=0x794C7630 dev=0:134 fstype=overlay`.
- **macOS host lane:** **not an exclusion.** It was measured and executed by the controller in repair
  round 3 — route, version, production-profile admission, two production defects found and fixed, and
  green host gates — and is recorded in *Repair round 3* and the lane inventory. Only its inode and
  SHA-256 cells were filled in by the controller.
- **Whole-lib parallel test selection:** *resolved in repair round 3.* It was intermittently red
  for two reasons. First, a success-path fixture deadline of 10 s is now a named 60 s constant. Second, and more
  important, a macOS production defect: `EPERM` from group signalling during exit, now fixed. After the fix, 8 of 8
  full-parallel `bridge-core` lib runs are green on the macOS host.
- **A5 ACL-granted-write row:** present in the control but excluded on this lane, reported at
  runtime as `A5 ACL-granted-write row excluded: applied=false, effective write still denied
  (root=true)`. `setfacl`/`getfacl` are not installed in this image; as uid 0 the profile compares
  mode bits and cannot observe an ACL that is not reflected in the group bits; and as an ordinary
  user the fixture profile trusts the effective uid, so POSIX owner-entry precedence makes an
  owner-granting ACL unconstructible on Linux. The row does construct on macOS, where NFSv4 ACLs are
  evaluated ahead of the mode bits, so it is enabled there by the same capability probe.
- **A5 `faccessat` write denial:** unexercised as root, and therefore the matrix's one non-flipping
  row (row 13 above, with its mechanism). The branch itself is exercised on the macOS host lane, where
  round 3's `a5_write_probe_errno_classification_admits_only_proven_denials` discriminates its errno
  classification fail-first.
- **A18 domain tags:** the `git-object-dir` / `git-alternate-dir` domain separation is asserted
  structurally but is **not** discriminated by an inequality mutation. Evidence keeps primary and
  alternate digests in distinct positional roles, so reversing the tags still yields unequal
  evidence for both the reordering and the role-swap rows. Recorded as an evidence limit rather than
  claimed as a flipped control; carried in round 4's deferred ledger as **S3**.
- **Workspace-wide gates:** see *Verification commands and results*.
- **A11a–A11c, A11d, A8/A8b bypass arms:** these rows' mutations are the `#[cfg(test)]` request-level
  seams, asserted inside the control in the same run; rows 32–43 additionally mutate the production
  guard itself.

## Verification and lane limits

The required macOS route was executed by the controller on the macOS host in repair round 3; it was
not executed in this container, which has no macOS toolchain. The native Linux ext4 route has not been
executed in any round and remains the one outstanding lane. The ext4 classifier is reusable by 2B2,
rejects overlayfs/fuse/nonmatching mountinfo, and since round 4 also rejects malformed `mountinfo`
shapes rather than admitting them. The process cleanup
control uses a POSIX-shell `kill -0` probe rather than `/proc`, so it executes on macOS too. Repair
round 2 runs the gates through cargo with the pinned toolchain, so round 1's hand-reconstructed rustc
unit no longer stands behind any claim here; the workspace-wide gates listed above are still not run,
and the controller's host gates remain the authority for them.

## Honest limits

- **HL1:** object-store route paths (`GIT_OBJECT_DIRECTORY`, `GIT_ALTERNATE_OBJECT_DIRECTORIES`) and any variable Git
  requires to be absolute are resolved by Git by path. A same-user swap between the recheck and the lookup is detected
  by the caller's post-exit check, not prevented. For 2B2, the content-addressed closure proof keeps a substituted
  store from yielding a wrong pack.
- **HL2:** an identical substitution of a newly created directory (empty, same owner and mode) between `mkdirat` and
  `openat` is undetectable. The substitute is necessarily a directory inside the retained parent at open time.
- **HL3:** root or another privileged user replacing and restoring the admitted route entirely within one child's
  window. A persisting replacement is detected after the child by the identity and digest recheck.
