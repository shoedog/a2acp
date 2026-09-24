---
task-type: implement
---
# ADR-0041 Slice 2B2a — descriptor seam and hardened Git runner

**Status:** review candidate, revision 1; planning/documentation only. Implementation is not authorized by this file.
On 2026-09-24 the owner chose to split this child out of Slice 2B2 (2B2 task revision 6, `cf93c7e4`, §16 option 1).
2B2a delivers the filesystem seam and the Git runner that 2B2's exporter builds on. It carries requirements already
shaped by the pre-review audit and three independent 2B2 review rounds (§12).

**Exact predecessor:** `5e431f4f2dd6f77c66d64fa28dc48054f396edf9` (`origin/main`, PR #105 merge).

**Sequence:** 2B2a → 2B2 (exporter and closure proof) → 2B2b (production non-Git framing) → 2B3 (restore). 2B2
implementation starts only after 2B2a is approved and merged, and uses 2B2a's API unchanged.

## 1. Outcome

2B2a adds a crate-private, reusable, capsule-agnostic effect seam to `bridge-core`, and nothing else:

1. four `PinnedDirectoryV1` methods for descriptor-relative child creation, opening, and command rooting (§3);
2. a `custody_git` runner that spawns a closed set of Git subcommands from an admitted Git binary route, in a closed
   environment, rooted at a retained directory descriptor, with bounded I/O, a required wall-clock deadline, and
   binary and caller-identity rechecks around every child (§4).

The child is complete only when:

1. every item is `pub(crate)` and production-unwired: no caller outside tests exists until 2B2;
2. the runner cannot execute an unadmitted binary, a free-form argv, an inherited environment variable, or an
   unrooted child;
3. every control in §5 discriminates its single guard on host macOS and on an admitted native Linux ext4 lane;
4. every failure is typed and leaves no effect outside the caller-provided pinned root.

2B2a has no knowledge of manifests, capsules, seals, capture capabilities, budgets beyond per-child I/O bounds, or
source repositories beyond an opaque object-store route (§4.2).

## 2. Authority, effects, and threat model

**Allowed effects:**

- descriptor-relative creation of directories and regular files beneath a caller-provided `PinnedDirectoryV1`;
- local Git subprocesses rooted at such a directory.

Tests use disposable fixtures only. No source repository, operator state, or network is touched.

**Excluded:** everything 2B2 owns, including export, closure proof, the scratch-wide ledger, sealing, the capture
capability, and publication. Also excluded are CLI, config, store, or scheduler wiring; dependencies or features; and
changes to `Cargo.lock`.

**Threat model** (owner ruling, 2026-09-24; also in 2B2 task §2.1):

- **In scope:** stale or retargeted paths, accidental concurrent mutation by the bridge, the operator, or tools,
  operator error, and crashes. Defenses are descriptor custody, pre- and post-effect identity rechecks that detect
  drift and refuse, and the portable preventions below.
- **Out of scope:** a hostile process running as the same user, or a more privileged one, that deliberately races a
  check-to-use window. Detection code for such windows stays mandatory. The residual windows are honest limits
  HL1–HL3 (§10), which the implementation handoff must restate verbatim.

## 3. `fs_custody` seam — four `PinnedDirectoryV1` methods

`PinnedDirectoryV1` keeps its directory `File` private and only opens existing regular children; `fs_custody` has no
descriptor-relative directory creation. 2B2a adds exactly these four `pub(crate)` methods:

- `create_new_child_directory(name, label) -> PinnedDirectoryV1`
  - takes a validated single-component name and runs `mkdirat` at mode `0700`, refusing any existing entry;
  - opens the new entry with `open_child_no_follow` plus `O_DIRECTORY`, and records identity from the opened
    descriptor, requiring it to be a directory;
  - detects substitution with three guards:
    - **emptiness:** enumerating a duplicated descriptor yields no entry other than `.` and `..`;
    - **owner and mode:** the directory is owned by the effective user with mode `0700`;
    - **parent-entry identity:** a no-follow `fstatat` of the name in the parent returns the opened identity;
  - requires no literal link count, because link counts are a convention some filesystems do not follow;
  - treats any mismatch as a typed refusal, then syncs the parent. An identical substitution is HL2.
- `open_existing_child_directory(name, label) -> PinnedDirectoryV1`: the same no-follow open and identity capture,
  without creating anything.
- `create_new_regular_child(name, label) -> File`: wraps the existing `create_new_regular_child_at` on the retained
  descriptor, creating an owner-private file that refuses an existing entry and follows no link.
- `root_command(&self, command: &mut std::process::Command) -> Result<(), FsCustodyError>`
  - duplicates the retained descriptor with `try_clone`, which keeps `O_CLOEXEC`;
  - moves that **owned** duplicate into a `pre_exec` closure that calls `fchdir` on it and fails the spawn if
    `fchdir` fails;
  - because the command owns the descriptor, dropping the pin or reusing a descriptor number cannot misroot a later
    spawn;
  - `fchdir` is async-signal-safe, and `O_CLOEXEC` closes the duplicate only at a successful `exec`, after `fchdir`,
    so the child does not inherit it;
  - a duplication failure is returned as a typed error.

The `unsafe` code authorized is exactly three audited boundaries, each with a safety comment:

- descriptor-relative `mkdirat`;
- `fdopendir`/`readdir` on a duplicated descriptor for the emptiness guard;
- the `pre_exec` `fchdir` closure.

Any other `fs_custody` change is a stop condition.

## 4. `custody_git` runner

### 4.1 Admitted Git route

The runner resolves one admitted Git binary once, in two phases.

1. **Bootstrap discovery.**
   - A candidate absolute path is metadata-checked against the route rule. On macOS the candidate may be the `xcrun`
     trampoline `/usr/bin/git`.
   - The candidate runs exactly one discovery child, `<candidate> --exec-path`. It runs with the §4.2 environment,
     rooted at an empty caller-provided probe directory, with no inherited descriptor and bounded stdout.
2. **Admission.**
   - The runner derives `<exec-path>/git`, canonicalizes it, and applies the route rule to that target.
   - The target must be a fixed point: its own `--exec-path` must map back to the same file identity.
   - The **target** is admitted, never the candidate. This handles the macOS trampoline, which execs a binary chosen
     by developer-directory state, and it is uniform on Linux.
   - The candidate and the target are both recorded as evidence. Only the admitted target runs the version probe or
     any other child.

**Route rule:**

- the final component is a regular file, not a symlink;
- the file and every ancestor directory of its canonical path are owned by the **trusted owner** (uid 0 in
  production) **and** deny write access to the executing user, checked with `faccessat(..., W_OK, AT_EACCESS)` so
  groups and ACLs count;
- in production, the runner itself does not run as uid 0.

A user-owned route, such as a Homebrew symlink, and any writable component are refused with a typed route refusal.

The rule is a pure function over collected metadata plus the trusted-owner uid. The production constructor fixes the
trusted owner at uid 0. A `#[cfg(test)]` constructor takes the test user's uid, so fixture binaries can be admitted:
mode-`0555` files in mode-`0555` directories owned by that user. With that uid, the executing user owns the fixture, so
the test-only constructor skips the uid-0 check.

**Rechecks:** the admitted route's file identity (device, inode, size, and modification time) and SHA-256 are
recorded at admission. They are rechecked **before every spawn and after every child exits**.

- A pre-spawn mismatch refuses before the child starts.
- A post-exit mismatch returns a typed `BinaryDrift` outcome, which the caller must treat as incomplete.
- A privileged replace-and-restore entirely within one child's window is HL3.

**Minimum version:** the runner runs `version` on the admitted target and requires a minimum version pinned at
implementation time by a probe. The probe must show that version honors every flag and variable in §4.2. The runner
parks below that version.

### 4.2 Closed environment, flags, and rooting

Each child starts from `env_clear()` followed by exactly this allowlist:

- `GIT_OPTIONAL_LOCKS=0`, `GIT_NO_REPLACE_OBJECTS=1`, `GIT_NO_LAZY_FETCH=1`, `GIT_TERMINAL_PROMPT=0`;
- `GIT_CONFIG_NOSYSTEM=1`, `GIT_CONFIG_GLOBAL=/dev/null`;
- `HOME`, `XDG_CONFIG_HOME`, and `GIT_DIR` as **relative** names validated as single components below the root;
- when the caller supplies a `GitObjectStoreRouteV1`, `GIT_OBJECT_DIRECTORY` and `GIT_ALTERNATE_OBJECT_DIRECTORIES`
  from that route. These are absolute by necessity (HL1). The route is an opaque, caller-built value; 2B2a does not
  interpret source repositories;
- a fixed minimal `PATH` and locale, recorded in evidence.

Nothing else is inherited, including `GIT_CONFIG_PARAMETERS`, `GIT_CONFIG_COUNT`, askpass, SSH, `GIT_EXEC_PATH`,
work-tree, index, namespace, and ceiling variables.

Every child carries:

```text
git --no-optional-locks --no-replace-objects --no-lazy-fetch \
  -c core.hooksPath=/dev/null -c core.fsmonitor=false -c protocol.allow=never \
  <subcommand> ...
```

Every child is rooted with `root_command` at the caller's retained directory. Every internal path is relative to that
root, so swapping an ancestor pathname cannot redirect the child's writes. The implementation-time probe identifies
any variable Git requires to be absolute. Such a variable is HL1, detected by the post-exit rechecks.

### 4.3 Closed subcommand set

The runner never accepts a caller argv string. The `GitCommandV1` enum has only the variants below, and each maps to
one fixed argv. Relative names are validated single components or fixed relative paths below the root.

| Variant | Fixed subcommand |
|---|---|
| `ExecPath` | `--exec-path` (discovery only) |
| `Version` | `version` |
| `InitBare { dir, object_format }` | `init --bare --template= --object-format=<sha1\|sha256> <dir>` |
| `CatFileBatchCheck` | `cat-file --batch-check` |
| `PackObjectsStdout` | `pack-objects --stdout` |
| `IndexPackStrictStdin` | `index-pack --strict --stdin` |
| `VerifyPack { index }` | `verify-pack -v <index>` |
| `CatFileAllObjects` | `cat-file --batch-all-objects --batch-check=%(objectname) %(objecttype)` |
| `RevListMissingPrint` | `rev-list --objects --no-object-names --missing=print --stdin` |
| `FsckStrict` | `fsck --strict --full --no-reflogs --no-dangling --no-progress` |

### 4.4 Spawn protocol

Each spawn takes the command, the rooted directory, an optional object-store route, and a bounded stdin source. It
also takes stdout and stderr byte caps and a **required** wall-clock deadline, plus caller pre-spawn and post-exit
check callbacks. It runs in this order:

1. binary pre-spawn recheck, then the caller's pre-spawn check;
2. rooted spawn;
3. a stdin writer, a stdout reader, and a stderr reader run concurrently, so a full pipe cannot deadlock. Stdout
   beyond its cap by one byte kills the child and returns a typed cap refusal;
4. at the deadline, the child is killed and reaped, and a typed timeout is returned;
5. wait for exit;
6. binary post-exit recheck, then the caller's post-exit check;
7. return a typed result.

The result carries the bounded stdout (or its streamed digest when the caller consumes stdout incrementally), the
bounded stderr, the exit status, and an evidence record. The evidence record holds the argv, the candidate and
admitted routes, the Git version, the environment keys, the exit status, and the lengths and SHA-256 of each stream.
Exit status alone is never success evidence; the caller parses output.

## 5. Required RED and behavioral controls

Capture structural RED on `5e431f4f` before production code. Each control names the single guard it mutates and must
fail on that compiling targeted mutation, then pass after restoration. Outer layers are bypassed by fixture so the
mutation is the only thing between the fixture and a wrong result. A mutation that does not flip its control is
inadmissible until the fixture is repaired.

| # | Mutation / fixture | Guard under test | Expected result | Origin |
|---|---|---|---|---|
| A1 | pinned root's pathname replaced before nested directory creation and before regular-child creation | §3 directory and regular-child methods | creation continues only under the retained descriptor; nothing appears in the replacement | 2B2 #23 |
| A2a | a hook between `mkdirat` and open exchanges the new child for a non-empty directory | emptiness guard | typed refusal; no pin | 2B2 #27a |
| A2b | a hook between `mkdirat` and open exchanges the new child for an empty directory with another mode; the owner arm is covered by an injected-metadata unit test | owner and mode guard | typed refusal; no pin | 2B2 #27b |
| A2c | a hook after open and before `fstatat` renames the opened directory aside and installs an identical one | parent-entry identity guard | typed refusal; with the guard removed, a pin to the detached directory is returned | 2B2 #27c |
| A2d | an injected-metadata seam reports link count 1 for a genuinely empty created directory (positive) | no literal link-count requirement | success | 2B2 #27d |
| A3 | a hook after the last parent-side recheck swaps the root's pathname for a replacement before spawn; the fixture binary writes a relative marker | `root_command` rooting and relative paths | the marker lands only under the retained root; the replacement is unchanged; the caller's post-exit check sees the drift | 2B2 #26 |
| A4 | a `root_command` pin is dropped and its descriptor number reused before spawn | owned duplicate descriptor | the child is still rooted at the original directory | 2B2 R3 S1 |
| A5 | route-rule table: trusted-owner file with a mode-`0777` component; untrusted-owner mode-`0555` file; symlink final component; ACL-granted write; production constructor as uid 0; standard root-owned Linux `/usr/bin/git` (positive, pure metadata) | §4.1 route rule | typed route refusal; the positive row is admitted | 2B2 #28 |
| A5a | a fake trampoline whose `--exec-path` names a distinct admitted target, and a target that is not a fixed point | §4.1 two-phase admission | only discovery runs the candidate; functional argv uses the target; a non-fixed-point target refuses | 2B2 #28a |
| A5b | an admitted route replaced at its path after admission, before the next spawn | §4.1 pre-spawn recheck | refusal before the next spawn | 2B2 #22 |
| A5c | an admitted fixture route replaced by an `exit 0` binary after the pre-spawn recheck, inside a deterministic hook | §4.1 post-exit recheck | typed `BinaryDrift` | 2B2 #28b |
| A6 | a fixture binary dumps its environment and cwd | §4.2 allowlist | the environment equals exactly the allowlist and the cwd identity equals the root; adding one inherited variable turns it red | new |
| A7 | golden argv per `GitCommandV1` variant | §4.3 fixed argv table | exact match; adding or removing one argument turns it red | new |
| A8 | a fixture binary writes cap+1 stdout bytes, and separately exactly cap bytes | §4.4 stdout bound | typed cap refusal with the child killed; the exact-cap run succeeds | new |
| A9 | a fixture binary sleeps past the deadline | §4.4 deadline | typed timeout; child killed and reaped; no zombie | new |
| A10 | a fixture binary that floods stderr while blocking on stdin | §4.4 concurrent I/O | completes or times out as typed, with no deadlock | new |
| A11a | lazy-fetch fixture (below); only `--no-lazy-fetch` active | the flag | object reported missing; no fetch | 2B2 #8a |
| A11b | lazy-fetch fixture; only `GIT_NO_LAZY_FETCH=1` active | the environment variable | object reported missing; no fetch | 2B2 #8b |
| A11c | lazy-fetch fixture; only `protocol.allow=never` active | the protocol override | object reported missing; no fetch | 2B2 #8c |
| A11d | object-store route's repository carries promisor config; the three flags bypassed; the mutation copies that config into the `InitBare` directory | `InitBare` writes no foreign config | object reported missing; no fetch | 2B2 #8d |
| A12 | admission-classifier table: `0xEF53` with a matching-device `ext4` entry (admit); matching `ext2` or `ext3`; wrong device; duplicate or ambiguous matches; malformed `mountinfo` | §7 ext4 admission | named exclusion for every non-admit row; deleting the fstype comparison turns the `ext3` row red | 2B2 #29 |

**Lazy-fetch fixture (A11a–A11c):**

- The fixture object store lacks one object, and `CatFileBatchCheck` is run for it.
- A test-only promisor remote is injected into the `InitBare` directory: `extensions.partialClone` plus
  `remote.p.promisor=true` and `remote.p.url=file://<fixture>`, where the fixture holds the object. That injection is
  the fixture-level bypass of A11d.
- Each row leaves exactly one guard active. With it active, the object stays missing. With it removed, the lazy fetch
  succeeds, observed as the object appearing in the fixture store.
- Every arm starts from a fresh disposable store copied from one immutable template. The harness asserts the object
  is absent before each arm and the template hash is unchanged afterwards. Row order is randomized, and contamination
  is an inadmissible precondition.
- Each row is enabled on a platform only after a probe proves the flip. Otherwise it is a named exclusion.

A compile error unrelated to the control, a bad fixture, an invalid flag, zero selected tests, a setup failure, or a
child refusal before the behavior under test is inadmissible evidence.

## 6. Owned paths

- `crates/bridge-core/src/fs_custody.rs` — only the four §3 methods and their in-module tests (A1–A4);
- `crates/bridge-core/src/custody_git.rs` — new `pub(crate)` runner: route admission, environment, `GitCommandV1`,
  the spawn protocol, and evidence;
- `crates/bridge-core/src/custody_git_tests.rs` — in-crate tests via `#[path = "custody_git_tests.rs"] mod tests;`
  (A5–A12), including fixture binaries created at test time and the reusable `#[cfg(test)]` ext4 admission
  classifier that 2B2 will import;
- `crates/bridge-core/src/lib.rs` — module declaration only;
- `docs/superpowers/reviews/2026-09-24-adr0041-slice2b2a-implementation-handoff.md` — evidence and lane handoff;
- this task, the 2B2 task, the Slice 2B planning handoff, and the reliability roadmap — status reconciliation only.

Do not add dependencies or features, and do not edit `Cargo.lock`. `libc` is already a dependency.

## 7. Verification

Run directly and report exact totals:

```text
cargo test --locked --offline -p bridge-core --lib fs_custody
cargo test --locked --offline -p bridge-core --lib custody_git
cargo test --locked --offline -p bridge-core
cargo test --locked --offline --workspace --all-targets
cargo test --locked --offline --workspace
cargo clippy --locked --offline --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
git diff --check
cargo deny check
cargo run --locked --offline -p a2a-bridge -- validate --repo-hygiene
```

Run on host macOS and on a native Linux ext4 lane (the GitHub Actions ubuntu runner). The ext4 lane is admitted only
when `fstatfs` on the retained fixture descriptor reports `0xEF53` **and** the `/proc/self/mountinfo` entry whose
`major:minor` equals the descriptor's `st_dev` reports `ext4`. Anything else is a named exclusion. Overlayfs does not
substitute. Record the Git version and admitted route in each lane. Before attributing any failure, run `5e431f4f` in
the same environment. An unrunnable gate is reported with its mechanism and is never counted as green.

## 8. Review gate

Declare a **two-admitted-round** hard-read-only spec review cap for this task, then a separate two-round cap for the
implementation review. Report WRONG before SMELL. Repair a closed enumerable population on the same artifact. Park an
open-class population or an exhausted nonconverging cap. Never restart a partially reviewed artifact.

## 9. Stop conditions

Stop for spec or design review if any of these occurs:

- an `fs_custody` change is needed beyond the four §3 methods;
- the runner needs a free-form argv or an inherited environment variable;
- a route cannot be admitted on standard macOS Command Line Tools or Ubuntu Git;
- the minimum Git version cannot honor §4.2;
- rooting cannot be applied to a spawn;
- a dependency or feature is required;
- the diff escapes §6.

## 10. Honest limits

These must be restated verbatim in the handoff.

- **HL1:** object-store route paths (`GIT_OBJECT_DIRECTORY`, `GIT_ALTERNATE_OBJECT_DIRECTORIES`) and any variable Git
  requires to be absolute are resolved by Git by path. A same-user swap between the recheck and the lookup is detected
  by the caller's post-exit check, not prevented. For 2B2, the content-addressed closure proof keeps a substituted
  store from yielding a wrong pack.
- **HL2:** an identical substitution of a newly created directory (empty, same owner and mode) between `mkdirat` and
  `openat` is undetectable. The substitute is necessarily a directory inside the retained parent at open time.
- **HL3:** root or another privileged user replacing and restoring the admitted route entirely within one child's
  window. A persisting replacement is detected after the child.

## 11. Next action

Independently review this task, round 1 of 2. Only a separately authorized, approved task may begin implementation.
Approval does not authorize push, merge, cleanup, or running-operator mutation.

## 12. Provenance

The following sections are carried from the 2B2 task, revision 6 (`cf93c7e4`):

| 2B2a section | 2B2 rev 6 source |
|---|---|
| §3 | §6 `fs_custody` methods |
| §4.1 | §4.1 route and rechecks |
| §4.2 | §4.1 environment, flags, and rooting |
| §2 | §2.1 threat model |
| §5 carried rows | the §7 rows named in the Origin column |
| §7 ext4 rule | §9 ext4 rule |
| §10 | §15 honest limits |

Those requirements were shaped by the pre-review audit and 2B2 review rounds 1–3. The records are
`docs/superpowers/reviews/2026-09-24-adr0041-slice2b2-spec-review-round{1,2,3}.md`, and the relevant findings are:

- round 1 W1 and S8;
- round 2 W1, W3, W4, W5, and W6;
- round 3 W1–W6 and S1–S2.

New in 2B2a, because the runner now stands alone:

- the closed `GitCommandV1` set;
- the required deadline and concurrent-I/O protocol;
- the caller pre-spawn and post-exit check callbacks;
- the trusted-owner test constructor;
- controls A4 and A6–A10.
