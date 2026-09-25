# Handoff — ADR-0041 Slice 2B2a descriptor seam and Git runner

**Implementation base:** `90c5c247d96861f932a043f3b5e0ce7b119c6ac6` (task base
`5e431f4f2dd6f77c66d64fa28dc48054f396edf9`). **Repair round 1** works from the retained candidate
`535cfb82e1f65481788b033ed3196ca58a21b70e` and closes the five BLOCKER WRONGs of the attempt-3
implementation review. **Repair round 2** closes the controller's two host-verification findings on
that round-1 tree: H1, the macOS portability errors, and H2, the incomplete mutation matrix. The
delivered diff is restricted to the paths named by the task §6. The runner remains crate-private and
production-unwired. This handoff records evidence only; it claims no review approval.

## Scope delivered

`PinnedDirectoryV1` now creates/opens children through its retained descriptor, checks new-directory
emptiness, effective owner/mode, and parent-entry identity, and roots commands using an owned
duplicate descriptor. `custody_git` admits a caller-pinned binary, records descriptor facts in a
digest-mismatch refusal, rechecks every route component before and after execution, uses a closed
Git command set and environment allowlist, terminates a process group on cap/deadline, and supports
file-descriptor stdin and root-confined stdout streaming. Captured stdout remains a separately
selected bounded mode for version parsing and small controls.

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
exits 0 (see *Verification commands and results*). **The macOS compile itself is still a named
exclusion** — this container has no macOS toolchain, so the fix is verified by construction and by
the Linux gates, not by a macOS build.

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

**Mutation matrix.** Round 2's 56-row matrix ran in the Linux container before this round's macOS changes. It was
not re-run on this host. The two new production fixes each carry the single-row fail-first proofs above.

## Pre-code lane inventory

| Lane | Route | inode / links | SHA-256 | Version | Status |
|---|---|---:|---|---|---|
| Implement container (uid 0) | `/opt/git/bin/git` | `82093288` / 148 | `d0f9299bdb98a5f3229f6ca1a545f1dc1abeffb2f7156d0550b58a79d7efe3c4` | `git version 2.54.0` | measured; root-owned 0755 with root-owned 0755 ancestors |
| macOS Command Line Tools | `/Library/Developer/CommandLineTools/usr/bin/git` | — | — | — | not measured |
| GitHub Actions Ubuntu | `/usr/bin/git` | — | — | — | not measured |

The container uses the test-system profile because it runs as uid 0; production admission refuses
uid 0. The route's `st_dev` is the container's overlay device (`0:134` in `/proc/self/mountinfo`) and
is instance-specific, so it is not recorded as a stable fact. The macOS and native-ext4 lanes are
named exclusions, not green evidence.

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
# test result: ok. 20 passed; 0 failed; 0 ignored; 740 filtered out

cargo test --locked --offline -p bridge-core --lib fs_custody
# test result: ok. 117 passed; 0 failed; 0 ignored; 643 filtered out

cargo test --locked --offline -p bridge-core --lib
# test result: ok. 760 passed; 0 failed; 0 ignored; 0 filtered out   (4 of 5 consecutive runs)
```

`custody_git` is 16 → 20 tests: the four new controls are the round-1 regressions for WRONG 1–3.
`fs_custody` stays at 117 and the whole lib at 760 (756 + 4). The clippy run covers `bridge-core`'s
lib target (production `cfg`) and its lib-test target, which is where the H1 `#[allow]` lives.

**The whole-lib selection is flaky, and it is reported rather than rounded up.** Running all 760
tests in parallel intermittently fails one `custody_git` control with `CustodyGitError::Timeout`.
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

56 targeted mutations were executed against the delivered tree, **in one complete foreground run**.
Each row applies one mutation to one named guard, rebuilds the `bridge-core` lib test harness, runs
the row's control selection (mutated result), restores the source byte-exactly from a pre-matrix
snapshot, proves the restoration with `git diff --no-index` and a SHA-256 comparison against that
snapshot, rebuilds, and runs the control again (restored result). A row counts only if the **named**
control turns red and the restoration turns it green; a mutation that does not compile is recorded as
inadmissible evidence and never as a flip.

- driver: `python3 .git/a2a-bridge/mutation/mutate.py`
- full log: `.git/a2a-bridge/mutation/mutation-log.txt`
- pre-matrix snapshot: `.git/a2a-bridge/mutation/snapshot/`

The run is `HEAD 535cfb82` plus the unstaged repair, started `2026-09-25T15:16:40+0000`, finished
`2026-09-25T15:38:48+0000`, 1311.5s wall clock. Every verdict below is copied from that log.

Result: **rows executed 56; flipped their named control 55; did not flip 1 (row 13); inadmissible 0.**

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

**Row 13 is the single non-flipping row and is inadmissible on this lane, not claimed as evidence.**
Its mutation deletes the `faccessat(W_OK, AT_EACCESS)` write-denial refusal in `deny_effective_write`.
The mechanism is that this container runs as uid 0, and `deny_effective_write` branches on
`effective_uid == 0` *before* `faccessat` is reached, returning on the mode-bit comparison
(`WriteCheckV1::ModeBitsAsRoot`). The deleted code is therefore unreachable on this lane, so its
named control `a5_route_rule_table_and_final_symlink_guard_are_discriminating` stayed green — as did
all 137 tests in both control selections, on three independent confirming runs. The row is admissible
only on a lane whose executing user is not root; the mode-bit branch that *is* reachable here is
separately discriminated by row 11.

After the matrix, all three owned sources equal the pre-matrix snapshot byte for byte, and the whole
working-tree diff is unchanged (`git diff` SHA-256
`65b04b699f1475b2b40988b2fa53b8fa6982117b42a9e012ba188251b68a0ab0` before and after).

Two mutations were **inadmissible on their first run in round 1** and the fixtures were repaired
rather than the results claimed: A2a (masked by the owner/mode guard) and the A17 production row
(masked by the temp-directory ancestor audit). Both flip in the run above, as rows 3 and 50 record.
No row was inadmissible in this run: all 56 mutations were compile-checked in a pre-flight pass
before the authoritative run, and all 56 compiled.

## Named exclusions

- **macOS host lane and native Linux ext4 lane:** not executed. The container's fixture filesystem is
  `overlayfs`, so the task §7 ext4 admission rule (`0xEF53` plus a matching `ext4` `mountinfo` entry)
  is not satisfied; overlayfs does not substitute. The H1 portability fix is therefore verified by
  construction and by the Linux gates, not by a macOS compile.
- **Whole-lib parallel test selection:** intermittently red on the delivered tree with no mutation
  applied, from a control fixture's fixed 10-second wall-clock deadline rather than any production
  guard. Measured rates, mechanism, and the affected controls are in *Verification commands and
  results*; the 56-row matrix judges every row on the two stable control selections instead. Recorded
  as a known defect, not repaired in this round.
- **A5 ACL-granted-write row:** present in the control but excluded on this lane, reported at
  runtime as `A5 ACL-granted-write row excluded: applied=false, effective write still denied
  (root=true)`. `setfacl`/`getfacl` are not installed in this image; as uid 0 the profile compares
  mode bits and cannot observe an ACL that is not reflected in the group bits; and as an ordinary
  user the fixture profile trusts the effective uid, so POSIX owner-entry precedence makes an
  owner-granting ACL unconstructible on Linux. The row does construct on macOS, where NFSv4 ACLs are
  evaluated ahead of the mode bits, so it is enabled there by the same capability probe.
- **A5 `faccessat` write denial:** unexercised as root, and therefore the matrix's one non-flipping
  row (row 13 above, with its mechanism).
- **A18 domain tags:** the `git-object-dir` / `git-alternate-dir` domain separation is asserted
  structurally but is **not** discriminated by an inequality mutation. Evidence keeps primary and
  alternate digests in distinct positional roles, so reversing the tags still yields unequal
  evidence for both the reordering and the role-swap rows. Recorded as an evidence limit rather than
  claimed as a flipped control.
- **Workspace-wide gates:** see *Verification commands and results*.
- **A11a–A11c, A11d, A8/A8b bypass arms:** these rows' mutations are the `#[cfg(test)]` request-level
  seams, asserted inside the control in the same run; rows 32–43 additionally mutate the production
  guard itself.

## Verification and lane limits

The required macOS route and native Linux ext4 route were not executed in this container. The ext4
classifier is reusable by 2B2 and rejects overlayfs/fuse/nonmatching mountinfo. The process cleanup
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
